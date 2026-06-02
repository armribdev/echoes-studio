#!/usr/bin/env node
/**
 * test-all.mjs — Run the full test/validation suite, cross-platform.
 *
 * Wraps the individual checks that a contributor must pass before opening a
 * PR into one command. Each step is run regardless of earlier failures so the
 * developer sees every problem in one pass; the process exits non-zero if any
 * step failed.
 *
 * Usage:
 *   node scripts/test-all.mjs
 *   node scripts/test-all.mjs --skip-rust    # frontend-only run
 *   node scripts/test-all.mjs --skip-js      # rust-only run
 */

import { spawn } from 'child_process';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import os from 'os';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');
const SRC_TAURI = join(ROOT, 'src-tauri');

const skipRust = process.argv.includes('--skip-rust');
const skipJs = process.argv.includes('--skip-js');

// ─── Colours ────────────────────────────────────────────────────────────────
const C = {
	reset: '\x1b[0m',
	bold: '\x1b[1m',
	dim: '\x1b[2m',
	green: '\x1b[32m',
	red: '\x1b[31m',
	cyan: '\x1b[36m'
};

/**
 * Run a child process and stream its output. Returns the exit code.
 * `cmd` is resolved through the shell so it works for both `pnpm` (a .cmd shim
 * on Windows) and `cargo` (a real .exe). We never pass user input to the shell
 * — only literals from the table below — so there is no injection surface.
 */
function run(cmd, args, cwd) {
	return new Promise((resolve) => {
		const useShell = os.platform() === 'win32'; // .cmd shims need a shell on Windows
		const child = spawn(cmd, args, { cwd, stdio: 'inherit', shell: useShell });
		child.on('exit', (code) => resolve(code ?? 1));
	});
}

// Step table — each entry is fully self-contained.
const steps = [
	{ id: 'check', label: 'TypeScript + Svelte check', cmd: 'pnpm', args: ['run', 'check'], cwd: ROOT, group: 'js' },
	{ id: 'lint', label: 'Prettier + ESLint', cmd: 'pnpm', args: ['run', 'lint'], cwd: ROOT, group: 'js' },
	{ id: 'fmt', label: 'cargo fmt --check', cmd: 'cargo', args: ['fmt', '--', '--check'], cwd: SRC_TAURI, group: 'rust' },
	// `--workspace`: src-tauri is now a workspace root (ADR-015 6c-ii); lint and
	// test the sibling crates (plugin_bridge, plugin_host_bridge) too.
	{ id: 'clippy', label: 'cargo clippy', cmd: 'cargo', args: ['clippy', '--workspace', '--', '-D', 'warnings'], cwd: SRC_TAURI, group: 'rust' },
	{ id: 'test', label: 'cargo test', cmd: 'cargo', args: ['test', '--workspace'], cwd: SRC_TAURI, group: 'rust' }
];

const results = [];

for (const step of steps) {
	if (step.group === 'rust' && skipRust) continue;
	if (step.group === 'js' && skipJs) continue;

	console.log(`\n${C.bold}${C.cyan}▶ ${step.label}${C.reset}`);
	console.log(`${C.dim}${step.cmd} ${step.args.join(' ')} (in ${step.cwd === ROOT ? '.' : 'src-tauri/'})${C.reset}`);
	const code = await run(step.cmd, step.args, step.cwd);
	results.push({ ...step, code });
}

// ─── Summary ────────────────────────────────────────────────────────────────

console.log(`\n${C.bold}Summary${C.reset}`);
console.log(`${C.dim}${'─'.repeat(52)}${C.reset}`);
let failed = 0;
for (const r of results) {
	const ok = r.code === 0;
	const tag = ok ? `${C.green}PASS${C.reset}` : `${C.red}FAIL${C.reset}`;
	console.log(`  ${tag}  ${r.label}`);
	if (!ok) failed++;
}
console.log('');
process.exit(failed === 0 ? 0 : 1);
