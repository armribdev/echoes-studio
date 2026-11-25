#!/usr/bin/env node
/**
 * check-colors.mjs — fail when a UI color is written as a literal.
 *
 * Every color comes from a token of `src/variables.scss` (SCSS `$token` in
 * components, `themeColor()` / `themeVar()` from `src/frontend/theme.ts` in
 * canvases and scripts). Only the files below may hold literals: the token
 * file itself and the identity palettes whose hex values are persisted in
 * projects or are lookup tables.
 *
 * Detected: hex, rgb()/hsl() with numbers, hwb/lab/lch/oklab/oklch/color(),
 * and CSS named colors (`white`, `red`…) used as a color value.
 *
 * Usage: node scripts/check-colors.mjs
 */

import { readdirSync, readFileSync } from 'fs';
import { dirname, join, relative } from 'path';
import { fileURLToPath } from 'url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SRC = join(ROOT, 'src');

const ALLOWED_FILES = new Set([
	'src/variables.scss',
	'src/frontend/trackColor.ts',
	'src/frontend/modulation/colors.ts',
	'src/frontend/pluginFormatColor.ts'
]);

const EXTENSIONS = ['.svelte', '.scss', '.css', '.ts', '.html'];

const NAMED_COLORS = `
	aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue blueviolet
	brown burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan
	darkblue darkcyan darkgoldenrod darkgray darkgreen darkgrey darkkhaki darkmagenta
	darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen darkslateblue
	darkslategray darkslategrey darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey
	dodgerblue firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite gold goldenrod gray
	green greenyellow grey honeydew hotpink indianred indigo ivory khaki lavender lavenderblush
	lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen
	lightgrey lightpink lightsalmon lightseagreen lightskyblue lightslategray lightslategrey
	lightsteelblue lightyellow lime limegreen linen magenta maroon mediumaquamarine mediumblue
	mediumorchid mediumpurple mediumseagreen mediumslateblue mediumspringgreen mediumturquoise
	mediumvioletred midnightblue mintcream mistyrose moccasin navajowhite navy oldlace olive
	olivedrab orange orangered orchid palegoldenrod palegreen paleturquoise palevioletred
	papayawhip peachpuff peru pink plum powderblue purple rebeccapurple red rosybrown royalblue
	saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue slateblue slategray
	slategrey snow springgreen steelblue tan teal thistle tomato turquoise violet wheat white
	whitesmoke yellow yellowgreen
`
	.trim()
	.split(/\s+/)
	.join('|');

// A name only counts as a word of its own: not inside `$black-wash`, `--white` or `gray-scale`.
const NAMED = `(?<![\\w$#-])(?:${NAMED_COLORS})(?![\\w-])`;

const CSS_COLOR_PROPERTY = [
	'color',
	'background(?:-color|-image)?',
	'border(?:-[a-z]+)*',
	'outline(?:-color)?',
	'fill',
	'stroke',
	'stop-color',
	'flood-color',
	'box-shadow',
	'text-shadow',
	'caret-color',
	'accent-color',
	'filter',
	'text-decoration(?:-color)?',
	'column-rule(?:-color)?'
].join('|');

const SCRIPT_COLOR_PROPERTY =
	'fillStyle|strokeStyle|shadowColor|fill|stroke|color|backgroundColor|background|borderColor|outlineColor';

const QUOTE = '[\'"`]';

const LITERAL_COLOR = [
	/#[0-9a-fA-F]{3,8}\b/,
	/\b(?:rgba?|hsla?)\(\s*[\d.]/,
	/\b(?:hwb|lab|lch|oklab|oklch)\(/,
	/\bcolor\(\s*(?:srgb|display-p3|a98-rgb|rec2020|xyz)/,
	// CSS declaration: `color: white`, `border: 1px solid red`
	new RegExp(`(?:^|[{;])\\s*(?:${CSS_COLOR_PROPERTY})\\s*:[^;{}]*${NAMED}`),
	// color-mix() / gradient argument
	new RegExp(`(?:color-mix|gradient)\\([^)]*${NAMED}`),
	// attribute: `fill="white"`
	new RegExp(`\\b(?:fill|stroke|stop-color|flood-color|color)=["']${NAMED}["']`),
	// script: `ctx.fillStyle = 'red'`, `{ color: "white" }`
	new RegExp(`\\b(?:${SCRIPT_COLOR_PROPERTY})\\s*[:=]\\s*${QUOTE}${NAMED}${QUOTE}`)
];

function* sourceFiles(dir) {
	for (const entry of readdirSync(dir, { withFileTypes: true })) {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) yield* sourceFiles(path);
		else if (EXTENSIONS.some((extension) => entry.name.endsWith(extension))) yield path;
	}
}

const offences = [];
for (const file of sourceFiles(SRC)) {
	const relativePath = relative(ROOT, file).replaceAll('\\', '/');
	if (ALLOWED_FILES.has(relativePath) || relativePath.endsWith('.test.ts')) continue;
	readFileSync(file, 'utf8')
		.split('\n')
		.forEach((line, index) => {
			if (LITERAL_COLOR.some((pattern) => pattern.test(line))) {
				offences.push(`${relativePath}:${index + 1}: ${line.trim()}`);
			}
		});
}

if (offences.length > 0) {
	console.error(`Hardcoded colors (${offences.length}) — use a token from src/variables.scss:\n`);
	console.error(offences.join('\n'));
	process.exit(1);
}
console.log('check-colors: no hardcoded color.');
