#!/usr/bin/env node
/**
 * check-env.mjs — Pre-build environment verification for Echoes Studio
 *
 * Checks that every required tool is installed at the right version before
 * you attempt to build. Safe to run at any time; it only reads, never writes.
 *
 * Usage: pnpm check-env
 *        node scripts/check-env.mjs
 */

import { execSync } from 'child_process';
import { existsSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import os from 'os';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');
const PLATFORM = os.platform(); // 'win32' | 'darwin' | 'linux'

// ─── ANSI colours (no external deps) ────────────────────────────────────────
const C = {
	reset: '\x1b[0m',
	bold: '\x1b[1m',
	dim: '\x1b[2m',
	green: '\x1b[32m',
	red: '\x1b[31m',
	yellow: '\x1b[33m',
	cyan: '\x1b[36m'
};

const fmt = {
	ok: `${C.green}✓ OK${C.reset}`,
	fail: `${C.red}✗ MISSING${C.reset}`,
	warn: `${C.yellow}⚠ WARNING${C.reset}`
};

// ─── State ───────────────────────────────────────────────────────────────────
let errorCount = 0;
let warnCount = 0;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/**
 * Run a shell command and return trimmed stdout, or null on failure.
 * Never throws — failures are surfaced as null.
 */
function run(cmd) {
	try {
		return execSync(cmd, {
			encoding: 'utf8',
			stdio: ['pipe', 'pipe', 'pipe'],
			timeout: 8000
		}).trim();
	} catch {
		return null;
	}
}

/**
 * Extract the first X.Y.Z version string from arbitrary command output.
 * Returns [major, minor, patch] or null if nothing matched.
 */
function parseVersion(str) {
	if (!str) return null;
	const m = str.match(/(\d+)\.(\d+)\.(\d+)/);
	return m ? [parseInt(m[1]), parseInt(m[2]), parseInt(m[3])] : null;
}

/** Returns true if `actual` is at least `required` (both [maj, min, patch]). */
function atLeast(actual, required) {
	if (!actual) return false;
	for (let i = 0; i < 3; i++) {
		if (actual[i] !== required[i]) return actual[i] > required[i];
	}
	return true; // equal
}

/** Format a version triple as a string. */
function vStr(v) {
	return v ? v.join('.') : 'not found';
}

/** Print one check row and update global counters. */
function check(label, status, detail, hint) {
	const icon = fmt[status];
	const col = status === 'ok' ? C.green : status === 'warn' ? C.yellow : C.red;
	console.log(`  ${icon}  ${C.bold}${label}${C.reset}  ${C.dim}${detail}${C.reset}`);
	if (hint && status !== 'ok') {
		// Multi-line hints: indent each line
		for (const line of hint.split('\n')) {
			console.log(`       ${C.dim}→ ${line}${C.reset}`);
		}
	}
	if (status === 'fail') errorCount++;
	if (status === 'warn') warnCount++;
}

/** Print a titled section separator. */
function section(title) {
	console.log(`\n${C.bold}${C.cyan}${title}${C.reset}`);
	console.log(`${C.dim}${'─'.repeat(52)}${C.reset}`);
}

// ─── Header ──────────────────────────────────────────────────────────────────

console.log(`\n${C.bold}Echoes Studio — Environment Check${C.reset}`);
console.log(`${C.dim}Platform : ${PLATFORM} ${os.arch()}${C.reset}`);
console.log(`${C.dim}Node     : ${process.version}${C.reset}`);
console.log(`${C.dim}Date     : ${new Date().toISOString().slice(0, 10)}${C.reset}`);

// ─── Node.js ─────────────────────────────────────────────────────────────────

section('Node.js');

const nodeV = parseVersion(process.version);
const nodeOk = atLeast(nodeV, [18, 0, 0]);
check(
	'Node.js ≥ 18.0.0',
	nodeOk ? 'ok' : 'fail',
	`found ${vStr(nodeV)}`,
	!nodeOk
		? 'Install the LTS release from https://nodejs.org\n' +
			'Or use a version manager: nvm (Unix) / nvm-windows (Windows) / volta'
		: undefined
);

// ─── pnpm ────────────────────────────────────────────────────────────────────

section('pnpm');

const pnpmOut = run('pnpm --version');
const pnpmV = parseVersion(pnpmOut);
const pnpmOk = atLeast(pnpmV, [8, 0, 0]);
check(
	'pnpm ≥ 8.0.0',
	pnpmV ? (pnpmOk ? 'ok' : 'fail') : 'fail',
	pnpmV ? `found ${vStr(pnpmV)}` : 'not found',
	!pnpmV || !pnpmOk
		? 'Install: npm install -g pnpm@latest\n' + 'Or via corepack: corepack enable && corepack prepare pnpm@latest --activate'
		: undefined
);

// ─── Rust toolchain ──────────────────────────────────────────────────────────

section('Rust toolchain');

const rustcOut = run('rustc --version');
const rustcV = parseVersion(rustcOut);
const rustcOk = atLeast(rustcV, [1, 60, 0]);
check(
	'rustc ≥ 1.60.0',
	rustcV ? (rustcOk ? 'ok' : 'fail') : 'fail',
	rustcV ? `found ${vStr(rustcV)}` : 'not found',
	!rustcV || !rustcOk
		? 'Install rustup from https://rustup.rs\n' +
			'Then run: rustup toolchain install stable'
		: undefined
);

const cargoOut = run('cargo --version');
const cargoV = parseVersion(cargoOut);
check(
	'cargo',
	cargoV ? 'ok' : 'fail',
	cargoV ? `found ${vStr(cargoV)}` : 'not found',
	!cargoV ? 'cargo ships with rustup — install from https://rustup.rs' : undefined
);

// ─── Node modules & Tauri CLI ─────────────────────────────────────────────

section('Node dependencies');

const nmExists = existsSync(join(ROOT, 'node_modules'));
check(
	'node_modules installed',
	nmExists ? 'ok' : 'fail',
	nmExists ? 'found' : 'missing',
	!nmExists ? 'Run: pnpm install' : undefined
);

// Check for Tauri CLI inside local node_modules (most reliable cross-platform check)
const tauriPkgDir = join(ROOT, 'node_modules', '@tauri-apps', 'cli');
const tauriInstalled = existsSync(tauriPkgDir);
if (tauriInstalled) {
	// Try to read version from the package.json in the installed package
	let tauriVersion = 'unknown';
	try {
		const { createRequire } = await import('module');
		const req = createRequire(import.meta.url);
		const tauriPkg = req(join(tauriPkgDir, 'package.json'));
		tauriVersion = tauriPkg.version ?? 'unknown';
	} catch {
		// Fallback: try running it
		const raw = run('pnpm exec tauri --version');
		tauriVersion = raw ?? 'unknown';
	}
	check('@tauri-apps/cli (local)', 'ok', `found ${tauriVersion}`, undefined);
} else {
	check(
		'@tauri-apps/cli (local)',
		nmExists ? 'fail' : 'warn',
		'not found in node_modules',
		'Run: pnpm install'
	);
}

// ─── Platform-specific requirements ──────────────────────────────────────────

section('Platform-specific requirements');

if (PLATFORM === 'linux') {
	// WebKit is mandatory for Tauri on Linux
	const webkit = run('pkg-config --exists webkit2gtk-4.0 2>/dev/null && echo yes');
	check(
		'webkit2gtk-4.0',
		webkit === 'yes' ? 'ok' : 'fail',
		webkit === 'yes' ? 'found via pkg-config' : 'missing',
		webkit !== 'yes'
			? 'Debian/Ubuntu : sudo apt install libwebkit2gtk-4.0-dev build-essential curl wget\n' +
				'Fedora        : sudo dnf install webkit2gtk3-devel\n' +
				'Arch          : sudo pacman -S webkit2gtk'
			: undefined
	);

	// GTK (usually pulled in by webkit but worth checking)
	const gtk = run('pkg-config --exists gtk+-3.0 2>/dev/null && echo yes');
	check(
		'gtk+-3.0',
		gtk === 'yes' ? 'ok' : 'warn',
		gtk === 'yes' ? 'found via pkg-config' : 'missing',
		gtk !== 'yes'
			? 'Debian/Ubuntu : sudo apt install libgtk-3-dev\n' +
				'Fedora        : sudo dnf install gtk3-devel'
			: undefined
	);

	// patchelf — needed to bundle AppImage
	const patchelf = run('patchelf --version');
	check(
		'patchelf (AppImage builds)',
		patchelf ? 'ok' : 'warn',
		patchelf ? `found ${patchelf.split('\n')[0]}` : 'missing (only needed for AppImage)',
		!patchelf ? 'Debian/Ubuntu : sudo apt install patchelf' : undefined
	);
} else if (PLATFORM === 'darwin') {
	// Xcode CLI tools are required (provides clang, linker, etc.)
	const xcrunPath = run('xcode-select -p');
	check(
		'Xcode Command Line Tools',
		xcrunPath ? 'ok' : 'fail',
		xcrunPath ? `found at ${xcrunPath}` : 'not installed',
		!xcrunPath ? 'Run: xcode-select --install' : undefined
	);

	// Check that the macOS SDK is accessible
	const sdk = run('xcrun --show-sdk-path');
	check(
		'macOS SDK',
		sdk ? 'ok' : 'warn',
		sdk ? sdk : 'not found',
		!sdk ? 'Reinstall Xcode CLT: sudo xcode-select --reset' : undefined
	);
} else if (PLATFORM === 'win32') {
	// WebView2 — bundled with Windows 11, must be installed separately on Win10
	// Check the two most common registry locations for the Evergreen runtime
	const reg1 = run(
		'reg query "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv'
	);
	const reg2 = run(
		'reg query "HKCU\\SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv'
	);
	const hasWebView2 = !!(reg1 || reg2);
	check(
		'Microsoft WebView2 Runtime',
		// On Win11 it's always there; treat absence as a warning, not a hard error,
		// because the Tauri installer can bundle it.
		hasWebView2 ? 'ok' : 'warn',
		hasWebView2 ? 'found in registry' : 'not found in registry (may still work on Win 11)',
		!hasWebView2
			? 'Download Evergreen Bootstrapper:\n' +
				'https://developer.microsoft.com/en-us/microsoft-edge/webview2/'
			: undefined
	);

	// MSVC Build Tools — needed for crates that compile C/C++ (e.g. cpal)
	const cl = run('where cl.exe');
	check(
		'MSVC Build Tools (cl.exe)',
		cl ? 'ok' : 'warn',
		cl ? `found at ${cl.split('\n')[0].trim()}` : 'not in PATH',
		!cl
			? 'Install Visual Studio Build Tools 2019 or later:\n' +
				'https://visualstudio.microsoft.com/visual-cpp-build-tools/\n' +
				'Select "Desktop development with C++" workload.\n' +
				'Alternatively, install the full Visual Studio IDE.'
			: undefined
	);
} else {
	check('Platform', 'warn', `Unsupported platform "${PLATFORM}" — no platform checks run`, undefined);
}

// ─── Summary ─────────────────────────────────────────────────────────────────

console.log(`\n${C.dim}${'─'.repeat(52)}${C.reset}`);

if (errorCount === 0 && warnCount === 0) {
	console.log(`${C.green}${C.bold}All checks passed — ready to build!${C.reset}`);
	console.log(`${C.dim}Run: pnpm tauri:dev${C.reset}\n`);
	process.exit(0);
} else if (errorCount === 0) {
	console.log(
		`${C.yellow}${C.bold}${warnCount} warning(s) — build will likely work but review the warnings above.${C.reset}`
	);
	console.log(`${C.dim}Run: pnpm tauri:dev${C.reset}\n`);
	process.exit(0);
} else {
	console.log(
		`${C.red}${C.bold}${errorCount} error(s), ${warnCount} warning(s) — fix the issues above before building.${C.reset}\n`
	);
	process.exit(1);
}
