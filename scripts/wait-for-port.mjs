#!/usr/bin/env node
/**
 * wait-for-port.mjs — Wait until a TCP port accepts connections on localhost.
 *
 * Cross-platform replacement for shell-specific port probes (PowerShell's
 * Test-NetConnection, bash's nc, etc.). Used by the VS Code "vite-ready" task
 * so the LLDB/cppvsdbg launcher waits for Vite before attaching to Tauri.
 *
 * Usage:
 *   node scripts/wait-for-port.mjs <port> [--timeout <seconds>] [--host <host>]
 *
 * Exit codes:
 *   0  port is open
 *   1  timed out
 *   2  bad arguments
 */

import net from 'net';

// ─── Argument parsing ────────────────────────────────────────────────────────

const args = process.argv.slice(2);
const positional = args.filter((a) => !a.startsWith('--'));
const port = parseInt(positional[0], 10);

if (!port || port < 1 || port > 65535) {
	console.error('usage: node scripts/wait-for-port.mjs <port> [--timeout <s>] [--host <host>]');
	process.exit(2);
}

function flag(name, fallback) {
	const i = args.indexOf(`--${name}`);
	return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
}

const timeoutSec = parseInt(flag('timeout', '60'), 10);
const host = flag('host', '127.0.0.1');

// ─── Probe loop ──────────────────────────────────────────────────────────────

/** Attempt a single TCP connection; resolves true if open, false otherwise. */
function probe() {
	return new Promise((resolve) => {
		const socket = net.createConnection({ host, port });
		// Per-attempt timeout is short — we want a tight polling loop overall.
		const t = setTimeout(() => {
			socket.destroy();
			resolve(false);
		}, 500);
		socket.once('connect', () => {
			clearTimeout(t);
			socket.end();
			resolve(true);
		});
		socket.once('error', () => {
			clearTimeout(t);
			resolve(false);
		});
	});
}

const deadline = Date.now() + timeoutSec * 1000;
process.stdout.write(`Waiting for ${host}:${port} (timeout ${timeoutSec}s)`);

while (Date.now() < deadline) {
	if (await probe()) {
		process.stdout.write(` — ready\n`);
		process.exit(0);
	}
	process.stdout.write('.');
	await new Promise((r) => setTimeout(r, 300));
}

process.stdout.write(`\nTimed out after ${timeoutSec}s waiting for ${host}:${port}\n`);
process.exit(1);
