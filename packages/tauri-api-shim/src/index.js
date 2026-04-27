// Top-level barrel matching `@tauri-apps/api`'s default export shape.

import { Channel, invoke } from './core.js';
import {
  TauriEvent,
  emit,
  emitTo,
  listen,
  once,
} from './event.js';
import {
  Window,
  WebviewWindow,
  getCurrentWebviewWindow,
  getCurrentWindow,
} from './window.js';

// Mirror the upstream `__TAURI_INTERNALS__` global so probes by user code or
// third-party libraries find something.
if (typeof globalThis !== 'undefined' && !globalThis.__TAURI_INTERNALS__) {
  globalThis.__TAURI_INTERNALS__ = {
    invoke,
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    transformCallback: (_cb, _once) => 0,
  };
}

export const core = { invoke, Channel };
export const event = { listen, once, emit, emitTo, TauriEvent };
export const window = {
  Window,
  WebviewWindow,
  getCurrentWindow,
  getCurrentWebviewWindow,
};
export {
  Channel,
  TauriEvent,
  WebviewWindow,
  Window,
  emit,
  emitTo,
  getCurrentWebviewWindow,
  getCurrentWindow,
  invoke,
  listen,
  once,
};
