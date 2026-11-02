import {
	get,
	post,
	type Conversation,
	type Event,
	type Message,
	type Routine,
} from './api';
import { conversations } from './conversations.svelte';
import { go, setChat } from './router.svelte';

/**
 * The conversation the agent window shows: its messages, merged with
 * those the events bring, and the answer being streamed. `follow` is
 * called on new text to scroll down, `always` even if the reader
 * scrolled up.
 */
export class Chat {
	routines = $state<Routine[]>([]);
	// null is a new conversation, kept with setChat
	current = $state<number | null>(null);
	messages = $state<Message[]>([]);
	// streamed text of the answer that is being written
	streaming = $state('');
	error = $state<string | null>(null);

	conversation = $derived(
		conversations.list.find(c => c.id === this.current) ?? null,
	);
	running = $derived(this.conversation?.status === 'running');
	// the output of each tool call, shown with the call
	outputs = $derived(
		new Map(
			this.messages
				.filter(m => m.kind === 'toolOutput')
				.map(m => [m.callId, m]),
		),
	);

	#follow: (always: boolean) => void;
	// the first load opens the conversation of the url or the latest one
	#first = true;

	constructor(current: number | null, follow: (always: boolean) => void) {
		this.current = current;
		this.#follow = follow;
	}

	async load() {
		try {
			await conversations.load();
			this.routines = await get('/routines');
		} catch (e: any) {
			this.error = e.message;
			return;
		}
		if (this.#first) {
			// the stored one may be gone
			const list = conversations.list;
			const known = list.some(c => c.id === this.current);
			const latest = list.find(c => c.kind === 'chat')?.id ?? null;
			const id = (known ? this.current : null) ?? latest;
			if (id !== null) await this.select(id);
		}
		this.#first = false;
	}

	async select(id: number | null) {
		this.current = id;
		setChat(id);
		this.streaming = '';
		this.messages = [];
		if (id === null) return;
		try {
			const resp = await get('/conversations/' + id);
			if (this.current !== id) return;
			// messages the events brought while it loaded stay
			const loaded = new Set(resp.messages.map((m: Message) => m.id));
			this.messages = [
				...resp.messages,
				...this.messages.filter(m => !loaded.has(m.id)),
			].sort((a, b) => a.id - b.id);
			this.error = null;
		} catch (e: any) {
			if (this.current === id) this.error = e.message;
		}
		this.#follow(true);
	}

	onEvent(ev: Event) {
		if (ev.type === 'hello') {
			this.load();
			if (this.current !== null) this.select(this.current);
		} else if (ev.type === 'conversation') {
			conversations.update(ev.conversation);
		} else if (ev.type === 'message') {
			const m = ev.message;
			if (m.conversationId !== this.current) return;
			if (this.messages.some(x => x.id === m.id)) return;
			if (m.kind === 'assistant') this.streaming = '';
			this.messages.push(m);
			this.#follow(false);
		} else if (ev.type === 'showPage') {
			// the agent shows a page to who follows its conversation
			if (ev.conversationId === this.current) go(ev.path);
		} else if (ev.type === 'delta') {
			if (ev.conversationId !== this.current) return;
			this.streaming += ev.text;
			this.#follow(false);
		}
	}

	/**
	 * Sends a message, a new conversation starts with the first one. An
	 * error shows and is thrown, the composer gives the text back.
	 */
	async send(text: string, files: string[] = []) {
		this.error = null;
		try {
			if (this.current === null) {
				const c: Conversation = await post('/conversations', {
					text,
					files,
				});
				conversations.update(c);
				await this.select(c.id);
			} else {
				await post(`/conversations/${this.current}/messages`, {
					text,
					files,
				});
			}
		} catch (e: any) {
			this.error = e.message;
			throw e;
		}
	}

	async runRoutine(r: Routine) {
		this.error = null;
		try {
			const c: Conversation = await post(`/routines/${r.name}/run`);
			conversations.update(c);
			await this.select(c.id);
		} catch (e: any) {
			this.error = e.message;
		}
	}

	stop() {
		if (this.current === null) return;
		post(`/conversations/${this.current}/stop`).catch(
			(e: any) => (this.error = e.message),
		);
	}

	/** an admin took care of what a routine or alert reported */
	resolve() {
		post(`/conversations/${this.current}/resolve`).catch(
			(e: any) => (this.error = e.message),
		);
	}
}
