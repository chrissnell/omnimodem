# RTL-SDR Phase D — hardening (auto-reconnect, overrun, tune arbitration, docs)

**Date:** 2026-07-10
**Issue:** GRA-313
**Design source of truth:** `docs/design/2026-07-06-rtl-tcp-sdr-input-design.md` (Phase D row).

## Goal

Make the `rtl_tcp` SDR input production-ready — survive server restarts and network
blips, behave predictably under consumer lag, define multi-client tune arbitration,
and ship end-user documentation. This is the final Phase-A-feature phase; Phases A/B/C
are merged (`main`).

Four deliverables:

1. **Auto-reconnect** — the capture thread today connects once and `break`s the loop
   on any read/send error, silently killing RX. Wrap connect → header → read loop in a
   reconnect-with-backoff supervisor (mirror `cpal_backend.rs`'s `backoff_wait` /
   `BACKOFF_RESET_AFTER`). On reconnect, re-apply **all** hardware params from
   `SdrControl` (rate, ppm, direct-sampling, bias-tee, gain mode+level, center) and
   preserve the in-thread NCO offset / squelch / demod-mode, so a transient blip never
   tears down the channel or loses the operator's tune.
2. **Overrun handling** — the delivery channel is bounded (`CHUNK_QUEUE_DEPTH`).
   Today `tx.send` **blocks** the socket read when the consumer lags. Switch to a
   non-blocking **drop-oldest** policy so capture keeps reading, and surface a
   dropped-chunk count (counter + rate-limited log) so overruns are observable.
3. **Multi-client tune arbitration** — `SdrControl` is shared, so concurrent gRPC
   clients can stomp each other. Adopt and **document** last-writer-wins, and confirm
   the `SdrState` event is broadcast on every mutating RPC so all clients reconcile.
4. **Docs** — an end-user handbook page for the `rtl_tcp` input, plus grpc-api /
   running / wiki updates so future agents find it.

## Constraints

- `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0`. **Never run `cargo fmt`.**
- Keep edits localized to `crates/omnimodem/src/audio/rtlsdr.rs` (shares the file
  with B/C — both merged, so conflict surface is only against `main`).
- No stubs. Docs ship in this PR.
- TDD: fake-`rtl_tcp`-server tests (drop-mid-stream → reconnect+resume+re-apply; slow
  consumer → keeps running + drops counted).
- `cargo test` and `cargo clippy --all-targets -- -D warnings` clean.

## Decisions

- **Reconnect supervisor lives in the capture thread**, not `open_capture`. The
  initial connect stays synchronous in `open_capture` so a bad address / non-rtl_tcp
  server still fails fast and caps publish immediately; the established connection is
  handed to the thread as the first supervisor iteration, and every later iteration
  reconnects. A shared `connect_and_handshake` helper (connect → header → publish caps
  → `send_initial_commands`) is reused for both.
- **All hardware state is re-derived from `SdrControl` on every (re)connect.** The
  control cell is the single source of truth and persists across drops, so the
  operator's tune/gain/ppm/bias-tee/direct-sampling/rate survive. `send_initial_commands`
  already sends the full set; the NCO offset, squelch, and demod mode are in-thread and
  the RX chain is rebuilt from current control at the top of each connection.
- **Shutdown across reconnects** via a shared `Arc<Mutex<Option<TcpStream>>>` slot.
  The stop hook sets the flag and shuts down whatever socket is current, unblocking a
  parked read regardless of which connection is live.
- **Overrun = drop-oldest of the pending backlog, never block.** A small
  `VecDeque<AudioChunk>` staging buffer in the capture thread absorbs what the bounded
  consumer channel won't take via `try_send`; when the backlog exceeds
  `CHUNK_QUEUE_DEPTH` the oldest queued chunk is dropped and a counter bumped. This
  keeps the socket read live (latency stays bounded) and prefers fresh audio for a
  live modem. Rationale for oldest-over-newest: decoding seconds-stale audio is
  pointless; dropping the oldest keeps the modem near real time.
- **Dropped-chunk counter on `SdrControl`** (`AtomicU64`), so it is shared, readable by
  tests, survives reconnects, and leaves a path to per-channel metrics later. A
  rate-limited `tracing::warn` surfaces it in the daemon log.
- **Multi-client arbitration = last-writer-wins, made observable.** Atomic setters mean
  the most recent `SetSdrTune`/`SetSdrGain`/`ConfigureSdr` wins; there is no lock or
  owner lease. Every mutating RPC already calls `emit_sdr_state`, so all clients
  (including late joiners, via snapshot-on-subscribe) converge on the effective state.
  A stronger advisory-owner guarantee is **not** warranted for a homelab single-operator
  tool; documenting LWW + guaranteed reconciliation is the right scope. **Known gap
  (Phase-C follow-up, out of scope here):** `SdrState` still omits `ppm`/`bias_tee`/
  `direct_sampling`, so those specific fields do not round-trip to other clients — the
  design assigns adding `ppm` to `SdrState` to Phase C; flagged for a follow-up.

## Steps (bite-sized, TDD)

### Step 1 — dropped-chunk counter on `SdrControl`
- Add `dropped_chunks: AtomicU64` to `SdrControlInner` (default 0); `dropped_chunks()`
  reader and `incr_dropped() -> u64` (returns new total). Not part of `generation()`.
- **Test:** default 0; `incr_dropped()` returns increasing totals visible through a clone.

### Step 2 — `connect_and_handshake` helper
- Extract connect → `read_exact` header → `parse_header` → `set_caps` →
  `send_initial_commands` (using `control.capture_rate()`) → `try_clone` cmd socket
  into `fn connect_and_handshake(addr, &SdrControl) -> Result<(TcpStream, TcpStream)>`.
- Rewire `open_capture` to call it for the initial (fail-fast) connect.
- **Test:** covered by existing `capture_reads_header_and_delivers_audio`,
  `capture_publishes_tuner_caps`, `bad_header_fails_capture` (no behavior change).

### Step 3 — reconnect supervisor + drop-oldest delivery in the capture thread
- Restructure the thread body into a `'supervisor` loop:
  - iteration 0 uses the handed-in socket pair; later iterations
    `connect_and_handshake`, backing off via a local `backoff_wait` on failure.
  - store a `try_clone` of the live read socket into the shared shutdown slot each
    iteration; reset `carry`; rebuild the RX chain + `seen_gen`/`cur_rate`/`cur_mode`
    from current control (so params that changed during downtime are honored).
  - inner read loop as today; on `Ok(0)` / read error / send-side gone (server),
    break to reconnect; on consumer `Disconnected`, return (terminal).
  - reset backoff after `BACKOFF_RESET_AFTER` of stable streaming; `backoff_wait`
    before reconnecting; honor `stop` at every checkpoint.
- Replace blocking `tx.send` with a `deliver` step: push to a `VecDeque` backlog,
  drain via `try_send`, drop-oldest past `CHUNK_QUEUE_DEPTH` (bump `incr_dropped`,
  rate-limited `tracing::warn`).
- Local `REBUILD_BACKOFF` / `BACKOFF_RESET_AFTER` / `backoff_wait` mirrored from
  `cpal_backend.rs` (that module is `#[cfg(not(test))]`, so constants are re-declared).
- **Test — reconnect:** a fake server that serves one connection (header + IQ) then
  drops, then accepts a second (header + IQ), recording the second connection's
  commands. Assert: audio arrives before the drop, audio resumes after reconnect, and
  the second connection received a `CenterFreq` (the operator's tune) and manual
  `TunerGain` — proving re-apply.
- **Test — overrun:** a fast-streaming fake server; leave the consumer un-drained
  briefly, assert `control.dropped_chunks() > 0` (capture kept producing instead of
  blocking), then drain and assert audio still flows.

### Step 4 — docs
- `docs/sdr-rtl-tcp.md` — end-user handbook: remote/local dongle setup
  (`rtl_tcp -a 0.0.0.0`), binding `rtltcp:host:port`, tuning + waterfall, demod modes,
  gain/squelch/ppm/bias-tee/direct-sampling, and the Phase-D hardening behavior
  (reconnect, overrun/drop-oldest, multi-client LWW).
- `docs/grpc-api.md` — note reconnect + drop-oldest + LWW under the SDR section.
- `docs/running.md` — link the handbook page from the config-file section.
- `docs/wiki/README.md` + `docs/wiki/code-map.md` + `docs/wiki/audio-devices-ptt.md`
  — index the page and the reconnect/overrun behavior so agents find the code fast.

## Verification

- `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0 cargo test -p omnimodem`
- `CARGO_TARGET_DIR=/tmp/omni-target CARGO_INCREMENTAL=0 cargo clippy --all-targets -- -D warnings`
- Paste output into the PR. Commit as `chrissnell`, meaningful branch name.
