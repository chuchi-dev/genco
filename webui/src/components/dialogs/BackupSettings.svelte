<script lang="ts">
	import {
		formatBytes,
		formatTime,
		get,
		post,
		type BackupSettings,
	} from '@/lib/api';
	import { message } from '@/lib/dialogs.svelte';
	import Button from '../win/Button.svelte';
	import Form from '../win/Form.svelte';
	import OneTimeSecret from '../OneTimeSecret.svelte';

	// the copy of the backups off the server: Backblaze B2 or any
	// S3-compatible storage

	let settings = $state<BackupSettings | null>(null);
	let provider = $state<'off' | 'b2' | 's3'>('off');
	let bucket = $state('');
	let path = $state('');
	let endpoint = $state('');
	let region = $state('');
	let keyId = $state('');
	let secret = $state('');
	let repoPassword = $state('');
	let keepDaily = $state(14);
	let keepWeekly = $state(8);
	let keepMonthly = $state(12);
	let limitUpload = $state<number | null>(null);
	let error = $state<string | null>(null);
	// the repository password genco made, shown once
	let generated = $state<string | null>(null);
	let testResult = $state<string | null>(null);
	let testing = $state(false);

	async function load() {
		const s: BackupSettings = await get('/settings/backup');
		settings = s;
		const r = s.remote;
		provider = r?.provider ?? 'off';
		bucket = r?.bucket ?? '';
		path = r?.path ?? '';
		endpoint = r?.endpoint ?? '';
		region = r?.region ?? '';
		keyId = r?.keyId ?? '';
		keepDaily = r?.keep.daily ?? 14;
		keepWeekly = r?.keep.weekly ?? 8;
		keepMonthly = r?.keep.monthly ?? 12;
		limitUpload = r?.limitUpload ?? null;
	}
	load().catch(e => (error = e.message));

	async function save() {
		// a stolen session mustn't send all data somewhere else
		const auth = await message.password(
			'Remote Backup',
			'change where the backups go',
		);
		if (auth === null) return false;
		generated = await post('/settings/backup', {
			provider,
			bucket,
			path,
			endpoint,
			region,
			keyId,
			secret,
			repoPassword,
			keep: {
				daily: Number(keepDaily),
				weekly: Number(keepWeekly),
				monthly: Number(keepMonthly),
			},
			limitUpload: limitUpload ? Number(limitUpload) : null,
			...auth,
		});
		secret = '';
		repoPassword = '';
		testResult = null;
		await load();
	}

	// made on the agent's request, an admin sees it once
	async function showNewPassword() {
		const auth = await message.password(
			'Remote Backup',
			"see the remote repository's password",
		);
		if (auth === null) return;
		try {
			generated = await post('/settings/backup/new-password', auth);
			await load();
		} catch (e: any) {
			error = e.message;
		}
	}

	async function test() {
		testing = true;
		testResult = 'Connecting…';
		try {
			testResult = await post('/settings/backup/test');
		} catch (e: any) {
			testResult = e.message;
		} finally {
			testing = false;
		}
	}

	const saved = $derived(settings?.remote ?? null);
	const copy = $derived(settings?.state ?? null);
</script>

<fieldset>
	<legend>Remote Copy</legend>
	<p>
		After each night's backup genco copies the new snapshots to a bucket off
		the server, which keeps its own history. If the server is lost, the
		groups and genco itself come back from there.
	</p>
	<div class="modes">
		<label>
			<input type="radio" bind:group={provider} value="off" />
			Only on this server
		</label>
		<label>
			<input type="radio" bind:group={provider} value="b2" />
			Backblaze B2
		</label>
		<label>
			<input type="radio" bind:group={provider} value="s3" />
			S3-compatible storage (Amazon S3, Wasabi, Infomaniak, …)
		</label>
	</div>
	{#if error}<p class="error">{error}</p>{/if}
	{#if settings}
		<Form onsubmit={save} ok="Save" done="Saved">
			{#if provider !== 'off'}
				{#if provider === 's3'}
					<label for="b-endpoint">Endpoint:</label>
					<input
						id="b-endpoint"
						placeholder="https://s3.eu-central-1.amazonaws.com"
						bind:value={endpoint}
					/>
					<label for="b-region">Region:</label>
					<input
						id="b-region"
						placeholder="(optional)"
						bind:value={region}
					/>
				{/if}
				<label for="b-bucket">Bucket:</label>
				<input id="b-bucket" bind:value={bucket} />
				<label for="b-path">Path:</label>
				<input
					id="b-path"
					placeholder={settings.defaultPath}
					bind:value={path}
				/>
				<label for="b-key"
					>{provider === 'b2' ? 'keyID' : 'Access key'}:</label
				>
				<input id="b-key" autocomplete="off" bind:value={keyId} />
				<label for="b-secret">
					{provider === 'b2' ? 'applicationKey' : 'Secret key'}:
				</label>
				<input
					id="b-secret"
					type="password"
					autocomplete="new-password"
					placeholder={saved ? '(unchanged)' : ''}
					bind:value={secret}
				/>
				<label for="b-repo">Repository password:</label>
				<input
					id="b-repo"
					type="password"
					autocomplete="new-password"
					placeholder={saved ? '(unchanged)' : '(genco makes one)'}
					bind:value={repoPassword}
				/>
				<label for="b-daily">Keep:</label>
				<div class="keep">
					<input
						id="b-daily"
						type="number"
						min="0"
						bind:value={keepDaily}
					/>
					daily
					<input
						type="number"
						min="0"
						aria-label="weekly"
						bind:value={keepWeekly}
					/>
					weekly
					<input
						type="number"
						min="0"
						aria-label="monthly"
						bind:value={keepMonthly}
					/>
					monthly
				</div>
				<label for="b-limit">Upload limit:</label>
				<div class="keep">
					<input
						id="b-limit"
						type="number"
						min="0"
						placeholder="none"
						bind:value={limitUpload}
					/>
					KiB/s
				</div>
			{/if}
		</Form>
		{#if provider !== 'off'}
			<p class="hint">
				Fill in the repository password only when this server takes over
				an existing remote repository (a new server after the old one
				was lost). The bucket must exist; the key needs read, write and
				delete rights on it.
			</p>
		{/if}
	{/if}
</fieldset>

{#if settings?.newPassword && !generated}
	<fieldset>
		<legend>Repository Password</legend>
		<p class="error">
			genco made a new password for the remote repository when an admin
			confirmed the agent's request. Look at it once and keep it outside
			this server: without it the remote copy can't be read when the
			server is lost.
		</p>
		<div class="row-buttons">
			<Button onclick={showNewPassword}>Show Password</Button>
		</div>
	</fieldset>
{/if}

{#if generated}
	<OneTimeSecret
		title="Repository Password"
		value={generated}
		label="The remote repository's password, shown only this once:"
		onclose={() => (generated = null)}
	>
		<p class="error">
			Keep it outside this server, e.g. in your password manager, with the
			bucket's key. Without it the remote copy can't be read when the
			server is lost.
		</p>
	</OneTimeSecret>
{/if}

{#if saved}
	<fieldset>
		<legend>Status</legend>
		<p class="selectable"><code>{saved.location}</code></p>
		<p>
			Last copy: {copy?.lastOk ? formatTime(copy.lastOk) : 'never'}
			{#if copy?.size !== null && copy?.size !== undefined}
				· {formatBytes(copy.size)} in the bucket
			{/if}
		</p>
		{#if copy?.lastError && copy.lastErrorAt}
			<p class="error">
				{formatTime(copy.lastErrorAt)}: {copy.lastError}
			</p>
		{/if}
		<div class="row-buttons">
			<Button onclick={test} disabled={testing}>Test Connection</Button>
		</div>
		{#if testResult}<p class="result selectable">{testResult}</p>{/if}
	</fieldset>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 0 0 8px;
	}

	fieldset {
		min-width: 0;
		margin-bottom: 10px;
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
	}

	.keep {
		display: flex;
		gap: 6px;
		align-items: center;

		input {
			width: 60px;
		}
	}

	.result {
		margin: 8px 0 0;
	}
</style>
