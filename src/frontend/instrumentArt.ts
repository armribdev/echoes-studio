export type InstrumentArtKind =
	| 'keys'
	| 'synth'
	| 'drums'
	| 'bass'
	| 'guitar'
	| 'strings'
	| 'vocals'
	| 'pad'
	| 'fx';

const KIND_PATTERNS: { kind: InstrumentArtKind; re: RegExp }[] = [
	{ kind: 'drums', re: /drum|kit|perc|808|909|beat|rhythm/i },
	{ kind: 'bass', re: /bass|sub\b|808bass/i },
	{ kind: 'guitar', re: /guitar|gtr|strat|les\s*paul/i },
	{ kind: 'strings', re: /string|violin|cello|viola|orchestra|ensemble/i },
	{ kind: 'vocals', re: /vocal|voice|choir|singer|mic/i },
	{ kind: 'pad', re: /\bpad\b|ambient|atmosphere|texture/i },
	{ kind: 'keys', re: /piano|keys|rhodes|epiano|organ|clav|harpsi|wurl/i },
	{ kind: 'synth', re: /synth|polysynth|lead|analog|fm\b|wave|osc/i },
	{ kind: 'fx', re: /fx|effect|noise|glitch/i }
];

/** Déduit une vignette style GarageBand depuis le nom/id d'instrument. */
export function instrumentArtKind(
	instrumentId: string | null | undefined,
	displayName?: string | null
): InstrumentArtKind {
	if (!instrumentId) return 'synth';
	const haystack = `${instrumentId} ${displayName ?? ''}`;
	for (const { kind, re } of KIND_PATTERNS) {
		if (re.test(haystack)) return kind;
	}
	return 'synth';
}
