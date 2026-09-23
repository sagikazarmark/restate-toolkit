//! Reading the clock as a journaled step.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use restate_sdk::prelude::*;

/// The name [`ContextClockExt::now`] journals its read under: the call it stands in for, as
/// the Java SDK names its `instantNow` entry `Instant.now()`.
const NOW_NAME: &str = "SystemTime::now()";

/// A Restate context the clock can be read on.
///
/// A clock read is a step like any other, and for the reason every step is one: the
/// reading is journaled, so a replay stamps what the first run stamped rather than
/// reading the wall clock again and dating the same work twice. Unlike the SDK's
/// `rand_uuid`, which draws from a generator seeded per invocation and journals nothing, a
/// time cannot be derived from a seed, so it has to be journaled.
///
/// A replay finds the entry by its position in the journal, not by its name, so reading
/// twice under one name is fine. The name is a label in Restate's UI and a guard: a
/// replay that meets a step under a different name at that position fails the invocation
/// rather than reading the wrong entry. [`Self::now`] uses `SystemTime::now()`; reach for
/// [`Self::now_named`] when a handler reads the clock more than once and the reads should
/// be told apart.
///
/// A trait with an impl per context rather than one function over `ContextSideEffects`:
/// the SDK does not promise its `run` future is `Send`, and a function generic over the
/// context cannot see through to the future that is, so each impl names its context and
/// lets the compiler look.
///
/// Sealed: it is implemented for the SDK's contexts and nothing else, so methods can be
/// added without breaking anyone.
pub trait ContextClockExt: sealed::Sealed {
    /// Journals one clock read under `SystemTime::now()`, resolving to the time it read, to
    /// the millisecond.
    fn now(&self) -> impl Future<Output = Result<SystemTime, TerminalError>> + Send {
        self.now_named(NOW_NAME)
    }

    /// [`Self::now`], journaled under `name`.
    fn now_named(
        &self,
        name: impl Into<String>,
    ) -> impl Future<Output = Result<SystemTime, TerminalError>> + Send;
}

mod sealed {
    pub trait Sealed {}
}

/// One impl per context, written out by the macro because the bodies are identical and
/// only the named type differs — see the note on [`ContextClockExt`] for why they cannot
/// be one generic impl.
macro_rules! impl_context_clock_ext {
    ($($context:ident),+ $(,)?) => {
        $(
            impl sealed::Sealed for $context<'_> {}

            impl ContextClockExt for $context<'_> {
                fn now_named(
                    &self,
                    name: impl Into<String>,
                ) -> impl Future<Output = Result<SystemTime, TerminalError>> + Send {
                    let reading = self.run(|| async { Ok(unix_millis()) }).name(name);
                    async move { reading.await.map(from_unix_millis) }
                }
            }
        )+
    };
}

impl_context_clock_ext!(
    Context,
    ObjectContext,
    SharedObjectContext,
    WorkflowContext,
    SharedWorkflowContext,
);

/// Milliseconds since the Unix epoch on the host clock: what the journal holds for a read.
///
/// A plain number rather than a serialized `SystemTime`, so the entry reads as a time in
/// Restate's UI and survives a JSON reader that holds numbers as doubles, which millis do
/// for the next quarter million years. Read through `web_time`, because `std` has no clock
/// on `wasm32-unknown-unknown` and panics instead; everywhere else `web_time` is `std`.
fn unix_millis() -> u64 {
    let since_epoch = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX)
}

/// The time a journaled read stands for. Arithmetic on `std`'s `SystemTime` works on every
/// target, so the handler sees the same type wherever it runs.
fn from_unix_millis(millis: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The journal holds millis, not seconds: a stamp in the wrong unit is the kind of
    /// mistake nothing downstream notices until dates read as 1970 or as the far future.
    #[test]
    fn the_clock_is_journaled_as_millis_since_the_epoch() {
        let now = unix_millis();
        // 2020-01-01 and 2100-01-01.
        assert!(
            (1_577_836_800_000..4_102_444_800_000).contains(&now),
            "{now}"
        );
    }

    #[test]
    fn a_journaled_reading_is_the_time_it_was_read() {
        let now = unix_millis();

        let read = from_unix_millis(now);

        assert_eq!(
            read.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_millis(now)
        );
    }
}
