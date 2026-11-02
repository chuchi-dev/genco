<script lang="ts">
	import { KINDS, KIND_NAMES, PHP_VERSIONS, type Kind } from '@/lib/api';

	// what serves a domain, used to create and to edit one
	let {
		kind = $bindable(),
		phpVersion = $bindable(),
		root = $bindable(),
		command = $bindable(),
		publicDir = $bindable(),
	}: {
		kind: Kind;
		phpVersion: string | null;
		root: string;
		command: string | null;
		publicDir: string | null;
	} = $props();

	const app = $derived(kind === 'node' || kind === 'rust');

	function setKind(k: Kind) {
		kind = k;
		phpVersion = k === 'php' ? (phpVersion ?? '8.4') : null;
		const isApp = k === 'node' || k === 'rust';
		if (isApp && !command)
			command = k === 'node' ? 'node build/index.js' : './server';
		if (!isApp) command = publicDir = null;
	}
</script>

<label for="d-kind">Type:</label>
<select
	id="d-kind"
	value={kind}
	onchange={e => setKind((e.target as HTMLSelectElement).value as Kind)}
>
	{#each KINDS as k (k)}
		<option value={k}>{KIND_NAMES[k]}</option>
	{/each}
</select>
{#if kind === 'php'}
	<label for="d-php">PHP version:</label>
	<select id="d-php" bind:value={phpVersion}>
		{#each PHP_VERSIONS as v (v)}
			<option value={v}>{v}</option>
		{/each}
	</select>
{/if}
<label for="d-root">{app ? 'Directory:' : 'Root:'}</label>
<input id="d-root" placeholder="public" bind:value={root} />
{#if app}
	<label for="d-cmd">Command:</label>
	<input id="d-cmd" class="mono" bind:value={command} />
	<label for="d-public">Public folder:</label>
	<input
		id="d-public"
		placeholder="optional, e.g. current/svelte/dist/public"
		title="nginx serves these files itself, everything else goes to the app"
		bind:value={publicDir}
	/>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.mono {
		font-family: $mono;
	}
</style>
