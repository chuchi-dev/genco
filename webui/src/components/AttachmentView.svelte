<script lang="ts">
	import { formatBytes, type Attachment } from '@/lib/api';
	import { attachmentUrl, isImage, saveAttachment } from '@/lib/attachments';

	// a file of a message: images as a small picture, others by name;
	// a click saves it
	let { file }: { file: Attachment } = $props();

	let url = $state<string | null>(null);

	$effect(() => {
		if (!isImage(file.name)) return;
		let mine: string | null = null;
		attachmentUrl(file)
			.then(u => (url = mine = u))
			.catch(() => (url = null));
		return () => {
			if (mine) URL.revokeObjectURL(mine);
		};
	});
</script>

<button
	class="file"
	title="{file.path}, click to save"
	onclick={() => saveAttachment(file)}
>
	{#if url}
		<img src={url} alt={file.name} />
	{:else}
		📎 {file.name} <span>({formatBytes(file.size)})</span>
	{/if}
</button>

<style lang="scss">
	@use 'styles/win' as *;

	.file {
		border: none;
		background: none;
		font: inherit;
		padding: 0;
		text-align: left;
		cursor: pointer;
		color: $link;

		span {
			color: $shadow;
		}

		img {
			display: block;
			max-width: 220px;
			max-height: 160px;
			border: 1px solid $shadow;
		}
	}
</style>
