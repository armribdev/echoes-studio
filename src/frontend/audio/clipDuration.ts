import type { Clip } from '../../types/global';

/** Source-file length for trim / resize — prefer explicit duration, then peak lanes. */
export function clipSourceDurationSec(
	clip: Pick<Clip, 'sample_rate' | 'source_duration_sec' | 'peaks_64' | 'peaks_512' | 'peaks_4096'>
): number {
	if (clip.source_duration_sec != null && clip.source_duration_sec > 0) {
		return clip.source_duration_sec;
	}
	const sr = clip.sample_rate || 1;
	if (clip.peaks_64?.length) return (clip.peaks_64.length * 64) / sr;
	if (clip.peaks_512?.length) return (clip.peaks_512.length * 512) / sr;
	if (clip.peaks_4096?.length) return (clip.peaks_4096.length * 4096) / sr;
	return 0;
}
