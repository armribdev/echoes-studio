/** `Track.instrument` value selecting the built-in sampler (ADR-028). */
export const SAMPLER_INSTRUMENT_ID = 'builtin:sampler';

export function isSamplerInstrument(instrument: string | null | undefined): boolean {
	return instrument === SAMPLER_INSTRUMENT_ID;
}
