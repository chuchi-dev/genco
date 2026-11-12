<script lang="ts" module>
	import { ApiError, get, type Proposal } from '@/lib/api';

	export const isPublic = true;

	export const KEY = 'genco-proposal-link';

	export type LinkView = {
		proposal: Proposal;
		/** the admin of the link */
		name: string;
		/** approving asks for the code of the authenticator app, otherwise
		 * for the admin's password */
		totp: boolean;
	};

	// the token goes out of the address bar, a reload (back from the
	// authenticator app) takes it from the tab's storage
	export async function loadProps({ token }: { token?: string }) {
		token ||= sessionStorage.getItem(KEY) ?? '';
		if (!token) return { token, link: null };
		sessionStorage.setItem(KEY, token);
		try {
			return {
				token,
				link: (await get(`/proposal-links/${token}`)) as LinkView,
			};
		} catch (e) {
			if (e instanceof ApiError && e.type === 'NOT_FOUND') {
				sessionStorage.removeItem(KEY);
				return { token, link: null };
			}
			throw e;
		}
	}
</script>

<script lang="ts">
	import { post } from '@/lib/api';
	import { go } from '@/lib/router.svelte';
	import Button from '@/components/win/Button.svelte';
	import Form from '@/components/win/Form.svelte';
	import Window from '@/components/win/Window.svelte';
	import ProposalText from '@/components/ProposalText.svelte';
	import NoteField from '@/components/NoteField.svelte';

	// the link of a proposal's mail: the admin it went to approves the
	// plan, sends it back to the planner with a change or rejects the
	// proposal without logging in, also on a phone. Opening it changes
	// nothing, mail scanners open links too.
	let { token, link }: { token: string; link: LinkView | null } = $props();

	// the link's token doesn't stay in the address bar and the history
	history.replaceState(history.state, '', '/proposal');

	let view = $state<'formatted' | 'request'>('formatted');
	let mode = $state<'approve' | 'change' | 'reject'>('approve');
	let code = $state('');
	let password = $state('');
	let reason = $state('');
	let note = $state('');
	// what happened, or that the link doesn't work (anymore)
	let done = $state<string | null>(null);
	let gone = $state(false);

	const proposal = $derived(link?.proposal);
	const plan = $derived(proposal?.plans.at(-1));
	const planning = $derived(proposal?.status === 'planning');

	/** Decides, a link that stopped working meanwhile says so. */
	async function decide(
		action: string,
		body: object,
		text: string,
		end = true,
	) {
		try {
			await post(`/proposal-links/${token}/${action}`, body);
		} catch (e) {
			if (!(e instanceof ApiError && e.type === 'NOT_FOUND')) throw e;
			gone = true;
		}
		if (!gone) done = text;
		if (end || gone) sessionStorage.removeItem(KEY);
	}

	const approve = () =>
		decide(
			'approve',
			link!.totp
				? { plan: plan!.id, code }
				: { plan: plan!.id, password },
			`Approved: genco's agent carries out the plan, its answer goes back to ${proposal!.from}.`,
		);

	// the link stays: it shows the new version once the planner is done
	const change = () =>
		decide(
			'change',
			{ plan: plan!.id, note },
			'Sent to the planner. You get a mail with the new version, or reload this page in a minute.',
			false,
		);

	const reject = () =>
		decide(
			'reject',
			{ message: reason },
			`Rejected, ${proposal!.from} was told.`,
		);
</script>

<div class="page">
	<Window
		title={proposal ? `Proposal #${proposal.id}` : 'Proposal'}
		icon="agent"
	>
		<div class="content">
			{#if gone || !link || !proposal}
				<p>
					This link no longer works (the proposal was decided or
					expired). Open the panel to see it.
				</p>
			{:else if done}
				<p>{done}</p>
			{:else}
				<ProposalText {proposal} {view} />
				{#if mode === 'reject'}
					<Form
						onsubmit={reject}
						oncancel={() => (mode = 'approve')}
						ok="Reject"
					>
						<NoteField
							id="pl-reason"
							label="Reason:"
							placeholder="optional, sent to {proposal.from}"
							autofocus
							bind:value={reason}
						/>
					</Form>
				{:else if mode === 'change' && plan}
					<Form
						onsubmit={change}
						oncancel={() => (mode = 'approve')}
						ok="Send"
					>
						<NoteField
							id="pl-note"
							label="Change:"
							placeholder="e.g. keep the old DNS for now, only the shop"
							autofocus
							bind:value={note}
						/>
					</Form>
				{:else if planning || !plan || plan.refused}
					<p class="hint">
						{planning
							? 'The planner works on a new version, you get a mail when it is there. Reload this page to see it.'
							: 'There is no plan to approve: ask the planner for another try with a change, or reject it.'}
					</p>
					<div class="row-buttons">
						{#if plan && !planning}
							<Button onclick={() => (mode = 'change')}>
								Change…
							</Button>
						{/if}
						<Button onclick={() => (mode = 'reject')}
							>Reject…</Button
						>
						<Button
							onclick={() =>
								(view =
									view === 'request'
										? 'formatted'
										: 'request')}
						>
							{view === 'request' ? 'Plan' : 'Request'}
						</Button>
					</div>
				{:else}
					<p class="hint">
						Approved, genco's agent carries out this plan as if you
						({link.name}) asked. Nothing happens without an
						approval.
					</p>
					<Form onsubmit={approve} ok="Approve">
						{#if link.totp}
							<label for="pl-code">Two-factor code:</label>
							<input
								id="pl-code"
								inputmode="numeric"
								autocomplete="one-time-code"
								bind:value={code}
							/>
						{:else}
							<label for="pl-password">Your password:</label>
							<input
								id="pl-password"
								type="password"
								autocomplete="current-password"
								bind:value={password}
							/>
						{/if}
						{#snippet extra()}
							<Button onclick={() => (mode = 'change')}>
								Change…
							</Button>
							<Button onclick={() => (mode = 'reject')}>
								Reject…
							</Button>
							<Button
								onclick={() =>
									(view =
										view === 'request'
											? 'formatted'
											: 'request')}
							>
								{view === 'request' ? 'Plan' : 'Request'}
							</Button>
						{/snippet}
					</Form>
				{/if}
			{/if}
			{#if gone || done || !link}
				<div class="row-buttons end">
					<Button primary onclick={() => go('/agents')}>
						Open Panel
					</Button>
				</div>
			{/if}
		</div>
	</Window>
</div>

<style lang="scss">
	// one window the width of a phone, or a little more
	.page {
		position: fixed;
		inset: 0;
		overflow: auto;
		padding: 8px;
		display: flex;
		justify-content: center;
		align-items: flex-start;

		:global(.window) {
			width: 720px;
			max-width: 100%;
		}
	}

	.content {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 8px;
	}

	p {
		margin: 0 0 8px;
	}
</style>
