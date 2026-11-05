<script lang="ts">
	import { message } from '@/lib/dialogs.svelte';
	import Button from './win/Button.svelte';
	import Dialog from './win/Dialog.svelte';

	export type EditedFile = {
		path: string;
		name: string;
		text: string;
		readOnly: boolean;
	};

	// a file of the file manager in Monaco; ctrl+s saves and keeps it open
	let {
		file,
		write,
		onclose,
	}: {
		file: EditedFile;
		/** saves the text, throws on an error */
		write: (value: string) => Promise<void>;
		onclose: () => void;
	} = $props();

	let editor: { getValue(): string } | undefined = $state();
	let saving = $state(false);
	// what is on the server
	// svelte-ignore state_referenced_locally
	let saved = file.text;

	/** true if it was saved */
	async function save(value: string): Promise<boolean> {
		// ctrl+s while the last save runs
		if (saving) return false;
		saving = true;
		try {
			await write(value);
			saved = value;
			return true;
		} catch (e: any) {
			message.show('Not Saved', e.message);
			return false;
		} finally {
			saving = false;
		}
	}

	/** asks first when there are changes */
	async function close() {
		const changed = !file.readOnly && editor?.getValue() !== saved;
		const sure =
			!changed ||
			(await message.confirm(
				'Close',
				'Close without saving the changes?',
			));
		if (sure) onclose();
	}
</script>

<!-- an editor wants room: most of the screen -->
<Dialog
	resizable
	title={file.path}
	icon="file"
	onclose={close}
	keep={() =>
		saving || (!file.readOnly && !!editor && editor.getValue() !== saved)}
	width={Math.round(innerWidth * 0.8)}
	height={Math.round(innerHeight * 0.85)}
>
	{#await import('./CodeEditor.svelte')}
		<p>Loading the editor…</p>
	{:then { default: CodeEditor }}
		<CodeEditor
			bind:this={editor}
			name={file.name}
			value={file.text}
			readonly={file.readOnly}
			onsave={save}
		/>
	{/await}
	<div class="row-buttons end">
		{#if file.readOnly}
			<span class="note">Read only</span>
			<Button primary onclick={onclose}>Close</Button>
		{:else}
			<span class="note">Ctrl+S saves and keeps it open</span>
			<Button
				primary
				disabled={saving}
				onclick={async () => {
					if (await save(editor?.getValue() ?? saved)) onclose();
				}}
			>
				Save
			</Button>
			<Button onclick={close}>Cancel</Button>
		{/if}
	</div>
</Dialog>

<style lang="scss">
	@use 'styles/win' as *;

	.note {
		flex: 1;
		align-self: center;
		color: $shadow;
	}
</style>
