//! The Greeter service: what the example serves, independent of how it is
//! configured or served.
//!
//! The binary binds [`Greeter`] into an endpoint with its configured policy;
//! the end-to-end test binds it with the policy it asserts.
//!
//! The greeting depends on the time of day, read with `restate-ext`'s
//! [`ContextClockExt::now`] rather than `SystemTime::now()`: the reading is
//! journaled, so a retried or resumed invocation greets with the time its first
//! attempt read instead of reading the clock again. What to say at a given time
//! is plain code, [`greeting`], tested without Restate.

use std::time::{SystemTime, UNIX_EPOCH};

use restate_ext::ContextClockExt;
use restate_sdk::prelude::*;

/// A greeting service.
pub struct Greeter;

#[restate_sdk::service(name = "Greeter")]
impl Greeter {
    /// Greets `name` for the time of day in UTC, journaling the time as the
    /// `SystemTime::now()` run and the greeting as the `greet-person` run.
    #[handler]
    async fn greet(&self, ctx: Context<'_>, name: String) -> HandlerResult<String> {
        let now = ctx.now().await?;
        let greeting = ctx
            .run(|| async move { Ok(greeting(&name, now)) })
            .name("greet-person")
            .await?;
        Ok(greeting)
    }
}

/// The greeting for `name` at `time`, by the hour in UTC.
#[must_use]
pub fn greeting(name: &str, time: SystemTime) -> String {
    let hour = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 3600
        % 24;
    match hour {
        5..=11 => format!("Good morning, {name}!"),
        12..=16 => format!("Good afternoon, {name}!"),
        17..=21 => format!("Good evening, {name}!"),
        _ => format!("Hello, {name}, you're up late!"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// `hour:minute` UTC on the first day of the epoch.
    fn at(hour: u64, minute: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_hours(hour) + Duration::from_mins(minute)
    }

    #[test]
    fn the_greeting_follows_the_hour() {
        for (time, expected) in [
            (at(4, 59), "Hello, Ada, you're up late!"),
            (at(5, 0), "Good morning, Ada!"),
            (at(11, 59), "Good morning, Ada!"),
            (at(12, 0), "Good afternoon, Ada!"),
            (at(17, 0), "Good evening, Ada!"),
            (at(21, 59), "Good evening, Ada!"),
            (at(22, 0), "Hello, Ada, you're up late!"),
        ] {
            assert_eq!(greeting("Ada", time), expected, "{time:?}");
        }
    }

    /// Only the time of day counts, not the date.
    #[test]
    fn a_later_day_greets_by_the_same_hour() {
        let a_year_later = at(9, 30) + Duration::from_hours(365 * 24);

        assert_eq!(greeting("Ada", a_year_later), "Good morning, Ada!");
    }
}
