# Omnimodem build orchestration.
#
#   make            build both the modem daemon and the TUI client (release)
#   make modem      build omnimodemd  → target/release/omnimodemd
#   make tui        build the TUI     → bin/omnimodem-tui
#   make proto      regenerate the TUI's Go gRPC bindings (needs protoc + plugins)
#   make test       run Rust + Go test suites
#   make run-modem  build + run the daemon
#   make run-tui    build + run the TUI (connects to the daemon's default socket)
#   make clean      remove build artifacts
#
# Prerequisites (not installed by this Makefile):
#   - Rust toolchain (cargo), protobuf-compiler (protoc), libasound2-dev (ALSA), pkg-config
#   - Go 1.26+
# See docs/running.md for setup + how to start both.

TUI_DIR := clients/omnimodem-tui
TUI_BIN := bin/omnimodem-tui
MODEM_BIN := target/release/omnimodemd

.PHONY: all modem tui proto test test-modem test-tui run-modem run-tui clean help

all: modem tui ## Build the daemon and the TUI

modem: ## Build omnimodemd (release) → target/release/omnimodemd
	cargo build --release -p omnimodemd

tui: ## Build the TUI client → bin/omnimodem-tui
	cd $(TUI_DIR) && go build -o $(CURDIR)/$(TUI_BIN) ./cmd/omnimodem-tui

proto: ## Regenerate the TUI's Go bindings from proto/omnimodem.proto
	cd $(TUI_DIR) && ./gen.sh

test: test-modem test-tui ## Run all tests

test-modem: ## Rust workspace tests
	cargo test --workspace

test-tui: ## Go TUI tests
	cd $(TUI_DIR) && go test ./...

run-modem: modem ## Build + run the daemon (socket at $$TMPDIR/omnimodem/omnimodem.sock)
	$(MODEM_BIN)

run-tui: tui ## Build + run the TUI (auto-connects to the daemon's default socket)
	./$(TUI_BIN)

clean: ## Remove build artifacts
	cargo clean
	rm -rf bin

help: ## List targets
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "} {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
