import { get, post, request, download } from './api';
import { putFile } from './upload';
import type { MenuEntry } from '@/components/win/Menu.svelte';

export type FileEntry = {
	name: string;
	kind: 'dir' | 'file' | 'link';
	dir: boolean;
	size: number;
	modified: number;
	target: string | null;
	/** a link whose target is missing */
	broken: boolean;
	/** where a link leads in the group, null for one out of it */
	real: string | null;
	/** a link that couldn't be resolved, real says nothing then */
	unresolved: boolean;
	/** the file manager doesn't change where the link leads */
	realReadOnly: boolean;
	/// deploys or tools write it, see READ_ONLY on the server
	readOnly: boolean;
};

export type Folder = {
	/** why the folder can't be changed here */
	readOnly: string | null;
	entries: FileEntry[];
};

/**
 * The file routes of a group, everything runs as its user on the server.
 * Paths are relative to the group's folder.
 */
export function groupFiles(groupId: number) {
	const base = `/groups/${groupId}/files`;
	const q = (path: string) => `?path=${encodeURIComponent(path)}`;

	return {
		list: (path: string): Promise<Folder> => get(base + q(path)),
		read: (path: string): Promise<string> =>
			request('GET', `${base}/read${q(path)}`, null, true),
		download: (path: string) => download(`${base}/download`, { path }),
		mkdir: (path: string) => post(`${base}/mkdir`, { path }),
		create: (path: string) => post(`${base}/create`, { path }),
		rename: (from: string, to: string) =>
			post(`${base}/rename`, { from, to }),
		copy: (from: string, to: string) => post(`${base}/copy`, { from, to }),
		/** unpacks a zip or tar into the folder `to` */
		extract: (path: string, to: string) =>
			post(`${base}/extract`, { path, to }),
		/** moves it to the trash */
		delete: (path: string) => post(`${base}/delete`, { path }),
		write: (
			path: string,
			body: Blob | string,
			progress?: (pct: number) => void,
		) => putFile(`${base}/write${q(path)}`, body, progress),
	};
}

/** the suggested name of a copy: a file keeps its extension */
export function copyName(e: FileEntry): string {
	const dot = e.dir ? -1 : e.name.lastIndexOf('.');
	return dot > 0
		? `${e.name.slice(0, dot)}-copy${e.name.slice(dot)}`
		: `${e.name}-copy`;
}

const ARCHIVE = /\.(zip|tar|tar\.gz|tgz)$/i;

/** what the context menu of an entry, or of the folder (null), does */
export type FileActions = {
	open: (e: FileEntry) => void;
	download: (e: FileEntry) => void;
	extract: (e: FileEntry) => void;
	copy: (e: FileEntry) => void;
	rename: (e: FileEntry) => void;
	remove: (e: FileEntry) => void;
	newFolder: () => void;
	newFile: () => void;
	uploadFiles: () => void;
	uploadFolder: () => void;
	refresh: () => void;
};

/** the context menu of an entry or of the folder, `ro` a read only one */
export function fileMenu(
	en: FileEntry | null,
	ro: boolean,
	a: FileActions,
): MenuEntry[] {
	if (en)
		return [
			{ label: en.dir ? 'Open' : 'Edit', onclick: () => a.open(en) },
			{ label: 'Download', onclick: () => a.download(en) },
			...(ARCHIVE.test(en.name) && !en.dir
				? [{ label: 'Extract…', onclick: () => a.extract(en) }]
				: []),
			{ separator: true },
			{ label: 'Copy…', onclick: () => a.copy(en), disabled: ro },
			{
				label: 'Rename…',
				onclick: () => a.rename(en),
				disabled: en.readOnly,
			},
			{
				label: 'Delete',
				onclick: () => a.remove(en),
				disabled: en.readOnly,
			},
		];
	return [
		{ label: 'New Folder…', onclick: a.newFolder, disabled: ro },
		{ label: 'New File…', onclick: a.newFile, disabled: ro },
		{ label: 'Upload Files…', onclick: a.uploadFiles, disabled: ro },
		{ label: 'Upload a Folder…', onclick: a.uploadFolder, disabled: ro },
		{ separator: true },
		{ label: 'Refresh', onclick: a.refresh },
	];
}
