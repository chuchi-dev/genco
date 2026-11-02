<script lang="ts">
	import type { Snippet } from 'svelte';
	import { track } from '@/lib/drag';
	import Window from './Window.svelte';
	import type { IconName } from './Icon.svelte';

	// a modal window: it starts in the middle and moves with its title
	// bar; a resizable one has a grip in the corner, for lists, logs and
	// editors that like more room
	let {
		title,
		icon,
		onclose,
		width = 380,
		height,
		resizable = false,
		children,
	}: {
		title: string;
		icon?: IconName;
		onclose: () => void;
		width?: number;
		/** a fixed start height, else the content decides */
		height?: number;
		resizable?: boolean;
		children: Snippet;
	} = $props();

	let el: HTMLElement | undefined = $state();
	let x = $state<number | null>(null);
	let y = $state(0);
	// svelte-ignore state_referenced_locally
	let w = $state(Math.min(width, innerWidth - 16));
	// its content decides until it is resized
	// svelte-ignore state_referenced_locally
	let h = $state<number | null>(
		height ? Math.min(height, innerHeight - 16) : null,
	);

	$effect(() => {
		if (x !== null || !el) return;
		const r = el.getBoundingClientRect();
		x = Math.max(8, (innerWidth - r.width) / 2);
		y = Math.max(8, (innerHeight - r.height) / 2);
	});

	// the keyboard works in the dialog: focus moves in (its autofocus
	// field, else the primary button), stays in it and goes back to where
	// it was when the dialog closes
	const FOCUSABLE =
		'button:not([disabled]), input:not([disabled]), select, textarea, a[href], [tabindex]:not([tabindex="-1"])';
	$effect(() => {
		const before = document.activeElement as HTMLElement | null;
		requestAnimationFrame(() => {
			if (!el || el.contains(document.activeElement)) return;
			const target =
				el.querySelector<HTMLElement>('[autofocus]') ??
				el.querySelector<HTMLElement>('button.primary') ??
				el.querySelector<HTMLElement>(FOCUSABLE) ??
				el;
			target.focus();
		});
		return () => {
			if (before?.isConnected) before.focus();
		};
	});

	function topmost(): boolean {
		const all = document.querySelectorAll('.dialog-overlay');
		return !!overlay && all[all.length - 1] === overlay;
	}

	// only the topmost dialog closes, a message box over the editor must
	// not close the editor too
	function onkeydown(e: KeyboardEvent) {
		if (!topmost()) return;
		if (e.key === 'Escape') {
			onclose();
		} else if (e.key === 'Tab' && el) {
			const items = [...el.querySelectorAll<HTMLElement>(FOCUSABLE)];
			if (!items.length) return;
			const [first, last] = [items[0], items[items.length - 1]];
			const inside = el.contains(document.activeElement);
			if (e.shiftKey && (document.activeElement === first || !inside)) {
				e.preventDefault();
				last.focus();
			} else if (
				!e.shiftKey &&
				(document.activeElement === last || !inside)
			) {
				e.preventDefault();
				first.focus();
			}
		}
	}

	let overlay: HTMLElement | undefined = $state();

	function drag(e: PointerEvent) {
		const [sx, sy] = [x ?? 0, y];
		track(e, (dx, dy) => {
			// the title bar stays on the screen
			x = Math.min(Math.max(sx + dx, 40 - w), innerWidth - 40);
			y = Math.min(Math.max(sy + dy, 0), innerHeight - 30);
		});
	}

	function resize(e: PointerEvent) {
		const r = el!.getBoundingClientRect();
		track(e, (dx, dy) => {
			w = Math.max(240, r.width + dx);
			h = Math.max(120, r.height + dy);
		});
	}
</script>

<svelte:window {onkeydown} />

<div class="overlay dialog-overlay" bind:this={overlay}>
	<div
		class="dialog"
		bind:this={el}
		style:left="{x ?? 0}px"
		style:top="{y}px"
		style:width="{w}px"
		style:height={h === null ? null : `${h}px`}
		style:visibility={x === null ? 'hidden' : null}
		role="dialog"
		aria-modal="true"
		aria-label={title}
		tabindex="-1"
	>
		<Window {title} {icon} {onclose} ondrag={drag}>
			<div class="content">
				{@render children()}
			</div>
		</Window>
		{#if resizable}
			<div
				class="grip"
				onpointerdown={resize}
				role="separator"
				aria-label="Resize"
			></div>
		{/if}
	</div>
</div>

<style lang="scss">
	@use 'styles/win' as *;

	.overlay {
		position: fixed;
		inset: 0;
		z-index: 100;
	}

	.dialog:focus {
		outline: none;
	}

	.dialog {
		position: absolute;
		display: flex;
		max-height: calc(100vh - 8px);

		> :global(.window) {
			flex: 1;
			min-width: 0;
		}
	}

	.content {
		flex: 1;
		display: flex;
		flex-direction: column;
		min-height: 0;
		padding: 10px;
		overflow: auto;
	}

	// the size grip of Windows 95: three diagonal lines in the corner
	.grip {
		position: absolute;
		right: 3px;
		bottom: 3px;
		width: 13px;
		height: 13px;
		cursor: nwse-resize;
		touch-action: none;
		background: linear-gradient(
			135deg,
			transparent 0 45%,
			$white 45% 50%,
			$shadow 50% 60%,
			transparent 60% 65%,
			$white 65% 70%,
			$shadow 70% 80%,
			transparent 80% 85%,
			$white 85% 90%,
			$shadow 90%
		);
	}
</style>
