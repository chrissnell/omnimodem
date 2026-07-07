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
	TransmitImage(ctx context.Context, ch, width, height uint32, rgb []byte, color bool, txspp uint32) (uint64, error)
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

// Dial connects to omnimodemd over UDS (path) or TCP (host:port). mTLS is out of
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

func (g *grpcClient) TransmitImage(ctx context.Context, ch, width, height uint32, rgb []byte, color bool, txspp uint32) (uint64, error) {
	r, err := g.c.TransmitImage(ctx, &pb.TransmitImageRequest{
		Channel: ch, Width: width, Height: height, Rgb: rgb, Color: color, Txspp: txspp,
	})
	if err != nil {
		return 0, err
	}
	return r.GetTransmitId(), nil
}

func (g *grpcClient) Subscribe(ctx context.Context) (pb.ModemControl_SubscribeEventsClient, error) {
	return g.c.SubscribeEvents(ctx, &pb.SubscribeRequest{})
}

func (g *grpcClient) Close() error { return g.conn.Close() }
