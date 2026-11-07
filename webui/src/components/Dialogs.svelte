<script lang="ts">
	import { dialogs } from '@/lib/dialogs.svelte';
	import { current, go } from '@/lib/router.svelte';
	import { tree } from '@/lib/tree.svelte';
	import ClientDialog from '@/components/dialogs/ClientDialog.svelte';
	import GroupDialog from '@/components/dialogs/GroupDialog.svelte';
	import DomainDialog from '@/components/dialogs/DomainDialog.svelte';
	import DatabaseDialog from '@/components/dialogs/DatabaseDialog.svelte';
	import MailDomainDialog from '@/components/dialogs/MailDomainDialog.svelte';
	import UsersDialog from '@/components/dialogs/UsersDialog.svelte';
	import PasswordDialog from '@/components/dialogs/PasswordDialog.svelte';
	import AccountDialog from '@/components/dialogs/AccountDialog.svelte';
	import ApplyDialog from '@/components/dialogs/ApplyDialog.svelte';
	import SettingsDialog from '@/components/dialogs/SettingsDialog.svelte';
	import TotpDialog from '@/components/dialogs/TotpDialog.svelte';
	import TokensDialog from '@/components/dialogs/TokensDialog.svelte';
	import AgentsDialog from '@/components/dialogs/AgentsDialog.svelte';
	import ApprovalsDialog from '@/components/dialogs/ApprovalsDialog.svelte';
	import FindDialog from '@/components/dialogs/FindDialog.svelte';
	import ActivityDialog from '@/components/dialogs/ActivityDialog.svelte';
	import PasswordsDialog from '@/components/dialogs/PasswordsDialog.svelte';
	import MessageBox from '@/components/dialogs/MessageBox.svelte';

	// the desktop's dialogs, one at a time (`dialogs.open`), and the
	// message boxes on top
	let {
		onpage,
		onchat,
	}: {
		/** shows a page in the Explorer */
		onpage: (path: string) => void;
		/** shows a conversation in the agent's window */
		onchat: (id: number) => void;
	} = $props();

	// the selected item's group and client, preselected in the dialogs
	const [, kind, idStr] = $derived(current.path.split('/'));
	const id = $derived(Number(idStr));
	const groupId = $derived.by(() => {
		if (kind === 'groups') return id;
		if (kind === 'domains') return tree.domain(id)?.groupId ?? null;
		if (kind === 'databases') return tree.database(id)?.groupId ?? null;
		if (kind === 'mail') return tree.mailDomain(id)?.groupId ?? null;
		return null;
	});
	const clientId = $derived(
		kind === 'clients'
			? id
			: groupId !== null
				? (tree.group(groupId)?.clientId ?? null)
				: null,
	);

	function created(kind: string) {
		return async (newId: number) => {
			dialogs.open = null;
			await tree.load();
			go(`/${kind}/${newId}`);
		};
	}
</script>

{#if dialogs.open === 'client'}
	<ClientDialog
		oncreated={created('clients')}
		onclose={() => (dialogs.open = null)}
	/>
{:else if dialogs.open === 'group'}
	<GroupDialog
		{clientId}
		oncreated={created('groups')}
		onclose={() => (dialogs.open = null)}
	/>
{:else if dialogs.open === 'domain'}
	<DomainDialog
		{groupId}
		oncreated={created('domains')}
		onclose={() => (dialogs.open = null)}
	/>
{:else if dialogs.open === 'database'}
	<DatabaseDialog
		{groupId}
		oncreated={created('databases')}
		onclose={() => (dialogs.open = null)}
	/>
{:else if dialogs.open === 'mail'}
	<MailDomainDialog
		{groupId}
		oncreated={created('mail')}
		onclose={() => (dialogs.open = null)}
	/>
{:else if dialogs.open === 'users'}
	<UsersDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'password'}
	<PasswordDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'account'}
	<AccountDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'apply'}
	<ApplyDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'settings'}
	<SettingsDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'totp'}
	<TotpDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'tokens'}
	<TokensDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'find'}
	<FindDialog onclose={() => (dialogs.open = null)} {onpage} {onchat} />
{:else if dialogs.open === 'agents'}
	<AgentsDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'approvals'}
	<ApprovalsDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'passwords'}
	<PasswordsDialog onclose={() => (dialogs.open = null)} />
{:else if dialogs.open === 'activity'}
	<ActivityDialog onclose={() => (dialogs.open = null)} />
{/if}
<MessageBox />
