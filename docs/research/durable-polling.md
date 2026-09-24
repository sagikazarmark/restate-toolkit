# Durable readiness polling in Rust / Restate

Researched and reviewed by three independent subagents 2026-09-24. Implemented in
[`restate-ext::poll`](../../crates/restate-ext/src/poll.rs), with the standalone
[`poll-counter` example](../../examples/poll-counter). The design and source research below
record the implementation contract.

## Recommendation

Use a sequential durable loop: **check → classify as finished/pending → durable sleep → check again**, with a configurable fixed delay and an optional cooperative timeout. Keep domain pending separate from operational failure. Let the caller supply the check; external I/O belongs in an individually awaited `ctx.run`, or in a durable Restate service call. These are design recommendations derived from the API constraints below, not an existing SDK polling API. [R1, R2]

Use a journaled-clock deadline checked between operations. Rust SDK 0.12.x cannot directly race an arbitrary async condition, a whole polling loop, or `ctx.run` against a durable timer using `restate_sdk::select!`. The budget therefore cannot interrupt an in-flight observation or revoke a pre-crash eligibility decision during replay. [R1, R3, R4]

## Version scope

- Workspace `Cargo.toml` declares `restate-sdk = { version = "0.12.0", default-features = false }`; `Cargo.lock` resolves **0.12.1**, with shared core **7.0.3**. The current official [Rust documentation entry point](https://docs.restate.dev/develop/rust) redirects to the latest Rustdoc, which was 0.12.1 when checked.
- Claims below target **0.12.0 and 0.12.1**. The [tag comparison](https://github.com/restatedev/sdk-rust/compare/v0.12.0...v0.12.1) leaves the relevant context, timer, run, and select source unchanged. The 0.12.1 additions concern ingress clients. Do not infer Rust availability from TypeScript/Java/Go APIs. [R0]
- SDK `select!` was introduced in 0.4.0 and remains documented as experimental; `DurableFuturesUnordered` arrived in 0.10.0; durable `map`/`map_ok`/`map_err` and the redesigned invocation handle arrived in 0.11.0. All are available at this repo's SDK version. [R0]

## Exact API constraints

Relevant declarations, with bodies omitted; `Serialize`/`Deserialize` here are **`restate_sdk::serde` traits**, not directly the serde traits. [R1, R2, R5]

```rust
pub trait ContextTimers<'ctx>: private::SealedContext<'ctx> {
    fn sleep(&self, duration: Duration)
        -> impl DurableFuture<Output = Result<(), TerminalError>> + Send + 'ctx;
}

pub trait ContextSideEffects<'ctx>: private::SealedContext<'ctx> {
    fn run<R, F, T>(&self, run_closure: R)
        -> impl RunFuture<Result<T, TerminalError>> + 'ctx
    where
        R: RunClosure<Fut = F, Output = T> + Send + 'ctx,
        F: Future<Output = HandlerResult<T>> + Send + 'ctx,
        T: Serialize + Deserialize + 'static;

}

pub trait RunClosure {
    type Output: Deserialize + Serialize + 'static;
    type Fut: Future<Output = HandlerResult<Self::Output>>;
    fn run(self) -> Self::Fut;
}
// Blanket implementation for FnOnce() -> Fut with the above result bounds.

pub trait RunFuture<O>: Future<Output = O> {
    fn retry_policy(self, retry_policy: RunRetryPolicy) -> Self;
    fn name(self, name: impl Into<String>) -> Self;
}

pub trait DurableFuture: Future + macro_support::SealedDurableFuture { /* ... */ }

pub trait CallFuture: DurableFuture<Output = Result<Self::Response, TerminalError>> {
    type Response;
    fn invocation_handle(&self)
        -> impl Future<Output = Result<InvocationHandle, TerminalError>> + Send;
    // Deprecated invocation_id() omitted.
}
// InvocationHandle inherent method:
// pub fn cancel(&self)
```

Implications for a helper:

- **Context capabilities:** `ContextTimers<'ctx>` provides sleep; the toolkit's `ContextClockExt` provides journaled clock reads with a `Send` future. Both support all five SDK contexts: `Context`, `ObjectContext`, `SharedObjectContext`, `WorkflowContext`, `SharedWorkflowContext`. They are sealed; use generics, not a user-supplied context implementation or `dyn ContextTimers` (not dyn-compatible). [R1, R6]
- `sleep` needs `&self`, not `&mut self`. Do not impose `'static` on the borrowed context/callback just because the journaled **output** requires `'static`. If the helper must return a `Send` future, its captured values and futures must support that; holding `&C` across awaits normally entails `C: Sync`. This is a Rust future/capture requirement, not a `ContextTimers` supertrait. [R1]
- An ordinary `Future` callback permits individually awaited `ctx.run` operations. Requiring `DurableFuture` instead substantially narrows supported observations to SDK-selectable operations. SDK state `get` and `peek_promise` also expose ordinary `Future`s, not `DurableFuture`s. [R1]
- Wrapping a durable operation in `async { ... }` does not preserve `DurableFuture`. The SDK's own `DurableFuture::map`, `map_ok`, and `map_err` do preserve it; arbitrary `FutureExt` combinators do not promise that. The trait's hidden supertrait exposes a VM context and notification handle; it is not a supported general-purpose adapter for composite polling futures. [R1, R0]

## Durable sleeps, clocks, and replay

- `ctx.sleep(delay).await?` uses a server-tracked durable timer and can suspend the invocation. It survives endpoint failures/restarts. The SDK eagerly registers the timer when `sleep` is called, before its future is awaited. Internally it computes an absolute wake-up time using `SystemTime::now()` and passes it to shared core. That is SDK protocol machinery, **not** a replay-safe user clock. SDK/server clock skew can affect actual sleep duration. [R4, R6]
- Shared core 7.0.3 tests explicitly replay an existing sleep command/completion despite a newly supplied wake-up timestamp; replay does not restart a completed sleep or issue a fresh timer for the same journal position. [R7]
- There is **no public `ctx.now()`/`ContextClock` API** in the reviewed 0.12.x context surface. For branch decisions based on time, journal serializable wall-clock samples with individually awaited `ctx.run` operations. A start sample and later samples can then be replayed consistently. A direct `Instant::elapsed()` or `SystemTime::now()` in polling control flow is not journaled, and elapsed time resets/changes on replay. A journaled wall clock is not guaranteed monotonic; account for backward clock movement. This clock recipe is an inference from the documented journaling contract, not a built-in Rust SDK deadline feature. [R1, R2, R4]
- Replay reconstructs the loop and retained observations by re-executing deterministic control flow while reusing recorded observations/sleeps. Keep configuration replay-stable (input, journaled configuration, or deployment-compatible constants). Do not use process-global mutable counters or changing environment configuration to determine the replay path. [R2, R8]
- An exclusive Virtual Object invocation retains exclusivity while sleeping. Waiting for another exclusive handler on the **same key** to update the readiness state can deadlock. Shared handlers can run concurrently but cannot mutate that object's state. [R1, R6]

## `ctx.run` retries versus domain pending

- `ctx.run` persists an observation result and reuses it on replay. **`Ok(Pending)` is a successful journaled step**; it will not trigger `RunRetryPolicy`. Polling again requires a new `run` operation at the next loop iteration. A transient I/O failure returned as `HandlerError` is instead subject to Restate retry handling. A terminal error stops that retry path. [R2, R4]
- Keep each observation's retry policy distinct from the readiness polling schedule. Encoding ordinary pending as a retryable error is technically possible, but moves readiness into infrastructure retry semantics and loses a successful pending observation as a separate checkpoint. Recommendation: reserve run retries for actual operational failures. [R2, R4]
- Immediately await each `ctx.run`; the SDK explicitly warns against interleaving it with other context operations. **Do not put the whole durable polling loop inside `ctx.run`**: context calls, including `sleep`, service calls, and nested `run`, are forbidden inside its closure. A whole external-I/O loop inside one run also lacks intermediate journal checkpoints. [R2]
- Important default distinction: omitting `.retry_policy(...)` uses invocation retry behavior (SDK documentation describes indefinite retries by default). Explicit `RunRetryPolicy::default()` sets initial delay **100 ms**, factor **2**, maximum delay **2 s**, no maximum attempts, and **50 s maximum duration**. `RunRetryPolicy::new()` instead has 100 ms initial delay, factor 1, and no maxima. Deployment/server invocation policy can differ. [R2, R4]
- Exact setters consume and return `Self`: `initial_delay(Duration)`, `exponentiation_factor(f32)`, `max_delay(Duration)`, `max_attempts(u32)`, `max_duration(Duration)`. Attempts include the initial execution. Limits may be exceeded because the side effect executes before its result reaches Restate. Exhaustion produces a `TerminalError`; `max_duration` is **not a preemptive timeout of one hung attempt**. [R2, R4]

## Durable timeout/select and cancellation

- `restate_sdk::select!` accepts SDK durable futures, such as a `CallFuture`, awakeable, promise, signal, or sleep. Its own documentation demonstrates awakeable-versus-sleep. It sends a `FirstCompleted` wait over VM notification handles, rather than racing arbitrary Rust futures with Tokio. [R3, R10]
- **`RunFuture` is not `DurableFuture` in 0.12.x.** The select implementation explicitly states async run is unsupported. Thus neither `select! { ... = ctx.run(...) ... }` nor racing a generic async polling helper is available via the supported API. `tokio::select!`/`tokio::time::timeout` are not durable substitutes for that orchestration. [R2, R3, R10]
- For a selectable service call, obtain its invocation handle before moving the call future into select, and race the call against `ctx.sleep(timeout)`. On timeout, explicitly request cancellation via `InvocationHandle::cancel(&self)` if that is the intended policy. This method is synchronous, fire-and-forget; it does not confirm completed cancellation. [R5]
- Losing futures owned by the macro are dropped at scope exit. **Dropping a call future does not cancel the remote invocation; dropping a sleep future does not retract its registered timer.** Source evidence: registration happens eagerly, the future implementations have no cancellation-on-drop behavior, and select emits no loser-cancellation command. A timeout must not imply remote work stopped or external side effects were undone. [R3, R4, R11]
- Select supports `on_cancel => ...`; without it, it propagates a terminal cancellation error (code 409). Bind and propagate the sleep result rather than treating every result as successful timer expiry. Pattern mismatch goes to `else` (or panics if omitted); it does not resume selecting like Tokio's pattern-disabled-branch behavior. [R3]
- Cancellation is cooperative, surfaced at SDK await boundaries; it is not immediate interruption of arbitrary external I/O. In the reviewed Rust implementation, an executing run closure is polled to completion before processing its result. Use an I/O-client timeout inside the run closure to bound that individual attempt, and distinguish that from a durable whole-poll deadline. One-way sends are detached from cancellation of the parent call tree. [R4, R8]
- Design consequence: a boundary-checked journaled deadline is suitable for a generic sequential helper if its non-preemptive semantics are explicit. A durable caller-side timeout needs a selectable service call (or other SDK durable future) and an explicit cancellation policy; even then it does not promise preemptive interruption of external work. Summed sleep durations alone exclude observation latency, retry time, and downtime. [R3, R4]

## Official examples found

The reviewed Rust SDK API/examples do not provide a generic readiness-polling helper. Closest verified examples:

- Official [Rust building blocks](https://github.com/restatedev/examples/blob/main/rust/basics/src/p1_building_blocks.rs): durable sleep, delayed calls, and individual journaled side effects; explicitly explains resuming the remaining sleep after a crash. This is a moving `main` example, not a version-pinned API authority.
- SDK [periodic task / cron](https://github.com/restatedev/sdk-rust/blob/v0.12.1/examples/cron.rs): an active flag and self-scheduling `send_after`, returning between iterations so a stop handler can run. It illustrates a recurring-task architecture, not an await-until-ready abstraction.
- SDK [`select!` documentation](https://docs.rs/restate-sdk/0.12.1/restate_sdk/macro.select.html): awakeable-versus-durable-sleep. [R3]

The absence statement is limited to the reviewed Rust API surface and examples, not all Restate repositories or SDK languages.

## Toolkit interface

The public API follows the reviewed design below.

Agreed public names: `Poller`, `PollerBuilder`, `PollOutcome::{Finished, Failed}`, and `PollFailure::{TimedOut, AttemptsExhausted}`. `Poller` has private, validated configuration; its `.poll(ctx, check)` method starts a fresh wait each time. `interval` is required; timeout and `max_attempts` both default to `None`. With neither limit set, polling is indefinite. “Budget” below describes a configured cooperative timeout, not a separate API concept.

The generic `.poll()` method in `restate-ext::poll` has three responsibilities:

1. Invoke a caller-provided async `check` callback.
2. Wait a configurable fixed delay through Restate after pending observations.
3. Stop on a domain decision, attempt exhaustion, or sampled timeout expiry, retaining the last pending value.

The callback returns `Result<ControlFlow<T, P>, HandlerError>`: `Break(T)` means the domain has reached a conclusion (including a permanent domain failure), `Continue(P)` retains diagnostic state and keeps waiting, and `Err` propagates operational failure. This directly accommodates the supplied domain decision, without a new readiness trait or requiring readiness to be a boolean. The caller already journals its observations; the helper must not wrap the callback in another `ctx.run`.

Types and call shape:

```rust
pub struct Poller {
    interval: Duration,
    timeout: Option<Duration>,
    max_attempts: Option<u32>,
}

pub enum PollOutcome<T, P> {
    Finished(T),
    Failed { reason: PollFailure, last: P },
}

pub enum PollFailure {
    TimedOut,
    AttemptsExhausted,
}

let poller = Poller::builder(Duration::from_secs(1))
    .timeout(Duration::from_secs(30))
    .max_attempts(10)
    .build()?;

// poller.poll(&ctx, || async { ... }).await
//   -> Result<PollOutcome<T, P>, HandlerError>
```

The callback is `FnMut() -> Fut`, with `Fut: Future<Output = Result<ControlFlow<T, P>, HandlerError>> + Send`. The callback itself and retained `P` must be `Send`, and the borrowed context must be `Sync`. Return `Break(T)` immediately, so `T` need not survive another await. Compile-check the promised `Send` future. Neither callbacks nor their borrowed inputs need `'static`; `FnMut() -> Fut` does not support lending mutable borrows of the closure's own captured state across calls. Examples should clone owned request inputs per observation when needed.

Always observing first means budget exhaustion necessarily has a pending value: `last: P` needs neither `Option` nor `Clone`. Only values crossing journal/state/service boundaries require serialization; the helper should impose no serialization bound on `T`, `P`, or its result.

The callback is responsible for replay-safe I/O and deterministic classification. It is invoked sequentially, and completed durable observations replay their recorded values. An unrecorded external request can execute again after failure. Configuration and captured state must reconstruct consistently on replay. The helper propagates operational errors rather than adding another retry loop. `Failed` describes a polling limit, while check/Restate errors use the outer `Result`.

### Attempt limit contract

`max_attempts(u32)` counts logical checks, including the first. One allows exactly one check and no sleeps; zero is rejected by `.build()`. Without a timeout, N checks permit at most N−1 sleeps. Each `.poll()` call creates its own local countdown; replay reconstructs it without a separate journal entry. SDK retries within a check do not consume additional polling attempts.

On `Break`, return `Finished` immediately; on a check error, propagate the error. After `Continue(last)`, decrement the countdown and return `Failed { reason: AttemptsExhausted, last }` if it is exhausted, before any further clock read or sleep. This reason takes precedence if timeout also applies at that point. Otherwise continue with timeout handling below. Neither limit interrupts an in-flight check or its retries.

### Deadline contract

The repo already provides a suitable journaled clock in [`ContextClockExt`](../../crates/restate-ext/src/clock.rs). Its per-concrete-context implementations also address the SDK's missing generic `Send` guarantee for `run` futures. Reuse this clock rather than building a second clock layer. A deadline-capable helper can use `ContextTimers<'ctx> + ContextClockExt + Sync`.

Resolved semantics:

1. `PollerBuilder::build` validates configuration once: require a positive interval, whole-millisecond durations, and a positive attempt limit if set. Each duration is capped at `u64::MAX / 2` milliseconds, reserving timestamp headroom in the unsigned 64-bit millisecond timer protocol. Invalid inputs produce a terminal 400 error. With a timeout, deadline addition is checked during `.poll()` against the host clock range and the protocol's epoch-millisecond range. The SDK adds a duration to its host timestamp; shared core converts the resulting milliseconds to `u64`. [R4]
2. With a timeout, journal the start before the first check and compute the absolute deadline with checked arithmetic. Immediately await each clock read. Without a timeout, skip clock reads and wait the full interval between pending checks until completion, attempt exhaustion, an error, or cancellation.
3. Perform one observation without an initial polling sleep. A zero budget means exactly one observation for valid configuration.
4. `Break(T)` returns `Finished(T)` immediately, even if the observation completed after the deadline. Observation errors propagate unchanged rather than becoming budget expiry.
5. `Continue(P)` replaces the last pending value. Check attempt exhaustion first as described above. With zero timeout, return `Failed { reason: TimedOut, last }` immediately; otherwise sample the journaled clock when a timeout is configured.
6. Maintain a replay-reconstructed running maximum of clock samples, initialized to the start. If that value is **greater than or equal to** the deadline, return `Failed { reason: TimedOut, last }`.
7. Sleep for `min(interval, deadline - max_sampled_time)`, using the running maximum from step 6 and propagating sleep errors. Then sample/check time again before another observation. A clipped sleep reaching the deadline does not earn one final observation.

This is **fixed delay**, measured after a pending observation and its clock check, rather than fixed-rate ticks. Clock/journal overhead can extend the spacing.

The budget does not interrupt SDK retries or hung external I/O. Latency, retry time, and downtime consume it when reflected in fresh clock samples. The running maximum prevents backward samples from replenishing the sampled budget, but does not create a monotonic wall clock or guarantee an elapsed-time bound. Clock errors propagate just like sleep errors; cancellation is never converted into normal budget expiry.

Replay preserves earlier decisions. A crash after a pre-observation clock check can result in an observation **starting after the real deadline** on recovery. Similarly, a crash between sampling remaining time and registering sleep can cause a fresh timer to be scheduled from a stale remaining duration. Already-journaled sleeps retain their original target. The guarantee is to stop at an expired **sampled boundary**, not to forbid all work after the real-world deadline. [R2, R4, R7]

The original prototype's `timeout / interval` was an attempt limit: 600 seconds / 3 seconds produced 200 observations and 199 sleeps (597 seconds of scheduled waiting). The current poller uses clock samples for timeout and an independent `max_attempts` setting for the check count.

### Scheduling and placement

Both `Poller::builder(interval)` and `PollerBuilder::new(interval)` require an interval up front; the builder has no empty/default form. Duration values are validated by `.build()`. Omitting both timeout and attempt limits permits indefinite polling. The implementation uses fixed delay only, with no schedule abstraction or additional dependency. Private fields allow configuration to evolve without exposing invalid `Poller` values.

Keep this helper usable inside a service, workflow, or shared object handler; a separate deployed polling service is a caller architecture choice. The supplied `SharedObjectContext` does not hold an exclusive object lock. When polling state directly, enable `lazy_state` on the shared handler so new reads can see updates. Reusing the helper inside an exclusive object handler would retain that exclusivity while waiting. Repeated independently scheduled polling invocations are a separate lifecycle design, appropriate for long-lived monitoring rather than a prerequisite for a reusable helper.

## Review findings and verification plan

Three independent reviews covered Restate correctness, Rust interface design, and behavioral contracts/testing. Their findings are incorporated above:

- Simplify scheduling to a fixed duration. Following the addition of attempt limits, group limit outcomes under `Failed { reason, last }`.
- Make the sampled, cooperative deadline explicit, including stale replayed eligibility and sleep decisions.
- Return the last pending observation directly, since expiry cannot precede the initial observation.
- Specify zero-budget behavior, deadline equality, error precedence, backward clock movement, and input validation.
- The initial free function was subsequently replaced with `Poller::poll` and a validated builder, after agreeing on optional timeout and reusable configuration. The callback remains an ordinary function or closure.

Implementation acceptance checks:

- Compile examples with borrowed callbacks and a `Send` future across all five SDK contexts, without serialization/clone/static bounds on retained observations.
- Exercise the production loop through a private clock/sleep seam with scripted times; keep the public interface tied to real SDK contexts. Cover immediate finish, zero budget, invalid delay/precision, overflow, latest pending retention, exact-deadline equality, clipped sleep, late finish/pending/error, fixed-delay spacing, backward clocks, and error propagation.
- Follow [`tests/clock.rs`](../../crates/restate-ext/tests/clock.rs) for real-server evidence of journal replay and external-observation counts. Exercise retry and downtime during durable sleep, plus cancellation propagation. Use controlled synchronization rather than tight timing assertions; Tokio paused time does not control Restate timers.
- Verify the documented stale-sample behavior using a controlled failure between a recorded boundary check and the next operation.
- Verify build-time validation, no clock reads without a timeout, cancellation of indefinite polling, and fresh timeout/attempt state when reusing the same poller.
- Verify exact attempt counts and sleeps, retained final pending values, success/error precedence on the last attempt, both limits together, and replay of an exhausted attempt limit without repeating completed external reads.
- Run repository formatting, Clippy, unit/doc tests, and documentation checks. Run the relevant ignored real-server tests explicitly, and report any unavailable infrastructure.

The harness's own HTTP polling uses Tokio timeouts for a different purpose; do not inherit its preemptive timeout or half-remaining-sleep behavior.

## Primary sources

- **R0:** [Official Rust SDK changelog](https://docs.restate.dev/changelog/rust-sdk).
- **R1:** [SDK 0.12.1 context traits and implementations](https://github.com/restatedev/sdk-rust/blob/v0.12.1/src/context/mod.rs); [0.12.0 counterpart](https://github.com/restatedev/sdk-rust/blob/v0.12.0/src/context/mod.rs).
- **R2:** [`ContextSideEffects` Rustdoc](https://docs.rs/restate-sdk/0.12.1/restate_sdk/context/trait.ContextSideEffects.html); [run traits and retry policy source](https://github.com/restatedev/sdk-rust/blob/v0.12.1/src/context/run.rs).
- **R3:** [Select macro source](https://github.com/restatedev/sdk-rust/blob/v0.12.1/src/context/select.rs).
- **R4:** [Context internals: sleep, call, run, cancellation](https://github.com/restatedev/sdk-rust/blob/v0.12.1/src/endpoint/context.rs).
- **R5:** [Request, `CallFuture`, and `InvocationHandle` source](https://github.com/restatedev/sdk-rust/blob/v0.12.1/src/context/request.rs).
- **R6:** [`ContextTimers` Rustdoc](https://docs.rs/restate-sdk/0.12.1/restate_sdk/context/trait.ContextTimers.html).
- **R7:** [Shared core 7.0.3 sleep/replay tests](https://github.com/restatedev/sdk-shared-core/blob/v7.0.3/src/tests/sleep.rs).
- **R8:** [Official invocation management: cancellation and replay compatibility](https://docs.restate.dev/services/invocation/managing-invocations).
- **R10:** [Select polling implementation](https://github.com/restatedev/sdk-rust/blob/v0.12.1/src/endpoint/futures/select_poll.rs).
- **R11:** [Durable future implementation](https://github.com/restatedev/sdk-rust/blob/v0.12.1/src/endpoint/futures/durable_future_impl.rs).
