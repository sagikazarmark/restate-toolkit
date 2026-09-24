//! Polling through a real Restate journal: replay, time spent paused, and cancellation.
//! Run with `RESTATE_SERVER_BIN` and `-- --ignored`.

#![cfg(unix)]

use std::{
    ops::ControlFlow,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use restate_e2e_harness::gate::{PROTOCOL_V7, SCOPED_VIRTUAL_OBJECTS, VQUEUES};
use restate_e2e_harness::{
    Admin, Call, JournalEntry, Restate, ReusePolicy, ServerSpec, launcher_or_skip, run_result_at,
};
use restate_ext::{
    ContextClockExt,
    poll::{PollFailure, PollOutcome, Poller},
};
use restate_sdk::prelude::*;
use serde_json::json;
use tokio::sync::Notify;

// Leave setup and admin operations ample room, then deliberately hold recovery
// for a full timeout after reaching the synchronized failure/pause point.
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

const SERVER: ServerSpec = ServerSpec {
    name: "restate-ext-poll",
    features: &[
        (VQUEUES, true),
        (PROTOCOL_V7, true),
        (SCOPED_VIRTUAL_OBJECTS, true),
    ],
    env: &[],
};

struct Polling {
    replay_reads: Arc<AtomicU64>,
    fail_once: AtomicBool,
    waiting_reads: Arc<AtomicU64>,
    failure_ready: Arc<Notify>,
    release_failure: Arc<Notify>,
    limited_reads: Arc<AtomicU64>,
    retry_limit_once: AtomicBool,
}

#[restate_sdk::service]
impl Polling {
    #[handler(
        journal_retention = "1d",
        invocation_retry_policy(initial_interval = "100ms", max_interval = "100ms")
    )]
    async fn replay(&self, ctx: Context<'_>) -> HandlerResult<Json<(u64, bool)>> {
        let start = ctx.now_named("test.start").await?;
        let context = &ctx;
        let mut observations = 0;

        let poller = Poller::builder(Duration::from_millis(20))
            .timeout(WAIT_TIMEOUT)
            .max_attempts(2)
            .build()?;

        let outcome = poller
            .poll(context, || {
                observations += 1;
                let second = observations == 2;
                async move {
                    // Fail after poll's second eligibility check, before recording
                    // the observation. A retry replays that check even after expiry.
                    if second && self.fail_once.swap(false, Ordering::SeqCst) {
                        // Test-only synchronization models a stalled endpoint.
                        // Release the failure only after the deadline has passed.
                        self.failure_ready.notify_one();
                        self.release_failure.notified().await;
                        return Err(
                            std::io::Error::other("fail before the second observation").into()
                        );
                    }
                    let value = context
                        .run(|| async { Ok(self.replay_reads.fetch_add(1, Ordering::SeqCst) + 1) })
                        .name("poll.read")
                        .await?;
                    Ok(if value >= 2 {
                        ControlFlow::Break(value)
                    } else {
                        ControlFlow::Continue(value)
                    })
                }
            })
            .await?;
        let end = ctx.now_named("test.end").await?;
        let value = match outcome {
            PollOutcome::Finished(value) => value,
            PollOutcome::Failed { .. } => {
                return Err(TerminalError::new("replayed conclusion was lost").into());
            }
        };
        Ok(Json((
            value,
            end.duration_since(start).unwrap_or_default() >= WAIT_TIMEOUT,
        )))
    }

    #[handler(journal_retention = "1d")]
    async fn wait(
        &self,
        ctx: Context<'_>,
        options: Json<(u64, Option<u64>)>,
    ) -> HandlerResult<u64> {
        let Json((interval_ms, timeout_ms)) = options;

        let mut builder = Poller::builder(Duration::from_millis(interval_ms));

        if let Some(timeout_ms) = timeout_ms {
            builder = builder.timeout(Duration::from_millis(timeout_ms));
        }

        let poller = builder.build()?;

        let outcome = poller
            .poll(&ctx, || async {
                let value = ctx
                    .run(|| async { Ok(self.waiting_reads.fetch_add(1, Ordering::SeqCst) + 1) })
                    .name("poll.read")
                    .await?;
                Ok(ControlFlow::<u64, _>::Continue(value))
            })
            .await?;
        match outcome {
            PollOutcome::Failed {
                reason: PollFailure::TimedOut,
                last,
            } => Ok(last),
            PollOutcome::Failed { .. } => {
                Err(TerminalError::new("unexpected attempt limit").into())
            }
            PollOutcome::Finished(value) => Ok(value),
        }
    }

    #[handler(
        journal_retention = "1d",
        invocation_retry_policy(initial_interval = "100ms", max_interval = "100ms")
    )]
    async fn limited(&self, ctx: Context<'_>) -> HandlerResult<u64> {
        let poller = Poller::builder(Duration::from_millis(20))
            .max_attempts(3)
            .build()?;

        let outcome = poller
            .poll(&ctx, || async {
                let value = ctx
                    .run(|| async {
                        let attempt = self.limited_reads.fetch_add(1, Ordering::SeqCst);

                        if attempt == 0 {
                            return Err(std::io::Error::other("transient read failure").into());
                        }

                        Ok(attempt)
                    })
                    .name("limited.read")
                    .retry_policy(
                        RunRetryPolicy::new()
                            .initial_delay(Duration::from_millis(20))
                            .max_attempts(2),
                    )
                    .await?;

                Ok(ControlFlow::<(), _>::Continue(value))
            })
            .await?;

        let PollOutcome::Failed {
            reason: PollFailure::AttemptsExhausted,
            last,
        } = outcome
        else {
            return Err(TerminalError::new("expected attempt exhaustion").into());
        };

        // Replay the entire exhausted poll. Completed external reads must not execute again,
        // and rebuilding the local attempt count must produce the same last value.
        if self.retry_limit_once.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::other("retry after exhausting attempts").into());
        }

        Ok(last)
    }
}

async fn await_sleep(admin: &Admin, id: &str) -> JournalEntry {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(entry) = admin
                .journal(id)
                .await
                .into_iter()
                .find(|entry| entry.entry_type == "Command: Sleep")
            {
                return entry;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("poll registered a durable sleep")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs RESTATE_SERVER_BIN"]
async fn e2e_poll_replays_observations_and_timers_and_propagates_cancellation() {
    let Some(launcher) = launcher_or_skip(ReusePolicy::Never) else {
        return;
    };
    let restate = launcher.launch(&SERVER).await;
    let replay_reads = Arc::new(AtomicU64::new(0));
    let waiting_reads = Arc::new(AtomicU64::new(0));
    let failure_ready = Arc::new(Notify::new());
    let release_failure = Arc::new(Notify::new());
    let limited_reads = Arc::new(AtomicU64::new(0));
    restate
        .deploy(
            Endpoint::builder()
                .bind(Polling {
                    replay_reads: replay_reads.clone(),
                    fail_once: AtomicBool::new(true),
                    waiting_reads: waiting_reads.clone(),
                    failure_ready: failure_ready.clone(),
                    release_failure: release_failure.clone(),
                    limited_reads: limited_reads.clone(),
                    retry_limit_once: AtomicBool::new(true),
                })
                .build(),
        )
        .await;

    assert_attempt_limit_replays(&restate, &limited_reads).await;

    let call = Call::service("Polling", "replay");
    let sent = restate.invoke(&call.send(), None, Some("replay")).await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    tokio::time::timeout(WAIT_TIMEOUT, failure_ready.notified())
        .await
        .expect("reached the second observation");
    tokio::time::sleep(WAIT_TIMEOUT).await;
    release_failure.notify_one();
    let reply = restate.invoke(&call, None, Some("replay")).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        reply.body,
        json!([2, true]),
        "second observation finished after the timeout"
    );
    assert_eq!(
        replay_reads.load(Ordering::SeqCst),
        2,
        "completed observations must not execute again"
    );
    let journal = restate.admin().journal(reply.invocation_id()).await;
    assert_eq!(
        journal
            .iter()
            .filter(|entry| entry.entry_type == "Command: Sleep")
            .count(),
        1
    );
    for (index, value) in [(0, "1"), (1, "2")] {
        assert!(
            run_result_at(&journal, "poll.read", index)
                .expect("recorded observation")
                .raw_contains(value)
        );
    }

    let wait = Call::service("Polling", "wait");
    let options = json!([WAIT_TIMEOUT.as_millis(), WAIT_TIMEOUT.as_millis()]);
    let sent = restate
        .invoke(&wait.send(), Some(&options), Some("paused-wait"))
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    let timer = await_sleep(restate.admin(), sent.invocation_id()).await;
    restate.admin().pause(sent.invocation_id()).await;
    // The timer and deadline expire while paused. Resume must reuse the timer and
    // stop before another external observation, rather than restarting the wait.
    tokio::time::sleep(WAIT_TIMEOUT).await;
    restate.admin().resume(sent.invocation_id()).await;
    let reply = restate
        .invoke(&wait, Some(&options), Some("paused-wait"))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body, json!(1));
    assert_eq!(waiting_reads.load(Ordering::SeqCst), 1);
    let journal = restate.admin().journal(reply.invocation_id()).await;
    let timers: Vec<_> = journal
        .iter()
        .filter(|entry| entry.entry_type == "Command: Sleep")
        .collect();
    assert_eq!(timers.len(), 1);
    assert_eq!(timers[0].raw, timer.raw, "the original timer was replayed");

    assert_indefinite_wait_cancellation(&restate, &waiting_reads).await;

    restate.finish().await;
}

async fn assert_indefinite_wait_cancellation(restate: &Restate, waiting_reads: &AtomicU64) {
    let reads_before = waiting_reads.load(Ordering::SeqCst);
    let wait = Call::service("Polling", "wait");

    // An indefinite wait still responds to cancellation and journals no clock reads.
    let options = json!([60_000, null]);
    let sent = restate
        .invoke(&wait.send(), Some(&options), Some("cancelled-wait"))
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    await_sleep(restate.admin(), sent.invocation_id()).await;
    restate.admin().cancel(sent.invocation_id()).await;
    let reply = restate
        .invoke(&wait, Some(&options), Some("cancelled-wait"))
        .await;
    assert_eq!(reply.status, 409, "{}", reply.body);
    let journal = restate.admin().journal(reply.invocation_id()).await;

    assert!(
        journal
            .iter()
            .all(|entry| entry.name.as_deref() != Some("poll.clock"))
    );

    assert_eq!(
        waiting_reads.load(Ordering::SeqCst),
        reads_before + 1,
        "cancellation must not poll again"
    );
}

async fn assert_attempt_limit_replays(restate: &Restate, limited_reads: &AtomicU64) {
    let reply = restate
        .invoke(&Call::service("Polling", "limited"), None, None)
        .await;

    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.body, json!(3));
    // Three logical checks include one retried read: four external attempts total.
    assert_eq!(limited_reads.load(Ordering::SeqCst), 4);

    let journal = restate.admin().journal(reply.invocation_id()).await;

    assert_eq!(
        journal
            .iter()
            .filter(|entry| entry.entry_type == "Command: Sleep")
            .count(),
        2
    );
    assert!(
        journal
            .iter()
            .all(|entry| entry.name.as_deref() != Some("poll.clock"))
    );

    for (index, value) in [(0, "1"), (1, "2"), (2, "3")] {
        assert!(
            run_result_at(&journal, "limited.read", index)
                .expect("recorded check")
                .raw_contains(value)
        );
    }
}
