import { describe, expect, it } from 'vitest';
import { hasSourceShape, isKnownSource, withoutSource } from './sources';
import { createCollection } from './collections';
import {
	idsOnlyInFolder,
	isInsideFolder,
	packAssetIds,
	packFolderOfSource,
	packLabel,
	packSourceId
} from './packs';

describe('pack source ids', () => {
	it('round-trips a folder path, drive letter included', () => {
		const source = packSourceId('C:\\Samples\\Kits');
		expect(packFolderOfSource(source)).toBe('C:\\Samples\\Kits');
	});

	it('does not read other sources as packs', () => {
		expect(packFolderOfSource('audio')).toBeNull();
		expect(packFolderOfSource('collection:abc')).toBeNull();
	});

	it('labels a pack with its folder name', () => {
		expect(packLabel('C:\\Samples\\Techno Kit')).toBe('Techno Kit');
		expect(packLabel('/home/me/loops/')).toBe('loops');
	});
});

describe('pack contents', () => {
	const ids = ['C:\\Kits\\a\\kick.wav', 'C:\\Kits\\b\\snare.wav', 'D:\\Other\\hat.wav'];

	it('lists the sounds inside the folder, whatever the separators and case', () => {
		expect(isInsideFolder('c:/kits/a/kick.wav', 'C:\\Kits')).toBe(true);
		expect(isInsideFolder('C:\\KitsExtra\\x.wav', 'C:\\Kits')).toBe(false);
		expect(packAssetIds(ids, 'C:\\Kits')).toEqual([ids[0], ids[1]]);
	});

	it('keeps a sound that another indexed folder still covers', () => {
		expect(idsOnlyInFolder(ids, 'C:\\Kits', ['C:\\Kits\\b'])).toEqual([ids[0]]);
		expect(idsOnlyInFolder(ids, 'C:\\Kits', [])).toEqual([ids[0], ids[1]]);
	});
});

describe('known sources', () => {
	const collection = createCollection('Techno', 'techno', null);
	const known = { collections: [collection], indexedFolders: ['C:\\Kits'] };

	it('accepts built-in sources, existing collections and indexed folders', () => {
		expect(isKnownSource('audio', known)).toBe(true);
		expect(isKnownSource(`collection:${collection.id}`, known)).toBe(true);
		expect(isKnownSource(packSourceId('C:\\Kits'), known)).toBe(true);
	});

	it('rejects a collection or a folder that is gone', () => {
		expect(isKnownSource('collection:gone', known)).toBe(false);
		expect(isKnownSource(packSourceId('C:\\Removed'), known)).toBe(false);
	});

	it('recognises the shape of every source id', () => {
		expect(hasSourceShape('drums')).toBe(true);
		expect(hasSourceShape('collection:x')).toBe(true);
		expect(hasSourceShape('pack:C:\\x')).toBe(true);
		expect(hasSourceShape('nonsense')).toBe(false);
	});

	it('forgets the contexts that pointed to a removed source', () => {
		const remembered = { a: 'audio', b: 'pack:C:\\Kits' } as const;
		expect(withoutSource(remembered, 'pack:C:\\Kits')).toEqual({ a: 'audio' });
	});
});
