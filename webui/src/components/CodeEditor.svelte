<script lang="ts">
	import { untrack } from 'svelte';
	import { languageFor, monaco } from '@/lib/monaco';

	// Monaco in a sunken field, ctrl+s saves
	let {
		name,
		value,
		readonly = false,
		onsave,
	}: {
		/** the file name decides the highlighting */
		name: string;
		value: string;
		readonly?: boolean;
		onsave: (value: string) => void;
	} = $props();

	let editor: monaco.editor.IStandaloneCodeEditor | undefined;

	export function getValue(): string {
		return editor?.getValue() ?? value;
	}
</script>

<div
	class="editor"
	{@attach el =>
		// made once: the props change on save, that must not rebuild it
		untrack(() => {
			editor = monaco.editor.create(el, {
				value,
				language: languageFor(name),
				readOnly: readonly,
				theme: 'vs',
				automaticLayout: true,
				minimap: { enabled: false },
				fontFamily: "'Lucida Console', 'Courier New', monospace",
				fontSize: 12,
				scrollBeyondLastLine: false,
				renderLineHighlight: 'none',
			});
			editor.addCommand(
				monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS,
				() => {
					if (!readonly) onsave(editor!.getValue());
				},
			);
			editor.focus();
			return () => editor?.dispose();
		})}
></div>

<style lang="scss">
	@use 'styles/win' as *;

	.editor {
		@include sunken;
		flex: 1;
		min-height: 200px;
		padding: 2px;
	}
</style>
