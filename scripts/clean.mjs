#!/usr/bin/env node
/**
 * clean.mjs — Cross-platform clean for Echoes Studio.
 *
 * Removes every build output directory used by the project so the next build
 * starts from a known-empty state. Safe to run at any time.
 *
 * Usage:
 *   node scripts/clean.mjs           # clean frontend + Rust target
 *   node scripts/clean.mjs --deep    # also remove node_modules and pnpm store cache for this project
 *
 * Why this script exists:
 *   `rm -rf` is not portable to Windows shells. Doing the same job in Node lets
 *   `pnpm clean` work identically on every OS without needing rimraf or shx.
 */

import { rm } from 'fs/promises';
import { existsSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');

const DEEP = process.argv.includes('--deep');

// ─── ANSI colours (matches check-env.mjs) ────────────────────────────────────
const C = {
	reset: '\x1b[0m',
	bold: '\x1b[1m',
	dim: '\x1b[2m',
	green: '\x1b[32m',
	yellow: '\x1b[33m',
	cyan: '\x1b[36m'
};

// Each entry: [absolute path, human label]. Order is from cheapest to most aggressive.
const targets = [
	[join(ROOT, 'build'), 'SvelteKit output (build/)'],
	[join(ROOT, '.svelte-kit'), 'SvelteKit cache (.svelte-kit/)'],
	[join(ROOT, 'src-tauri', 'target'), 'Rust build cache (src-tauri/target/)']
];

if (DEEP) {
	targets.push([join(ROOT, 'node_modules'), 'Node modules (node_modules/)']);
}

console.log(`\n${C.bold}${C.cyan}Cleaning build outputs${C.reset}`);
console.log(`${C.dim}${'─'.repeat(52)}${C.reset}`);

let removed = 0;
let skipped = 0;

for (const [path, label] of targets) {
	if (!existsSync(path)) {
		console.log(`  ${C.dim}· skipped${C.reset}  ${label} ${C.dim}(not present)${C.reset}`);
		skipped++;
		continue;
	}
	process.stdout.write(`  ${C.yellow}…${C.reset} removing ${label} `);
	try {
		// recursive + force handles read-only files on Windows (e.g. .git inside target/)
		// maxRetries copes with antivirus / file-handle races on Windows.
		await rm(path, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
		process.stdout.write(`${C.green}done${C.reset}\n`);
		removed++;
	} catch (err) {
		process.stdout.write(`${C.dim}failed${C.reset}\n`);
		console.error(`     → ${err.message}`);
		process.exitCode = 1;
	}
}

console.log(`${C.dim}${'─'.repeat(52)}${C.reset}`);
console.log(
	`${C.bold}${removed} removed${C.reset}, ${skipped} skipped. ` +
		(DEEP ? `Re-run ${C.bold}pnpm install${C.reset} before building.\n` : '\n')
);
