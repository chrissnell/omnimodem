// Command omnimodem-client is a small Go gRPC client for exercising an
// omnimodemd daemon during development and hardware bring-up. It enumerates
// audio/PTT devices, then drives a channel through the full Phase-2 surface:
// configure audio, configure PTT, key/unkey the transmitter, and transmit a
// generated PCM tone — printing the event stream throughout.
//
// It connects over the daemon's authorized Unix-domain socket (SO_PEERCRED is
// satisfied by running as the same user as the daemon).
//
// Examples:
//
//	omnimodem-client -socket /run/omnimodem/omnimodem.sock
//	omnimodem-client -socket /tmp/omni.sock -ptt-method rigctld -ptt-node 127.0.0.1:4532
package main

import (
	"context"
	"encoding/binary"
	"flag"
	"fmt"
	"log"
	"math"
	"os"
	"strings"
	"time"

	pb "github.com/chrissnell/omnimodem/clients/omnimodem-client/omnimodemv1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

func main() {
	socket := flag.String("socket", envOr("OMNIMODEM_SOCK", "/run/omnimodem/omnimodem.sock"),
		"path to the daemon's Unix-domain socket")
	channel := flag.Uint("channel", 0, "channel id to operate")
	device := flag.String("device", "", "audio/PTT DeviceId (default: first device from ListDevices)")
	rate := flag.Uint("rate", 48000, "requested sample rate (Hz)")
	pttMethod := flag.String("ptt-method", "serial_rts",
		"PTT method: none|vox|serial_rts|serial_dtr|cm108|gpio|rigctld")
	pttNode := flag.String("ptt-node", "/dev/ttyUSB-mock",
		"PTT node: serial tty / gpiochip / host:port for rigctld")
	pttPin := flag.Uint("ptt-pin", 0, "CM108 pin (1-8) or gpiochip line offset")
	pttInvert := flag.Bool("ptt-invert", false, "invert PTT polarity")
	toneHz := flag.Uint("tone", 1000, "transmit tone frequency (Hz)")
	toneMs := flag.Uint("tone-ms", 500, "transmit tone duration (ms)")
	flag.Parse()

	conn, err := grpc.NewClient("unix://"+*socket, grpc.WithTransportCredentials(insecure.NewCredentials()))
	if err != nil {
		log.Fatalf("dial %s: %v", *socket, err)
	}
	defer conn.Close()
	client := pb.NewModemControlClient(conn)

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	// --- 1. Enumerate audio + PTT devices ---------------------------------
	fmt.Println("== ListDevices ==")
	devResp, err := client.ListDevices(ctx, &pb.ListDevicesRequest{})
	if err != nil {
		log.Fatalf("ListDevices: %v", err)
	}
	if len(devResp.Devices) == 0 {
		fmt.Println("  (no devices reported)")
	}
	for i, d := range devResp.Devices {
		fmt.Printf("  [%d] %s  label=%q capture=%v playback=%v\n",
			i, d.DeviceId, d.Label, d.HasCapture, d.HasPlayback)
	}
	deviceID := *device
	if deviceID == "" {
		if len(devResp.Devices) == 0 {
			log.Fatal("no -device given and ListDevices returned nothing")
		}
		deviceID = devResp.Devices[0].DeviceId
		fmt.Printf("  -> using first device: %s\n", deviceID)
	}

	// --- 2. Subscribe to the event stream (background) --------------------
	evCtx, evCancel := context.WithCancel(context.Background())
	defer evCancel()
	stream, err := client.SubscribeEvents(evCtx, &pb.SubscribeRequest{})
	if err != nil {
		log.Fatalf("SubscribeEvents: %v", err)
	}
	go printEvents(stream)
	time.Sleep(150 * time.Millisecond) // let the snapshot arrive first

	// --- 3. Configure the channel + audio + PTT --------------------------
	ch := uint32(*channel)
	fmt.Println("== ConfigureChannel ==")
	if _, err := client.ConfigureChannel(ctx, &pb.ConfigureChannelRequest{
		Channel: ch, Name: "client-test", Mode: "none",
	}); err != nil {
		log.Fatalf("ConfigureChannel: %v", err)
	}

	fmt.Println("== ConfigureAudio ==")
	audio, err := client.ConfigureAudio(ctx, &pb.ConfigureAudioRequest{
		Channel: ch, DeviceId: deviceID, SampleRate: uint32(*rate), Fanout: 1,
	})
	if err != nil {
		log.Fatalf("ConfigureAudio: %v", err)
	}
	fmt.Printf("  actual_sample_rate=%d\n", audio.ActualSampleRate)

	fmt.Println("== ConfigurePtt ==")
	method, err := pttMethodValue(*pttMethod)
	if err != nil {
		log.Fatal(err)
	}
	if _, err := client.ConfigurePtt(ctx, &pb.ConfigurePttRequest{
		Channel:   ch,
		DeviceId:  deviceID,
		Method:    method,
		Node:      *pttNode,
		PinOrLine: uint32(*pttPin),
		Invert:    *pttInvert,
	}); err != nil {
		log.Fatalf("ConfigurePtt: %v", err)
	}

	// --- 4. Exercise PTT: key, hold, unkey -------------------------------
	fmt.Println("== KeyPtt (key, hold 300ms, unkey) ==")
	if _, err := client.KeyPtt(ctx, &pb.KeyPttRequest{Channel: ch, Keyed: true}); err != nil {
		log.Fatalf("KeyPtt(true): %v", err)
	}
	time.Sleep(300 * time.Millisecond)
	if _, err := client.KeyPtt(ctx, &pb.KeyPttRequest{Channel: ch, Keyed: false}); err != nil {
		log.Fatalf("KeyPtt(false): %v", err)
	}

	// --- 5. Exercise audio: transmit a PCM tone --------------------------
	pcm := sineTone(int(*rate), int(*toneHz), int(*toneMs))
	fmt.Printf("== Transmit (%d Hz tone, %d ms, %d bytes PCM) ==\n", *toneHz, *toneMs, len(pcm))
	if _, err := client.Transmit(ctx, &pb.TransmitRequest{Channel: ch, Payload: pcm}); err != nil {
		log.Fatalf("Transmit: %v", err)
	}

	// Give the event stream a moment to deliver the transmit/PTT events.
	time.Sleep(750 * time.Millisecond)
	evCancel()
	fmt.Println("== done ==")
}

// printEvents prints each event from the subscription until the stream ends.
func printEvents(stream grpc.ServerStreamingClient[pb.Event]) {
	for {
		ev, err := stream.Recv()
		if err != nil {
			return
		}
		switch k := ev.Kind.(type) {
		case *pb.Event_Snapshot:
			fmt.Printf("  event: snapshot (%d channels)\n", len(k.Snapshot.Channels))
		case *pb.Event_PttState:
			fmt.Printf("  event: ptt_state channel=%d keyed=%v\n", k.PttState.Channel, k.PttState.Keyed)
		case *pb.Event_TransmitStarted:
			fmt.Printf("  event: transmit_started channel=%d id=%d\n",
				k.TransmitStarted.Channel, k.TransmitStarted.TransmitId)
		case *pb.Event_TransmitComplete:
			fmt.Printf("  event: transmit_complete channel=%d id=%d\n",
				k.TransmitComplete.Channel, k.TransmitComplete.TransmitId)
		case *pb.Event_DeviceArrived:
			fmt.Printf("  event: device_arrived %s\n", k.DeviceArrived.DeviceId)
		case *pb.Event_DeviceDeparted:
			fmt.Printf("  event: device_departed %s\n", k.DeviceDeparted.DeviceId)
		case *pb.Event_ChannelConfigured:
			fmt.Printf("  event: channel_configured channel=%d\n", k.ChannelConfigured.Channel)
		default:
			fmt.Printf("  event: %T\n", k)
		}
	}
}

// sineTone returns mono little-endian i16 PCM for a sine wave.
func sineTone(rate, hz, ms int) []byte {
	n := rate * ms / 1000
	buf := make([]byte, n*2)
	for i := 0; i < n; i++ {
		s := int16(math.Sin(2*math.Pi*float64(hz)*float64(i)/float64(rate)) * 12000)
		binary.LittleEndian.PutUint16(buf[i*2:], uint16(s))
	}
	return buf
}

// pttMethodValue maps a CLI method string to the proto enum value.
func pttMethodValue(s string) (pb.PttMethod, error) {
	key := "PTT_METHOD_" + strings.ToUpper(strings.TrimSpace(s))
	v, ok := pb.PttMethod_value[key]
	if !ok || v == int32(pb.PttMethod_PTT_METHOD_UNSPECIFIED) {
		return 0, fmt.Errorf("unknown -ptt-method %q", s)
	}
	return pb.PttMethod(v), nil
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
