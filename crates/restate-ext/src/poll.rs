//! Fixed-delay polling with durable timers, optional timeout and attempt limits.

use std::{
    ops::ControlFlow,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use restate_sdk::prelude::{ContextTimers, HandlerError, TerminalError};

use crate::ContextClockExt;

// Timers carry unsigned 64-bit epoch milliseconds. Reserve half that range for
// the host timestamp, including when a sleep is registered after a replay.
const MAX_DURATION: Duration = Duration::from_millis(u64::MAX / 2);

/// Checks a condition repeatedly, waiting a fixed interval between checks.
///
/// Construct a poller with [`Self::builder`]. The builder validates its configuration
/// before it can be used. Each call to [`Self::poll`] starts a new wait, so the same
/// poller can be reused. Without a timeout or attempt limit, it keeps checking
/// until the callback finishes, returns an error, or the invocation is cancelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Poller {
    interval: Duration,
    timeout: Option<Duration>,
    max_attempts: Option<u32>,
}

/// Configures a [`Poller`]. Supply the interval to [`Self::new`], optionally set a
/// timeout or attempt limit, then call [`Self::build`] to validate the configuration.
#[derive(Clone, Debug)]
pub struct PollerBuilder {
    interval: Duration,
    timeout: Option<Duration>,
    max_attempts: Option<u32>,
}

impl PollerBuilder {
    /// Starts configuring a poller with the delay between checks.
    ///
    /// The first check runs immediately. Each later check waits this long after
    /// the previous check finishes. The wait is shortened when less time remains
    /// before the timeout. The interval must be greater than zero.
    /// Validation happens in [`Self::build`].
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            timeout: None,
            max_attempts: None,
        }
    }

    /// How long to keep polling, starting before the first check.
    ///
    /// The timeout is checked between operations; it does not interrupt a check
    /// or its retries. A running check can therefore finish after this time.
    /// Zero means check once without waiting or checking again. Leave this setting
    /// unset to keep checking without a time limit. Validation happens in [`Self::build`].
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);

        self
    }

    /// Maximum number of checks, including the first check.
    ///
    /// One means check once without sleeping. A final result or error on the last
    /// allowed check is returned normally; a check that still needs more time
    /// produces [`PollFailure::AttemptsExhausted`]. Leave unset for no attempt limit.
    ///
    /// Restate replays and retries inside a check do not consume extra attempts.
    /// Each call to [`Poller::poll`] starts a fresh count. Must be greater than zero;
    /// validation happens in [`Self::build`].
    #[must_use]
    pub fn max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = Some(max_attempts);

        self
    }

    /// Validates the configuration and creates a reusable poller.
    ///
    /// # Errors
    ///
    /// Returns a terminal error with code 400 if the interval or attempt limit is
    /// zero, or a duration is not a whole number of milliseconds or exceeds
    /// `u64::MAX / 2` milliseconds. No Restate context is needed for validation.
    pub fn build(self) -> Result<Poller, TerminalError> {
        if self.interval.is_zero() {
            return Err(TerminalError::new("poll interval must be positive").with_code(400));
        }

        validate_duration("interval", self.interval)?;

        if let Some(timeout) = self.timeout {
            validate_duration("timeout", timeout)?;
        }

        if self.max_attempts == Some(0) {
            return Err(TerminalError::new("poll max_attempts must be positive").with_code(400));
        }

        Ok(Poller {
            interval: self.interval,
            timeout: self.timeout,
            max_attempts: self.max_attempts,
        })
    }
}

fn validate_duration(name: &str, duration: Duration) -> Result<(), TerminalError> {
    if !duration.subsec_nanos().is_multiple_of(1_000_000) || duration > MAX_DURATION {
        return Err(TerminalError::new(format!(
            "poll {name} must be whole milliseconds, at most {}ms",
            MAX_DURATION.as_millis()
        ))
        .with_code(400));
    }

    Ok(())
}

/// Why polling stopped and the result of the last check.
///
/// `T` is the result returned when a check decides to stop. `P` is the value
/// returned by a check that needs more time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollOutcome<T, P> {
    /// A check decided to stop and returned this result.
    ///
    /// This can mean the condition was met or that waiting longer would not help.
    /// The callback chooses by returning `ControlFlow::Break(result)`.
    Finished(T),

    /// A polling limit was reached before a check returned a final result.
    /// Errors from the check or Restate are returned through the outer `Result` instead.
    Failed {
        /// Which limit stopped polling.
        reason: PollFailure,

        /// The value from the last check, returned as `ControlFlow::Continue(value)`.
        /// For example, the last count when waiting for a counter to reach ten.
        last: P,
    },
}

/// The limit that stopped polling before the check supplied a final result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollFailure {
    /// The configured timeout was reached.
    TimedOut,

    /// The last allowed check still asked to keep waiting.
    AttemptsExhausted,
}

impl Poller {
    /// Starts configuring a poller with the delay between checks.
    /// Timeout and attempt limits are optional. See [`PollerBuilder::new`].
    #[must_use]
    pub fn builder(interval: Duration) -> PollerBuilder {
        PollerBuilder::new(interval)
    }

    /// Calls `check` immediately, then repeats it after each interval until it returns
    /// a final result or a configured limit is reached.
    ///
    /// `check` returns [`ControlFlow::Break`] when the condition is met or waiting longer
    /// would not help, or [`ControlFlow::Continue`] with the latest value to keep waiting.
    /// It owns durable I/O: use individually awaited `ctx.run` operations, durable calls,
    /// or other journaled context operations. This method does not wrap the callback in
    /// `ctx.run` and does not retry callback errors itself.
    ///
    /// # Limits and replay
    ///
    /// Each call starts a fresh timeout before the first check. After a check asks to
    /// keep waiting, the clock is read and the sleep is shortened to the remaining time.
    /// Time is checked again after sleeping; reaching the deadline stops polling without
    /// another check. The last value is returned without cloning it.
    ///
    /// Without a timeout, polling makes no clock reads and uses the full interval between
    /// checks. Without either limit, it continues until `check` finishes, an error occurs,
    /// or the invocation is cancelled.
    ///
    /// An attempt limit counts logical checks, including the first. Restate retries
    /// inside a check and journal replay do not consume extra attempts. After a check
    /// returns `Continue`, the attempt limit is checked before the timeout. If both
    /// limits apply, [`PollFailure::AttemptsExhausted`] takes precedence. A final
    /// `Break` or a check error takes precedence over either limit.
    ///
    /// The timeout does not interrupt a check or its retries. A completed `Break` wins
    /// even after the deadline; errors are propagated rather than converted into timeouts.
    /// Bound individual external requests in their own I/O client as needed.
    ///
    /// Clock reads are journaled as `poll.clock`. Replay preserves earlier decisions: a
    /// pre-crash clock check can authorize a check or a new sleep after the actual deadline.
    /// Already-journaled timers keep their original target. Downtime counts once a fresh
    /// clock sample is reached. Backward clock samples do not restore elapsed time, but
    /// the timeout still depends on host wall clocks rather than a monotonic clock.
    /// Configuration and callback control flow must remain replay-compatible.
    ///
    /// An exclusive object handler retains its lock while waiting. Use a shared handler
    /// or a separate service when the object must accept updates during the wait.
    /// When polling object state directly, enable `lazy_state` on the shared handler
    /// so new reads see updates instead of the state loaded at invocation start.
    ///
    /// # Errors
    ///
    /// Returns a terminal error with code 400 if the deadline calculated from the
    /// journaled start time cannot be represented. Configuration is already validated
    /// by [`PollerBuilder::build`]. Propagates check, clock, and timer errors, including
    /// cancellation.
    ///
    /// # Example
    ///
    /// The read is journaled; deciding whether the value is sufficient is plain Rust.
    ///
    /// ```no_run
    /// use std::{ops::ControlFlow, time::Duration};
    /// use restate_ext::poll::{Poller, PollOutcome};
    /// use restate_sdk::prelude::*;
    /// # async fn read_remote_counter() -> HandlerResult<u64> { Ok(10) }
    ///
    /// fn until_ten(value: u64) -> ControlFlow<u64, u64> {
    ///     if value >= 10 { ControlFlow::Break(value) }
    ///     else { ControlFlow::Continue(value) }
    /// }
    ///
    /// async fn wait(ctx: &Context<'_>) -> HandlerResult<PollOutcome<u64, u64>> {
    ///     let poller = Poller::builder(Duration::from_secs(1))
    ///         .timeout(Duration::from_secs(60))
    ///         .build()?;
    ///
    ///     poller.poll(ctx, || async {
    ///         let value = ctx.run(read_remote_counter).name("counter.read").await?;
    ///
    ///         Ok(until_ten(value))
    ///     }).await
    /// }
    /// ```
    pub fn poll<'ctx, C, F, Fut, T, P>(
        &self,
        ctx: &C,
        check: F,
    ) -> impl Future<Output = Result<PollOutcome<T, P>, HandlerError>> + Send
    where
        C: ContextTimers<'ctx> + ContextClockExt + Sync,
        F: FnMut() -> Fut + Send,
        Fut: Future<Output = Result<ControlFlow<T, P>, HandlerError>> + Send,
        P: Send,
    {
        poll_with(
            self,
            || ctx.now_named("poll.clock"),
            |delay| ctx.sleep(delay),
            check,
        )
    }
}

// The production loop's clock/timer seam, also used by deterministic tests.
async fn poll_with<F, Fut, T, P, N, NF, S, SF>(
    poller: &Poller,
    mut now: N,
    mut sleep: S,
    mut check: F,
) -> Result<PollOutcome<T, P>, HandlerError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<ControlFlow<T, P>, HandlerError>>,
    N: FnMut() -> NF,
    NF: Future<Output = Result<SystemTime, TerminalError>>,
    S: FnMut(Duration) -> SF,
    SF: Future<Output = Result<(), TerminalError>>,
{
    let mut deadline = match poller.timeout {
        Some(timeout) => Some(Deadline::new(now().await?, timeout)?),
        None => None,
    };
    let mut remaining_attempts = poller.max_attempts;

    loop {
        let last = match check().await? {
            ControlFlow::Break(value) => return Ok(PollOutcome::Finished(value)),
            ControlFlow::Continue(value) => value,
        };

        if let Some(remaining) = &mut remaining_attempts {
            *remaining -= 1;

            if *remaining == 0 {
                return Ok(PollOutcome::Failed {
                    reason: PollFailure::AttemptsExhausted,
                    last,
                });
            }
        }

        if poller.timeout == Some(Duration::ZERO) {
            return Ok(PollOutcome::Failed {
                reason: PollFailure::TimedOut,
                last,
            });
        }

        let delay = match &mut deadline {
            Some(deadline) => {
                let remaining = deadline.remaining(now().await?);

                if remaining.is_zero() {
                    return Ok(PollOutcome::Failed {
                        reason: PollFailure::TimedOut,
                        last,
                    });
                }

                poller.interval.min(remaining)
            }
            None => poller.interval,
        };

        sleep(delay).await?;

        if let Some(deadline) = &mut deadline
            && deadline.remaining(now().await?).is_zero()
        {
            return Ok(PollOutcome::Failed {
                reason: PollFailure::TimedOut,
                last,
            });
        }
    }
}

struct Deadline {
    at: SystemTime,
    latest: SystemTime,
}

impl Deadline {
    fn new(start: SystemTime, timeout: Duration) -> Result<Self, TerminalError> {
        let at = start.checked_add(timeout).ok_or_else(|| {
            TerminalError::new("poll timeout exceeds the clock range").with_code(400)
        })?;

        if at
            .duration_since(UNIX_EPOCH)
            .map_or(true, |duration| duration.as_millis() > u128::from(u64::MAX))
        {
            return Err(TerminalError::new("poll deadline exceeds the timer range").with_code(400));
        }

        Ok(Self { at, latest: start })
    }

    fn remaining(&mut self, now: SystemTime) -> Duration {
        self.latest = self.latest.max(now);

        self.at.duration_since(self.latest).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
        future::ready,
        time::UNIX_EPOCH,
    };

    use super::*;

    #[test]
    fn building_rejects_zero_attempts() {
        let error = Poller::builder(Duration::from_secs(1))
            .max_attempts(0)
            .build()
            .unwrap_err();

        assert_eq!(error.code(), 400);
        assert_eq!(error.message(), "poll max_attempts must be positive");
    }

    fn error_text(error: &HandlerError) -> String {
        let error: &dyn std::error::Error = error.as_ref();

        error.to_string()
    }

    #[test]
    fn invalid_durations_fail_during_build() {
        for (interval, timeout) in [
            (Duration::ZERO, Duration::ZERO),
            (Duration::from_nanos(1), Duration::from_secs(1)),
            (Duration::from_secs(1), Duration::from_nanos(1)),
            (Duration::MAX, Duration::from_secs(1)),
            (Duration::from_secs(1), Duration::MAX),
        ] {
            let result = PollerBuilder::new(interval).timeout(timeout).build();

            assert!(result.is_err(), "accepted {interval:?}, {timeout:?}");
            assert_eq!(result.unwrap_err().code(), 400);
        }
    }

    #[tokio::test]
    async fn pending_observations_wait_before_trying_again() {
        let mut times = [0, 2, 5, 7, 10].into_iter();
        let mut values = [2, 6, 10].into_iter();
        let mut sleeps = Vec::new();

        let outcome = poll_with(
            &poller(30),
            || ready(Ok(UNIX_EPOCH + Duration::from_secs(times.next().unwrap()))),
            |duration| {
                sleeps.push(duration);

                ready(Ok(()))
            },
            || {
                let value = values.next().unwrap();

                ready(Ok(if value >= 10 {
                    ControlFlow::Break(value)
                } else {
                    ControlFlow::Continue(value)
                }))
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome, PollOutcome::Finished(10));
        assert_eq!(sleeps, [Duration::from_secs(3); 2]);
    }

    struct Timeline {
        readings: RefCell<VecDeque<u64>>,
        sleeps: RefCell<Vec<Duration>>,
    }

    impl Timeline {
        fn new(seconds: &[u64]) -> Self {
            Self {
                readings: RefCell::new(seconds.iter().copied().collect()),
                sleeps: RefCell::default(),
            }
        }

        fn now(&self) -> impl Future<Output = Result<SystemTime, TerminalError>> {
            ready(Ok(UNIX_EPOCH
                + Duration::from_secs(
                    self.readings
                        .borrow_mut()
                        .pop_front()
                        .expect("unexpected clock read"),
                )))
        }

        fn sleep(&self, duration: Duration) -> impl Future<Output = Result<(), TerminalError>> {
            self.sleeps.borrow_mut().push(duration);

            ready(Ok(()))
        }
    }

    fn poller(timeout: u64) -> Poller {
        Poller::builder(Duration::from_secs(3))
            .timeout(Duration::from_secs(timeout))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn attempt_limit_includes_the_first_check_and_keeps_the_last_value() {
        for (limit, expected_sleeps) in [(1, vec![]), (3, vec![Duration::from_secs(3); 2])] {
            let poller = Poller::builder(Duration::from_secs(3))
                .max_attempts(limit)
                .build()
                .unwrap();
            let time = Timeline::new(&[]);
            let mut checks = 0;

            let outcome = poll_with(
                &poller,
                || time.now(),
                |duration| time.sleep(duration),
                || {
                    checks += 1;
                    assert!(checks <= limit, "no check after the attempt limit");

                    ready(Ok(ControlFlow::<(), _>::Continue(checks)))
                },
            )
            .await
            .unwrap();

            assert_eq!(checks, limit);
            assert_eq!(
                outcome,
                PollOutcome::Failed {
                    reason: PollFailure::AttemptsExhausted,
                    last: limit,
                }
            );
            assert_eq!(*time.sleeps.borrow(), expected_sleeps);
        }
    }

    #[tokio::test]
    async fn a_final_result_on_the_last_allowed_check_wins() {
        let poller = Poller::builder(Duration::from_secs(3))
            .max_attempts(2)
            .build()
            .unwrap();

        for conclusion in [Ok(10), Err("resource failed")] {
            let time = Timeline::new(&[]);
            let mut checks = 0;

            let outcome = poll_with(
                &poller,
                || time.now(),
                |duration| time.sleep(duration),
                || {
                    checks += 1;

                    ready(Ok(if checks == 1 {
                        ControlFlow::Continue(9)
                    } else {
                        ControlFlow::Break(conclusion)
                    }))
                },
            )
            .await
            .unwrap();

            assert_eq!(outcome, PollOutcome::Finished(conclusion));
            assert_eq!(checks, 2);
            assert_eq!(*time.sleeps.borrow(), [Duration::from_secs(3)]);
        }
    }

    #[tokio::test]
    async fn attempt_limit_wins_when_both_limits_apply_after_a_pending_check() {
        for timeout in [Duration::ZERO, Duration::from_secs(5)] {
            let poller = Poller::builder(Duration::from_secs(1))
                .timeout(timeout)
                .max_attempts(1)
                .build()
                .unwrap();
            let seconds = Cell::new(0);

            let outcome = poll_with(
                &poller,
                || ready(Ok(UNIX_EPOCH + Duration::from_secs(seconds.get()))),
                |_| async { panic!("the last allowed check cannot sleep") },
                || {
                    seconds.set(20);

                    ready(Ok(ControlFlow::<(), _>::Continue("pending")))
                },
            )
            .await
            .unwrap();

            assert_eq!(
                outcome,
                PollOutcome::Failed {
                    reason: PollFailure::AttemptsExhausted,
                    last: "pending",
                }
            );
        }
    }

    #[tokio::test]
    async fn timeout_can_stop_polling_before_all_attempts_are_used() {
        let poller = Poller::builder(Duration::from_secs(3))
            .timeout(Duration::from_secs(2))
            .max_attempts(10)
            .build()
            .unwrap();
        let time = Timeline::new(&[0, 1, 2]);
        let mut checks = 0;

        let outcome = poll_with(
            &poller,
            || time.now(),
            |duration| time.sleep(duration),
            || {
                checks += 1;

                ready(Ok(ControlFlow::<(), _>::Continue(checks)))
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            PollOutcome::Failed {
                reason: PollFailure::TimedOut,
                last: 1
            }
        );
        assert_eq!(checks, 1);
        assert_eq!(*time.sleeps.borrow(), [Duration::from_secs(1)]);
    }

    #[tokio::test]
    async fn final_results_and_errors_win_over_both_limits() {
        let poller = Poller::builder(Duration::from_secs(1))
            .timeout(Duration::ZERO)
            .max_attempts(1)
            .build()
            .unwrap();
        let time = Timeline::new(&[0]);

        let outcome = poll_with(
            &poller,
            || time.now(),
            |_| async { panic!("a final result stops polling") },
            || ready(Ok(ControlFlow::<_, ()>::Break(10))),
        )
        .await
        .unwrap();

        assert_eq!(outcome, PollOutcome::Finished(10));

        for error in [
            HandlerError::from(std::io::Error::other("offline")),
            TerminalError::new("gone").with_code(404).into(),
        ] {
            let expected = error_text(&error);
            let mut error = Some(error);
            let time = Timeline::new(&[0]);

            let result = poll_with(
                &poller,
                || time.now(),
                |_| async { panic!("a check error stops polling") },
                || ready(Err::<ControlFlow<(), ()>, _>(error.take().unwrap())),
            )
            .await;

            assert_eq!(error_text(&result.unwrap_err()), expected);
        }
    }

    #[tokio::test]
    async fn reusing_a_poller_starts_a_fresh_attempt_count() {
        let poller = Poller::builder(Duration::from_secs(3))
            .max_attempts(2)
            .build()
            .unwrap();
        let time = Timeline::new(&[]);
        let mut checks = 0;

        for expected_last in [2, 4] {
            let outcome = poll_with(
                &poller,
                || time.now(),
                |duration| time.sleep(duration),
                || {
                    checks += 1;

                    ready(Ok(ControlFlow::<(), _>::Continue(checks)))
                },
            )
            .await
            .unwrap();

            assert_eq!(
                outcome,
                PollOutcome::Failed {
                    reason: PollFailure::AttemptsExhausted,
                    last: expected_last,
                }
            );
        }

        assert_eq!(*time.sleeps.borrow(), [Duration::from_secs(3); 2]);
    }

    #[tokio::test]
    async fn no_timeout_keeps_checking_without_reading_the_clock() {
        let poller = Poller::builder(Duration::from_secs(3)).build().unwrap();
        let time = Timeline::new(&[]);
        let mut values = [1, 4, 9, 10].into_iter();

        let outcome = poll_with(
            &poller,
            || time.now(),
            |duration| time.sleep(duration),
            || {
                let value = values.next().expect("stop at ten");

                ready(Ok(if value >= 10 {
                    ControlFlow::Break(value)
                } else {
                    ControlFlow::Continue(value)
                }))
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome, PollOutcome::Finished(10));
        assert_eq!(*time.sleeps.borrow(), [Duration::from_secs(3); 3]);
    }

    #[tokio::test]
    async fn reusing_a_poller_starts_a_fresh_timeout() {
        let poller = poller(5);

        for start in [0, 100] {
            let time = Timeline::new(&[start, start + 4, start + 5]);

            let outcome = poll_with(
                &poller,
                || time.now(),
                |duration| time.sleep(duration),
                || ready(Ok(ControlFlow::<(), _>::Continue("pending"))),
            )
            .await
            .unwrap();

            assert_eq!(
                outcome,
                PollOutcome::Failed {
                    reason: PollFailure::TimedOut,
                    last: "pending"
                }
            );
            assert_eq!(*time.sleeps.borrow(), [Duration::from_secs(1)]);
        }
    }

    #[tokio::test]
    async fn no_timeout_still_propagates_check_errors_and_cancellation() {
        let poller = Poller::builder(Duration::from_secs(3)).build().unwrap();
        let time = Timeline::new(&[]);

        let result = poll_with(
            &poller,
            || time.now(),
            |_| async { panic!("an error stops polling") },
            || {
                ready(Err::<ControlFlow<(), ()>, _>(
                    std::io::Error::other("offline").into(),
                ))
            },
        )
        .await;

        assert_eq!(error_text(&result.unwrap_err()), "Retryable error: offline");

        let result = poll_with(
            &poller,
            || time.now(),
            |_| ready(Err(TerminalError::new("cancelled").with_code(409))),
            || ready(Ok(ControlFlow::<(), _>::Continue("pending"))),
        )
        .await;

        assert_eq!(
            error_text(&result.unwrap_err()),
            "Terminal error [409]: cancelled"
        );
    }

    #[tokio::test]
    async fn zero_timeout_observes_once_and_preserves_pending_or_finished() {
        for decision in [ControlFlow::Break(10), ControlFlow::Continue(4)] {
            let time = Timeline::new(&[0]);
            let mut observations = 0;

            let outcome = poll_with(
                &poller(0),
                || time.now(),
                |duration| time.sleep(duration),
                || {
                    observations += 1;

                    ready(Ok(decision))
                },
            )
            .await
            .unwrap();

            assert_eq!(observations, 1);
            assert!(time.sleeps.borrow().is_empty());

            match decision {
                ControlFlow::Break(_) => assert_eq!(outcome, PollOutcome::Finished(10)),
                ControlFlow::Continue(_) => assert_eq!(
                    outcome,
                    PollOutcome::Failed {
                        reason: PollFailure::TimedOut,
                        last: 4
                    }
                ),
            }
        }
    }

    #[tokio::test]
    async fn timeout_keeps_the_latest_pending_value_and_clips_the_last_sleep() {
        // Two seconds in each observation; no observation at the ten-second deadline.
        let time = Timeline::new(&[0, 2, 5, 7, 10]);
        let mut observations = 0;

        let outcome = poll_with(
            &poller(10),
            || time.now(),
            |duration| time.sleep(duration),
            || {
                observations += 1;

                ready(Ok(ControlFlow::<(), _>::Continue(observations)))
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            PollOutcome::Failed {
                reason: PollFailure::TimedOut,
                last: 2
            }
        );
        assert_eq!(*time.sleeps.borrow(), [Duration::from_secs(3); 2]);

        let time = Timeline::new(&[0, 1, 2]);

        let outcome = poll_with(
            &poller(2),
            || time.now(),
            |duration| time.sleep(duration),
            || ready(Ok(ControlFlow::<(), _>::Continue("pending"))),
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            PollOutcome::Failed {
                reason: PollFailure::TimedOut,
                last: "pending"
            }
        );
        assert_eq!(*time.sleeps.borrow(), [Duration::from_secs(1)]);
    }

    #[tokio::test]
    async fn observation_time_counts_and_deadline_equality_expires() {
        for completed_at in [5, 7] {
            let time = Timeline::new(&[0, completed_at]);

            let outcome = poll_with(
                &poller(5),
                || time.now(),
                |duration| time.sleep(duration),
                || ready(Ok(ControlFlow::<(), _>::Continue("still starting"))),
            )
            .await
            .unwrap();

            assert_eq!(
                outcome,
                PollOutcome::Failed {
                    reason: PollFailure::TimedOut,
                    last: "still starting"
                }
            );
            assert!(time.sleeps.borrow().is_empty());
        }
    }

    #[tokio::test]
    async fn a_late_conclusion_wins_without_a_post_completion_clock_read() {
        let seconds = Cell::new(0);

        let outcome = poll_with(
            &poller(5),
            || ready(Ok(UNIX_EPOCH + Duration::from_secs(seconds.get()))),
            |_| async { panic!("no sleep after a conclusion") },
            || {
                seconds.set(20);

                ready(Ok(ControlFlow::<_, ()>::Break("resource failed")))
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome, PollOutcome::Finished("resource failed"));
    }

    #[tokio::test]
    async fn backward_clock_readings_do_not_restore_time() {
        let time = Timeline::new(&[100, 104, 103, 102, 105]);

        let outcome = poll_with(
            &poller(5),
            || time.now(),
            |duration| time.sleep(duration),
            || ready(Ok(ControlFlow::<(), _>::Continue(1))),
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            PollOutcome::Failed {
                reason: PollFailure::TimedOut,
                last: 1
            }
        );
        assert_eq!(*time.sleeps.borrow(), [Duration::from_secs(1); 2]);
    }

    #[tokio::test]
    async fn time_passing_during_sleep_stops_before_another_observation() {
        let time = Timeline::new(&[0, 1, 60]);
        let mut observations = 0;

        let outcome = poll_with(
            &poller(10),
            || time.now(),
            |duration| time.sleep(duration),
            || {
                observations += 1;

                ready(Ok(ControlFlow::<(), _>::Continue(observations)))
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            PollOutcome::Failed {
                reason: PollFailure::TimedOut,
                last: 1
            }
        );
        assert_eq!(observations, 1);
    }

    #[tokio::test]
    async fn observation_errors_preserve_retryable_and_terminal_classification() {
        for error in [
            HandlerError::from(std::io::Error::other("offline")),
            TerminalError::new("gone").with_code(404).into(),
        ] {
            let expected = error_text(&error);
            let mut error = Some(error);
            let time = Timeline::new(&[0]);

            let result = poll_with(
                &poller(0),
                || time.now(),
                |duration| time.sleep(duration),
                || ready(Err::<ControlFlow<(), ()>, _>(error.take().unwrap())),
            )
            .await;

            assert_eq!(error_text(&result.unwrap_err()), expected);
            assert!(time.sleeps.borrow().is_empty());
        }
    }

    #[tokio::test]
    async fn clock_errors_and_sleep_cancellation_are_not_timeouts() {
        // A clock can fail at the start, after an observation, or after a sleep.
        for failure_at in 0..3 {
            let mut reads = 0;

            let result = poll_with(
                &poller(10),
                || {
                    let result = if reads == failure_at {
                        Err(TerminalError::new("clock cancelled").with_code(409))
                    } else {
                        Ok(UNIX_EPOCH)
                    };

                    reads += 1;

                    ready(result)
                },
                |_| ready(Ok(())),
                || ready(Ok(ControlFlow::<(), _>::Continue(1))),
            )
            .await;

            assert_eq!(
                error_text(&result.unwrap_err()),
                "Terminal error [409]: clock cancelled"
            );
        }

        let time = Timeline::new(&[0, 1]);

        let result = poll_with(
            &poller(10),
            || time.now(),
            |_| ready(Err(TerminalError::new("sleep cancelled").with_code(409))),
            || ready(Ok(ControlFlow::<(), _>::Continue(1))),
        )
        .await;

        assert_eq!(
            error_text(&result.unwrap_err()),
            "Terminal error [409]: sleep cancelled"
        );
    }

    #[tokio::test]
    async fn unrepresentable_deadlines_are_terminal_configuration_errors() {
        // Windows has a narrower SystemTime range than the timer protocol. Exercise
        // whichever limit the host reaches first without panicking in test setup.
        let (start, poller, expected) =
            match UNIX_EPOCH.checked_add(Duration::from_millis(u64::MAX)) {
                Some(start) => (start, poller(1), "poll deadline exceeds the timer range"),
                None => (
                    UNIX_EPOCH,
                    Poller::builder(Duration::from_secs(1))
                        .timeout(MAX_DURATION)
                        .build()
                        .unwrap(),
                    "poll timeout exceeds the clock range",
                ),
            };

        let result = poll_with(
            &poller,
            || ready(Ok(start)),
            |_| async { panic!("invalid deadline cannot sleep") },
            || async { panic!("invalid deadline cannot check") },
        )
        .await;
        let result: Result<PollOutcome<(), ()>, _> = result;

        assert_eq!(
            error_text(&result.unwrap_err()),
            format!("Terminal error [400]: {expected}")
        );
    }

    #[test]
    fn duration_ranges_include_the_cap_but_not_the_next_millisecond() {
        for duration in [MAX_DURATION, MAX_DURATION + Duration::from_millis(1)] {
            let interval = Poller::builder(duration).build();
            let timeout = Poller::builder(Duration::from_secs(1))
                .timeout(duration)
                .build();

            for result in [interval, timeout] {
                if duration == MAX_DURATION {
                    assert!(result.is_ok());
                } else {
                    assert_eq!(result.unwrap_err().code(), 400);
                }
            }
        }
    }

    #[tokio::test]
    async fn an_observation_error_after_expiry_is_still_the_original_error() {
        let seconds = Cell::new(0);

        let result = poll_with(
            &poller(5),
            || ready(Ok(UNIX_EPOCH + Duration::from_secs(seconds.get()))),
            |_| async { panic!("no sleep after an error") },
            || {
                seconds.set(20);

                ready(Err::<ControlFlow<(), ()>, _>(
                    TerminalError::new("gone").with_code(404).into(),
                ))
            },
        )
        .await;

        assert_eq!(
            error_text(&result.unwrap_err()),
            "Terminal error [404]: gone"
        );
    }
}
