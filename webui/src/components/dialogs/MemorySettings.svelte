<script lang="ts">
	import { get, post, type MemoryStatus } from '@/lib/api';
	import Form from '../win/Form.svelte';

	// how the RAM is split: the reserve for the system, the rest all groups
	// may use together; genco's agent may move it too
	let status = $state<MemoryStatus | null>(null);
	let reserve = $state('');
	let error = $state<string | null>(null);

	async function load() {
		try {
			status = await get('/settings/memory');
			reserve = status!.default ? '' : String(status!.reserveMb);
			error = null;
		} catch (e: any) {
			error = e.message;
		}
	}
	load();

	async function save() {
		const v = reserve.trim();
		const reserveMb = v === '' ? null : Number(v);
		if (reserveMb !== null && !Number.isInteger(reserveMb))
			throw new Error('The reserve is a whole number of MB or empty');
		const report = await post('/settings/memory', { reserveMb });
		if (!report.ok)
			throw new Error(
				report.log
					.filter((l: string) => l.startsWith('error'))
					.join('\n'),
			);
		await load();
	}

	const mb = (v: number | null) => (v === null ? '?' : `${v} MB`);
</script>

{#if error}<p class="error">{error}</p>{/if}
{#if status}
	<p>
		The server has {status.ramMb} MB of RAM. All groups together may use
		{status.capMb} MB, {status.reserveMb} MB stay for the system (databases, nginx,
		mail, genco). The groups' own limits add up to {status.limitsMb} MB, that
		is fine, they rarely peak together.
	</p>
	<p class="hint">
		Now the groups use {mb(status.groupsNowMb)} (at most {mb(
			status.groupsPeakMb,
		)} since their slice started), the system {mb(status.systemNowMb)}.
	</p>
	<Form onsubmit={save} ok="Save" done="Saved">
		<label for="ms-reserve">Reserve:</label>
		<span class="unit">
			<input
				id="ms-reserve"
				inputmode="numeric"
				placeholder={String(status.defaultReserveMb)}
				bind:value={reserve}
			/>
			MB for the system, {status.minReserveMb} to {status.maxReserveMb},
			empty for the default
		</span>
	</Form>
	<p class="hint">
		genco's agent moves it too when the split doesn't fit what the groups
		and the system use, and says why.
	</p>
{/if}

<style lang="scss">
	p {
		margin: 0 0 8px;
	}

	.unit {
		display: flex;
		gap: 6px;
		align-items: center;

		input {
			width: 80px;
		}
	}
</style>
