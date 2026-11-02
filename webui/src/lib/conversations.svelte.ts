import { get, newest, type Conversation } from './api';

// the agent's conversations, the agent window keeps them up to date and
// the taskbar shows the routines and alerts that need attention
class ConversationStore {
	list = $state<Conversation[]>([]);

	async load() {
		this.list = await get('/conversations');
	}

	/** a new one or a changed status */
	update(c: Conversation) {
		const i = this.list.findIndex(x => x.id === c.id);
		if (i >= 0) this.list[i] = c;
		else this.list = [c, ...this.list];
	}

	get attention(): Conversation[] {
		return this.list.filter(c => c.report === 'attention');
	}
}

export const conversations = new ConversationStore();

/**
 * The ids of conversations whose messages contain `query()`, searched on
 * the server from three letters on, a quarter second after the typing.
 * Call it while a component starts, it stops with it.
 */
export function messageSearch(query: () => string) {
	const found = $state({ ids: [] as number[] });
	const search = newest();
	$effect(() => {
		const text = query();
		found.ids = [];
		if (text.length < 3) return;
		const t = setTimeout(() => {
			search(
				get('/conversations/search?q=' + encodeURIComponent(text)),
				(ids: number[]) => (found.ids = ids),
			);
		}, 250);
		return () => clearTimeout(t);
	});
	return found;
}
