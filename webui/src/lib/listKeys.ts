/**
 * Keyboard for a list view whose rows select on click (`tr.selected`)
 * and open on a double click: ↑ ↓ Home End move, Enter opens. Rows get
 * the same events a mouse sends, the list's own handlers do the rest.
 * `{@attach listKeys}` on the `.list-view`.
 */
export function listKeys(el: HTMLElement) {
	if (!el.hasAttribute('tabindex')) el.tabIndex = 0;

	function onkeydown(e: KeyboardEvent) {
		// inputs in a row keep their keys
		if (e.target !== el) return;
		const rows = [
			...el.querySelectorAll<HTMLTableRowElement>('tbody tr'),
		].filter(r => !r.querySelector('td.empty'));
		if (!rows.length) return;
		const at = rows.findIndex(r => r.classList.contains('selected'));
		let next: number;
		switch (e.key) {
			case 'ArrowDown':
				next = at < 0 ? 0 : Math.min(at + 1, rows.length - 1);
				break;
			case 'ArrowUp':
				next = at < 0 ? 0 : Math.max(at - 1, 0);
				break;
			case 'Home':
				next = 0;
				break;
			case 'End':
				next = rows.length - 1;
				break;
			case 'Enter':
				if (at < 0) return;
				rows[at].dispatchEvent(
					new MouseEvent('dblclick', { bubbles: true }),
				);
				e.preventDefault();
				return;
			default:
				return;
		}
		e.preventDefault();
		rows[next].click();
		rows[next].scrollIntoView({ block: 'nearest' });
	}

	el.addEventListener('keydown', onkeydown);
	return () => el.removeEventListener('keydown', onkeydown);
}
