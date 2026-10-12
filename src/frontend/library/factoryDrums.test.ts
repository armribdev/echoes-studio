import { describe, expect, it } from 'vitest';
import { drumIdsWithSoundsFirst, factoryDrumAsset } from './factoryDrums';

describe('factoryDrumAsset', () => {
	it('lists a generated file as a factory one-shot tagged drums', () => {
		const asset = factoryDrumAsset({
			id: 'factory://drums/kick',
			name: 'Grosse caisse',
			note: 36,
			path: 'C:\\data\\factory_samples\\v1\\kick.wav',
			duration_sec: 0.4
		});
		expect(asset).toMatchObject({
			id: 'factory://drums/kick',
			kind: 'one_shot',
			extension: 'wav',
			durationSec: 0.4,
			origin: 'factory',
			tags: ['drums']
		});
	});
});

describe('drumIdsWithSoundsFirst', () => {
	it('puts the sounds before the MIDI patterns, keeping both orders', () => {
		const ids = drumIdsWithSoundsFirst(['factory://midi/a', 'factory://midi/b'], ['s1', 's2']);
		expect(ids).toEqual(['s1', 's2', 'factory://midi/a', 'factory://midi/b']);
	});

	it('does not list a sound twice when it is loaded again', () => {
		expect(drumIdsWithSoundsFirst(['s1', 's2', 'factory://midi/a'], ['s1', 's2'])).toEqual([
			's1',
			's2',
			'factory://midi/a'
		]);
	});
});
