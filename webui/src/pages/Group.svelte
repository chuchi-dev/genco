<script lang="ts" module>
	import { objectPage } from '@/lib/router.svelte';
	import { tree } from '@/lib/tree.svelte';

	const page = objectPage(id => tree.group(id), 'This group is gone.');
	export const loadProps = page.loadProps;
</script>

<script lang="ts">
	import { GROUP_TABS } from '@/lib/groupTabs';
	import {
		KIND_NAMES,
		applyErrors,
		formatTime,
		get,
		liftQuarantine,
		newest,
		quarantineGroup,
		saveGroup,
		type ApplyReport,
		type ConfigFile,
	} from '@/lib/api';
	import { dialogs, message, requestDelete } from '@/lib/dialogs.svelte';
	import { go } from '@/lib/router.svelte';
	import Button from '@/components/win/Button.svelte';
	import Form from '@/components/win/Form.svelte';
	import Tabs from '@/components/win/Tabs.svelte';
	import ConfigFiles from '@/components/ConfigFiles.svelte';
	import Deploys from '@/components/Deploys.svelte';
	import Snapshots from '@/components/Snapshots.svelte';
	import Usage from '@/components/Usage.svelte';
	import Files from '@/components/Files.svelte';
	import GroupServices from '@/components/GroupServices.svelte';
	import GroupProcs from '@/components/GroupProcs.svelte';
	import GroupDatabases from '@/components/GroupDatabases.svelte';
	import GroupSmtp from '@/components/GroupSmtp.svelte';
	import GroupLimits from '@/components/GroupLimits.svelte';

	let {
		id,
		tab: tabProp,
		path,
		worker,
	}: {
		id: string;
		tab?: string;
		path?: string;
		/** a worker of the tree, selected in the services tab */
		worker?: string;
	} = $props();

	const TABS = GROUP_TABS;
	// /groups/1/files/shared/craft opens the files tab in that folder
	const tab = $derived(
		path !== undefined
			? 'files'
			: TABS.some(t => t.id === tabProp)
				? tabProp!
				: 'general',
	);
	const filesPath = $derived(
		(path ?? '')
			.split('/')
			.filter(Boolean)
			.map(s => {
				try {
					return decodeURIComponent(s);
				} catch (_e) {
					return s;
				}
			})
			.join('/'),
	);

	const live = page.live(() => id);
	const group = $derived(live.current);

	let clientId = $state(live.current.clientId);
	let notes = $state(live.current.notes);
	// as text in the fields, numbers when saved
	let php = $state({
		uploadMb: String(live.current.php.uploadMb),
		memoryMb: String(live.current.php.memoryMb),
		maxExecutionS: String(live.current.php.maxExecutionS),
		maxInputVars: String(live.current.php.maxInputVars),
	});
	const hasPhp = $derived(group.domains.some(d => d.kind === 'php'));

	let files = $state<ConfigFile[] | null>(null);
	let loadError = $state<string | null>(null);

	// the config, again when the tree changes (e.g. after an apply)
	const load = newest();
	$effect(() => {
		void tree.clients;
		if (tab !== 'config') return;
		load(
			get(`/groups/${group.id}/config`),
			v => ((files = v), (loadError = null)),
			e => (loadError = e.message),
		);
	});

	/** Puts the group in quarantine or lifts it, apply runs right away. */
	async function quarantine() {
		let report: ApplyReport;
		try {
			if (group.quarantine) {
				const yes = await message.confirm(
					'Lift Quarantine',
					`Start the services of ${group.name} again and serve its sites? Only when the cause is removed.`,
				);
				if (!yes) return;
				report = await liftQuarantine(group);
			} else {
				const reason = await message.ask(
					'Quarantine',
					`Stop every service of ${group.name}, its sites answer 503, its SMTP login and deploys are off. Files, databases and mailboxes stay. Why?`,
				);
				if (!reason) return;
				report = await quarantineGroup(group, reason);
			}
			await tree.load();
			if (!report.ok) {
				message.show(
					'Quarantine',
					'Applied with errors:\n' + applyErrors(report),
				);
			}
		} catch (e: any) {
			message.show('Quarantine', e.message);
		}
	}

	async function save() {
		const n = (v: string) => {
			const x = Number(v);
			if (!Number.isInteger(x) || x <= 0)
				throw new Error('The PHP settings are whole numbers');
			return x;
		};
		await saveGroup(group, {
			clientId,
			notes,
			php: {
				uploadMb: n(php.uploadMb),
				memoryMb: n(php.memoryMb),
				maxExecutionS: n(php.maxExecutionS),
				maxInputVars: n(php.maxInputVars),
			},
		});
	}
</script>

<Tabs
	tabs={TABS}
	selected={tab}
	onselect={t => go(`/groups/${group.id}/${t}`, true)}
>
	{#if group.quarantine}
		<p class="error">
			In quarantine since {formatTime(group.quarantine.at, true)}
			({group.quarantine.by}): {group.quarantine.reason}. Its services are
			stopped, its sites answer 503, its SMTP login and deploys are off.
		</p>
	{/if}
	{#if tab === 'general'}
		<dl class="props">
			<dt>Linux user:</dt>
			<dd><code>{group.user}</code></dd>
			<dt>Files:</dt>
			<dd><code>{group.dir}</code></dd>
		</dl>
		<Form onsubmit={save} ok="Save" done="Saved">
			<label for="gv-client">Client:</label>
			<select id="gv-client" bind:value={clientId}>
				{#each tree.clients as c (c.id)}
					<option value={c.id}>{c.name}</option>
				{/each}
			</select>
			<label for="gv-notes">Notes:</label>
			<textarea id="gv-notes" rows="4" bind:value={notes}></textarea>
			{#if hasPhp}
				<label for="gv-upload">PHP uploads:</label>
				<span class="unit">
					<input
						id="gv-upload"
						inputmode="numeric"
						bind:value={php.uploadMb}
					/>
					MB, at most per file
				</span>
				<label for="gv-memory">PHP memory:</label>
				<span class="unit">
					<input
						id="gv-memory"
						inputmode="numeric"
						bind:value={php.memoryMb}
					/>
					MB per request
				</span>
				<label for="gv-time">PHP time:</label>
				<span class="unit">
					<input
						id="gv-time"
						inputmode="numeric"
						bind:value={php.maxExecutionS}
					/>
					seconds per request
				</span>
				<label for="gv-vars">PHP input vars:</label>
				<span class="unit">
					<input
						id="gv-vars"
						inputmode="numeric"
						bind:value={php.maxInputVars}
					/>
					form fields per request
				</span>
			{/if}
		</Form>
		{#key group.id}
			<GroupLimits {group} />
		{/key}
		{#if group.mailDomains.length}
			<hr class="groove" />
			{#key group.id}
				<GroupSmtp groupId={group.id} />
			{/key}
		{/if}
		{#if hasPhp}
			<p class="hint">
				The PHP settings apply to every PHP domain of the group after
				Server → Apply Changes; nginx takes uploads a bit larger.
			</p>
		{/if}
		<hr class="groove" />
		<div class="row-buttons">
			<Button onclick={quarantine}>
				{group.quarantine ? 'Lift Quarantine…' : 'Quarantine…'}
			</Button>
			<Button
				onclick={() =>
					requestDelete(
						`/groups/${group.id}`,
						`the group ${group.name}`,
					)}
				disabled={group.domains.length > 0 ||
					group.databases.length > 0}
			>
				Delete Group…
			</Button>
		</div>
	{:else if tab === 'domains'}
		<div class="list-view">
			<table>
				<thead>
					<tr>
						<th>Domain</th>
						<th>Type</th>
						<th>Directory</th>
					</tr>
				</thead>
				<tbody>
					{#each group.domains as d (d.id)}
						<tr>
							<td><a href="/domains/{d.id}">{d.name}</a></td>
							<td>
								{KIND_NAMES[d.kind]}{d.phpVersion
									? ' ' + d.phpVersion
									: ''}
							</td>
							<td>
								<code>
									{d.kind === 'proxy'
										? d.upstream
										: d.kind === 'redirect'
											? d.target
											: d.root}
								</code>
							</td>
						</tr>
					{:else}
						<tr>
							<td colspan="3" class="empty">No domains yet.</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
		<div class="row-buttons">
			<Button onclick={() => (dialogs.open = 'domain')}>
				New Domain…
			</Button>
		</div>
	{:else if tab === 'services'}
		<GroupProcs {group} worker={worker ? Number(worker) : null} />
		<GroupServices {group} />
	{:else if tab === 'config'}
		{#if files}
			<ConfigFiles {files} group={group.name} />
		{:else}
			<p class="hint">Loading…</p>
		{/if}
	{:else if tab === 'databases'}
		<GroupDatabases {group} />
	{:else if tab === 'deploy'}
		<Deploys {group} />
	{:else if tab === 'files'}
		<Files {group} path={filesPath} />
	{:else if tab === 'backups'}
		<Snapshots {group} />
	{:else if tab === 'usage'}
		<Usage scope="group:{group.id}" />
	{/if}
	{#if loadError}<p class="error">{loadError}</p>{/if}
</Tabs>

<style lang="scss">
	@use 'styles/win' as *;

	.unit {
		display: flex;
		gap: 6px;
		align-items: center;

		input {
			width: 80px;
		}
	}

	.hint {
		margin: 8px 0;
	}
</style>
