<script lang="ts">
	import { preferences, settingsPanelOpen } from '../stores/preferences';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import type { Preferences } from '../stores/preferences';
	import { onMount, onDestroy } from 'svelte';
	import CustomSelect from './CustomSelect.svelte';
	import Toggle from './Toggle.svelte';
	import PillGroup from './PillGroup.svelte';
	import StepSlider from './StepSlider.svelte';
	import NumberInput from './NumberInput.svelte';
	import Button from './Button.svelte';
	import Icon from './Icon.svelte';
	import { listAudioDevices, applyAudioSettings, getSupportedSampleRates, getDeviceChannels, setActiveOutputChannels } from '../lib/audioBackend';
	import type { AudioDeviceInfo } from '../lib/audioBackend';
	import { listen } from '@tauri-apps/api/event';
	import type { UnlistenFn } from '@tauri-apps/api/event';

	// Routing state (local, not persisted yet)
	let masterRouting = 'out-1-2';
	let headphoneRouting = 'mirror';

	type Section = 'audio' | 'project' | 'interface';
	let activeSection: Section = 'audio';

	// Local draft — confirmed on close or explicit save
	let draft: Preferences = { ...$preferences, audio: { ...$preferences.audio }, project: { ...$preferences.project }, interface: { ...$preferences.interface } };

	const SAMPLE_RATES = [44100, 48000, 88200, 96000] as const;
	const BUFFER_SIZES = [64, 128, 256, 512, 1024, 2048] as const;
	const BIT_DEPTHS = [16, 24, 32] as const;

	$: latencyMs = draft.audio.bufferSize / draft.audio.sampleRate * 1000;

	// ── Device lists (fetched from Rust/CPAL on panel open) ──────────────────
	const DEFAULT_DEVICE: AudioDeviceInfo = { id: 'default', name: 'Par défaut (système)' };
	let outputDevices: AudioDeviceInfo[] = [DEFAULT_DEVICE];
	let inputDevices:  AudioDeviceInfo[] = [DEFAULT_DEVICE];
	let devicesLoading = false;
	let applyError: string | null = null;

	$: outputDeviceOptions = outputDevices.map(d => ({ value: d.id, label: d.name }));
	$: inputDeviceOptions  = inputDevices.map(d =>  ({ value: d.id, label: d.name }));

	async function loadDevices() {
		devicesLoading = true;
		try {
			const list = await listAudioDevices();
			outputDevices = list.outputs.length ? list.outputs : [DEFAULT_DEVICE];
			inputDevices  = list.inputs.length  ? list.inputs  : [DEFAULT_DEVICE];
		} finally {
			devicesLoading = false;
		}
	}

	// ── Supported sample rates for the selected output device ─────────────────
	// All rates are available by default until the async check resolves.
	let supportedSampleRates: Set<number> = new Set(SAMPLE_RATES);

	// ── Real device channel counts ────────────────────────────────────────────
	let realInputChannels  = $preferences.audio.inputChannels;
	let realOutputChannels = $preferences.audio.outputChannels;
	let channelsLoading = false;

	async function refreshInputChannels(deviceId: string) {
		realInputChannels = await getDeviceChannels(deviceId, true);
	}

	// Input device: only refresh channels (no stream restart needed yet).
	$: refreshInputChannels(draft.audio.inputDevice);

	// ── Live channel-routing preview ──────────────────────────────────────────
	// No stream restart — just an atomic write in the CPAL callback (~5 ms).
	$: void setActiveOutputChannels(
		Math.min(draft.audio.outputChannels, realOutputChannels)
	);

	// ── Live preview: output device change ───────────────────────────────────
	// Sequenced to avoid a double restart:
	//   1. Fetch supported rates + channel count in parallel (IPC)
	//   2. Auto-correct sample rate if not supported by new device
	//   3. Update UI (slider options, channel count)
	//   4. Single stream restart with all corrected values
	//
	// On Cancel the draft reverts → prevOutputDevice differs again →
	// applyDeviceChange is called with the original device → reverts cleanly.
	let prevOutputDevice = draft.audio.outputDevice;
	$: if (draft.audio.outputDevice !== prevOutputDevice) {
		const d = draft.audio.outputDevice;
		prevOutputDevice = d;
		void applyDeviceChange(d);
	}

	async function applyDeviceChange(deviceId: string) {
		applyError = null;
		channelsLoading = true;
		try {
			// Fetch rates and channel count in parallel.
			const [rates, outCh] = await Promise.all([
				getSupportedSampleRates(deviceId),
				getDeviceChannels(deviceId, false),
			]);

			// Update UI options.
			supportedSampleRates = new Set(rates.length ? rates : SAMPLE_RATES);
			realOutputChannels   = outCh;

			// Auto-correct sample rate if the current selection isn't supported.
			let targetSR = draft.audio.sampleRate;
			if (!supportedSampleRates.has(targetSR)) {
				targetSR = ([...supportedSampleRates] as (typeof draft.audio.sampleRate)[])
					.reduce((p, c) =>
						Math.abs(c - draft.audio.sampleRate) < Math.abs(p - draft.audio.sampleRate)
							? c : p
					);
				// Update prevSRBufKey BEFORE updating draft so the non-device
				// streamKey reactive below doesn't fire a duplicate restart.
				prevSRBufKey = `${targetSR}|${draft.audio.bufferSize}|${draft.audio.inputDevice}`;
				draft = { ...draft, audio: { ...draft.audio, sampleRate: targetSR } };
			}

			// Single stream restart with the fully-corrected settings.
			await applyAudioSettings({
				outputDevice:   deviceId,
				inputDevice:    draft.audio.inputDevice,
				sampleRate:     targetSR,
				bufferSize:     draft.audio.bufferSize,
				outputChannels: Math.min(draft.audio.outputChannels, realOutputChannels),
				masterSoftClip: draft.audio.masterSoftClip,
			});
		} catch (err) {
			applyError = typeof err === 'string' ? err : 'Impossible d\'appliquer les paramètres audio.';
		} finally {
			channelsLoading = false;
		}
	}

	// ── Live preview: sample rate / buffer size / input device changes ─────────
	// Device changes are handled above. This reactive only fires for the
	// remaining stream-restart params so we never get a double restart.
	let prevSRBufKey = `${draft.audio.sampleRate}|${draft.audio.bufferSize}|${draft.audio.inputDevice}`;
	$: {
		const key = `${draft.audio.sampleRate}|${draft.audio.bufferSize}|${draft.audio.inputDevice}`;
		if (key !== prevSRBufKey) {
			prevSRBufKey = key;
			void liveApplyStreamSettings();
		}
	}

	async function liveApplyStreamSettings() {
		applyError = null;
		try {
			await applyAudioSettings({
				outputDevice:   draft.audio.outputDevice,
				inputDevice:    draft.audio.inputDevice,
				sampleRate:     draft.audio.sampleRate,
				bufferSize:     draft.audio.bufferSize,
				outputChannels: Math.min(draft.audio.outputChannels, realOutputChannels),
				masterSoftClip: draft.audio.masterSoftClip,
			});
		} catch (err) {
			applyError = typeof err === 'string' ? err : 'Impossible d\'appliquer les paramètres audio.';
		}
	}

	// Slider options — only include rates the device actually supports.
	// Unsupported rates are simply absent from the slider positions.
	$: sampleRateSliderOptions = SAMPLE_RATES
		.filter(sr => supportedSampleRates.has(sr))
		.map(sr => ({
			value: sr,
			label: sr >= 1000 ? (sr / 1000) + ' kHz' : sr + ' Hz',
		}));

	// Buffer size slider options (always all 6 — fixed-buffer failures are
	// handled at stream build time, not at enumeration time).
	const bufferSliderOptions = BUFFER_SIZES.map(bs => ({
		value: bs,
		label: String(bs),
	}));

	// ── Persistence + backend application ────────────────────────────────────

	async function applyAndClose() {
		applyError = null;

		// Push audio settings to CPAL engine (restarts stream if needed)
		try {
			await applyAudioSettings({
				outputDevice:    draft.audio.outputDevice,
				inputDevice:     draft.audio.inputDevice,
				sampleRate:      draft.audio.sampleRate,
				bufferSize:      draft.audio.bufferSize,
				outputChannels:  Math.min(draft.audio.outputChannels, realOutputChannels),
				masterSoftClip:  draft.audio.masterSoftClip,
			});
		} catch (err) {
			applyError = typeof err === 'string' ? err : 'Impossible d\'appliquer les paramètres audio.';
			// Still save preferences so settings survive restart
		}

		preferences.set(draft);
		settingsPanelOpen.set(false);
		keyboardFocus.setTimeline();
	}

	function cancelAndClose() {
		applyError = null;
		draft = { ...$preferences, audio: { ...$preferences.audio }, project: { ...$preferences.project }, interface: { ...$preferences.interface } };
		// Resetting prevOutputDevice to the current (not original) value means
		// the $: device reactive sees a change from current → original and fires
		// applyDeviceChange with the original device → clean revert.
		// (prevSRBufKey is left as-is; liveApplyStreamSettings handles the rest.)
		settingsPanelOpen.set(false);
		keyboardFocus.setTimeline();
	}

	function resetSection() {
		if (activeSection === 'audio') draft.audio = { inputDevice: 'default', outputDevice: 'default', sampleRate: 48000, bufferSize: 256, inputChannels: 2, outputChannels: 2, masterSoftClip: false };
		else if (activeSection === 'project') draft.project = { defaultFolder: '', autoSave: true, autoSaveIntervalSec: 60, autoSaveMaxBackups: 5, defaultBpm: 120, defaultNumerator: 4, defaultDenominator: 4, exportBitDepth: 32, exportMp3Bitrate: 320 };
		else if (activeSection === 'interface') draft.interface = { showMinimap: false };
		draft = { ...draft };
	}

	// ── Channel level meters ──────────────────────────────────────────────────
	// Driven by LEVELS_UPDATED engine events emitted ~30 fps by the Rust reporter.
	// `rawLevels` are updated instantly from events; `displayLevels` decay via rAF
	// so the meter has a fast attack (immediate snap-up) and slow release (~400 ms).

	let rawOutputLevels:     number[] = Array(16).fill(0);
	let rawInputLevels:      number[] = Array(16).fill(0);
	let xrunCount = 0;
	let displayOutputLevels: number[] = Array(16).fill(0);
	let displayInputLevels:  number[] = Array(16).fill(0);

	let unlistenLevels: UnlistenFn | null = null;
	let rafId = 0;
	let rafRunning = false;

	// Decay factor per animation frame (~60 fps). 0.93^18 ≈ 0.27, so a full
	// signal drops to ~27% in 300 ms — a typical VU release time.
	const DECAY = 0.93;

	function decayLoop() {
		let changed = false;
		for (let i = 0; i < 16; i++) {
			// Attack: snap up instantly.
			const newOut = rawOutputLevels[i] > displayOutputLevels[i]
				? rawOutputLevels[i]
				: displayOutputLevels[i] * DECAY;
			const newIn  = rawInputLevels[i] > displayInputLevels[i]
				? rawInputLevels[i]
				: displayInputLevels[i] * DECAY;

			if (Math.abs(newOut - displayOutputLevels[i]) > 0.0001) changed = true;
			if (Math.abs(newIn  - displayInputLevels[i])  > 0.0001) changed = true;

			displayOutputLevels[i] = newOut;
			displayInputLevels[i]  = newIn;
		}
		// Only trigger Svelte reactivity when something actually changed.
		if (changed) {
			displayOutputLevels = displayOutputLevels;
			displayInputLevels  = displayInputLevels;
		}
		// Continue only while the panel is open — stop to save CPU otherwise.
		if (rafRunning) rafId = requestAnimationFrame(decayLoop);
	}

	// Start / stop the rAF loop in sync with panel visibility.
	$: if ($settingsPanelOpen) {
		keyboardFocus.setSettings();
		if (!rafRunning) {
			rafRunning = true;
			rafId = requestAnimationFrame(decayLoop);
		}
	} else {
		rafRunning = false;
		cancelAnimationFrame(rafId);
		// Reset display so meters open fresh next time.
		displayOutputLevels = Array(16).fill(0);
		displayInputLevels  = Array(16).fill(0);
		xrunCount = 0;
	}

	/**
	 * Map a linear amplitude peak to a CSS height percentage using a dB scale.
	 * −60 dB → 0 %   |   0 dB → 100 %   |   above 0 dB is clamped to 100 %.
	 */
	function levelToHeight(amp: number): string {
		if (amp <= 0) return '0%';
		const db = 20 * Math.log10(amp);
		const pct = Math.max(0, Math.min(100, (db + 60) / 60 * 100));
		return `${pct.toFixed(1)}%`;
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') cancelAndClose();
	}

	onMount(async () => {
		window.addEventListener('keydown', handleKeydown);
		loadDevices();

		unlistenLevels = await listen<{
			type: string;
			payload: { output: number[]; input: number[]; position?: number; xruns?: number };
		}>('engine-event', (event) => {
			const ev = event.payload;
			// Levels are piggybacked on POSITION_UPDATED while playing,
			// and sent as a standalone LEVELS_UPDATED event when stopped.
			if (ev?.type === 'LEVELS_UPDATED' || ev?.type === 'POSITION_UPDATED') {
				if (ev.payload.output) rawOutputLevels = ev.payload.output;
				if (ev.payload.input)  rawInputLevels  = ev.payload.input;
				if (typeof ev.payload.xruns === 'number') xrunCount = ev.payload.xruns;
			}
		});
	});
	onDestroy(() => {
		window.removeEventListener('keydown', handleKeydown);
		cancelAnimationFrame(rafId);
		unlistenLevels?.();
	});

	const SECTIONS: { id: Section; label: string }[] = [
		{ id: 'audio',     label: 'Audio'     },
		{ id: 'project',   label: 'Par défaut' },
		{ id: 'interface', label: 'Interface' }
	];
</script>

{#if $settingsPanelOpen}
<!-- svelte-ignore a11y-click-events-have-key-events -->
<!-- svelte-ignore a11y-no-static-element-interactions -->
<div class="overlay" on:click|self={cancelAndClose}>
	<div class="panel" role="dialog" aria-modal="true" aria-label="Préférences">

		<!-- ── Header ── -->
		<div class="panel-header">
			<span class="header-icon"><Icon name="settings" size={14} /></span>
			<span class="panel-title">Préférences</span>
			<button class="close-btn" on:click={cancelAndClose} title="Fermer (Échap)">
				<Icon name="close" size={15} strokeWidth={1.5} />
			</button>
		</div>

		<!-- ── Body ── -->
		<div class="panel-body">

			<!-- Sidebar nav -->
			<nav class="sidebar">
				{#each SECTIONS as s}
					<button
						class="nav-item"
						class:active={activeSection === s.id}
						on:click={() => (activeSection = s.id)}
					>
						<span class="nav-icon">
							{#if s.id === 'audio'}
								<Icon name="volume" size={15} />
							{:else if s.id === 'project'}
								<Icon name="folder" size={15} />
							{:else if s.id === 'interface'}
								<Icon name="monitor" size={15} />
							{/if}
						</span>
						<span class="nav-label">{s.label}</span>
					</button>
				{/each}
			</nav>

			<!-- Content -->
			<div class="content">

			{#if activeSection === 'audio'}
			<div class="section">
				<h2 class="section-title">Périphériques Audio</h2>

				<!-- ── Deux colonnes IN / OUT ─────────────────────────────────── -->
				<div class="io-grid">

					<!-- Colonne Entrée -->
					<div class="io-col">
						<div class="io-device-row">
							<span class="io-device-label">
								Entrée
								<span class="badge badge--in">IN</span>
								{#if devicesLoading}<span class="loading-dot">…</span>{/if}
							</span>
							<CustomSelect bind:value={draft.audio.inputDevice} options={inputDeviceOptions} />
							<span class="field-hint">Microphone, interface, entrée virtuelle</span>
						</div>

						<div class="channel-card channel-card--in">
							<div class="channel-card-header">
								<span class="channel-card-title">Canaux actifs</span>
								<div class="channel-count-control">
									<button class="count-btn"
										disabled={draft.audio.inputChannels <= 1}
										on:click={() => draft.audio.inputChannels = Math.max(1, draft.audio.inputChannels - 1)}>−</button>
									<span class="count-value">{Math.min(draft.audio.inputChannels, realInputChannels)}</span>
									<button class="count-btn"
										disabled={draft.audio.inputChannels >= realInputChannels}
										on:click={() => draft.audio.inputChannels = Math.min(realInputChannels, draft.audio.inputChannels + 1)}>+</button>
								</div>
							</div>
							<div class="channel-strips">
								{#each Array(Math.min(realInputChannels, 16)) as _, i}
									<div
										class="channel-strip channel-strip--in"
										class:channel-strip--inactive={i >= Math.min(draft.audio.inputChannels, realInputChannels)}
										title="Entrée {i + 1}"
									>
										<span class="strip-num">{i + 1}</span>
										<div class="strip-meter">
											<div
												class="strip-level strip-level--in"
												style="height: {levelToHeight(displayInputLevels[i] ?? 0)}"
											></div>
										</div>
									</div>
								{/each}
							</div>
						</div>
					</div>

					<!-- Flèche centrale -->
					<div class="channel-route-arrow">
						<Icon name="arrow-right" size={14} />
					</div>

					<!-- Colonne Sortie -->
					<div class="io-col">
						<div class="io-device-row">
							<span class="io-device-label">
								Sortie
								<span class="badge badge--out">OUT</span>
								{#if devicesLoading}<span class="loading-dot">…</span>{/if}
							</span>
							<CustomSelect bind:value={draft.audio.outputDevice} options={outputDeviceOptions} />
							<span class="field-hint">Haut-parleurs, casque, interface de sortie</span>
						</div>

						<div class="channel-card channel-card--out">
							<div class="channel-card-header">
								<span class="channel-card-title">Canaux actifs</span>
								{#if channelsLoading}<span class="loading-dot">…</span>{/if}
								<div class="channel-count-control">
									<button class="count-btn"
										disabled={draft.audio.outputChannels <= 1}
										on:click={() => draft.audio.outputChannels = Math.max(1, draft.audio.outputChannels - 1)}>−</button>
									<span class="count-value">{Math.min(draft.audio.outputChannels, realOutputChannels)}</span>
									<button class="count-btn"
										disabled={draft.audio.outputChannels >= realOutputChannels}
										on:click={() => draft.audio.outputChannels = Math.min(realOutputChannels, draft.audio.outputChannels + 1)}>+</button>
								</div>
							</div>
							<div class="channel-strips">
								{#each Array(Math.min(realOutputChannels, 16)) as _, i}
									<div
										class="channel-strip channel-strip--out"
										class:channel-strip--inactive={i >= Math.min(draft.audio.outputChannels, realOutputChannels)}
										title="Sortie {i + 1}"
									>
										<span class="strip-num">{i + 1}</span>
										<div class="strip-meter">
											<div
												class="strip-level strip-level--out"
												style="height: {levelToHeight(displayOutputLevels[i] ?? 0)}"
											></div>
										</div>
									</div>
								{/each}
							</div>
						</div>
					</div>

				</div><!-- /io-grid -->

				{#if applyError}
				<div class="apply-error">
					<Icon name="alert-triangle" size={13} />
					<span>{applyError}</span>
				</div>
				{/if}

				<div class="divider"></div>
				<h3 class="subsection-title">Qualité & Latence</h3>

					<div class="field-grid">
						<div class="field">
							<span class="field-label">Fréquence d'échantillonnage</span>
							{#if sampleRateSliderOptions.length <= 1}
								<div class="single-value-display">
									{sampleRateSliderOptions[0]?.label ?? (draft.audio.sampleRate >= 1000 ? draft.audio.sampleRate / 1000 + ' kHz' : draft.audio.sampleRate + ' Hz')}
								</div>
								<span class="field-hint">Seule fréquence supportée par ce périphérique</span>
							{:else}
								<StepSlider
									bind:value={draft.audio.sampleRate}
									options={sampleRateSliderOptions}
								/>
							{/if}
						</div>

						<div class="field">
							<span class="field-label">Taille de buffer</span>
							<StepSlider
								bind:value={draft.audio.bufferSize}
								options={bufferSliderOptions}
							/>
						</div>

						<div class="field">
							<span class="field-label">Bit depth</span>
							<div class="single-value-display">
								<Icon name="cpu" size={14} />
								32-bit float
							</div>
							<span class="field-hint">Profondeur maximale sans conversion</span>
						</div>

					</div>

					<div class="latency-bar">
						<span class="latency-label">Latence estimée</span>
						<span class="latency-value">{latencyMs.toFixed(2)} ms</span>
						<div class="latency-indicator">
							<div
								class="latency-fill"
								class:latency-low={latencyMs < 10}
								class:latency-mid={latencyMs >= 10 && latencyMs < 25}
								class:latency-high={latencyMs >= 25}
								style="width: {Math.min(latencyMs / 50 * 100, 100)}%"
							></div>
						</div>
						<span class="latency-hint">
							{#if latencyMs < 10}Excellente — idéale pour l'enregistrement live
							{:else if latencyMs < 25}Bonne — confortable pour la production
							{:else}Élevée — envisager un buffer plus petit{/if}
						</span>
					</div>

					<div class="xrun-bar" class:xrun-warning={xrunCount > 0}>
						<span class="xrun-label">Underruns (XRun)</span>
						<span class="xrun-value">{xrunCount}</span>
						<span class="xrun-hint">
							{#if xrunCount === 0}Aucun underrun détecté sur cette session
							{:else}Le callback audio a été en retard — augmenter le buffer si cela persiste{/if}
						</span>
					</div>

				<div class="divider"></div>
				<h3 class="subsection-title">Sortie master</h3>

				<Toggle
					bind:checked={draft.audio.masterSoftClip}
					label="Limiteur master (soft clip)"
					description="Désactivé : sortie linéaire. Activé : saturation douce tanh sur le signal sommé."
				/>

				<div class="divider"></div>
				<h3 class="subsection-title">Routage Master</h3>

					<div class="routing-row">
						<div class="routing-label">
							<span class="routing-name">Master L/R</span>
							<span class="routing-desc">Sortie principale stéréo</span>
						</div>
						<div class="routing-assign">
							<CustomSelect
								small
								bind:value={masterRouting}
						options={[
								{ value: 'out-1-2', label: 'Sorties 1–2' },
								...(Math.min(draft.audio.outputChannels, realOutputChannels) >= 4 ? [{ value: 'out-3-4', label: 'Sorties 3–4' }] : []),
								...(Math.min(draft.audio.outputChannels, realOutputChannels) >= 6 ? [{ value: 'out-5-6', label: 'Sorties 5–6' }] : [])
							]}
							/>
						</div>
					</div>

					<div class="routing-row">
						<div class="routing-label">
							<span class="routing-name">Casque</span>
							<span class="routing-desc">Mix monitoring indépendant</span>
						</div>
						<div class="routing-assign">
							<CustomSelect
								small
								bind:value={headphoneRouting}
						options={[
								{ value: 'mirror', label: 'Miroir Master' },
								...(Math.min(draft.audio.outputChannels, realOutputChannels) >= 4 ? [{ value: 'out-3-4', label: 'Sorties 3–4' }] : []),
								...(Math.min(draft.audio.outputChannels, realOutputChannels) >= 6 ? [{ value: 'out-5-6', label: 'Sorties 5–6' }] : [])
							]}
							/>
						</div>
					</div>
				</div>

				{:else if activeSection === 'project'}
				<div class="section">
					<h2 class="section-title">Projets & sauvegarde (globaux)</h2>

					<div class="field">
						<span class="field-label">Dossier de projets par défaut</span>
						<div class="field-row">
							<input class="text-input" type="text" bind:value={draft.project.defaultFolder} placeholder="~/Documents/Echoes Studio" />
							<Button compact on:click={() => {}}>Parcourir…</Button>
						</div>
					</div>

					<div class="divider"></div>
					<h3 class="subsection-title">Sauvegarde automatique</h3>

					<Toggle
						bind:checked={draft.project.autoSave}
						label="Sauvegarde automatique"
						description="Enregistre le projet à intervalles réguliers"
					/>

					{#if draft.project.autoSave}
					<div class="field-grid" style="margin-top: 0">
						<div class="field">
							<span class="field-label">Intervalle</span>
							<NumberInput bind:value={draft.project.autoSaveIntervalSec} min={10} max={600} unit="sec" />
						</div>
						<div class="field">
							<span class="field-label">Sauvegardes conservées</span>
							<NumberInput bind:value={draft.project.autoSaveMaxBackups} min={1} max={20} unit="max" />
						</div>
					</div>
					{/if}

					<div class="divider"></div>
					<h3 class="subsection-title">Export Mixdown</h3>

					<div class="field">
						<span class="field-label">Profondeur de bits WAV</span>
						<PillGroup
							bind:value={draft.project.exportBitDepth}
							options={[
								{ value: 16, label: '16 bit' },
								{ value: 24, label: '24 bit' },
								{ value: 32, label: '32-bit float' },
							]}
						/>
						<span class="field-hint">
							{#if draft.project.exportBitDepth === 16}CD / streaming — compatible partout, taille réduite
							{:else if draft.project.exportBitDepth === 24}Standard pro — livraison mastering et plateformes
							{:else}Archival / stems — préserve la plage dynamique float intacte{/if}
						</span>
					</div>

					<div class="field">
						<span class="field-label">Débit MP3</span>
						<PillGroup
							bind:value={draft.project.exportMp3Bitrate}
							options={[
								{ value: 128, label: '128 kbps' },
								{ value: 192, label: '192 kbps' },
								{ value: 320, label: '320 kbps' },
							]}
						/>
						<span class="field-hint">
							{#if draft.project.exportMp3Bitrate === 128}Streaming léger — ~1 Mo/min
							{:else if draft.project.exportMp3Bitrate === 192}Haute qualité — distribution recommandée
							{:else}Qualité maximale — archivage et livraison pro{/if}
						</span>
					</div>

					<div class="divider"></div>
					<h3 class="subsection-title">Nouveau projet — Défauts</h3>

					<div class="field-grid">
						<div class="field">
							<span class="field-label">BPM</span>
							<NumberInput bind:value={draft.project.defaultBpm} min={20} max={400} unit="BPM" />
						</div>
						<div class="field">
							<span class="field-label">Mesure</span>
							<div class="field-row">
								<NumberInput bind:value={draft.project.defaultNumerator} min={1} max={16} width="42px" />
								<span class="field-slash">/</span>
								<CustomSelect
									small
									bind:value={draft.project.defaultDenominator}
									options={[
										{ value: 2, label: '2' },
										{ value: 4, label: '4' },
										{ value: 8, label: '8' },
										{ value: 16, label: '16' }
									]}
								/>
							</div>
						</div>
					</div>
				</div>

			{:else if activeSection === 'interface'}
			<div class="section">
				<h2 class="section-title">Interface</h2>

				<Toggle
					bind:checked={draft.interface.showMinimap}
					label="Mini-carte de la timeline"
					description="Vue d'ensemble navigable au-dessus de la timeline"
				/>
			</div>
				{/if}

			</div>
		</div>

		<!-- ── Footer ── -->
		<div class="panel-footer">
			<Button variant="ghost" on:click={resetSection} title="Réinitialiser cette section">Réinitialiser</Button>
			<div class="footer-actions">
				<Button variant="secondary" on:click={cancelAndClose}>Annuler</Button>
				<Button variant="primary" on:click={applyAndClose}>Enregistrer</Button>
			</div>
		</div>

	</div>
</div>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

	// ── Overlay ────────────────────────────────────────────────────────────────
	.overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.65);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 1000;
		backdrop-filter: blur(4px);
	}

	// ── Panel shell ───────────────────────────────────────────────────────────
	.panel {
		width: 720px;
		max-width: calc(100vw - 48px);
		height: 620px;
		max-height: calc(100vh - 80px);
		background: #1e1e24;
		border: 1px solid $border-color;
		border-radius: $radius-xl;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		box-shadow: 0 24px 64px rgba(0, 0, 0, 0.7), 0 0 0 0.5px rgba(255,255,255,0.06);
	}

	// ── Header ────────────────────────────────────────────────────────────────
	.panel-header {
		display: flex;
		align-items: center;
		padding: 14px 18px;
		border-bottom: 1px solid $border-color;
		background: $panel-background-color;
		flex-shrink: 0;
		gap: 9px;
	}

	.header-icon {
		color: rgba(247, 247, 249, 0.35);
		flex-shrink: 0;
		margin-right: 2px;
	}

	.panel-title {
		font-size: 13px;
		font-weight: 600;
		color: $text-color;
		letter-spacing: 0.2px;
		flex: 1;
	}

	.close-btn {
		background: none;
		border: none;
		color: rgba(247, 247, 249, 0.4);
		font-size: 14px;
		width: 26px;
		height: 26px;
		border-radius: 4px;
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition: background 0.1s, color 0.1s;

		&:hover {
			background: rgba(255, 255, 255, 0.1);
			color: $text-color;
		}
	}

	// ── Body (sidebar + content) ───────────────────────────────────────────────
	.panel-body {
		display: flex;
		flex: 1;
		overflow: hidden;
	}

	// ── Sidebar ───────────────────────────────────────────────────────────────
	.sidebar {
		width: 130px;
		flex-shrink: 0;
		background: $background-color;
		border-right: 1px solid $border-color;
		padding: 10px 6px;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.nav-item {
		display: flex;
		align-items: center;
		gap: 9px;
		padding: 8px 10px;
		border: none;
		background: none;
		border-radius: 6px;
		cursor: pointer;
		color: rgba(247, 247, 249, 0.5);
		font-size: 12px;
		text-align: left;
		transition: background 0.12s, color 0.12s;
		width: 100%;

		&:hover {
			background: rgba(255, 255, 255, 0.06);
			color: rgba(247, 247, 249, 0.85);
		}

		&:hover .nav-icon { opacity: 0.85; }

		&.active {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;

			.nav-icon { opacity: 1; }
		}
	}

	.nav-icon {
		display: flex;
		align-items: center;
		justify-content: center;
		opacity: 0.6;
		flex-shrink: 0;
		transition: opacity 0.12s;
	}

	.nav-label {
		font-weight: 500;
	}

	// ── Content ───────────────────────────────────────────────────────────────
	.content {
		flex: 1;
		overflow-y: auto;
		padding: 20px 22px;

		&::-webkit-scrollbar {
			width: 5px;
		}

		&::-webkit-scrollbar-thumb {
			background: rgba(255,255,255,0.12);
			border-radius: 3px;
		}
	}

	.section {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.section-title {
		font-size: 14px;
		font-weight: 600;
		color: $text-color;
		margin: 0 0 4px;
		letter-spacing: 0.1px;
	}

	.subsection-title {
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.8px;
		color: rgba(247, 247, 249, 0.35);
		margin: 0;
	}

	.divider {
		height: 1px;
		background: $border-color;
		margin: 8px 0;
	}

	// ── Field ─────────────────────────────────────────────────────────────────
	.field {
		display: flex;
		flex-direction: column;
		gap: 5px;
	}


	.field-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
		gap: 16px;
		margin-top: 4px;
	}

	.field-label {
		line-height: 16px;
		font-size: 11px;
		color: rgba(247, 247, 249, 0.55);
		font-weight: 500;
		letter-spacing: 0.3px;
	}

	.field-row {
		display: flex;
		align-items: center;
		gap: 7px;
	}

	.field-hint {
		line-height: 1.2;
		font-size: 10px;
		color: rgba(247, 247, 249, 0.28);
	}

	// ── Badge ─────────────────────────────────────────────────────────────────
	.badge {
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.8px;
		padding: 2px 5px;
		border-radius: 3px;

		&--in {
			background: rgba(0, 200, 120, 0.15);
			color: #00c878;
			border: 1px solid rgba(0, 200, 120, 0.3);
		}

		&--out {
			background: rgba(21, 113, 255, 0.15);
			color: #5599ff;
			border: 1px solid rgba(21, 113, 255, 0.3);
		}
	}

	// ── Latency bar ───────────────────────────────────────────────────────────
	.latency-bar {
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.07);
		border-radius: 7px;
		padding: 12px 14px;
		display: grid;
		grid-template-columns: 1fr auto;
		grid-template-rows: auto auto auto;
		gap: 6px 8px;
		align-items: center;
	}

	.latency-label {
		font-size: 11px;
		color: rgba(247, 247, 249, 0.45);
		grid-column: 1;
		grid-row: 1;
	}

	.latency-value {
		font-size: 13px;
		font-weight: 600;
		font-family: $font-mono;
		color: $text-color;
		grid-column: 2;
		grid-row: 1;
		text-align: right;
	}

	.latency-indicator {
		grid-column: 1 / -1;
		grid-row: 2;
		height: 4px;
		background: rgba(255, 255, 255, 0.07);
		border-radius: 2px;
		overflow: hidden;
	}

	.latency-fill {
		height: 100%;
		border-radius: 2px;
		transition: width 0.3s, background-color 0.3s;

		&.latency-low { background: $color-success; }
		&.latency-mid { background: $color-warning; }
		&.latency-high { background: $color-error; }
	}

	.latency-hint {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.3);
		grid-column: 1 / -1;
		grid-row: 3;
	}

	.xrun-bar {
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.07);
		border-radius: 7px;
		padding: 12px 14px;
		display: grid;
		grid-template-columns: 1fr auto;
		grid-template-rows: auto auto;
		gap: 6px 8px;
		align-items: center;

		&.xrun-warning {
			border-color: rgba(255, 120, 80, 0.35);
			background: rgba(255, 80, 40, 0.06);
		}
	}

	.xrun-label {
		font-size: 11px;
		color: rgba(247, 247, 249, 0.45);
		grid-column: 1;
		grid-row: 1;
	}

	.xrun-value {
		font-size: 13px;
		font-weight: 600;
		font-family: $font-mono;
		color: $text-color;
		grid-column: 2;
		grid-row: 1;
		text-align: right;

		.xrun-warning & {
			color: $color-warning;
		}
	}

	.xrun-hint {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.3);
		grid-column: 1 / -1;
		grid-row: 2;

		.xrun-warning & {
			color: rgba(255, 180, 120, 0.75);
		}
	}


	// ── IO two-column grid ────────────────────────────────────────────────────
	.io-grid {
		display: grid;
		grid-template-columns: 1fr auto 1fr;
		gap: 10px;
		align-items: start;
	}

	.io-col {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-width: 0;
	}

	.io-device-row {
		display: flex;
		flex-direction: column;
		gap: 5px;
	}

	.io-device-label {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 11px;
		color: rgba(247, 247, 249, 0.55);
		font-weight: 500;
		letter-spacing: 0.3px;
	}

	// ── Channel cards ─────────────────────────────────────────────────────────
	.channel-card {
		background: $surface-1;
		border: 1px solid $border-faint;
		border-radius: $radius-lg;
		padding: 12px;
		display: flex;
		flex-direction: column;
		gap: 10px;

		&--in  { border-top: 2px solid rgba(0, 200, 120, 0.35); }
		&--out { border-top: 2px solid rgba(21, 113, 255, 0.35); }
	}

	.channel-card-header {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.channel-card-title {
		font-size: 11px;
		font-weight: 500;
		color: rgba(247, 247, 249, 0.45);
	}

	.channel-count-control {
		display: flex;
		gap: 8px;
	}

	.count-btn {
		width: 20px;
		height: 20px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: $surface-2;
		border-radius: $radius-md;
		color: $text-color;
		font-size: 12px;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: background 0.1s, border-color 0.1s;

		&:hover:not(:disabled) {
			background: rgba(255, 255, 255, 0.1);
			border-color: rgba(255, 255, 255, 0.2);
		}

		&:disabled {
			opacity: 0.3;
			cursor: default;
		}
	}

	.count-value {
		font-size: 16px;
		font-weight: 600;
		font-family: $font-mono;
		color: $text-color;
		min-width: 22px;
		text-align: center;
	}

	.channel-route-arrow {
		display: flex;
		align-items: center;
		color: rgba(247, 247, 249, 0.2);
		flex-shrink: 0;
		align-self: center;
	}

	.channel-strips {
		display: flex;
		gap: 3px;
		align-items: flex-end;
		flex-wrap: wrap;
	}

	.channel-strip {
		width: 16px;
		border-radius: 3px 3px 0 0;
		overflow: hidden;
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.08);
		display: flex;
		flex-direction: column;
		align-items: center;
		padding: 2px 0 2px;
		gap: 2px;
		height: 36px;

		&--in { border-bottom: 2px solid rgba(0, 200, 120, 0.5); }
		&--out { border-bottom: 2px solid rgba(21, 113, 255, 0.5); }

		&--inactive {
			opacity: 0.4;
			filter: grayscale(1);
		}
	}

	.strip-num {
		font-size: 7px;
		color: rgba(247, 247, 249, 0.3);
		font-family: ui-monospace, monospace;
		line-height: 1;
	}

	.strip-meter {
		flex: 1;
		width: 6px;
		background: rgba(255, 255, 255, 0.05);
		border-radius: 1px;
		display: flex;
		align-items: flex-end;
	}

	.strip-level {
		width: 100%;
		border-radius: 1px;

		&--in  { background: rgba(0, 200, 120, 0.65); }
		&--out { background: rgba(21, 113, 255, 0.65); }
	}



	// ── Routing ───────────────────────────────────────────────────────────────
	.routing-row {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 10px 12px;
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.07);
		border-radius: 6px;
	}

	.routing-label {
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.routing-name {
		font-size: 12px;
		font-weight: 500;
		color: $text-color;
	}

	.routing-desc {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.35);
	}

	.routing-assign {
		flex-shrink: 0;
	}

	// ── Single-value display (e.g. unique sample rate) ───────────────────────
	.single-value-display {
		display: flex;
		align-items: center;
		gap: 6px;
		height: $ctrl-height;
		padding: 0 10px;
		background: $surface-1;
		border: 1px solid $border-faint;
		border-radius: $radius-md;
		font-size: 12px;
		font-weight: 600;
		font-family: $font-mono;
		color: $text-color;
		cursor: default;
	}

	// ── Device loading / error states ────────────────────────────────────────
	.loading-dot {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.3);
		margin-left: 2px;
	}

	.apply-error {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 8px 10px;
		background: rgba(255, 60, 60, 0.08);
		border: 1px solid rgba(255, 60, 60, 0.25);
		border-radius: 6px;
		color: #ff7070;
		font-size: 11px;
	}

	// ── Inputs ────────────────────────────────────────────────────────────────
	.text-input {
		flex: 1;
		background: $surface-2;
		border: 1px solid $border-subtle;
		border-radius: $radius-md;
		color: $text-color;
		font-size: 12px;
		padding: 5px 9px;
		height: $ctrl-height;
		box-sizing: border-box;
		outline: none;

		&::placeholder { color: $text-faint; }
		&:focus { border-color: $accent-color; background: rgba(21, 113, 255, 0.06); }
	}

	.field-slash {
		font-size: 15px;
		color: rgba(247, 247, 249, 0.3);
		font-family: monospace;
		line-height: 1;
		flex-shrink: 0;
	}

	// ── Footer ────────────────────────────────────────────────────────────────
	.panel-footer {
		display: flex;
		align-items: center;
		padding: 12px 18px;
		border-top: 1px solid $border-color;
		background: $panel-background-color;
		flex-shrink: 0;
	}

	.footer-actions {
		margin-left: auto;
		display: flex;
		gap: 8px;
	}
</style>
