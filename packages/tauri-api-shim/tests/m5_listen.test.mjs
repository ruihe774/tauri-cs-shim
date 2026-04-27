// Drives the JS shim against a Rust shim server started by the Cargo test
// `m5_js_shim::js_shim_listen_round_trip`. The base URL is in
// `process.env.TAURI_DEBUG_BASE`.

import test from 'node:test';
import assert from 'node:assert/strict';

import { invoke } from '../src/core.js';
import { listen, once, __resetForTests as __resetEvent } from '../src/event.js';
import { __resetForTests as __resetRuntime } from '../src/runtime.js';

function withTimeout(promise, ms, label = 'timed out') {
  return Promise.race([
    promise,
    new Promise((_, reject) => setTimeout(() => reject(new Error(label)), ms)),
  ]);
}

// EventSource keeps the event loop alive. Each test must close it via
// __resetEvent() in a finally block; otherwise node never exits after the
// last test in this file.

test('listen receives an event triggered by an invoke', async () => {
  __resetEvent();
  __resetRuntime();
  try {
    const { promise, resolve } = Promise.withResolvers();
    const unlisten = await listen('greeting', (ev) => resolve(ev));
    await invoke('trigger_greeting', { msg: 'hello from node' });
    const ev = await withTimeout(promise, 3000, 'never received greeting');
    assert.equal(ev.event, 'greeting');
    assert.deepEqual(ev.payload, { from: 'rust', echoed: 'hello from node' });
    assert.equal(ev.source.kind, 'App');
    await unlisten();
  } finally {
    __resetEvent();
  }
});

test('once auto-unregisters', async () => {
  __resetEvent();
  __resetRuntime();
  try {
    let seen = 0;
    const { promise, resolve } = Promise.withResolvers();
    await once('one-shot', (_ev) => {
      seen++;
      resolve();
    });
    await invoke('trigger_one_shot');
    await invoke('trigger_one_shot');
    await withTimeout(promise, 3000, 'never fired');
    await new Promise((r) => setTimeout(r, 200));
    assert.equal(seen, 1);
  } finally {
    __resetEvent();
  }
});

test('targeted emit reaches only matching window', async () => {
  __resetEvent();
  __resetRuntime();
  try {
    let mainSeen = 0;
    let settingsSeen = 0;
    const unlistenMain = await listen('shouted', () => {
      mainSeen++;
    });
    const unlistenSettings = await listen('whisper', () => {
      settingsSeen++;
    });

    await invoke('emit_to_window', { label: 'settings', name: 'whisper' });
    await invoke('emit_to_window', { label: 'main', name: 'shouted' });

    await new Promise((r) => setTimeout(r, 300));
    assert.equal(mainSeen, 1, 'main should have received its targeted event');
    assert.equal(
      settingsSeen,
      0,
      'main should NOT have received the settings-targeted event'
    );
    await unlistenMain();
    await unlistenSettings();
  } finally {
    __resetEvent();
  }
});
