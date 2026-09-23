# Restate Config

[![ci](https://img.shields.io/github/actions/workflow/status/sagikazarmark/restate-toolkit/dagger.yaml?style=flat-square&label=ci)](https://github.com/sagikazarmark/restate-toolkit/actions/workflows/dagger.yaml)
[![openssf scorecard](https://api.securityscorecards.dev/projects/github.com/sagikazarmark/restate-toolkit/badge?style=flat-square&label=openssf%20scorecard)](https://securityscorecards.dev/viewer/?uri=github.com/sagikazarmark/restate-toolkit)
[![crates.io](https://img.shields.io/crates/v/restate-config?style=flat-square)](https://crates.io/crates/restate-config)
[![docs.rs](https://img.shields.io/docsrs/restate-config?style=flat-square)](https://docs.rs/restate-config)

Serde configuration types and SDK option adapters for [Restate](https://restate.dev/) endpoints.

Configure endpoint identity verification, service policies, and handler overrides using human-readable durations and your preferred configuration source. Unset policy fields preserve existing SDK/service settings, while explicit `false` and zero values are retained.

The crate integrates with the Restate SDK's endpoint builder. Your application owns configuration loading, service binding, the HTTP server lifecycle, shutdown, and logging.

## Installation

Requires Rust **1.92 or later** and Restate SDK **0.12**.

```toml
[dependencies]
restate-config = "0.1"
restate-sdk = "0.12"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

`serde_json` is used in the example below. The configuration types work with any Serde-compatible source, including configuration loaders such as Figment.

## Usage

Define your application's service configuration, deserialize it, and apply each policy when binding its service:

```rust
use restate_config::{Config, ConfigureEndpointExt, ServiceOptionsConfig};
use restate_sdk::prelude::*;
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServicesConfig {
    notifications: ServiceOptionsConfig,
}

struct Notifications;

#[restate_sdk::service]
impl Notifications {
    #[restate_sdk::handler]
    async fn send(&self, _: Context<'_>) -> Result<(), HandlerError> {
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config<ServicesConfig> = serde_json::from_str(r#"
        {
            "endpoint": {
                "listener": "127.0.0.1:9080"
            },
            "services": {
                "notifications": {
                    "inactivity_timeout": "30s",
                    "retry_policy_initial_interval": "500ms",
                    "retry_policy_max_attempts": 5,
                    "retry_policy_on_max_attempts": "pause",
                    "handlers": {
                        "send": {
                            "abort_timeout": "5s"
                        }
                    }
                }
            }
        }
    "#)?;

    let endpoint = Endpoint::builder()
        .configure(&config.endpoint)?
        .bind(config.services.notifications.apply(Notifications)?)
        .build();

    // Bind config.endpoint.listener and serve endpoint using the SDK's HttpServer.
    Ok(())
}
```

`ServiceOptionsConfig::apply` works with services, virtual objects, and workflows. It checks handler override names before returning an SDK service definition for `.bind(...)`. `ConfigureEndpointExt::configure` applies the endpoint-wide settings, such as identity keys, to the builder.

The fields in `ServicesConfig` are application-defined: they do not need to match discovered service names. Use a struct for a fixed set of services, or `Config<BTreeMap<String, ServiceOptionsConfig>>` for a dynamic set. Handler override keys must match the exact, case-sensitive discovered handler names.

## Configuration

All built-in configuration types support Serde serialization and deserialization and reject unknown fields. Omitted endpoint settings use their defaults; omitted service and handler policy fields leave the existing policy unchanged. Your application's service configuration controls its own defaults and unknown-field handling, as shown above.

### Endpoint settings

| Field | Default | Description |
| --- | --- | --- |
| `listener` | `0.0.0.0:9080` | IP address and port to bind. IPv6 is supported; port `0` requests an ephemeral port. |
| `identity_keys` | `[]` | Trusted Restate public verification keys in `publickeyv1_...` format. |

`identity_keys` accepts either a list of strings or a comma/whitespace-delimited string. Multiple keys support key rotation. `EndpointConfig::apply` adds these keys to an existing SDK builder and returns `ConfigError::IdentityKey` if a key is invalid. An empty list leaves the builder's identity configuration unchanged.

The listener address is configuration only: your application binds it when starting the HTTP server.

### Service and handler policies

These fields are available on both `ServiceOptionsConfig` and `HandlerOptionsConfig`:

| Field | Value | Description |
| --- | --- | --- |
| `metadata` | String-to-string map | Metadata published in discovery. |
| `inactivity_timeout` | Duration | Time before Restate asks an inactive invocation to suspend. |
| `abort_timeout` | Duration | Grace period for suspension before aborting an invocation. |
| `idempotency_retention` | Duration | Retention of idempotent invocation results. |
| `journal_retention` | Duration | Retention of completed invocation journals. |
| `enable_lazy_state` | Boolean | Enable lazy state for virtual objects and workflows. |
| `ingress_private` | Boolean | Restrict invocation to other Restate services rather than public ingress. |
| `retry_policy_initial_interval` | Duration | Delay before the first invocation retry. |
| `retry_policy_exponentiation_factor` | Float | Multiplier between successive retry intervals. |
| `retry_policy_max_interval` | Duration | Maximum invocation retry delay. |
| `retry_policy_max_attempts` | Unsigned integer | Maximum attempts, including the initial invocation. |
| `retry_policy_on_max_attempts` | `"pause"` or `"kill"` | Pause for manual intervention or fail the invocation when attempts are exhausted. |

Durations use human-readable strings such as `"500ms"`, `"30s"`, `"1h"`, or `"2days"`.

Service policies also have a `handlers` map of handler names to `HandlerOptionsConfig`. Handler policies additionally support `workflow_retention`, a duration controlling workflow result retention on the workflow entrypoint.

### Direct SDK option conversion

Both policy types convert into their corresponding SDK options with `.into()`. When attaching options to an SDK service definition directly, validate handler names first:

```rust,ignore
use restate_sdk::service::IntoServiceDefinition;

let policy = config.services.notifications;
policy.validate_handlers::<Notifications>()?;
let definition = Notifications.into_service_definition().options(policy.into());
let builder = Endpoint::builder().bind(definition);
```

`ServiceOptionsConfig::apply` performs this validation automatically, returning `ConfigError::UnknownHandler` for an unrecognized handler. Direct conversion alone does not validate names.

See the [API documentation](https://docs.rs/restate-config) for details.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
