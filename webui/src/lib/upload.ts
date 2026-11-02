import { session } from './session.svelte';

/**
 * PUTs a file to `url` (an /api path), with the upload's progress: XHR,
 * fetch has none. Saving the editor goes through here too. Resolves with
 * the response's body.
 */
export function putFile(
	url: string,
	body: Blob | string,
	progress?: (pct: number) => void,
): Promise<string> {
	return new Promise((resolve, reject) => {
		const xhr = new XMLHttpRequest();
		xhr.open('PUT', '/api' + url);
		xhr.setRequestHeader('session-token', session.token ?? '');
		xhr.upload.onprogress = ev => {
			if (progress && ev.total)
				progress(Math.round((ev.loaded / ev.total) * 100));
		};
		xhr.onload = () => {
			if (xhr.status < 300) return resolve(xhr.responseText);
			let msg = xhr.statusText;
			try {
				msg = JSON.parse(xhr.responseText).message;
			} catch (_e) {
				// keep the status
			}
			reject(new Error(msg));
		};
		xhr.onerror = () => reject(new Error('the server is not reachable'));
		xhr.send(body);
	});
}

export type Upload = { path: string; file: File };

/** the files of dropped files and folders, their paths under `dir` */
export async function droppedFiles(
	entries: any[],
	dir: string,
): Promise<Upload[]> {
	const all = await Promise.all(entries.map(e => readEntry(e, dir)));
	return all.flat();
}

function join(name: string, dir: string) {
	return dir ? `${dir}/${name}` : name;
}

async function readEntry(entry: any, dir: string): Promise<Upload[]> {
	if (entry.isFile) {
		const file: File = await new Promise((res, rej) =>
			entry.file(res, rej),
		);
		return [{ path: join(entry.name, dir), file }];
	}
	const reader = entry.createReader();
	const items: Upload[] = [];
	// at most 100 entries per call
	for (;;) {
		const batch: any[] = await new Promise((res, rej) =>
			reader.readEntries(res, rej),
		);
		if (!batch.length) break;
		for (const child of batch)
			items.push(...(await readEntry(child, join(entry.name, dir))));
	}
	return items;
}
