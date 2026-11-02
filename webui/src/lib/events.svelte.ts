import type { Event } from './api';
import { session } from './session.svelte';

type Listener = (ev: Event) => void;

const listeners = new Set<Listener>();
let source: EventSource | null = null;
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
}

function connect() {
	// a closed stream (e.g. the token was refused) is opened again
	if (source?.readyState === EventSource.CLOSED) source = null;
	if (source || !session.token) return;

	source = new EventSource(
		'/api/events?token=' + encodeURIComponent(session.token),
	);
	source.onerror = () => {
		// EventSource reconnects by itself
		connection.lost = true;
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
