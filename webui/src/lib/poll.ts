/**
 * Calls `tick` every `ms`, the next call only after the last one ended,
 * until it returns false or the returned function stops it (an $effect
 * can return that). An answer after the stop is ignored by `stopped()`.
 */
export function poll(
	tick: (stopped: () => boolean) => Promise<boolean>,
	ms: number,
): () => void {
	let stopped = false;
	let timer: ReturnType<typeof setTimeout>;
	const next = async () => {
		if (stopped) return;
		if (await tick(() => stopped)) timer = setTimeout(next, ms);
	};
	timer = setTimeout(next, ms);
	return () => {
		stopped = true;
		clearTimeout(timer);
	};
}
