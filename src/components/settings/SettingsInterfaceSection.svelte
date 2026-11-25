<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { AppLocale } from '../../lib/i18n';
	import Field from '../Field.svelte';
	import PillGroup from '../PillGroup.svelte';
	import Section from '../Section.svelte';
	import SubSection from '../SubSection.svelte';
	import Toggle from '../Toggle.svelte';
	import type { Preferences } from '../../stores/preferences';

	export let interfacePrefs: Preferences['interface'];

	$: localeOptions = [
		{ value: 'en' as AppLocale, label: $t('locale.en') },
		{ value: 'fr' as AppLocale, label: $t('locale.fr') }
	];
</script>

<Section title={$t('settings.interface.title')}>
	<Field label={$t('locale.label')} hint={$t('locale.description')}>
		<PillGroup bind:value={interfacePrefs.locale} options={localeOptions} />
	</Field>

	<SubSection>
		<Toggle
			bind:checked={interfacePrefs.showMinimap}
			label={$t('settings.interface.minimap')}
			description={$t('settings.interface.minimapDesc')}
		/>

		<Toggle
			bind:checked={interfacePrefs.showArrangeGrid}
			label={$t('settings.interface.arrangeGrid')}
			description={$t('settings.interface.arrangeGridDesc')}
		/>
	</SubSection>
</Section>
