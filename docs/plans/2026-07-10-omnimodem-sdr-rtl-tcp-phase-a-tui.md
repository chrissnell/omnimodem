# RTL-SDR (`rtl_tcp`) — Phase A, Plan 4: TUI tuning view + waterfall cursor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development
> (or executing-plans) to implement this plan task-by-task. Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** Give the operator a first-class SDR tuning surface in the Go/Bubble Tea
client (`clients/omnimodem-tui`): tune the RTL-SDR by step or direct entry, watch
the wideband RF waterfall, place the demod channel by eye with a cursor overlay,
and drive gain / ppm / demod-mode / squelch — closing the user-facing side of
Phase A. Depends on the merged Plan 2 (`RtlTcpBackend` + RF-referenced spectrum)
and Plan 3 (`SetSdrTune` / `SetSdrGain` / `ConfigureSdr` / `GetSdrCaps` +
`SdrState` event) work.

**Architecture:** The daemon is authoritative for tune/gain/demod/squelch state; it
broadcasts an `SdrState` event whenever any of those change. The TUI folds that
event into the per-channel `chanLive` (exactly like `rxDbfs`/`pttKeyed`), so the
readout survives view rebuilds and stays in sync across clients. The new
`sdrView` holds only *editing* state (current step size, direct-entry buffer, gain
table cursor, pending ppm) and reads the authoritative fields from `chanLive` each
render. Every control maps to one of the Plan-3 RPCs. The RF waterfall reuses the
existing `SpectrumFrame` renderer verbatim; a thin cursor-column overlay marks the
demod frequency.

**Conventions:**
- Build/test from `clients/omnimodem-tui`: `go build ./...`, `go test ./...`.
- Generated stubs live in `internal/pb`; regenerate with `bash gen.sh` (needs
  `protoc` + `protoc-gen-go` + `protoc-gen-go-grpc`).
- Model-level tests use the `client.Fake`; no live daemon.

---

### Task 1: Regenerate the Go stubs for the Plan-3 proto

Plan 3 added the SDR RPCs/messages to `proto/omnimodem.proto`; the TUI's generated
`internal/pb` is still pre-Plan-3. Regenerate so `SetSdrTune`, `SetSdrGain`,
`ConfigureSdr`, `GetSdrCaps`, `SdrState`, and `DemodMode` are available in Go.

- [ ] `bash clients/omnimodem-tui/gen.sh`
- [ ] Confirm `go build ./...` still green (no callers yet).

---

### Task 2: Client methods for the four SDR RPCs

Add `SetSdrTune`, `SetSdrGain`, `ConfigureSdr`, `GetSdrCaps` to the `ModemClient`
interface, the `grpcClient` impl, and the `client.Fake` — mirroring `SetAudioGain`
/ `ConfigureSpectrum`. The Fake records requests and returns canned responses so
model tests can assert what the UI sent.

**Files:** `internal/client/client.go`, `internal/client/fake.go`.

---

### Task 3: Fold `SdrState` into live channel state

Add SDR fields to `chanLive` (`center/offset/freq Hz`, `gainAuto`, `gainDb`,
`demod`, `squelchDb`, `haveSdrState`) and a `case *pb.Event_SdrState` in
`Model.applyEvent` that ensures the channel and overwrites those fields
(latest-wins, LOSSY — matching the other event folds).

**Files:** `internal/app/model.go`.

---

### Task 4: Waterfall cursor overlay

Extend `waterfall` with a cursor-column helper and cursor-aware render, without
changing the existing `render`/`spectrumLine` call sites:
- `cursorColumn(width int, freqHz float64) int` — map Hz → bin via
  `freqStart`/`freqStep`, then bin → display column; `-1` when out of the shown
  span or no axis yet. **Pure, unit-tested.**
- `spectrumLineCursor(bins []byte, width, cursorCol int) string` — `spectrumLine`
  plus a marker glyph at `cursorCol`; `spectrumLine` becomes a `cursorCol = -1`
  wrapper.
- `renderCursor(width, rows, cursorCol int) string` — `render` plus the marker;
  `render` becomes a wrapper.

**Files:** `internal/app/waterfall.go`, `internal/app/waterfall_test.go`.

---

### Task 5: RF spectrum enable + tune helpers (commands)

- `enableRFSpectrumCmd(c, ch, binCount)` — `ConfigureSpectrum{enable:true, bin_count}`
  with **no** passband clamp (the daemon drives the RF axis from the source type;
  a `freq_hi_hz` clamp would zoom to a few kHz).
- Command wrappers + typed msgs for the four RPCs: `sdrTuneMsg`, `sdrGainMsg`,
  `sdrConfigMsg`, `sdrCapsMsg` (and a `getSdrCapsCmd`).

**Files:** `internal/app/waterfall.go` (spectrum), `internal/app/sdr.go` (new:
commands + msgs + pure tuning/gain helpers).

---

### Task 6: Pure tuning + gain-table helpers (unit-tested)

In `internal/app/sdr.go`, small pure functions the view and tests share:
- `sdrSteps = []float64{1000, 5000, 12500, 25000}` and `stepLabel`.
- `clampFreq(freq, min, max float64) float64`.
- `nearestGainIdx(gains []float32, db float32) int` and
  `stepIdx(idx, dir, n int) int` (clamp helper for gain + step cycling).

**Files:** `internal/app/sdr.go`, `internal/app/sdr_test.go`.

---

### Task 7: `sdrView` (the tuning screen)

`internal/app/view_sdr.go` implementing the `View` interface (mirror
`view_operate.go`):
- **Render:** large RF frequency readout (effective demod freq from `chanLive`);
  the RF waterfall with the demod cursor overlay + axis; a control bar with step
  size, gain (auto/manual + dB), ppm, demod mode, squelch threshold + open/closed
  indicator, and a live signal meter (from `rxDbfs`).
- **Update / keys:** `←`/`→` tune ∓ step; `s` cycle step size; `g` toggle gain
  auto/manual; `[`/`]` step manual gain through the caps table; `m` cycle demod
  mode; `,`/`.` squelch ∓; `-`/`+` ppm ∓; `f` begin direct entry (digits + `.`,
  `enter` applies MHz, `esc` cancels the edit); `esc` disables the RF spectrum and
  pops back. Each control fires exactly one RPC command.
- **Entry:** on push, request caps (`getSdrCapsCmd`) and enable the RF waterfall.

**Files:** `internal/app/view_sdr.go`.

---

### Task 8: Route SDR-bound channels into the SDR view

In `view_channels.go`, on `o`/`enter`, branch on the selected channel's device id:
`rtltcp:` → `newSdrView` + `enableRFSpectrumCmd`; else the existing operate path.
Add an `isSDRDevice(id string) bool` helper.

**Files:** `internal/app/view_channels.go`.

---

### Task 9: Model-level tests

Bubble Tea tests driving `sdrView` through a `Fake` (no daemon):
- step tune emits `SetSdrTune` with `freq ± step`, clamped to caps;
- step-size cycle changes the applied delta;
- direct entry parses MHz and emits `SetSdrTune`;
- gain toggle / manual step emit `SetSdrGain`;
- demod cycle + squelch/ppm emit `ConfigureSdr`;
- cursor↔frequency mapping (`cursorColumn`) lands the marker in the right column;
- routing: an `rtltcp:` channel opens `sdrView`, a soundcard channel opens
  `operateView`.

**Files:** `internal/app/view_sdr_test.go`.

---

### Task 10: Verify + PR

- [ ] `go build ./...` and `go test ./...` green in `clients/omnimodem-tui`; paste output.
- [ ] Commit as `chrissnell`, meaningful branch, open a PR.
- [ ] Update the LLM wiki / docs so future agents find the SDR view quickly.

Phase A is "done" only once APRS decodes end-to-end off a real/remote dongle with
working click/step-tune in the TUI; that end-to-end confirmation is the parent
issue's gate.
