# Greeter Example

A small but complete [Restate](https://restate.dev/) worker, written to be copied from.

It shows every piece a real service needs:

- services and handlers
- endpoint
- configuration
- logging
- graceful shutdown
- and an end-to-end test against a real Restate server.

This README walks through those pieces in the order they build on each other. Read it next to the code:
[`src/lib.rs`](src/lib.rs) holds the service, [`src/main.rs`](src/main.rs) configures it and turns it into a
running process, and [`tests/e2e.rs`](tests/e2e.rs) tests it against a real server.

## The big picture

You don't call your code directly when you use Restate. Clients call the **Restate server**, which records
each call and each step your code takes in a **journal**, then forwards the call to your code over HTTP. If
your process crashes or a step fails, Restate calls it again and replays the journal, so steps that already
finished return their recorded results instead of running twice.

```text
client ──HTTP──▶ Restate server ──HTTP/2──▶ your worker (this example)
         :8080   (ingress, journal,  :9080    └─ endpoint
                  retries)                       └─ Greeter service
                                                    └─ greet
```

Your side of that arrow is made of three layers, from the inside out.

### 1. Services and handlers

A **service** is a named group of **handlers**, and a handler is an async function Restate can invoke. Here
that is `Greeter`, with one handler, `greet` ([`src/lib.rs`](src/lib.rs)):

```rust
#[restate_sdk::service(name = "Greeter")]
impl Greeter {
    #[handler]
    async fn greet(&self, ctx: Context<'_>, name: String) -> HandlerResult<String> { ... }
}
```

Clients address a handler by service and handler name, for example `POST /Greeter/greet`. Besides plain
services, the SDK also has virtual objects (keyed, with state) and workflows. They plug into the rest of
this example the same way.

Handlers are ordinary Rust with one rule: anything with a side effect or a result that can change between
runs, like network calls, randomness or the clock, goes through the context (`ctx`) so it's journaled:

- `ctx.run(...)` runs a closure as a named step and journals its result. After a retry, the step isn't
  run again; its recorded result is returned.
- `ctx.now()` comes from [`restate-ext`](../../crates/restate-ext). It reads the wall clock as a journaled
  step. `greet` uses it instead of `SystemTime::now()`, so a retried invocation greets with the time its
  first attempt saw.

Keep the business logic outside the handler where you can. Picking the greeting for a given time is the
plain function `greeting`, unit tested without Restate.

### 2. The endpoint

An **endpoint** is what the Restate server talks to: every service your process serves, bound together
behind one HTTP handler. When you register the worker with Restate, the server runs **discovery** against
the endpoint. It asks which services and handlers exist and what options they have, and after that it
routes invocations to them.

`endpoint` in [`src/main.rs`](src/main.rs) builds it:

```rust
fn endpoint(config: Config<ServicesConfig>) -> Result<Endpoint, ConfigError> {
    Ok(Endpoint::builder()
        .configure(&config.endpoint)?
        .bind(config.services.greeter.apply(Greeter)?)
        .build())
}
```

`configure` (from `restate-config`'s `ConfigureEndpointExt`) applies the endpoint-wide settings, and each
service is bound with its policy applied (`apply(Greeter)`). To add a service, add another `.bind(...)`.

### 3. Configuration

[`restate-config`](../../crates/restate-config) provides the configuration types. A `Config` has two halves:

- **`endpoint`** (`EndpointConfig`) configures the process: the address to listen on (default
  `0.0.0.0:9080`), and the `identity_keys` the endpoint uses to check that requests really come from your
  Restate server. Set the keys in production.
- **`services`** is a type you define with one `ServiceOptionsConfig` per service. Here that's
  `ServicesConfig { greeter }`. A service's options are the policies Restate enforces for it: retries,
  timeouts, journal retention, metadata, and per-handler overrides.

The example's configuration (`config` in [`src/main.rs`](src/main.rs)) sets these for `Greeter`:

| Setting | Value | Effect |
| ------- | ----- | ------ |
| `metadata` | `team = greetings` | Labels the service in Restate's discovery and UI. |
| `retry_policy_*` | 500ms, 5 attempts, then pause | Retries a failing invocation, then pauses it for an operator instead of failing it. |

Policies you leave unset keep the SDK's and the server's defaults.

To keep the example self-contained, the configuration is built in code. A real service would load it with
a configuration library such as [Figment](https://docs.rs/figment), for example a TOML file with environment
overrides. The types are plain Serde models, so the file has the same shape as the code:

```toml
[endpoint]
listener = "0.0.0.0:9080"
identity_keys = ["publickeyv1_..."]

[services.greeter]
metadata = { team = "greetings" }
retry_policy_initial_interval = "500ms"
retry_policy_max_attempts = 5
retry_policy_on_max_attempts = "pause"
```

## Putting it together: the worker

[`src/main.rs`](src/main.rs) turns those layers into a process you can deploy. Each step is commented in the
source. In order, it:

1. **Sets up logging.** It installs a `tracing` subscriber that reads `RUST_LOG` (default `info`) and uses
   the SDK's `ReplayAwareFilter`. Restate re-runs handler code when it replays a journal, and without the
   filter every log line from before the replay would be printed again.
2. **Loads configuration** as described above.
3. **Builds the endpoint** with `endpoint`. Invalid configuration, such as an override for a
   handler that doesn't exist, stops the process here, before it serves anything.
4. **Binds the listener** at the configured address.
5. **Serves until asked to stop.** It uses `HttpServer::serve_with_cancel` with `restate-ext`'s
   `shutdown_signal`, which resolves on Ctrl-C (SIGINT) and also on SIGTERM, the signal `docker stop` and
   Kubernetes send. On either one, the server stops accepting connections and gives open invocations up to
   ten seconds to finish. Restate retries whatever is left on the next worker.

Doing this by hand instead of calling `listen_and_serve` matters: the SDK's default only stops on SIGINT,
so without it a container stop kills the worker in the middle of a step.

## Writing your own service

Starting from this example:

1. Define your service in the library, apart from how it is served: a struct plus a `#[restate_sdk::service]` (or `object` /
   `workflow`) impl. Put side effects in `ctx.run` and keep the logic in plain, unit-tested functions.
2. In `main.rs`, add a field for it to `ServicesConfig`, and a
   `.bind(config.services.<name>.apply(YourService)?)` to `endpoint`.
3. Load `Config<ServicesConfig>` from your configuration source in `main`. Keep the logging and shutdown
   code as they are.
4. Write an end-to-end test like [`tests/e2e.rs`](tests/e2e.rs) that binds your service with a policy,
   deploys it to a real server, and checks both the behavior and the policies the server reports.

## Running the worker

```shell
cargo run -p greeter
```

The worker listens on `0.0.0.0:9080`. Register it with a running Restate server and call it through the
ingress:

```shell
restate deployments register http://localhost:9080
curl localhost:8080/Greeter/greet --json '"Ada"'
```

Use `RUST_LOG` to change what's logged, for example `RUST_LOG=greeter=debug,info`. Stop the worker with
Ctrl-C or SIGTERM.

## Testing

`cargo test -p greeter` runs the unit tests for the greeting logic.

The end-to-end test in [`tests/e2e.rs`](tests/e2e.rs) uses
[`restate-e2e-harness`](../../crates/restate-e2e-harness). It starts its own `restate-server` on free
loopback ports and deploys the endpoint to it. Then it calls `greet` through the ingress and checks the
greeting against the time around the call. It reads the retained journal to see the journaled clock and
greeting steps, and checks that the configured policies show up in the server's discovery
(`GET /services/Greeter`). The test binds `Greeter` itself, with the policy it checks.

The test is ignored by default because it needs a `restate-server` binary (see
[Getting a `restate-server`](../../crates/restate-e2e-harness/README.md#getting-a-restate-server)).
Point `RESTATE_SERVER_BIN` at it and include ignored tests:

```shell
RESTATE_SERVER_BIN="$PWD/restate-server" cargo test -p greeter -- --ignored
```

Verified with Restate server **1.7.8**. Unix only.

## Dagger

This directory is a Dagger workspace of its own ([`dagger.toml`](dagger.toml)), with its own CI workflow
([`dagger-examples-greeter.yaml`](../../.github/workflows/dagger-examples-greeter.yaml)).
Run its checks from here:

```shell
dagger check
```

The `rust` module runs fmt, clippy, the tests and docs for this crate, on the container of the repository's
[`rust-setup`](../../.dagger/modules/rust-setup/main.dang) module: the Rust image with `restate-server`
copied from the Restate image and selected with `RESTATE_SERVER_BIN`. The crate's manifest configures the
module's `test` to pass `--include-ignored` to the test binaries (`hack.test = ["--", "--include-ignored"]`
in `[[package.metadata.dagger.modules]]`), so `rust:test` runs the end-to-end test too.
