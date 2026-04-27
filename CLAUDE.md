# CLAUDE.md

Notes for future Claude sessions working on this repo.

## What this is

A drop-in replacement for the `tauri` 2.x crate (and `@tauri-apps/api` JS
package) that turns a Tauri app into a plain HTTP server. The user is a Rust
developer who wants real Chrome DevTools instead of the system webview.

`docs/tauri-debug-shim-plan.md` is the build spec and is the source of truth
for scope decisions. Read it before making API surface changes.

## Layout

- `crates/tauri/` — package name `tauri`, version `2.99.0` (mimics upstream
  2.x so `[patch.crates-io]` satisfies user `tauri = "2"` requirements).
- `crates/tauri-macros/` — `#[command]`, `generate_handler!`,
  `generate_context!` proc-macros.
- `crates/tauri-build/` — no-op stub crate, version `2.99.0`. Only exists so
  user `build.rs` calling `tauri_build::build()` compiles.
- `packages/tauri-api-shim/` — JS half. Aliased to `@tauri-apps/api` via the
  consumer's Vite config. ESM only.
- `examples/basic/` — smallest demo binary.

## How commands work

The `#[command]` macro emits a wrapper `__tauri_cmd_<name>(req)` next to the
user's fn. `generate_handler![a, b, c]` builds a dispatch closure that
matches `req.name()` against a string table and calls the right wrapper.
This is the explicit-list approach, matching upstream — not `inventory`.

Argument extraction is **trait-dispatched** via `ipc::CommandArg`. The
macro emits `<T as CommandArg>::from_command(&req, key)` for every
parameter. There are specific impls for `State<'_, T>`, `AppHandle<Wry>`,
`Window<Wry>` plus a blanket impl for `DeserializeOwned`. Coherence works
because the special types deliberately don't implement `Deserialize`.

Crucially this lets type aliases like
`type ManagedState<'a> = State<'a, T>` work — Rust resolves the alias
before trait selection. **Do not regress this back to syntactic detection;
real user code uses aliases.**

## Events

`tokio::sync::broadcast::Sender<EventEnvelope>` lives on `AppInner`. SSE
clients subscribe and filter at the receiver. Backend `Listener::listen`
inserts into `Mutex<HashMap<EventId, ListenerEntry>>` and a single dispatch
task spawned in `Builder::bind` walks the registry per envelope.

The routing rule is `event.rs::event_matches_listener`. **Preserve the
upstream tauri#11561 quirk** — a plain `Any` listener does NOT match
`AnyLabel` events. User code that worked against real Tauri must keep
working here.

## Window<R> and WebviewWindow<R>

A type alias (`pub type WebviewWindow<R> = Window<R>`) per the user's
decision. One logical window per connected client; `webview_windows()`
enumerates whatever labels showed up on `X-Tauri-Window` headers. Methods
like `set_title` are no-ops returning `Ok(())`.

## Tests

`cargo test -p tauri --tests` runs the full grid:

- `m1_command.rs` — basic command round-trip
- `m2_result.rs` — `Result<T, E>` → 200/422
- `m3_state.rs` — managed state
- `m4_special_params.rs` — `AppHandle`/`Window` injection
- `m5_emit.rs` — backend emit + SSE
- `m5_js_shim.rs` — drives Node against the JS shim (skips if no `node`)
- `m6_listen.rs` — backend listeners + dispatch task
- `m6_js_shim.rs` — Node `event.emit` round-trip
- `m_mayday_fit.rs` — `run_on_main_thread`, camelCase params, struct-payload
  decode

Tests bind to `127.0.0.1:0` and use a `oneshot` for graceful shutdown. SSE
tests use a second `oneshot` to signal "GET headers received" before
emitting, so subscribe-vs-emit isn't a race. **Don't replace those signals
with `tokio::time::sleep` — it's flaky.**

JS-side tests must call `__resetForTests()` from `event.js` in a `finally`
to close the EventSource connection. Otherwise Node's event loop never
exits and the test hangs.

## Conventions

- Edition 2024, resolver 3, latest stable crate versions.
- Don't add backwards-compat shims when patching; this is a debug tool, the
  whole audience is rebuilds-from-scratch.
- Stub upstream APIs liberally — many Tauri methods are no-ops here
  (windowing, exit hooks). Document each stub at the call site so future
  readers know it's not a TODO.
- Plugin support is explicitly out. If a user needs a plugin they're
  expected to feature-gate it in their own code or fork the shim.

## Common gotchas

- `[patch.crates-io]` with a path-only entry only applies if the path
  crate's `version` field satisfies the consumer's requirement. We use
  `2.99.0` for `tauri` and `tauri-build` so any `^2` works.
- If the consumer's `Cargo.lock` was already populated against upstream
  Tauri, `cargo update -p tauri --precise 2.99.0` is needed to flip the
  lockfile over. Cargo otherwise warns "patch was not used in the crate
  graph".
- `State::inner()` returns `&'r T` (the State's lifetime parameter), not
  `&T` borrowed from `&self`. User code threads borrows past temporary
  States, e.g. `state.inner().lock()` returning `MutexGuard<'a, T>`.
- The dispatch task in `Builder::bind` runs as a tokio task, not on the
  serving runtime's worker. Backend listeners run there; if a handler
  panics it's caught by `catch_unwind` and logged.
- Node `EventSource` is gated behind `--experimental-eventsource` on some
  Node 22 patch versions; the integration tests pass that flag.

## Sample-fit work

`m_mayday_fit.rs` exists to capture concrete gaps surfaced by surveying
real user projects. When testing against a new sample app, prefer adding
focused tests to that file (or a sibling) rather than expanding existing
milestone tests.

The realistic path for "make this user project work":

1. Survey the app's `tauri::*` surface and its frontend `@tauri-apps/api/*`
   imports (use the Explore agent).
2. Add stubs for any new methods or types that show up.
3. Patch the consumer to drop unused plugin deps and wire the
   `[patch.crates-io]` table + Vite alias on a **new branch** in their
   repo, never on `main`.
4. `cargo build` the consumer, fix what surfaces, iterate.
5. Smoke-test the binary with `curl` against a known command before
   declaring done.

## Things deferred

`docs/tauri-debug-shim-plan.md` §9 lists M7-M10 (npm-publishable JS package,
`ipc::Request`/raw bytes/`Channel<T>`, `cargo tauri-debug` CLI, polish).
None of these are required for the current sample-fit work. Don't pull them
in unless asked.
