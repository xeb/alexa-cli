.PHONY: build test install fmt clippy run

build:
	cargo build --release

test:
	cargo test

install:
	cargo install --path .

fmt:
	cargo fmt

clippy:
	cargo clippy -- -D warnings

run:
	cargo run --
