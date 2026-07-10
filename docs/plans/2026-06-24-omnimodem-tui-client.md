# Omnimodem TUI Client Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Bubble Tea terminal client for `omnimodem` from `docs/design/2026-06-23-omnimodem-tui-client.md` — connect, configure audio/PTT, pick a digital mode, and transmit — with a status bar, macro bar, activity pane, a real waterfall, and a hard TX abort.

**Architecture:** A single Elm-style root `Model` routes between screens (Connect → Dashboard → Config → Operate). All daemon I/O goes through a `ModemClient` interface (real gRPC impl + a fake for tests), so every `Update` is unit-testable without a live daemon. The `SubscribeEvents` stream is bridged into Bubble Tea via a goroutine → buffered channel → re-issued `tea.Cmd`. Mutating RPCs run as `tea.Cmd`s returning typed result msgs; state of record is `GetState` snapshot + event deltas.

**Tech Stack:** Go 1.26, [Bubble Tea](https://github.com/charmbracelet/bubbletea) + Bubbles + Lipgloss, gRPC-Go, `protoc-gen-go`/`protoc-gen-go-grpc`. UDS transport (mTLS deferred).

**Scope:** MVP = design §10 build-order phases 1–4 (Connect, Config, Operate-ragchew, Operate-FT8), including the waterfall (now backed by `SpectrumFrame`, #24) and real per-mode params (`mode_params`, #26). **RX decode (`RxFrame` rendering) is deferred** — its panes are scaffolded but inert. This plan is large; each Phase ends at working, testable software, so it can be executed and reviewed phase-by-phase.

**Resolved since the design was written:**
- Waterfall feed exists: `SpectrumFrame` event + `ConfigureSpectrum` RPC (#24). Build the real strip.
- Mode params exist: `ConfigureChannelRequest.mode_params` typed oneof (#26). Configure CW/RTTY/PSK31/Olivia params for real.
- TX keying contract confirmed: the **daemon auto-keys PTT** for the duration of a `Transmit` (emits `PttKeyed true/false`). The TUI never keys PTT around a transmit; `KeyPtt` is only the manual config-screen test.

---

## Build env

```bash
# Go is at /usr/local/go/bin/go (1.26). Proto plugins are NOT preinstalled:
go install google.golang.org/protobuf/cmd/protoc-gen-go@latest
go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@latest
export PATH="$PATH:$(go env GOPATH)/bin"   # so protoc finds the plugins
# protoc itself: install as in the omnimodem-build-env memory if absent.
```
CI: extend `.github/workflows/ci.yml` with a Go job (Task 16) so `go test ./...` runs on every PR — same async-verification model as the Rust gate. (Editing the workflow file requires pushing over SSH, not the HTTPS PAT.)

## File Structure

Go module rooted at `clients/omnimodem-tui/` (module path `github.com/chrissnell/omnimodem/clients/omnimodem-tui`).

- `clients/omnimodem-tui/go.mod` — module + deps.
- `clients/omnimodem-tui/gen.sh` — protoc codegen (no edits to the shared `.proto`; uses `M`-mapping).
- `clients/omnimodem-tui/internal/pb/` — generated `omnimodem.v1` bindings (`omnimodemv1` package).
- `clients/omnimodem-tui/internal/client/client.go` — `ModemClient` interface + gRPC impl + UDS dial.
- `clients/omnimodem-tui/internal/client/fake.go` — in-test fake `ModemClient`.
- `clients/omnimodem-tui/internal/app/msgs.go` — typed `tea.Msg`s + RPC `tea.Cmd`s.
- `clients/omnimodem-tui/internal/app/events.go` — `SubscribeEvents` → channel → `waitForEvent` bridge.
- `clients/omnimodem-tui/internal/app/model.go` — root `Model`, screen enum, `Update`/`View` router.
- `clients/omnimodem-tui/internal/app/status.go` — status bar.
- `clients/omnimodem-tui/internal/app/connect.go` — connection screen.
- `clients/omnimodem-tui/internal/app/dashboard.go` — channel dashboard.
- `clients/omnimodem-tui/internal/app/config.go` — configuration screen.
- `clients/omnimodem-tui/internal/app/modes.go` — mode table + `mode_params` builder.
- `clients/omnimodem-tui/internal/app/operate.go` — operate screen shell (status + activity + macro chrome).
- `clients/omnimodem-tui/internal/app/tx.go` — TX flow state machine.
- `clients/omnimodem-tui/internal/app/macros.go` — macro bar + expansion.
- `clients/omnimodem-tui/internal/app/waterfall.go` — `SpectrumFrame` → ramp strip.
- `clients/omnimodem-tui/internal/app/ft8.go` — FT8 auto-sequence ladder.
- `clients/omnimodem-tui/internal/app/qsolog.go` — append-only QSO log.
- `clients/omnimodem-tui/cmd/omnimodem-tui/main.go` — flags + wiring + `tea.Program`.

---

## Phase 0 — Module, codegen, gRPC client, connection

### Task 1: Go module + proto codegen

**Files:**
- Create: `clients/omnimodem-tui/go.mod`
- Create: `clients/omnimodem-tui/gen.sh`
- Create: `clients/omnimodem-tui/internal/pb/.gitkeep`

- [ ] **Step 1: Init the module**

```bash
mkdir -p clients/omnimodem-tui/internal/pb
cd clients/omnimodem-tui
go mod init github.com/chrissnell/omnimodem/clients/omnimodem-tui
go get google.golang.org/grpc@latest google.golang.org/protobuf@latest
go get github.com/charmbracelet/bubbletea@latest github.com/charmbracelet/bubbles@latest github.com/charmbracelet/lipgloss@latest
```

- [ ] **Step 2: Write the codegen script** `clients/omnimodem-tui/gen.sh` (generates into `internal/pb` without touching the shared proto, via `M`-mapping):

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
PROTO_DIR=../../proto
MAP="Momnimodem.proto=github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
protoc -I "$PROTO_DIR" \
  --go_out=internal/pb --go_opt=paths=import --go_opt="$MAP" \
  --go-grpc_out=internal/pb --go-grpc_opt=paths=import --go-grpc_opt="$MAP" \
  "$PROTO_DIR/omnimodem.proto"
# Flatten the import-path nesting protoc creates into internal/pb directly.
find internal/pb -name '*.pb.go' -path '*clients*' -exec sh -c 'mv "$1" internal/pb/' _ {} \; 2>/dev/null || true
find internal/pb -type d -empty -delete 2>/dev/null || true
echo "generated: $(ls internal/pb/*.pb.go)"
```

- [ ] **Step 3: Generate and verify it compiles**

Run:
```bash
chmod +x gen.sh && ./gen.sh
go build ./internal/pb/...
```
Expected: `internal/pb/omnimodem.pb.go` and `omnimodem_grpc.pb.go` exist (package `omnimodemv1`), build clean. The package exposes `ModemControlClient`, `ConfigureChannelRequest`, `ModeParams`, `SpectrumFrame`, `Event`, etc.

- [ ] **Step 4: Commit**

```bash
git add clients/omnimodem-tui/go.mod clients/omnimodem-tui/go.sum clients/omnimodem-tui/gen.sh clients/omnimodem-tui/internal/pb
git commit -m "tui: scaffold Go module + generated omnimodem.v1 bindings"
```

### Task 2: `ModemClient` interface + UDS dial

**Files:**
- Create: `clients/omnimodem-tui/internal/client/client.go`
- Test: `clients/omnimodem-tui/internal/client/client_test.go`

- [ ] **Step 1: Write the failing test** (the address builder is the pure, testable seam):

```go
package client

import "testing"

func TestDialTarget(t *testing.T) {
	cases := map[string]string{
		"/run/omnimodem.sock": "unix:///run/omnimodem.sock",
		"127.0.0.1:9000":      "dns:///127.0.0.1:9000",
	}
	for in, want := range cases {
		if got := dialTarget(in); got != want {
			t.Fatalf("dialTarget(%q) = %q, want %q", in, got, want)
		}
	}
}
```

- [ ] **Step 2: Run it, expect FAIL** (`undefined: dialTarget`)

Run: `go test ./internal/client/` — Expected: build error / FAIL.

- [ ] **Step 3: Implement** `client.go`:

```go
// Package client wraps the generated omnimodem.v1 gRPC client behind a narrow
// interface so the UI can be driven by a fake in tests.
package client

import (
	"context"
	"strings"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

// ModemClient is the subset of ModemControl the TUI uses. The fake in tests and
// the gRPC impl below both satisfy it.
type ModemClient interface {
	GetState(context.Context) (*pb.ModemState, error)
	ListDevices(context.Context) ([]*pb.DeviceInfo, error)
	ConfigureChannel(context.Context, *pb.ConfigureChannelRequest) error
	ConfigureAudio(context.Context, *pb.ConfigureAudioRequest) (*pb.ConfigureAudioResponse, error)
	ConfigurePtt(context.Context, *pb.ConfigurePttRequest) error
	KeyPtt(context.Context, uint32, bool) error
	SetAudioGain(context.Context, *pb.SetAudioGainRequest) error
	ConfigureSpectrum(context.Context, *pb.ConfigureSpectrumRequest) (*pb.ConfigureSpectrumResponse, error)
	SuggestUdevRule(context.Context, string) (*pb.SuggestUdevRuleResponse, error)
	AcquireTxLease(context.Context, uint32) (*pb.TxLeaseResponse, error)
	ReleaseTxLease(context.Context, uint32) error
	Transmit(context.Context, uint32, []byte) (uint64, error)
	Subscribe(context.Context) (pb.ModemControl_SubscribeEventsClient, error)
	Close() error
}

// dialTarget maps a user address to a gRPC target: an absolute path is a UDS,
// anything else is treated as host:port.
func dialTarget(addr string) string {
	if strings.HasPrefix(addr, "/") {
		return "unix://" + addr
	}
	return "dns:///" + addr
}

type grpcClient struct {
	conn *grpc.ClientConn
	c    pb.ModemControlClient
}

// Dial connects to omnimodem over UDS (path) or TCP (host:port). mTLS is out of
// scope for the MVP; local UDS relies on socket-mode + SO_PEERCRED authz.
func Dial(addr string) (ModemClient, error) {
	conn, err := grpc.NewClient(dialTarget(addr), grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		return nil, err
	}
	return &grpcClient{conn: conn, c: pb.NewModemControlClient(conn)}, nil
}

func (g *grpcClient) GetState(ctx context.Context) (*pb.ModemState, error) {
	return g.c.GetState(ctx, &pb.GetStateRequest{})
}
func (g *grpcClient) ListDevices(ctx context.Context) ([]*pb.DeviceInfo, error) {
	r, err := g.c.ListDevices(ctx, &pb.ListDevicesRequest{})
	if err != nil {
		return nil, err
	}
	return r.GetDevices(), nil
}
func (g *grpcClient) ConfigureChannel(ctx context.Context, req *pb.ConfigureChannelRequest) error {
	_, err := g.c.ConfigureChannel(ctx, req)
	return err
}
func (g *grpcClient) ConfigureAudio(ctx context.Context, req *pb.ConfigureAudioRequest) (*pb.ConfigureAudioResponse, error) {
	return g.c.ConfigureAudio(ctx, req)
}
func (g *grpcClient) ConfigurePtt(ctx context.Context, req *pb.ConfigurePttRequest) error {
	_, err := g.c.ConfigurePtt(ctx, req)
	return err
}
func (g *grpcClient) KeyPtt(ctx context.Context, ch uint32, keyed bool) error {
	_, err := g.c.KeyPtt(ctx, &pb.KeyPttRequest{Channel: ch, Keyed: keyed})
	return err
}
func (g *grpcClient) SetAudioGain(ctx context.Context, req *pb.SetAudioGainRequest) error {
	_, err := g.c.SetAudioGain(ctx, req)
	return err
}
func (g *grpcClient) ConfigureSpectrum(ctx context.Context, req *pb.ConfigureSpectrumRequest) (*pb.ConfigureSpectrumResponse, error) {
	return g.c.ConfigureSpectrum(ctx, req)
}
func (g *grpcClient) SuggestUdevRule(ctx context.Context, dev string) (*pb.SuggestUdevRuleResponse, error) {
	return g.c.SuggestUdevRule(ctx, &pb.SuggestUdevRuleRequest{DeviceId: dev})
}
func (g *grpcClient) AcquireTxLease(ctx context.Context, ch uint32) (*pb.TxLeaseResponse, error) {
	return g.c.AcquireTxLease(ctx, &pb.TxLeaseRequest{Channel: ch})
}
func (g *grpcClient) ReleaseTxLease(ctx context.Context, ch uint32) error {
	_, err := g.c.ReleaseTxLease(ctx, &pb.TxLeaseRequest{Channel: ch})
	return err
}
func (g *grpcClient) Transmit(ctx context.Context, ch uint32, payload []byte) (uint64, error) {
	r, err := g.c.Transmit(ctx, &pb.TransmitRequest{Channel: ch, Payload: payload})
	if err != nil {
		return 0, err
	}
	return r.GetTransmitId(), nil
}
func (g *grpcClient) Subscribe(ctx context.Context) (pb.ModemControl_SubscribeEventsClient, error) {
	return g.c.SubscribeEvents(ctx, &pb.SubscribeRequest{})
}
func (g *grpcClient) Close() error { return g.conn.Close() }
```

- [ ] **Step 4: Run it, expect PASS**

Run: `go test ./internal/client/` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/client/
git commit -m "tui: ModemClient interface + UDS gRPC dial"
```

### Task 3: Fake client for tests

**Files:**
- Create: `clients/omnimodem-tui/internal/client/fake.go`

- [ ] **Step 1: Implement the fake** (records calls; returns canned data; satisfies `ModemClient`):

```go
package client

import (
	"context"
	"sync"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// Fake is an in-memory ModemClient for tests. Set the *Resp fields to control
// returns; inspect the *Calls slices to assert what the UI sent.
type Fake struct {
	mu             sync.Mutex
	State          *pb.ModemState
	Devices        []*pb.DeviceInfo
	AudioResp      *pb.ConfigureAudioResponse
	SpectrumResp   *pb.ConfigureSpectrumResponse
	LeaseResp      *pb.TxLeaseResponse
	NextTransmitID uint64
	Err            error // if set, every RPC returns it

	ChannelCalls  []*pb.ConfigureChannelRequest
	AudioCalls    []*pb.ConfigureAudioRequest
	PttCalls      []*pb.ConfigurePttRequest
	GainCalls     []*pb.SetAudioGainRequest
	SpectrumCalls []*pb.ConfigureSpectrumRequest
	TransmitCalls []*pb.TransmitRequest
	LeaseAcquired []uint32
	LeaseReleased []uint32
}

func (f *Fake) GetState(context.Context) (*pb.ModemState, error) {
	if f.Err != nil {
		return nil, f.Err
	}
	if f.State == nil {
		return &pb.ModemState{}, nil
	}
	return f.State, nil
}
func (f *Fake) ListDevices(context.Context) ([]*pb.DeviceInfo, error) { return f.Devices, f.Err }
func (f *Fake) ConfigureChannel(_ context.Context, r *pb.ConfigureChannelRequest) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.ChannelCalls = append(f.ChannelCalls, r)
	return f.Err
}
func (f *Fake) ConfigureAudio(_ context.Context, r *pb.ConfigureAudioRequest) (*pb.ConfigureAudioResponse, error) {
	f.AudioCalls = append(f.AudioCalls, r)
	if f.AudioResp == nil {
		f.AudioResp = &pb.ConfigureAudioResponse{ActualSampleRate: 48000}
	}
	return f.AudioResp, f.Err
}
func (f *Fake) ConfigurePtt(_ context.Context, r *pb.ConfigurePttRequest) error {
	f.PttCalls = append(f.PttCalls, r)
	return f.Err
}
func (f *Fake) KeyPtt(context.Context, uint32, bool) error { return f.Err }
func (f *Fake) SetAudioGain(_ context.Context, r *pb.SetAudioGainRequest) error {
	f.GainCalls = append(f.GainCalls, r)
	return f.Err
}
func (f *Fake) ConfigureSpectrum(_ context.Context, r *pb.ConfigureSpectrumRequest) (*pb.ConfigureSpectrumResponse, error) {
	f.SpectrumCalls = append(f.SpectrumCalls, r)
	if f.SpectrumResp == nil {
		f.SpectrumResp = &pb.ConfigureSpectrumResponse{BinCount: 64, FreqStepHz: 50, RateHz: 15}
	}
	return f.SpectrumResp, f.Err
}
func (f *Fake) SuggestUdevRule(context.Context, string) (*pb.SuggestUdevRuleResponse, error) {
	return &pb.SuggestUdevRuleResponse{Rule: "RULE", Instructions: "put it here"}, f.Err
}
func (f *Fake) AcquireTxLease(_ context.Context, ch uint32) (*pb.TxLeaseResponse, error) {
	f.LeaseAcquired = append(f.LeaseAcquired, ch)
	if f.LeaseResp == nil {
		f.LeaseResp = &pb.TxLeaseResponse{Granted: true}
	}
	return f.LeaseResp, f.Err
}
func (f *Fake) ReleaseTxLease(_ context.Context, ch uint32) error {
	f.LeaseReleased = append(f.LeaseReleased, ch)
	return f.Err
}
func (f *Fake) Transmit(_ context.Context, ch uint32, payload []byte) (uint64, error) {
	f.TransmitCalls = append(f.TransmitCalls, &pb.TransmitRequest{Channel: ch, Payload: payload})
	return f.NextTransmitID, f.Err
}
func (f *Fake) Subscribe(context.Context) (pb.ModemControl_SubscribeEventsClient, error) {
	return nil, f.Err // event bridge is tested via injected channel, not Subscribe
}
func (f *Fake) Close() error { return nil }
```

- [ ] **Step 2: Build**

Run: `go build ./internal/...` — Expected: clean (Fake satisfies ModemClient).

- [ ] **Step 3: Commit**

```bash
git add clients/omnimodem-tui/internal/client/fake.go
git commit -m "tui: in-memory fake ModemClient for tests"
```

---

## Phase 1 — App skeleton, event bridge, status bar, dashboard

### Task 4: Messages + RPC commands

**Files:**
- Create: `clients/omnimodem-tui/internal/app/msgs.go`
- Test: `clients/omnimodem-tui/internal/app/msgs_test.go`

- [ ] **Step 1: Write the failing test** (a command wraps an RPC and yields a typed msg):

```go
package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
)

func TestSnapshotCmd(t *testing.T) {
	f := &client.Fake{}
	msg := snapshotCmd(f)()
	if _, ok := msg.(snapshotMsg); !ok {
		t.Fatalf("got %T, want snapshotMsg", msg)
	}
}
```

- [ ] **Step 2: Run, expect FAIL** (`undefined: snapshotCmd`).

Run: `go test ./internal/app/ -run TestSnapshotCmd`

- [ ] **Step 3: Implement** `msgs.go`:

```go
package app

import (
	"context"
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

func rpcCtx() (context.Context, context.CancelFunc) {
	return context.WithTimeout(context.Background(), 5*time.Second)
}

// --- typed messages returned by commands / the event bridge ---
type snapshotMsg struct{ state *pb.ModemState }
type devicesMsg struct{ devices []*pb.DeviceInfo }
type rpcOKMsg struct{ what string }    // generic "mutating RPC succeeded"
type rpcErrMsg struct{ err error }     // any RPC failure
type audioCfgMsg struct{ resp *pb.ConfigureAudioResponse }
type spectrumCfgMsg struct{ resp *pb.ConfigureSpectrumResponse }
type leaseMsg struct{ resp *pb.TxLeaseResponse }
type transmitMsg struct{ id uint64 }
type eventMsg struct{ ev *pb.Event }
type eventClosedMsg struct{ err error }
type tickMsg time.Time

func snapshotCmd(c client.ModemClient) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		st, err := c.GetState(ctx)
		if err != nil {
			return rpcErrMsg{err}
		}
		return snapshotMsg{st}
	}
}

func devicesCmd(c client.ModemClient) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		d, err := c.ListDevices(ctx)
		if err != nil {
			return rpcErrMsg{err}
		}
		return devicesMsg{d}
	}
}

// tick drives the FT8 slot clock and the TX watchdog at 4 Hz.
func tickCmd() tea.Cmd {
	return tea.Tick(250*time.Millisecond, func(t time.Time) tea.Msg { return tickMsg(t) })
}
```

- [ ] **Step 4: Run, expect PASS**

Run: `go test ./internal/app/ -run TestSnapshotCmd` — Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/msgs.go clients/omnimodem-tui/internal/app/msgs_test.go
git commit -m "tui: typed messages + GetState/ListDevices/tick commands"
```

### Task 5: Event-stream bridge

**Files:**
- Create: `clients/omnimodem-tui/internal/app/events.go`
- Test: `clients/omnimodem-tui/internal/app/events_test.go`

- [ ] **Step 1: Write the failing test** (a queued event is delivered as `eventMsg`; a closed channel yields `eventClosedMsg`):

```go
package app

import (
	"testing"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func TestWaitForEvent(t *testing.T) {
	ch := make(chan *pb.Event, 1)
	ch <- &pb.Event{Kind: &pb.Event_PttState{PttState: &pb.PttState{Channel: 0, Keyed: true}}}
	if m, ok := waitForEvent(ch)().(eventMsg); !ok || m.ev.GetPttState() == nil {
		t.Fatalf("want eventMsg with PttState, got %T", waitForEvent(ch)())
	}
	close(ch)
	if _, ok := waitForEvent(ch)().(eventClosedMsg); !ok {
		t.Fatalf("closed channel should yield eventClosedMsg")
	}
}
```

- [ ] **Step 2: Run, expect FAIL** (`undefined: waitForEvent`).

- [ ] **Step 3: Implement** `events.go`:

```go
package app

import (
	"context"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

// startEventStream opens SubscribeEvents and pumps events into a buffered
// channel from a goroutine; the bridge between gRPC streaming and Bubble Tea's
// single-threaded Update loop. Returns the channel to feed waitForEvent.
func startEventStream(ctx context.Context, c client.ModemClient) <-chan *pb.Event {
	out := make(chan *pb.Event, 256)
	go func() {
		defer close(out)
		stream, err := c.Subscribe(ctx)
		if err != nil {
			return
		}
		for {
			ev, err := stream.Recv()
			if err != nil {
				return
			}
			select {
			case out <- ev:
			case <-ctx.Done():
				return
			}
		}
	}()
	return out
}

// waitForEvent blocks on the next event and wraps it as a tea.Msg. Re-issued
// from Update after each eventMsg so the stream keeps draining.
func waitForEvent(ch <-chan *pb.Event) tea.Cmd {
	return func() tea.Msg {
		ev, ok := <-ch
		if !ok {
			return eventClosedMsg{}
		}
		return eventMsg{ev}
	}
}
```

- [ ] **Step 4: Run, expect PASS** — `go test ./internal/app/ -run TestWaitForEvent`.

- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/events.go clients/omnimodem-tui/internal/app/events_test.go
git commit -m "tui: SubscribeEvents → channel → waitForEvent bridge"
```

### Task 6: Root model + live state from events

**Files:**
- Create: `clients/omnimodem-tui/internal/app/model.go`
- Test: `clients/omnimodem-tui/internal/app/model_test.go`

- [ ] **Step 1: Write the failing test** (events mutate the shared live state; LOSSY events keep latest):

```go
package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func TestApplyEventUpdatesLiveState(t *testing.T) {
	m := New(&client.Fake{}, "/run/omnimodem.sock")
	m.applyEvent(&pb.Event{Kind: &pb.Event_AudioLevel{AudioLevel: &pb.AudioLevel{Channel: 0, Dbfs: -18}}})
	m.applyEvent(&pb.Event{Kind: &pb.Event_AudioLevel{AudioLevel: &pb.AudioLevel{Channel: 0, Dbfs: -12}}})
	if got := m.live[0].rxDbfs; got != -12 {
		t.Fatalf("rxDbfs = %v, want -12 (latest wins)", got)
	}
	m.applyEvent(&pb.Event{Kind: &pb.Event_PttState{PttState: &pb.PttState{Channel: 0, Keyed: true}}})
	if !m.live[0].pttKeyed {
		t.Fatalf("pttKeyed should be true")
	}
}
```

- [ ] **Step 2: Run, expect FAIL** (`undefined: New`).

- [ ] **Step 3: Implement** `model.go`:

```go
package app

import (
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

type screen int

const (
	screenConnect screen = iota
	screenDashboard
	screenConfig
	screenOperate
)

// chanLive is the per-channel live state, fed by the event stream.
type chanLive struct {
	name      string
	mode      string
	deviceID  string
	running   bool
	rxDbfs    float32
	txDbfs    float32
	pttKeyed  bool
	clockSync bool
	clockOff  float64
}

// Model is the Elm root. Screens read/write the shared fields; only Update mutates.
type Model struct {
	c       client.ModemClient
	addr    string
	screen  screen
	width   int
	height  int
	err     string
	live    map[uint32]*chanLive
	sel     uint32 // selected channel
	events  <-chan *pb.Event
	connected bool

	// sub-screen state attached in later tasks (config, operate, ...)
	cfg *configState
	op  *operateState
}

func New(c client.ModemClient, addr string) *Model {
	return &Model{c: c, addr: addr, screen: screenConnect, live: map[uint32]*chanLive{}}
}

func (m *Model) Init() tea.Cmd { return connectCmd(m.c) }

// applyEvent folds one event into live state. LOSSY events overwrite (latest
// wins); the snapshot rebuilds the channel map.
func (m *Model) applyEvent(ev *pb.Event) {
	ensure := func(ch uint32) *chanLive {
		cl := m.live[ch]
		if cl == nil {
			cl = &chanLive{}
			m.live[ch] = cl
		}
		return cl
	}
	switch k := ev.Kind.(type) {
	case *pb.Event_Snapshot:
		m.live = map[uint32]*chanLive{}
		for _, ci := range k.Snapshot.GetChannels() {
			m.live[ci.GetChannel()] = &chanLive{
				name: ci.GetName(), mode: ci.GetMode(),
				deviceID: ci.GetDeviceId(), running: ci.GetRunning(),
			}
		}
	case *pb.Event_AudioLevel:
		ensure(k.AudioLevel.GetChannel()).rxDbfs = k.AudioLevel.GetDbfs()
	case *pb.Event_PttState:
		ensure(k.PttState.GetChannel()).pttKeyed = k.PttState.GetKeyed()
	case *pb.Event_ClockOffset:
		for _, cl := range m.live {
			cl.clockSync = k.ClockOffset.GetSynchronized()
			cl.clockOff = k.ClockOffset.GetOffsetS()
		}
	case *pb.Event_ChannelConfigured:
		ensure(k.ChannelConfigured.GetChannel())
	}
}
```

- [ ] **Step 4: Run, expect PASS** — `go test ./internal/app/ -run TestApplyEventUpdatesLiveState`. (`connectCmd` is added in Task 7; if the build fails on it now, do Task 7 Step 3 first, then return.)

- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/model.go clients/omnimodem-tui/internal/app/model_test.go
git commit -m "tui: root Model + applyEvent live-state folding"
```

### Task 7: Connect screen + Update router + status bar

**Files:**
- Create: `clients/omnimodem-tui/internal/app/connect.go`
- Create: `clients/omnimodem-tui/internal/app/status.go`
- Modify: `clients/omnimodem-tui/internal/app/model.go` (add `Update`/`View`)
- Test: `clients/omnimodem-tui/internal/app/connect_test.go`

- [ ] **Step 1: Write the failing test** (a successful connect transitions to the dashboard and starts the snapshot + event drain):

```go
package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func TestConnectedTransitionsToDashboard(t *testing.T) {
	m := New(&client.Fake{State: &pb.ModemState{}}, "/run/omnimodem.sock")
	next, _ := m.Update(connectedMsg{events: make(chan *pb.Event)})
	if next.(*Model).screen != screenDashboard {
		t.Fatalf("screen = %v, want dashboard", next.(*Model).screen)
	}
}
```

- [ ] **Step 2: Run, expect FAIL** (`undefined: connectedMsg`, `Update`).

- [ ] **Step 3: Implement** `connect.go`:

```go
package app

import (
	"context"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

type connectedMsg struct{ events <-chan *pb.Event }

// connectCmd opens the event stream (the act of subscribing also proves the
// daemon is reachable; the first event is the snapshot).
func connectCmd(c client.ModemClient) tea.Cmd {
	return func() tea.Msg {
		ch := startEventStream(context.Background(), c)
		return connectedMsg{events: ch}
	}
}
```

And add to `model.go`:

```go
func (m *Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		return m, nil
	case tea.KeyMsg:
		if msg.String() == "ctrl+c" {
			return m, tea.Quit
		}
		return m.updateScreen(msg)
	case connectedMsg:
		m.connected = true
		m.screen = screenDashboard
		m.events = msg.events
		return m, tea.Batch(snapshotCmd(m.c), waitForEvent(m.events), tickCmd())
	case eventMsg:
		m.applyEvent(msg.ev)
		return m, waitForEvent(m.events)
	case eventClosedMsg:
		m.connected = false
		m.err = "event stream closed"
		return m, nil
	case snapshotMsg:
		m.applyEvent(&pb.Event{Kind: &pb.Event_Snapshot{Snapshot: msg.state}})
		return m, nil
	case rpcErrMsg:
		m.err = msg.err.Error()
		return m, nil
	case tickMsg:
		return m.updateScreen(msg)
	}
	return m.updateScreen(msg)
}

// updateScreen dispatches to the active screen's handler (filled in per screen).
func (m *Model) updateScreen(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch m.screen {
	case screenConfig:
		return m.updateConfig(msg)
	case screenOperate:
		return m.updateOperate(msg)
	case screenDashboard:
		return m.updateDashboard(msg)
	}
	return m, nil
}

func (m *Model) View() string {
	body := ""
	switch m.screen {
	case screenConnect:
		body = "Connecting to " + m.addr + " …"
	case screenDashboard:
		body = m.viewDashboard()
	case screenConfig:
		body = m.viewConfig()
	case screenOperate:
		body = m.viewOperate()
	}
	return body + "\n" + m.statusBar()
}
```

Implement `status.go`:

```go
package app

import (
	"fmt"

	"github.com/charmbracelet/lipgloss"
)

var statusStyle = lipgloss.NewStyle().Reverse(true)

// statusBar renders the always-on bottom line: channel, mode, PTT, clock, levels.
func (m *Model) statusBar() string {
	cl := m.live[m.sel]
	if cl == nil {
		if m.err != "" {
			return statusStyle.Render(" omnimodem · " + m.err + " ")
		}
		return statusStyle.Render(" omnimodem · no channel ")
	}
	ptt := "▢"
	if cl.pttKeyed {
		ptt = "▣ TX"
	}
	clk := "clk ✗"
	if cl.clockSync {
		clk = "clk ✓"
	}
	s := fmt.Sprintf(" omnimodem · ch%d ▸ %s · PTT %s · %s · RX %.0f dBFS ",
		m.sel, orNone(cl.mode), ptt, clk, cl.rxDbfs)
	if m.err != "" {
		s += "· " + m.err + " "
	}
	return statusStyle.Render(s)
}

func orNone(s string) string {
	if s == "" {
		return "none"
	}
	return s
}
```

- [ ] **Step 4: Run, expect PASS** — `go test ./internal/app/ -run TestConnectedTransitionsToDashboard`.

- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/connect.go clients/omnimodem-tui/internal/app/status.go clients/omnimodem-tui/internal/app/model.go clients/omnimodem-tui/internal/app/connect_test.go
git commit -m "tui: connect screen, Update router, status bar"
```

### Task 8: Dashboard screen

**Files:**
- Create: `clients/omnimodem-tui/internal/app/dashboard.go`
- Test: `clients/omnimodem-tui/internal/app/dashboard_test.go`

- [ ] **Step 1: Write the failing test** (dashboard lists live channels; `c` opens Config, `o` opens Operate for the selection):

```go
package app

import (
	"strings"
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	tea "github.com/charmbracelet/bubbletea"
)

func TestDashboardListsChannelsAndRoutes(t *testing.T) {
	m := New(&client.Fake{}, "x")
	m.screen = screenDashboard
	m.live[0] = &chanLive{name: "vfo-a", mode: "psk31", rxDbfs: -20}
	if !strings.Contains(m.viewDashboard(), "vfo-a") {
		t.Fatalf("dashboard should list vfo-a")
	}
	next, _ := m.updateDashboard(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("o")})
	if next.(*Model).screen != screenOperate {
		t.Fatalf("'o' should open Operate")
	}
}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** `dashboard.go`:

```go
package app

import (
	"fmt"
	"sort"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
)

func (m *Model) updateDashboard(msg tea.Msg) (tea.Model, tea.Cmd) {
	key, ok := msg.(tea.KeyMsg)
	if !ok {
		return m, nil
	}
	switch key.String() {
	case "j", "down":
		m.sel = nextChannel(m.live, m.sel, +1)
	case "k", "up":
		m.sel = nextChannel(m.live, m.sel, -1)
	case "c":
		m.enterConfig()
		return m, devicesCmd(m.c)
	case "o":
		m.enterOperate()
		return m, nil
	}
	return m, nil
}

func (m *Model) viewDashboard() string {
	var b strings.Builder
	b.WriteString("Channels  (j/k select · c configure · o operate)\n\n")
	for _, ch := range sortedChannels(m.live) {
		cl := m.live[ch]
		cursor := "  "
		if ch == m.sel {
			cursor = "▸ "
		}
		b.WriteString(fmt.Sprintf("%sch%d  %-10s %-8s  RX %.0f dBFS\n",
			cursor, ch, orNone(cl.name), orNone(cl.mode), cl.rxDbfs))
	}
	if len(m.live) == 0 {
		b.WriteString("  (none — configure a channel)\n")
	}
	return b.String()
}

func sortedChannels(live map[uint32]*chanLive) []uint32 {
	out := make([]uint32, 0, len(live))
	for ch := range live {
		out = append(out, ch)
	}
	sort.Slice(out, func(i, j int) bool { return out[i] < out[j] })
	return out
}

func nextChannel(live map[uint32]*chanLive, cur uint32, dir int) uint32 {
	chs := sortedChannels(live)
	if len(chs) == 0 {
		return cur
	}
	idx := 0
	for i, c := range chs {
		if c == cur {
			idx = i
		}
	}
	idx = (idx + dir + len(chs)) % len(chs)
	return chs[idx]
}
```

(Stubs `enterConfig`/`enterOperate` land in Tasks 9 and 11; if building now, add empty methods `func (m *Model) enterConfig() {}` / `enterOperate()` and flesh them out there.)

- [ ] **Step 4: Run, expect PASS**.

- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/dashboard.go clients/omnimodem-tui/internal/app/dashboard_test.go
git commit -m "tui: dashboard screen with channel list + routing"
```

### Task 9: `main.go` entrypoint + smoke run

**Files:**
- Create: `clients/omnimodem-tui/cmd/omnimodem-tui/main.go`

- [ ] **Step 1: Implement** `main.go`:

```go
package main

import (
	"flag"
	"fmt"
	"os"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/app"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	tea "github.com/charmbracelet/bubbletea"
)

func main() {
	addr := flag.String("addr", defaultSock(), "omnimodem address: a UDS path or host:port")
	flag.Parse()

	c, err := client.Dial(*addr)
	if err != nil {
		fmt.Fprintln(os.Stderr, "dial:", err)
		os.Exit(1)
	}
	defer c.Close()

	if _, err := tea.NewProgram(app.New(c, *addr), tea.WithAltScreen()).Run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func defaultSock() string {
	if dir := os.Getenv("XDG_RUNTIME_DIR"); dir != "" {
		return dir + "/omnimodem.sock"
	}
	return "/run/omnimodem.sock"
}
```

- [ ] **Step 2: Build & vet**

Run: `go build ./... && go vet ./...` — Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add clients/omnimodem-tui/cmd/
git commit -m "tui: cmd entrypoint with --addr flag"
```

---

## Phase 2 — Configuration screen

### Task 10: Mode table + `mode_params` builder

**Files:**
- Create: `clients/omnimodem-tui/internal/app/modes.go`
- Test: `clients/omnimodem-tui/internal/app/modes_test.go`

- [ ] **Step 1: Write the failing test** (CW selection with params builds the right `ModeParams` oneof):

```go
package app

import (
	"testing"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func TestModeParamsForCW(t *testing.T) {
	mp := modeParamsFor("cw", map[string]float64{"wpm": 25, "tone": 600})
	cw := mp.GetCw()
	if cw == nil || cw.GetWpm() != 25 || cw.GetToneHz() != 600 {
		t.Fatalf("cw params = %+v, want wpm 25 tone 600", cw)
	}
	if modeParamsFor("ft8", nil) != nil {
		t.Fatalf("ft8 has no params → nil ModeParams")
	}
}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** `modes.go`:

```go
package app

import pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"

// modeParam describes one editable parameter for a mode (label + default).
type modeParam struct {
	key string
	def float64
}

// modeInfo: the modes the operate screen offers, their interaction shape, and
// their editable params. shape "chat" → ragchew surface; "ft8" → sequencer.
type modeInfo struct {
	label  string
	shape  string // "chat" | "ft8"
	params []modeParam
}

var modes = []modeInfo{
	{"psk31", "chat", []modeParam{{"center", 1000}}},
	{"rtty", "chat", []modeParam{{"baud", 45.45}, {"shift", 170}}},
	{"cw", "chat", []modeParam{{"wpm", 20}, {"tone", 700}}},
	{"afsk1200", "chat", nil},
	{"ft8", "ft8", nil},
}

func modeByLabel(label string) *modeInfo {
	for i := range modes {
		if modes[i].label == label {
			return &modes[i]
		}
	}
	return nil
}

// modeParamsFor builds the typed ModeParams oneof for a mode, or nil for modes
// without params (the daemon then uses the bare-label defaults).
func modeParamsFor(label string, vals map[string]float64) *pb.ModeParams {
	get := func(k string, d float64) float64 {
		if vals != nil {
			if v, ok := vals[k]; ok {
				return v
			}
		}
		return d
	}
	switch label {
	case "cw":
		return &pb.ModeParams{Params: &pb.ModeParams_Cw{Cw: &pb.CwParams{
			Wpm: uint32(get("wpm", 20)), ToneHz: float32(get("tone", 700)),
		}}}
	case "rtty":
		return &pb.ModeParams{Params: &pb.ModeParams_Rtty{Rtty: &pb.RttyParams{
			Baud: float32(get("baud", 45.45)), ShiftHz: float32(get("shift", 170)),
		}}}
	case "psk31":
		return &pb.ModeParams{Params: &pb.ModeParams_Psk31{Psk31: &pb.Psk31Params{
			CenterHz: float32(get("center", 1000)),
		}}}
	case "afsk1200":
		return &pb.ModeParams{Params: &pb.ModeParams_Afsk1200{Afsk1200: &pb.Afsk1200Params{Tx: true}}}
	default:
		return nil // ft8/ft4/jt65/jt9/wspr: no params
	}
}
```

- [ ] **Step 4: Run, expect PASS**.

- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/modes.go clients/omnimodem-tui/internal/app/modes_test.go
git commit -m "tui: mode table + typed mode_params builder"
```

### Task 11: Configuration screen (devices, channel, audio, PTT, gain, udev)

**Files:**
- Create: `clients/omnimodem-tui/internal/app/config.go`
- Test: `clients/omnimodem-tui/internal/app/config_test.go`

- [ ] **Step 1: Write the failing test** (applying config sends `ConfigureChannel` with `mode_params`, then `ConfigureAudio` with the chosen device):

```go
package app

import (
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func TestConfigApplyBuildsChannelAndAudio(t *testing.T) {
	f := &client.Fake{Devices: []*pb.DeviceInfo{{DeviceId: "usb:1:2:", HasCapture: true, HasPlayback: true}}}
	m := New(f, "x")
	m.sel = 0
	m.enterConfig()
	m.cfg.devices = f.Devices
	m.cfg.rxDev = "usb:1:2:"
	m.cfg.modeLabel = "cw"
	m.cfg.params = map[string]float64{"wpm": 25, "tone": 600}
	m.cfg.name = "vfo-a"

	cmd := m.applyConfig() // returns a tea.Cmd that runs ConfigureChannel
	cmd()                   // execute the channel step
	if len(f.ChannelCalls) != 1 {
		t.Fatalf("want 1 ConfigureChannel, got %d", len(f.ChannelCalls))
	}
	cc := f.ChannelCalls[0]
	if cc.GetMode() != "cw" || cc.GetModeParams().GetCw().GetWpm() != 25 {
		t.Fatalf("channel req wrong: %+v", cc)
	}
}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** `config.go` (form state + the apply pipeline; widget rendering uses `bubbles/list` for the device pickers — kept minimal here, real code):

```go
package app

import (
	"fmt"
	"strings"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

type configState struct {
	devices   []*pb.DeviceInfo
	name      string
	modeLabel string
	params    map[string]float64
	rxDev     string // capture device id
	txDev     string // optional playback device id ("" = same as rxDev)
	pttDev    string
	pttMethod pb.PttMethod
	focus     int // which field is focused
	udev      string
}

func (m *Model) enterConfig() {
	m.screen = screenConfig
	cl := m.live[m.sel]
	cs := &configState{name: "vfo-a", modeLabel: "psk31", params: map[string]float64{}, pttMethod: pb.PttMethod_PTT_METHOD_VOX}
	if cl != nil && cl.name != "" {
		cs.name = cl.name
	}
	m.cfg = cs
}

// applyConfig runs the bind pipeline as a sequence of RPC commands:
// ConfigureChannel → ConfigureAudio → ConfigurePtt. Each returns a typed msg;
// Update chains the next on success (see updateConfig).
func (m *Model) applyConfig() tea.Cmd {
	cs := m.cfg
	ch := m.sel
	req := &pb.ConfigureChannelRequest{
		Channel:    ch,
		Name:       cs.name,
		Mode:       cs.modeLabel,
		ModeParams: modeParamsFor(cs.modeLabel, cs.params),
	}
	c := m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.ConfigureChannel(ctx, req); err != nil {
			return rpcErrMsg{err}
		}
		return rpcOKMsg{what: "channel"}
	}
}

func (m *Model) configureAudioCmd() tea.Cmd {
	cs := m.cfg
	ch := m.sel
	c := m.c
	req := &pb.ConfigureAudioRequest{Channel: ch, DeviceId: cs.rxDev, SampleRate: 48000, TxDeviceId: cs.txDev}
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.ConfigureAudio(ctx, req)
		if err != nil {
			return rpcErrMsg{err}
		}
		return audioCfgMsg{resp}
	}
}

func (m *Model) configurePttCmd() tea.Cmd {
	cs := m.cfg
	ch := m.sel
	c := m.c
	req := &pb.ConfigurePttRequest{Channel: ch, DeviceId: cs.pttDev, Method: cs.pttMethod}
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.ConfigurePtt(ctx, req); err != nil {
			return rpcErrMsg{err}
		}
		return rpcOKMsg{what: "ptt"}
	}
}

func (m *Model) updateConfig(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case devicesMsg:
		m.cfg.devices = msg.devices
		return m, nil
	case rpcOKMsg:
		switch msg.what {
		case "channel":
			return m, m.configureAudioCmd() // chain audio after channel
		case "ptt":
			m.screen = screenDashboard // bind complete
			return m, snapshotCmd(m.c)
		}
	case audioCfgMsg:
		return m, m.configurePttCmd() // chain ptt after audio
	case tea.KeyMsg:
		switch msg.String() {
		case "esc":
			m.screen = screenDashboard
		case "enter":
			return m, m.applyConfig()
		case "tab":
			m.cfg.focus++
		}
	}
	return m, nil
}

func (m *Model) viewConfig() string {
	cs := m.cfg
	var b strings.Builder
	b.WriteString(fmt.Sprintf("Configure ch%d   (tab field · enter apply · esc cancel)\n\n", m.sel))
	b.WriteString("Name:    " + cs.name + "\n")
	b.WriteString("Mode:    " + cs.modeLabel + paramSummary(cs) + "\n")
	b.WriteString("RX dev:  " + orNone(cs.rxDev) + "\n")
	b.WriteString("TX dev:  " + orSame(cs.txDev) + "\n")
	b.WriteString("PTT dev: " + orNone(cs.pttDev) + "  method " + cs.pttMethod.String() + "\n\n")
	b.WriteString("Capture devices:\n")
	for _, d := range cs.devices {
		if d.GetHasCapture() {
			b.WriteString("  " + d.GetDeviceId() + "  " + d.GetLabel() + "\n")
		}
	}
	if cs.udev != "" {
		b.WriteString("\nudev rule:\n" + cs.udev + "\n")
	}
	return b.String()
}

func paramSummary(cs *configState) string {
	mi := modeByLabel(cs.modeLabel)
	if mi == nil || len(mi.params) == 0 {
		return ""
	}
	parts := make([]string, 0, len(mi.params))
	for _, p := range mi.params {
		v := p.def
		if cs.params != nil {
			if got, ok := cs.params[p.key]; ok {
				v = got
			}
		}
		parts = append(parts, fmt.Sprintf("%s=%g", p.key, v))
	}
	return " (" + strings.Join(parts, ", ") + ")"
}

func orSame(s string) string {
	if s == "" {
		return "(same as RX)"
	}
	return s
}

// udevCmd fetches an install-ready udev rule for the PTT device (config helper).
func (m *Model) udevCmd(dev string) tea.Cmd {
	c := m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		r, err := c.SuggestUdevRule(ctx, dev)
		if err != nil {
			return rpcErrMsg{err}
		}
		return rpcOKMsg{what: "udev:" + r.GetRule()}
	}
}

var _ = client.Fake{} // keep import while wiring widgets
```

- [ ] **Step 4: Run, expect PASS** — `go test ./internal/app/ -run TestConfigApply`.

- [ ] **Step 5: Commit**

```bash
git add clients/omnimodem-tui/internal/app/config.go clients/omnimodem-tui/internal/app/config_test.go
git commit -m "tui: configuration screen — channel/audio/ptt bind pipeline + gain + udev"
```

### Task 12: Gain sliders (SetAudioGain)

**Files:**
- Modify: `clients/omnimodem-tui/internal/app/config.go`
- Test: `clients/omnimodem-tui/internal/app/config_test.go`

- [ ] **Step 1: Write the failing test**:

```go
func TestSetGainCmd(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.sel = 1
	cmd := m.setGainCmd(2.0, 1.0)
	cmd()
	if len(f.GainCalls) != 1 || f.GainCalls[0].GetRxGain() != 2.0 || f.GainCalls[0].GetChannel() != 1 {
		t.Fatalf("gain call wrong: %+v", f.GainCalls)
	}
}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** (append to `config.go`):

```go
func (m *Model) setGainCmd(rx, tx float32) tea.Cmd {
	ch := m.sel
	c := m.c
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		if err := c.SetAudioGain(ctx, &pb.SetAudioGainRequest{Channel: ch, RxGain: rx, TxGain: tx}); err != nil {
			return rpcErrMsg{err}
		}
		return rpcOKMsg{what: "gain"}
	}
}
```

- [ ] **Step 4: Run, expect PASS**. **Step 5: Commit** `git commit -am "tui: RX/TX gain via SetAudioGain"`.

---

## Phase 3 — Operate screen (ragchew) + waterfall

### Task 13: TX flow state machine

**Files:**
- Create: `clients/omnimodem-tui/internal/app/tx.go`
- Test: `clients/omnimodem-tui/internal/app/tx_test.go`

- [ ] **Step 1: Write the failing test** (send → acquire lease → transmit → complete → release; Halt aborts; watchdog trips):

```go
package app

import (
	"testing"
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
)

func TestTxLifecycle(t *testing.T) {
	tx := &txState{}
	if tx.phase != txIdle {
		t.Fatal("starts idle")
	}
	tx.begin([]byte("CQ"))
	if tx.phase != txAcquiring {
		t.Fatal("begin → acquiring")
	}
	tx.onLeaseGranted()
	if tx.phase != txTransmitting {
		t.Fatal("lease → transmitting")
	}
	tx.onComplete()
	if tx.phase != txIdle {
		t.Fatal("complete → idle")
	}
}

func TestTxWatchdogTrips(t *testing.T) {
	tx := &txState{watchdog: 10 * time.Second}
	tx.begin([]byte("CQ"))
	tx.onLeaseGranted()
	tx.startedAt = time.Now().Add(-11 * time.Second)
	if !tx.watchdogExpired(time.Now()) {
		t.Fatal("watchdog should expire after ceiling")
	}
}

var _ = client.Fake{}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** `tx.go`:

```go
package app

import (
	"time"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

type txPhase int

const (
	txIdle txPhase = iota
	txAcquiring
	txTransmitting
)

// txState is the per-operate TX lifecycle. The daemon auto-keys PTT for the
// burst, so the client only sequences lease → Transmit → complete → release.
type txState struct {
	phase     txPhase
	payload   []byte
	id        uint64
	startedAt time.Time
	watchdog  time.Duration // 0 = disabled
}

func (t *txState) begin(payload []byte) {
	t.phase = txAcquiring
	t.payload = payload
}
func (t *txState) onLeaseGranted() {
	t.phase = txTransmitting
	t.startedAt = time.Now()
}
func (t *txState) onComplete() { t.phase = txIdle; t.payload = nil }
func (t *txState) halt()       { t.phase = txIdle; t.payload = nil }
func (t *txState) active() bool { return t.phase != txIdle }
func (t *txState) watchdogExpired(now time.Time) bool {
	return t.watchdog > 0 && t.phase == txTransmitting && now.Sub(t.startedAt) > t.watchdog
}

// commands that drive the FSM transitions:
func acquireLeaseCmd(c client.ModemClient, ch uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		r, err := c.AcquireTxLease(ctx, ch)
		if err != nil {
			return rpcErrMsg{err}
		}
		return leaseMsg{r}
	}
}
func transmitCmd(c client.ModemClient, ch uint32, payload []byte) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		id, err := c.Transmit(ctx, ch, payload)
		if err != nil {
			return rpcErrMsg{err}
		}
		return transmitMsg{id}
	}
}
func releaseLeaseCmd(c client.ModemClient, ch uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		_ = c.ReleaseTxLease(ctx, ch)
		return rpcOKMsg{what: "lease-released"}
	}
}

var _ = pb.TxLeaseResponse{}
```

- [ ] **Step 4: Run, expect PASS**. **Step 5: Commit** `git commit -am "tui: TX flow state machine (lease→transmit→complete, halt, watchdog)"`.

### Task 14: Macro bar

**Files:**
- Create: `clients/omnimodem-tui/internal/app/macros.go`
- Test: `clients/omnimodem-tui/internal/app/macros_test.go`

- [ ] **Step 1: Write the failing test** (macro text expands `{mycall}`/`{call}`/`{rst}` placeholders):

```go
package app

import "testing"

func TestMacroExpand(t *testing.T) {
	ctx := macroCtx{myCall: "NW5W", theirCall: "W1AW", rst: "599"}
	got := expandMacro("{mycall} de {call} ur {rst}", ctx)
	if got != "NW5W de W1AW ur 599" {
		t.Fatalf("expand = %q", got)
	}
}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** `macros.go`:

```go
package app

import "strings"

type macro struct {
	key  string // function-key label, e.g. "F1"
	name string // "CQ", "Call", "RST", "73", "Brag"
	text string // template with {mycall}/{call}/{rst} placeholders
}

type macroCtx struct{ myCall, theirCall, rst string }

var defaultMacros = []macro{
	{"F1", "CQ", "CQ CQ de {mycall} {mycall} K"},
	{"F2", "Call", "{call} de {mycall} {mycall}"},
	{"F3", "RST", "{call} de {mycall} ur {rst} {rst}"},
	{"F4", "73", "{call} de {mycall} 73 e e"},
	{"F5", "Brag", "{call} de {mycall} rig is omnimodem pwr 50w"},
}

func expandMacro(tmpl string, ctx macroCtx) string {
	r := strings.NewReplacer(
		"{mycall}", ctx.myCall,
		"{call}", ctx.theirCall,
		"{rst}", ctx.rst,
	)
	return r.Replace(tmpl)
}

// macroBar renders the F-key strip plus the TX/Halt affordances.
func macroBar() string {
	var parts []string
	for _, mc := range defaultMacros {
		parts = append(parts, mc.key+" "+mc.name)
	}
	return strings.Join(parts, "  ") + "      [Esc] HALT TX"
}
```

- [ ] **Step 4: Run, expect PASS**. **Step 5: Commit** `git commit -am "tui: macro bar + placeholder expansion"`.

### Task 15: Waterfall strip (SpectrumFrame)

**Files:**
- Create: `clients/omnimodem-tui/internal/app/waterfall.go`
- Test: `clients/omnimodem-tui/internal/app/waterfall_test.go`

- [ ] **Step 1: Write the failing test** (a `SpectrumFrame`'s uint8 bins map to a ramp; brightest bin → densest glyph):

```go
package app

import (
	"testing"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

func TestWaterfallLine(t *testing.T) {
	wf := &waterfall{}
	wf.push(&pb.SpectrumFrame{Bins: []byte{0, 64, 128, 255}})
	line := wf.line(4)
	runes := []rune(line)
	if runes[0] != ' ' { // floor bin → blank
		t.Fatalf("bin 0 should be blank, got %q", string(runes[0]))
	}
	if runes[3] != '█' { // ceiling bin → full block
		t.Fatalf("bin 255 should be full block, got %q", string(runes[3]))
	}
}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** `waterfall.go`:

```go
package app

import (
	"strings"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

// ramp maps a 0..255 intensity to a density glyph (low→high).
var ramp = []rune{' ', '·', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'}

type waterfall struct {
	last      *pb.SpectrumFrame
	freqStart float32
	freqStep  float32
	enabled   bool
}

func (w *waterfall) push(f *pb.SpectrumFrame) {
	w.last = f
	w.freqStart = f.GetFreqStartHz()
	w.freqStep = f.GetFreqStepHz()
}

// line renders the latest spectrum into `width` glyphs (resampling bins to fit).
func (w *waterfall) line(width int) string {
	if w.last == nil || width <= 0 {
		return strings.Repeat(" ", max(0, width))
	}
	bins := w.last.GetBins()
	if len(bins) == 0 {
		return strings.Repeat(" ", width)
	}
	var b strings.Builder
	for x := 0; x < width; x++ {
		bi := x * len(bins) / width
		v := bins[bi]
		g := ramp[int(v)*(len(ramp)-1)/255]
		b.WriteRune(g)
	}
	return b.String()
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}

// enableSpectrumCmd asks the daemon to start the per-channel spectrum stream
// for the operate screen (default sizing; the daemon clamps + echoes actuals).
func enableSpectrumCmd(c client.ModemClient, ch uint32, binCount uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		resp, err := c.ConfigureSpectrum(ctx, &pb.ConfigureSpectrumRequest{
			Channel: ch, Enable: true, BinCount: binCount, FreqHiHz: 3000,
		})
		if err != nil {
			return rpcErrMsg{err}
		}
		return spectrumCfgMsg{resp}
	}
}

func disableSpectrumCmd(c client.ModemClient, ch uint32) tea.Cmd {
	return func() tea.Msg {
		ctx, cancel := rpcCtx()
		defer cancel()
		_, _ = c.ConfigureSpectrum(ctx, &pb.ConfigureSpectrumRequest{Channel: ch, Enable: false})
		return rpcOKMsg{what: "spectrum-off"}
	}
}
```

- [ ] **Step 4: Run, expect PASS**. **Step 5: Commit** `git commit -am "tui: waterfall strip from SpectrumFrame + ConfigureSpectrum"`.

### Task 16: Operate screen — ragchew (compose + transcript + macros + TX) and Go CI

**Files:**
- Create: `clients/omnimodem-tui/internal/app/operate.go`
- Modify: `.github/workflows/ci.yml`
- Test: `clients/omnimodem-tui/internal/app/operate_test.go`

- [ ] **Step 1: Write the failing test** (Enter on a composed line begins TX and, on lease, transmits the typed bytes; the line is appended to the transcript as a `›` TX entry):

```go
package app

import (
	"strings"
	"testing"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

func TestOperateSendTransmits(t *testing.T) {
	f := &client.Fake{}
	m := New(f, "x")
	m.live[0] = &chanLive{mode: "psk31"}
	m.sel = 0
	m.enterOperate()
	m.op.compose = "CQ CQ de NW5W"
	// Enter → begin TX (acquire lease)
	m.updateOperate(tea.KeyMsg{Type: tea.KeyEnter})
	if m.op.tx.phase != txAcquiring {
		t.Fatalf("enter should start TX, phase=%v", m.op.tx.phase)
	}
	// lease granted → transmit
	m.updateOperate(leaseMsg{&pb.TxLeaseResponse{Granted: true}})
	if len(f.TransmitCalls) != 1 || string(f.TransmitCalls[0].GetPayload()) != "CQ CQ de NW5W" {
		t.Fatalf("transmit payload wrong: %+v", f.TransmitCalls)
	}
	if !strings.Contains(m.viewOperate(), "CQ CQ de NW5W") {
		t.Fatalf("transcript should show the sent line")
	}
}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** `operate.go`:

```go
package app

import (
	"fmt"
	"strings"
	"time"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
	tea "github.com/charmbracelet/bubbletea"
)

type transcriptLine struct {
	t   time.Time
	dir rune // '›' TX, '‹' RX
	txt string
}

type operateState struct {
	compose    string
	transcript []transcriptLine
	tx         txState
	wf         waterfall
	myCall     string
	theirCall  string
	rst        string
}

func (m *Model) enterOperate() {
	m.screen = screenOperate
	m.op = &operateState{
		myCall: "NW5W", rst: "599",
		tx: txState{watchdog: 30 * time.Second},
	}
}

func (m *Model) updateOperate(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case eventMsg:
		// status/levels handled centrally; spectrum + tx-complete handled here.
		if sf := msg.ev.GetSpectrumFrame(); sf != nil {
			m.op.wf.push(sf)
		}
		if tc := msg.ev.GetTransmitComplete(); tc != nil {
			m.op.tx.onComplete()
			return m, releaseLeaseCmd(m.c, m.sel)
		}
		m.applyEvent(msg.ev)
		return m, waitForEvent(m.events)
	case leaseMsg:
		if msg.resp.GetGranted() {
			m.op.tx.onLeaseGranted()
			return m, transmitCmd(m.c, m.sel, m.op.tx.payload)
		}
		m.op.tx.halt()
		m.err = fmt.Sprintf("TX lease held by ch%d", msg.resp.GetHeldBy())
		return m, nil
	case transmitMsg:
		m.op.tx.id = msg.id
		return m, nil
	case tickMsg:
		if m.op.tx.watchdogExpired(time.Time(msg)) {
			m.op.tx.halt()
			m.err = "TX watchdog: aborted"
			return m, releaseLeaseCmd(m.c, m.sel)
		}
		return m, tickCmd()
	case tea.KeyMsg:
		switch msg.String() {
		case "esc":
			if m.op.tx.active() {
				m.op.tx.halt()
				return m, releaseLeaseCmd(m.c, m.sel)
			}
			m.screen = screenDashboard
			return m, nil
		case "enter":
			return m, m.sendCompose()
		case "f1", "f2", "f3", "f4", "f5":
			m.op.compose = expandMacro(macroForKey(msg.String()), macroCtx{
				myCall: m.op.myCall, theirCall: m.op.theirCall, rst: m.op.rst,
			})
			return m, nil
		case "backspace":
			if len(m.op.compose) > 0 {
				m.op.compose = m.op.compose[:len(m.op.compose)-1]
			}
		default:
			if len(msg.Runes) > 0 {
				m.op.compose += string(msg.Runes)
			}
		}
	}
	return m, nil
}

// sendCompose appends the line to the transcript and starts the TX FSM.
func (m *Model) sendCompose() tea.Cmd {
	line := strings.TrimSpace(m.op.compose)
	if line == "" || m.op.tx.active() {
		return nil
	}
	m.op.transcript = append(m.op.transcript, transcriptLine{t: time.Now(), dir: '›', txt: line})
	m.op.tx.begin([]byte(line))
	m.op.compose = ""
	return acquireLeaseCmd(m.c, m.sel)
}

func macroForKey(k string) string {
	idx := map[string]int{"f1": 0, "f2": 1, "f3": 2, "f4": 3, "f5": 4}[k]
	return defaultMacros[idx].text
}

func (m *Model) viewOperate() string {
	op := m.op
	var b strings.Builder
	b.WriteString("Activity            │ Transcript\n")
	for _, l := range op.transcript {
		b.WriteString(fmt.Sprintf("                    │ %s %c %s\n", l.t.Format("15:04"), l.dir, l.txt))
	}
	b.WriteString("                    │ " + op.wf.line(40) + "\n")
	b.WriteString("\n› " + op.compose)
	if op.tx.active() {
		b.WriteString("    [TX ACTIVE]")
	}
	b.WriteString("\n" + macroBar())
	return b.String()
}
```

- [ ] **Step 4: Add the Go CI job.** In `.github/workflows/ci.yml`, add a second job:

```yaml
  tui:
    name: tui (go)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-go@v5
        with:
          go-version: "1.26"
      - name: Install protoc + plugins
        run: |
          sudo apt-get update && sudo apt-get install -y protobuf-compiler
          go install google.golang.org/protobuf/cmd/protoc-gen-go@latest
          go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@latest
          echo "$(go env GOPATH)/bin" >> "$GITHUB_PATH"
      - name: Generate + test
        working-directory: clients/omnimodem-tui
        run: |
          ./gen.sh
          go vet ./...
          go test ./...
```

- [ ] **Step 5: Run locally, expect PASS** — `go test ./internal/app/ -run TestOperateSendTransmits`. Then commit (workflow edit must be pushed over SSH, not the HTTPS PAT):

```bash
git add clients/omnimodem-tui/internal/app/operate.go clients/omnimodem-tui/internal/app/operate_test.go .github/workflows/ci.yml
git commit -m "tui: operate screen (ragchew) — compose/transcript/macros/TX + Go CI job"
```

---

## Phase 4 — Operate screen (FT8)

### Task 17: FT8 auto-sequence ladder + slot clock

**Files:**
- Create: `clients/omnimodem-tui/internal/app/ft8.go`
- Test: `clients/omnimodem-tui/internal/app/ft8_test.go`

- [ ] **Step 1: Write the failing test** (the standard ladder advances CQ→grid→report→R-report→RR73→73; the slot clock derives the 0–15 s position):

```go
package app

import (
	"testing"
	"time"
)

func TestFT8LadderAdvance(t *testing.T) {
	seq := newFT8Seq("NW5W", "EM10")
	seq.target("W1AW", "FN31")
	if got := seq.current(); got != "W1AW NW5W EM10" {
		t.Fatalf("Tx1 = %q", got)
	}
	seq.advance()
	if got := seq.current(); got != "W1AW NW5W -10" { // default report
		t.Fatalf("Tx2 = %q", got)
	}
}

func TestSlotPosition(t *testing.T) {
	at := time.Date(2026, 1, 1, 0, 0, 7, 0, time.UTC)
	if p := slotPosition(at); p < 6.9 || p > 7.1 {
		t.Fatalf("slot pos at :07 = %v, want ~7", p)
	}
}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** `ft8.go`:

```go
package app

import (
	"fmt"
	"time"
)

// ft8Seq is the standard FT8 QSO message ladder (WSJT-X Tx1..Tx5/Tx6).
type ft8Seq struct {
	myCall, myGrid string
	dxCall, dxGrid string
	report         int
	step           int
}

func newFT8Seq(myCall, myGrid string) *ft8Seq {
	return &ft8Seq{myCall: myCall, myGrid: myGrid, report: -10}
}
func (s *ft8Seq) target(call, grid string) { s.dxCall, s.dxGrid, s.step = call, grid, 0 }
func (s *ft8Seq) advance()                 { s.step++ }

// current returns the message for the current ladder step.
func (s *ft8Seq) current() string {
	switch s.step {
	case 0:
		return fmt.Sprintf("%s %s %s", s.dxCall, s.myCall, s.myGrid) // Tx1: grid
	case 1:
		return fmt.Sprintf("%s %s %+d", s.dxCall, s.myCall, s.report) // Tx2: report
	case 2:
		return fmt.Sprintf("%s %s R%+d", s.dxCall, s.myCall, s.report) // Tx3: R-report
	case 3:
		return fmt.Sprintf("%s %s RR73", s.dxCall, s.myCall) // Tx4
	default:
		return fmt.Sprintf("%s %s 73", s.dxCall, s.myCall) // Tx5
	}
}

// cq is the calling message (Tx6).
func (s *ft8Seq) cq() string { return fmt.Sprintf("CQ %s %s", s.myCall, s.myGrid) }

// finished reports whether the ladder has reached 73/RR73 (→ prompt a log entry).
func (s *ft8Seq) finished() bool { return s.step >= 3 }

// slotPosition returns seconds into the current 15 s FT8 slot (0..15).
func slotPosition(at time.Time) float64 {
	sec := float64(at.UTC().Second()%15) + float64(at.Nanosecond())/1e9
	return sec
}
```

- [ ] **Step 4: Run, expect PASS**. **Step 5: Commit** `git commit -am "tui: FT8 auto-sequence ladder + slot clock"`.

### Task 18: Mini QSO log + FT8 operate wiring

**Files:**
- Create: `clients/omnimodem-tui/internal/app/qsolog.go`
- Modify: `clients/omnimodem-tui/internal/app/operate.go` (FT8 surface selection by mode shape)
- Test: `clients/omnimodem-tui/internal/app/qsolog_test.go`

- [ ] **Step 1: Write the failing test** (logging a QSO appends a UTC-stamped record):

```go
package app

import "testing"

func TestQSOLogAppend(t *testing.T) {
	var lg qsoLog
	lg.add("W1AW", "FN31", "-10")
	if len(lg.entries) != 1 || lg.entries[0].call != "W1AW" {
		t.Fatalf("log entry wrong: %+v", lg.entries)
	}
}
```

- [ ] **Step 2: Run, expect FAIL**.

- [ ] **Step 3: Implement** `qsolog.go`:

```go
package app

import "time"

type qsoEntry struct {
	utc  time.Time
	call string
	grid string
	rst  string
}

type qsoLog struct{ entries []qsoEntry }

func (l *qsoLog) add(call, grid, rst string) {
	l.entries = append(l.entries, qsoEntry{utc: time.Now().UTC(), call: call, grid: grid, rst: rst})
}
```

Then in `operate.go`, select the surface by mode shape: when `modeByLabel(m.live[m.sel].mode)` has `shape=="ft8"`, render the sequencer (Tx1–Tx6 ladder + `slotPosition` in the status bar) and, on reaching `seq.finished()`, call `qsoLog.add`. Wire `enterOperate` to attach an `*ft8Seq` + `qsoLog` to `operateState` and route Enter to "Enable Tx for the next slot" rather than free-send. (Keep the ragchew path unchanged for `shape=="chat"`.)

- [ ] **Step 4: Run, expect PASS**. **Step 5: Commit** `git commit -am "tui: mini QSO log + FT8 operate surface wiring"`.

---

## Self-review checklist (run before opening the PR)

- **Spec coverage** (design §1 goals): connect ✓(T7) · enumerate+select devices ✓(T11) · bind channel/audio/ptt ✓(T11) · live levels+PTT ✓(T6/T7) · select mode + compose + transmit ✓(T16) · TX progress+lease+clock ✓(T13/T16/T17) · macros+status+activity+abort ✓(T7/T14/T16). Waterfall ✓(T15, now real via #24). Mode params ✓(T10/T11, via #26). RX decode — **deferred** by design (panes scaffolded in T16).
- **Type consistency:** `ModemClient` method set identical in `client.go`, `fake.go`, and every `*Cmd`. `chanLive`/`operateState`/`configState`/`txState` fields referenced in tests match their structs.
- **No placeholders:** every code step is real Go; no `TODO`. Bubble Tea API used: `Init() tea.Cmd`, `Update(tea.Msg)(tea.Model,tea.Cmd)`, `View() string`, `tea.Cmd=func()tea.Msg`, `tea.Batch`, `tea.Tick`, `tea.Quit`.
- **Gen note:** if `gen.sh`'s import-path flattening doesn't place files in `internal/pb/`, adjust the `paths=` mode or add `option go_package` to a generation-only copy — do **not** edit the shared `proto/omnimodem.proto` solely for Go.

## Out of scope (fast-follow, not this plan)

- **RX decode:** render `RxFrame` into the transcript (inbound `‹` lines) and populate the activity roster + `ChannelMetrics` SNR → RST. The transcript/roster panes are already laid out (T16), so this drops in without rework. **The user asked to start RX immediately after this PR is QC'd — confirm scope with them then.**
- mTLS/remote (`--addr` host:port works; TLS creds wiring deferred).
