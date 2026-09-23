//! Holds [`ContextClockExt`] to a real `restate-server`: each read is journaled under its
//! name, and a retried attempt replays the readings rather than reading the clock again.
//!
//! Ignored: run with `RESTATE_SERVER_BIN` set and `-- --ignored`.

#![cfg(unix)]

use std::{
    sync::{
        Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use restate_e2e_harness::gate::{PROTOCOL_V7, SCOPED_VIRTUAL_OBJECTS, VQUEUES};
use restate_e2e_harness::{Call, ReusePolicy, ServerSpec, launcher_or_skip, run_result};
use restate_ext::ContextClockExt;
use restate_sdk::prelude::*;
use serde_json::json;

const SERVER: ServerSpec = ServerSpec {
    name: "restate-ext-clock",
    features: &[
        (VQUEUES, true),
        (PROTOCOL_V7, true),
        (SCOPED_VIRTUAL_OBJECTS, true),
    ],
    env: &[],
};

/// The name `now()` journals under; private to the crate, so spelled out here.
const NOW_NAME: &str = "SystemTime::now()";

/// How many attempts the handler has started. The endpoint runs in this process, so the
/// test sees what the handler counted.
static ATTEMPTS: AtomicU32 = AtomicU32::new(0);

/// What the first attempt read, before it failed.
static FIRST_ATTEMPT: Mutex<Option<(u64, u64)>> = Mutex::new(None);

struct Clock;

#[restate_sdk::service(name = "Clock")]
impl Clock {
    /// Reads the clock under the default name and under one of its own, then fails the first
    /// attempt, so the second attempt has to replay both reads.
    #[handler(journal_retention = "1d")]
    async fn stamp(&self, ctx: Context<'_>) -> HandlerResult<Json<(u64, u64)>> {
        let first = millis(ctx.now().await?);
        let second = millis(ctx.now_named("second").await?);

        if ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
            *FIRST_ATTEMPT.lock().unwrap() = Some((first, second));
            // Let the wall clock move on, so an attempt that read it again could not land on
            // the same millisecond and pass for a replay.
            tokio::time::sleep(Duration::from_millis(20)).await;
            return Err(std::io::Error::other("fail the first attempt").into());
        }

        Ok(Json((first, second)))
    }
}

fn millis(time: SystemTime) -> u64 {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap();
    u64::try_from(since_epoch.as_millis()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs RESTATE_SERVER_BIN"]
async fn e2e_clock_reads_are_journaled_and_replayed() {
    let Some(launcher) = launcher_or_skip(ReusePolicy::Never) else {
        return;
    };
    let restate = launcher.launch(&SERVER).await;
    restate
        .deploy(Endpoint::builder().bind(Clock).build())
        .await;

    let reply = restate
        .invoke(&Call::service("Clock", "stamp"), None, None)
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);

    // The retry replayed what the first attempt read, rather than reading the clock again.
    assert_eq!(ATTEMPTS.load(Ordering::SeqCst), 2, "one failure, one retry");
    let (first, second) = FIRST_ATTEMPT
        .lock()
        .unwrap()
        .expect("the first attempt read the clock");
    assert_eq!(reply.body, json!([first, second]));
    assert!(first <= second, "{first} then {second}");

    // Each read is its own run entry, under the name it was given, holding the millis.
    let id = reply.invocation_id();
    assert_eq!(restate.admin().runs(id).await, [NOW_NAME, "second"]);
    let journal = restate.admin().journal(id).await;
    for (name, reading) in [(NOW_NAME, first), ("second", second)] {
        assert!(
            run_result(&journal, name)
                .unwrap_or_else(|| panic!("retained {name} result"))
                .raw_contains(&reading.to_string()),
            "{name} journals {reading}"
        );
    }

    restate.finish().await;
}
