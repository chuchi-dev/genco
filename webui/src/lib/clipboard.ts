/** copies text, with a readable error where the browser has no clipboard
 * (plain http) */
export async function copyText(text: string): Promise<void> {
	if (!navigator.clipboard)
		throw new Error('This page may not copy, select it and copy by hand.');

	await navigator.clipboard.writeText(text);
}
