//! Endpoint settings and application-defined service policy configuration.

use serde::{Deserialize, Serialize};

use crate::EndpointConfig;

/// Shared endpoint settings with an application-defined set of service policies.
///
/// `ServicesConfig` can be a struct with named service/object/workflow fields or a map.
/// Each application chooses its service names and policy layout. Deserialization
/// uses [`Default`] for omitted endpoint and service settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
#[serde(bound(deserialize = "ServicesConfig: Deserialize<'de> + Default"))]
pub struct Config<ServicesConfig> {
    /// Request-identity verification keys and the endpoint's listener address.
    pub endpoint: EndpointConfig,
    /// Application-specific policies for the services bound to this endpoint.
    pub services: ServicesConfig,
}

/// Invalid endpoint settings detected before serving requests.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// An identity verification key was rejected by the SDK.
    #[error("invalid request identity key at index {index}: {reason}")]
    IdentityKey {
        /// Zero-based position in `identity_keys`.
        index: usize,
        /// The SDK's key-validation error.
        reason: String,
    },
    /// A handler override does not name a handler on the service.
    #[error("unknown handler {handler:?} on service {service:?}")]
    UnknownHandler {
        /// The discovered service name.
        service: String,
        /// The unrecognized, case-sensitive handler name.
        handler: String,
    },
}
