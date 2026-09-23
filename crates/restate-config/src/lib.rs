//! Configuration types and option adapters for Restate SDK 0.12 endpoints.
//!
//! [`ServiceOptionsConfig`] and [`HandlerOptionsConfig`] preserve unset SDK
//! policy, accept human-readable durations, and work with any Serde source.
//! [`Config`] groups application-defined service policies with endpoint
//! identity and listener settings. [`EndpointConfig`] applies verification keys
//! to an existing SDK builder, as `builder.configure(&config.endpoint)?` with
//! [`ConfigureEndpointExt`] in scope. Keep using `Endpoint::builder()`,
//! `.bind(...)`, and `HttpServer` directly. Attach policy with `config.apply(service)?`, or use
//! `.options(config.into())` when working with SDK service definitions.
//!
//! Applications own configuration sources, the server lifecycle, shutdown
//! signals, and logging setup. The models work with Figment through Serde.
//!
//! ```
//! use restate_config::{Config, ConfigureEndpointExt, ServiceOptionsConfig};
//! use restate_sdk::prelude::*;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Default, Deserialize, Serialize)]
//! #[serde(default, deny_unknown_fields)]
//! struct ServicesConfig {
//!     example: ServiceOptionsConfig,
//! }
//!
//! struct Example;
//! #[restate_sdk::service]
//! impl Example {
//!     #[restate_sdk::handler]
//!     async fn greet(&self, _: Context<'_>) -> Result<(), HandlerError> {
//!         Ok(())
//!     }
//! }
//!
//! # fn main() -> Result<(), restate_config::ConfigError> {
//! let config = Config::<ServicesConfig>::default();
//! let endpoint = Endpoint::builder()
//!     .configure(&config.endpoint)?
//!     .bind(config.services.example.apply(Example)?)
//!     .build();
//! // Bind config.endpoint.listener and serve using the SDK's HttpServer.
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod config;
mod endpoint;
mod options;

pub use config::{Config, ConfigError};
pub use endpoint::{ConfigureEndpointExt, EndpointConfig};
pub use options::{HandlerOptionsConfig, RetryPolicyOnMaxAttempts, ServiceOptionsConfig};
