<script lang="ts">
	// a new password and its repeat, two rows of a Form; the form's
	// onsubmit calls `check()` first
	let {
		value = $bindable(''),
		id,
		label = 'Password:',
		placeholder = 'At least 10 characters',
		autofocus = false,
	}: {
		value?: string;
		id: string;
		label?: string;
		placeholder?: string;
		autofocus?: boolean;
	} = $props();

	let repeat = $state('');

	export function check() {
		if (value !== repeat) throw new Error("The passwords don't match.");
	}
</script>

<label for={id}>{label}</label>
<!-- svelte-ignore a11y_autofocus -->
<input
	{id}
	type="password"
	autocomplete="new-password"
	{autofocus}
	{placeholder}
	bind:value
/>
<label for="{id}-repeat">Repeat:</label>
<input
	id="{id}-repeat"
	type="password"
	autocomplete="new-password"
	bind:value={repeat}
/>
