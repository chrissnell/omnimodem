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
	TuneResp       *pb.SetSdrTuneResponse
	SdrGainResp    *pb.SetSdrGainResponse
	SdrConfigResp  *pb.ConfigureSdrResponse
	CapsResp       *pb.GetSdrCapsResponse
	NextTransmitID uint64
	Err            error // if set, every RPC returns it

	StateCalls         int // GetState invocations (asserts a live-state refresh happened)
	ChannelCalls       []*pb.ConfigureChannelRequest
	AudioCalls         []*pb.ConfigureAudioRequest
	PttCalls           []*pb.ConfigurePttRequest
	GainCalls          []*pb.SetAudioGainRequest
	SpectrumCalls      []*pb.ConfigureSpectrumRequest
	TuneCalls          []*pb.SetSdrTuneRequest
	SdrGainCalls       []*pb.SetSdrGainRequest
	SdrConfigCalls     []*pb.ConfigureSdrRequest
	CapsCalls          []*pb.GetSdrCapsRequest
	TransmitCalls      []*pb.TransmitRequest
	TransmitImageCalls []*pb.TransmitImageRequest
	LeaseAcquired      []uint32
	LeaseReleased      []uint32
}

func (f *Fake) GetState(context.Context) (*pb.ModemState, error) {
	f.mu.Lock()
	f.StateCalls++
	f.mu.Unlock()
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

func (f *Fake) SetSdrTune(_ context.Context, r *pb.SetSdrTuneRequest) (*pb.SetSdrTuneResponse, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.TuneCalls = append(f.TuneCalls, r)
	if f.Err != nil {
		return nil, f.Err
	}
	if f.TuneResp != nil {
		return f.TuneResp, nil
	}
	// Echo the request as a trivial split: whole target on the NCO, center unchanged.
	return &pb.SetSdrTuneResponse{ActualFreqHz: r.GetFreqHz(), CenterHz: r.GetFreqHz(), OffsetHz: 0}, nil
}

func (f *Fake) SetSdrGain(_ context.Context, r *pb.SetSdrGainRequest) (*pb.SetSdrGainResponse, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.SdrGainCalls = append(f.SdrGainCalls, r)
	if f.Err != nil {
		return nil, f.Err
	}
	if f.SdrGainResp != nil {
		return f.SdrGainResp, nil
	}
	return &pb.SetSdrGainResponse{ActualGainDb: r.GetGainDb()}, nil
}

func (f *Fake) ConfigureSdr(_ context.Context, r *pb.ConfigureSdrRequest) (*pb.ConfigureSdrResponse, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.SdrConfigCalls = append(f.SdrConfigCalls, r)
	if f.Err != nil {
		return nil, f.Err
	}
	if f.SdrConfigResp != nil {
		return f.SdrConfigResp, nil
	}
	return &pb.ConfigureSdrResponse{ActualCaptureRate: r.GetCaptureRate()}, nil
}

func (f *Fake) GetSdrCaps(_ context.Context, r *pb.GetSdrCapsRequest) (*pb.GetSdrCapsResponse, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.CapsCalls = append(f.CapsCalls, r)
	if f.Err != nil {
		return nil, f.Err
	}
	if f.CapsResp != nil {
		return f.CapsResp, nil
	}
	return &pb.GetSdrCapsResponse{}, nil
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

func (f *Fake) TransmitImage(_ context.Context, ch, width, height uint32, rgb []byte, color bool, txspp uint32) (uint64, error) {
	f.TransmitImageCalls = append(f.TransmitImageCalls, &pb.TransmitImageRequest{
		Channel: ch, Width: width, Height: height, Rgb: rgb, Color: color, Txspp: txspp,
	})
	return f.NextTransmitID, f.Err
}

func (f *Fake) Subscribe(context.Context) (pb.ModemControl_SubscribeEventsClient, error) {
	return nil, f.Err // event bridge is tested via injected channel, not Subscribe
}

func (f *Fake) Close() error { return nil }
