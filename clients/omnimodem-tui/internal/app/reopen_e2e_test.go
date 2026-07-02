package app

import (
	"context"
	"errors"
	"sync"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	pb "github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb"
)

// daemonFake is a stateful ModemClient that mirrors the real daemon's persist +
// snapshot semantics (verified by the Rust test
// core::tests::snapshot_reports_all_devices_after_config_and_mode_change):
//   - ConfigureChannel updates name/mode, preserving audio + PTT bindings.
//   - ConfigureAudio stores RX; empty TX means "same as RX".
//   - ConfigurePtt stores the device + method.
//   - GetState reports TX empty when it mirrors RX, and reports a real PTT device
//     regardless of method (only the internal placeholder is hidden — here an
//     empty device id stands in for that).
//
// It lets a test drive the whole pick -> persist -> snapshot -> reopen loop.
type daemonFake struct {
	*client.Fake
	mu     sync.Mutex
	chans  map[uint32]*fakeChan
	pttErr error // if set, ConfigurePtt persists the device but returns this error
}

type fakeChan struct {
	name, mode     string
	rx, tx, pttDev string
	pttMethod      pb.PttMethod
}

func newDaemonFake() *daemonFake {
	return &daemonFake{Fake: &client.Fake{}, chans: map[uint32]*fakeChan{}}
}

func (d *daemonFake) ch(id uint32) *fakeChan {
	c := d.chans[id]
	if c == nil {
		c = &fakeChan{}
		d.chans[id] = c
	}
	return c
}

func (d *daemonFake) ConfigureChannel(_ context.Context, r *pb.ConfigureChannelRequest) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	c := d.ch(r.GetChannel())
	c.name, c.mode = r.GetName(), r.GetMode() // bindings preserved
	return nil
}

func (d *daemonFake) ConfigureAudio(_ context.Context, r *pb.ConfigureAudioRequest) (*pb.ConfigureAudioResponse, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	c := d.ch(r.GetChannel())
	c.rx = r.GetDeviceId()
	if r.GetTxDeviceId() == "" {
		c.tx = c.rx // empty TX mirrors RX
	} else {
		c.tx = r.GetTxDeviceId()
	}
	return &pb.ConfigureAudioResponse{ActualSampleRate: 48000, ActualTxSampleRate: 48000}, nil
}

func (d *daemonFake) ConfigurePtt(_ context.Context, r *pb.ConfigurePttRequest) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	c := d.ch(r.GetChannel())
	// Persist the device choice BEFORE (possibly) reporting a driver failure —
	// mirrors the daemon's configure_ptt, which commits then opens the driver.
	c.pttDev, c.pttMethod = r.GetDeviceId(), r.GetMethod()
	return d.pttErr
}

func (d *daemonFake) GetState(context.Context) (*pb.ModemState, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	st := &pb.ModemState{}
	for id, c := range d.chans {
		tx := c.tx
		if tx == c.rx {
			tx = "" // report "same as RX" as empty
		}
		st.Channels = append(st.Channels, &pb.ChannelInfo{
			Channel: id, Name: c.name, Mode: c.mode, Running: true,
			DeviceId: c.rx, TxDeviceId: tx, PttDeviceId: c.pttDev, PttMethod: c.pttMethod,
		})
	}
	return st, nil
}

// Reopening Configure after picking RX, TX, and PTT must show all three — the
// full pick -> persist -> snapshot-refresh -> reopen loop against a client that
// behaves exactly like the verified daemon.
func TestConfigReopenReflectsAllSavedDevices(t *testing.T) {
	run := func(t *testing.T, rapid bool) {
		d := newDaemonFake()
		m := New(d, "x")
		m.connected = true
		m.push(newChannelsView(m))
		m.sel = 0
		m.live[0] = &chanLive{name: "vfo-a"}
		v := newConfigView(m)
		v.setDevices([]*pb.DeviceInfo{
			devItem("usb:rx", "Mic", true, false),
			devItem("usb:tx", "Spk", false, true),
			devItem("usb:ptt", "Rig", true, true),
		})
		m.push(v)

		if rapid {
			// All picks before any save completes, then esc drives them out.
			v.rxID = "usb:rx"
			rxSave := v.maybePersist()
			v.txID = "usb:tx"
			v.maybePersist()
			v.pttID = "usb:ptt"
			v.maybePersist()
			_, escCmd := m.Update(tea.KeyMsg{Type: tea.KeyEsc})
			drive(m, rxSave)
			drive(m, escCmd)
		} else {
			// Deliberate: each pick's save completes before the next.
			v.rxID = "usb:rx"
			drive(m, v.maybePersist())
			v.txID = "usb:tx"
			drive(m, v.maybePersist())
			v.pttID = "usb:ptt"
			drive(m, v.maybePersist())
			_, escCmd := m.Update(tea.KeyMsg{Type: tea.KeyEsc})
			drive(m, escCmd)
		}

		// Reopen Configure — it preloads from m.live, which the close refreshed.
		re := newConfigView(m)
		if re.rxID != "usb:rx" {
			t.Fatalf("RX not shown on reopen: %q", re.rxID)
		}
		if re.txID != "usb:tx" {
			t.Fatalf("TX not shown on reopen: %q (m.live tx=%q)", re.txID, m.live[0].txDeviceID)
		}
		if re.pttID != "usb:ptt" {
			t.Fatalf("PTT not shown on reopen: %q (m.live ptt=%q)", re.pttID, m.live[0].pttDeviceID)
		}
	}
	t.Run("deliberate", func(t *testing.T) { run(t, false) })
	t.Run("rapid", func(t *testing.T) { run(t, true) })
}

// A device-based PTT method persists the device but the daemon returns an error
// (its driver can't open without a serial node). The client must still refresh
// from the snapshot on error, so reopening Configure shows the saved PTT device
// rather than "(none)" — the "PTT device still not persisted" report.
func TestConfigReopenShowsPttDevicePersistedDespiteDriverError(t *testing.T) {
	d := newDaemonFake()
	m := New(d, "x")
	m.connected = true
	m.push(newChannelsView(m))
	m.sel = 0
	m.live[0] = &chanLive{name: "vfo-a"}
	v := newConfigView(m)
	v.setDevices([]*pb.DeviceInfo{
		devItem("usb:rx", "Mic", true, false),
		devItem("usb:tx", "Spk", false, true),
		devItem("usb:ptt", "Rig", true, true),
	})
	m.push(v)

	// RX/TX save cleanly.
	v.rxID = "usb:rx"
	drive(m, v.maybePersist())
	v.txID = "usb:tx"
	drive(m, v.maybePersist())

	// The PTT save persists the device but errors (driver open fails).
	d.pttErr = errors.New("configure ptt: device gone")
	v.pttID = "usb:ptt"
	drive(m, v.maybePersist())
	_, escCmd := m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	drive(m, escCmd)

	// Reopen: the persisted PTT device must be visible even though its save errored.
	re := newConfigView(m)
	if re.pttID != "usb:ptt" {
		t.Fatalf("PTT device must show on reopen despite the driver error, got %q (m.live ptt=%q)", re.pttID, m.live[0].pttDeviceID)
	}
}
