//! Turning any error into a terminal one.
//!
//! The SDK's own `TerminalErrorExt` resolves to a `HandlerError`, which has already lost
//! the terminal error and its status code. This one resolves to the [`TerminalError`], so a
//! code can still be set before `?` converts it:
//!
//! ```rust,no_run
//! use restate_ext::TerminalErrorExt;
//! use restate_sdk::prelude::*;
//!
//! async fn handle() -> Result<(), HandlerError> {
//!     let parsed: i32 = "not a number"
//!         .parse()
//!         .terminal()
//!         .map_err(|error| error.with_code(400))?;
//!     Ok(())
//! }
//! ```
//!
//! Import it by name over the prelude's glob: a named import shadows the SDK trait, so
//! `.terminal()` is not ambiguous.
//!
//! A copy of the SDK trait as changed by
//! [restatedev/sdk-rust#132](https://github.com/restatedev/sdk-rust/pull/132); deprecate it
//! in favour of the SDK's once a release ships that change.

use restate_sdk::prelude::*;

/// Extension trait for converting any `Result` error into a [`TerminalError`].
///
/// This trait provides a convenient way to convert errors from fallible operations
/// into terminal errors that will not be retried by Restate.
///
/// # Example
///
/// ```rust,no_run
/// use restate_ext::TerminalErrorExt;
/// use restate_sdk::prelude::*;
///
/// async fn handle() -> Result<(), HandlerError> {
///     let parsed: i32 = "not a number".parse().terminal()?;
///     Ok(())
/// }
/// ```
pub trait TerminalErrorExt<T, E> {
    /// Convert the error into a [`TerminalError`] with the default status code (500).
    ///
    /// # Errors
    ///
    /// Returns the error as a [`TerminalError`], its message the error's `Display`.
    fn terminal(self) -> Result<T, TerminalError>;
}

impl<T, E> TerminalErrorExt<T, E> for Result<T, E>
where
    E: std::fmt::Display + Send + Sync + 'static,
{
    fn terminal(self) -> Result<T, TerminalError> {
        self.map_err(|err| TerminalError::new(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_becomes_a_terminal_error_with_its_message_and_the_default_code() {
        let error = "not a number".parse::<i32>().terminal().unwrap_err();

        assert_eq!(error.message(), "invalid digit found in string");
        assert_eq!(error.code(), 500);
    }

    #[test]
    fn a_code_can_be_set_before_the_error_leaves_the_handler() {
        let error = "not a number"
            .parse::<i32>()
            .terminal()
            .map_err(|error| error.with_code(400))
            .unwrap_err();

        assert_eq!(error.code(), 400);
    }

    #[test]
    fn a_success_passes_through() {
        assert_eq!("42".parse::<i32>().terminal().unwrap(), 42);
    }
}
