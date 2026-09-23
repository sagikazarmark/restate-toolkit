# Restate Toolkit

[![ci](https://img.shields.io/github/actions/workflow/status/sagikazarmark/restate-toolkit/dagger.yaml?style=flat-square&label=ci)](https://github.com/sagikazarmark/restate-toolkit/actions/workflows/dagger.yaml)
[![openssf scorecard](https://api.securityscorecards.dev/projects/github.com/sagikazarmark/restate-toolkit/badge?style=flat-square&label=openssf%20scorecard)](https://securityscorecards.dev/viewer/?uri=github.com/sagikazarmark/restate-toolkit)

Rust crates for building and testing [Restate](https://restate.dev/) services with the [Restate Rust SDK](https://docs.rs/restate-sdk).

## Crates

| Crate | Description | |
| ----- | ----------- | - |
| [`restate-config`](crates/restate-config) | Serde configuration types and SDK option adapters for Restate endpoints | [![crates.io](https://img.shields.io/crates/v/restate-config?style=flat-square)](https://crates.io/crates/restate-config) [![docs.rs](https://img.shields.io/docsrs/restate-config?style=flat-square)](https://docs.rs/restate-config) |
| [`restate-e2e-harness`](crates/restate-e2e-harness) | End-to-end test harness for Restate SDK endpoints against a real `restate-server` (Unix only) | [![crates.io](https://img.shields.io/crates/v/restate-e2e-harness?style=flat-square)](https://crates.io/crates/restate-e2e-harness) [![docs.rs](https://img.shields.io/docsrs/restate-e2e-harness?style=flat-square)](https://docs.rs/restate-e2e-harness) |

All crates are released together and share a version.

## Example

[`examples/greeter`](examples/greeter) is an endpoint configured with `restate-config` and tested
end-to-end with `restate-e2e-harness`. Its end-to-end test is ignored by default; run it with a
`restate-server` binary:

```shell
RESTATE_SERVER_BIN="$PWD/restate-server" cargo test -p greeter -- --ignored
```

## Development

Run the local Rust checks with:

```shell
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --doc
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
```

Run the same repository gate used by CI with:

```shell
dagger check
```

See the individual crate READMEs for crate-specific tests (e.g. the end-to-end tests of `restate-e2e-harness`).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
