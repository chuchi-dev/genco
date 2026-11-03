import { ticket, type Event } from './api';
import { session } from './session.svelte';

type Listener = (ev: Event) => void;

const listeners = new Set<Listener>();
let source: EventSource | null = null;
// a ticket is being asked for or a reconnect waits
let opening = false;
let retry: ReturnType<typeof setTimeout> | null = null;
// counts disconnects, a ticket asked for before one is not used
let generation = 0;
// the server process we talk to, a new one has a new webui
let boot: number | null = null;

export const connection = $state({ lost: false });

/** Calls `fn` for every event until the returned function is called. */
export function listen(fn: Listener): () => void {
	listeners.add(fn);
	connect();
	return () => listeners.delete(fn);
}

/** Ends the stream, the next listener opens a new one with the new token. */
export function disconnect() {
	source?.close();
	source = null;
	boot = null;
	generation++;
	if (retry) clearTimeout(retry);
	retry = null;
}

/**
 * Opens the stream with a single-use ticket, the session token stays out
 * of the URL (a proxy in front logs it).
 */
async function connect() {
	if (source || opening || retry || !session.token) return;

	opening = true;
	const gen = generation;
	let t: string;
	try {
		t = await ticket('/api/events');
	} catch (_e) {
		// a refused session logs out (see request), which disconnects
		opening = false;
		if (gen === generation) reconnectLater();
		return;
	}
	opening = false;
	// logged out meanwhile, maybe in again
	if (gen !== generation) return connect();

	source = new EventSource('/api/events?ticket=' + encodeURIComponent(t));
	source.onerror = () => {
		// the automatic retry would reuse the used ticket
		source?.close();
		source = null;
		reconnectLater();
	};
	source.onmessage = msg => {
		const ev: Event = JSON.parse(msg.data);

		if (ev.type === 'hello') {
			if (boot !== null && boot !== ev.boot) {
				window.location.reload();
				return;
			}
			const reconnect = boot !== null;
			boot = ev.boot;
			connection.lost = false;
			// the pages load their data themselves, only after a lost
			// connection they need to load what they missed
			if (!reconnect) return;
		}

		listeners.forEach(fn => fn(ev));
	};
}

function reconnectLater() {
	connection.lost = true;
	retry = setTimeout(() => {
		retry = null;
		connect();
	}, 3000);
}
