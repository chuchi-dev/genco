/**
 * Follows the pointer from a pointerdown until it is released or the
 * browser cancels it (a touch turning into a scroll): `move` gets how far
 * it went, `end` runs at the end.
 */
export function track(
	e: PointerEvent,
	move: (dx: number, dy: number) => void,
	end?: () => void,
) {
	e.preventDefault();
	const target = e.currentTarget as Element;
	target.setPointerCapture(e.pointerId);
	const sx = e.clientX;
	const sy = e.clientY;

	const onmove = (ev: Event) => {
		const p = ev as PointerEvent;
		move(p.clientX - sx, p.clientY - sy);
	};
	const onend = () => {
		target.removeEventListener('pointermove', onmove);
		target.removeEventListener('pointerup', onend);
		target.removeEventListener('pointercancel', onend);
		end?.();
	};
	target.addEventListener('pointermove', onmove);
	target.addEventListener('pointerup', onend);
	target.addEventListener('pointercancel', onend);
}
