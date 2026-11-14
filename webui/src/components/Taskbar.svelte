<script lang="ts">
	import Icon, { type IconName } from './win/Icon.svelte';
	import DropMenu from './win/DropMenu.svelte';
	import type { MenuEntry } from './win/Menu.svelte';
	import { conversations } from '@/lib/conversations.svelte';
	import { connection } from '@/lib/events.svelte';
	import { dialogs } from '@/lib/dialogs.svelte';
	import {
		approvalKey,
		approvalLine,
		approvals,
	} from '@/lib/approvals.svelte';
	import { formatHour } from '@/lib/format';

	let {
		windows,
		start,
		onattention,
	}: {
		/// a button per window, pressed while it is shown
		windows: {
			title: string;
			icon: IconName;
			shown: boolean;
			ontoggle: () => void;
		}[];
		start: MenuEntry[];
		/// opens a routine or alert that needs attention
		onattention: (id: number) => void;
	} = $props();

	const attention = $derived(conversations.attention);
	const waiting = $derived(approvals.waiting);
	// proposals the planner still works on show too, they come next
	const open = $derived(approvals.undecided);
	const planning = $derived(open.length - waiting.length);

	let time = $state(formatHour(Date.now()));

	$effect(() => {
		const iv = setInterval(() => (time = formatHour(Date.now())), 10_000);
		return () => clearInterval(iv);
	});
</script>

<footer>
	<div class="start-wrap">
		<DropMenu entries={start} align="up">
			{#snippet button({ open, toggle })}
				<button class="start" class:open onclick={toggle}>
					<Icon name="logo" />
					<b>Start</b>
				</button>
			{/snippet}
			<div class="brand"><span>genco</span></div>
		</DropMenu>
	</div>
	<div class="tasks">
		{#each windows as w (w.title)}
			<button class="task" class:pressed={w.shown} onclick={w.ontoggle}>
				<Icon name={w.icon} />
				<span>{w.title}</span>
			</button>
		{/each}
	</div>
	<div class="tray">
		{#if attention.length}
			<button
				class="attention"
				title={attention.map(c => c.title).join('\n')}
				onclick={() => onattention(attention[0].id)}
			>
				⚠ {attention.length}
			</button>
		{/if}
		{#if open.length}
			<button
				class="approve"
				title={open.map(approvalLine).join('\n')}
				onclick={() => {
					// the only one opens selected
					if (open.length === 1)
						dialogs.approval = approvalKey(open[0]);
					dialogs.open = 'approvals';
				}}
			>
				<Icon name="agent" />
				{[
					waiting.length ? `${waiting.length} to approve` : '',
					planning ? `${planning} being planned` : '',
				]
					.filter(Boolean)
					.join(', ')}
			</button>
		{/if}
		{#if connection.lost}
			<span title="The connection to the server is lost">⚠ offline</span>
		{/if}
		<span>{time}</span>
	</div>
</footer>

<style lang="scss">
	@use 'styles/win' as *;

	footer {
		@include raised;
		display: flex;
		align-items: center;
		gap: 4px;
		height: 28px;
		padding: 2px;
		box-shadow:
			inset 0 1px $light,
			inset 0 2px $white;
	}

	// the drop of the Start menu: the brand next to the entries, taller
	.start-wrap :global(.drop) {
		z-index: 60;
		display: flex;
		@include raised;
		padding: 3px;
	}

	.start-wrap :global(.drop ul) {
		box-shadow: none;
		padding: 0;
	}

	.start-wrap :global(.drop button) {
		padding: 8px 24px 8px 8px;
	}

	button {
		@include raised;
		display: flex;
		align-items: center;
		gap: 4px;
		height: 22px;
		padding: 0 5px;
		border: none;
		font: inherit;

		&.open,
		&.pressed {
			@include pressed;
		}

		&.pressed {
			background: repeating-conic-gradient($face 0 25%, $white 0 50%) 0
				0 / 2px 2px;
			font-weight: bold;
		}
	}

	.brand {
		width: 22px;
		background: linear-gradient(to top, $title, #1084d0);
		display: flex;
		align-items: flex-end;

		span {
			writing-mode: vertical-rl;
			transform: rotate(180deg);
			color: $white;
			font-weight: bold;
			font-size: 16px;
			padding: 6px 0;
		}
	}

	.tasks {
		flex: 1;
		display: flex;
		gap: 3px;
		min-width: 0;

		.task {
			width: 160px;
			min-width: 0;
			justify-content: flex-start;

			span {
				overflow: hidden;
				text-overflow: ellipsis;
				white-space: nowrap;
			}
		}
	}

	.tray {
		@include groove;

		.attention {
			box-shadow: none;
			background: none;
			height: auto;
			padding: 0;
			color: $error;
			font-weight: bold;
		}

		.approve {
			display: flex;
			gap: 4px;
			align-items: center;
			box-shadow: none;
			background: none;
			height: auto;
			padding: 0;
			font-weight: bold;
		}

		display: flex;
		gap: 8px;
		align-items: center;
		height: 22px;
		padding: 0 8px;
	}
</style>
