import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import type { Track } from '../types/global';
import type { Preferences } from '../stores/preferences';
import { notify } from '../stores/status';

export async function exportMixdown(
	tracks: Track[],
	preferences: Preferences
): Promise<void> {
	let totalDuration = 0;
	for (const track of tracks) {
		for (const clip of track.clips) {
			totalDuration = Math.max(totalDuration, clip.end);
		}
	}
	if (totalDuration <= 0) {
		notify('Aucun clip à exporter', 'error');
		return;
	}

	const filePath = await save({
		filters: [{ name: 'Fichier WAV', extensions: ['wav'] }],
		defaultPath: 'mixdown.wav',
	});
	if (!filePath) return;

	notify('Export en cours…', 'info');

	const sampleRate = preferences.audio.sampleRate;
	const length = Math.ceil(totalDuration * sampleRate);
	const offCtx = new OfflineAudioContext(2, length, sampleRate);

	const anySolo = tracks.some((t) => t.solo);

	for (const track of tracks) {
		let effectiveGain = track.volume / 100;
		if (track.muted || (anySolo && !track.solo)) effectiveGain = 0;

		const gainNode = offCtx.createGain();
		gainNode.gain.value = effectiveGain;
		const panNode = offCtx.createStereoPanner();
		panNode.pan.value = track.pan / 100;
		gainNode.connect(panNode);
		panNode.connect(offCtx.destination);

		for (const clip of track.clips) {
			if (clip.raw_samples.length === 0) continue;

			const samples = new Float32Array(clip.raw_samples);
			let clipBuffer: AudioBuffer;

			if (clip.sample_rate === sampleRate) {
				clipBuffer = offCtx.createBuffer(1, samples.length, clip.sample_rate);
				clipBuffer.copyToChannel(samples, 0);
			} else {
				const duration = samples.length / clip.sample_rate;
				const resCtx = new OfflineAudioContext(
					1,
					Math.ceil(duration * sampleRate),
					sampleRate
				);
				const srcBuf = resCtx.createBuffer(1, samples.length, clip.sample_rate);
				srcBuf.copyToChannel(samples, 0);
				const srcNode = resCtx.createBufferSource();
				srcNode.buffer = srcBuf;
				srcNode.connect(resCtx.destination);
				srcNode.start();
				clipBuffer = await resCtx.startRendering();
			}

			const source = offCtx.createBufferSource();
			source.buffer = clipBuffer;
			source.connect(gainNode);
			source.start(clip.start);
		}
	}

	const rendered = await offCtx.startRendering();
	const samplesLeft = Array.from(rendered.getChannelData(0));
	const samplesRight = rendered.numberOfChannels > 1
		? Array.from(rendered.getChannelData(1))
		: samplesLeft;

	await invoke('export_wav', {
		path: filePath,
		samplesLeft,
		samplesRight,
		sampleRate,
		bitDepth: preferences.project.exportBitDepth,
	});

	const filename = (filePath as string).split(/[\\/]/).pop() ?? 'mixdown.wav';
	notify(`Exporté : ${filename}`, 'success');
}
