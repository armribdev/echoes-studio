import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { loadLibraryPersistence, saveLibraryPersistence } from './persistence';

function stubLocalStorage(initial?: unknown) {
	const store = new Map<string, string>();
	if (initial !== undefined) store.set('echoes.library.v1', JSON.stringify(initial));
	vi.stubGlobal('localStorage', {
		getItem: (key: string) => store.get(key) ?? null,
		setItem: (key: string, value: string) => void store.set(key, value)
	});
}

beforeEach(() => stubLocalStorage());
afterEach(() => vi.unstubAllGlobals());

describe('collections persistence', () => {
	it('round-trips a collection and reopens on it', () => {
		const data = loadLibraryPersistence();
		saveLibraryPersistence({
			...data,
			collections: [{ id: 'c1', name: 'Techno drums', query: 'techno', kind: 'audio' }],
			lastSource: 'collection:c1'
		});
		const loaded = loadLibraryPersistence();
		expect(loaded.collections).toEqual([
			{ id: 'c1', name: 'Techno drums', query: 'techno', kind: 'audio' }
		]);
		expect(loaded.lastSource).toBe('collection:c1');
	});

	it('falls back to audio when the last collection no longer exists', () => {
		stubLocalStorage({ lastSource: 'collection:gone', collections: [] });
		expect(loadLibraryPersistence().lastSource).toBe('audio');
	});

	it('drops collections without a name, without a criterion or with a duplicate id', () => {
		stubLocalStorage({
			collections: [
				{ id: 'a', name: 'Kept', query: '', kind: 'midi_pattern' },
				{ id: 'a', name: 'Duplicate id', query: 'x', kind: null },
				{ id: 'b', name: '  ', query: 'x', kind: null },
				{ id: 'c', name: 'No criterion', query: ' ', kind: 'unknown' },
				'garbage'
			]
		});
		expect(loadLibraryPersistence().collections.map((c) => c.id)).toEqual(['a']);
	});

	it('keeps only valid context sources', () => {
		stubLocalStorage({
			contextSources: {
				track_audio_lane: 'drums',
				track_midi_lane: 'collection:c1',
				track_empty: 'nonsense',
				not_a_context: 'audio'
			}
		});
		expect(loadLibraryPersistence().contextSources).toEqual({
			track_audio_lane: 'drums',
			track_midi_lane: 'collection:c1'
		});
	});

	it('keeps old saves without collections working', () => {
		stubLocalStorage({ favoriteIds: ['x'] });
		const loaded = loadLibraryPersistence();
		expect(loaded.collections).toEqual([]);
		expect(loaded.favoriteIds).toEqual(['x']);
	});
});
