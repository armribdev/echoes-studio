#!/usr/bin/env node
/**
 * check-controls.mjs — fail when a clickable widget is styled by hand.
 *
 * Every button, chip, field, dropdown, segmented group and slider takes its look from the
 * mixins of `src/controls.scss` (or from a shared component), so they all look alike. A
 * `.svelte` style rule that declares `cursor: pointer` must therefore `@include` one of
 * those mixins. Surfaces that are not a standard control (a clip, a pad, a ruler…) are
 * listed in `CUSTOM_SURFACES` with the reason.
 *
 * Also rejected: `cursor: not-allowed`, `accent-color` (native slider look) and slider thumb
 * pseudo-elements outside `controls.scss`.
 *
 * Usage: node scripts/check-controls.mjs
 */

import { readdirSync, readFileSync } from 'fs';
import { dirname, join, relative } from 'path';
import { fileURLToPath } from 'url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SRC = join(ROOT, 'src');

const CONTROL_MIXIN =
	/@include (?:button|button-primary|button-square|chip|icon-button|power-button|list-item|segment|field|range-slider)\b/;

/** `file::selector` of rules that are clickable on purpose without being a standard control. */
const CUSTOM_SURFACES = new Map([
	[
		'src/components/effects/EffectSlotHead.svelte::.slot-toggle',
		'whole effect header is the toggle'
	],
	['src/components/MarkerLane.svelte::.region', 'timeline region'],
	['src/components/MarkerLane.svelte::.marker', 'timeline marker'],
	['src/components/MenuBar.svelte::.tempo', 'transport readout that opens the project settings'],
	['src/components/pianoRoll/PianoRollKeys.svelte::.key', 'piano key'],
	['src/components/PianoRoll.svelte::.piano-roll--preview .grid-canvas', 'canvas'],
	['src/components/PianoRoll.svelte::.grid-scroll--preview', 'canvas'],
	['src/components/SamplerPads.svelte::.pad', 'drum pad'],
	['src/components/StepSlider.svelte::.label', 'label under a step slider'],
	['src/components/TimeRuler.svelte::.time-ruler', 'timeline ruler'],
	['src/components/Toggle.svelte::.toggle-switch', 'the switch itself (shared component)'],
	['src/components/Track.svelte::.lane-insert', 'floating insert handle on a track lane'],
	['src/components/TrackStrip.svelte::.color-bar', 'track color swatch'],
	['src/components/TrackStrip.svelte::.color-opt', 'track color swatch']
]);

function* svelteFiles(dir) {
	for (const entry of readdirSync(dir, { withFileTypes: true })) {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) yield* svelteFiles(path);
		else if (entry.name.endsWith('.svelte')) yield path;
	}
}

/** Rules of a stylesheet with their own declarations (nested rules excluded). */
function parseRules(style) {
	const rules = [];
	const stack = [];
	let buffer = '';
	for (const character of style) {
		if (character === '{') {
			const selector = buffer
				.replace(/^[\s\S]*<style[^>]*>/, '')
				.trim()
				.split(/[;}]/)
				.pop()
				.trim();
			stack.push({ selector: selector.replace(/\s+/g, ' '), own: '' });
			buffer = '';
		} else if (character === '}') {
			const rule = stack.pop();
			if (rule) {
				rule.own += buffer;
				rules.push(rule);
			}
			buffer = '';
		} else if (character === ';') {
			if (stack.length > 0) stack[stack.length - 1].own += `${buffer};`;
			buffer = '';
		} else {
			buffer += character;
		}
	}
	return rules;
}

const offences = [];
for (const file of svelteFiles(SRC)) {
	const path = relative(ROOT, file).replaceAll('\\', '/');
	const source = readFileSync(file, 'utf8');
	const styleStart = source.indexOf('<style');
	if (styleStart < 0) continue;

	for (const { selector, own } of parseRules(source.slice(styleStart))) {
		if (/cursor:\s*not-allowed/.test(own)) {
			offences.push(`${path}: "${selector}" uses cursor: not-allowed — use cursor: default`);
		}
		if (/(?:^|[;\s])accent-color\s*:/.test(own)) {
			offences.push(`${path}: "${selector}" uses accent-color — use @include range-slider`);
		}
		if (/::-(?:webkit|moz)-(?:slider|range)/.test(selector)) {
			offences.push(`${path}: "${selector}" styles a slider by hand — use @include range-slider`);
		}
		if (!/cursor:\s*pointer/.test(own) || CONTROL_MIXIN.test(own)) continue;
		if (CUSTOM_SURFACES.has(`${path}::${selector}`)) continue;
		offences.push(
			`${path}: "${selector}" is clickable but includes no mixin from src/controls.scss`
		);
	}
}

if (offences.length > 0) {
	console.error(
		`Hand-styled controls (${offences.length}) — see "Contrôles" in docs/claude/ui_conventions.md:\n`
	);
	console.error(offences.join('\n'));
	process.exit(1);
}
console.log('check-controls: every control uses the shared look.');
