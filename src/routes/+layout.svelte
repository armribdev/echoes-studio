<script lang="ts">
	export let params: Record<string, string> = {};
	$: void params;
</script>

<slot />
