/** day and time, `short` without the year (the chat) */
export function formatTime(ms: number, short = false): string {
	return new Date(ms).toLocaleString([], {
		day: '2-digit',
		month: '2-digit',
		year: short ? undefined : 'numeric',
		hour: '2-digit',
		minute: '2-digit',
	});
}

/** hours and minutes */
export function formatHour(ms: number): string {
	return new Date(ms).toLocaleTimeString([], {
		hour: '2-digit',
		minute: '2-digit',
	});
}

/** the time of today, the day and month of an earlier day */
export function formatRecent(ms: number): string {
	const d = new Date(ms);
	return new Date().toDateString() === d.toDateString()
		? formatHour(ms)
		: d.toLocaleDateString([], { day: '2-digit', month: '2-digit' });
}

export function formatBytes(n: number): string {
	if (n < 1024) return n + ' bytes';
	const units = ['KB', 'MB', 'GB', 'TB'];
	let i = -1;
	do {
		n /= 1024;
		i++;
	} while (n >= 1024 && i < units.length - 1);
	return n.toFixed(n < 10 ? 1 : 0) + ' ' + units[i];
}

/** dollars with cents, what costs less shows as such and not as $0.00 */
export function formatUsd(v: number): string {
	return v === 0 ? '$0' : v < 0.01 ? '<$0.01' : `$${v.toFixed(2)}`;
}
