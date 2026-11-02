/**
 * The space around a chart's plot for the axis labels. The left one is
 * per chart, as wide as its longest value.
 */
export const PAD = { top: 8, right: 8, bottom: 18 };

/** 1, 2 or 5 times a power of ten, at least v */
export function niceMax(v: number): number {
	const p = 10 ** Math.floor(Math.log10(v));
	for (const m of [1, 2, 5, 10]) if (m * p >= v) return m * p;
	return v;
}

/** the y of a value in a chart `height` high, from 0 to `top` */
export function yScale(top: number, height: number) {
	const h = height - PAD.top - PAD.bottom;
	return (v: number) => PAD.top + h - (v / top) * h;
}
