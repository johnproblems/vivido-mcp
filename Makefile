.PHONY: fmt lint test test-headless build check clean

fmt:
	cargo fmt

lint:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test --locked

# Real headless coverage. Needs the Vivido binary (VIVIDO_BIN or `vivido` on
# PATH) with a wgpu adapter, hardware or llvmpipe.
test-headless:
	VIVIDO_BIN="$${VIVIDO_BIN:-vivido}" cargo test --locked --test headless_real -- --ignored --test-threads=1

build:
	cargo build --locked

check: fmt lint test
	cargo fmt --check

clean:
	cargo clean
