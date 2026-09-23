#![expect(
    clippy::unused_async,
    reason = "SDK handlers require async signatures; these handlers only supply discovery metadata"
)]

use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use restate_config::{
    Config, ConfigError, ConfigureEndpointExt, EndpointConfig, ServiceOptionsConfig,
};
use restate_sdk::{prelude::*, service::IntoServiceDefinition};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

struct Example;

#[restate_sdk::object]
impl Example {
    #[restate_sdk::handler]
    async fn fetch_info(&self, _: ObjectContext<'_>) -> Result<(), HandlerError> {
        Ok(())
    }
}

struct Workflow;

#[restate_sdk::workflow]
impl Workflow {
    #[restate_sdk::handler]
    async fn run(&self, _: WorkflowContext<'_>) -> Result<(), HandlerError> {
        Ok(())
    }
}

struct Notifications;

#[restate_sdk::service]
impl Notifications {
    #[restate_sdk::handler]
    async fn send(&self, _: Context<'_>) -> Result<(), HandlerError> {
        Ok(())
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct ServicesConfig {
    notifications: ServiceOptionsConfig,
    accounts: ServiceOptionsConfig,
    checkout: ServiceOptionsConfig,
}

async fn discovery(endpoint: Endpoint) -> Value {
    let response = endpoint.handle(
        http::Request::builder()
            .uri("/discover")
            .header("accept", "application/vnd.restate.endpointmanifest.v4+json")
            .body(Empty::<Bytes>::new())
            .unwrap(),
    );
    assert_eq!(response.status(), 200);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn defaults_preserve_the_service_definition_and_do_not_publish_policy() {
    let definition = Example.into_service_definition().options(
        ServiceOptions::new()
            .inactivity_timeout(Duration::from_secs(42))
            .ingress_private(true),
    );
    let original = discovery(Endpoint::builder().bind(definition).build()).await;
    let definition = Example.into_service_definition().options(
        ServiceOptions::new()
            .inactivity_timeout(Duration::from_secs(42))
            .ingress_private(true),
    );
    let defaults: ServiceOptionsConfig = serde_json::from_value(json!({})).unwrap();
    assert_eq!(serde_json::to_value(&defaults).unwrap(), json!({}));
    let configured = discovery(
        Endpoint::builder()
            .bind(definition.options(defaults.into()))
            .build(),
    )
    .await;
    assert_eq!(original, configured);
    assert!(
        configured["services"][0]
            .get("retryPolicyMaxAttempts")
            .is_none()
    );
}

#[tokio::test]
async fn service_and_handler_policies_reach_discovery_with_explicit_false_and_zero() {
    let options: ServiceOptionsConfig = serde_json::from_value(json!({
        "metadata": {"owner": "platform"},
        "inactivity_timeout": "30s",
        "abort_timeout": "5s",
        "idempotency_retention": "1day",
        "journal_retention": "1h",
        "enable_lazy_state": true,
        "ingress_private": true,
        "retry_policy_initial_interval": "500ms",
        "retry_policy_exponentiation_factor": 2.5,
        "retry_policy_max_interval": "10s",
        "retry_policy_max_attempts": 5,
        "retry_policy_on_max_attempts": "pause",
        "handlers": {"fetch_info": {
            "metadata": {"owner": "reader"},
            "inactivity_timeout": "10s",
            "abort_timeout": "1s",
            "idempotency_retention": "0s",
            "journal_retention": "0s",
            "enable_lazy_state": false,
            "ingress_private": false,
            "retry_policy_initial_interval": "100ms",
            "retry_policy_exponentiation_factor": 1.0,
            "retry_policy_max_interval": "2s",
            "retry_policy_max_attempts": 1,
            "retry_policy_on_max_attempts": "kill"
        }}
    }))
    .unwrap();
    options.validate_handlers::<Example>().unwrap();
    let endpoint = Endpoint::builder()
        .bind(Example.into_service_definition().options(options.into()))
        .build();
    let manifest = discovery(endpoint).await;
    let service = &manifest["services"][0];
    let handler = &service["handlers"][0];
    for (field, service_value, handler_value) in [
        (
            "metadata",
            json!({"owner": "platform"}),
            json!({"owner": "reader"}),
        ),
        ("inactivityTimeout", json!(30_000), json!(10_000)),
        ("abortTimeout", json!(5_000), json!(1_000)),
        ("idempotencyRetention", json!(86_400_000), json!(0)),
        ("journalRetention", json!(3_600_000), json!(0)),
        ("enableLazyState", json!(true), json!(false)),
        ("ingressPrivate", json!(true), json!(false)),
        ("retryPolicyInitialInterval", json!(500), json!(100)),
        ("retryPolicyExponentiationFactor", json!(2.5), json!(1.0)),
        ("retryPolicyMaxInterval", json!(10_000), json!(2_000)),
        ("retryPolicyMaxAttempts", json!(5), json!(1)),
        ("retryPolicyOnMaxAttempts", json!("PAUSE"), json!("KILL")),
    ] {
        assert_eq!(service[field], service_value, "service {field}");
        assert_eq!(handler[field], handler_value, "handler {field}");
    }
}

#[tokio::test]
async fn workflow_result_retention_is_a_handler_option() {
    let options: ServiceOptionsConfig = serde_json::from_value(json!({
        "handlers": {"run": {"workflow_retention": "2days"}}
    }))
    .unwrap();
    options.validate_handlers::<Workflow>().unwrap();
    let manifest = discovery(
        Endpoint::builder()
            .bind(Workflow.into_service_definition().options(options.into()))
            .build(),
    )
    .await;
    assert_eq!(
        manifest["services"][0]["handlers"][0]["workflowCompletionRetention"],
        172_800_000
    );
}

#[test]
fn rejects_config_typos_bad_durations_and_unknown_handlers_before_binding() {
    for input in [
        json!({"inactivity_timout": "30s"}),
        json!({"inactivity_timeout": "soon"}),
        json!({"retry_policy_on_max_attempts": "ignore"}),
        json!({"handlers": {"fetch_info": {"ingress_privat": true}}}),
    ] {
        assert!(serde_json::from_value::<ServiceOptionsConfig>(input).is_err());
    }
    let config: ServiceOptionsConfig = serde_json::from_value(json!({
        "handlers": {"Fetch_info": {"abort_timeout": "1s"}}
    }))
    .unwrap();
    assert!(
        matches!(config.apply(Example), Err(ConfigError::UnknownHandler { handler, .. }) if handler == "Fetch_info")
    );
}

#[tokio::test]
async fn sdk_builder_composes_services_objects_and_workflows_with_independent_options() {
    let config: Config<ServicesConfig> = serde_json::from_value(json!({
        "endpoint": {"listener": "[::1]:9081"},
        "services": {
            "accounts": {
                "ingress_private": true,
                "handlers": {"fetch_info": {"abort_timeout": "2s"}}
            },
            "checkout": {"handlers": {"run": {"workflow_retention": "1day"}}},
            "notifications": {"inactivity_timeout": "30s"}
        }
    }))
    .unwrap();
    let endpoint = Endpoint::builder()
        .configure(&config.endpoint)
        .unwrap()
        .bind(config.services.notifications.apply(Notifications).unwrap())
        .bind(config.services.accounts.apply(Example).unwrap())
        .bind(config.services.checkout.apply(Workflow).unwrap())
        .build();
    assert_eq!(config.endpoint.listener.to_string(), "[::1]:9081");
    let manifest = discovery(endpoint).await;
    let services = manifest["services"].as_array().unwrap();
    assert_eq!(services.len(), 3);
    let service = |name| {
        services
            .iter()
            .find(|service| service["name"] == name)
            .unwrap()
    };
    assert_eq!(service("Notifications")["ty"], "SERVICE");
    assert_eq!(service("Example")["ty"], "VIRTUAL_OBJECT");
    assert_eq!(service("Workflow")["ty"], "WORKFLOW");
    assert_eq!(service("Notifications")["inactivityTimeout"], 30_000);
    assert!(service("Notifications").get("ingressPrivate").is_none());
    assert_eq!(service("Example")["ingressPrivate"], true);
    assert_eq!(service("Example")["handlers"][0]["abortTimeout"], 2_000);
    assert!(service("Example").get("inactivityTimeout").is_none());
    assert_eq!(
        service("Workflow")["handlers"][0]["workflowCompletionRetention"],
        86_400_000
    );
    assert!(service("Workflow").get("ingressPrivate").is_none());
}

#[test]
fn generic_config_preserves_application_defaults_and_round_trips_named_or_dynamic_services() {
    #[derive(Deserialize)]
    #[serde(default)]
    struct ApplicationPolicy {
        timeout: u64,
    }
    impl Default for ApplicationPolicy {
        fn default() -> Self {
            Self { timeout: 42 }
        }
    }

    for input in [
        json!({}),
        json!({"endpoint": {"identity_keys": "key-a, key-b"}}),
        json!({"services": {}}),
    ] {
        let config: Config<ApplicationPolicy> = serde_json::from_value(input).unwrap();
        assert_eq!(config.services.timeout, 42);
        assert_eq!(config.endpoint.listener.to_string(), "0.0.0.0:9080");
    }

    let input = json!({
        "endpoint": {"listener": "127.0.0.1:0", "identity_keys": ["key-a", "key-b"]},
        "services": {"accounts": {"ingress_private": false}}
    });
    let named: Config<ServicesConfig> = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(named.services.accounts.ingress_private, Some(false));
    assert!(named.services.checkout.metadata.is_empty());
    let round_trip: Config<ServicesConfig> =
        serde_json::from_value(serde_json::to_value(&named).unwrap()).unwrap();
    assert_eq!(round_trip.endpoint.listener, named.endpoint.listener);
    assert_eq!(
        round_trip.endpoint.identity_keys,
        named.endpoint.identity_keys
    );
    assert_eq!(round_trip.services.accounts.ingress_private, Some(false));

    let dynamic: Config<std::collections::BTreeMap<String, ServiceOptionsConfig>> =
        serde_json::from_value(input).unwrap();
    assert_eq!(dynamic.services["accounts"].ingress_private, Some(false));
}

#[test]
fn generic_config_rejects_unknown_fields_and_invalid_listener_addresses() {
    for input in [
        json!({"endpont": {}}),
        json!({"endpoint": {"listner": "127.0.0.1:9080"}}),
        json!({"endpoint": {"listener": "not-an-address"}}),
        json!({"endpoint": {"listener": "127.0.0.1:65536"}}),
        json!({"services": {"acounts": {}}}),
        json!({"services": {"accounts": {"ingress_privat": true}}}),
    ] {
        assert!(serde_json::from_value::<Config<ServicesConfig>>(input).is_err());
    }
}

#[test]
fn identity_keys_accept_rotation_lists_or_delimited_strings_and_enforce_verification() {
    // Public verification keys only, from the SDK's public examples.
    let first = "publickeyv1_w7YHemBctH5Ck2nQRQ47iBBqhNHy4FV7t2Usbye2A6f";
    let second = "publickeyv1_ChjENKeMvCtRnqG2mrBK1HmPKufgFUc98K8B3ononQvp";
    let list: EndpointConfig =
        serde_json::from_value(json!({"identity_keys": [first, second]})).unwrap();
    let text: EndpointConfig = serde_json::from_value(json!({
        "identity_keys": format!("  {first},\r\n\t{second} ")
    }))
    .unwrap();
    assert_eq!(list.identity_keys, text.identity_keys);
    let endpoint = Endpoint::builder().configure(&list).unwrap().build();
    let request = || {
        http::Request::builder()
            .uri("/discover")
            .body(Empty::<Bytes>::new())
            .unwrap()
    };
    assert_eq!(endpoint.handle(request()).status(), 401);
    let unsecured = EndpointConfig::default()
        .apply(Endpoint::builder())
        .unwrap()
        .build();
    assert_eq!(unsecured.handle(request()).status(), 200);

    let invalid = EndpointConfig {
        identity_keys: vec![first.to_owned(), "invalid".to_owned()],
        ..EndpointConfig::default()
    };
    assert!(matches!(
        Endpoint::builder().configure(&invalid),
        Err(ConfigError::IdentityKey { index: 1, .. })
    ));
}
