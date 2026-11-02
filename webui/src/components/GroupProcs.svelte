<script lang="ts">
	import { post, type Group, type Schedule, type Worker } from '@/lib/api';
	import { confirmDelete, message } from '@/lib/dialogs.svelte';
	import Button from '@/components/win/Button.svelte';
	import ListView from '@/components/win/ListView.svelte';
	import WorkerDialog from '@/components/dialogs/WorkerDialog.svelte';
	import ScheduleDialog from '@/components/dialogs/ScheduleDialog.svelte';

	// the group's workers (always running) and schedules (at times); the
	// ones of genco.toml change only with the repository
	let { group }: { group: Group } = $props();

	let selWorker = $state<number | null>(null);
	let selSchedule = $state<number | null>(null);
	const worker = $derived(group.workers.find(w => w.id === selWorker));
	const schedule = $derived(group.schedules.find(s => s.id === selSchedule));
	let editing = $state<
		| { kind: 'worker'; item: Worker | null }
		| { kind: 'schedule'; item: Schedule | null }
		| null
	>(null);

	const REPO =
		'comes with the deploys (genco.toml or detected), change it in the repository';

	async function runNow(s: Schedule) {
		try {
			await post(`/schedules/${s.id}/run`, {});
			message.show(
				'Schedule started',
				`${s.name} runs now, its output is in the journal of ` +
					`genco-${group.name}-cron-${s.name}.`,
			);
		} catch (e: any) {
			message.show('Not started', e.message);
		}
	}
</script>

<h4>Workers</h4>
<ListView
	items={group.workers}
	key={w => w.id}
	bind:selected={selWorker}
	columns={[
		{ label: 'Name' },
		{ label: 'Command' },
		{ label: 'Directory' },
		{ label: 'Port', num: true },
	]}
	empty="No workers. Craft's queue gets one with the next deploy."
	ondblclick={w =>
		w.source === 'panel' && (editing = { kind: 'worker', item: w })}
	maxHeight={200}
>
	{#snippet row(w)}
		<td title={w.source === 'repo' ? REPO : ''}>
			{w.name}{w.source === 'repo' ? ' (repository)' : ''}
		</td>
		<td class="mono">{w.command}</td>
		<td><code>{w.dir}</code></td>
		<td class="num">{w.port ?? ''}</td>
	{/snippet}
</ListView>
<div class="row-buttons">
	<Button onclick={() => (editing = { kind: 'worker', item: null })}>
		New Worker…
	</Button>
	<Button
		disabled={!worker || worker.source !== 'panel'}
		onclick={() => worker && (editing = { kind: 'worker', item: worker })}
	>
		Change…
	</Button>
	<Button
		disabled={!worker || worker.source !== 'panel'}
		onclick={() =>
			worker &&
			confirmDelete({
				title: 'Delete Worker',
				text: `Stop and remove the worker ${worker.name}? It stops with the next Apply Changes.`,
				path: `/workers/${worker.id}`,
			})}
	>
		Delete…
	</Button>
</div>

<h4>Schedules</h4>
<ListView
	items={group.schedules}
	key={s => s.id}
	bind:selected={selSchedule}
	columns={[
		{ label: 'Name' },
		{ label: 'Runs' },
		{ label: 'Command' },
		{ label: 'Directory' },
	]}
	empty="No schedules."
	ondblclick={s =>
		s.source === 'panel' && (editing = { kind: 'schedule', item: s })}
	maxHeight={200}
>
	{#snippet row(s)}
		<td title={s.source === 'repo' ? REPO : ''}>
			{s.name}{s.source === 'repo' ? ' (repository)' : ''}
		</td>
		<td class="mono">{s.every}</td>
		<td class="mono">{s.command}</td>
		<td><code>{s.dir}</code></td>
	{/snippet}
</ListView>
<div class="row-buttons">
	<Button onclick={() => (editing = { kind: 'schedule', item: null })}>
		New Schedule…
	</Button>
	<Button
		disabled={!schedule || schedule.source !== 'panel'}
		onclick={() =>
			schedule && (editing = { kind: 'schedule', item: schedule })}
	>
		Change…
	</Button>
	<Button disabled={!schedule} onclick={() => schedule && runNow(schedule)}>
		Run Now
	</Button>
	<Button
		disabled={!schedule || schedule.source !== 'panel'}
		onclick={() =>
			schedule &&
			confirmDelete({
				title: 'Delete Schedule',
				text: `Remove the schedule ${schedule.name}? Its timer stops with the next Apply Changes.`,
				path: `/schedules/${schedule.id}`,
			})}
	>
		Delete…
	</Button>
</div>

<h4>Units</h4>

{#if editing?.kind === 'worker'}
	<WorkerDialog
		groupId={group.id}
		worker={editing.item}
		onclose={() => (editing = null)}
	/>
{:else if editing?.kind === 'schedule'}
	<ScheduleDialog
		groupId={group.id}
		schedule={editing.item}
		onclose={() => (editing = null)}
	/>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	h4 {
		margin: 4px 0 6px;

		&:not(:first-child) {
			margin-top: 14px;
		}
	}

	.mono {
		font-family: $mono;
	}
</style>
