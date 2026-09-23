//! Deploys the configured Greeter endpoint to a real `restate-server` and
//! checks that the in-code configuration reaches the server.
//!
//! Ignored: run with `RESTATE_SERVER_BIN` set and `-- --ignored`.

#![cfg(unix)]

use restate_e2e_harness::gate::{PROTOCOL_V7, SCOPED_VIRTUAL_OBJECTS, VQUEUES};
use restate_e2e_harness::{Call, ReusePolicy, ServerSpec, launcher_or_skip, run_result};
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
    let endpoint = greeter::endpoint(greeter::config()).expect("valid configuration");
    restate.deploy(endpoint).await;

    // The public handler answers through the ingress.
    let reply = restate
        .invoke(
            &Call::service("Greeter", "greet"),
            Some(&json!("Ada")),
            None,
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body, json!("Hello, Ada!"));

    // The configured journal retention keeps the completed run inspectable.
    let id = reply.invocation_id();
    assert_eq!(restate.admin().runs(id).await, ["greet-person"]);
    let journal = restate.admin().journal(id).await;
    assert!(
        run_result(&journal, "greet-person")
            .expect("retained greet-person result")
            .raw_contains("Hello, Ada!")
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
    let audit = service["handlers"]
        .as_array()
        .expect("handlers")
        .iter()
        .find(|handler| handler["name"] == "audit")
        .expect("audit handler");
    assert_eq!(audit["public"], false, "{audit}");

    // The ingress-private handler is refused at the ingress.
    let reply = restate
        .invoke(
            &Call::service("Greeter", "audit"),
            Some(&json!("Ada")),
            None,
        )
        .await;
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert_eq!(reply.body["message"], "the invoked service is not public");

    restate.finish().await;
}
