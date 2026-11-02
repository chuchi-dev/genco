<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		onclick,
		disabled = false,
		primary = false,
		type = 'button',
		title,
		children,
	}: {
		onclick?: (e: MouseEvent) => void;
		disabled?: boolean;
		/// the default button of a dialog gets a black frame
		primary?: boolean;
		type?: 'button' | 'submit';
		title?: string;
		children: Snippet;
	} = $props();
</script>

<button {type} {onclick} {disabled} {title} class:primary>
	{@render children()}
</button>

<style lang="scss">
	@use 'styles/win' as *;

	button {
		@include raised;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 4px;
		min-width: 75px;
		min-height: 23px;
		padding: 0 12px;
		border: none;
		border-radius: 0;
		font: inherit;
		color: #000;
		cursor: default;

		&:active:not(:disabled) {
			@include pressed;
			padding: 1px 11px 0 13px;
		}

		&:focus-visible {
			outline: 1px dotted #000;
			outline-offset: -5px;
		}

		&.primary {
			box-shadow:
				inset -1px -1px $dark,
				inset 1px 1px $dark,
				inset -2px -2px $dark,
				inset 2px 2px $white,
				inset -3px -3px $shadow,
				inset 3px 3px $light;
		}

		&:disabled {
			color: $shadow;
			text-shadow: 1px 1px $white;
		}
	}
</style>
