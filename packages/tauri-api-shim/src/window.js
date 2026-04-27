// `@tauri-apps/api/window` and `webviewWindow` shim. The shim has one
// logical window per connected client; methods that don't apply (set_title,
// show, hide …) are no-ops that resolve.

import { emit, emitTo, listen } from './event.js';
import { windowLabel } from './runtime.js';

class WindowProxy {
  /** @param {string} label */
  constructor(label) {
    this.label = label;
  }

  // Emit / listen route through the same singleton SSE/HTTP pipes.
  async listen(event, handler) {
    return listen(event, handler);
  }
  async once(event, handler) {
    const unlisten = await listen(event, (ev) => {
      unlisten().catch(() => {});
      handler(ev);
    });
    return unlisten;
  }
  async emit(event, payload) {
    return emit(event, payload);
  }
  async emitTo(target, event, payload) {
    return emitTo(target, event, payload);
  }

  // Window-side methods that don't apply to a debug HTTP server. Resolve
  // with sensible defaults so user code keeps moving.
  async setTitle(_title) {}
  async show() {}
  async hide() {}
  async close() {}
  async isFocused() { return true; }
  async isVisible() { return true; }
  async isMinimized() { return false; }
  async isMaximized() { return false; }
}

export function getCurrentWindow() {
  return new WindowProxy(windowLabel());
}

export function getCurrentWebviewWindow() {
  return new WindowProxy(windowLabel());
}

export const Window = WindowProxy;
export const WebviewWindow = WindowProxy;
