import { get, type Conversation } from './api';

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
