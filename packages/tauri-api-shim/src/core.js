// `@tauri-apps/api/core` shim: invoke() and Channel.

import { baseUrl, clientId, windowLabel } from './runtime.js';

/**
 * Call a backend command. Mirrors `@tauri-apps/api/core.invoke`.
 *
 * @param {string} cmd
 * @param {Record<string, unknown>} [args]
 * @param {{ headers?: Record<string,string> }} [options]
 * @returns {Promise<unknown>}
 */
export async function invoke(cmd, args = {}, options = {}) {
  const url = `${baseUrl()}/__tauri/invoke/${encodeURIComponent(cmd)}`;
  const headers = {
    'content-type': 'application/json',
    'x-tauri-window': windowLabel(),
    'x-tauri-client-id': clientId(),
    ...(options.headers || {}),
  };
  const resp = await fetch(url, {
    method: 'POST',
    headers,
    body: JSON.stringify(args ?? {}),
  });
  const text = await resp.text();
  const value = text.length === 0 ? null : JSON.parse(text);
  if (!resp.ok) {
    // Match upstream's promise-rejection behaviour: throw the error JSON.
    throw value;
  }
  return value;
}

/**
 * Stream-style channel placeholder. Real implementation lands in M8.
 */
export class Channel {
  constructor() {
    this.id = Math.random().toString(36).slice(2);
    /** @type {((value: unknown) => void) | null} */
    this.onmessage = null;
  }
}
