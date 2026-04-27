#!/usr/bin/env node
// Tiny replacement for the upstream `tauri` CLI when running under
// tauri-cs-shim. The user's package.json declares `@tauri-apps/cli` as a
// `file:` dep pointing here; pnpm/npm then drops a `tauri` bin into
// node_modules/.bin/ and `pnpm tauri dev` runs this script.
//
// Supported subcommand: `dev`. Reads src-tauri/tauri.conf.json, spawns the
// frontend dev command and the Rust binary in parallel, polls the dev URL,
// then opens a browser.

import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import process from 'node:process';
import { parseArgs } from 'node:util';

const SUB = process.argv[2];

if (!SUB || SUB === '-h' || SUB === '--help' || SUB === 'help') {
  printHelp();
  process.exit(SUB ? 0 : 1);
}

if (SUB === '-V' || SUB === '--version') {
  console.log('tauri-cs-shim cli 2.99.0');
  process.exit(0);
}

if (SUB === 'dev') {
  await runDev(process.argv.slice(3));
} else if (SUB === 'build') {
  console.error(
    '[tauri-cs-shim] `tauri build` is not implemented in the shim. The shim exists to run a Tauri app as an HTTP server in a real browser; it cannot produce a webview bundle. Use the upstream Tauri CLI for production builds.',
  );
  process.exit(64);
} else {
  console.error(`[tauri-cs-shim] unknown subcommand: ${SUB}`);
  printHelp();
  process.exit(64);
}

function printHelp() {
  console.log(
    [
      'tauri-cs-shim — minimal `tauri` CLI replacement',
      '',
      'Usage:',
      '  tauri dev [--shim-port <port>] [--no-open]',
      '  tauri build   (not supported under the shim)',
      '',
      'The dev subcommand reads src-tauri/tauri.conf.json and:',
      "  1. starts build.beforeDevCommand (e.g. `pnpm dev`)",
      '  2. cargo run --manifest-path src-tauri/Cargo.toml',
      '  3. polls build.devUrl until reachable',
      '  4. opens that URL in the system browser',
      '',
      'Env:',
      '  TAURI_DEBUG_PORT  port for the Rust shim HTTP server (default 1421)',
      '  TAURI_DEBUG_HOST  bind host (default 127.0.0.1)',
    ].join('\n'),
  );
}

async function runDev(rawArgs) {
  const { values } = parseArgs({
    args: rawArgs,
    options: {
      'shim-port': { type: 'string' },
      'no-open': { type: 'boolean' },
    },
    allowPositionals: true,
    strict: false,
  });

  const projectDir = process.cwd();
  const tauriDir = path.join(projectDir, 'src-tauri');
  const confPath = path.join(tauriDir, 'tauri.conf.json');

  if (!fs.existsSync(confPath)) {
    console.error(
      `[tauri-cs-shim] no src-tauri/tauri.conf.json at ${confPath}. Run from the project root.`,
    );
    process.exit(2);
  }
  const conf = JSON.parse(fs.readFileSync(confPath, 'utf8'));
  const beforeDev = conf?.build?.beforeDevCommand;
  const devUrlStr = conf?.build?.devUrl;
  if (!devUrlStr) {
    console.error('[tauri-cs-shim] build.devUrl missing in tauri.conf.json');
    process.exit(2);
  }
  const devUrl = new URL(devUrlStr);

  const shimPort = String(values['shim-port'] ?? process.env.TAURI_DEBUG_PORT ?? '1421');
  const shimHost = process.env.TAURI_DEBUG_HOST ?? '127.0.0.1';
  const shouldOpen = !values['no-open'];

  /** @type {import('node:child_process').ChildProcess[]} */
  const children = [];
  let shuttingDown = false;

  const shutdown = (code = 0) => {
    if (shuttingDown) return;
    shuttingDown = true;
    console.error('[tauri-cs-shim] shutting down…');
    // pnpm/cargo wrap their real workers in deeper subprocesses (sh → pnpm →
    // node → vite, cargo → rustc → target/debug/<bin>) and we cannot rely on
    // process-group inheritance to reach them. Walk the tree explicitly.
    for (const c of children) {
      if (c.exitCode === null && !c.killed && c.pid) {
        killTree(c.pid, 'SIGTERM');
      }
    }
    // Some intermediates (pnpm in particular) catch SIGTERM, ack it, then
    // hang. Hard-kill the tree once the grace window elapses. Don't unref
    // the timer — we need it to fire even after the event loop empties.
    setTimeout(() => {
      for (const c of children) {
        if (c.pid) killTree(c.pid, 'SIGKILL');
      }
      process.exit(code);
    }, 1500);
  };

  process.on('SIGINT', () => shutdown(130));
  process.on('SIGTERM', () => shutdown(143));
  process.on('SIGHUP', () => shutdown(129));
  // Last-resort cleanup if something else (uncaught exception, parent
  // disowning us) brings the launcher down without going through shutdown.
  process.on('exit', () => {
    if (shuttingDown) return;
    for (const c of children) {
      if (c.pid && c.exitCode === null) {
        try {
          for (const p of collectDescendants(c.pid).reverse()) {
            try {
              process.kill(p, 'SIGKILL');
            } catch {}
          }
        } catch {}
      }
    }
  });

  if (beforeDev) {
    const fe = spawnPretty('frontend', beforeDev, { cwd: projectDir });
    children.push(fe);
    fe.on('exit', (code, signal) => {
      if (!shuttingDown) {
        console.error(`[tauri-cs-shim] frontend exited (code=${code}, signal=${signal})`);
        shutdown(code ?? 1);
      }
    });
  } else {
    console.error(
      '[tauri-cs-shim] build.beforeDevCommand missing — only starting the Rust shim. Run your dev server manually.',
    );
  }

  const cargoArgs = ['run', '--manifest-path', path.join(tauriDir, 'Cargo.toml')];
  const rs = spawnPretty('rust', `cargo ${cargoArgs.join(' ')}`, {
    cwd: projectDir,
    binary: 'cargo',
    args: cargoArgs,
    env: {
      ...process.env,
      TAURI_DEBUG_PORT: shimPort,
      TAURI_DEBUG_HOST: shimHost,
    },
  });
  children.push(rs);
  rs.on('exit', (code, signal) => {
    if (!shuttingDown) {
      console.error(`[tauri-cs-shim] rust shim exited (code=${code}, signal=${signal})`);
      shutdown(code ?? 1);
    }
  });

  // Wait for both the dev server and the Rust shim to be reachable.
  const devPort = devUrl.port ? Number(devUrl.port) : devUrl.protocol === 'https:' ? 443 : 80;
  const devHost = devUrl.hostname || '127.0.0.1';
  const [feOk, rsOk] = await Promise.all([
    waitForTcp(devHost, devPort, 120_000),
    waitForTcp(shimHost, Number(shimPort), 120_000),
  ]);
  if (!feOk) {
    console.error(
      `[tauri-cs-shim] timed out waiting for ${devUrl.toString()} — is the frontend dev server actually starting?`,
    );
    shutdown(1);
    return;
  }
  if (!rsOk) {
    console.error(
      `[tauri-cs-shim] timed out waiting for the Rust shim at http://${shimHost}:${shimPort} — check the [rust] log above.`,
    );
    shutdown(1);
    return;
  }
  console.log(`[tauri-cs-shim] dev server ready at ${devUrl.toString()}`);
  console.log(`[tauri-cs-shim] rust shim listening at http://${shimHost}:${shimPort}`);
  if (shouldOpen) {
    openBrowser(devUrl.toString());
  } else {
    console.log(`[tauri-cs-shim] open ${devUrl.toString()} in your browser`);
  }
}

/**
 * Spawn a child as a shell command, prefix its stdout/stderr lines, and return it.
 *
 * @param {string} label
 * @param {string} pretty - just for logging
 * @param {{cwd?: string, env?: NodeJS.ProcessEnv, binary?: string, args?: string[]}} opts
 */
function spawnPretty(label, pretty, opts = {}) {
  console.log(`[tauri-cs-shim] starting ${label}: ${pretty}`);
  const child = opts.binary
    ? spawn(opts.binary, opts.args ?? [], {
        cwd: opts.cwd,
        env: opts.env,
        stdio: ['inherit', 'pipe', 'pipe'],
      })
    : spawn(pretty, {
        cwd: opts.cwd,
        env: opts.env,
        stdio: ['inherit', 'pipe', 'pipe'],
        shell: true,
      });
  pipeWithPrefix(child.stdout, label, false);
  pipeWithPrefix(child.stderr, label, true);
  return child;
}

/**
 * @param {NodeJS.ReadableStream | null} stream
 * @param {string} label
 * @param {boolean} isErr
 */
function pipeWithPrefix(stream, label, isErr) {
  if (!stream) return;
  const out = isErr ? process.stderr : process.stdout;
  let buf = '';
  stream.setEncoding('utf8');
  stream.on('data', (chunk) => {
    buf += chunk;
    let nl;
    while ((nl = buf.indexOf('\n')) !== -1) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      out.write(`[${label}] ${line}\n`);
    }
  });
  stream.on('end', () => {
    if (buf.length > 0) out.write(`[${label}] ${buf}\n`);
  });
}

async function waitForTcp(host, port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await tryTcp(host, port)) return true;
    await new Promise((r) => setTimeout(r, 250));
  }
  return false;
}

function tryTcp(host, port) {
  return new Promise((resolve) => {
    const sock = net.createConnection({ host, port, timeout: 1000 }, () => {
      sock.end();
      resolve(true);
    });
    sock.on('error', () => resolve(false));
    sock.on('timeout', () => {
      sock.destroy();
      resolve(false);
    });
  });
}

/**
 * Recursively kill `pid` and every descendant.
 *
 * Collects the full descendant set before signalling anything: if we killed
 * inner nodes first, detached grandchildren (e.g. pnpm → vite) would
 * reparent to PID 1 the instant pnpm died, and a naive recursive walk
 * would lose them.
 *
 * @param {number} pid
 * @param {NodeJS.Signals} signal
 */
function killTree(pid, signal) {
  if (process.platform === 'win32') {
    try {
      execFileSync('taskkill', ['/pid', String(pid), '/T', '/F'], { stdio: 'ignore' });
    } catch {}
    return;
  }
  const all = collectDescendants(pid);
  // Leaves first; root last.
  for (const p of all.slice().reverse()) {
    try {
      process.kill(p, signal);
    } catch {}
  }
}

/**
 * Walk the process tree from `root` and return every PID found, in
 * breadth-first order with `root` at index 0.
 *
 * @param {number} root
 * @returns {number[]}
 */
function collectDescendants(root) {
  const all = [root];
  const queue = [root];
  while (queue.length > 0) {
    const p = /** @type {number} */ (queue.shift());
    let children = [];
    try {
      const out = execFileSync('pgrep', ['-P', String(p)], {
        stdio: ['ignore', 'pipe', 'ignore'],
        encoding: 'utf8',
      });
      children = out
        .split('\n')
        .map((s) => Number.parseInt(s.trim(), 10))
        .filter((n) => Number.isFinite(n));
    } catch {
      // pgrep exits 1 when there are no matches; that's a leaf.
    }
    for (const c of children) {
      if (!all.includes(c)) {
        all.push(c);
        queue.push(c);
      }
    }
  }
  return all;
}

function openBrowser(url) {
  const platform = process.platform;
  let cmd;
  let args;
  if (platform === 'darwin') {
    cmd = 'open';
    args = [url];
  } else if (platform === 'win32') {
    cmd = 'cmd';
    args = ['/c', 'start', '""', url];
  } else {
    cmd = 'xdg-open';
    args = [url];
  }
  try {
    const child = spawn(cmd, args, { detached: true, stdio: 'ignore' });
    child.unref();
  } catch (e) {
    console.error(`[tauri-cs-shim] could not open browser: ${e.message}`);
  }
}
