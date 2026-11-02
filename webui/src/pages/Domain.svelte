<script lang="ts" module>
	import { tree } from '@/lib/tree.svelte';

	export function loadProps({ id }: { id: string }) {
		if (!tree.domain(Number(id))) throw new Error('This domain is gone.');
	}
</script>

<script lang="ts">
	import { KIND_NAMES, get, post, type ConfigFile, newest } from '@/lib/api';
	import Form from '@/components/win/Form.svelte';
	import Button from '@/components/win/Button.svelte';
	import { requestDelete } from '@/lib/dialogs.svelte';
	import { go } from '@/lib/router.svelte';
	import ConfigFiles from '@/components/ConfigFiles.svelte';
	import Usage from '@/components/Usage.svelte';
	import DomainFields from '@/components/dialogs/DomainFields.svelte';
	import AppEnv from '@/components/AppEnv.svelte';

	let { id }: { id: string } = $props();

	// svelte-ignore state_referenced_locally
	const start = tree.domain(Number(id))!;
	const domain = $derived(tree.domain(Number(id)) ?? start);
	const group = $derived(tree.group(domain.groupId)!);
	const app = $derived(domain.kind === 'node' || domain.kind === 'rust');

	let kind = $state(start.kind);
	let phpVersion = $state(start.phpVersion);
	let root = $state(start.root);
	let command = $state(start.command);
	let publicDir = $state(start.publicDir);
	let upstream = $state(start.upstream);
	let www = $state(start.www);
	let nginxExtra = $state(start.nginxExtra);
	let files = $state<ConfigFile[] | null>(null);

	// the domain's files, again when the tree changes (e.g. after an apply)
	const loadFiles = newest();
	$effect(() => {
		void tree.clients;
		const name = domain.name;
		loadFiles(get(`/groups/${group.id}/config`), (all: ConfigFile[]) => {
			files = all.filter(
				f =>
					f.path.endsWith(`/${name}.conf`) ||
					f.path.endsWith(`/www.${name}.conf`) ||
					f.path.endsWith(`/${name}.sh`) ||
					f.path.endsWith(`-${name}.service`),
			);
		});
	});

	async function save() {
		await post('/domains/' + domain.id, {
			kind,
			phpVersion,
			root,
			command,
			publicDir,
			upstream,
			www,
			nginxExtra,
		});
	}
</script>

<dl class="props">
	<dt>Domain:</dt>
	<dd>
		<a href="https://{domain.name}" target="_blank" rel="noreferrer">
			{domain.name}
		</a>
	</dd>
	<dt>Group:</dt>
	<dd><a href="/groups/{group.id}">{group.name}</a></dd>
	<dt>Serves:</dt>
	<dd>
		{#if domain.kind === 'proxy'}
			Forwards to <code>{domain.upstream}</code>
		{:else}
			{KIND_NAMES[domain.kind]}{domain.phpVersion
				? ' ' + domain.phpVersion
				: ''}
			{app ? 'in' : 'from'}
			<code>{group.dir}/{domain.root}</code>
		{/if}
	</dd>
	{#if domain.publicDir}
		<dt>Public files:</dt>
		<dd>
			<a href="/groups/{group.id}/files/{domain.publicDir}">
				<code>{group.dir}/{domain.publicDir}</code>
			</a>
			(served by nginx)
		</dd>
	{/if}
	{#if app}
		<dt>Port:</dt>
		<dd>
			<code>127.0.0.1:{20000 + domain.id}</code>
			(<code>PORT</code>
			and
			<code>HOST</code>
			are set)
		</dd>
	{/if}
</dl>

<Form onsubmit={save} ok="Save" done="Saved">
	<DomainFields
		bind:kind
		bind:phpVersion
		bind:root
		bind:command
		bind:publicDir
		bind:upstream
		bind:www
		name={domain.name}
	/>
	<label for="d-extra">Extra nginx:</label>
	<textarea
		id="d-extra"
		class="mono"
		rows="3"
		placeholder="added to the server block, e.g. add_header …;"
		bind:value={nginxExtra}
	></textarea>
</Form>

<div class="row-buttons">
	<Button onclick={() => go(`/groups/${group.id}/files/${domain.root}`)}>
		Open Folder
	</Button>
	<Button
		onclick={() =>
			requestDelete(`/domains/${domain.id}`, `the domain ${domain.name}`)}
	>
		Delete Domain…
	</Button>
</div>

{#if app}
	<hr class="groove" />

	<h4>Environment</h4>
	{#key domain.id}
		<AppEnv domainId={domain.id} />
	{/key}
{/if}

<hr class="groove" />

<h4>Usage</h4>
<Usage scope="domain:{domain.id}" {app} />

<hr class="groove" />

{#if files}
	<ConfigFiles {files} />
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.mono {
		font-family: $mono;
	}
</style>
