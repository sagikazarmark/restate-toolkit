# Naming an async readiness poller

Researched 2026-09-24. Scope: four primary documentation sources/API families,
prioritizing Rust; application-level readiness checks with interval/timeout controls.

## Recommendation

**`Poller` is a defensible, recognizable name for this abstraction.** It has direct
application-level precedent in Azure's Rust and Go SDKs, beyond OS I/O polling.
However, this small sample supports “established in LRO APIs,” not “the standard
or dominant Rust name for a generic reusable readiness loop.” `Waiter` is an
established alternative for waiting until a resource reaches a desired state. [R1–R4]

## Verified public APIs

| Source | Exact public names | Behavior and relevance |
| --- | --- | --- |
| Azure Rust, `azure_core` 1.1.0 | `azure_core::http::poller::Poller<M, F = JsonFormat>`; `PollerOptions<'a>` in the same module | `Poller::new` repeatedly invokes an async callback to monitor an LRO. Implements `IntoFuture` for awaiting the final result and `Stream` for intermediate status responses. `PollerOptions::frequency` configures the fallback interval without `retry-after` (default 30 s, minimum 1 s). Options expose `context` and `frequency`, **no explicit overall timeout field**. Strong async application-level precedent, but LRO-specific rather than an arbitrary readiness predicate. [R1] |
| Rust `polling` 3.11.0 | `polling::Poller`; `Poller::add`, `Poller::wait`, `Poller::wait_deadline` | Registers file descriptors/sockets and waits for I/O events; `wait` accepts an optional timeout. This is OS event readiness, **not evidence for an interval-based application readiness loop**. [R2] |
| Azure Go, `azcore` v1.23.1 | `runtime.Poller[T]`; `(*Poller[T]).PollUntilDone`; `runtime.PollUntilDoneOptions.Frequency` | Package `github.com/Azure/azure-sdk-for-go/sdk/azcore/runtime`. Encapsulates an LRO; `PollUntilDone` loops with sleeps until terminal state, error, or context expiry. Frequency controls the fallback interval without `Retry-After`; the supplied context bounds the wait. Close behavioral precedent, though the Go method blocks its calling goroutine. [R3] |
| AWS SDK for Rust | `aws_sdk_s3::client::Waiters`; `wait_until_bucket_exists()`; waiter builder `.wait(Duration).await` | AWS calls the abstraction **waiters**: client-side polling until a desired resource state is reached or deemed unreachable. Service clients expose `wait_until_<condition>` methods. Strong alternative terminology for application readiness, with service-specific conditions rather than a generic callback interface. [R4] |

## Naming judgment

For a reusable async check → sleep → check helper, **`Poller` is reasonable and
consistent with existing SDK terminology**. Document it as “polls an async
readiness check at a configured interval until ready or the waiting budget
expires.” The name alone does not specify timeout semantics, durability, or
whether an instance represents one operation versus reusable configuration.
The cited Azure types primarily represent individual LROs; their precedent does
not establish that reusable configuration is conventionally named `Poller`.

## Primary sources

- **R1:** Azure Rust [`Poller`](https://docs.rs/azure_core/1.1.0/azure_core/http/poller/struct.Poller.html) and companion [`PollerOptions`](https://docs.rs/azure_core/1.1.0/azure_core/http/poller/struct.PollerOptions.html).
- **R2:** Rust polling [`polling::Poller`](https://docs.rs/polling/3.11.0/polling/struct.Poller.html).
- **R3:** Azure Go [`runtime.Poller`](https://pkg.go.dev/github.com/Azure/azure-sdk-for-go/sdk/azcore@v1.23.1/runtime#Poller), [`PollUntilDone`](https://pkg.go.dev/github.com/Azure/azure-sdk-for-go/sdk/azcore@v1.23.1/runtime#Poller.PollUntilDone), and [`PollUntilDoneOptions`](https://pkg.go.dev/github.com/Azure/azure-sdk-for-go/sdk/azcore@v1.23.1/runtime#PollUntilDoneOptions).
- **R4:** AWS [Using waiters in the AWS SDK for Rust](https://docs.aws.amazon.com/sdk-for-rust/latest/dg/waiters.html).
