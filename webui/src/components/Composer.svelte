<script lang="ts">
	import type { Chat } from '@/lib/chat.svelte';
	import { formatBytes } from '@/lib/api';
	import { uploadAttachment } from '@/lib/attachments';
	import Button from './win/Button.svelte';

	// where a message to the agent is written, with files to attach
	// (button, drop or paste), Stop while it works and Resolved when a
	// routine or alert needs attention
	let { chat }: { chat: Chat } = $props();

	const touch = matchMedia('(pointer: coarse)').matches;

	type Pending = {
		key: number;
		name: string;
		size: number;
		pct: number;
		id: string | null;
		error: string | null;
	};

	let text = $state('');
	let files = $state<Pending[]>([]);
	let sending = $state(false);
	let dragging = $state(false);
	let picker: HTMLInputElement;
	let next = 0;

	const uploading = $derived(files.some(f => !f.id && !f.error));
	const ready = $derived(
		(text.trim() !== '' || files.length > 0) &&
			!uploading &&
			!files.some(f => f.error) &&
			!sending,
	);

	function attach(list: FileList | File[] | null | undefined) {
		for (const file of list ?? []) {
			const f: Pending = {
				key: next++,
				name: file.name || 'pasted.png',
				size: file.size,
				pct: 0,
				id: null,
				error: null,
			};
			files.push(f);
			const item = files[files.length - 1];
			uploadAttachment(file, pct => (item.pct = pct))
				.then(a => (item.id = a.id))
				.catch(e => (item.error = e.message));
		}
	}

	function remove(key: number) {
		files = files.filter(f => f.key !== key);
	}

	async function send() {
		if (!ready) return;
		const msg = text.trim();
		const sent = files;
		// what is typed while it sends stays, a failed message comes back
		sending = true;
		text = '';
		files = [];
		try {
			await chat.send(
				msg,
				sent.map(f => f.id!),
			);
		} catch {
			text = text ? `${msg}\n${text}` : msg;
			files = [...sent, ...files];
		}
		sending = false;
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey && !touch) {
			e.preventDefault();
			send();
		}
	}

	function onpaste(e: ClipboardEvent) {
		const pasted = e.clipboardData?.files;
		if (pasted?.length) {
			e.preventDefault();
			attach(pasted);
		}
	}

	function ondrop(e: DragEvent) {
		e.preventDefault();
		dragging = false;
		attach(e.dataTransfer?.files);
	}
</script>

<div
	class="composer"
	class:dragging
	role="group"
	ondragover={e => {
		e.preventDefault();
		dragging = true;
	}}
	ondragleave={() => (dragging = false)}
	{ondrop}
>
	<div class="write">
		{#if files.length}
			<ul class="files">
				{#each files as f (f.key)}
					<li class:error={!!f.error} title={f.error ?? ''}>
						📎 {f.name}
						<span>
							{f.error
								? 'failed'
								: f.id
									? formatBytes(f.size)
									: `${f.pct} %`}
						</span>
						<button
							aria-label="Remove {f.name}"
							onclick={() => remove(f.key)}>×</button
						>
					</li>
				{/each}
			</ul>
		{/if}
		<textarea
			rows="3"
			placeholder="Message the agent… (drop or paste files)"
			spellcheck={!touch}
			bind:value={text}
			{onkeydown}
			{onpaste}
		></textarea>
	</div>
	<div class="buttons">
		<Button primary onclick={send} disabled={!ready}>Send</Button>
		<Button
			title="Attach files for the agent"
			onclick={() => picker.click()}
		>
			Attach…
		</Button>
		<input
			bind:this={picker}
			type="file"
			multiple
			hidden
			onchange={() => {
				attach(picker.files);
				picker.value = '';
			}}
		/>
		{#if chat.running}
			<Button onclick={() => chat.stop()}>Stop</Button>
		{/if}
		{#if chat.conversation?.report === 'attention' && !chat.running}
			<Button
				title="An admin took care of it"
				onclick={() => chat.resolve()}
			>
				Resolved
			</Button>
		{/if}
	</div>
</div>

<style lang="scss">
	@use 'styles/win' as *;

	.composer {
		display: flex;
		gap: 4px;
		margin-top: 4px;

		&.dragging textarea {
			outline: 2px dashed $select;
		}

		.write {
			flex: 1;
			display: flex;
			flex-direction: column;
			gap: 3px;
			min-width: 0;
		}

		textarea {
			flex: 1;
			resize: none;
		}

		.buttons {
			display: flex;
			flex-direction: column;
			gap: 4px;
		}
	}

	.files {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 3px 10px;

		li {
			white-space: nowrap;

			span {
				color: $shadow;
			}

			&.error {
				color: $error;
			}
		}

		button {
			border: none;
			background: none;
			font: inherit;
			padding: 0 2px;
			cursor: pointer;
		}
	}
</style>
