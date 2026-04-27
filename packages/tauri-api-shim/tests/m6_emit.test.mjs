// Drives the JS shim's emit/emitTo against the Rust shim started by the
// Cargo test `m6_js_shim::js_shim_emit_round_trip`.

import test from 'node:test';
import assert from 'node:assert/strict';

import { invoke } from '../src/core.js';
import { emit, emitTo, __resetForTests as __resetEvent } from '../src/event.js';
import { __resetForTests as __resetRuntime } from '../src/runtime.js';

async function poll(predicate, { timeout = 3000, interval = 50 } = {}) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const v = await predicate();
    if (v) return v;
    await new Promise((r) => setTimeout(r, interval));
  }
  throw new Error('poll timed out');
}

test('event.emit reaches a backend App listener', async () => {
  __resetEvent();
  __resetRuntime();
  await emit('greeting', { from: 'node' });
  const rows = await poll(async () => {
    const r = await invoke('show_recorded');
    return Array.isArray(r) && r.some(([n]) => n === 'greeting') ? r : null;
  });
  const greetings = rows.filter(([n]) => n === 'greeting');
  assert.equal(greetings.length, 1);
  assert.deepEqual(greetings[0][1], { from: 'node' });
});

test('event.emitTo with a label is filtered out by an App listener', async () => {
  __resetEvent();
  __resetRuntime();
  // emitTo('popup', ...) → target=AnyLabel{popup}. App listeners do NOT
  // match labelled events (per upstream's routing); the broadcast emit() is
  // what should reach the App listener.
  await emitTo('popup', 'targeted', 'pew');
  await emit('targeted', 'broadcast');
  const rows = await poll(async () => {
    const r = await invoke('show_recorded');
    return r.filter(([n]) => n === 'targeted').length >= 1 ? r : null;
  });
  const targeted = rows.filter(([n]) => n === 'targeted');
  // Only the `Any`-target emit landed in the App listener.
  assert.equal(targeted.length, 1, `expected 1 hit, got ${targeted.length}`);
  assert.equal(targeted[0][1], 'broadcast');
});
