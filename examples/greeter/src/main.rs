//! Serves the Greeter endpoint the way a production service would: with logging that
//! survives replays, and a graceful stop on SIGTERM as well as Ctrl-C.

use std::{collections::BTreeMap, time::Duration};

use greeter::Greeter;
use restate_config::{
    Config, ConfigError, ConfigureEndpointExt, EndpointConfig, RetryPolicyOnMaxAttempts,
    ServiceOptionsConfig,
};
use restate_ext::shutdown_signal;
use restate_sdk::{endpoint::Endpoint, filter::ReplayAwareFilter, http_server::HttpServer};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Logging comes first, so everything after it (including the SDK's own lines) is captured.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                // `RUST_LOG` picks what gets logged; without it, `info` and above.
                .with_filter(
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
                )
                // Restate re-runs handler code while replaying a journal. This filter
                // drops the lines a replay re-emits, so each line is logged once.
                .with_filter(ReplayAwareFilter),
        )
        .init();

    // Configuration: where to listen, which requests to trust, and each service's policy.
    let config = config();

    // Bind the listener ourselves rather than through `listen_and_serve`: that way a
    // taken port is reported as an error instead of a panic, and we can pass our own
    // shutdown signal to the server below.
    let listener = TcpListener::bind(&config.endpoint.listener).await?;
    info!(address = %listener.local_addr()?, "serving the Greeter endpoint");

    // The SDK endpoint: every service bound with its configured policy. This fails on an
    // invalid configuration (an unknown handler override, a malformed identity key), so
    // the process exits before it ever accepts a request.
    let endpoint = endpoint(config)?;

    // Serve until asked to stop. The SDK alone only reacts to Ctrl-C (SIGINT);
    // `shutdown_signal` also resolves on SIGTERM, which is what `docker stop` and
    // Kubernetes send. Either one starts a graceful drain: the listener closes, open
    // invocations get up to ten seconds to finish, and Restate retries the rest.
    HttpServer::new(endpoint)
        .serve_with_cancel(listener, shutdown_signal())
        .await;

    info!("stopped");
    Ok(())
}

/// The endpoint configuration, built in code to keep the example self-contained.
///
/// A real service would load it instead, typically with a configuration library such as
/// [Figment](https://docs.rs/figment) merging a file with environment overrides:
///
/// ```ignore
/// let config: Config<ServicesConfig> = Figment::new()
///     .merge(Toml::file("greeter.toml"))
///     .merge(Env::prefixed("GREETER_").split("__"))
///     .extract()?;
/// ```
///
/// The `restate-config` types are plain Serde models, so the configuration has the same
/// shape whichever way it is built, and durations read as `"500ms"` or `"1m"`.
fn config() -> Config<ServicesConfig> {
    Config {
        // Listen on 0.0.0.0:9080 and accept requests without verifying their identity.
        // In production, set `identity_keys` so only your Restate server can call in.
        endpoint: EndpointConfig::default(),
        services: ServicesConfig {
            greeter: ServiceOptionsConfig {
                // Free-form labels, published with the service in Restate's discovery.
                metadata: BTreeMap::from([("team".to_owned(), "greetings".to_owned())]),
                // Retry a failing invocation from 500ms, and pause it after five attempts
                // rather than failing it, so an operator can fix the cause and resume.
                retry_policy_initial_interval: Some(Duration::from_millis(500)),
                retry_policy_max_attempts: Some(5),
                retry_policy_on_max_attempts: Some(RetryPolicyOnMaxAttempts::Pause),
                ..ServiceOptionsConfig::default()
            },
        },
    }
}

/// The services this endpoint binds, each with its own policy: the `services` half of the
/// configuration.
///
/// Field names are the application's choice; they do not have to match the discovered
/// service names. `deny_unknown_fields` turns a misspelled service in the configuration
/// file into a startup error instead of a silently ignored section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct ServicesConfig {
    /// Policy for the [`Greeter`] service.
    greeter: ServiceOptionsConfig,
}

/// Builds the SDK endpoint: applies the endpoint-wide settings, then binds every service
/// with its configured policy. To serve another service, add a `.bind(...)`.
///
/// The listener address is not part of the SDK endpoint: `main` binds it when serving.
///
/// # Errors
/// Returns [`ConfigError`] for an unknown handler override or an invalid identity key.
fn endpoint(config: Config<ServicesConfig>) -> Result<Endpoint, ConfigError> {
    Ok(Endpoint::builder()
        .configure(&config.endpoint)?
        .bind(config.services.greeter.apply(Greeter)?)
        .build())
}
