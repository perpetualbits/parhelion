# Parhelion — developer task runner.
#
# These targets mirror the CI pipeline. `test` is the completion gate CLAUDE.md
# requires before any task is declared done (workspace tests + clippy). Comments
# are per CLAUDE.md's rule that all code, Makefiles included, is documented.

# Build every crate in the workspace.
build:
	cargo build --workspace

# The completion gate: run all workspace tests, then run clippy with warnings
# denied — the "CI profile" where the workspace's warn-level lints (see
# Cargo.toml [workspace.lints]) become hard failures. A warning here fails the
# build even though a plain local `cargo build` only warns.
test:
	cargo test --workspace
	cargo clippy --workspace --all-targets -- -D warnings

# Format all workspace code in place with rustfmt.
fmt:
	cargo fmt --all

# None of these targets produce a file of the same name.
.PHONY: build test fmt
