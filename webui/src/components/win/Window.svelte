<script lang="ts">
	import type { Snippet } from 'svelte';
	import Icon, { type IconName } from './Icon.svelte';

	let {
		title,
		icon,
		onclose,
		onminimize,
		ondrag,
		inactive = false,
		menu,
		status,
		children,
	}: {
		title: string;
		icon?: IconName;
		onclose?: () => void;
		onminimize?: () => void;
		/// the title bar was pressed, dialogs move with it
		ondrag?: (e: PointerEvent) => void;
		/// a grey title bar, a dialog flashes with it
		inactive?: boolean;
		/// menu bar below the title
		menu?: Snippet;
		/// status bar at the bottom
		status?: Snippet;
		children: Snippet;
	} = $props();
</script>

<section class="window">
	<!-- dragging is a mouse thing, the window stays usable without it -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<header
		class:draggable={!!ondrag}
		class:inactive
		onpointerdown={e => {
			// the buttons of the title bar stay buttons
			if (!(e.target as Element).closest('button')) ondrag?.(e);
		}}
	>
		{#if icon}<Icon name={icon} />{/if}
		<h2>{title}</h2>
		{#if onminimize}
			<button class="ctl" onclick={onminimize} aria-label="Minimize">
				<svg width="8" height="7" viewBox="0 0 8 7">
					<rect x="1" y="5" width="6" height="2" />
				</svg>
			</button>
		{/if}
		{#if onclose}
			<button class="ctl close" onclick={onclose} aria-label="Close">
				<svg width="8" height="7" viewBox="0 0 8 7">
					<path
						d="M0 0h2v1h1v1h2V1h1V0h2v1H7v1H6v1H5v1h1v1h1v1h1v1H6V6H5V5H3v1H2v1H0V6h1V5h1V4h1V3H2V2H1V1H0z"
					/>
				</svg>
			</button>
		{/if}
	</header>
	{#if menu}
		<nav class="menu">{@render menu()}</nav>
	{/if}
	<div class="body">
		{@render children()}
	</div>
	{#if status}
		<footer>{@render status()}</footer>
	{/if}
</section>

<style lang="scss">
	@use 'styles/win' as *;

	.window {
		@include raised;
		display: flex;
		flex-direction: column;
		padding: 3px;
		min-height: 0;
		min-width: 0;
	}

	header {
		display: flex;
		align-items: center;
		gap: 3px;
		height: 18px;
		padding: 0 2px 0 3px;
		background: $title;
		color: $white;
		flex-shrink: 0;

		&.inactive {
			background: $title-inactive;
		}

		&.draggable {
			touch-action: none;
		}
	}

	h2 {
		flex: 1;
		margin: 0;
		font-size: 12px;
		font-weight: bold;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.ctl {
		@include raised;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 16px;
		height: 14px;
		padding: 0;
		border: none;

		&:active {
			@include pressed;
		}

		&.close {
			margin-left: 2px;
		}
	}

	.menu {
		display: flex;
		padding: 1px 0;
		flex-shrink: 0;
	}

	.body {
		flex: 1;
		display: flex;
		flex-direction: column;
		min-height: 0;
		padding: 2px;
	}

	footer {
		display: flex;
		gap: 2px;
		padding-top: 2px;
		flex-shrink: 0;
	}
</style>
