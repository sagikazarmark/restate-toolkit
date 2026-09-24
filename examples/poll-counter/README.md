# Poll a counter until it reaches ten

A standalone [Restate](https://restate.dev/) example of fixed-delay polling with
[`restate-ext::poll`](../../crates/restate-ext/src/poll.rs).

One Virtual Object, **`Counter`**, has two handlers:

- **`add`** updates the count.
- **`wait`** reads the count once per second until it reaches or passes ten, with a
  hard-coded 30-second timeout. It is a shared handler, so `add` can run while it waits.

## The pattern

The waiting handler in [`src/lib.rs`](src/lib.rs) reads its own state and checks a
plain Rust condition:

```rust
let poller = Poller::builder(Duration::from_secs(1))
    .timeout(Duration::from_secs(30))
    .build()?;

let outcome = poller.poll(&ctx, || async {
    let value = ctx.get::<u64>("value").await?.unwrap_or_default();

    if value >= 10 {
        Ok(ControlFlow::Break(value))
    } else {
        Ok(ControlFlow::Continue(value))
    }
})
.await?;
```

The state read goes through Restate; the condition is an ordinary Rust comparison.

Two details let this handler see concurrent updates:

- **`SharedObjectContext`** allows `add` to run while `wait` is active.
- **`#[handler(lazy_state)]`** makes each new `ctx.get` read from the server. Without
  lazy state, reads can keep returning the state loaded when the invocation started.

State reads are journaled. On replay, a completed read returns its recorded value;
later checks make new reads that can see updates.

For an external HTTP API, replace the state read with an individually awaited
`ctx.run` that performs the request. Keep the domain condition separate, and give the
HTTP client an appropriate per-request timeout. The polling callback already performs
durable operations, so it must not itself be wrapped in another `ctx.run`.

The builder validates the interval and timeout. `poller.poll` handles the fixed delay,
journaled clock reads, and timeout. `Finished(value)`
returns the count. `Failed { reason, last }` reports a polling limit. This example
only configures a timeout, so a failure becomes a terminal 408 response with the
last count. Other errors propagate unchanged.

The example sets a 30-second timeout explicitly. The library also supports
`.max_attempts(n)` to limit the number of checks. Omitting both limits creates a
poller that waits until the check finishes, an error occurs, or the invocation is cancelled.

## Run it

Start a Restate server, then run the worker:

```shell
cargo run -p poll-counter
```

It listens on port 9080. Register it with Restate (use a host address reachable from
the server if it runs in a container):

```shell
restate deployments register http://localhost:9080
```

Start waiting in one terminal. The counter key is in the URL; no request body is needed:

```shell
curl -X POST localhost:8080/Counter/demo/wait
```

In another terminal, update the same counter:

```shell
curl localhost:8080/Counter/demo/add --json '4'
curl localhost:8080/Counter/demo/add --json '5'
curl localhost:8080/Counter/demo/add --json '1'
```

The waiting request returns `10` after the next check. An update taking the
counter straight to `12` works too. Counts persist per key; choose a new key for a new run.

If the counter stays below ten, the request returns 408 after the 30-second timeout,
with the last count in the error message. A counter already at or above ten returns
on the first check.

## Timing and concurrency

- The first check happens immediately, without a polling sleep.
- If the count is still below ten, the handler waits one second after the check
  finishes before checking again. The wait is shortened near the timeout.
- The timeout is checked between operations. It cannot interrupt an in-flight request
  or its retries, and a check can return a final result after expiry.
- Replay preserves recorded clock decisions. A check before a failure can authorize
  another operation after recovery, even when the wall-clock deadline has passed.
- `wait` holds no exclusive counter lock. Using `ObjectContext` instead of
  `SharedObjectContext` would block `add` until the wait completes.

## Tests

```shell
cargo test --locked -p poll-counter
```

The end-to-end test in [`tests/e2e.rs`](tests/e2e.rs) starts a real server and:

1. Waits for a journaled sleep after reading nine.
2. Updates the counter to ten through a separate invocation and verifies completion.
3. Verifies timeout diagnostics for a counter left at seven, using the real 30-second timeout.
4. Checks that a counter already above ten returns without sleeping.

Run it explicitly with a [Restate server binary](../../crates/restate-e2e-harness/README.md#getting-a-restate-server):

```shell
RESTATE_SERVER_BIN=/path/to/restate-server \
  cargo test --locked -p poll-counter --test e2e -- --ignored
```

Verified against Restate **1.7.8**. The library also has real-server tests for replay,
paused time consuming the timeout, timer reuse, and cancellation in
[`restate-ext/tests/poll.rs`](../../crates/restate-ext/tests/poll.rs).

## CI

The [GitHub Actions workflow](../../.github/workflows/dagger-examples-poll-counter.yaml)
runs the repository's Rust image with `restate-server` installed. It selects both
`poll-counter` and `restate-ext`, including ignored tests, so the example and the
library's replay, pause, and cancellation scenarios run in CI too.

Run the same command from the repository root, preserving the Cargo workspace:

```shell
dagger call rust test --package=poll-counter,restate-ext --args=--include-ignored
```
