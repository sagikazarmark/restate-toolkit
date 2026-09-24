//! A shared wait handler polls the counter while its add handler keeps accepting updates.
//! Run with `RESTATE_SERVER_BIN` and `-- --ignored`.

#![cfg(unix)]

use std::time::Duration;

use poll_counter::Counter;
use restate_e2e_harness::gate::{PROTOCOL_V7, SCOPED_VIRTUAL_OBJECTS, VQUEUES};
use restate_e2e_harness::{Call, ReusePolicy, ServerSpec, launcher_or_skip};
use restate_sdk::endpoint::Endpoint;
use serde_json::json;

const SERVER: ServerSpec = ServerSpec {
    name: "poll-counter",
    features: &[
        (VQUEUES, true),
        (PROTOCOL_V7, true),
        (SCOPED_VIRTUAL_OBJECTS, true),
    ],
    env: &[],
};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs RESTATE_SERVER_BIN"]
async fn e2e_wait_for_ten_while_the_counter_keeps_accepting_updates() {
    let Some(launcher) = launcher_or_skip(ReusePolicy::Never) else {
        return;
    };
    let restate = launcher.launch(&SERVER).await;
    restate
        .deploy(Endpoint::builder().bind(Counter).build())
        .await;

    let add = Call::object("Counter", "example", "add");
    assert_eq!(
        restate.invoke(&add, Some(&json!(9)), None).await.body,
        json!(9)
    );

    let wait = Call::object("Counter", "example", "wait");

    let sent = restate
        .invoke(&wait.send(), None, Some("wait-for-ten"))
        .await;

    assert_eq!(sent.status, 202, "{}", sent.body);

    // Synchronize on the durable sleep: the waiter has already observed nine.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if restate
                .admin()
                .journal(sent.invocation_id())
                .await
                .iter()
                .any(|entry| entry.entry_type == "Command: Sleep")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("waiter started polling");

    assert_eq!(
        restate.invoke(&add, Some(&json!(1)), None).await.body,
        json!(10)
    );

    // The idempotency key attaches this request to the original wait.
    let reply = restate.invoke(&wait, None, Some("wait-for-ten")).await;

    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body, json!(10));
    assert_eq!(reply.invocation_id(), sent.invocation_id());

    let slow = Call::object("Counter", "slow", "add");

    assert_eq!(
        restate.invoke(&slow, Some(&json!(7)), None).await.body,
        json!(7)
    );

    let reply = restate
        .invoke(&Call::object("Counter", "slow", "wait"), None, None)
        .await;

    assert_eq!(reply.status, 408, "{}", reply.body);
    assert!(
        reply.body["message"]
            .as_str()
            .unwrap()
            .contains("last value: 7")
    );

    // A counter already beyond the threshold completes without sleeping.
    assert_eq!(
        restate.invoke(&add, Some(&json!(2)), None).await.body,
        json!(12)
    );
    let reply = restate.invoke(&wait, None, None).await;

    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body, json!(12));
    assert!(
        restate
            .admin()
            .journal(reply.invocation_id())
            .await
            .iter()
            .all(|entry| entry.entry_type != "Command: Sleep")
    );

    restate.finish().await;
}
