import { get, type Proposal } from './api';
import { session } from './session.svelte';

// the AI agents' proposals, the taskbar shows the ones waiting for the
// logged in admin and Start → AI Agents lists them all
class ProposalStore {
	list = $state<Proposal[]>([]);

	async load() {
		this.list = await get('/proposals');
	}

	/** pending ones this admin may decide on */
	get mine(): Proposal[] {
		const email = session.user?.email;
		return this.list.filter(
			p =>
				p.status === 'pending' &&
				(p.approverEmail === null || p.approverEmail === email),
		);
	}
}

export const proposals = new ProposalStore();
