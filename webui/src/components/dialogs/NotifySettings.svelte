<script lang="ts">
	import { get, post } from '@/lib/api';
	import { dialogs } from '@/lib/dialogs.svelte';
	import Button from '../win/Button.svelte';
	import Form from '../win/Form.svelte';

	// where reports that need attention go: the admins' mail; the webhooks
	// are the AI agents'
	let mail = $state(true);
	let error = $state<string | null>(null);

	// the form only once the settings are there: a Save before would
	// write the defaults over them
	let loaded = $state(false);

	get('/settings/notify')
		.then(s => {
			loaded = true;
			mail = s.mail;
		})
		.catch(e => (error = e.message));

	async function save() {
		await post('/settings/notify', { mail });
	}
</script>

<p>
	Reports that need attention (alerts, the health check, the update round, the
	security review, failed updates) go here. Deletion confirmations and invites
	always go by mail.
</p>
{#if error}<p class="error">{error}</p>{/if}
{#if loaded}
	<Form onsubmit={save} ok="Save" done="Saved">
		<span></span>
		<label class="check">
			<input type="checkbox" bind:checked={mail} />
			Mail every admin
		</label>
	</Form>
{/if}

<fieldset>
	<legend>Webhooks</legend>
	<div class="row">
		<span class="hint">
			Another AI agent gets the reports by webhook, set up in Start → AI
			Agents.
		</span>
		<Button onclick={() => (dialogs.open = 'agents')}>AI Agents…</Button>
	</div>
</fieldset>

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 8px;
	}

	.check {
		display: flex;
		gap: 6px;
		align-items: center;

		input {
			box-shadow: none;
			padding: 0;
		}
	}

	fieldset {
		margin-top: 10px;
	}

	.row {
		display: flex;
		gap: 6px;
		align-items: center;

		.hint {
			flex: 1;
		}
	}
</style>
