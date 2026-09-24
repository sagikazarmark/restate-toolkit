//! A counter with a shared handler that waits for its value to reach ten.
//!
//! Each check reads the counter's state and compares it to ten. Timers and the
//! timeout belong to [`Poller`].

use std::{ops::ControlFlow, time::Duration};

use restate_ext::poll::{PollOutcome, Poller};
use restate_sdk::prelude::*;

const WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// A keyed counter whose writes are serialized by Restate.
pub struct Counter;

#[restate_sdk::object]
impl Counter {
    /// Adds to the counter, including while a shared `wait` handler is running.
    #[handler]
    async fn add(&self, ctx: ObjectContext<'_>, amount: u64) -> HandlerResult<u64> {
        let value = ctx.get::<u64>("value").await?.unwrap_or_default();

        let value = value
            .checked_add(amount)
            .ok_or_else(|| TerminalError::new("counter overflow").with_code(400))?;

        ctx.set("value", value);

        Ok(value)
    }

    /// Checks once per second for up to 30 seconds until the counter reaches ten.
    /// A shared handler lets `add` update the counter while this handler waits.
    /// Lazy state makes each new read see updates instead of the initial state.
    #[handler(lazy_state, journal_retention = "1d")]
    async fn wait(&self, ctx: SharedObjectContext<'_>) -> HandlerResult<u64> {
        let poller = Poller::builder(Duration::from_secs(1))
            .timeout(WAIT_TIMEOUT)
            .build()?;

        let outcome = poller
            .poll(&ctx, || async {
                let value = ctx.get::<u64>("value").await?.unwrap_or_default();

                // The domain condition, independent of Restate or how the counter is read.
                // Values above ten count too: updates can skip straight past the threshold.
                if value >= 10 {
                    Ok(ControlFlow::Break(value))
                } else {
                    Ok(ControlFlow::Continue(value))
                }
            })
            .await?;

        match outcome {
            PollOutcome::Finished(value) => Ok(value),
            // This example configures only a timeout, so that is the only polling limit.
            PollOutcome::Failed { last, .. } => Err(TerminalError::new(format!(
                "counter did not reach ten within {}s; last value: {last}",
                WAIT_TIMEOUT.as_secs()
            ))
            .with_code(408)
            .into()),
        }
    }
}
