PREFIX ?= /usr/local
BINDIR  = $(DESTDIR)$(PREFIX)/bin
BIN     = kudzu
TARGET  = target/release/$(BIN)

.PHONY: all build install uninstall clean check-rust ci fmt clippy test bump-version

all: build

check-rust:
	@if ! command -v cargo >/dev/null 2>&1; then \
		if [ -f "$$HOME/.cargo/env" ]; then \
			echo "Found ~/.cargo/env, sourcing it for this build."; \
		else \
			echo "cargo not found. Installing Rust via rustup..."; \
			if ! command -v curl >/dev/null 2>&1; then \
				echo "Error: curl is required to install Rust. Please install curl first." >&2; \
				exit 1; \
			fi; \
			curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal; \
		fi; \
	fi

build: check-rust
	@. "$$HOME/.cargo/env" 2>/dev/null; cargo build --release

ci: fmt clippy test

fmt:
	cargo fmt --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test

bump-version:
	@if [ -z "$(bump)" ]; then \
		echo "Usage: make bump-version bump=patch|minor|major [tag=1] [no_verify=1]" >&2; \
		exit 2; \
	fi
	./scripts/bump-version "$(bump)" $(if $(tag),--tag,) $(if $(no_verify),--no-verify,)

install: build
	install -d $(BINDIR)
	install -m 0755 $(TARGET) $(BINDIR)/$(BIN)
	ln -sf $(BIN) $(BINDIR)/kz
	@echo "Installed $(BINDIR)/$(BIN) (with kz symlink)"

uninstall:
	rm -f $(BINDIR)/$(BIN) $(BINDIR)/kz
	@echo "Removed $(BINDIR)/$(BIN) and kz"

clean:
	@. "$$HOME/.cargo/env" 2>/dev/null; cargo clean
