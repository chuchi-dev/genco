<script lang="ts">
	import {
		formatBytes,
		get,
		saveGroup,
		type Group,
		type Limits,
		type Stats,
	} from '@/lib/api';
	import Form from '@/components/win/Form.svelte';

	// what the group's slice may use; empty fields are the defaults
	let { group }: { group: Group } = $props();

	const text = (v: number | null) => (v === null ? '' : String(v));

	// svelte-ignore state_referenced_locally
	let fields = $state({
		memoryMaxMb: text(group.limits.memoryMaxMb),
		memoryHighMb: text(group.limits.memoryHighMb),
		cpuPercent: text(group.limits.cpuPercent),
		tasksMax: text(group.limits.tasksMax),
	});
	let warning = $state<string | null>(null);
	// what the group uses now and what the kernel holds it to
	let usage = $state<{ memory: number | null; limit: number | null }>({
		memory: null,
		limit: null,
	});

	const defaults = $derived(group.limits.defaults);

	$effect(() => {
		get(`/stats?scope=group:${group.id}&range=1h`)
			.then((s: Stats) => {
				const last = s.points.findLast(p => p.memory !== null);
				usage = { memory: last?.memory ?? null, limit: s.memoryLimit };
			})
			.catch(() => {});
	});

	async function save() {
		const n = (v: string) => {
			if (v.trim() === '') return null;
			const x = Number(v);
			if (!Number.isInteger(x) || x <= 0)
				throw new Error('The limits are whole numbers or empty');
			return x;
		};
		const limits: Limits = {
			memoryMaxMb: n(fields.memoryMaxMb),
			memoryHighMb: n(fields.memoryHighMb),
			cpuPercent: n(fields.cpuPercent),
			tasksMax: n(fields.tasksMax),
		};
		warning = (await saveGroup(group, { limits })).warning;
	}
</script>

<fieldset>
	<legend>Limits</legend>
	<p class="hint">
		What all the group's processes may use together (its systemd slice).
		Empty is the default; they apply with Server → Apply Changes, without a
		restart.
		{#if usage.memory !== null}
			Now it uses {formatBytes(usage.memory)}{usage.limit
				? ` of ${formatBytes(usage.limit)}`
				: ''}.
		{/if}
	</p>
	<Form onsubmit={save} ok="Save" done="Saved">
		<label for="gl-memory">Memory limit:</label>
		<span class="unit">
			<input
				id="gl-memory"
				inputmode="numeric"
				placeholder={String(defaults.memoryMaxMb)}
				bind:value={fields.memoryMaxMb}
			/>
			MB for all its processes
		</span>
		<label for="gl-high">Slows down from:</label>
		<span class="unit">
			<input
				id="gl-high"
				inputmode="numeric"
				placeholder={String(defaults.memoryHighMb)}
				bind:value={fields.memoryHighMb}
			/>
			MB, 2/3 of the limit if empty
		</span>
		<label for="gl-cpu">CPU:</label>
		<span class="unit">
			<input
				id="gl-cpu"
				inputmode="numeric"
				placeholder={String(defaults.cpuPercent)}
				bind:value={fields.cpuPercent}
			/>
			%, 100 is one core
		</span>
		<label for="gl-tasks">Tasks:</label>
		<span class="unit">
			<input
				id="gl-tasks"
				inputmode="numeric"
				placeholder={String(defaults.tasksMax)}
				bind:value={fields.tasksMax}
			/>
			processes and threads
		</span>
	</Form>
	{#if warning}<p class="error">{warning}</p>{/if}
</fieldset>

<style lang="scss">
	.unit {
		display: flex;
		gap: 6px;
		align-items: center;

		input {
			width: 80px;
		}
	}
</style>
