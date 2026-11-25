import { readFileSync } from 'fs';
import { describe, expect, it } from 'vitest';
import { THEME_TOKENS } from './theme';

describe('THEME_TOKENS', () => {
	it('lists exactly the tokens app.scss emits as --es-* custom properties', () => {
		const appStyles = readFileSync('src/app.scss', 'utf8');
		const mapBody = appStyles.match(/\$runtime-tokens:\s*\(([\s\S]*?)\n\);/)?.[1] ?? '';
		const emitted = [...mapBody.matchAll(/'([a-z0-9-]+)':/g)].map((match) => match[1]);

		expect([...THEME_TOKENS].sort()).toEqual(emitted.sort());
	});

	it('only names tokens that exist in variables.scss', () => {
		const variables = readFileSync('src/variables.scss', 'utf8');
		for (const token of THEME_TOKENS) {
			expect(variables).toContain(`$${token}:`);
		}
	});
});
