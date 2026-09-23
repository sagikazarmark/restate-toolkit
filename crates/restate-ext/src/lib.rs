//! Plumbing for [`restate_sdk`] services, with no application in it.
//!
//! What a durable service needs around its handlers and keeps rewriting: a clock read that
//! survives a replay and a stop that honours SIGTERM.
//!
//! - [`clock`] — read the wall clock as a journaled step, so a replay stamps what the first
//!   run stamped.
//! - [`errors`] — turn any error into a terminal one, and still set its code.
//! - `shutdown` — a drain that starts on SIGTERM as well as SIGINT, behind the `shutdown`
//!   feature. Wasm has no process signals, so leave it off there.
//!
//! Every context trait here is implemented for all five SDK contexts, and names the steps it
//! journals. A replay finds an entry by its position, not its name; the name is what an
//! operator finds the step by in Restate's UI, and a replay that meets a different name at
//! that position fails rather than reading the wrong entry.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod clock;
pub mod errors;
#[cfg(feature = "shutdown")]
#[cfg_attr(docsrs, doc(cfg(feature = "shutdown")))]
pub mod shutdown;

pub use clock::ContextClockExt;
pub use errors::TerminalErrorExt;
#[cfg(feature = "shutdown")]
#[cfg_attr(docsrs, doc(cfg(feature = "shutdown")))]
pub use shutdown::shutdown_signal;
