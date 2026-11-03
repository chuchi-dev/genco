<script lang="ts">
	import { fileState, type ConfigFile } from '@/lib/api';
	import { dialogs } from '@/lib/dialogs.svelte';
	import Button from './win/Button.svelte';

	// config files with what genco writes and what is on the disk
	let { files, group = '<group>' }: { files: ConfigFile[]; group?: string } =
		$props();

	// svelte-ignore state_referenced_locally
	let path = $state(
		(files.find(f => fileState(f) !== 'ok') ?? files[0])?.path ?? '',
	);
	let showDisk = $state(false);

	const file = $derived(files.find(f => f.path === path) ?? files[0]);
	const current = $derived(file ? fileState(file) : 'ok');
	const pending = $derived(
		files.filter(f => ['missing', 'changed'].includes(fileState(f))).length,
	);
	const foreign = $derived(files.filter(f => f.foreign).length);
	const LABELS = {
		ok: '',
		missing: ' (missing)',
		changed: ' (differs)',
		foreign: " (not genco's)",
	};
</script>

{#if pending}
	<div class="pending">
		<span>
			{pending} file{pending > 1 ? 's differ' : ' differs'} from what genco
			writes.
		</span>
		<Button onclick={() => (dialogs.open = 'apply')}>Apply…</Button>
	</div>
{/if}

{#if foreign}
	<div class="pending">
		<span>
			{foreign} drop-in{foreign > 1 ? 's' : ''} not written by genco override{foreign >
			1
				? ''
				: 's'} the group's units. Move limits into the group's limits (General
			tab, or
			<code>genco group adopt-limits {group}</code>).
		</span>
	</div>
{/if}

{#if file}
	<div class="bar">
		<select aria-label="Config file" bind:value={path}>
			{#each files as f (f.path)}
				<option value={f.path}>{f.path}{LABELS[fileState(f)]}</option>
			{/each}
		</select>
		{#if current === 'changed'}
			<label>
				<input type="checkbox" bind:checked={showDisk} />
				on the disk
			</label>
		{/if}
	</div>
	<pre class="file selectable">{(showDisk && current === 'changed') ||
		current === 'foreign'
			? file.actual
			: file.expected}</pre>
{:else}
	<p class="hint">No config files.</p>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.pending {
		display: flex;
		align-items: center;
		gap: 10px;
		justify-content: space-between;
		padding: 6px 8px;
		margin-bottom: 8px;
		background: #ffffe1;
		border: 1px solid #000;
	}

	.bar {
		display: flex;
		gap: 10px;
		align-items: center;
		margin-bottom: 6px;

		select {
			flex: 1;
			min-width: 0;
		}

		label {
			display: flex;
			gap: 4px;
			align-items: center;
			white-space: nowrap;
		}
	}

	.file {
		@include sunken;
		flex: 1;
		margin: 0;
		padding: 6px;
		overflow: auto;
		min-height: 160px;
	}

	.hint {
		color: $shadow;
	}
</style>
