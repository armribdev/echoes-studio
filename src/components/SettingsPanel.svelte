<script lang="ts">
	import { preferences, settingsPanelOpen } from '../stores/preferences';
	import type { Preferences } from '../stores/preferences';
	import { onMount, onDestroy } from 'svelte';
	import CustomSelect from './CustomSelect.svelte';
	import Toggle from './Toggle.svelte';
	import PillGroup from './PillGroup.svelte';
	import NumberInput from './NumberInput.svelte';
	import Button from './Button.svelte';
	import Icon from './Icon.svelte';

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

	function applyAndClose() {
		preferences.set(draft);
		settingsPanelOpen.set(false);
	}

	function cancelAndClose() {
		draft = { ...$preferences, audio: { ...$preferences.audio }, project: { ...$preferences.project }, interface: { ...$preferences.interface } };
		settingsPanelOpen.set(false);
	}

	function resetSection() {
		if (activeSection === 'audio') draft.audio = { inputDevice: 'default', outputDevice: 'default', sampleRate: 48000, bufferSize: 256, bitDepth: 24, inputChannels: 2, outputChannels: 2 };
		else if (activeSection === 'project') draft.project = { defaultFolder: '', autoSave: true, autoSaveIntervalSec: 60, autoSaveMaxBackups: 5, defaultBpm: 120, defaultNumerator: 4, defaultDenominator: 4 };
		else if (activeSection === 'interface') draft.interface = { showMinimap: false };
		draft = { ...draft };
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') cancelAndClose();
	}

	onMount(() => window.addEventListener('keydown', handleKeydown));
	onDestroy(() => window.removeEventListener('keydown', handleKeydown));

	const SECTIONS: { id: Section; label: string }[] = [
		{ id: 'audio',     label: 'Audio'     },
		{ id: 'project',   label: 'Projet'    },
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

					<div class="field-group">
						<div class="field">
							<span class="field-label">Entrée</span>
							<div class="field-row">
								<CustomSelect
									bind:value={draft.audio.inputDevice}
									options={[{ value: 'default', label: 'Par défaut (système)' }]}
								/>
								<span class="badge badge--in">IN</span>
							</div>
							<span class="field-hint">Interface audio, microphone, ou entrée virtuelle</span>
						</div>

						<div class="field">
							<span class="field-label">Sortie</span>
							<div class="field-row">
								<CustomSelect
									bind:value={draft.audio.outputDevice}
									options={[{ value: 'default', label: 'Par défaut (système)' }]}
								/>
								<span class="badge badge--out">OUT</span>
							</div>
							<span class="field-hint">Haut-parleurs, casque, ou interface de sortie</span>
						</div>
					</div>

					<div class="divider"></div>
					<h3 class="subsection-title">Qualité & Latence</h3>

					<div class="field-grid">
						<div class="field">
							<span class="field-label">Fréquence d'échantillonnage</span>
							<PillGroup
								bind:value={draft.audio.sampleRate}
								options={SAMPLE_RATES.map(sr => ({ value: sr, label: sr >= 1000 ? (sr / 1000) + ' kHz' : sr + ' Hz' }))}
							/>
						</div>

						<div class="field">
							<span class="field-label">Taille de buffer</span>
							<PillGroup
								bind:value={draft.audio.bufferSize}
								options={BUFFER_SIZES.map(bs => ({ value: bs, label: String(bs) }))}
							/>
						</div>

						<div class="field">
							<span class="field-label">Profondeur de bits</span>
							<PillGroup
								bind:value={draft.audio.bitDepth}
								options={BIT_DEPTHS.map(bd => ({ value: bd, label: bd + ' bit' }))}
							/>
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

					<div class="divider"></div>
					<h3 class="subsection-title">Canaux</h3>

					<div class="channel-grid">
						<div class="channel-card channel-card--in">
							<div class="channel-card-header">
								<span class="channel-card-badge badge--in">IN</span>
								<span class="channel-card-title">Entrées</span>
							</div>
							<div class="channel-count-control">
								<button class="count-btn" on:click={() => draft.audio.inputChannels = Math.max(1, draft.audio.inputChannels - 1)}>−</button>
								<span class="count-value">{draft.audio.inputChannels}</span>
								<button class="count-btn" on:click={() => draft.audio.inputChannels = Math.min(32, draft.audio.inputChannels + 1)}>+</button>
							</div>
							<span class="channel-card-hint">{draft.audio.inputChannels === 1 ? 'Mono' : draft.audio.inputChannels === 2 ? 'Stéréo' : `${draft.audio.inputChannels} canaux`}</span>
							<div class="channel-strips">
								{#each Array(Math.min(draft.audio.inputChannels, 16)) as _, i}
									<div class="channel-strip channel-strip--in" title="Entrée {i + 1}">
										<span class="strip-num">{i + 1}</span>
										<div class="strip-meter"><div class="strip-level"></div></div>
									</div>
								{/each}
								{#if draft.audio.inputChannels > 16}
									<span class="strip-more">+{draft.audio.inputChannels - 16}</span>
								{/if}
							</div>
						</div>

					<div class="channel-route-arrow">
						<Icon name="arrow-right" size={14} />
					</div>

						<div class="channel-card channel-card--out">
							<div class="channel-card-header">
								<span class="channel-card-badge badge--out">OUT</span>
								<span class="channel-card-title">Sorties</span>
							</div>
							<div class="channel-count-control">
								<button class="count-btn" on:click={() => draft.audio.outputChannels = Math.max(1, draft.audio.outputChannels - 1)}>−</button>
								<span class="count-value">{draft.audio.outputChannels}</span>
								<button class="count-btn" on:click={() => draft.audio.outputChannels = Math.min(32, draft.audio.outputChannels + 1)}>+</button>
							</div>
							<span class="channel-card-hint">{draft.audio.outputChannels === 1 ? 'Mono' : draft.audio.outputChannels === 2 ? 'Stéréo' : `${draft.audio.outputChannels} canaux`}</span>
							<div class="channel-strips">
								{#each Array(Math.min(draft.audio.outputChannels, 16)) as _, i}
									<div class="channel-strip channel-strip--out" title="Sortie {i + 1}">
										<span class="strip-num">{i + 1}</span>
										<div class="strip-meter"><div class="strip-level"></div></div>
									</div>
								{/each}
								{#if draft.audio.outputChannels > 16}
									<span class="strip-more">+{draft.audio.outputChannels - 16}</span>
								{/if}
							</div>
						</div>
					</div>

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
									...(draft.audio.outputChannels >= 4 ? [{ value: 'out-3-4', label: 'Sorties 3–4' }] : []),
									...(draft.audio.outputChannels >= 6 ? [{ value: 'out-5-6', label: 'Sorties 5–6' }] : [])
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
									...(draft.audio.outputChannels >= 4 ? [{ value: 'out-3-4', label: 'Sorties 3–4' }] : []),
									...(draft.audio.outputChannels >= 6 ? [{ value: 'out-5-6', label: 'Sorties 5–6' }] : [])
								]}
							/>
						</div>
					</div>
				</div>

				{:else if activeSection === 'project'}
				<div class="section">
					<h2 class="section-title">Projet & Sauvegarde</h2>

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
		background: $pannel-background-color;
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
		gap: 16px;
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
		margin: 2px 0;
	}

	// ── Field ─────────────────────────────────────────────────────────────────
	.field {
		display: flex;
		flex-direction: column;
		gap: 5px;
	}

	.field-group {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.field-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
		gap: 16px;
		margin-top: 4px;
	}

	.field-label {
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

	// ── Channel cards ─────────────────────────────────────────────────────────
	.channel-grid {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.channel-card {
		flex: 1;
		background: $surface-1;
		border: 1px solid $border-faint;
		border-radius: $radius-lg;
		padding: 14px;
		display: flex;
		flex-direction: column;
		gap: 10px;

		&--in { border-top: 2px solid rgba(0, 200, 120, 0.35); }
		&--out { border-top: 2px solid rgba(21, 113, 255, 0.35); }
	}

	.channel-card-header {
		display: flex;
		align-items: center;
		gap: 7px;
	}

	.channel-card-badge {
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.8px;
		padding: 2px 5px;
		border-radius: 3px;
	}

	.channel-card-title {
		font-size: 12px;
		font-weight: 600;
		color: $text-color;
	}

	.channel-count-control {
		display: flex;
		align-items: center;
		gap: 10px;
	}

	.count-btn {
		width: 26px;
		height: 26px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: $surface-2;
		border-radius: $radius-md;
		color: $text-color;
		font-size: 16px;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: background 0.1s, border-color 0.1s;

		&:hover {
			background: rgba(255, 255, 255, 0.1);
			border-color: rgba(255, 255, 255, 0.2);
		}
	}

	.count-value {
		font-size: 22px;
		font-weight: 600;
		font-family: $font-mono;
		color: $text-color;
		min-width: 30px;
		text-align: center;
	}

	.channel-card-hint {
		font-size: 11px;
		color: rgba(247, 247, 249, 0.35);
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
		background: rgba(0, 200, 120, 0.5);
		border-radius: 1px;
		transition: height 0.1s;
	}

	.strip-more {
		font-size: 10px;
		color: rgba(247, 247, 249, 0.3);
		align-self: center;
	}

	.channel-route-arrow {
		font-size: 20px;
		color: rgba(247, 247, 249, 0.2);
		flex-shrink: 0;
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
		background: $pannel-background-color;
		flex-shrink: 0;
	}

	.footer-actions {
		margin-left: auto;
		display: flex;
		gap: 8px;
	}
</style>
