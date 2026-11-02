<script lang="ts">
	import { get, post } from '@/lib/api';
	import Button from '../win/Button.svelte';
	import Form from '../win/Form.svelte';

	// where reports that need attention go: the admins' mail, a webhook
	let mail = $state(true);
	let webhookUrl = $state('');
	let webhookToken = $state('');
	let webhookAll = $state(false);
	let hasToken = $state(false);
	let savedUrl = $state('');
	let error = $state<string | null>(null);
	let testResult = $state<string | null>(null);

	// the form only once the settings are there: a Save before would
	// write the defaults over them
	let loaded = $state(false);

	get('/settings/notify')
		.then(s => {
			loaded = true;
			mail = s.mail;
			webhookUrl = savedUrl = s.webhookUrl;
			webhookAll = s.webhookAll;
			hasToken = s.hasToken;
		})
		.catch(e => (error = e.message));

	async function save() {
		await post('/settings/notify', {
			mail,
			webhookUrl,
			webhookToken,
			webhookAll,
		});
		// another address drops the old token
		hasToken = !!webhookToken || (hasToken && webhookUrl === savedUrl);
		savedUrl = webhookUrl.trim();
		webhookToken = '';
	}

	async function test() {
		testResult = 'Sending…';
		try {
			await post('/settings/notify/test', {});
			testResult = 'The webhook took it.';
		} catch (e: any) {
			testResult = e.message;
		}
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
		<label for="n-url">Webhook:</label>
		<input
			id="n-url"
			type="url"
			placeholder="https://… (empty for none)"
			bind:value={webhookUrl}
		/>
		<label for="n-token">Token:</label>
		<input
			id="n-token"
			type="password"
			autocomplete="new-password"
			placeholder={hasToken && webhookUrl.trim() === savedUrl
				? '(unchanged)'
				: 'sent as Authorization: Bearer'}
			bind:value={webhookToken}
		/>
		<span></span>
		<label class="check">
			<input type="checkbox" bind:checked={webhookAll} />
			Send the OK reports to the webhook too
		</label>
	</Form>
{/if}

<fieldset>
	<legend>Test</legend>
	<div class="test">
		<span class="hint">A test to the saved webhook.</span>
		<Button onclick={test} disabled={!savedUrl}>Send Test</Button>
	</div>
	{#if testResult}<p class="result selectable">{testResult}</p>{/if}
	<p class="hint">
		The webhook gets a POST with JSON: <code>server</code>,
		<code>status</code> (ok or attention), <code>kind</code>,
		<code>title</code>, <code>text</code> and <code>url</code>.
	</p>
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

	.test {
		display: flex;
		gap: 6px;
		align-items: center;

		.hint {
			flex: 1;
		}
	}

	.result {
		margin: 8px 0;
	}
</style>
