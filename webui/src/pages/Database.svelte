<script lang="ts" module>
	import { tree } from '@/lib/tree.svelte';

	export function loadProps({ id }: { id: string }) {
		if (!tree.database(Number(id)))
			throw new Error('This database is gone.');
	}
</script>

<script lang="ts">
	import { poll } from '@/lib/poll';
	import {
		download,
		formatBytes,
		get,
		newest,
		type DatabaseInfo,
	} from '@/lib/api';
	import { message, requestDelete } from '@/lib/dialogs.svelte';
	import { putFile } from '@/lib/upload';
	import LogView from '@/components/LogView.svelte';
	import Button from '@/components/win/Button.svelte';

	function downloadDump() {
		download(`/databases/${encodeURIComponent(database.name)}/dump`).catch(
			e => message.show('Can’t Download', e.message),
		);
	}

	let { id }: { id: string } = $props();

	// what it was when the page opened, a confirmed deletion removes it
	// from the tree while the page still shows
	// svelte-ignore state_referenced_locally
	const start = tree.database(Number(id))!;
	const database = $derived(tree.database(Number(id)) ?? start);
	// svelte-ignore state_referenced_locally
	const startGroup = tree.group(start.groupId)!;
	const group = $derived(tree.group(database.groupId) ?? startGroup);

	// an import: its upload, then the job on the server
	let picker: HTMLInputElement | undefined = $state();
	let uploading = $state<number | null>(null);
	let importId = $state<number | null>(null);
	let importStatus = $state<string | null>(null);
	let importError = $state<string | null>(null);

	async function importFile(file: File) {
		const yes = await message.confirm(
			'Import Database',
			`Import ${file.name} into ${database.name}? It replaces what the database has; the current content goes to the trash first.`,
		);
		if (!yes) return;
		importError = null;
		importId = null;
		uploading = 0;
		try {
			const id = await putFile(
				`/databases/${database.id}/import`,
				file,
				pct => (uploading = pct),
			);
			importId = Number(id);
			importStatus = 'running';
		} catch (e: any) {
			importError = e.message;
		}
		uploading = null;
	}

	// the job's state every two seconds while it runs
	// one request after the other, an answer after the cleanup is dropped
	$effect(() => {
		const id = importId;
		if (id === null || importStatus !== 'running') return;
		return poll(async stopped => {
			try {
				const imp = await get(`/imports/${id}`);
				if (stopped() || imp.status === 'running') return true;
				importStatus = imp.status;
				// the size and tables changed
				reloadInfo++;
			} catch (e: any) {
				if (!stopped()) importError = e.message;
			}
			return false;
		}, 2000);
	});

	let info = $state<DatabaseInfo | null>(null);
	let loadError = $state<string | null>(null);
	const load = newest();
	let reloadInfo = $state(0);

	// primitives: the tree gives new objects on every reload, the info
	// would load again each time
	const name = $derived(database.name);
	const groupId = $derived(group.id);

	$effect(() => {
		void reloadInfo;
		const n = name;
		load(
			get(`/groups/${groupId}/databases`),
			(infos: DatabaseInfo[]) => {
				info = infos.find(i => i.name === n) ?? null;
				loadError = null;
			},
			e => (loadError = e.message),
		);
	});
</script>

<dl class="props">
	<dt>Database:</dt>
	<dd>{database.name}</dd>
	<dt>Group:</dt>
	<dd><a href="/groups/{group.id}">{group.name}</a></dd>
	<dt>Server:</dt>
	{#if database.engine === 'postgres'}
		<dd>
			PostgreSQL on the local socket:
			<code>postgresql:///{database.name}?host=/run/postgresql</code>
		</dd>
	{:else}
		<dd>MariaDB on the local socket</dd>
	{/if}
	<dt>User:</dt>
	<dd>
		<code>{group.user}</code>, no password: it logs in as the group's Linux
		user
	</dd>
	{#if info}
		<dt>Status:</dt>
		<dd>{info.exists ? 'exists' : 'not created yet, apply the changes'}</dd>
		{#if info.exists}
			<dt>Size:</dt>
			<dd>{formatBytes(info.sizeBytes)}</dd>
			<dt>Tables:</dt>
			<dd>{info.tables}</dd>
		{/if}
	{/if}
</dl>
{#if loadError}<p class="error">{loadError}</p>{/if}

<p class="hint">
	The download is a gzipped dump. A Craft database (MariaDB) comes without its
	personal data (form submissions, orders, customers, sessions), for
	development copies. Scripts download with a token of Start → API Tokens.
</p>

<div class="row-buttons">
	<Button onclick={downloadDump}>Download…</Button>
	<Button
		onclick={() => picker?.click()}
		disabled={uploading !== null || importStatus === 'running'}
	>
		Import…
	</Button>
	<input
		bind:this={picker}
		type="file"
		accept=".sql,.gz,.zip"
		hidden
		onchange={e => {
			const input = e.target as HTMLInputElement;
			const file = input.files?.[0];
			input.value = '';
			if (file) importFile(file);
		}}
	/>
	<Button
		onclick={() =>
			requestDelete(
				`/databases/${database.id}`,
				`the database ${database.name}`,
			)}
	>
		Delete Database…
	</Button>
</div>

{#if uploading !== null}
	<p class="status">Uploading… {uploading}%</p>
{/if}
{#if importError}<p class="error">{importError}</p>{/if}
{#if importId !== null}
	<p class="status {importStatus}">
		{importStatus === 'running'
			? 'Importing…'
			: importStatus === 'done'
				? 'Imported. A Craft site may need "php craft up" and its caches cleared.'
				: 'The import failed and may be half done. The log names the file in the trash with the old content.'}
	</p>
	<LogView
		url="/imports/{importId}/log"
		live={importStatus === 'running'}
		height={160}
	/>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.hint {
		margin: 0 0 4px;
		color: $hint;
	}
</style>
