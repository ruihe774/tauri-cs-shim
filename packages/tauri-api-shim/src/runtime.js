// Shared runtime helpers: base URL, window label, client id.
//
// In a browser, the page is served by Vite (e.g. http://localhost:1420)
// while the Rust shim listens on a different port (default 1421). Default
// to `http://<hostname>:1421` so /__tauri/* requests reach the shim, not
// Vite. Override by setting `window.__TAURI_DEBUG_BASE__` (e.g. in
// index.html) if you've moved the shim with TAURI_DEBUG_PORT.
//
// Under Node-driven tests the caller sets globalThis.__TAURI_DEBUG_BASE__
// (or TAURI_DEBUG_BASE in the process env) to the absolute URL.

export function baseUrl() {
  if (typeof globalThis.__TAURI_DEBUG_BASE__ === 'string') {
    return globalThis.__TAURI_DEBUG_BASE__;
  }
  if (typeof process !== 'undefined' && process.env && process.env.TAURI_DEBUG_BASE) {
    return process.env.TAURI_DEBUG_BASE;
  }
  if (typeof window !== 'undefined' && window.location) {
    return `${window.location.protocol}//${window.location.hostname}:1421`;
  }
  return '';
}

export function windowLabel() {
  if (typeof globalThis.__TAURI_DEBUG_WINDOW__ === 'string') {
    return globalThis.__TAURI_DEBUG_WINDOW__;
  }
  if (typeof process !== 'undefined' && process.env && process.env.TAURI_DEBUG_WINDOW) {
    return process.env.TAURI_DEBUG_WINDOW;
  }
  return 'main';
}

let _clientId = null;
export function clientId() {
  if (_clientId === null) {
    _clientId = Math.random().toString(36).slice(2);
  }
  return _clientId;
}

// Test helper. Resets the cached client id and forgets the SSE singleton in
// event.js. Not part of the public surface.
export function __resetForTests() {
  _clientId = null;
}
