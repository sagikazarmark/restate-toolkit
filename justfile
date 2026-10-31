# List the recipes.
[private]
default:
    @just --list --unsorted

# Run all CI jobs.
ci: fmt check clippy test e2e doc build audit

# Build the workspace.
build:
    cargo build --workspace --all-features --locked

# Check formatting.
fmt:
    cargo fmt --all --check

# Check the workspace.
check:
    cargo hack check --workspace --feature-powerset --all-targets --locked

# Run clippy.
clippy:
    cargo hack clippy --workspace --each-feature --all-targets --locked -- --deny warnings

# Run tests.
test:
    cargo hack test --workspace --each-feature --locked

# `--tests` leaves out the doctests: `--ignored` would run their `ignore` blocks.

# Run the end-to-end tests against a real restate-server (RESTATE_SERVER_BIN, set by devenv).
e2e:
    cargo test --workspace --all-features --tests --locked -- --ignored

# Run rustdoc.
[env("RUSTDOCFLAGS", "--deny warnings")]
doc:
    cargo hack doc --workspace --each-feature --no-deps --locked

# Audit Cargo.lock for security advisories (ignores in .cargo/audit.toml).
audit:
    cargo audit
