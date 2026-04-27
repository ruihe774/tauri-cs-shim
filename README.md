# tauri-cs-shim

A debug shim for [Tauri 2.x](https://tauri.app) that turns a Tauri app into an
ordinary HTTP server, so you can drive the Rust backend from a real browser
with full Chrome / Firefox DevTools — instead of through the system webview.

The user's application source code is unmodified. The shim swaps in for the
`tauri` crate via `[patch.crates-io]` and for `@tauri-apps/api` via a Vite
alias. Backend commands round-trip through `POST /__tauri/invoke/{name}` and
events flow through Server-Sent Events on `/__tauri/events`.

## What works

| Surface | Status |
| --- | --- |
| `#[tauri::command]` (sync, async, `Result<T, E>`) | yes |
| Parameter injection: `State<'_, T>` (incl. type aliases), `AppHandle`, `Window`, `WebviewWindow` | yes |
| `Builder::default/manage/setup/invoke_handler/run/bind` | yes |
| `Manager` trait (state, windows, config stub) | yes |
| `Emitter` trait (`emit`, `emit_to`, `emit_filter`) | yes |
| `Listener` trait (`listen`, `listen_any`, `once`, `unlisten`) with backend dispatch task | yes |
| `EventTarget` routing including the `tauri#11561` `AnyLabel` quirk | yes |
| `async_runtime::{spawn, spawn_blocking, block_on, Mutex, RwLock, JoinHandle}` | yes |
| `AppHandle::run_on_main_thread`, `AppHandle::exit` | yes |
| `tauri_build::build()` and friends | no-op stub |
| `generate_handler!`, `generate_context!`, `Window`/stub `set_title`/`show`/etc. | yes |
| JS shim: `core.invoke`, `event.{listen, once, emit, emitTo, TauriEvent}`, `window.getCurrentWindow` | yes |

## What's explicitly out of scope

- All `tauri-plugin-*` plugins (fs, dialog, shell, store, window-state, …).
  The shim's user is a Rust developer who wants better DevTools, not a
  frontend-only developer trying to avoid Rust.
- The capabilities / permissions system, tray icons, menus, global shortcuts,
  updater, deep links, mobile.
- Real window creation. The shim has one logical "window" per connected SSE
  client (default label `main`, or whatever the JS shim sends in
  `X-Tauri-Window`). `set_title` / `show` / `hide` / `close` are no-ops.

## Repository layout

```
tauri-cs-shim/
├── crates/
│   ├── tauri/             # the shim (package name = "tauri")
│   ├── tauri-build/       # no-op stub for build.rs
│   └── tauri-macros/      # #[command], generate_handler!, generate_context!
├── packages/
│   └── tauri-api-shim/    # JS half — aliased to @tauri-apps/api
├── examples/
│   └── basic/             # smallest end-to-end demo
└── docs/
    └── tauri-debug-shim-plan.md
```

## Wiring it into a Tauri project

In your project's workspace `Cargo.toml`:

```toml
[patch.crates-io]
tauri = { path = "/path/to/tauri-cs-shim/crates/tauri" }
tauri-build = { path = "/path/to/tauri-cs-shim/crates/tauri-build" }
```

If your `Cargo.lock` already pinned the upstream versions, run
`cargo update -p tauri --precise 2.99.0 && cargo update -p tauri-build --precise 2.99.0`
once to switch the lockfile over. (The shim crates use a `2.99.0` version so
they satisfy any `tauri = "2"` requirement.)

In your `vite.config.ts`:

```ts
resolve: {
  alias: [
    { find: /^@tauri-apps\/api\/core$/,
      replacement: "/path/to/tauri-cs-shim/packages/tauri-api-shim/src/core.js" },
    { find: /^@tauri-apps\/api\/event$/,
      replacement: "/path/to/tauri-cs-shim/packages/tauri-api-shim/src/event.js" },
    { find: /^@tauri-apps\/api\/(window|webviewWindow)$/,
      replacement: "/path/to/tauri-cs-shim/packages/tauri-api-shim/src/window.js" },
    { find: /^@tauri-apps\/api$/,
      replacement: "/path/to/tauri-cs-shim/packages/tauri-api-shim/src/index.js" },
  ],
}
```

Then:

```sh
# Terminal 1 — the Rust shim binary
cargo run

# Terminal 2 — the Vite dev server
pnpm dev    # or npm / yarn

# Open http://localhost:5173 (or whatever Vite reports) in Chrome.
```

The shim listens on `127.0.0.1:1421` by default. Override with
`TAURI_DEBUG_PORT` and `TAURI_DEBUG_HOST`.

## Running the example

```sh
cargo run -p basic-example &
curl -X POST -H 'content-type: application/json' \
     -d '{"name":"world"}' \
     http://127.0.0.1:1421/__tauri/invoke/greet
# → "Hello, world!"
```

## Tests

```sh
cargo test --workspace
```

Some tests spawn `node --test` against the JS shim (`crates/tauri/tests/m5_js_shim.rs`,
`m6_js_shim.rs`). They skip automatically if `node` isn't on `PATH`. Node 22+
is required for the built-in `EventSource` global; the test runner passes
`--experimental-eventsource` for older Node 22 patch versions.

## Status

This is a development/debugging tool — not a production runtime. Plugin code
paths are unimplemented and would need to be worked around in the consuming
app. See `docs/tauri-debug-shim-plan.md` for the full design and scope notes.
