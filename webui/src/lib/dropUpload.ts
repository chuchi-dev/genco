import type { Attachment } from 'svelte/attachments';
import { message } from './dialogs.svelte';
import { droppedFiles, type Upload } from './upload';

/**
 * Files and folders dropped on the element, with their paths under
 * `dir`: `{@attach dropUpload({ ... })}`. `dragging` says when something
 * is dragged over it, to show it takes the drop.
 */
export function dropUpload(opts: {
	dir: string;
	/** a read only folder takes no drop */
	disabled: boolean;
	dragging: (over: boolean) => void;
	upload: (items: Upload[]) => void;
}): Attachment<HTMLElement> {
	return el => {
		function ondragover(e: DragEvent) {
			// without it the browser opens the file
			e.preventDefault();
			opts.dragging(!opts.disabled);
		}

		function ondragleave() {
			opts.dragging(false);
		}

		async function ondrop(e: DragEvent) {
			e.preventDefault();
			opts.dragging(false);
			if (opts.disabled || !e.dataTransfer) return;
			const dropped = [...e.dataTransfer.items]
				.map(i => i.webkitGetAsEntry?.())
				.filter(Boolean);
			try {
				opts.upload(await droppedFiles(dropped, opts.dir));
			} catch (err: any) {
				message.show('Upload Failed', err.message);
			}
		}

		el.addEventListener('dragover', ondragover);
		el.addEventListener('dragleave', ondragleave);
		el.addEventListener('drop', ondrop);
		return () => {
			el.removeEventListener('dragover', ondragover);
			el.removeEventListener('dragleave', ondragleave);
			el.removeEventListener('drop', ondrop);
		};
	};
}
