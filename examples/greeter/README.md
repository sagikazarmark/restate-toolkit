# Greeter Example

A small [Restate](https://restate.dev/) endpoint that shows the toolkit crates working together:

- [`restate-config`](../../crates/restate-config) configures the endpoint and the `Greeter` service
  ([`src/lib.rs`](src/lib.rs)). The configuration is built in code for brevity; the same `Config` type
  deserializes from any Serde source (JSON, TOML, Figment, ...). It sets service metadata, journal
  retention and a retry policy, and marks the `audit` handler ingress-private.
- [`restate-e2e-harness`](../../crates/restate-e2e-harness) deploys that endpoint to a real
  `restate-server` ([`tests/e2e.rs`](tests/e2e.rs)), invokes it through the ingress, reads the retained
  journal, and checks that the configured policies reached the server's discovery (`GET /services/Greeter`)
  and that the private handler is refused at the ingress.

## Running the endpoint

```shell
cargo run -p greeter
```

The endpoint listens on `0.0.0.0:9080`. Register it with a running Restate server and call it:

```shell
restate deployments register http://localhost:9080
curl localhost:8080/Greeter/greet --json '"Ada"'
```

## Running the end-to-end test

The test is ignored by default because it needs a `restate-server` binary (see
[Getting a `restate-server`](../../crates/restate-e2e-harness/README.md#getting-a-restate-server)).
Point `RESTATE_SERVER_BIN` at it and include ignored tests:

```shell
RESTATE_SERVER_BIN="$PWD/restate-server" cargo test -p greeter -- --ignored
```

The harness spawns a server of its own on free loopback ports and stops it when the test ends.
Verified with Restate server **1.7.8**. Unix only.

## Dagger

This directory is a Dagger workspace of its own ([`dagger.toml`](dagger.toml)), with its own CI workflow
([`dagger-examples-greeter.yaml`](../../.github/workflows/dagger-examples-greeter.yaml)).
Run its checks from here:

```shell
dagger check
```

The `rust` module runs fmt, clippy, the tests and docs for this crate, on the container of the repository's
[`rust-setup`](../../.dagger/modules/rust-setup/main.dang) module: the Rust image with `restate-server`
copied from the Restate image and selected with `RESTATE_SERVER_BIN`. The crate's manifest configures the
module's `test` to pass `--include-ignored` to the test binaries (`hack.test = ["--", "--include-ignored"]`
in `[[package.metadata.dagger.modules]]`), so `rust:test` runs the end-to-end test too.
