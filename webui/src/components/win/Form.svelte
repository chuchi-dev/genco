<script lang="ts">
	import type { Snippet } from 'svelte';
	import Button from './Button.svelte';

	// a dialog form: labeled rows, OK / Cancel and the error line. It runs
	// `onsubmit` once at a time; an error it throws shows below the rows,
	// with `done` the OK button says so for a moment (e.g. "Saved"), not
	// when `onsubmit` returns false (canceled).
	// `extra` buttons go to the left end of the OK row.
	let {
		onsubmit,
		oncancel,
		ok = 'OK',
		done,
		extra,
		children,
	}: {
		onsubmit: () => unknown;
		oncancel?: () => void;
		ok?: string;
		done?: string;
		extra?: Snippet;
		children: Snippet;
	} = $props();

	let busy = $state(false);
	let error = $state<string | null>(null);
	let flash = $state(false);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (busy) return;
		busy = true;
		error = null;
		try {
			const result = await onsubmit();
			if (done && result !== false) {
				flash = true;
				setTimeout(() => (flash = false), 1500);
			}
		} catch (err: any) {
			error = err?.message ?? String(err);
		} finally {
			busy = false;
		}
	}
</script>

<form onsubmit={submit}>
	<div class="rows">
		{@render children()}
	</div>
	{#if error}
		<p class="error">{error}</p>
	{/if}
	<div class="buttons">
		{#if extra}
			{@render extra()}
			<span class="gap"></span>
		{/if}
		<Button type="submit" primary disabled={busy}>
			{flash && done ? done : ok}
		</Button>
		{#if oncancel}
			<Button onclick={oncancel}>Cancel</Button>
		{/if}
	</div>
</form>

<style lang="scss">
	@use 'styles/win' as *;
	.rows {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: 8px 10px;
		align-items: center;

		:global(label) {
			white-space: nowrap;
		}

		:global(textarea) {
			resize: vertical;
		}
	}

	.error {
		margin: 12px 0 0;
		color: $error;
		user-select: text;
	}

	.buttons {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: 6px;
		margin-top: 16px;
	}

	.gap {
		flex: 1;
	}
</style>
