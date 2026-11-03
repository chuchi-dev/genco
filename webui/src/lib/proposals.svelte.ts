import { get, type Proposal } from './api';

// the AI agents' proposals, the taskbar shows the waiting ones (any admin
// decides) and Start → AI Agents lists them all
class ProposalStore {
	list = $state<Proposal[]>([]);

	async load() {
		this.list = await get('/proposals');
	}

	get waiting(): Proposal[] {
		return this.list.filter(p => p.status === 'pending');
	}
}

export const proposals = new ProposalStore();
