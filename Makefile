.PHONY: build release dev check test clippy fmt clean run install dist smoke

# ── Configuration ──────────────────────────────────────────────────────
BINARY   := officecli
RELEASE  := target/release/$(BINARY)
DEBUG    := target/debug/$(BINARY)
DIST_DIR := dist

# ── Build ──────────────────────────────────────────────────────────────
dev:            ## Build debug binary
	cargo build

build:          ## Build release binary (alias: release)
	cargo build --release

release: build

check:          ## Fast compile check (no binary output)
	cargo check

# ── Test & Lint ────────────────────────────────────────────────────────
test:           ## Run all tests
	cargo test

clippy:         ## Run Clippy lints
	cargo clippy --all-targets -- -D warnings

fmt:            ## Check formatting (no auto-fix)
	cargo fmt -- --check

fmt-fix:        ## Auto-fix formatting
	cargo fmt

lint: fmt clippy  ## Run fmt + clippy

# ── Run ─────────────────────────────────────────────────────────────────
run: dev        ## Run debug binary (pass ARGS=... for CLI args)
	cargo run -- $(ARGS)

install: build  ## Install release binary to ~/.cargo/bin
	cargo install --path crates/officecli

# ── Distribution ───────────────────────────────────────────────────────
dist: build     ## Build + copy binary to dist/ with SHA256
	@mkdir -p $(DIST_DIR)
	@OS=$$(uname -s | tr '[:upper:]' '[:lower:]') && \
	ARCH=$$(uname -m) && \
	case "$$OS" in \
	  darwin) case "$$ARCH" in \
	    arm64) NAME=$(BINARY)-mac-arm64 ;; \
	    x86_64) NAME=$(BINARY)-mac-x64 ;; \
	    *) NAME=$(BINARY)-mac-$$ARCH ;; \
	  esac ;; \
	  linux) NAME=$(BINARY)-linux-$$ARCH ;; \
	  *) NAME=$(BINARY)-$$OS-$$ARCH ;; \
	esac && \
	cp $(RELEASE) $(DIST_DIR)/$$NAME && \
	chmod +x $(DIST_DIR)/$$NAME && \
	if [ "$$OS" = "darwin" ]; then codesign -s - -f $(DIST_DIR)/$$NAME 2>/dev/null || true; fi && \
	(cd $(DIST_DIR) && sha256sum $$NAME > SHA256SUMS || shasum -a 256 $$NAME > SHA256SUMS) && \
	echo "Built: $(DIST_DIR)/$$NAME" && cat $(DIST_DIR)/SHA256SUMS

smoke: build    ## Quick smoke test of release binary
	$(RELEASE) --version
	$(RELEASE) info
	$(RELEASE) create /tmp/smoke_test.docx && \
	$(RELEASE) view /tmp/smoke_test.docx --mode stats && \
	rm -f /tmp/smoke_test.docx
	@echo "Smoke test passed."

# ── Clean ──────────────────────────────────────────────────────────────
clean:          ## Remove build artifacts
	cargo clean
	rm -rf $(DIST_DIR)

# ── Help ───────────────────────────────────────────────────────────────
help:           ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-15s\033[0m %s\n", $$1, $$2}'