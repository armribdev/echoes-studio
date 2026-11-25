import { describe, expect, it } from 'vitest';
import { filterAssetsByQuery } from './search';
import { MAX_TAG_LENGTH, mergeTags, normalizeTag, withoutTag, withTag } from './tags';
import { assetFromPath } from './types';

describe('normalizeTag', () => {
	it('trims, lowercases and collapses inner spaces', () => {
		expect(normalizeTag('  Dark   Techno ')).toBe('dark techno');
	});

	it('cuts a long tag without leaving a trailing space', () => {
		const tag = normalizeTag(`${'a'.repeat(MAX_TAG_LENGTH - 1)} bcd`);
		expect(tag).toBe('a'.repeat(MAX_TAG_LENGTH - 1));
	});
});

describe('withTag / withoutTag', () => {
	it('adds a new tag in its normalized form', () => {
		expect(withTag(['kick'], ' Sombre ')).toEqual(['kick', 'sombre']);
	});

	it('keeps the same array for an empty or duplicate tag', () => {
		const tags = ['kick'];
		expect(withTag(tags, '   ')).toBe(tags);
		expect(withTag(tags, 'KICK')).toBe(tags);
	});

	it('removes only the named tag', () => {
		expect(withoutTag(['kick', 'sombre'], 'kick')).toEqual(['sombre']);
	});
});

describe('mergeTags', () => {
	it('puts the user tags after the catalog tags, once each', () => {
		expect(mergeTags(['drums'], ['sombre', 'drums'])).toEqual(['drums', 'sombre']);
	});

	it('leaves an asset without any tag untagged', () => {
		expect(mergeTags(undefined, undefined)).toBeUndefined();
		expect(mergeTags(undefined, [])).toBeUndefined();
	});
});

describe('filterAssetsByQuery', () => {
	const kick = { ...assetFromPath('C:\\sounds\\kick_01.wav'), tags: ['sombre'] };
	const snare = assetFromPath('C:\\sounds\\snare_01.wav');

	it('finds an asset by a tag its name does not contain', () => {
		expect(filterAssetsByQuery([kick, snare], 'somb')).toEqual([kick]);
	});

	it('still finds an asset by name', () => {
		expect(filterAssetsByQuery([kick, snare], 'snare')).toEqual([snare]);
	});

	it('returns every asset for an empty query', () => {
		expect(filterAssetsByQuery([kick, snare], '  ')).toEqual([kick, snare]);
	});
});
