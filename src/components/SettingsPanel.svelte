<script lang="ts">
	import { preferences, settingsPanelOpen } from '../stores/preferences';
	import { keyboardFocus } from '../stores/keyboardFocus';
	import type { Preferences, ShortcutOverrides } from '../stores/preferences';
	import { onMount, onDestroy } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { setAppLocale } from '../lib/i18n';
	import Button from './Button.svelte';
	import Icon from './Icon.svelte';
	import ShortcutSettings from './ShortcutSettings.svelte';
	import SettingsAudioDevices from './settings/SettingsAudioDevices.svelte';
	import SettingsAudioOptions from './settings/SettingsAudioOptions.svelte';
	import SettingsInterfaceSection from './settings/SettingsInterfaceSection.svelte';
	import SettingsMidiSection, { type MidiDeviceRow } from './settings/SettingsMidiSection.svelte';
	import SettingsProjectSection from './settings/SettingsProjectSection.svelte';
	import Dialog from './Dialog.svelte';
	import Section from './Section.svelte';
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
	import type { AudioDeviceInfo } from '../lib/audioBackend';
	import type { MidiInputDeviceInfo } from '../lib/midiBackend';
	import { midiDeviceList } from '../stores/midiDevices';
	import { listen } from '@tauri-apps/api/event';
	import type { UnlistenFn } from '@tauri-apps/api/event';

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
	$: SECTIONS = [
		{ id: 'audio' as const, label: $t('settings.nav.audio') },
		{ id: 'midi' as const, label: $t('settings.nav.midi') },
		{ id: 'project' as const, label: $t('settings.nav.project') },
		{ id: 'interface' as const, label: $t('settings.nav.interface') },
		{ id: 'shortcuts' as const, label: $t('settings.nav.shortcuts') }
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
			.map((d) => ({ name: d.id, alias: '', enabled: false, mpe: false, connected: true }));
		return [...updated, ...discovered];
	}

	const unsubMidiDeviceList = midiDeviceList.subscribe((visible) => {
		midiRows = reconcileMidiRows(midiRows, visible);
	});

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

	// ── Persistence + backend application ────────────────────────────────────

	async function applyAndClose() {
		applyError = null;
		draft.midi = {
			...draft.midi,
			devices: midiRows.map(({ name, alias, enabled, mpe }) => ({
				name,
				alias: alias.trim(),
				enabled,
				mpe
			}))
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
				.map((row) => ({ ...row, alias: '', enabled: false, mpe: false }));
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
						<SettingsAudioDevices
							bind:audio={draft.audio}
							{inputDeviceOptions}
							{outputDeviceOptions}
							{devicesLoading}
							{channelsLoading}
							{realInputChannels}
							{realOutputChannels}
							{displayInputLevels}
							{displayOutputLevels}
							{applyError}
						/>
						<SettingsAudioOptions
							bind:audio={draft.audio}
							{sampleRateSliderOptions}
							{xrunCount}
							{realOutputChannels}
						/>
					</Section>
				{:else if activeSection === 'midi'}
					<SettingsMidiSection bind:rows={midiRows} bind:midi={draft.midi} />
				{:else if activeSection === 'project'}
					<SettingsProjectSection bind:project={draft.project} />
				{:else if activeSection === 'interface'}
					<SettingsInterfaceSection bind:interfacePrefs={draft.interface} />
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
	@use '../controls.scss' as *;

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
		padding: $space-5 $space-3;
		display: flex;
		flex-direction: column;
		gap: $space-1;
	}

	.nav-item {
		@include list-item($ctrl-height, 12px);
		gap: $space-4;
		padding: 0 $space-5;
		border-radius: $radius-control;
		color: $text-muted;

		&:hover .nav-icon {
			opacity: 0.85;
		}

		&.active {
			@include list-item-selected;

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
		padding: $space-7;

		&::-webkit-scrollbar {
			width: 5px;
		}

		&::-webkit-scrollbar-thumb {
			background: $surface-hover;
			border-radius: $radius-xs;
		}
	}
</style>
