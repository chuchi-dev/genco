<script lang="ts" module>
	/** MAX_NOTE of the server: a rejection's reason, an approval's comment */
	export const MAX_NOTE = 2000;
</script>

<script lang="ts">
	// an admin's note on a proposal (the reason of a rejection, the comment
	// of an approval), two rows of a Form
	let {
		value = $bindable(''),
		id,
		label,
		placeholder,
		rows = 4,
		autofocus = false,
	}: {
		value?: string;
		id: string;
		label: string;
		placeholder: string;
		rows?: number;
		autofocus?: boolean;
	} = $props();

	// phones' autocorrect garbles names and commands
	const touch = matchMedia('(pointer: coarse)').matches;
</script>

<label for={id}>{label}</label>
<!-- svelte-ignore a11y_autofocus -->
<textarea
	{id}
	{rows}
	maxlength={MAX_NOTE}
	{autofocus}
	{placeholder}
	spellcheck={!touch}
	bind:value
></textarea>
