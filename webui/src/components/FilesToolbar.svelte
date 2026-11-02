<script lang="ts">
	import type { FileEntry } from '@/lib/files';
	import Button from './win/Button.svelte';

	// the buttons above the file list, those of an entry act on the
	// selected one
	let {
		entry,
		canUp,
		ro,
		uploading,
		onup,
		onnewfolder,
		onnewfile,
		onupload,
		onopen,
		ondownload,
		onrename,
		ondelete,
	}: {
		entry: FileEntry | null;
		/** the folder isn't the group's own */
		canUp: boolean;
		/** the folder is read only */
		ro: boolean;
		uploading: boolean;
		onup: () => void;
		onnewfolder: () => void;
		onnewfile: () => void;
		onupload: () => void;
		onopen: (e: FileEntry) => void;
		ondownload: (e: FileEntry) => void;
		onrename: (e: FileEntry) => void;
		ondelete: (e: FileEntry) => void;
	} = $props();
</script>

<div class="toolbar">
	<Button onclick={onup} disabled={!canUp} title="Up one level (Backspace)">
		Up
	</Button>
	<Button onclick={onnewfolder} disabled={ro}>New Folder…</Button>
	<Button onclick={onnewfile} disabled={ro}>New File…</Button>
	<Button onclick={onupload} disabled={ro || uploading}>Upload…</Button>
	<span class="sep"></span>
	<Button onclick={() => entry && onopen(entry)} disabled={!entry}
		>Open</Button
	>
	<Button onclick={() => entry && ondownload(entry)} disabled={!entry}>
		Download
	</Button>
	<Button
		onclick={() => entry && onrename(entry)}
		disabled={!entry || entry.readOnly}
		title="F2"
	>
		Rename…
	</Button>
	<Button
		onclick={() => entry && ondelete(entry)}
		disabled={!entry || entry.readOnly}
		title="Del"
	>
		Delete
	</Button>
</div>

<style lang="scss">
	.toolbar {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin-bottom: 6px;

		:global(button) {
			min-width: 0;
			padding: 0 8px;
		}

		.sep {
			width: 6px;
		}
	}
</style>
