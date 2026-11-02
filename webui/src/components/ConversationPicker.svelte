<script lang="ts">
	import { untrack } from 'svelte';
	import { type Conversation } from '@/lib/api';
	import { messageSearch } from '@/lib/conversations.svelte';
	import { formatRecent } from '@/lib/format';

	// picks a conversation of the agent: a field like a drop-down list that
	// opens a search over the titles, and over the messages from three
	// letters on
	let {
		conversations,
		current,
		onselect,
	}: {
		/** newest first */
		conversations: Conversation[];
		current: Conversation | null;
		onselect: (id: number) => void;
	} = $props();

	let open = $state(false);
	let query = $state('');
	let active = $state(0);
	let list: HTMLElement | undefined = $state();

	const q = $derived(query.trim().toLowerCase());
	// conversations whose messages contain the query, from the server
	const found = messageSearch(() => q);
	const shown = $derived(
		q
			? conversations.filter(
					c =>
						c.title.toLowerCase().includes(q) ||
						found.ids.includes(c.id),
				)
			: conversations,
	);
	const chats = $derived(shown.filter(c => c.kind === 'chat'));
	const reports = $derived(shown.filter(c => c.kind !== 'chat'));
	// in the order they are listed, for the arrow keys
	const ordered = $derived([...chats, ...reports]);

	// the highlight starts at the open conversation, or on top; only when
	// it opens or the search changes, not on every status change
	$effect(() => {
		if (!open) return;
		void q;
		untrack(() => {
			const i = ordered.findIndex(c => c.id === current?.id);
			active = q || i < 0 ? 0 : i;
		});
	});

	$effect(() => {
		if (!open || !list) return;
		list.querySelector(`[data-index="${active}"]`)?.scrollIntoView({
			block: 'nearest',
		});
	});

	function pick(c: Conversation) {
		open = false;
		query = '';
		onselect(c.id);
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') {
			active = Math.min(active + 1, ordered.length - 1);
		} else if (e.key === 'ArrowUp') {
			active = Math.max(active - 1, 0);
		} else if (e.key === 'Enter' && ordered[active]) {
			pick(ordered[active]);
		} else if (e.key === 'Escape') {
			open = false;
		} else {
			return;
		}
		e.preventDefault();
		e.stopPropagation();
	}

	function mark(c: Conversation): string {
		if (c.status === 'running') return '●';
		if (c.report === 'attention') return '⚠';
		if (c.report === 'ok') return '✓';
		return '';
	}
</script>

<svelte:window onclick={() => (open = false)} />

<div class="picker">
	<button
		class="field"
		aria-haspopup="listbox"
		aria-expanded={open}
		onclick={e => {
			e.stopPropagation();
			// a new search every time it opens
			query = '';
			open = !open;
		}}
	>
		<span class="title">
			{current
				? `${mark(current)} ${current.title}`.trim()
				: '(new conversation)'}
		</span>
		<span class="arrow" aria-hidden="true">▾</span>
	</button>

	{#if open}
		<!-- svelte-ignore a11y_click_events_have_key_events -->
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="drop" onclick={e => e.stopPropagation()}>
			<input
				type="search"
				placeholder="Search titles and messages…"
				aria-label="Search conversations"
				bind:value={query}
				{onkeydown}
				{@attach el => el.focus()}
			/>
			<div class="list" role="listbox" bind:this={list}>
				{#each [['Conversations', chats], ['Routines and alerts', reports]] as const as [label, items] (label)}
					{#if items.length}
						<div class="group">{label}</div>
						{#each items as c (c.id)}
							{@const i = ordered.indexOf(c)}
							<button
								class="item"
								class:active={i === active}
								class:current={c.id === current?.id}
								role="option"
								aria-selected={c.id === current?.id}
								data-index={i}
								onmousemove={() => (active = i)}
								onclick={() => pick(c)}
							>
								<span class="mark">{mark(c)}</span>
								<span class="title">{c.title}</span>
								<span class="when"
									>{formatRecent(c.updatedAt)}</span
								>
							</button>
						{/each}
					{/if}
				{/each}
				{#if !ordered.length}
					<div class="empty">
						{q.length >= 3
							? 'Nothing found.'
							: 'No conversation matches.'}
					</div>
				{/if}
			</div>
		</div>
	{/if}
</div>

<style lang="scss">
	@use 'styles/win' as *;

	.picker {
		position: relative;
		flex: 1;
		min-width: 0;
	}

	// looks like the drop-down lists of the panel
	.field {
		@include sunken;
		display: flex;
		align-items: center;
		width: 100%;
		height: 100%;
		min-height: 22px;
		padding: 0 0 0 4px;
		border: none;
		background: $white;
		font: inherit;
		text-align: left;

		.title {
			flex: 1;
			min-width: 0;
			overflow: hidden;
			text-overflow: ellipsis;
			white-space: nowrap;
		}

		.arrow {
			@include raised;
			display: flex;
			align-items: center;
			justify-content: center;
			align-self: stretch;
			width: 16px;
			margin: 2px;
			font-size: 10px;
		}
	}

	.drop {
		@include raised;
		position: absolute;
		top: calc(100% + 2px);
		left: 0;
		// wider than the field, the buttons next to it are short
		min-width: 100%;
		width: min(460px, calc(100vw - 16px));
		z-index: 50;
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 4px;
		max-height: min(70vh, 520px);

		input {
			width: 100%;
		}
	}

	.list {
		@include sunken;
		background: $white;
		overflow-y: auto;
		min-height: 0;
	}

	.group {
		padding: 4px 6px 2px;
		color: $hint;
		font-weight: bold;
	}

	.item {
		display: flex;
		gap: 6px;
		width: 100%;
		padding: 2px 6px;
		border: none;
		background: none;
		font: inherit;
		text-align: left;

		&.current .title {
			font-weight: bold;
		}

		&.active {
			background: $select;
			color: $white;

			.when {
				color: $white;
			}
		}

		.mark {
			width: 12px;
			flex: none;
		}

		.title {
			flex: 1;
			min-width: 0;
			overflow: hidden;
			text-overflow: ellipsis;
			white-space: nowrap;
		}

		.when {
			flex: none;
			color: $hint;
		}
	}

	.empty {
		padding: 6px;
		color: $hint;
	}
</style>
