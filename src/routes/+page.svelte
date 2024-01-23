<script>
	import MenuBar from '../components/MenuBar.svelte';
	import MainPannel from '../components/MainPannel.svelte';
	import StatusBar from '../components/StatusBar.svelte';
</script>

<div class="app">
	<MenuBar />
	<div class="content">
		<MainPannel />
	</div>
	<StatusBar />
</div>

<style>
	.app {
		display: flex;
		flex-direction: column;
		/* height: 100vh; */
	}

	.content {
		flex: 1;
	}
</style>
