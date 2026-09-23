use std::net::SocketAddr;

use restate_sdk::endpoint::Builder;
use serde::{Deserialize, Deserializer, Serialize};

use crate::ConfigError;

/// Endpoint-wide request identity and listener settings, independent of service policies.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct EndpointConfig {
    /// TCP address to bind, default `0.0.0.0:9080`.
    ///
    /// Port zero requests an ephemeral port. The application binds the listener
    /// and passes it to the SDK's HTTP server.
    pub listener: SocketAddr,
    /// Trusted Restate public verification keys (`publickeyv1_...`).
    ///
    /// Accepts a list or a comma/whitespace-delimited string. Multiple keys
    /// support rotation; an empty list leaves the SDK builder unchanged.
    #[serde(
        deserialize_with = "identity_keys",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub identity_keys: Vec<String>,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            listener: SocketAddr::from(([0, 0, 0, 0], 9080)),
            identity_keys: Vec::new(),
        }
    }
}

impl EndpointConfig {
    /// Adds the configured verification keys to a caller-owned SDK builder.
    /// The caller separately binds [`Self::listener`] when starting the server.
    ///
    /// # Errors
    /// Returns [`ConfigError::IdentityKey`] if any key is invalid.
    pub fn apply(&self, mut builder: Builder) -> Result<Builder, ConfigError> {
        for (index, key) in self.identity_keys.iter().enumerate() {
            builder = builder
                .identity_key(key)
                .map_err(|error| ConfigError::IdentityKey {
                    index,
                    reason: error.to_string(),
                })?;
        }
        Ok(builder)
    }
}

fn identity_keys<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Keys {
        List(Vec<String>),
        Delimited(String),
    }

    Ok(match Keys::deserialize(deserializer)? {
        Keys::List(keys) => keys,
        Keys::Delimited(keys) => keys
            .split(|character: char| character == ',' || character.is_whitespace())
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
            .collect(),
    })
}
