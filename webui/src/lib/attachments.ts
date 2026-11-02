import { putFile } from './upload';
import { session } from './session.svelte';
import type { Attachment } from './api';

/** uploads a file for the next message, the server answers with its id */
export async function uploadAttachment(
	file: File,
	progress?: (pct: number) => void,
): Promise<Attachment> {
	const name = encodeURIComponent(file.name || 'pasted.png');
	return JSON.parse(
		await putFile(`/conversations/uploads?name=${name}`, file, progress),
	);
}

/**
 * The attachment as an object URL, for thumbnails and downloads: the
 * request needs the session's header, an <img> can't send it.
 */
export async function attachmentUrl(a: Attachment): Promise<string> {
	const resp = await fetch(`/api/conversations/uploads/${a.id}`, {
		headers: { 'session-token': session.token ?? '' },
	});
	if (!resp.ok) throw new Error(`${a.name} is gone`);
	return URL.createObjectURL(await resp.blob());
}

export function isImage(name: string): boolean {
	return /\.(png|jpe?g|gif|webp)$/i.test(name);
}

/** saves the attachment with its name */
export async function saveAttachment(a: Attachment) {
	const url = await attachmentUrl(a);
	const link = document.createElement('a');
	link.href = url;
	link.download = a.name;
	link.click();
	setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
