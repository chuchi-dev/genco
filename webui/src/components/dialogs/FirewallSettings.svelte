<script lang="ts">
	import { formatTime, get, newest, post } from '@/lib/api';
	import { message, confirmDelete } from '@/lib/dialogs.svelte';
	import Button from '../win/Button.svelte';
	import Form from '../win/Form.svelte';
	import ListView from '../win/ListView.svelte';

	// the blocklist: addresses that can't reach the sites, mail and
	// workers' public ports (ssh stays open)
	type Block = {
		id: number;
		address: string;
		note: string;
		createdBy: string;
		createdAt: number;
		until: number | null;
	};

	let blocks = $state<Block[]>([]);
	let selected = $state<number | null>(null);
	let adding = $state(false);
	let address = $state('');
	let note = $state('');
	let duration = $state('24h');

	const fetchBlocks = newest();
	function load() {
		fetchBlocks(
			get('/firewall/blocks'),
			b => (blocks = b),
			e => message.show('Firewall', e.message),
		);
	}
	load();

	async function add() {
		await post('/firewall/blocks', { address, note, duration });
		adding = false;
		address = '';
		note = '';
		load();
	}

	async function remove() {
		const block = blocks.find(b => b.id === selected);
		const removed =
			block &&
			(await confirmDelete({
				title: 'Unblock',
				text: `Unblock ${block.address}? It reaches the sites and mail again.`,
				path: `/firewall/blocks/${block.id}`,
			}));
		if (!removed) return;
		selected = null;
		load();
	}
</script>

{#if adding}
	<Form onsubmit={add} oncancel={() => (adding = false)} ok="Block">
		<label for="fw-address">Address:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			id="fw-address"
			autofocus
			placeholder="e.g. 203.0.113.7 or 203.0.113.0/24"
			bind:value={address}
		/>
		<label for="fw-note">Note:</label>
		<input id="fw-note" placeholder="why" bind:value={note} />
		<label for="fw-duration">For:</label>
		<select id="fw-duration" bind:value={duration}>
			<option value="1h">1 hour</option>
			<option value="24h">24 hours</option>
			<option value="7d">7 days</option>
			<option value="">until unblocked</option>
		</select>
	</Form>
{:else}
	<p>
		Blocked addresses can't reach the sites, mail and the workers' public
		ports. ssh stays open: whoever locked themselves out unblocks with
		<code>genco firewall unblock</code>.
	</p>
	<ListView
		items={blocks}
		key={b => b.id}
		bind:selected
		columns={[
			{ label: 'Address' },
			{ label: 'Note' },
			{ label: 'By' },
			{ label: 'Until' },
		]}
		empty="No blocked addresses."
		minHeight={110}
		class="blocks"
	>
		{#snippet row(b)}
			<td>{b.address}</td>
			<td>{b.note}</td>
			<td>{b.createdBy}</td>
			<td>{b.until ? formatTime(b.until) : 'unblocked'}</td>
		{/snippet}
	</ListView>
	<div class="row-buttons">
		<Button onclick={() => (adding = true)}>Block…</Button>
		<Button onclick={remove} disabled={selected === null}>Unblock</Button>
	</div>
{/if}

<style lang="scss">
	p {
		margin: 0 0 8px;
	}

	:global(.list-view.blocks) {
		margin-bottom: 8px;
	}
</style>
