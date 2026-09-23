//! Deploys the Greeter endpoint to a real `restate-server` and checks that it
//! greets by the journaled time and that the configured policies reach the
//! server.
//!
//! Ignored: run with `RESTATE_SERVER_BIN` set and `-- --ignored`.

#![cfg(unix)]

use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime},
};

use greeter::{Greeter, greeting};
use restate_config::{RetryPolicyOnMaxAttempts, ServiceOptionsConfig};
use restate_e2e_harness::gate::{PROTOCOL_V7, SCOPED_VIRTUAL_OBJECTS, VQUEUES};
use restate_e2e_harness::{Call, ReusePolicy, ServerSpec, launcher_or_skip, run_result};
use restate_sdk::endpoint::Endpoint;
use serde_json::{Value, json};

const SERVER: ServerSpec = ServerSpec {
    name: "greeter",
    features: &[
        (VQUEUES, true),
        (PROTOCOL_V7, true),
        (SCOPED_VIRTUAL_OBJECTS, true),
    ],
    env: &[],
};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs RESTATE_SERVER_BIN"]
async fn e2e_greeter() {
    let Some(launcher) = launcher_or_skip(ReusePolicy::Never) else {
        return;
    };
    let restate = launcher.launch(&SERVER).await;
    let endpoint = Endpoint::builder()
        .bind(policy().apply(Greeter).expect("valid policy"))
        .build();
    restate.deploy(endpoint).await;

    // The public handler answers through the ingress, greeting for the time of
    // day. It reads the clock somewhere between these two readings, so it greets
    // as one of them would: the same greeting, unless the call straddles a
    // change of the hour.
    let before = SystemTime::now();
    let reply = restate
        .invoke(
            &Call::service("Greeter", "greet"),
            Some(&json!("Ada")),
            None,
        )
        .await;
    let after = SystemTime::now();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let expected = [greeting("Ada", before), greeting("Ada", after)];
    let greeted = expected
        .iter()
        .find(|greeting| reply.body == json!(greeting))
        .unwrap_or_else(|| panic!("{} is none of {expected:?}", reply.body));

    // The configured journal retention keeps the completed runs inspectable:
    // the clock read by `ctx.now()`, then the greeting.
    let id = reply.invocation_id();
    assert_eq!(
        restate.admin().runs(id).await,
        ["SystemTime::now()", "greet-person"]
    );
    let journal = restate.admin().journal(id).await;
    assert!(
        run_result(&journal, "greet-person")
            .expect("retained greet-person result")
            .raw_contains(greeted)
    );

    // The configured policy is published in discovery.
    let service: Value = reqwest::get(format!("{}/services/Greeter", restate.admin_url()))
        .await
        .expect("GET /services/Greeter")
        .json()
        .await
        .expect("service JSON");
    assert_eq!(service["metadata"]["team"], "greetings", "{service}");
    assert_eq!(service["journal_retention"], "1d", "{service}");
    assert_eq!(
        service["retry_policy"]["initial_interval"], "500ms",
        "{service}"
    );
    assert_eq!(service["retry_policy"]["max_attempts"], 5, "{service}");
    assert_eq!(
        service["retry_policy"]["on_max_attempts"], "Pause",
        "{service}"
    );

    restate.finish().await;
}

/// The Greeter policy the test checks reaches the server. The test binds the
/// service itself: the harness serves the endpoint on a port of its own, with
/// no identity keys, so only the service policy matters.
fn policy() -> ServiceOptionsConfig {
    ServiceOptionsConfig {
        metadata: BTreeMap::from([("team".to_owned(), "greetings".to_owned())]),
        // Keep completed journals so their runs can be inspected.
        journal_retention: Some(Duration::from_hours(24)),
        retry_policy_initial_interval: Some(Duration::from_millis(500)),
        retry_policy_max_attempts: Some(5),
        retry_policy_on_max_attempts: Some(RetryPolicyOnMaxAttempts::Pause),
        ..ServiceOptionsConfig::default()
    }
}
