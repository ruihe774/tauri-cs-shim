// Shared runtime helpers: base URL, window label, client id.
//
// In a real Vite-served frontend the shim talks to /__tauri/* on the same
// origin as the page, so baseUrl() returns "". Under Node-driven tests the
// caller sets globalThis.__TAURI_DEBUG_BASE__ (or TAURI_DEBUG_BASE in the
// process env) to the absolute URL of the shim server.

export function baseUrl() {
  if (typeof globalThis.__TAURI_DEBUG_BASE__ === 'string') {
    return globalThis.__TAURI_DEBUG_BASE__;
  }
  if (typeof process !== 'undefined' && process.env && process.env.TAURI_DEBUG_BASE) {
    return process.env.TAURI_DEBUG_BASE;
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
