import { describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

import { blockedAnnouncement, type BlockedPlugin } from './plugins';

const crashed: BlockedPlugin = { path: 'C:/vst3/Broken.vst3', name: 'Broken', reason: 'crashed' };
const hung: BlockedPlugin = { path: 'C:/vst3/Slow.vst3', name: 'Slow', reason: 'timed_out' };

describe('blockedAnnouncement', () => {
	it('says nothing when no file was blocked', () => {
		expect(blockedAnnouncement([])).toBeNull();
	});

	it('names a plugin that crashed', () => {
		expect(blockedAnnouncement([crashed])).toEqual({
			key: 'blockedPlugins.announcedCrashed',
			values: { name: 'Broken' }
		});
	});

	it('names a plugin that stopped answering', () => {
		expect(blockedAnnouncement([hung])).toEqual({
			key: 'blockedPlugins.announcedTimedOut',
			values: { name: 'Slow' }
		});
	});

	it('gives one message with the count when several files were blocked', () => {
		expect(blockedAnnouncement([crashed, hung])).toEqual({
			key: 'blockedPlugins.announcedMany',
			values: { n: 2 }
		});
	});
});
