//! An example Restate endpoint wired with `restate-config`.
//!
//! [`config`] builds the endpoint and service policies in code. A real
//! application would typically deserialize the same [`Config`] from a file or
//! the environment (for example with Figment); the types are plain Serde
//! models, so the shape is identical either way.
//!
//! [`endpoint`] turns that configuration into an SDK `Endpoint`, which the
//! binary serves and the end-to-end test deploys against a real server.

use std::{collections::BTreeMap, time::Duration};

use restate_config::{
    Config, ConfigError, HandlerOptionsConfig, RetryPolicyOnMaxAttempts, ServiceOptionsConfig,
};
use restate_sdk::prelude::*;
use serde::{Deserialize, Serialize};

/// The services this endpoint binds, each with its own policy.
///
/// Field names are the application's choice; they do not have to match the
/// discovered service names.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServicesConfig {
    /// Policy for the [`Greeter`] service.
    pub greeter: ServiceOptionsConfig,
}

/// A greeting service with a public and an internal handler.
pub struct Greeter;

#[restate_sdk::service(name = "Greeter")]
impl Greeter {
    /// Greets `name`, journaling the greeting as the `greet-person` run.
    #[handler]
    async fn greet(&self, ctx: Context<'_>, name: String) -> HandlerResult<String> {
        let greeting = ctx
            .run(|| async move { Ok(format!("Hello, {name}!")) })
            .name("greet-person")
            .await?;
        Ok(greeting)
    }

    /// Only callable by other Restate services: configuration marks it
    /// ingress-private.
    #[handler]
    async fn audit(&self, ctx: Context<'_>, name: String) -> HandlerResult<String> {
        let entry = ctx
            .run(|| async move { Ok(format!("audited {name}")) })
            .name("audit-person")
            .await?;
        Ok(entry)
    }
}

/// The endpoint configuration, built in code.
#[must_use]
pub fn config() -> Config<ServicesConfig> {
    Config {
        // Listener 0.0.0.0:9080, no identity keys.
        endpoint: restate_config::EndpointConfig::default(),
        services: ServicesConfig {
            greeter: ServiceOptionsConfig {
                metadata: BTreeMap::from([("team".to_owned(), "greetings".to_owned())]),
                // Keep completed journals so their runs can be inspected.
                journal_retention: Some(Duration::from_hours(24)),
                retry_policy_initial_interval: Some(Duration::from_millis(500)),
                retry_policy_max_attempts: Some(5),
                retry_policy_on_max_attempts: Some(RetryPolicyOnMaxAttempts::Pause),
                handlers: BTreeMap::from([(
                    "audit".to_owned(),
                    HandlerOptionsConfig {
                        ingress_private: Some(true),
                        ..HandlerOptionsConfig::default()
                    },
                )]),
                ..ServiceOptionsConfig::default()
            },
        },
    }
}

/// Binds every configured service and applies the endpoint settings.
///
/// The listener address is not part of the SDK endpoint: the caller binds
/// [`restate_config::EndpointConfig::listener`] when serving.
///
/// # Errors
/// Returns [`ConfigError`] for an unknown handler override or an invalid
/// identity key.
pub fn endpoint(config: Config<ServicesConfig>) -> Result<Endpoint, ConfigError> {
    let builder = Endpoint::builder().bind(config.services.greeter.apply(Greeter)?);
    Ok(config.endpoint.apply(builder)?.build())
}
