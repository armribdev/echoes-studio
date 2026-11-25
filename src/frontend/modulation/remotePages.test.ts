import { describe, expect, it } from 'vitest';
import { normalisedValue, pageParams, withRemotePages } from './remotePages';
import type { PluginParamInfo } from './targets';

function param(native_id: number, module = 'Other'): PluginParamInfo {
	return {
		native_id,
		name: `P${native_id}`,
		module,
		min: 0,
		max: 1,
		default: 0,
		automatable: true,
		modulatable: true,
		hidden: false
	};
}

describe('withRemotePages', () => {
	const params = [param(1), param(2), param(3), param(4)];

	it('keeps the list as it is without any page', () => {
		expect(withRemotePages(params, [])).toBe(params);
	});

	it('puts the parameters of a page first, under its name, in the plugin order', () => {
		const result = withRemotePages(params, [
			{ section: 'Scene A', name: 'Filters', native_ids: [3, 1] }
		]);
		expect(result.map((p) => [p.native_id, p.module])).toEqual([
			[3, 'Scene A / Filters'],
			[1, 'Scene A / Filters'],
			[2, 'Other'],
			[4, 'Other']
		]);
	});

	it('does not repeat the section when the page name already starts with it', () => {
		const result = withRemotePages(params, [
			{ section: 'Scene A', name: 'Scene A Mixer', native_ids: [2] }
		]);
		expect(result[0].module).toBe('Scene A Mixer');
	});

	it('leaves a parameter on the first page that names it and ignores unknown ids', () => {
		const result = withRemotePages(params, [
			{ section: '', name: 'One', native_ids: [2, 99] },
			{ section: '', name: 'Two', native_ids: [2, 4] }
		]);
		expect(result.map((p) => [p.native_id, p.module])).toEqual([
			[2, 'One'],
			[4, 'Two'],
			[1, 'Other'],
			[3, 'Other']
		]);
	});

	it('keeps the list when no page names a known parameter', () => {
		expect(withRemotePages(params, [{ section: '', name: 'Ghost', native_ids: [99] }])).toBe(
			params
		);
	});
});

describe('pageParams', () => {
	const params = [param(1), param(2), param(3)];

	it('lists the parameters of a page in the plugin order and skips unknown ones', () => {
		const page = { section: '', name: 'P', native_ids: [3, 99, 1] };
		expect(pageParams(page, params).map((p) => p.native_id)).toEqual([3, 1]);
	});
});

describe('normalisedValue', () => {
	const cutoff = { ...param(1), min: 20, max: 20020 };

	it('places a plain value in the range of the parameter', () => {
		expect(normalisedValue(cutoff, 20)).toBe(0);
		expect(normalisedValue(cutoff, 10020)).toBe(0.5);
		expect(normalisedValue(cutoff, 20020)).toBe(1);
	});

	it('stays inside 0..1 and survives an empty range', () => {
		expect(normalisedValue(cutoff, -5)).toBe(0);
		expect(normalisedValue(cutoff, 99999)).toBe(1);
		expect(normalisedValue({ ...cutoff, max: 20 }, 20)).toBe(0);
	});
});
