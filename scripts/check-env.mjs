#!/usr/bin/env node
/**
 * check-env.mjs — Pre-build environment verification for Echoes Studio
 *
 * Checks that every required tool is installed at the right version before
 * you attempt to build. Safe to run at any time; it only reads, never writes.
 *
 * Usage:
 *   pnpm check-env
 *   node scripts/check-env.mjs              # full report
 *   node scripts/check-env.mjs --quiet      # only print failures (used by predev hook)
 *
 * Exit codes:
 *   0  every required tool is present
 *   1  one or more required tools are missing
 */

import { execSync, execFileSync } from 'child_process';
import { existsSync, statSync, readFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import os from 'os';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');
const PLATFORM = os.platform(); // 'win32' | 'darwin' | 'linux'
const QUIET = process.argv.includes('--quiet');

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
const failures = []; // collected for quiet mode

// ─── Helpers ─────────────────────────────────────────────────────────────────

/**
 * Run a command through the shell and return trimmed stdout, or null on failure.
 * Never throws. Used for commands whose stderr we want to discard regardless
 * of the platform (and where the args are static literals).
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
 * Run an executable directly (no shell). Use this when the command path may
 * contain spaces — typical on Windows ("C:\\Program Files\\..."). Returns
 * trimmed stdout, or null on failure.
 */
function runFile(file, args = []) {
	try {
		return execFileSync(file, args, {
			encoding: 'utf8',
			stdio: ['pipe', 'pipe', 'pipe'],
			timeout: 8000,
			windowsHide: true
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
	return true;
}

/** Format a version triple as a string. */
function vStr(v) {
	return v ? v.join('.') : 'not found';
}

/** Print one check row and update global counters. */
function check(label, status, detail, hint) {
	if (status === 'fail') errorCount++;
	if (status === 'warn') warnCount++;
	if (status !== 'ok') failures.push({ label, status, detail, hint });
	if (QUIET && status === 'ok') return;
	const icon = fmt[status];
	console.log(`  ${icon}  ${C.bold}${label}${C.reset}  ${C.dim}${detail}${C.reset}`);
	if (hint && status !== 'ok') {
		for (const line of hint.split('\n')) {
			console.log(`       ${C.dim}→ ${line}${C.reset}`);
		}
	}
}

/** Print a titled section separator. Suppressed in quiet mode. */
function section(title) {
	if (QUIET) return;
	console.log(`\n${C.bold}${C.cyan}${title}${C.reset}`);
	console.log(`${C.dim}${'─'.repeat(52)}${C.reset}`);
}

// ─── Header ──────────────────────────────────────────────────────────────────

if (!QUIET) {
	console.log(`\n${C.bold}Echoes Studio — Environment Check${C.reset}`);
	console.log(`${C.dim}Platform : ${PLATFORM} ${os.arch()}${C.reset}`);
	console.log(`${C.dim}Node     : ${process.version}${C.reset}`);
	console.log(`${C.dim}Date     : ${new Date().toISOString().slice(0, 10)}${C.reset}`);
}

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
		? 'Install: npm install -g pnpm@latest\n' +
			'Or via corepack: corepack enable && corepack prepare pnpm@latest --activate'
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
		? 'Install rustup from https://rustup.rs\n' + 'Then run: rustup toolchain install stable'
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

// ─── Project files & Node modules ───────────────────────────────────────────

section('Project state');

const lockPath = join(ROOT, 'pnpm-lock.yaml');
const pkgPath = join(ROOT, 'package.json');
const nmPath = join(ROOT, 'node_modules');
const nmModulesYaml = join(nmPath, '.modules.yaml');

const lockExists = existsSync(lockPath);
check(
	'pnpm-lock.yaml present',
	lockExists ? 'ok' : 'fail',
	lockExists ? 'found' : 'missing',
	!lockExists ? 'pnpm-lock.yaml must be committed — run: pnpm install' : undefined
);

const nmExists = existsSync(nmPath);
check(
	'node_modules installed',
	nmExists ? 'ok' : 'fail',
	nmExists ? 'found' : 'missing',
	!nmExists ? 'Run: pnpm install' : undefined
);

// Detect stale node_modules vs. lockfile (common cause of mysterious errors
// after a `git pull`). We compare mtimes — not perfect, but fast and useful.
if (nmExists && lockExists) {
	try {
		const lockMtime = statSync(lockPath).mtimeMs;
		// .modules.yaml is rewritten by pnpm on every install; use it as the
		// "last install" marker rather than node_modules/ itself (whose mtime
		// changes whenever Cargo/Vite touches sibling paths).
		const installMtime = existsSync(nmModulesYaml) ? statSync(nmModulesYaml).mtimeMs : 0;
		const stale = lockMtime > installMtime + 1000; // 1s tolerance
		check(
			'node_modules up to date',
			stale ? 'warn' : 'ok',
			stale ? 'pnpm-lock.yaml is newer than the last install' : 'in sync with lock file',
			stale ? 'Run: pnpm install' : undefined
		);
	} catch {
		// stat failure shouldn't break the whole check — silently skip.
	}
}

// Tauri CLI inside local node_modules.
const tauriPkgDir = join(ROOT, 'node_modules', '@tauri-apps', 'cli');
const tauriInstalled = existsSync(tauriPkgDir);
if (tauriInstalled) {
	let tauriVersion = 'unknown';
	try {
		const tauriPkg = JSON.parse(readFileSync(join(tauriPkgDir, 'package.json'), 'utf8'));
		tauriVersion = tauriPkg.version ?? 'unknown';
	} catch {
		tauriVersion = run('pnpm exec tauri --version') ?? 'unknown';
	}
	check('@tauri-apps/cli (local)', 'ok', `found ${tauriVersion}`);
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
	// Tauri's Linux requirements come in three groups: webkit (rendering),
	// GTK + GLib (UI shell), and bundler tools (rsvg, patchelf, appindicator).
	const linuxLibs = [
		{ pc: 'webkit2gtk-4.0', label: 'webkit2gtk-4.0', required: true,
		  hint: 'Debian/Ubuntu : sudo apt install libwebkit2gtk-4.0-dev\nFedora        : sudo dnf install webkit2gtk3-devel\nArch          : sudo pacman -S webkit2gtk' },
		{ pc: 'gtk+-3.0', label: 'gtk+-3.0', required: true,
		  hint: 'Debian/Ubuntu : sudo apt install libgtk-3-dev\nFedora        : sudo dnf install gtk3-devel' },
		{ pc: 'librsvg-2.0', label: 'librsvg-2.0', required: false,
		  hint: 'Debian/Ubuntu : sudo apt install librsvg2-dev\nFedora        : sudo dnf install librsvg2-devel' },
		{ pc: 'ayatana-appindicator3-0.1', label: 'ayatana-appindicator3-0.1', required: false, alt: 'appindicator3-0.1',
		  hint: 'Debian/Ubuntu : sudo apt install libayatana-appindicator3-dev\nFedora        : sudo dnf install libappindicator-gtk3-devel' }
	];
	for (const lib of linuxLibs) {
		const has = runFile('pkg-config', ['--exists', lib.pc]) !== null;
		const altHas = lib.alt ? runFile('pkg-config', ['--exists', lib.alt]) !== null : false;
		const ok = has || altHas;
		check(
			lib.label + (lib.required ? '' : ' (optional)'),
			ok ? 'ok' : lib.required ? 'fail' : 'warn',
			ok ? 'found via pkg-config' : 'missing',
			!ok ? lib.hint : undefined
		);
	}

	// patchelf is needed only when bundling AppImage but we surface it as warn.
	const patchelf = runFile('patchelf', ['--version']);
	check(
		'patchelf (AppImage builds)',
		patchelf ? 'ok' : 'warn',
		patchelf ? `found ${patchelf.split('\n')[0]}` : 'missing (only needed for AppImage)',
		!patchelf ? 'Debian/Ubuntu : sudo apt install patchelf' : undefined
	);
} else if (PLATFORM === 'darwin') {
	// Xcode CLI tools provide the linker and SDK; they are mandatory.
	const xcrunPath = runFile('xcode-select', ['-p']);
	check(
		'Xcode Command Line Tools',
		xcrunPath ? 'ok' : 'fail',
		xcrunPath ? `found at ${xcrunPath}` : 'not installed',
		!xcrunPath ? 'Run: xcode-select --install' : undefined
	);
	const sdk = runFile('xcrun', ['--show-sdk-path']);
	check(
		'macOS SDK',
		sdk ? 'ok' : 'warn',
		sdk ? sdk : 'not found',
		!sdk ? 'Reinstall Xcode CLT: sudo xcode-select --reset' : undefined
	);
} else if (PLATFORM === 'win32') {
	// WebView2 — bundled with Windows 11, must be installed separately on Win10.
	// The Evergreen runtime registers under two possible registry keys.
	const reg1 = run(
		'reg query "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv'
	);
	const reg2 = run(
		'reg query "HKCU\\SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv'
	);
	const hasWebView2 = !!(reg1 || reg2);
	check(
		'Microsoft WebView2 Runtime',
		// On Win11 it's always there; absence is a warning, not a hard error,
		// because the Tauri installer can bundle the bootstrapper.
		hasWebView2 ? 'ok' : 'warn',
		hasWebView2 ? 'found in registry' : 'not found in registry (may still work on Win 11)',
		!hasWebView2
			? 'Download Evergreen Bootstrapper:\nhttps://developer.microsoft.com/en-us/microsoft-edge/webview2/'
			: undefined
	);

	// MSVC Build Tools — `where cl.exe` only works inside the Developer
	// Command Prompt. Probing via vswhere.exe (shipped by every VS 2017+
	// installer) detects the toolchain from any shell.
	const programFilesX86 = process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)';
	const vswhere = join(programFilesX86, 'Microsoft Visual Studio', 'Installer', 'vswhere.exe');
	let msvcOk = false;
	let msvcDetail = 'not found';

	if (existsSync(vswhere)) {
		// `-products *` includes Build Tools (not just full VS), `-requires` the C++ workload.
		const out = runFile(vswhere, [
			'-latest',
			'-products', '*',
			'-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
			'-property', 'installationVersion'
		]);
		if (out) {
			msvcOk = true;
			msvcDetail = `found VS Build Tools ${out}`;
		}
	}

	// Fallback: cl.exe on PATH (works inside a Developer Command Prompt).
	if (!msvcOk) {
		const cl = run('where cl.exe');
		if (cl) {
			msvcOk = true;
			msvcDetail = `found cl.exe at ${cl.split(/\r?\n/)[0].trim()}`;
		}
	}

	check(
		'MSVC Build Tools (C++ workload)',
		msvcOk ? 'ok' : 'fail',
		msvcDetail,
		!msvcOk
			? 'Install Visual Studio Build Tools 2019 or later:\n' +
				'https://visualstudio.microsoft.com/visual-cpp-build-tools/\n' +
				'Select "Desktop development with C++" workload.'
			: undefined
	);
} else {
	check('Platform', 'warn', `Unsupported platform "${PLATFORM}" — no platform checks run`);
}

// ─── Summary ─────────────────────────────────────────────────────────────────

if (!QUIET) {
	console.log(`\n${C.dim}${'─'.repeat(52)}${C.reset}`);
}

if (errorCount === 0 && warnCount === 0) {
	if (!QUIET) {
		console.log(`${C.green}${C.bold}All checks passed — ready to build!${C.reset}`);
		console.log(`${C.dim}Run: pnpm tauri:dev${C.reset}\n`);
	}
	process.exit(0);
} else if (errorCount === 0) {
	if (!QUIET) {
		console.log(
			`${C.yellow}${C.bold}${warnCount} warning(s) — build will likely work but review the warnings above.${C.reset}`
		);
		console.log(`${C.dim}Run: pnpm tauri:dev${C.reset}\n`);
	}
	process.exit(0);
} else {
	if (QUIET) {
		// In quiet mode we deferred all output until now; print only failures.
		console.log(`\n${C.red}${C.bold}Environment check failed:${C.reset}`);
		for (const f of failures) {
			if (f.status !== 'fail') continue;
			console.log(`  ${fmt.fail}  ${C.bold}${f.label}${C.reset}  ${C.dim}${f.detail}${C.reset}`);
			if (f.hint) {
				for (const line of f.hint.split('\n')) {
					console.log(`       ${C.dim}→ ${line}${C.reset}`);
				}
			}
		}
		console.log(`\n${C.dim}Run ${C.bold}pnpm check-env${C.reset}${C.dim} for the full report.${C.reset}\n`);
	} else {
		console.log(
			`${C.red}${C.bold}${errorCount} error(s), ${warnCount} warning(s) — fix the issues above before building.${C.reset}\n`
		);
	}
	process.exit(1);
}
