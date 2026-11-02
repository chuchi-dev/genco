<script lang="ts">
	import { post, type Schedule } from '@/lib/api';
	import Dialog from '../win/Dialog.svelte';
	import Form from '../win/Form.svelte';

	// a new schedule of a group, or one to change
	let {
		groupId,
		schedule = null,
		onclose,
	}: {
		groupId: number;
		schedule?: Schedule | null;
		onclose: () => void;
	} = $props();

	// svelte-ignore state_referenced_locally
	let name = $state(schedule?.name ?? '');
	// svelte-ignore state_referenced_locally
	let dir = $state(schedule?.dir ?? 'current');
	// svelte-ignore state_referenced_locally
	let command = $state(schedule?.command ?? '');
	// svelte-ignore state_referenced_locally
	let every = $state(schedule?.every ?? 'hourly');
	// svelte-ignore state_referenced_locally
	let timeout = $state(String(schedule?.timeout ?? 60));

	async function save() {
		const t = Number(timeout);
		if (!Number.isInteger(t) || t <= 0)
			throw new Error('The time limit is a number of minutes');
		const body = { name, dir, command, every, timeout: t };
		if (schedule) await post(`/schedules/${schedule.id}`, body);
		else await post(`/groups/${groupId}/schedules`, body);
		onclose();
	}
</script>

<Dialog
	title={schedule ? `Schedule ${schedule.name}` : 'New Schedule'}
	icon="gear"
	{onclose}
	width={460}
>
	<Form onsubmit={save} oncancel={onclose}>
		<label for="s-name">Name:</label>
		<!-- svelte-ignore a11y_autofocus -->
		<input id="s-name" autofocus placeholder="cleanup" bind:value={name} />
		<label for="s-dir">Directory:</label>
		<input
			id="s-dir"
			placeholder="current/craft"
			title="relative to the group's directory"
			bind:value={dir}
		/>
		<label for="s-cmd">Command:</label>
		<input
			id="s-cmd"
			class="mono"
			spellcheck="false"
			placeholder="php craft gc"
			bind:value={command}
		/>
		<label for="s-every">Runs:</label>
		<input
			id="s-every"
			class="mono"
			spellcheck="false"
			title="systemd's OnCalendar: hourly, daily, *:0/5 (every 5 minutes), Mon *-*-* 03:00"
			bind:value={every}
		/>
		<label for="s-timeout">Time limit:</label>
		<span class="unit">
			<input id="s-timeout" inputmode="numeric" bind:value={timeout} />
			minutes
		</span>
	</Form>
	<p class="hint">
		Times like <code>hourly</code>, <code>daily</code>,
		<code>*:0/5</code> (every 5 minutes) or
		<code>Mon *-*-* 03:00</code>. Runs as the group's user; a failed run
		shows up as a problem.
	</p>
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;

	.mono {
		font-family: $mono;
	}

	.unit {
		display: flex;
		gap: 6px;
		align-items: center;

		input {
			width: 80px;
		}
	}

	.hint {
		margin: 8px 0 0;
	}
</style>
