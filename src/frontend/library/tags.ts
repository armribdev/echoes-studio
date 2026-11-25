export const MAX_TAG_LENGTH = 24;

export function normalizeTag(raw: string): string {
	return raw.trim().toLowerCase().replace(/\s+/g, ' ').slice(0, MAX_TAG_LENGTH).trim();
}

/** Returns `tags` itself when `raw` is empty or already present. */
export function withTag(tags: readonly string[], raw: string): readonly string[] {
	const tag = normalizeTag(raw);
	if (!tag || tags.includes(tag)) return tags;
	return [...tags, tag];
}

export function withoutTag(tags: readonly string[], tag: string): string[] {
	return tags.filter((existing) => existing !== tag);
}

export function mergeTags(
	catalogTags: readonly string[] | undefined,
	userTags: readonly string[] | undefined
): string[] | undefined {
	if (!userTags?.length) return catalogTags ? [...catalogTags] : undefined;
	return [...new Set([...(catalogTags ?? []), ...userTags])];
}
