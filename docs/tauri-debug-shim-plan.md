# Tauri Debug Shim — Implementation Plan

A debug shim that allows a Tauri 2.x application to be run as an ordinary HTTP server, so the Rust backend can be exercised from a real browser (with full Chrome/Firefox devtools) instead of through the system webview. The user's application source code is unmodified; the shim swaps in for the `tauri` crate and the `@tauri-apps/api` JS package via Cargo patching and a Vite alias.

This document is the build specification. It is intended to be handed to a coding agent.

## 1. Goals and Non-Goals

**In scope.** Replicating the surface of the `tauri` crate that is used by hand-written Rust application code: the `#[command]` macro, the builder, `AppHandle`, `Window`, `WebviewWindow`, `State`, `Manager`, `Emitter`, `Listener`, `async_runtime`, `generate_handler!`, `generate_context!`, and the `ipc::Request` type. On the frontend, replicating the parts of `@tauri-apps/api` that the user's frontend imports: at minimum `core` (`invoke`, `Channel`) and `event` (`listen`, `once`, `emit`, `emitTo`, `TauriEvent`).

**Out of scope.** All Tauri plugins (`tauri-plugin-fs`, `tauri-plugin-dialog`, `tauri-plugin-shell`, `tauri-plugin-store`, `tauri-plugin-window-state`, etc.). The capabilities and permissions system. Window creation/management beyond a single logical window per connected client. Tray icons, menus, global shortcuts, updater, deep links. Mobile targets. The justification: plugins exist primarily so that frontend-only developers can avoid Rust, but our target user is a Rust developer who wants better devtools. Plugin code paths can stay unsupported and the user works around them.

**Success criterion.** A Tauri app whose Rust code uses only commands, state, events, and direct emits/listens compiles and runs under the shim, and its frontend behaves correctly when loaded in Chrome at `http://localhost:PORT`. The user can set breakpoints in Chrome DevTools, see IPC traffic in the Network tab, and profile the frontend with Chrome's profiler.

## 2. Architecture Overview

The shim consists of four artifacts:

A Rust crate named `tauri` (the shim) that exposes the same public API as the real `tauri` crate but routes commands through an embedded `axum` HTTP server instead of a webview IPC bridge. The user activates it through `[patch.crates-io]` in their `Cargo.toml` or workspace.

A companion procedural-macro crate `tauri-macros` (also same-named as the upstream) providing `command`, `generate_handler`, and `generate_context` so that user code compiles unchanged.

An npm package `@tauri-apps/api` (the shim, published under a different name and aliased via Vite/Webpack — see §8) that mirrors the upstream JS API but talks to the HTTP server: `invoke()` becomes a `POST`, `listen()` opens a Server-Sent Events stream, `emit()` becomes a `POST`.

A small CLI runner `cargo-tauri-debug` that orchestrates the build: it ensures the patch is applied, builds the user's Rust code as a binary, copies the JS shim into `node_modules`, starts the dev server with Vite aliasing, and opens the browser. This is convenience tooling; everything works without it if the user wires the pieces manually.

The Rust shim uses `tokio` as its runtime, `axum` for HTTP, `tower-http` for static-file serving and CORS, and `tokio::sync::broadcast` for the event bus. No `wry` or `tao` dependencies — the shim must compile without any GUI toolkit.

## 3. Repository Structure

```
tauri-debug-shim/
├── Cargo.toml              # workspace
├── crates/
│   ├── tauri/              # the shim crate, package name = "tauri"
│   ├── tauri-macros/       # proc-macros, package name = "tauri-macros"
│   └── tauri-debug-cli/    # the cargo subcommand
├── packages/
│   └── tauri-api-shim/     # npm package, aliased to @tauri-apps/api
└── examples/
    └── basic/              # a Tauri app used as the integration test
```

The crate names are deliberately `tauri` and `tauri-macros` so that a `[patch.crates-io]` entry replaces the upstream cleanly. The on-disk directory names differ to avoid confusion.

## 4. The Rust Shim Crate

### 4.1 Module layout

The shim must mirror the upstream module paths that user code imports. At minimum: the crate root re-exports `AppHandle`, `App`, `Builder`, `Window`, `WebviewWindow`, `State`, `Manager`, `Emitter`, `Listener`, `Runtime`, `Wry`, `EventTarget`, `Event`, `EventId`, `async_runtime`, and `ipc`. The `ipc` module exposes `Request`, `Response`, `InvokeError`, `Channel`. The `async_runtime` module re-exports `tokio::spawn`, `tokio::task::block_on` (or wraps `tokio::runtime::Handle::current().block_on`), and `tokio::sync::Mutex`/`RwLock` aliases.

### 4.2 `Runtime` and `Wry`

`Runtime` is a marker trait with no required methods for the shim's purposes. `Wry` is a unit struct implementing `Runtime`. All generic bounds `R: Runtime` carry through; user code that writes `AppHandle<Wry>` continues to compile, and the default-generic pattern (`AppHandle` defaults to `AppHandle<Wry>`) is preserved.

### 4.3 `App`, `AppHandle`, and the inner state

There is one logical `App` per running shim process. Internally, it holds an `Arc<AppInner>` containing: a `StateManager` (a `type_map::concurrent::TypeMap` keyed by `TypeId`), a `tokio::sync::broadcast::Sender<EventEnvelope>` for outbound events, a registry of backend listeners (`HashMap<EventId, ListenerEntry>`), the command router (`HashMap<&'static str, CommandHandler>`), and configuration (port, static-file directory).

`AppHandle` is `pub struct AppHandle<R: Runtime = Wry> { inner: Arc<AppInner>, _r: PhantomData<R> }`. It is `Clone` (cheap, just bumps the `Arc`). All methods that the upstream `AppHandle` provides and that user code might call must be present:

- `clone()`, debug formatting
- `manage<T>(&self, state: T) -> bool` — install state into the type map
- `state<T>(&self) -> State<'_, T>` — fetch via the `Manager` trait
- `try_state<T>(&self) -> Option<State<'_, T>>`
- `get_webview_window(&self, label: &str) -> Option<WebviewWindow>` — see §4.6
- `webview_windows(&self) -> HashMap<String, WebviewWindow>`
- `path() -> PathResolver` — stub returning a struct whose methods (`app_data_dir`, `config_dir`, etc.) call the same `dirs`/`directories` crate routines the real Tauri uses, so plugin-free filesystem code keeps working
- `exit(code: i32)` — calls `std::process::exit`
- `restart()` — best-effort `exec` of `std::env::current_exe()`; document as desktop-only
- The `Emitter` and `Listener` trait methods (see §4.7)

`App` (the type passed to `setup`) wraps `AppHandle` plus a one-shot signal that `run()` is waiting on. `App::handle()` returns a reference to the `AppHandle`. `App` implements all of `AppHandle`'s convenience methods through `Deref` or duplication.

### 4.4 `Builder`

Mirrors `tauri::Builder<R>`. Methods to implement:

- `default() -> Self`
- `manage<T: Send + Sync + 'static>(self, state: T) -> Self`
- `setup<F>(self, f: F) -> Self` where `F: FnOnce(&mut App) -> Result<(), Box<dyn Error>> + Send + 'static`
- `invoke_handler<F>(self, handler: F) -> Self` where `F` is the closure produced by `generate_handler!` (see §4.10). The handler receives a `CommandRequest` and returns a `Pin<Box<dyn Future<Output = Result<JsonValue, InvokeError>> + Send>>`
- `on_window_event`, `on_page_load`, `plugin` — accept and ignore (or warn once at runtime); we explicitly do not support plugins, but accepting calls keeps user code compiling
- `run<C>(self, context: C) -> Result<(), Error>` — finalizes setup, starts the Axum server, blocks until shutdown

`run()` is the entry point. It builds the `Arc<AppInner>`, runs the user's `setup` closure synchronously (Tauri does the same), then `tokio::runtime::Runtime::new()?.block_on(serve(...))` to start Axum on the configured port (default 1421, override via `TAURI_DEBUG_PORT`).

### 4.5 `State<'_, T>` and `Manager`

`State<'r, T>` wraps `&'r T` and implements `Deref<Target = T>`. Construction is internal; users only obtain it from `Manager::state()` or as a command parameter.

`Manager<R>` is a trait with methods `state<T>()`, `try_state<T>()`, `manage<T>()`, `unmanage<T>()`, `get_webview_window`, `webview_windows`, `app_handle`, and `config` (return a stub). Implemented on `App`, `AppHandle`, `Window`, and `WebviewWindow` — all of which hold an `Arc<AppInner>` reference.

The `StateManager` itself is `RwLock<TypeMap>`. Reads must produce `State<'_, T>` whose lifetime is tied to a guard; the simplest implementation is to leak references via `Arc<T>` storage and hand out `State { value: Arc<T> }` with a `Deref` to `T`. This deviates from upstream's exact internal representation but is observationally identical to user code.

### 4.6 `Window` and `WebviewWindow`

In the shim there is one logical "window" per connected SSE client, plus a default `"main"` label. `WebviewWindow` and `Window` both wrap `Arc<AppInner>` plus a label `String`. Methods to implement:

- `label() -> &str`
- `app_handle() -> &AppHandle`
- `emit`, `emit_to`, `emit_filter` (via the `Emitter` trait)
- `listen`, `once`, `unlisten`, `listen_any` (via the `Listener` trait)
- `is_focused`, `is_visible`, `is_minimized`, etc. — stub to return `Ok(true)` or sensible defaults; document as no-ops
- `set_title`, `set_size`, `show`, `hide`, `close` — no-ops that log a debug message

`Window` and `WebviewWindow` are distinct types upstream but for the shim they can be a type alias (`pub type WebviewWindow = Window`) or two thin wrappers around the same inner. Choose whichever produces fewer compile errors against real-world user code; type alias is simpler but breaks if user code does `impl SomeTrait for Window` and separately for `WebviewWindow`. Recommendation: implement both as separate newtypes around the same inner, with a private constructor.

### 4.7 `Emitter` and `Listener` traits

`Emitter<R>` requires `emit`, `emit_to`, `emit_filter` with the upstream signatures. The implementation pushes an `EventEnvelope { name, payload_json, target }` onto the broadcast channel. Filtering happens at the receiver side: SSE clients filter by their window label, backend listeners filter by their registered target.

`Listener<R>` requires `listen`, `once`, `unlisten`, with `listen_any` and `once_any` as provided methods. Backend listeners are stored in a `Mutex<HashMap<EventId, ListenerEntry>>` inside `AppInner`. A background task drains the broadcast channel and dispatches to matching backend listeners. `EventId` is a `u32` allocated atomically.

`EventTarget` is an enum mirroring upstream: `Any`, `AnyLabel { label }`, `App`, `Window { label }`, `Webview { label }`, `WebviewWindow { label }`. The `Into<EventTarget>` impl for `&str` produces `AnyLabel`. Note the upstream quirk documented in tauri#11561 where `AnyLabel` does not match plain `Any` listeners; replicate the upstream behaviour exactly so user code that works against real Tauri also works here.

### 4.8 `ipc::Request` and `ipc::Response`

`Request` exposes `body() -> &Body` (where `Body` is `Json(serde_json::Value)` or `Raw(Vec<u8>)`) and `headers() -> &HeaderMap`. The HTTP server populates these from the incoming POST. `Response` is the symmetric type for raw responses. `InvokeError` wraps an error JSON value; `From<E: Display>` converts user errors.

### 4.9 `async_runtime`

Re-export `tokio::spawn` as `async_runtime::spawn`, `tokio::task::spawn_blocking` as `spawn_blocking`, and provide `block_on<F: Future>(f: F) -> F::Output` that uses `tokio::runtime::Handle::try_current()` and falls back to a fresh runtime. Re-export `tokio::sync::Mutex` and `RwLock` under `async_runtime::Mutex` / `RwLock` for parity. Provide `JoinHandle` as a re-export of `tokio::task::JoinHandle`.

### 4.10 The `#[command]` macro

This is the load-bearing piece. The macro must accept the same syntax as upstream:

```rust
#[tauri::command]
async fn foo(state: State<'_, Db>, app: AppHandle, name: String) -> Result<User, String> { ... }

#[tauri::command(rename_all = "snake_case")]
fn bar(window: Window, payload: MyPayload) { ... }

#[tauri::command(async)]
fn baz() -> i32 { 42 }
```

For each annotated function, the macro generates:

1. The user's function, unchanged.
2. A wrapper function `__cmd__<name>` with signature `fn __cmd__<name>(req: CommandRequest) -> Pin<Box<dyn Future<Output = Result<JsonValue, InvokeError>> + Send>>`. The wrapper inspects each parameter type: if it matches one of the special parameters (`State<'_, T>`, `AppHandle`, `AppHandle<R>`, `Window`, `Window<R>`, `WebviewWindow`, `WebviewWindow<R>`, `ipc::Request`), it extracts that value from the `CommandRequest`. Otherwise, it deserializes the parameter from the request's JSON body, using `rename_all` to map to the JS-side key (default `camelCase`, matching upstream).
3. An `inventory::submit!` registration so `generate_handler!` can collect commands. (Alternative: have `generate_handler!` accept the function names directly and generate a registration array; this is what upstream does. Either works. Use `inventory` only if the macro should also make commands discoverable across crates without listing them; otherwise the explicit list is simpler and matches upstream behaviour. Recommendation: explicit list, to match upstream semantics including the "commands cannot be `pub` in `lib.rs`" gotcha.)

Special-parameter detection works by syntactic matching on the type's leading path segment. This is fragile in principle (a user could `use tauri::State as MyState`), but it is exactly what upstream's macro does, so we match the limitation. Document it.

The wrapper's argument deserialization handles two cases by inspecting `Content-Type` on the incoming HTTP request:

- `application/json` — body is a JSON object whose keys map to non-special parameters by name (post-`rename_all`).
- `application/octet-stream` or any other type — body is treated as raw bytes, available via `ipc::Request::body()` only; non-special non-`Request` parameters are an error.

Result handling: if the user function returns `T: Serialize`, serialize to JSON and return `Ok`. If it returns `Result<T, E: Serialize>`, serialize the variant accordingly; `Err(e)` becomes an `InvokeError` and the HTTP response is status 422 with the error JSON in the body. If it returns `()`, the JSON response is `null`.

Async detection: if the function is `async fn`, the wrapper awaits it directly. If `#[command(async)]` is used on a sync function, the wrapper wraps the call in `async_runtime::spawn_blocking`. Borrowed parameters in async functions inherit the upstream restriction (must return `Result`) — replicate the same compile-time error message where possible, but it is acceptable to defer to the type checker's natural error.

### 4.11 `generate_handler!` and `generate_context!`

`generate_handler!(cmd1, cmd2, ...)` expands to a closure `|req: CommandRequest| -> Pin<Box<dyn Future<...>>>` that matches on `req.name` and dispatches to `__cmd__cmd1`, `__cmd__cmd2`, etc. Returns an opaque type assignable to `Builder::invoke_handler`'s parameter.

`generate_context!()` expands to a unit struct `Context;` since the shim does not need real config. If user code calls `.config()` on it, return a stubbed `Config` with empty defaults. The macro must accept the optional path argument that upstream supports, and ignore it.

## 5. The HTTP Server

The Axum router exposes:

- `POST /__tauri/invoke/:cmd` — body is the JSON arguments object. Headers `X-Tauri-Window` and `X-Tauri-Client-Id` (set by the JS shim) identify the calling window. Response is the serialized return value as JSON, with status 200, 422 (user error), or 500 (handler panic / serialization failure).
- `GET /__tauri/events?clientId=...&window=...` — Server-Sent Events stream. The server subscribes to the broadcast channel, filters envelopes against the client's window label and any per-listener target, and writes `event: <name>\ndata: <json>\n\n`. Heartbeat comments every 15s to keep the connection alive through proxies.
- `POST /__tauri/emit` — frontend-originated emit. Body: `{ event, payload, target }`. Pushed onto the same broadcast channel.
- `POST /__tauri/listen` and `POST /__tauri/unlisten` — used by the JS shim for accounting, mainly so the Rust side knows which events have any frontend listener (parity with upstream's `__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener`). For the MVP these can be no-ops; the SSE stream already carries everything.
- `GET /*` — static file fallback, serving the user's frontend dist directory (configured via `TAURI_DEBUG_FRONTEND_DIR` or a `Builder::frontend_dir` extension).

CORS is permissive when `TAURI_DEBUG_PORT` differs from the frontend dev-server port, since the user typically runs Vite on a separate port and points it at the shim. In production usage they can serve the built frontend through the shim itself.

## 6. The JS Frontend Shim Package

Published as `@tauri-debug-shim/api` (or similar) and aliased to `@tauri-apps/api` via the user's bundler:

```js
// vite.config.ts
resolve: { alias: { '@tauri-apps/api': '@tauri-debug-shim/api' } }
```

The package mirrors the upstream module structure: subpath imports `@tauri-apps/api/core`, `@tauri-apps/api/event`, `@tauri-apps/api/window`, `@tauri-apps/api/webviewWindow`. Each subpath is a separate entry point so the alias picks them up.

`core.invoke(cmd, args, options)` issues `fetch('/__tauri/invoke/' + cmd, { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-Tauri-Window': currentWindowLabel(), 'X-Tauri-Client-Id': clientId() }, body: JSON.stringify(args ?? {}) })`. On HTTP 200, returns the parsed JSON. On 422, parses the error and `throw`s it (matching upstream's promise-rejection behaviour). The third argument supports `headers` (forwarded) and raw `ArrayBuffer`/`Uint8Array` payloads (sent as `application/octet-stream`).

`core.Channel` — Tauri's `Channel<T>` is a one-shot ID that the backend uses to push streamed messages to a specific frontend callback. Implement as a wrapper that registers a callback in a `Map<channelId, callback>` and listens to a dedicated SSE event name `__channel:<id>`. The backend `Channel<T>` shim emits to that name.

`event.listen(name, handler, options)` opens (or reuses) a singleton SSE connection and registers `(name, target, handler)` in a local map. Returns an async unlisten function. `event.once` is `listen` plus auto-removal. `event.emit(name, payload)` posts to `/__tauri/emit`. `event.emitTo(target, name, payload)` includes the target. The `TauriEvent` enum (with values like `DRAG_DROP`, `DRAG_ENTER`) is re-exported as constants; we never fire these from the shim, but user code that imports the names must compile.

`window.getCurrentWindow()` and `webviewWindow.getCurrentWebviewWindow()` return a stub object whose `label` is `"main"` by default (overridable via a query string), and whose methods either delegate to `event.*` or no-op.

The `__TAURI_INTERNALS__` global is set on `window` to an object with at least `invoke`, `metadata`, and `transformCallback`, so any user code or third-party library that probes for the global keeps working.

## 7. Event Transport

The backend broadcast channel carries `EventEnvelope { id: u32, name: String, payload: serde_json::Value, target: EventTarget, source: EventTarget }`. `source` is set to whatever entity emitted the event (e.g. the calling window's label for a frontend-originated emit, `EventTarget::App` for a backend `app_handle.emit`).

Each SSE connection runs a small filter: it accepts envelopes whose target is `Any`, or matches the connection's window label, or matches a per-listener label that the JS shim communicates via `/__tauri/listen` (optional MVP enhancement). To keep MVP simple, push every envelope to every SSE client and let the JS shim filter by registered listeners. This wastes bandwidth but is simpler and the data volumes in a debug session are negligible.

Backend listeners (registered via `AppHandle::listen`) run in a single dispatch task that subscribes to the broadcast channel and walks the listener registry per envelope. Each listener stores its target filter; matching uses the same logic as upstream (replicate the AnyLabel quirk).

## 8. Integration with User Apps

Two integration paths.

**Manual.** The user adds a Cargo workspace patch:

```toml
[patch.crates-io]
tauri = { path = "path/to/tauri-debug-shim/crates/tauri" }
tauri-macros = { path = "path/to/tauri-debug-shim/crates/tauri-macros" }
```

They build with `cargo build` as usual; the resulting binary is the HTTP server. They add the Vite alias as shown in §6 and run their frontend dev server. They open `http://localhost:5173` (Vite) which talks to `http://localhost:1421` (shim).

**Via the CLI.** `cargo install --path crates/tauri-debug-cli` installs a `cargo tauri-debug` subcommand. Running it from a Tauri project root:

1. Detects the project's `src-tauri` directory.
2. Generates a temporary workspace `Cargo.toml` overlay that injects the patch entries, using a separate `target/` directory so the user's normal `tauri dev` build cache is not invalidated.
3. Builds the Rust binary with the patch active.
4. Starts the user's frontend dev server with the alias env var set, or serves the built frontend through the shim directly.
5. Launches the Rust binary and opens the browser.

The CLI is a quality-of-life layer; the manual path is the source of truth and must work standalone.

## 9. Implementation Milestones

A coding agent should land these in order, each behind its own PR with a working integration test.

**M1 — Hello, command.** A single sync `#[command] fn greet(name: String) -> String` round-trips through `POST /__tauri/invoke/greet` and returns the right JSON. No state, no events, no special params other than the body. The `Builder`, `run()`, and Axum scaffolding land here. The `tauri-macros` crate exists with a minimal `command` macro.

**M2 — Async commands and Result.** Async functions work; `Result<T, E>` maps to 200/422 responses; serialization of complex types (`#[derive(Serialize)]`) works.

**M3 — State.** `Builder::manage`, `State<'_, T>` parameter injection, `Manager::state()` access. The `App`/`AppHandle` types land here in skeleton form.

**M4 — AppHandle and Window injection.** Special-parameter detection in the macro for `AppHandle`, `Window`, `WebviewWindow`. `app_handle.state()` works inside spawned tasks.

**M5 — Events, backend → frontend.** `Emitter::emit` and `emit_to`; SSE endpoint; JS shim's `event.listen`. `EventTarget` enum, `Wry`/`Runtime` plumbing.

**M6 — Events, frontend → backend.** `event.emit` from JS; `Listener::listen` on the Rust side; backend listener dispatch task; `unlisten`.

**M7 — JS shim package.** Package up the JS code as a real npm-publishable artifact with subpath exports; integration tests run a real Vite project against it.

**M8 — `ipc::Request`, raw payloads, `Channel`.** Octet-stream bodies, header passthrough, streaming via `Channel<T>`.

**M9 — CLI runner.** `cargo tauri-debug` command, project detection, automated workspace patching.

**M10 — Polish and gotchas.** `rename_all`, `generate_context!` config stubbing, `path()` resolver, no-op `set_title`/`show`/`hide`, panic-to-500 conversion, structured logging, port configuration, graceful shutdown on SIGINT.

A vertical slice through M1 is the first deliverable and the place to validate the architecture before committing to the rest.

## 10. Testing Strategy

Each milestone ships with two kinds of test.

Unit tests inside the shim crate cover the type map, broadcast filtering, target matching, and macro expansion (using `trybuild` for compile-pass and compile-fail snapshots).

Integration tests in `examples/basic/` consist of a small Tauri app that uses commands, state, and events. The test runner spawns the app under the shim, hits its endpoints with `reqwest`, opens an SSE stream, drives a scripted scenario, and asserts on responses. The same app should also build and run under real Tauri (without the patch) to verify that the shim's API really is API-compatible. CI runs both modes.

A second example `examples/with-frontend/` adds a Vite + React frontend and uses Playwright to drive a real Chromium against it, confirming end-to-end IPC, events, and DevTools behaviour.

## 11. Open Decisions

A few things to nail down during M1 rather than up front.

The exact mechanism for command registration — `inventory` versus explicit `generate_handler!` arrays. Upstream uses the explicit list; matching it avoids surprising users whose code relies on the `pub fn` restriction.

Whether `Window` and `WebviewWindow` are a type alias or separate newtypes. Decide based on what real-world user code needs; type alias is simpler but breaks impls.

How to handle the `tauri-build` crate, which user `build.rs` files invoke. Most likely: ship a stub `tauri-build` crate as part of the workspace, also patched in, that does nothing. Add to the patch instructions.

Whether to pre-build a sample Tauri project's crates against the shim in CI to catch upstream API drift. Recommended yes, against `tauri = "2"` latest.

Configuration surface — environment variables versus a `tauri-debug.toml` versus extension methods on `Builder`. Start with env vars (`TAURI_DEBUG_PORT`, `TAURI_DEBUG_FRONTEND_DIR`, `TAURI_DEBUG_HOST`); add config file later if needed.
