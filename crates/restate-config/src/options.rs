use std::{collections::BTreeMap, time::Duration};

use restate_sdk::{
    endpoint::{HandlerOptions, ServiceOptions},
    service::{Discoverable, IntoServiceDefinition, ServiceDefinition},
};
use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// Service policy overrides. Unset fields leave the SDK/service policy alone.
///
/// The flat field names match the existing Rust endpoint configuration files.
/// Durations are strings such as `"500ms"`, `"10s"`, or `"1day"`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServiceOptionsConfig {
    /// Metadata published in discovery.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Time before Restate asks an inactive invocation to suspend.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub inactivity_timeout: Option<Duration>,
    /// Time to allow graceful suspension before aborting an invocation.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub abort_timeout: Option<Duration>,
    /// Retention of idempotent invocation results.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub idempotency_retention: Option<Duration>,
    /// Retention of completed invocation journals.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub journal_retention: Option<Duration>,
    /// Enable lazy state for virtual objects and workflows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_lazy_state: Option<bool>,
    /// Restrict invocation to other Restate services rather than public ingress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress_private: Option<bool>,
    /// Delay before the first invocation retry.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub retry_policy_initial_interval: Option<Duration>,
    /// Multiplier between successive retry intervals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_policy_exponentiation_factor: Option<f64>,
    /// Maximum invocation retry delay.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub retry_policy_max_interval: Option<Duration>,
    /// Maximum attempts, including the initial invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_policy_max_attempts: Option<u64>,
    /// Action when the retry attempt budget is exhausted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_policy_on_max_attempts: Option<RetryPolicyOnMaxAttempts>,
    /// Overrides keyed by exact, case-sensitive discovered handler names.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub handlers: BTreeMap<String, HandlerOptionsConfig>,
}

/// Handler policy overrides, including workflow-only result retention.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HandlerOptionsConfig {
    /// Metadata published in discovery.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Time before Restate asks an inactive invocation to suspend.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub inactivity_timeout: Option<Duration>,
    /// Time to allow graceful suspension before aborting an invocation.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub abort_timeout: Option<Duration>,
    /// Retention of idempotent invocation results.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub idempotency_retention: Option<Duration>,
    /// Retention of workflow results (workflow entrypoint only).
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub workflow_retention: Option<Duration>,
    /// Retention of completed invocation journals.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub journal_retention: Option<Duration>,
    /// Restrict invocation to other Restate services rather than public ingress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingress_private: Option<bool>,
    /// Enable lazy state for virtual objects and workflows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_lazy_state: Option<bool>,
    /// Delay before the first invocation retry.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub retry_policy_initial_interval: Option<Duration>,
    /// Multiplier between successive retry intervals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_policy_exponentiation_factor: Option<f64>,
    /// Maximum invocation retry delay.
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none")]
    pub retry_policy_max_interval: Option<Duration>,
    /// Maximum attempts, including the initial invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_policy_max_attempts: Option<u64>,
    /// Action when the retry attempt budget is exhausted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_policy_on_max_attempts: Option<RetryPolicyOnMaxAttempts>,
}

/// Restate's action when the configured retry budget is exhausted.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RetryPolicyOnMaxAttempts {
    /// Pause for manual intervention.
    Pause,
    /// Fail the invocation.
    Kill,
}

impl ServiceOptionsConfig {
    /// Validates and attaches options to a service, virtual object, or workflow.
    ///
    /// Returns an SDK service definition for
    /// `Endpoint::builder().bind(config.apply(service)?)`. The caller retains
    /// endpoint composition and serving; conversion to SDK options happens here.
    ///
    /// # Errors
    /// Returns [`ConfigError::UnknownHandler`] for an unknown handler name.
    pub fn apply<S: Discoverable + IntoServiceDefinition>(
        self,
        service: S,
    ) -> Result<ServiceDefinition, ConfigError> {
        self.validate_handlers::<S>()?;
        Ok(service.into_service_definition().options(self.into()))
    }

    /// Checks overrides against a macro-generated service/object/workflow.
    ///
    /// Call before `.options(config.into())` to turn handler-name typos into
    /// startup errors instead of the SDK's binding panic. This does not create
    /// or bind a service; endpoint composition remains in the caller.
    /// [`Self::apply`] performs this check automatically.
    ///
    /// # Errors
    /// Returns [`ConfigError::UnknownHandler`] for an unknown handler name.
    pub fn validate_handlers<S: Discoverable>(&self) -> Result<(), ConfigError> {
        let discovery = S::discover();
        for name in self.handlers.keys() {
            if !discovery
                .handlers
                .iter()
                .any(|handler| handler.name.as_str() == name)
            {
                return Err(ConfigError::UnknownHandler {
                    service: discovery.name.to_string(),
                    handler: name.clone(),
                });
            }
        }
        Ok(())
    }
}

// Both SDK option types expose the same setters. Keep that forwarding in one
// place without changing the existing flat service/handler configuration shape.
macro_rules! apply_common_options {
    ($config:ident, $options:ident) => {
        for (key, value) in $config.metadata {
            $options = $options.metadata(key, value);
        }
        if let Some(value) = $config.inactivity_timeout {
            $options = $options.inactivity_timeout(value);
        }
        if let Some(value) = $config.abort_timeout {
            $options = $options.abort_timeout(value);
        }
        if let Some(value) = $config.idempotency_retention {
            $options = $options.idempotency_retention(value);
        }
        if let Some(value) = $config.journal_retention {
            $options = $options.journal_retention(value);
        }
        if let Some(value) = $config.enable_lazy_state {
            $options = $options.enable_lazy_state(value);
        }
        if let Some(value) = $config.ingress_private {
            $options = $options.ingress_private(value);
        }
        if let Some(value) = $config.retry_policy_initial_interval {
            $options = $options.retry_policy_initial_interval(value);
        }
        if let Some(value) = $config.retry_policy_exponentiation_factor {
            $options = $options.retry_policy_exponentiation_factor(value);
        }
        if let Some(value) = $config.retry_policy_max_interval {
            $options = $options.retry_policy_max_interval(value);
        }
        if let Some(value) = $config.retry_policy_max_attempts {
            $options = $options.retry_policy_max_attempts(value);
        }
        if let Some(value) = $config.retry_policy_on_max_attempts {
            $options = match value {
                RetryPolicyOnMaxAttempts::Pause => $options.retry_policy_pause_on_max_attempts(),
                RetryPolicyOnMaxAttempts::Kill => $options.retry_policy_kill_on_max_attempts(),
            };
        }
    };
}

/// Converts to SDK options for `.options(config.into())`. Call
/// [`ServiceOptionsConfig::validate_handlers`] first to check handler names.
impl From<ServiceOptionsConfig> for ServiceOptions {
    fn from(config: ServiceOptionsConfig) -> Self {
        let mut options = Self::new();
        apply_common_options!(config, options);
        for (name, handler) in config.handlers {
            options = options.handler(name, handler.into());
        }
        options
    }
}

impl From<HandlerOptionsConfig> for HandlerOptions {
    fn from(config: HandlerOptionsConfig) -> Self {
        let mut options = Self::new();
        apply_common_options!(config, options);
        if let Some(retention) = config.workflow_retention {
            options = options.workflow_retention(retention);
        }
        options
    }
}
