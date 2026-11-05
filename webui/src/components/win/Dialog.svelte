<script lang="ts" module>
	import { getContext } from 'svelte';

	/** what a Form in a dialog tells it */
	export type DialogContext = { saved: (form: HTMLFormElement) => void };

	export const dialogContext = () =>
		getContext<DialogContext | undefined>('dialog');
</script>

<script lang="ts">
	import { setContext, type Snippet } from 'svelte';
	import { track } from '@/lib/drag';
	import Window from './Window.svelte';
	import type { IconName } from './Icon.svelte';

	// a modal window: it starts in the middle and moves with its title
	// bar; a resizable one has a grip in the corner, for lists, logs and
	// editors that like more room. With `remember` the size the admin
	// gave it stays for the next time.
	let {
		title,
		icon,
		onclose,
		width = 380,
		height,
		resizable = false,
		remember,
		keep,
		children,
	}: {
		title: string;
		icon?: IconName;
		onclose: () => void;
		width?: number;
		/** a fixed start height, else the content decides */
		height?: number;
		resizable?: boolean;
		/** the name its size is kept under */
		remember?: string;
		/** true while a click beside it mustn't close it, besides the
		 * changes and requests of its forms */
		keep?: () => boolean;
		children: Snippet;
	} = $props();

	// svelte-ignore state_referenced_locally
	const KEY = remember && `genco-dialog-${remember}`;
	const kept: { w?: number; h?: number } = KEY
		? JSON.parse(localStorage.getItem(KEY) ?? '{}')
		: {};

	let el: HTMLElement | undefined = $state();
	let x = $state<number | null>(null);
	let y = $state(0);
	// svelte-ignore state_referenced_locally
	let w = $state(Math.min(kept.w ?? width, innerWidth - 16));
	// its content decides until it is resized
	// svelte-ignore state_referenced_locally
	const startHeight = kept.h ?? height;
	let h = $state<number | null>(
		startHeight ? Math.min(startHeight, innerHeight - 16) : null,
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

	function focusIn() {
		if (!el || el.contains(document.activeElement)) return;
		const target =
			el.querySelector<HTMLElement>('[autofocus]') ??
			el.querySelector<HTMLElement>('button.primary') ??
			el.querySelector<HTMLElement>(FOCUSABLE) ??
			el;
		target.focus();
	}

	$effect(() => {
		const before = document.activeElement as HTMLElement | null;
		requestAnimationFrame(focusIn);
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

	// a field of a form counts as changed when its value differs from the
	// one it had when the admin first pressed or focused it; values a
	// dialog loads before that aren't changes
	type Field = HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;
	const initial = new Map<Field, string | boolean>();
	const touched = new Set<Field>();

	function fieldOf(target: EventTarget | null): Field | null {
		if (!(target instanceof Element)) return null;
		// a click on a label changes its checkbox
		const f = target.closest('label')?.control ?? target;
		return (f instanceof HTMLInputElement ||
			f instanceof HTMLSelectElement ||
			f instanceof HTMLTextAreaElement) &&
			f.form
			? f
			: null;
	}

	const valueOf = (f: Field) =>
		f.type === 'checkbox' || f.type === 'radio'
			? (f as HTMLInputElement).checked
			: f.value;

	function note(e: Event) {
		const f = fieldOf(e.target);
		if (f && !initial.has(f)) initial.set(f, valueOf(f));
	}

	function change(e: Event) {
		const f = fieldOf(e.target);
		if (!f) return;
		// a checkbox can change without being pressed or focused
		if (!initial.has(f) && f.type === 'checkbox')
			initial.set(f, !valueOf(f));
		touched.add(f);
	}

	// a saved form starts over with its values
	setContext<DialogContext>('dialog', {
		saved: form => {
			for (const f of touched)
				if (f.form === form) initial.set(f, valueOf(f));
		},
	});

	function closable(): boolean {
		if (keep?.() || el?.querySelector('form[aria-busy="true"]'))
			return false;
		return ![...touched].some(
			f =>
				f.isConnected &&
				(!initial.has(f) || initial.get(f) !== valueOf(f)),
		);
	}

	// a click beside it closes it like Cancel; with changes or a running
	// request its title bar flashes like in Windows and it comes into
	// view. A press that starts in the dialog (selecting text) and ends
	// beside it is no click beside it.
	let pressedBeside = false;
	let inactive = $state(false);
	let flashing: ReturnType<typeof setInterval> | undefined;

	function press(e: PointerEvent) {
		pressedBeside = e.target === overlay;
	}

	function clickBeside(e: MouseEvent) {
		if (e.target !== overlay || !pressedBeside) return;
		pressedBeside = false;
		if (closable()) return onclose();

		clearInterval(flashing);
		inactive = false;
		let n = 0;
		flashing = setInterval(() => {
			inactive = !inactive;
			if (++n === 6) clearInterval(flashing);
		}, 100);

		const r = el!.getBoundingClientRect();
		x = Math.min(Math.max(x ?? 0, 0), Math.max(0, innerWidth - r.width));
		y = Math.min(Math.max(y, 0), Math.max(0, innerHeight - r.height));
		focusIn();
	}

	$effect(() => () => clearInterval(flashing));

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
		track(
			e,
			(dx, dy) => {
				w = Math.max(240, r.width + dx);
				h = Math.max(120, r.height + dy);
			},
			() => KEY && localStorage.setItem(KEY, JSON.stringify({ w, h })),
		);
	}
</script>

<svelte:window {onkeydown} />

<!-- a click beside it is a mouse thing, Escape closes it too -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
	class="overlay dialog-overlay"
	bind:this={overlay}
	onpointerdown={press}
	onmousedown={e => {
		// the field in the dialog keeps the focus
		if (e.target === overlay) e.preventDefault();
	}}
	onclick={clickBeside}
>
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
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
		onpointerdowncapture={note}
		onfocusin={note}
		oninput={change}
		onchange={change}
	>
		<Window {title} {icon} {onclose} {inactive} ondrag={drag}>
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
