<script lang="ts">
	import { get, post } from '@/lib/api';
	import Form from '../win/Form.svelte';

	// the server agent's model; which ones work depends on the keys in
	// /etc/genco/config.toml
	type Choice = { id: string; name: string; available: boolean };

	let model = $state('');
	let models = $state<Choice[]>([]);
	let error = $state<string | null>(null);

	get('/settings/model')
		.then(s => {
			model = s.model;
			models = s.models;
		})
		.catch(e => (error = e.message));

	async function save() {
		await post('/settings/model', { model });
	}
</script>

<p>
	The model the server agent works with, in the chat, the routines and the
	alerts. Claude goes directly to Anthropic with an Anthropic key, the others
	need an OpenRouter key.
</p>
{#if error}<p class="error">{error}</p>{/if}
{#if models.length}
	<Form onsubmit={save} ok="Save" done="Saved">
		<label for="ag-model">Model:</label>
		<select id="ag-model" bind:value={model}>
			{#each models as m (m.id)}
				<option value={m.id} disabled={!m.available}>
					{m.name}{m.available ? '' : ' (needs an OpenRouter key)'}
				</option>
			{/each}
			{#if !models.some(m => m.id === model)}
				<option value={model}>{model} (from the config)</option>
			{/if}
		</select>
	</Form>
	<p class="hint">
		Keys go into <code>/etc/genco/config.toml</code> (<code
			>anthropic-key</code
		>,
		<code>openrouter-key</code>), see <code>genco docs operations</code>.
	</p>
{/if}

<style lang="scss">
	p {
		margin: 0 0 8px;
	}

	.hint {
		margin-top: 10px;
	}
</style>
