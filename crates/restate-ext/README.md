# Restate Ext

[![ci](https://img.shields.io/github/actions/workflow/status/sagikazarmark/restate-toolkit/dagger.yaml?style=flat-square&label=ci)](https://github.com/sagikazarmark/restate-toolkit/actions/workflows/dagger.yaml)
[![openssf scorecard](https://api.securityscorecards.dev/projects/github.com/sagikazarmark/restate-toolkit/badge?style=flat-square&label=openssf%20scorecard)](https://securityscorecards.dev/viewer/?uri=github.com/sagikazarmark/restate-toolkit)
[![crates.io](https://img.shields.io/crates/v/restate-ext?style=flat-square)](https://crates.io/crates/restate-ext)
[![docs.rs](https://img.shields.io/docsrs/restate-ext?style=flat-square)](https://docs.rs/restate-ext)

Plumbing for [Restate](https://restate.dev/) services built with the [Restate Rust SDK](https://docs.rs/restate-sdk): a replay-safe clock, durable polling, terminal error conversion, and a SIGTERM-aware shutdown signal.

These are the small pieces every durable service needs around its handlers and tends to rewrite: a clock read that survives a replay, an error that stops retries with the status code you choose, and a stop that honours the signal containers are stopped with.

## Installation

Requires Rust **1.92 or later** and Restate SDK **0.12**.

```toml
[dependencies]
restate-ext = "0.1"
restate-sdk = "0.12"
```

To use the shutdown signal, enable the `shutdown` feature:

```toml
[dependencies]
restate-ext = { version = "0.1", features = ["shutdown"] }
```

### Features

| Feature | Default | Description |
| --- | --- | --- |
| `shutdown` | off | [`shutdown_signal`](#shutdown-signal), which listens for process signals through Tokio. Leave it off on wasm, which has no process signals. |

## Usage

### Replay-safe clock

Reading `SystemTime::now()` in a handler is non-deterministic: when Restate replays the journal, for example after a retry or a suspension, the handler reads the clock again and dates the same work differently. `ContextClockExt` reads the clock as a journaled step instead, so a replay sees the time the first attempt read:

```rust
use restate_ext::ContextClockExt;
use restate_sdk::prelude::*;

struct Invoices;

#[restate_sdk::service]
impl Invoices {
    #[restate_sdk::handler]
    async fn issue(&self, ctx: Context<'_>) -> Result<(), HandlerError> {
        // Journaled as `SystemTime::now()`.
        let issued_at = ctx.now().await?;

        // Name the reads when a handler takes more than one.
        let due_at = ctx.now_named("due-date").await?;

        let _ = (issued_at, due_at);
        Ok(())
    }
}
```

`ContextClockExt` is implemented for all five SDK contexts: `Context`, `ObjectContext`, `SharedObjectContext`, `WorkflowContext`, and `SharedWorkflowContext`. Both methods resolve to a `std::time::SystemTime`, with millisecond precision.

- `now()` journals the read under the name `SystemTime::now()`.
- `now_named(name)` journals it under `name`.

A replay matches journal entries by position, not by name, so reading the clock more than once under the same name is correct. The name is what an operator sees in Restate's UI, and it acts as a guard: a replay that finds a step with a different name at that position fails the invocation instead of reading the wrong entry. Use `now_named` to tell several reads apart.

The journal stores the time as milliseconds since the Unix epoch, so the entry reads as a number in Restate's UI. The clock is read through [`web-time`](https://docs.rs/web-time), so it also works on `wasm32-unknown-unknown`, where `std`'s clock panics. (Building the Restate SDK for that target needs `getrandom` 0.2 with its `js` feature enabled.)

### Durable polling

`restate_ext::poll::Poller` runs a check repeatedly with a fixed delay:

```rust,ignore
use std::{ops::ControlFlow, time::Duration};
use restate_ext::poll::{Poller, PollOutcome};

let poller = Poller::builder(Duration::from_secs(1))
    .timeout(Duration::from_secs(60))
    .max_attempts(10)
    .build()?;

let outcome = poller.poll(&ctx, || async {
    let value = ctx.get::<u64>("value").await?.unwrap_or_default();

    if value >= 10 {
        Ok(ControlFlow::Break(value))
    } else {
        Ok(ControlFlow::Continue(value))
    }
}).await?;
```

The callback returns `ControlFlow::Break(result)` to finish, or
`ControlFlow::Continue(value)` to keep waiting. The result is
`PollOutcome::Finished(result)` or `PollOutcome::Failed { reason, last }`.
The failure reason is `PollFailure::TimedOut` or `PollFailure::AttemptsExhausted`,
with the last value available for a useful message. Check and Restate errors
propagate through the outer `Result`.

`.build()` validates the configuration before polling can start. Both
`Poller::builder(interval)` and `PollerBuilder::new(interval)` require the interval
up front; timeout and attempt limits are optional. Without either limit, polling
continues until the check finishes, returns an error, or the invocation is cancelled.
Without `.timeout(...)`, it makes no clock reads. A poller can be reused; every
`.poll()` call starts its own timeout and attempt count.

`max_attempts` includes the first check: a limit of ten permits ten checks and at
most nine sleeps. One checks once; zero is rejected by `.build()`. Retries inside
a check and Restate replay do not consume extra attempts.

A final result or check error always wins. After a check asks to keep waiting,
the attempt limit is checked first, followed by the timeout. If both limits apply
at that point, the reason is `AttemptsExhausted`.

For this state-polling example, use a `SharedObjectContext` and `#[handler(lazy_state)]`:
the shared handler allows updates, and lazy state makes each new read see those updates.

The callback owns durable I/O: read state through the context, make a durable service
call, or individually await a `ctx.run` for external I/O. The condition itself is plain
Rust. Neither the callback nor its output needs an additional serialization layer
from the poller.

- Checks once immediately; zero timeout means one check without sleeping.
- Sleeps through Restate between checks, shortening the delay near timeout.
- With a timeout, uses journaled `poll.clock` readings and checks the deadline between operations.
- Accepts a conclusive observation even if it completes after the deadline. The timeout
  cannot interrupt an in-flight operation or retries; replay preserves earlier decisions.
- Supports all five SDK contexts. Exclusive object handlers retain their lock while waiting.
- Requires a positive interval and whole-millisecond durations, each at most
  `u64::MAX / 2` milliseconds. Invalid settings produce a terminal 400 error from `.build()`.

The standalone [`poll-counter` example](../../examples/poll-counter) shows a counter
reaching ten, with runnable requests and an end-to-end test.

### Terminal errors

`TerminalErrorExt::terminal` turns any `Result` error into a `TerminalError`, which Restate does not retry. Unlike the SDK's trait of the same name, which resolves to a `HandlerError`, it keeps the `TerminalError`, so you can still set a status code:

```rust
use restate_ext::TerminalErrorExt;
use restate_sdk::prelude::*;

async fn handle(input: &str) -> Result<i32, HandlerError> {
    let parsed: i32 = input
        .parse()
        .terminal()
        .map_err(|error| error.with_code(400))?;
    Ok(parsed)
}
```

Without a code, the error has the default status code, 500. The message is the original error's `Display` output.

Import the trait by name. The named import shadows the SDK's trait from `restate_sdk::prelude::*`, so `.terminal()` is not ambiguous.

### Shutdown signal

The SDK's HTTP server only stops on SIGINT (Ctrl-C), but `docker stop` and Kubernetes send SIGTERM. Without handling it, the process is killed mid-step and every in-flight invocation has to be replayed.

`shutdown_signal` resolves on either signal. Pass it to `HttpServer::serve_with_cancel` (the SDK's `http_server` feature):

```rust,ignore
use restate_ext::shutdown_signal;
use restate_sdk::http_server::HttpServer;
use tokio::net::TcpListener;

let listener = TcpListener::bind("0.0.0.0:9080").await?;

HttpServer::new(endpoint)
    .serve_with_cancel(listener, shutdown_signal())
    .await;
```

Either signal starts the SDK's graceful drain: the listener closes, open invocations get up to ten seconds to finish, and Restate retries whatever is left. The received signal is logged through [`tracing`](https://docs.rs/tracing). SIGTERM is only handled on Unix.

See the [API documentation](https://docs.rs/restate-ext) for details, and [`examples/greeter`](../../examples/greeter) for both used in a complete endpoint.

## Testing

The end-to-end tests run against a real `restate-server`. They check clock replay and
polling across retries, pauses, and cancellation, including timer reuse and a late
observation authorized by a replayed clock check. They are Unix only and ignored by
default; run them with a `restate-server` binary:

```shell
RESTATE_SERVER_BIN="$PWD/restate-server" cargo test -p restate-ext -- --ignored
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
