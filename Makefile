.PHONY: validate fmt clippy test snapshots

validate: fmt clippy test

fmt:
	cargo fmt --all --check

clippy:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test --workspace

snapshots:
	cargo insta review
