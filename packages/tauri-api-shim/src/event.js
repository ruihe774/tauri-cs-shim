// `@tauri-apps/api/event` shim. M5: listen / once. M6 will fill in emit /
// emitTo against POST /__tauri/emit.

import { baseUrl, clientId, windowLabel } from './runtime.js';

/** @type {EventSource | null} */
let _sse = null;
/** @type {Promise<EventSource> | null} */
let _ssePromise = null;

/** @type {Map<number, { name: string, listener: (e: MessageEvent) => void }>} */
const _listenerEntries = new Map();
let _nextListenerId = 1;

/**
 * Open the SSE connection (or reuse it) and resolve once the server has
 * accepted the connection (the `open` event has fired). Resolving on `open`
 * matters for tests: it guarantees the server's handler has already
 * subscribed to the broadcast bus, so a subsequent backend emit will reach
 * this client.
 *
 * @returns {Promise<EventSource>}
 */
async function ensureSSE() {
  if (_sse !== null) return _sse;
  if (_ssePromise !== null) return _ssePromise;
  if (typeof EventSource === 'undefined') {
    throw new Error(
      '@tauri-apps/api/event (shim) requires global EventSource (Node 22+ or a browser)'
    );
  }
  const url = `${baseUrl()}/__tauri/events?clientId=${encodeURIComponent(
    clientId()
  )}&window=${encodeURIComponent(windowLabel())}`;
  _ssePromise = new Promise((resolve, reject) => {
    const es = new EventSource(url);
    const onOpen = () => {
      es.removeEventListener('open', onOpen);
      es.removeEventListener('error', onError);
      _sse = es;
      resolve(es);
    };
    const onError = (e) => {
      // EventSource auto-reconnects, but until a connection is ever
      // established the open event has not fired. Treat the first error
      // before any open as a fatal connection failure.
      if (_sse === null) {
        es.removeEventListener('open', onOpen);
        es.removeEventListener('error', onError);
        es.close();
        _ssePromise = null;
        reject(new Error('failed to open SSE connection'));
      }
    };
    es.addEventListener('open', onOpen);
    es.addEventListener('error', onError);
  });
  return _ssePromise;
}

/**
 * @param {string} name
 * @param {(event: { event: string, id: number, payload: unknown, source: unknown }) => void} handler
 * @returns {Promise<() => Promise<void>>}
 */
export async function listen(name, handler) {
  const es = await ensureSSE();
  const id = _nextListenerId++;
  const listener = (msg) => {
    let envelope;
    try {
      envelope = JSON.parse(msg.data);
    } catch {
      // Non-JSON payload — wrap so user code sees something usable.
      envelope = { id: Number(msg.lastEventId) || 0, event: name, payload: msg.data, source: null };
    }
    handler({
      id: envelope.id,
      event: envelope.event ?? name,
      payload: envelope.payload,
      source: envelope.source,
    });
  };
  es.addEventListener(name, listener);
  _listenerEntries.set(id, { name, listener });
  return async () => {
    const entry = _listenerEntries.get(id);
    if (entry) {
      es.removeEventListener(entry.name, entry.listener);
      _listenerEntries.delete(id);
    }
  };
}

/**
 * @param {string} name
 * @param {(event: { event: string, id: number, payload: unknown, source: unknown }) => void} handler
 */
export async function once(name, handler) {
  const unlisten = await listen(name, (ev) => {
    unlisten().catch(() => {});
    handler(ev);
  });
  return unlisten;
}

/**
 * Send an event to the backend (target = Any).
 *
 * @param {string} name
 * @param {unknown} [payload]
 */
export async function emit(name, payload) {
  await postEmit({ event: name, payload, target: { kind: 'Any' } });
}

/**
 * Send an event with a specific target. `target` may be a label string
 * (interpreted as `AnyLabel`) or a structured `EventTarget`.
 *
 * @param {string | { kind: string, label?: string }} target
 * @param {string} name
 * @param {unknown} [payload]
 */
export async function emitTo(target, name, payload) {
  const t =
    typeof target === 'string' ? { kind: 'AnyLabel', label: target } : target;
  await postEmit({ event: name, payload, target: t });
}

async function postEmit(body) {
  const resp = await fetch(`${baseUrl()}/__tauri/emit`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-tauri-window': windowLabel(),
      'x-tauri-client-id': clientId(),
    },
    body: JSON.stringify(body),
  });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`emit failed: ${resp.status} ${text}`);
  }
}

/** Constants user code may import. The shim never actually fires these. */
export const TauriEvent = Object.freeze({
  WINDOW_RESIZED: 'tauri://resize',
  WINDOW_MOVED: 'tauri://move',
  WINDOW_CLOSE_REQUESTED: 'tauri://close-requested',
  WINDOW_DESTROYED: 'tauri://destroyed',
  WINDOW_FOCUS: 'tauri://focus',
  WINDOW_BLUR: 'tauri://blur',
  WINDOW_SCALE_FACTOR_CHANGED: 'tauri://scale-change',
  WINDOW_THEME_CHANGED: 'tauri://theme-changed',
  WINDOW_CREATED: 'tauri://window-created',
  WEBVIEW_CREATED: 'tauri://webview-created',
  DRAG_ENTER: 'tauri://drag-enter',
  DRAG_OVER: 'tauri://drag-over',
  DRAG_DROP: 'tauri://drag-drop',
  DRAG_LEAVE: 'tauri://drag-leave',
});

// Test helper: tear down the SSE singleton between tests in the same process.
export function __resetForTests() {
  if (_sse) {
    _sse.close();
    _sse = null;
  }
  _ssePromise = null;
  _listenerEntries.clear();
  _nextListenerId = 1;
}
