#!/usr/bin/env bash
# Generate Go bindings for omnimodem.v1 into internal/pb, without editing the
# shared proto (the go_package is supplied via the M-mapping below).
set -euo pipefail
cd "$(dirname "$0")"
PKG="github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/pb;omnimodemv1"
protoc -I ../../proto \
  --go_out=internal/pb --go_opt=paths=source_relative --go_opt="Momnimodem.proto=$PKG" \
  --go-grpc_out=internal/pb --go-grpc_opt=paths=source_relative --go-grpc_opt="Momnimodem.proto=$PKG" \
  ../../proto/omnimodem.proto
echo "generated: $(ls internal/pb/*.pb.go)"
