#!/usr/bin/env bash
# Regenerate the Go gRPC stubs from the canonical proto. Requires protoc plus
# protoc-gen-go and protoc-gen-go-grpc on PATH:
#   go install google.golang.org/protobuf/cmd/protoc-gen-go@latest
#   go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@latest
# The proto carries no `option go_package`, so it's mapped on the command line
# (keeps the proto language-agnostic).
set -euo pipefail
cd "$(dirname "$0")/../.."   # repo root
PKG="github.com/chrissnell/omnimodem/clients/omnimodem-client/omnimodemv1;omnimodemv1"
protoc -I proto \
  --go_out=clients/omnimodem-client/omnimodemv1 --go_opt=Momnimodem.proto="$PKG" --go_opt=paths=source_relative \
  --go-grpc_out=clients/omnimodem-client/omnimodemv1 --go-grpc_opt=Momnimodem.proto="$PKG" --go-grpc_opt=paths=source_relative \
  proto/omnimodem.proto
