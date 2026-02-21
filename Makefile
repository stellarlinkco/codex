OUT ?= /Users/chenwenjie/bin/codex

.PHONY: release-codex
release-codex:
	env -u CARGO_PROFILE_RELEASE_LTO \
		-u CARGO_PROFILE_RELEASE_CODEGEN_UNITS \
		-u CARGO_PROFILE_RELEASE_DEBUG \
		-u CARGO_PROFILE_RELEASE_STRIP \
		sh -c 'cd codex-rs && cargo build -p codex-cli --bin codex --release'
	mkdir -p "$(dir $(OUT))"
	install -m 755 codex-rs/target/release/codex "$(OUT)"
