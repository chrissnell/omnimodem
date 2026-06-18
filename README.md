# omnimodem

A gRPC-driven, building-block-based multi-mode software modem.

omnimodem runs multiple amateur-radio modes simultaneously from a single binary,
each bound to its own audio interface and PTT, and is operated entirely over a
stable gRPC API so developers can build their own frontends.

## Building

Requires a Rust toolchain and the Protocol Buffers compiler, `protoc`, which
the gRPC codegen (`tonic-build`) invokes at build time:

```sh
# Debian/Ubuntu: apt install -y protobuf-compiler
# macOS:         brew install protobuf
cargo build
cargo test
```

If `protoc` is not on `PATH`, point the build at it explicitly:

```sh
PROTOC=/path/to/protoc cargo build
```

## Design

See [`docs/design/2026-06-17-omnimodem-design.md`](docs/design/2026-06-17-omnimodem-design.md)
for the full design.
