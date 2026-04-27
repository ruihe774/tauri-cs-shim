# tauri-cs-shim

A debug shim for [Tauri 2.x](https://tauri.app) that turns a Tauri app into an
ordinary HTTP server, so you can drive the Rust backend from a real browser
with full Chrome / Firefox DevTools — instead of through the system webview.

The user's application source code is unmodified. The shim swaps in for the
`tauri` crate via `[patch.crates-io]` and for the `@tauri-apps/api` and
`@tauri-apps/cli` npm packages via `link:` deps. Backend commands round-trip
through `POST /__tauri/invoke/{name}` and events flow through Server-Sent
Events on `/__tauri/events`.

## What works

| Surface | Status |
| --- | --- |
| `#[tauri::command]` (sync, async, `Result<T, E>`) | yes |
| Parameter injection: `State<'_, T>` (incl. type aliases), `AppHandle`, `Window`, `WebviewWindow` | yes |
| `Builder::default/new/manage/setup/invoke_handler/run/bind` | yes |
| `Manager` trait (state, windows, config stub) | yes |
| `Emitter` trait (`emit`, `emit_to`, `emit_filter`) | yes |
| `Listener` trait (`listen`, `listen_any`, `once`, `unlisten`) with backend dispatch task | yes |
| `EventTarget` routing including the `tauri#11561` `AnyLabel` quirk | yes |
| `async_runtime::{spawn, spawn_blocking, block_on, Mutex, RwLock, JoinHandle}` | yes |
| `AppHandle::run_on_main_thread`, `AppHandle::exit` | yes |
| `tauri_build::build()` and friends | no-op stub |
| `generate_handler!`, `generate_context!`, `Window`/stub `set_title`/`show`/etc. | yes |
| JS shim: `core.invoke`, `event.{listen, once, emit, emitTo, TauriEvent}`, `window.getCurrentWindow` | yes |
| `tauri` CLI: `tauri dev` (vite + cargo run + browser open + Ctrl-C teardown) | yes |
| `tauri build` | rejected (no webview to bundle) |

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
│   ├── tauri-api-shim/    # JS half — package name @tauri-apps/api
│   └── tauri-cs-cli/      # CLI launcher — package name @tauri-apps/cli
├── examples/
│   └── basic/             # smallest end-to-end demo
└── docs/
    └── tauri-debug-shim-plan.md
```

## Wiring it into a Tauri project

The shim is designed to live alongside your project on disk. Clone it next to
your project and patch its packages in.

### Rust side — workspace `Cargo.toml`

```toml
[patch.crates-io]
tauri = { path = "../tauri-cs-shim/crates/tauri" }
tauri-build = { path = "../tauri-cs-shim/crates/tauri-build" }
```

If `Cargo.lock` already pinned the upstream versions, run

```sh
cargo update -p tauri --precise 2.99.0
cargo update -p tauri-build --precise 2.99.0
```

once to switch the lockfile over. (The shim crates use a `2.99.0` version so
they satisfy any `tauri = "2"` requirement.)

If your project's `src-tauri/Cargo.toml` declares `tauri-plugin-*`
build-dependencies or runtime dependencies, drop those — the shim doesn't
implement plugins and the plugin crates will fail to compile. Likewise drop
any `Builder::plugin(...)` chains in `main.rs`.

### Frontend — `package.json`

```json
{
  "dependencies": {
    "@tauri-apps/api": "link:../tauri-cs-shim/packages/tauri-api-shim"
  },
  "devDependencies": {
    "@tauri-apps/cli": "link:../tauri-cs-shim/packages/tauri-cs-cli"
  }
}
```

**Use `link:`, not `file:`.** pnpm copies `file:` deps into `node_modules` and
edits to the shim won't be picked up; `link:` creates a symlink.

After editing `package.json`, run `pnpm install --no-frozen-lockfile` to
register the new symlinks. No Vite alias is needed — Vite resolves the
subpath imports (`@tauri-apps/api/core`, `/event`, `/window`,
`/webviewWindow`) through the shim's `package.json` `exports` map.

### Run

```sh
pnpm tauri dev
```

The shim's CLI launcher starts the Rust binary (`cargo run --manifest-path
src-tauri/Cargo.toml`) on `127.0.0.1:1421` and the dev server on whatever
`build.devUrl` says (typically `http://localhost:1420`), polls both ports,
and opens the system browser. `Ctrl-C` tears the whole tree down. Override
the Rust port with `TAURI_DEBUG_PORT` / `TAURI_DEBUG_HOST` if 1421 is busy.

`pnpm tauri build` is rejected with a clear error — the shim has no webview
to bundle. Use the upstream Tauri CLI for production builds (i.e., off this
branch).

## Running the bundled example

`examples/basic/` is a full end-to-end demo: a Vite frontend in
`examples/basic/{index.html,src/}` driving a Rust backend in
`examples/basic/src-tauri/`. It exercises sync commands, async commands
returning `Result<T, E>`, managed `State`, and a backend timer that emits
`tick` events through the SSE bus.

```sh
cd examples/basic
pnpm install --no-frozen-lockfile     # wires the link: deps once
pnpm tauri dev                         # starts vite + cargo + opens the browser
```

You can also poke just the Rust side without a frontend:

```sh
cargo run -p basic-example &
curl -X POST -H 'content-type: application/json' \
     -d '{"name":"world"}' \
     http://127.0.0.1:1421/__tauri/invoke/greet
# → "Hello, world! (from the Rust shim)"
```

## Debugging a shimmed app from an LLM

Once your app is running under the shim it is just a website plus an HTTP
server, so any browser-automation tool an LLM can drive will work — no
"Tauri DevTools protocol" needed. Two reliable options:

### Playwright MCP

Run the shim, then ask Claude to use the
[`@playwright/mcp`](https://github.com/microsoft/playwright-mcp) server to
navigate to your dev URL (`http://localhost:1420`). Claude can take
accessibility snapshots, click, fill forms, run page JS via
`browser_evaluate`, and read the console — everything flows through real
HTTP/SSE so any backend issue surfaces with normal stack traces. The
example above was verified end-to-end this way.

A useful pattern for backend round-trips: use `browser_evaluate` to call
`window.__TAURI_INTERNALS__.invoke('your_command', {…})` and inspect the
result without touching the DOM.

### Claude Code for Chrome

The
[Claude for Chrome](https://claude.com/claude-for-chrome) extension
([docs](https://code.claude.com/docs/en/chrome))
lets the agent attach to your already-open Chromium tab. Open
`http://localhost:1420` in Chrome, hand control to the agent, and it can
inspect the live DOM, watch the network panel for `/__tauri/invoke/*`
calls, read SSE frames on `/__tauri/events`, and edit the page. Because
the shim runs the backend as a normal HTTP origin (with permissive CORS),
nothing about the cross-origin handshake is special — DevTools shows
exactly what the agent sees.

Either way, the win over the system webview is the same: real DevTools
network/performance/memory panels, real `console.log`, real source maps,
and a UI surface an LLM can actually drive.

## Tests

```sh
cargo test --workspace
```

Some tests spawn `node --test` against the JS shim
(`crates/tauri/tests/m5_js_shim.rs`, `m6_js_shim.rs`). They skip
automatically if `node` isn't on `PATH`. Node 22+ is required for the
built-in `EventSource` global; the test runner passes
`--experimental-eventsource` for older Node 22 patch versions.

## Status

This is a development/debugging tool — not a production runtime. Plugin code
paths are unimplemented and would need to be worked around in the consuming
app. See `docs/tauri-debug-shim-plan.md` for the full design and scope notes,
and `CLAUDE.md` for notes on the repo's internals aimed at future
contributors.
