<script lang="ts">
	import { get, post } from '@/lib/api';
	import { mail } from '@/lib/mail.svelte';
	import { message } from '@/lib/dialogs.svelte';
	import { session } from '@/lib/session.svelte';
	import Button from '../win/Button.svelte';
	import Form from '../win/Form.svelte';
	import DnsTable from '../DnsTable.svelte';

	// how postfix sends the mails of genco and the sites

	let mode = $state('direct');
	let relayHost = $state('');
	let relayPort = $state(587);
	let relayUser = $state('');
	let relayPassword = $state('');
	let hasPassword = $state(false);
	let sender = $state('');
	let error = $state<string | null>(null);
	let testTo = $state(session.user?.email ?? '');
	let testResult = $state<string | null>(null);
	// whether mail servers answer on port 25, null while it checks
	let port25 = $state<{ blocked: boolean } | null>(null);

	async function checkPort25(again = false) {
		port25 = null;
		try {
			port25 = await get(
				'/settings/mail/port25' + (again ? '?again=true' : ''),
			);
		} catch (e: any) {
			error = e.message;
		}
	}
	checkPort25();

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
			sender = s.sender;
		})
		.catch(e => (error = e.message));

	async function save() {
		// a stolen session mustn't send the mails through its own relay
		const auth = await message.password(
			'Mail Settings',
			'change how mail is sent',
		);
		if (auth === null) return false;
		await post('/settings/mail', {
			mode,
			relayHost,
			relayPort: Number(relayPort),
			relayUser,
			relayPassword,
			sender,
			...auth,
		});
		if (relayPassword) hasPassword = true;
		relayPassword = '';
		mail.mode = mode;
	}

	async function test() {
		testResult = 'Sending and waiting for postfix…';
		try {
			testResult = await post('/settings/mail/test', { to: testTo });
		} catch (e: any) {
			testResult = e.message;
		}
	}
</script>

<fieldset>
	<legend>Mail</legend>
	<p>
		genco and the sites send mail through postfix on this server. Deletions
		are confirmed by mail, so it has to arrive; without mail they are
		confirmed with your password.
	</p>
	<div class="modes">
		<label>
			<input type="radio" bind:group={mode} value="direct" />
			Deliver directly (needs SPF for the domain and a PTR record)
		</label>
		{#if port25?.blocked}
			<div class="port25">
				<span class="error">
					Outgoing port 25 seems blocked (common at cloud hosts like
					Infomaniak): use an SMTP relay.
				</span>
				<Button onclick={() => checkPort25(true)}>Check Again</Button>
			</div>
		{/if}
		<label>
			<input type="radio" bind:group={mode} value="relay" />
			Through an SMTP relay (Infomaniak, Amazon SES, …)
		</label>
		<label>
			<input type="radio" bind:group={mode} value="off" />
			No mail (port 25 blocked and no relay)
		</label>
	</div>
	{#if mode === 'off'}
		<p class="hint">
			genco sends no mail: alerts only show in the panel, there are no
			invites and no new mail domains.
		</p>
	{/if}
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
				<label for="m-sender">Sender:</label>
				<input
					id="m-sender"
					type="email"
					placeholder="genco@ the panel's name"
					bind:value={sender}
				/>
			{/if}
		</Form>
	{/if}
</fieldset>

{#if mode === 'direct'}
	<fieldset>
		<legend>DNS for genco's mails</legend>
		<p class="hint">
			genco sends its mails (alerts, confirmations, invites) as genco@ the
			panel's name, signed with its own DKIM key. Set these at that name's
			DNS host.
		</p>
		<DnsTable path="/settings/mail/dns" />
	</fieldset>
{:else if mode === 'relay'}
	<p class="hint">
		The relay sends genco's mails; set up SPF/DKIM as the relay's provider
		asks. The sender (From and bounce address) is one the relay may send as,
		empty is genco@ the panel's name.
	</p>
{/if}

{#if mail.mode !== 'off'}
	<fieldset>
		<legend>Test</legend>
		<div class="test">
			<input
				type="email"
				aria-label="Send the test to"
				bind:value={testTo}
			/>
			<Button onclick={test}>Send Test Mail</Button>
		</div>
		{#if testResult}<p class="result selectable">{testResult}</p>{/if}
	</fieldset>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 8px;
	}

	// the DNS table scrolls instead of widening the dialog
	fieldset {
		min-width: 0;
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

	.port25 {
		display: flex;
		gap: 6px;
		align-items: center;
		margin-left: 20px;
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
