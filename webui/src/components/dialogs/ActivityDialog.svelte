<script lang="ts">
	import { formatTime, get, type Activity } from '@/lib/api';
	import Button from '../win/Button.svelte';
	import Dialog from '../win/Dialog.svelte';

	let { onclose }: { onclose: () => void } = $props();

	// newest first, "Older" loads the next page
	let entries = $state<Activity[]>([]);
	let more = $state(false);
	let error = $state<string | null>(null);

	const PAGE = 200;

	let busy = $state(false);

	async function load(older = false) {
		if (busy) return;
		busy = true;
		const before = older ? entries.at(-1)?.id : undefined;
		try {
			const page: Activity[] = await get(
				'/activity' + (before ? `?before=${before}` : ''),
			);
			entries = older ? [...entries, ...page] : page;
			more = page.length === PAGE;
			error = null;
		} catch (e: any) {
			error = e.message;
		}
		busy = false;
	}
	load();
</script>

<Dialog title="Activity Log" resizable icon="computer" {onclose} width={760}>
	<p class="hint">
		Who changed what: admins in the panel, API tokens, the agent and
		commands in the terminal. Kept for a year.
	</p>
	<div class="list-view log">
		<table>
			<thead>
				<tr>
					<th>Time</th>
					<th>Who</th>
					<th>Action</th>
					<th>On</th>
					<th>Details</th>
				</tr>
			</thead>
			<tbody>
				{#each entries as e (e.id)}
					<tr>
						<td>{formatTime(e.at)}</td>
						<td>{e.actor}</td>
						<td class="status" class:failed={e.ok === false}>
							{e.action}{e.ok === false ? ' (failed)' : ''}
						</td>
						<td>{e.target}</td>
						<td class="detail" title={e.detail}>{e.detail}</td>
					</tr>
				{:else}
					<tr><td colspan="5" class="empty">Nothing yet.</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
	{#if error}<p class="error">{error}</p>{/if}
	<div class="row-buttons">
		<Button onclick={() => load()}>Refresh</Button>
		<Button onclick={() => load(true)} disabled={!more || busy}
			>Older</Button
		>
		<span class="gap"></span>
		<Button primary onclick={onclose}>Close</Button>
	</div>
</Dialog>

<style lang="scss">
	.hint {
		margin: 0 0 8px;
	}

	.log {
		flex: 1;
		min-height: 260px;
	}

	.detail {
		max-width: 320px;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>
