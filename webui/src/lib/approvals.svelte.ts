import { get, type Approval, type PendingAction, type Plan } from './api';

// Start → Approvals: the AI agents' proposals and the pending actions
// (deletions, the agent's remote backup changes); the taskbar shows the
// undecided ones, any admin decides
class ApprovalStore {
	list = $state<Approval[]>([]);

	async load() {
		this.list = await get('/approvals');
	}

	/** what an admin can decide on now */
	get waiting(): Approval[] {
		return this.list.filter(a => status(a) === 'pending');
	}

	/** the proposals the planner still works on */
	get planning(): Approval[] {
		return this.list.filter(a => status(a) === 'planning');
	}

	/** "2 to approve, 1 being planned", each part only when there are some */
	get summary(): string {
		const waiting = this.waiting.length;
		const planning = this.planning.length;
		return [
			waiting ? `${waiting} to approve` : '',
			planning ? `${planning} being planned` : '',
		]
			.filter(Boolean)
			.join(', ');
	}

	/** also the proposals the planner works on */
	get undecided(): Approval[] {
		return this.list.filter(a => undecided(a));
	}
}

export const approvals = new ApprovalStore();

/** "p12" or "a12", one key for both kinds */
export function approvalKey(a: Approval): string {
	return a.kind === 'proposal' ? `p${a.proposal.id}` : `a${a.action.id}`;
}

/** "#12" or "A12" */
export function approvalNumber(a: Approval): string {
	return a.kind === 'proposal' ? `#${a.proposal.id}` : a.action.number;
}

export function status(a: Approval): string {
	return a.kind === 'proposal' ? a.proposal.status : a.action.status;
}

/** waiting for an admin or the planner */
export function undecided(a: Approval): boolean {
	return ['pending', 'planning'].includes(status(a));
}

/** the newest plan of a proposal, the one an admin decides on */
export function newestPlan(a: Approval): Plan | null {
	return a.kind === 'proposal' ? (a.proposal.plans.at(-1) ?? null) : null;
}

const PROPOSAL_STATUS: Record<string, string> = {
	planning: 'being planned',
	pending: 'waiting',
	approved: 'the agent works on it',
	answered: 'answered',
	rejected: 'rejected',
	expired: 'expired',
};

const ACTION_STATUS: Record<PendingAction['status'], string> = {
	pending: 'waiting',
	running: 'running',
	done: 'done',
	failed: 'failed',
	rejected: 'rejected',
	expired: 'expired',
};

/** the status as the list shows it */
export function statusText(a: Approval): string {
	return a.kind === 'proposal'
		? PROPOSAL_STATUS[a.proposal.status]
		: ACTION_STATUS[a.action.status];
}

export function createdAt(a: Approval): number {
	return a.kind === 'proposal' ? a.proposal.createdAt : a.action.createdAt;
}

export function expiresAt(a: Approval): number {
	return a.kind === 'proposal' ? a.proposal.expiresAt : a.action.expiresAt;
}

/** whether this admin can confirm it in the panel: with 2FA, or the
 * password when nothing was mailed (mailed before mail went off still
 * needs 2FA, the server says so); a proposal needs a plan */
export function canConfirm(a: Approval, totp: boolean, mailOff: boolean) {
	if (a.kind === 'proposal' && newestPlan(a)?.refused !== false) return false;
	if (totp) return true;
	return a.kind === 'proposal' ? mailOff : a.action.byPassword;
}

const KINDS: Record<PendingAction['kind'], string> = {
	delete_domain: 'Delete domain',
	delete_database: 'Delete database',
	delete_group: 'Delete group',
	delete_mail_domain: 'Delete mail domain',
	delete_mailbox: 'Delete mailbox',
	backup_remote_set: 'Remote backup change',
	backup_remote_off: 'Remote backup off',
};

export function approvalKind(a: Approval): string {
	return a.kind === 'proposal' ? 'Proposal' : KINDS[a.action.kind];
}

/** the first line of a proposal's plan (of its text before plans
 * existed), the first sentence of an action */
export function approvalTitle(a: Approval): string {
	if (a.kind === 'proposal') {
		const plan = newestPlan(a);
		if (!plan) {
			if (a.proposal.status === 'planning') return 'Being planned…';
			return a.proposal.text.split('\n')[0];
		}
		if (plan.refused) return `No plan: ${plan.text.split('\n')[0]}`;
		return plan.text.split('\n')[0].replace(/^[#\s]+/, '');
	}
	return a.action.description.split('. ')[0].replace(/\.$/, '');
}

/** the taskbar's line of an undecided one */
export function approvalLine(a: Approval): string {
	const from = a.kind === 'proposal' ? `${a.proposal.from}: ` : '';
	return `${approvalNumber(a)} ${from}${approvalTitle(a)}`;
}

/** "Sören", or "genco's agent in Clean up the shop" with its conversation */
export function requestedBy(a: Approval, titles: (id: number) => string) {
	if (a.kind === 'proposal') return `${a.proposal.from} (AI agent)`;
	const c = a.action.conversationId;
	return c === null
		? a.action.requestedBy
		: `genco's agent in “${titles(c)}”`;
}
