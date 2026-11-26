#!/usr/bin/env node
/**
 * prepare-bridge.mjs — builds `plugin_host_bridge` in release and stages it where the
 * Tauri bundler expects a sidecar: `src-tauri/binaries/plugin_host_bridge-<target triple>[.exe]`.
 *
 * Run by `beforeBuildCommand`. The sidecar itself is declared in `src-tauri/tauri.bundle.conf.json`
 * (passed by `pnpm tauri:build` only), so plain `cargo` / `tauri dev` builds do not need it.
 */

import { execFileSync } from 'child_process';
import { copyFileSync, existsSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const TAURI_DIR = join(ROOT, 'src-tauri');
const EXE_SUFFIX = process.platform === 'win32' ? '.exe' : '';

const hostLine = execFileSync('rustc', ['-vV'], { encoding: 'utf8' })
	.split('\n')
	.find((line) => line.startsWith('host: '));
if (!hostLine) {
	console.error('prepare-bridge: cannot read the target triple from `rustc -vV`');
	process.exit(1);
}
const targetTriple = hostLine.slice('host: '.length).trim();

execFileSync(
	'cargo',
	[
		'build',
		'--release',
		'--manifest-path',
		join(TAURI_DIR, 'Cargo.toml'),
		'-p',
		'plugin_host_bridge'
	],
	{ stdio: 'inherit' }
);

const built = join(TAURI_DIR, 'target', 'release', `plugin_host_bridge${EXE_SUFFIX}`);
if (!existsSync(built)) {
	console.error(`prepare-bridge: ${built} was not produced`);
	process.exit(1);
}

const stagingDir = join(TAURI_DIR, 'binaries');
mkdirSync(stagingDir, { recursive: true });
copyFileSync(built, join(stagingDir, `plugin_host_bridge-${targetTriple}${EXE_SUFFIX}`));
