<script lang="ts">
	import { formatBytes, formatTime, type Group } from '@/lib/api';
	import { message } from '@/lib/dialogs.svelte';
	import { dropUpload } from '@/lib/dropUpload';
	import {
		copyName,
		fileMenu,
		groupFiles,
		type FileEntry as Entry,
	} from '@/lib/files';
	import { go } from '@/lib/router.svelte';
	import type { Upload } from '@/lib/upload';
	import FileAddress from './FileAddress.svelte';
	import FileEditor, { type EditedFile } from './FileEditor.svelte';
	import FilesToolbar from './FilesToolbar.svelte';
	import Icon from './win/Icon.svelte';
	import MenuList from './win/MenuList.svelte';

	// the files of a group, everything runs as its user on the server; the
	// open folder is part of the url
	let { group, path }: { group: Group; path: string } = $props();

	let entries = $state<Entry[] | null>(null);
	// why the open folder can't be changed here
	let dirReadOnly = $state<string | null>(null);
	let selected = $state<string | null>(null);
	let error = $state<string | null>(null);
	let uploading = $state<string | null>(null);
	let extracting = $state<string | null>(null);
	let editing = $state<EditedFile | null>(null);
	// the context menu at the pointer, of an entry or the folder
	let menu = $state<{ x: number; y: number; entry: Entry | null } | null>(
		null,
	);
	let fileInput: HTMLInputElement | undefined = $state();
	let folderInput: HTMLInputElement | undefined = $state();
	let list: HTMLElement | undefined = $state();

	const files = $derived(groupFiles(group.id));
	const entry = $derived(entries?.find(e => e.name === selected) ?? null);
	const parts = $derived(path.split('/').filter(Boolean));
	const ro = $derived(dirReadOnly !== null);

	function join(name: string, dir = path) {
		return dir ? `${dir}/${name}` : name;
	}

	/** the page of a folder */
	function urlFor(p: string) {
		const segs = p.split('/').filter(Boolean).map(encodeURIComponent);
		return (
			`/groups/${group.id}/files` +
			(segs.length ? '/' + segs.join('/') : '')
		);
	}

	async function load() {
		error = null;
		try {
			const res = await files.list(path);
			entries = res.entries;
			dirReadOnly = res.readOnly;
			if (!entries.some(e => e.name === selected))
				selected = entries[0]?.name ?? null;
		} catch (e: any) {
			error = e.message;
		}
	}
	load().then(() => list?.focus());

	function up() {
		if (parts.length) go(urlFor(parts.slice(0, -1).join('/')));
	}

	async function open(e: Entry) {
		if (e.dir) return go(urlFor(join(e.name)));
		const p = join(e.name);
		try {
			const text = await files.read(p);
			editing = { path: p, name: e.name, text, readOnly: e.readOnly };
		} catch (err: any) {
			message.show('Can’t Open', `${e.name}: ${err.message}`);
		}
	}

	function download(e: Entry) {
		files
			.download(join(e.name))
			.catch(err =>
				message.show('Can’t Download', `${e.name}: ${err.message}`),
			);
	}

	/** runs a change and shows its error, the list loads again */
	async function act(title: string, fn: () => Promise<unknown>) {
		try {
			await fn();
			await load();
		} catch (e: any) {
			message.show(title, e.message);
		}
		list?.focus();
	}

	async function newFolder() {
		const name = await message.ask('New Folder', 'Name of the new folder:');
		if (name)
			act('No Folder', async () => {
				await files.mkdir(join(name));
				selected = name;
			});
	}

	async function newFile() {
		const name = await message.ask(
			'New File',
			'Name of the new file, e.g. .env:',
		);
		if (!name) return;
		act('No File', async () => {
			await files.create(join(name));
			selected = name;
			editing = { path: join(name), name, text: '', readOnly: false };
		});
	}

	async function rename(e: Entry) {
		const name = await message.ask(
			'Rename',
			`New name of ${e.name}:`,
			e.name,
		);
		if (name && name !== e.name)
			act('Not Renamed', async () => {
				await files.rename(join(e.name), join(name));
				selected = name;
			});
	}

	async function copy(e: Entry) {
		const name = await message.ask(
			'Copy',
			`Name of the copy of ${e.name}:`,
			copyName(e),
		);
		if (name)
			act('Not Copied', async () => {
				await files.copy(join(e.name), join(name));
				selected = name;
			});
	}

	/** unpacks a zip or tar, e.g. a big assets folder uploaded at once */
	async function extract(e: Entry) {
		const to = await message.ask(
			'Extract',
			`Extract ${e.name} into the folder (relative to ${group.dir}, created if missing):`,
			path,
		);
		if (to === null) return;
		const replace = await message.confirm(
			'Extract',
			'Files with the same names in that folder are replaced. Go on?',
		);
		if (!replace) return;
		extracting = e.name;
		await act('Not Extracted', () =>
			files.extract(join(e.name), to.trim()),
		);
		extracting = null;
	}

	async function remove(e: Entry) {
		if (
			!(await message.confirm(
				'Delete',
				`Move ${e.name} to the trash (.trash, 30 days)?`,
			))
		)
			return;
		act('Not Deleted', () => files.delete(join(e.name)));
	}

	async function upload(items: Upload[]) {
		if (!items.length) return;
		const existing = new Set(entries?.map(e => e.name));
		const replaced = items
			.map(i => i.path.split('/')[parts.length])
			.filter((n, i, all) => existing.has(n) && all.indexOf(n) === i);
		if (
			replaced.length &&
			!(await message.confirm(
				'Upload',
				`Replace ${replaced.join(', ')}?`,
			))
		)
			return;
		try {
			for (const i of items)
				await files.write(
					i.path,
					i.file,
					pct => (uploading = `${i.path} (${pct}%)`),
				);
		} catch (e: any) {
			message.show('Upload Failed', e.message);
		}
		uploading = null;
		await load();
	}

	function picked(input: HTMLInputElement) {
		const items = [...(input.files ?? [])].map(file => ({
			// a picked folder has the path inside it
			path: join(file.webkitRelativePath || file.name),
			file,
		}));
		input.value = '';
		upload(items);
	}

	let dragging = $state(false);

	function onmenu(e: MouseEvent, en: Entry | null) {
		e.preventDefault();
		e.stopPropagation();
		if (en) selected = en.name;
		// it opens towards the middle of the screen
		const x = e.clientX > innerWidth - 200 ? e.clientX - 180 : e.clientX;
		const y = e.clientY > innerHeight - 200 ? e.clientY - 170 : e.clientY;
		menu = { x, y, entry: en };
	}

	const actions = {
		open,
		download,
		extract,
		copy,
		rename,
		remove,
		newFolder,
		newFile,
		uploadFiles: () => fileInput?.click(),
		uploadFolder: () => folderInput?.click(),
		refresh: load,
	};

	/** like the details view of Windows' explorer */
	function onkeydown(e: KeyboardEvent) {
		if (!entries || e.ctrlKey || e.metaKey) return;
		const i = entries.findIndex(en => en.name === selected);
		const pick = (n: number) => {
			const to = entries![Math.max(0, Math.min(entries!.length - 1, n))];
			if (to) selected = to.name;
		};

		switch (e.key) {
			case 'ArrowDown':
				pick(i + 1);
				break;
			case 'ArrowUp':
				if (e.altKey) up();
				else pick(i - 1);
				break;
			case 'Home':
				pick(0);
				break;
			case 'End':
				pick(entries.length - 1);
				break;
			case 'PageDown':
				pick(i + 10);
				break;
			case 'PageUp':
				pick(i - 10);
				break;
			case 'Enter':
				if (entry) open(entry);
				break;
			case 'Backspace':
				up();
				break;
			case 'Delete':
				if (entry && !entry.readOnly) remove(entry);
				break;
			case 'F2':
				if (entry && !entry.readOnly) rename(entry);
				break;
			case 'F5':
				load();
				break;
			default: {
				// a letter jumps to the next entry starting with it
				if (e.key.length !== 1 || e.altKey) return;
				const k = e.key.toLowerCase();
				const order = [
					...entries.slice(i + 1),
					...entries.slice(0, i + 1),
				];
				const next = order.find(en =>
					en.name.toLowerCase().startsWith(k),
				);
				if (next) selected = next.name;
			}
		}
		e.preventDefault();
	}

	// the selected row stays visible
	$effect(() => {
		void selected;
		list?.querySelector('tr.selected')?.scrollIntoView({
			block: 'nearest',
		});
	});
</script>

<svelte:window
	onclick={() => (menu = null)}
	onkeydown={e => {
		if (e.key === 'Escape') menu = null;
		// up a folder also when the list has no focus, not while typing
		// or with a dialog open
		const t = e.target as HTMLElement;
		const typing = t.closest('input, textarea, select, [contenteditable]');
		const upKey =
			e.key === 'Backspace' || (e.altKey && e.key === 'ArrowUp');
		if (
			upKey &&
			!typing &&
			!editing &&
			!list?.contains(t) &&
			!document.querySelector('.dialog-overlay')
		) {
			e.preventDefault();
			up();
		}
	}}
/>

<FilesToolbar
	{entry}
	canUp={parts.length > 0}
	{ro}
	uploading={uploading !== null}
	onup={up}
	onnewfolder={newFolder}
	onnewfile={newFile}
	onupload={() => fileInput?.click()}
	onopen={open}
	ondownload={download}
	onrename={rename}
	ondelete={remove}
/>
<input
	type="file"
	multiple
	hidden
	bind:this={fileInput}
	onchange={e => picked(e.target as HTMLInputElement)}
/>
<input
	type="file"
	webkitdirectory
	hidden
	bind:this={folderInput}
	onchange={e => picked(e.target as HTMLInputElement)}
/>

<FileAddress root={group.dir} {parts} href={urlFor} />

<div
	class="list-view files"
	class:dragging
	role="listbox"
	aria-label="Files"
	tabindex="0"
	bind:this={list}
	{onkeydown}
	{@attach dropUpload({
		dir: path,
		disabled: ro,
		dragging: over => (dragging = over),
		upload,
	})}
	oncontextmenu={e => onmenu(e, null)}
>
	<table>
		<thead>
			<tr>
				<th>Name</th>
				<th>Size</th>
				<th>Modified</th>
			</tr>
		</thead>
		<tbody>
			{#each entries ?? [] as e (e.name)}
				<tr
					class:selected={selected === e.name}
					class:ro={e.readOnly}
					title={e.readOnly ? 'read only' : undefined}
					role="option"
					aria-selected={selected === e.name}
					onclick={() => (selected = e.name)}
					ondblclick={() => open(e)}
					oncontextmenu={ev => onmenu(ev, e)}
				>
					<td class="name">
						<Icon
							name={e.kind === 'link'
								? 'link'
								: e.dir
									? 'folder'
									: 'file'}
						/>
						<span>{e.name}</span>
						{#if e.target}<span class="target">
								→ {e.target}
							</span>{/if}
					</td>
					<td>{e.dir ? '' : formatBytes(e.size)}</td>
					<td>{formatTime(e.modified)}</td>
				</tr>
			{:else}
				<tr>
					<td colspan="3" class="empty">
						{entries
							? 'Empty. Drop files or folders here to upload them.'
							: 'Loading…'}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>

{#if menu}
	<div class="context" style:left="{menu.x}px" style:top="{menu.y}px">
		<MenuList
			entries={fileMenu(menu.entry, ro, actions)}
			onpick={() => (menu = null)}
		/>
	</div>
{/if}

<div class="status">
	{#if error}
		<span class="error">{error}</span>
	{:else if uploading}
		Uploading {uploading}…
	{:else if extracting}
		Extracting {extracting}…
	{:else if dirReadOnly}
		Read only: {dirReadOnly}.
	{:else}
		{entries?.length ?? 0} object(s){entry && !entry.dir
			? `, ${formatBytes(entry.size)}`
			: ''}
	{/if}
</div>

{#if editing}
	{@const file = editing}
	<FileEditor
		{file}
		write={async value => {
			await files.write(file.path, value);
			await load();
		}}
		onclose={() => {
			editing = null;
			list?.focus();
		}}
	/>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	.files {
		flex: 1;
		min-height: 200px;

		&:focus {
			outline: 1px dotted #000;
			outline-offset: -3px;
		}

		&.dragging {
			outline: 2px dashed $title;
			outline-offset: -4px;
		}

		tr {
			cursor: default;
		}
	}

	.name {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.target {
		color: $shadow;
	}

	.list-view tbody tr.selected .target {
		color: $light;
	}

	// deploys or tools write these, the file manager only reads them
	tr.ro td {
		opacity: 0.55;
	}

	.list-view tbody tr.ro.selected td {
		opacity: 0.8;
	}

	.context {
		position: fixed;
		z-index: 90;
	}

	.status {
		@include groove;
		margin-top: 4px;
		padding: 2px 6px;
	}
</style>
