<script lang="ts">
	import { get, post } from '@/lib/api';
	import { session } from '@/lib/session.svelte';
	import Button from '../win/Button.svelte';
	import Form from '../win/Form.svelte';

	// how postfix sends the mails of genco and the sites

	let mode = $state('direct');
	let relayHost = $state('');
	let relayPort = $state(587);
	let relayUser = $state('');
	let relayPassword = $state('');
	let hasPassword = $state(false);
	let error = $state<string | null>(null);
	let testTo = $state(session.user?.email ?? '');
	let testResult = $state<string | null>(null);

	// the form only once the settings are there: a Save before would
	// write the defaults over them
	let loaded = $state(false);

	get('/settings/mail')
		.then(s => {
			loaded = true;
			mode = s.mode;
			relayHost = s.relayHost;
			relayPort = s.relayPort || 587;
			relayUser = s.relayUser;
			hasPassword = s.hasPassword;
		})
		.catch(e => (error = e.message));

	async function save() {
		await post('/settings/mail', {
			mode,
			relayHost,
			relayPort: Number(relayPort),
			relayUser,
			relayPassword,
		});
		if (relayPassword) hasPassword = true;
		relayPassword = '';
	}

	async function test() {
		testResult = 'Sending…';
		try {
			await post('/settings/mail/test', { to: testTo });
			testResult = `Sent to ${testTo}, check the inbox (and the spam folder).`;
		} catch (e: any) {
			testResult = e.message;
		}
	}
</script>

<fieldset>
	<legend>Mail</legend>
	<p>
		genco and the sites send mail through postfix on this server. Deletions
		are confirmed by mail, so it has to arrive.
	</p>
	<div class="modes">
		<label>
			<input type="radio" bind:group={mode} value="direct" />
			Deliver directly (needs SPF for the domain and a PTR record)
		</label>
		<label>
			<input type="radio" bind:group={mode} value="relay" />
			Through an SMTP relay (Infomaniak, Amazon SES, …)
		</label>
	</div>
	{#if error}<p class="error">{error}</p>{/if}
	{#if loaded}
		<Form onsubmit={save} ok="Save" done="Saved">
			{#if mode === 'relay'}
				<label for="m-host">Host:</label>
				<input
					id="m-host"
					placeholder="mail.infomaniak.com"
					bind:value={relayHost}
				/>
				<label for="m-port">Port:</label>
				<input id="m-port" type="number" bind:value={relayPort} />
				<label for="m-user">User:</label>
				<input id="m-user" autocomplete="off" bind:value={relayUser} />
				<label for="m-pw">Password:</label>
				<input
					id="m-pw"
					type="password"
					autocomplete="new-password"
					placeholder={hasPassword ? '(unchanged)' : ''}
					bind:value={relayPassword}
				/>
			{/if}
		</Form>
	{/if}
</fieldset>

<fieldset>
	<legend>Test</legend>
	<div class="test">
		<input type="email" aria-label="Send the test to" bind:value={testTo} />
		<Button onclick={test}>Send Test Mail</Button>
	</div>
	{#if testResult}<p class="result selectable">{testResult}</p>{/if}
</fieldset>

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 8px;
	}

	.modes {
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin-bottom: 10px;

		label {
			display: flex;
			gap: 6px;
			align-items: center;
		}

		input {
			box-shadow: none;
			padding: 0;
		}
	}

	.test {
		display: flex;
		gap: 6px;

		input {
			flex: 1;
		}
	}

	.result {
		margin: 8px 0 0;
	}
</style>
