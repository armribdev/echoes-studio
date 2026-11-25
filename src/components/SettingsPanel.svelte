<script lang="ts">
	import { preferences, settingsPanelOpen } from '../stores/preferences';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import type { KnownMidiDevice, Preferences, ShortcutOverrides } from '../stores/preferences';
	import { onMount, onDestroy } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { setAppLocale, type AppLocale } from '../lib/i18n';
	import CustomSelect from './CustomSelect.svelte';
	import Toggle from './Toggle.svelte';
	import PillGroup from './PillGroup.svelte';
	import StepSlider from './StepSlider.svelte';
	import NumberInput from './NumberInput.svelte';
	import Button from './Button.svelte';
	import Icon from './Icon.svelte';
	import ShortcutSettings from './ShortcutSettings.svelte';
	import Dialog from './Dialog.svelte';
	import Field from './Field.svelte';
	import FieldGrid from './FieldGrid.svelte';
	import Section from './Section.svelte';
	import SubSection from './SubSection.svelte';
	import TextInput from './TextInput.svelte';
	import TimeSignatureInput from './TimeSignatureInput.svelte';
	import DialogFooter from './DialogFooter.svelte';
	import DialogHeader from './DialogHeader.svelte';
	import { syncMenuAccelerators } from '../frontend/shortcuts/keymap';
	import {
		listAudioDevices,
		applyAudioSettings,
		getSupportedSampleRates,
		getDeviceChannels,
		setActiveOutputChannels
	} from '../lib/audioBackend';
	import { plugins } from '../stores/plugins';
	import type { AudioDeviceInfo } from '../lib/audioBackend';
	import type { MidiInputDeviceInfo } from '../lib/midiBackend';
	import { midiDeviceList } from '../stores/midiDevices';
	import { listen } from '@tauri-apps/api/event';
	import type { UnlistenFn } from '@tauri-apps/api/event';

	// Routing state (local, not persisted yet)
	let masterRouting = 'out-1-2';
	let headphoneRouting = 'mirror';

	type Section = 'audio' | 'midi' | 'project' | 'interface' | 'shortcuts';
	let activeSection: Section = 'audio';

	// Local draft — confirmed on close or explicit save
	let draft: Preferences = {
		...$preferences,
		audio: { ...$preferences.audio },
		midi: { ...$preferences.midi },
		project: { ...$preferences.project },
		interface: { ...$preferences.interface },
		shortcuts: { ...$preferences.shortcuts }
	};

	/** Shortcuts apply live while the panel is open; Cancel restores this snapshot. */
	let shortcutsBaseline: ShortcutOverrides = { ...$preferences.shortcuts };
	let shortcutsPanelSession = false;
	let lastLiveShortcutsJson = JSON.stringify(draft.shortcuts);

	$: if ($settingsPanelOpen) {
		if (!shortcutsPanelSession) {
			shortcutsPanelSession = true;
			shortcutsBaseline = { ...$preferences.shortcuts };
			draft.shortcuts = { ...$preferences.shortcuts };
			lastLiveShortcutsJson = JSON.stringify(draft.shortcuts);
		}
		const key = JSON.stringify(draft.shortcuts);
		if (key !== lastLiveShortcutsJson) {
			lastLiveShortcutsJson = key;
			if (JSON.stringify($preferences.shortcuts) !== key) {
				preferences.update((p) => ({ ...p, shortcuts: { ...draft.shortcuts } }));
			}
		}
	} else if (shortcutsPanelSession) {
		shortcutsPanelSession = false;
	}

	const SAMPLE_RATES = [44100, 48000, 88200, 96000] as const;
	const BUFFER_SIZES = [64, 128, 256, 512, 1024, 2048] as const;

	$: latencyMs = (draft.audio.bufferSize / draft.audio.sampleRate) * 1000;

	// ── Device lists (fetched from Rust/CPAL on panel open) ──────────────────
	$: defaultDeviceLabel = $t('common.defaultSystem');
	$: DEFAULT_DEVICE = { id: 'default', name: defaultDeviceLabel } as AudioDeviceInfo;
	let outputDevices: AudioDeviceInfo[] = [{ id: 'default', name: 'System default' }];
	let inputDevices: AudioDeviceInfo[] = [{ id: 'default', name: 'System default' }];
	let devicesLoading = false;
	let applyError: string | null = null;

	$: {
		const label = defaultDeviceLabel;
		outputDevices = outputDevices.map((d) => (d.id === 'default' ? { ...d, name: label } : d));
		inputDevices = inputDevices.map((d) => (d.id === 'default' ? { ...d, name: label } : d));
	}

	$: outputDeviceOptions = outputDevices.map((d) => ({ value: d.id, label: d.name }));
	$: inputDeviceOptions = inputDevices.map((d) => ({ value: d.id, label: d.name }));
	$: localeOptions = [
		{ value: 'en' as AppLocale, label: $t('locale.en') },
		{ value: 'fr' as AppLocale, label: $t('locale.fr') }
	];
	$: SECTIONS = [
		{ id: 'audio' as const, label: $t('settings.nav.audio') },
		{ id: 'midi' as const, label: $t('settings.nav.midi') },
		{ id: 'project' as const, label: $t('settings.nav.project') },
		{ id: 'interface' as const, label: $t('settings.nav.interface') },
		{ id: 'shortcuts' as const, label: $t('settings.nav.shortcuts') }
	];
	$: isolationOptions = [
		{ value: 'off' as const, label: $t('common.none') },
		{ value: 'chosen' as const, label: $t('common.chosen') },
		{ value: 'all' as const, label: $t('common.all') }
	];

	// Preview language while the panel is open; cancel restores the saved locale.
	$: if ($settingsPanelOpen) {
		setAppLocale(draft.interface.locale);
		if (typeof document !== 'undefined') {
			document.documentElement.lang = draft.interface.locale;
		}
	}

	async function loadDevices() {
		devicesLoading = true;
		try {
			const list = await listAudioDevices();
			outputDevices = list.outputs.length ? list.outputs : [DEFAULT_DEVICE];
			inputDevices = list.inputs.length ? list.inputs : [DEFAULT_DEVICE];
		} finally {
			devicesLoading = false;
		}
	}

	// ── Hardware MIDI input (ADR-022) ─────────────────────────────────────────
	// Every keyboard seen once stays listed — unplugged ones greyed out — with
	// its alias and enabled flag. Draft only: saving the preferences is what
	// reaches the engine (the subscriber in +page.svelte also owns opening/
	// closing connections, including on hotplug — this panel only edits state).
	interface MidiDeviceRow extends KnownMidiDevice {
		connected: boolean;
	}
	let midiRows: MidiDeviceRow[] = draft.midi.devices.map((d) => ({ ...d, connected: false }));

	// Keeps in-progress edits (alias, enabled) — only `connected` and brand-new
	// devices come from the live list, so a hotplug event never discards typing.
	function reconcileMidiRows(
		rows: MidiDeviceRow[],
		visible: MidiInputDeviceInfo[]
	): MidiDeviceRow[] {
		const visibleNames = new Set(visible.map((d) => d.id));
		const knownNames = new Set(rows.map((r) => r.name));
		const updated = rows.map((r) => ({ ...r, connected: visibleNames.has(r.name) }));
		const discovered = visible
			.filter((d) => !knownNames.has(d.id))
			.map((d) => ({ name: d.id, alias: '', enabled: false, connected: true }));
		return [...updated, ...discovered];
	}

	const unsubMidiDeviceList = midiDeviceList.subscribe((visible) => {
		midiRows = reconcileMidiRows(midiRows, visible);
	});

	let renamingMidiDevice: string | null = null;

	function midiDeviceHint(row: MidiDeviceRow): string {
		return row.alias ? row.name : '';
	}

	function startMidiRename(event: MouseEvent, name: string) {
		if ((event.target as HTMLElement).closest('button')) return;
		renamingMidiDevice = name;
	}

	// Enter and blur both land here; the guard ignores the blur that follows Enter.
	function finishMidiRename(row: MidiDeviceRow, value: string) {
		if (renamingMidiDevice !== row.name) return;
		const name = value.trim();
		row.alias = name === row.name ? '' : name;
		midiRows = midiRows;
		renamingMidiDevice = null;
	}

	function focusAndSelect(node: HTMLInputElement) {
		node.focus();
		node.select();
	}

	// ── Supported sample rates for the selected output device ─────────────────
	// All rates are available by default until the async check resolves.
	let supportedSampleRates: Set<number> = new Set(SAMPLE_RATES);

	// ── Real device channel counts ────────────────────────────────────────────
	let realInputChannels = $preferences.audio.inputChannels;
	let realOutputChannels = $preferences.audio.outputChannels;
	let channelsLoading = false;

	async function refreshInputChannels(deviceId: string) {
		realInputChannels = await getDeviceChannels(deviceId, true);
	}

	// Input device: only refresh channels (no stream restart needed yet).
	$: refreshInputChannels(draft.audio.inputDevice);

	// ── Live channel-routing preview ──────────────────────────────────────────
	// No stream restart — just an atomic write in the CPAL callback (~5 ms).
	$: void setActiveOutputChannels(Math.min(draft.audio.outputChannels, realOutputChannels));

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
				getDeviceChannels(deviceId, false)
			]);

			// Update UI options.
			supportedSampleRates = new Set(rates.length ? rates : SAMPLE_RATES);
			realOutputChannels = outCh;

			// Auto-correct sample rate if the current selection isn't supported.
			let targetSR = draft.audio.sampleRate;
			if (!supportedSampleRates.has(targetSR)) {
				targetSR = ([...supportedSampleRates] as (typeof draft.audio.sampleRate)[]).reduce(
					(p, c) =>
						Math.abs(c - draft.audio.sampleRate) < Math.abs(p - draft.audio.sampleRate) ? c : p
				);
				// Update prevSRBufKey BEFORE updating draft so the non-device
				// streamKey reactive below doesn't fire a duplicate restart.
				prevSRBufKey = `${targetSR}|${draft.audio.bufferSize}|${draft.audio.inputDevice}`;
				draft = { ...draft, audio: { ...draft.audio, sampleRate: targetSR } };
			}

			// Single stream restart with the fully-corrected settings.
			await applyAudioSettings({
				outputDevice: deviceId,
				inputDevice: draft.audio.inputDevice,
				sampleRate: targetSR,
				bufferSize: draft.audio.bufferSize,
				outputChannels: Math.min(draft.audio.outputChannels, realOutputChannels),
				masterSoftClip: draft.audio.masterSoftClip
			});
		} catch (err) {
			applyError = typeof err === 'string' ? err : get(t)('settings.audio.applyError');
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
				outputDevice: draft.audio.outputDevice,
				inputDevice: draft.audio.inputDevice,
				sampleRate: draft.audio.sampleRate,
				bufferSize: draft.audio.bufferSize,
				outputChannels: Math.min(draft.audio.outputChannels, realOutputChannels),
				masterSoftClip: draft.audio.masterSoftClip
			});
		} catch (err) {
			applyError = typeof err === 'string' ? err : get(t)('settings.audio.applyError');
		}
	}

	// Slider options — only include rates the device actually supports.
	// Unsupported rates are simply absent from the slider positions.
	$: sampleRateSliderOptions = SAMPLE_RATES.filter((sr) => supportedSampleRates.has(sr)).map(
		(sr) => ({
			value: sr,
			label: sr >= 1000 ? sr / 1000 + ' kHz' : sr + ' Hz'
		})
	);

	// Buffer size slider options (always all 6 — fixed-buffer failures are
	// handled at stream build time, not at enumeration time).
	const bufferSliderOptions = BUFFER_SIZES.map((bs) => ({
		value: bs,
		label: String(bs)
	}));

	// ── Persistence + backend application ────────────────────────────────────

	async function applyAndClose() {
		applyError = null;
		draft.midi = {
			...draft.midi,
			devices: midiRows.map(({ name, alias, enabled }) => ({ name, alias: alias.trim(), enabled }))
		};

		// Push audio settings to CPAL engine (restarts stream if needed)
		try {
			await applyAudioSettings({
				outputDevice: draft.audio.outputDevice,
				inputDevice: draft.audio.inputDevice,
				sampleRate: draft.audio.sampleRate,
				bufferSize: draft.audio.bufferSize,
				outputChannels: Math.min(draft.audio.outputChannels, realOutputChannels),
				masterSoftClip: draft.audio.masterSoftClip
			});
		} catch (err) {
			applyError = typeof err === 'string' ? err : get(t)('settings.audio.applyError');
			// Still save preferences so settings survive restart
		}
		preferences.set(draft);
		setAppLocale(draft.interface.locale);
		if (typeof document !== 'undefined') {
			document.documentElement.lang = draft.interface.locale;
		}
		syncMenuAccelerators(draft.shortcuts);
		settingsPanelOpen.set(false);
		keyboardFocus.setTimeline();
	}

	function cancelAndClose() {
		applyError = null;
		preferences.update((p) => ({ ...p, shortcuts: { ...shortcutsBaseline } }));
		lastLiveShortcutsJson = JSON.stringify(shortcutsBaseline);
		const prefs = get(preferences);
		draft = {
			...prefs,
			audio: { ...prefs.audio },
			midi: { ...prefs.midi },
			project: { ...prefs.project },
			interface: { ...prefs.interface },
			shortcuts: { ...shortcutsBaseline }
		};
		setAppLocale(prefs.interface.locale);
		if (typeof document !== 'undefined') {
			document.documentElement.lang = prefs.interface.locale;
		}
		midiRows = reconcileMidiRows(
			draft.midi.devices.map((d) => ({ ...d, connected: false })),
			$midiDeviceList
		);
		// Resetting prevOutputDevice to the current (not original) value means
		// the $: device reactive sees a change from current → original and fires
		// applyDeviceChange with the original device → clean revert.
		// (prevSRBufKey is left as-is; liveApplyStreamSettings handles the rest.)
		settingsPanelOpen.set(false);
		keyboardFocus.setTimeline();
	}

	function resetSection() {
		if (activeSection === 'audio')
			draft.audio = {
				inputDevice: 'default',
				outputDevice: 'default',
				sampleRate: 48000,
				bufferSize: 256,
				inputChannels: 2,
				outputChannels: 2,
				masterSoftClip: false,
				pluginIsolation: 'chosen',
				alwaysIsolated: []
			};
		else if (activeSection === 'midi') {
			draft.midi = { devices: [], localControlOff: true };
			midiRows = midiRows
				.filter((row) => row.connected)
				.map((row) => ({ ...row, alias: '', enabled: false }));
		} else if (activeSection === 'project')
			draft.project = {
				defaultFolder: '',
				autoSave: true,
				autoSaveIntervalSec: 60,
				autoSaveMaxBackups: 5,
				defaultBpm: 120,
				defaultNumerator: 4,
				defaultDenominator: 4,
				exportBitDepth: 32,
				exportMp3Bitrate: 320
			};
		else if (activeSection === 'interface')
			draft.interface = {
				showMinimap: false,
				showArrangeGrid: false,
				locale: draft.interface.locale
			};
		else if (activeSection === 'shortcuts') {
			draft.shortcuts = {};
			lastLiveShortcutsJson = '';
		}
		draft = { ...draft };
	}

	// ── Channel level meters ──────────────────────────────────────────────────
	// Driven by LEVELS_UPDATED engine events emitted ~30 fps by the Rust reporter.
	// `rawLevels` are updated instantly from events; `displayLevels` decay via rAF
	// so the meter has a fast attack (immediate snap-up) and slow release (~400 ms).

	let rawOutputLevels: number[] = Array(16).fill(0);
	let rawInputLevels: number[] = Array(16).fill(0);
	let xrunCount = 0;
	let displayOutputLevels: number[] = Array(16).fill(0);
	let displayInputLevels: number[] = Array(16).fill(0);

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
			const newOut =
				rawOutputLevels[i] > displayOutputLevels[i]
					? rawOutputLevels[i]
					: displayOutputLevels[i] * DECAY;
			const newIn =
				rawInputLevels[i] > displayInputLevels[i]
					? rawInputLevels[i]
					: displayInputLevels[i] * DECAY;

			if (Math.abs(newOut - displayOutputLevels[i]) > 0.0001) changed = true;
			if (Math.abs(newIn - displayInputLevels[i]) > 0.0001) changed = true;

			displayOutputLevels[i] = newOut;
			displayInputLevels[i] = newIn;
		}
		// Only trigger Svelte reactivity when something actually changed.
		if (changed) {
			displayOutputLevels = displayOutputLevels;
			displayInputLevels = displayInputLevels;
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
		displayInputLevels = Array(16).fill(0);
		xrunCount = 0;
	}

	/**
	 * Map a linear amplitude peak to a CSS height percentage using a dB scale.
	 * −60 dB → 0 %   |   0 dB → 100 %   |   above 0 dB is clamped to 100 %.
	 */
	function levelToHeight(amp: number): string {
		if (amp <= 0) return '0%';
		const db = 20 * Math.log10(amp);
		const pct = Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
		return `${pct.toFixed(1)}%`;
	}

	onMount(async () => {
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
				if (ev.payload.input) rawInputLevels = ev.payload.input;
				if (typeof ev.payload.xruns === 'number') xrunCount = ev.payload.xruns;
			}
		});
	});
	onDestroy(() => {
		cancelAnimationFrame(rafId);
		unsubMidiDeviceList();
		unlistenLevels?.();
	});
</script>

{#if $settingsPanelOpen}
	<Dialog label={$t('settings.aria')} width={720} height={620} on:close={cancelAndClose}>
		<DialogHeader title={$t('settings.title')} on:close={cancelAndClose}>
			<Icon slot="icon" name="settings" size={14} />
		</DialogHeader>

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
							{:else if s.id === 'midi'}
								<Icon name="cpu" size={15} />
							{:else if s.id === 'project'}
								<Icon name="folder" size={15} />
							{:else if s.id === 'interface'}
								<Icon name="monitor" size={15} />
							{:else if s.id === 'shortcuts'}
								<Icon name="keyboard" size={15} />
							{/if}
						</span>
						<span class="nav-label">{s.label}</span>
					</button>
				{/each}
			</nav>

			<!-- Content -->
			<div class="content">
				{#if activeSection === 'audio'}
					<Section title={$t('settings.audio.devicesTitle')}>
						<!-- ── Deux colonnes IN / OUT ─────────────────────────────────── -->
						<div class="io-grid">
							<!-- Colonne Entrée -->
							<div class="io-col">
								<div class="io-device-row">
									<span class="io-device-label">
										{$t('settings.audio.input')}
										<span class="badge badge--in">IN</span>
										{#if devicesLoading}<span class="loading-dot">…</span>{/if}
									</span>
									<CustomSelect bind:value={draft.audio.inputDevice} options={inputDeviceOptions} />
									<span class="field-hint">{$t('settings.audio.inputHint')}</span>
								</div>

								<div class="channel-card channel-card--in">
									<div class="channel-card-header">
										<span class="channel-card-title">{$t('settings.audio.activeChannels')}</span>
										<div class="channel-count-control">
											<button
												class="count-btn"
												disabled={draft.audio.inputChannels <= 1}
												on:click={() =>
													(draft.audio.inputChannels = Math.max(1, draft.audio.inputChannels - 1))}
												>−</button
											>
											<span class="count-value"
												>{Math.min(draft.audio.inputChannels, realInputChannels)}</span
											>
											<button
												class="count-btn"
												disabled={draft.audio.inputChannels >= realInputChannels}
												on:click={() =>
													(draft.audio.inputChannels = Math.min(
														realInputChannels,
														draft.audio.inputChannels + 1
													))}>+</button
											>
										</div>
									</div>
									<div class="channel-strips">
										{#each Array(Math.min(realInputChannels, 16)) as _, i}
											<div
												class="channel-strip channel-strip--in"
												class:channel-strip--inactive={i >=
													Math.min(draft.audio.inputChannels, realInputChannels)}
												title={$t('settings.audio.inputChannel', { values: { n: i + 1 } })}
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
										{$t('settings.audio.output')}
										<span class="badge badge--out">OUT</span>
										{#if devicesLoading}<span class="loading-dot">…</span>{/if}
									</span>
									<CustomSelect
										bind:value={draft.audio.outputDevice}
										options={outputDeviceOptions}
									/>
									<span class="field-hint">{$t('settings.audio.outputHint')}</span>
								</div>

								<div class="channel-card channel-card--out">
									<div class="channel-card-header">
										<span class="channel-card-title">{$t('settings.audio.activeChannels')}</span>
										{#if channelsLoading}<span class="loading-dot">…</span>{/if}
										<div class="channel-count-control">
											<button
												class="count-btn"
												disabled={draft.audio.outputChannels <= 1}
												on:click={() =>
													(draft.audio.outputChannels = Math.max(
														1,
														draft.audio.outputChannels - 1
													))}>−</button
											>
											<span class="count-value"
												>{Math.min(draft.audio.outputChannels, realOutputChannels)}</span
											>
											<button
												class="count-btn"
												disabled={draft.audio.outputChannels >= realOutputChannels}
												on:click={() =>
													(draft.audio.outputChannels = Math.min(
														realOutputChannels,
														draft.audio.outputChannels + 1
													))}>+</button
											>
										</div>
									</div>
									<div class="channel-strips">
										{#each Array(Math.min(realOutputChannels, 16)) as _, i}
											<div
												class="channel-strip channel-strip--out"
												class:channel-strip--inactive={i >=
													Math.min(draft.audio.outputChannels, realOutputChannels)}
												title={$t('settings.audio.outputChannel', { values: { n: i + 1 } })}
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
						</div>
						<!-- /io-grid -->

						{#if applyError}
							<div class="apply-error">
								<Icon name="alert-triangle" size={13} />
								<span>{applyError}</span>
							</div>
						{/if}

						<SubSection title={$t('settings.audio.qualityTitle')}>
							<FieldGrid>
								<Field label={$t('settings.audio.sampleRate')}>
									{#if sampleRateSliderOptions.length <= 1}
										<div class="single-value-display">
											{sampleRateSliderOptions[0]?.label ??
												(draft.audio.sampleRate >= 1000
													? draft.audio.sampleRate / 1000 + ' kHz'
													: draft.audio.sampleRate + ' Hz')}
										</div>
										<span class="field-hint">{$t('settings.audio.sampleRateSingleHint')}</span>
									{:else}
										<StepSlider
											bind:value={draft.audio.sampleRate}
											options={sampleRateSliderOptions}
										/>
									{/if}
								</Field>

								<Field label={$t('settings.audio.bufferSize')}>
									<StepSlider bind:value={draft.audio.bufferSize} options={bufferSliderOptions} />
								</Field>

								<Field
									label={$t('settings.audio.bitDepth')}
									hint={$t('settings.audio.bitDepthHint')}
								>
									<div class="single-value-display">
										<Icon name="cpu" size={14} />
										32-bit float
									</div>
								</Field>
							</FieldGrid>

							<div class="latency-bar">
								<span class="latency-label">{$t('settings.audio.estimatedLatency')}</span>
								<span class="latency-value">{latencyMs.toFixed(2)} ms</span>
								<div class="latency-indicator">
									<div
										class="latency-fill"
										class:latency-low={latencyMs < 10}
										class:latency-mid={latencyMs >= 10 && latencyMs < 25}
										class:latency-high={latencyMs >= 25}
										style="width: {Math.min((latencyMs / 50) * 100, 100)}%"
									></div>
								</div>
								<span class="latency-hint">
									{#if latencyMs < 10}{$t('settings.audio.latencyExcellent')}
									{:else if latencyMs < 25}{$t('settings.audio.latencyGood')}
									{:else}{$t('settings.audio.latencyHigh')}{/if}
								</span>
							</div>

							<div class="xrun-bar" class:xrun-warning={xrunCount > 0}>
								<span class="xrun-label">{$t('settings.audio.xruns')}</span>
								<span class="xrun-value">{xrunCount}</span>
								<span class="xrun-hint">
									{#if xrunCount === 0}{$t('settings.audio.xrunsNone')}
									{:else}{$t('settings.audio.xrunsHint')}{/if}
								</span>
							</div>
						</SubSection><SubSection title={$t('settings.audio.masterOutTitle')}>
							<Toggle
								bind:checked={draft.audio.masterSoftClip}
								label={$t('settings.audio.softClip')}
								description={$t('settings.audio.softClipDesc')}
							/>
						</SubSection><SubSection title={$t('settings.audio.isolationTitle')}>
							<Field label={$t('settings.audio.isolatedPlugins')}>
								<PillGroup bind:value={draft.audio.pluginIsolation} options={isolationOptions} />
								<span slot="hint">
									{#if draft.audio.pluginIsolation === 'off'}{$t('settings.audio.isolationOff')}
									{:else if draft.audio.pluginIsolation === 'chosen'}{$t(
											'settings.audio.isolationChosen'
										)}
									{:else}{$t('settings.audio.isolationAll')}{/if}
									{$t('settings.audio.isolationSupportedFormats')}
								</span>
							</Field>

							{#if draft.audio.pluginIsolation === 'chosen' && draft.audio.alwaysIsolated.length > 0}
								<Field
									label={$t('settings.audio.alwaysIsolated')}
									hint={$t('settings.audio.alwaysIsolatedHint')}
								>
									<div class="isolated-list">
										{#each draft.audio.alwaysIsolated as pluginId (pluginId)}
											<span class="isolated-chip">
												{$plugins.find((p) => p.id === pluginId)?.name ?? pluginId}
												<button
													type="button"
													title={$t('settings.audio.removeAlwaysIsolate')}
													on:click={() =>
														(draft.audio.alwaysIsolated = draft.audio.alwaysIsolated.filter(
															(id) => id !== pluginId
														))}>×</button
												>
											</span>
										{/each}
									</div>
								</Field>
							{/if}
						</SubSection><SubSection title={$t('settings.audio.routingTitle')}>
							<div class="routing-row">
								<div class="routing-label">
									<span class="routing-name">{$t('settings.audio.masterLr')}</span>
									<span class="routing-desc">{$t('settings.audio.masterLrDesc')}</span>
								</div>
								<div class="routing-assign">
									<CustomSelect
										small
										bind:value={masterRouting}
										options={[
											{ value: 'out-1-2', label: $t('settings.audio.outputs12') },
											...(Math.min(draft.audio.outputChannels, realOutputChannels) >= 4
												? [{ value: 'out-3-4', label: $t('settings.audio.outputs34') }]
												: []),
											...(Math.min(draft.audio.outputChannels, realOutputChannels) >= 6
												? [{ value: 'out-5-6', label: $t('settings.audio.outputs56') }]
												: [])
										]}
									/>
								</div>
							</div>

							<div class="routing-row">
								<div class="routing-label">
									<span class="routing-name">{$t('settings.audio.headphones')}</span>
									<span class="routing-desc">{$t('settings.audio.headphonesDesc')}</span>
								</div>
								<div class="routing-assign">
									<CustomSelect
										small
										bind:value={headphoneRouting}
										options={[
											{ value: 'mirror', label: $t('settings.audio.mirrorMaster') },
											...(Math.min(draft.audio.outputChannels, realOutputChannels) >= 4
												? [{ value: 'out-3-4', label: $t('settings.audio.outputs34') }]
												: []),
											...(Math.min(draft.audio.outputChannels, realOutputChannels) >= 6
												? [{ value: 'out-5-6', label: $t('settings.audio.outputs56') }]
												: [])
										]}
									/>
								</div>
							</div>
						</SubSection></Section
					>
				{:else if activeSection === 'midi'}
					<Section title={$t('settings.midi.title')}>
						<span class="field-hint">{$t('settings.midi.intro')}</span>

						<SubSection>
							{#if midiRows.length === 0}
								<span class="field-hint">{$t('settings.midi.none')}</span>
							{:else}
								{#each midiRows as row (row.name)}
									<!-- svelte-ignore a11y-no-static-element-interactions -->
									<div
										class:midi-device--offline={!row.connected}
										title={$t('settings.midi.renameHint')}
										on:dblclick={(e) => startMidiRename(e, row.name)}
									>
										{#if renamingMidiDevice === row.name}
											<Toggle bind:checked={row.enabled} label={row.alias || row.name}>
												<input
													slot="info"
													class="midi-rename"
													type="text"
													value={row.alias || row.name}
													aria-label={$t('settings.midi.renameAria', {
														values: { name: row.name }
													})}
													use:focusAndSelect
													on:keydown={(e) => {
														if (e.key === 'Enter') finishMidiRename(row, e.currentTarget.value);
													}}
													on:blur={(e) => finishMidiRename(row, e.currentTarget.value)}
												/>
											</Toggle>
										{:else}
											<Toggle
												bind:checked={row.enabled}
												label={row.alias || row.name}
												description={midiDeviceHint(row)}
											/>
										{/if}
									</div>
								{/each}
							{/if}
						</SubSection><SubSection title={$t('settings.midi.keyboardSound')}>
							<Toggle
								bind:checked={draft.midi.localControlOff}
								label={$t('settings.midi.localControlOff')}
								description={$t('settings.midi.localControlOffDesc')}
							/>
						</SubSection></Section
					>
				{:else if activeSection === 'project'}
					<Section title={$t('settings.project.title')}>
						<Field label={$t('settings.project.defaultFolder')}>
							<div class="field-row">
								<TextInput
									bind:value={draft.project.defaultFolder}
									placeholder="~/Documents/Echoes Studio"
								/>
								<Button compact on:click={() => {}}>{$t('common.browse')}</Button>
							</div>
						</Field>

						<SubSection title={$t('settings.project.autoSaveTitle')}>
							<Toggle
								bind:checked={draft.project.autoSave}
								label={$t('settings.project.autoSave')}
								description={$t('settings.project.autoSaveDesc')}
							/>

							{#if draft.project.autoSave}
								<FieldGrid>
									<Field label={$t('settings.project.interval')}>
										<NumberInput
											bind:value={draft.project.autoSaveIntervalSec}
											min={10}
											max={600}
											unit="sec"
										/>
									</Field>
									<Field label={$t('settings.project.backupsKept')}>
										<NumberInput
											bind:value={draft.project.autoSaveMaxBackups}
											min={1}
											max={20}
											unit="max"
										/>
									</Field>
								</FieldGrid>
							{/if}
						</SubSection><SubSection title={$t('settings.project.exportTitle')}>
							<Field label={$t('settings.project.wavBitDepth')}>
								<PillGroup
									bind:value={draft.project.exportBitDepth}
									options={[
										{ value: 16, label: '16 bit' },
										{ value: 24, label: '24 bit' },
										{ value: 32, label: '32-bit float' }
									]}
								/>
								<span slot="hint">
									{#if draft.project.exportBitDepth === 16}{$t('settings.project.wav16')}
									{:else if draft.project.exportBitDepth === 24}{$t('settings.project.wav24')}
									{:else}{$t('settings.project.wav32')}{/if}
								</span>
							</Field>

							<Field label={$t('settings.project.mp3Bitrate')}>
								<PillGroup
									bind:value={draft.project.exportMp3Bitrate}
									options={[
										{ value: 128, label: '128 kbps' },
										{ value: 192, label: '192 kbps' },
										{ value: 320, label: '320 kbps' }
									]}
								/>
								<span slot="hint">
									{#if draft.project.exportMp3Bitrate === 128}{$t('settings.project.mp3_128')}
									{:else if draft.project.exportMp3Bitrate === 192}{$t('settings.project.mp3_192')}
									{:else}{$t('settings.project.mp3_320')}{/if}
								</span>
							</Field>
						</SubSection><SubSection title={$t('settings.project.newProjectDefaults')}>
							<FieldGrid>
								<Field label={$t('settings.project.bpm')}>
									<NumberInput
										bind:value={draft.project.defaultBpm}
										min={20}
										max={400}
										unit="BPM"
									/>
								</Field>
								<Field label={$t('settings.project.timeSignature')}>
									<TimeSignatureInput
										bind:numerator={draft.project.defaultNumerator}
										bind:denominator={draft.project.defaultDenominator}
									/>
								</Field>
							</FieldGrid>
						</SubSection></Section
					>
				{:else if activeSection === 'interface'}
					<Section title={$t('settings.interface.title')}>
						<Field label={$t('locale.label')} hint={$t('locale.description')}>
							<PillGroup bind:value={draft.interface.locale} options={localeOptions} />
						</Field>

						<SubSection>
							<Toggle
								bind:checked={draft.interface.showMinimap}
								label={$t('settings.interface.minimap')}
								description={$t('settings.interface.minimapDesc')}
							/>

							<Toggle
								bind:checked={draft.interface.showArrangeGrid}
								label={$t('settings.interface.arrangeGrid')}
								description={$t('settings.interface.arrangeGridDesc')}
							/>
						</SubSection></Section
					>
				{:else if activeSection === 'shortcuts'}
					<Section title={$t('settings.shortcuts.title')}>
						<ShortcutSettings bind:overrides={draft.shortcuts} />
					</Section>
				{/if}
			</div>
		</div>

		<DialogFooter>
			<Button
				slot="start"
				variant="ghost"
				on:click={resetSection}
				title={$t('settings.resetSection')}>{$t('common.reset')}</Button
			>
			<Button variant="secondary" on:click={cancelAndClose}>{$t('common.cancel')}</Button>
			<Button variant="primary" on:click={applyAndClose}>{$t('common.save')}</Button>
		</DialogFooter>
	</Dialog>
{/if}

<style lang="scss">
	@use '../variables.scss' as *;

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
		transition:
			background $duration-base,
			color $duration-base;
		width: 100%;

		&:hover {
			background: rgba(255, 255, 255, 0.06);
			color: rgba(247, 247, 249, 0.85);
		}

		&:hover .nav-icon {
			opacity: 0.85;
		}

		&.active {
			background: rgba(21, 113, 255, 0.15);
			color: $text-color;

			.nav-icon {
				opacity: 1;
			}
		}
	}

	.nav-icon {
		display: flex;
		align-items: center;
		justify-content: center;
		opacity: 0.6;
		flex-shrink: 0;
		transition: opacity $duration-base;
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
			background: rgba(255, 255, 255, 0.12);
			border-radius: $radius-xs;
		}
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
		border-radius: $radius-xs;

		&--in {
			background: rgba(0, 200, 120, 0.15);
			color: $color-success;
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
		transition:
			width 0.3s,
			background-color 0.3s;

		&.latency-low {
			background: $color-success;
		}
		&.latency-mid {
			background: $color-warning;
		}
		&.latency-high {
			background: $color-error;
		}
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

		&--in {
			border-top: 2px solid rgba(0, 200, 120, 0.35);
		}
		&--out {
			border-top: 2px solid rgba(21, 113, 255, 0.35);
		}
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
		transition:
			background $duration-base,
			border-color $duration-base;

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

		&--in {
			border-bottom: 2px solid rgba(0, 200, 120, 0.5);
		}
		&--out {
			border-bottom: 2px solid rgba(21, 113, 255, 0.5);
		}

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

		&--in {
			background: rgba(0, 200, 120, 0.65);
		}
		&--out {
			background: rgba(21, 113, 255, 0.65);
		}
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

	.midi-device--offline {
		opacity: 0.45;
	}

	// Sits where the Toggle label is, so the row keeps its height while renaming.
	.midi-rename {
		width: 100%;
		padding: 0;
		border: none;
		background: transparent;
		color: $text-color;
		font: inherit;
		font-size: 12px;
		font-weight: 500;
		outline: none;
	}

	.isolated-list {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}

	.isolated-chip {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 2px 4px 2px 8px;
		font-size: 11px;
		border: 1px solid rgba(34, 197, 94, 0.45);
		background: rgba(34, 197, 94, 0.1);
		border-radius: $radius-xl;
		color: #4ade80;

		button {
			border: none;
			background: none;
			color: inherit;
			cursor: pointer;
			font-size: 13px;
			line-height: 1;
			padding: 0 2px;
			&:hover {
				color: #ff6b61;
			}
		}
	}
</style>
