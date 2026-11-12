import {
	get,
	post,
	type Conversation,
	type Event,
	type Message,
	type Routine,
} from './api';
import { conversations } from './conversations.svelte';
import { current as page, go, setChat } from './router.svelte';

/** a reload after this long without a message starts a new conversation */
const STALE_MS = 60 * 60 * 1000;

/**
 * The conversation the agent window shows: its messages, merged with
 * those the events bring, and the answer being streamed. `follow` is
 * called when a conversation opens or the reader sends, to go to its end.
 */
export class Chat {
	routines = $state<Routine[]>([]);
	// null is a new conversation, kept with setChat
	current = $state<number | null>(null);
	messages = $state<Message[]>([]);
	// streamed text of the answer that is being written
	streaming = $state('');
	error = $state<string | null>(null);
	// the messages of the conversation are on their way
	loading = $state(true);
	// the old conversation a new one replaced on a reload, to continue it
	resume = $state<number | null>(null);

	conversation = $derived(
		conversations.list.find(c => c.id === this.current) ?? null,
	);
	running = $derived(this.conversation?.status === 'running');
	// the agent works but has nothing to show yet: no text, no tool call
	thinking = $derived.by(() => {
		const last = this.messages.at(-1)?.kind;
		return (
			this.running &&
			!this.streaming &&
			(last === 'user' || last === 'toolOutput' || last === 'system')
		);
	});
	// the output of each tool call, shown with the call
	outputs = $derived(
		new Map(
			this.messages
				.filter(m => m.kind === 'toolOutput')
				.map(m => [m.callId, m]),
		),
	);

	#follow: () => void;
	// the first load opens the conversation of the url or the latest one
	#first = true;

	constructor(current: number | null, follow: () => void) {
		this.current = current;
		this.#follow = follow;
	}

	async load() {
		try {
			await conversations.load();
			this.routines = await get('/routines');
		} catch (e: any) {
			this.error = e.message;
			this.loading = false;
			return;
		}
		if (this.#first) {
			// the stored one may be gone
			const list = conversations.list;
			const known = list.some(c => c.id === this.current);
			const latest = list.find(c => c.kind === 'chat')?.id ?? null;
			const id = (known ? this.current : null) ?? latest;
			// after a break a new one, unless a link asked for this one
			const c = list.find(c => c.id === id);
			const stale =
				c &&
				!page.linked &&
				c.status !== 'running' &&
				Date.now() - c.updatedAt > STALE_MS;
			if (stale) {
				await this.select(null);
				this.resume = c.id;
			} else {
				await this.select(id);
			}
		}
		this.#first = false;
	}

	async select(id: number | null) {
		this.current = id;
		setChat(id);
		this.streaming = '';
		this.messages = [];
		this.#follow();
		this.loading = id !== null;
		if (id === null) return;
		this.resume = null;
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
		if (this.current === id) this.loading = false;
		this.#follow();
	}

	onEvent(ev: Event) {
		if (ev.type === 'hello') {
			this.load();
			if (this.current !== null) this.select(this.current);
		} else if (ev.type === 'conversation') {
			conversations.update(ev.conversation);
		} else if (ev.type === 'tree') {
			// the agent's commands write the database in their own process,
			// e.g. `genco attention resolve`
			conversations.load().catch(() => {});
		} else if (ev.type === 'message') {
			const m = ev.message;
			if (m.conversationId !== this.current) return;
			if (this.messages.some(x => x.id === m.id)) return;
			if (m.kind === 'assistant') this.streaming = '';
			this.messages.push(m);
		} else if (ev.type === 'showPage') {
			// the agent shows a page to who follows its conversation
			if (ev.conversationId === this.current) go(ev.path);
		} else if (ev.type === 'delta') {
			if (ev.conversationId !== this.current) return;
			this.streaming += ev.text;
		}
	}

	/**
	 * Sends a message, a new conversation starts with the first one. An
	 * error shows and is thrown, the composer gives the text back.
	 */
	async send(text: string, files: string[] = []) {
		this.error = null;
		this.#follow();
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
