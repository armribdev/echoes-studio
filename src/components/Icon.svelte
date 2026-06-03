<script lang="ts">
	export let name:
		| 'settings'
		| 'volume'
		| 'folder'
		| 'monitor'
		| 'close'
		| 'arrow-right'
		| 'alert-triangle'
		| 'cpu'
		| 'inspector'
		| 'library';
	export let size: number = 14;
	export let strokeWidth: number = 1.8;
</script>

<svg
	width={size}
	height={size}
	viewBox="0 0 24 24"
	fill="none"
	stroke="currentColor"
	stroke-width={strokeWidth}
	stroke-linecap="round"
	stroke-linejoin="round"
	xmlns="http://www.w3.org/2000/svg"
	aria-hidden="true"
>
	{#if name === 'settings'}
		<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/>
		<circle cx="12" cy="12" r="3"/>
	{:else if name === 'volume'}
		<polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/>
		<path d="M15.54 8.46a5 5 0 0 1 0 7.07"/>
		<path d="M19.07 4.93a10 10 0 0 1 0 14.14"/>
	{:else if name === 'folder'}
		<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
	{:else if name === 'monitor'}
		<rect x="2" y="3" width="20" height="14" rx="2"/>
		<line x1="8" y1="21" x2="16" y2="21"/>
		<line x1="12" y1="17" x2="12" y2="21"/>
	{:else if name === 'close'}
		<path d="M18 6L6 18M6 6l12 12"/>
	{:else if name === 'arrow-right'}
		<line x1="5" y1="12" x2="19" y2="12"/>
		<polyline points="12 5 19 12 12 19"/>
	{:else if name === 'alert-triangle'}
		<path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/>
		<line x1="12" y1="9" x2="12" y2="13"/>
		<line x1="12" y1="17" x2="12.01" y2="17"/>
	{:else if name === 'cpu'}
		<rect x="4" y="4" width="16" height="16" rx="2"/>
		<rect x="9" y="9" width="6" height="6"/>
		<line x1="9" y1="1" x2="9" y2="4"/>
		<line x1="15" y1="1" x2="15" y2="4"/>
		<line x1="9" y1="20" x2="9" y2="23"/>
		<line x1="15" y1="20" x2="15" y2="23"/>
		<line x1="1" y1="9" x2="4" y2="9"/>
		<line x1="1" y1="15" x2="4" y2="15"/>
		<line x1="20" y1="9" x2="23" y2="9"/>
		<line x1="20" y1="15" x2="23" y2="15"/>
	{:else if name === 'inspector'}
		<rect x="3" y="3" width="18" height="18" rx="2"/>
		<line x1="9" y1="3" x2="9" y2="21"/>
		<line x1="5" y1="8" x2="7" y2="8"/>
		<line x1="5" y1="12" x2="7" y2="12"/>
		<line x1="5" y1="16" x2="7" y2="16"/>
	{:else if name === 'library'}
		<rect x="3" y="3" width="7" height="7" rx="1"/>
		<rect x="14" y="3" width="7" height="7" rx="1"/>
		<rect x="3" y="14" width="7" height="7" rx="1"/>
		<rect x="14" y="14" width="7" height="7" rx="1"/>
	{/if}
</svg>
