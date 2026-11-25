import { describe, expect, it } from 'vitest';
import {
	canSaveAsCollection,
	collectionForSource,
	collectionIdOfSource,
	collectionSourceId,
	createCollection,
	filterAssets,
	normalizeCollectionName,
	MAX_COLLECTION_NAME_LENGTH
} from './collections';
import type { LibraryAsset } from './types';

function asset(id: string, kind: LibraryAsset['kind'], tags: string[] = []): LibraryAsset {
	return { id, path: id, name: id, kind, extension: 'wav', tags };
}

describe('collection source ids', () => {
	it('round-trips a collection id', () => {
		const source = collectionSourceId('abc');
		expect(source).toBe('collection:abc');
		expect(collectionIdOfSource(source)).toBe('abc');
	});

	it('does not read a built-in source as a collection', () => {
		expect(collectionIdOfSource('audio')).toBeNull();
	});

	it('finds the collection a source points to', () => {
		const collection = createCollection('Techno', 'techno', null);
		expect(collectionForSource([collection], collectionSourceId(collection.id))).toBe(collection);
		expect(collectionForSource([collection], 'collection:missing')).toBeNull();
		expect(collectionForSource([collection], 'audio')).toBeNull();
	});
});

describe('createCollection', () => {
	it('trims the name and the query, and gives a unique id', () => {
		const a = createCollection('  Techno   drums ', ' techno ', 'audio');
		const b = createCollection('Techno drums', 'techno', 'audio');
		expect(a).toMatchObject({ name: 'Techno drums', query: 'techno', kind: 'audio' });
		expect(a.id).not.toBe(b.id);
	});

	it('caps the name length', () => {
		expect(normalizeCollectionName('x'.repeat(100))).toHaveLength(MAX_COLLECTION_NAME_LENGTH);
	});
});

describe('canSaveAsCollection', () => {
	it('needs a query or a kind', () => {
		expect(canSaveAsCollection('', null)).toBe(false);
		expect(canSaveAsCollection('   ', null)).toBe(false);
		expect(canSaveAsCollection('kick', null)).toBe(true);
		expect(canSaveAsCollection('', 'midi_pattern')).toBe(true);
	});
});

describe('filterAssets', () => {
	const assets = [
		asset('kick', 'one_shot', ['drums', 'techno']),
		asset('pad', 'loop', ['techno']),
		asset('arp', 'midi_pattern', ['techno']),
		asset('prog', 'chord_progression')
	];

	it('keeps everything without a criterion', () => {
		expect(filterAssets(assets, { query: '', kind: null })).toEqual(assets);
	});

	it('groups loops and one-shots under audio', () => {
		const ids = filterAssets(assets, { query: '', kind: 'audio' }).map((a) => a.id);
		expect(ids).toEqual(['kick', 'pad']);
	});

	it('combines the query (name or tag) with the kind', () => {
		const ids = filterAssets(assets, { query: 'techno', kind: 'audio' }).map((a) => a.id);
		expect(ids).toEqual(['kick', 'pad']);
		expect(filterAssets(assets, { query: 'techno', kind: 'chord_progression' })).toEqual([]);
	});
});
