<script lang="ts">
	import { applyErrors, get, saveEnv } from '@/lib/api';
	import Button from '@/components/win/Button.svelte';

	// environment variables of a Node or Rust app; a secret's value never
	// comes back, empty keeps it
	let { domainId }: { domainId: number } = $props();

	// `from`: the name it had when it loaded, a renamed secret keeps its
	// value on the server
	type Var = {
		name: string;
		value: string;
		secret: boolean;
		from?: string;
	};
	const loaded = (list: Var[]) => list.map(v => ({ ...v, from: v.name }));

	let vars = $state<Var[]>([]);
	let error = $state<string | null>(null);
	let saved = $state(false);
	let busy = $state(false);

	$effect(() => {
		get(`/domains/${domainId}/env`)
			.then((v: Var[]) => (vars = loaded(v)))
			.catch(e => (error = e.message));
	});

	function add() {
		vars.push({ name: '', value: '', secret: false });
	}

	async function save() {
		busy = true;
		error = null;
		try {
			const list = vars
				.filter(v => v.name.trim())
				.map(({ name, value, secret, from }) => ({
					name,
					value,
					secret,
					from,
				}));
			const report = await saveEnv(domainId, list);
			// reload first: they are saved even when apply failed, a
			// renamed secret's `from` must be its new name
			vars = loaded(await get(`/domains/${domainId}/env`));
			if (!report.ok)
				throw new Error(
					`Saved, but apply failed: ${applyErrors(report)}`,
				);
			saved = true;
			setTimeout(() => (saved = false), 1500);
		} catch (e: any) {
			error = e.message;
		} finally {
			busy = false;
		}
	}
</script>

<div class="list-view">
	<table>
		<thead>
			<tr>
				<th>Name</th>
				<th>Value</th>
				<th>Secret</th>
				<th></th>
			</tr>
		</thead>
		<tbody>
			{#each vars as v, i (i)}
				<tr>
					<td>
						<input
							class="mono"
							aria-label="Name"
							spellcheck="false"
							bind:value={v.name}
						/>
					</td>
					<td>
						<input
							class="mono"
							aria-label="Value of {v.name}"
							spellcheck="false"
							type={v.secret ? 'password' : 'text'}
							autocomplete="off"
							placeholder={v.secret && v.from !== undefined
								? '(unchanged)'
								: ''}
							bind:value={v.value}
						/>
					</td>
					<td class="center">
						<input
							type="checkbox"
							aria-label="{v.name} is a secret"
							bind:checked={v.secret}
						/>
					</td>
					<td>
						<Button onclick={() => vars.splice(i, 1)}>Remove</Button
						>
					</td>
				</tr>
			{:else}
				<tr>
					<td colspan="4" class="empty">No variables yet.</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
{#if error}<p class="error">{error}</p>{/if}
<div class="row-buttons">
	<Button onclick={add}>Add Variable</Button>
	<Button primary onclick={save} disabled={busy}>
		{saved ? 'Saved' : 'Save'}
	</Button>
</div>
<p class="hint">
	The app gets them with <code>HOST</code> and <code>PORT</code>: saving
	applies right away and restarts it.
</p>

<style lang="scss">
	@use 'styles/win' as *;

	.mono {
		font-family: $mono;
		width: 100%;
	}

	td {
		padding: 1px 4px;
	}

	.center {
		text-align: center;
	}

	.hint {
		margin: 8px 0 0;
	}
</style>
