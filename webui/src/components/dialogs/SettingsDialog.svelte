<script lang="ts">
	import Dialog from '../win/Dialog.svelte';
	import Tabs from '../win/Tabs.svelte';
	import AgentSettings from './AgentSettings.svelte';
	import MailSettings from './MailSettings.svelte';
	import NotifySettings from './NotifySettings.svelte';
	import RunnerSettings from './RunnerSettings.svelte';
	import UpdateSettings from './UpdateSettings.svelte';

	let { onclose }: { onclose: () => void } = $props();

	const TABS = [
		{ id: 'mail', label: 'Mail' },
		{ id: 'notify', label: 'Notifications' },
		{ id: 'agent', label: 'Agent' },
		{ id: 'update', label: 'Updates' },
		{ id: 'runners', label: 'Runners' },
	];
	let tab = $state('mail');
</script>

<Dialog title="Settings" icon="gear" {onclose} width={500}>
	<Tabs tabs={TABS} selected={tab} onselect={t => (tab = t)}>
		<!-- all stay mounted: a token shown once mustn't go with a tab
		switch -->
		<div class="tab" hidden={tab !== 'mail'}><MailSettings /></div>
		<div class="tab" hidden={tab !== 'notify'}><NotifySettings /></div>
		<div class="tab" hidden={tab !== 'agent'}><AgentSettings /></div>
		<div class="tab" hidden={tab !== 'update'}><UpdateSettings /></div>
		<div class="tab" hidden={tab !== 'runners'}><RunnerSettings /></div>
	</Tabs>
</Dialog>

<style lang="scss">
	.tab {
		padding: 4px;
		min-height: 300px;
	}
</style>
