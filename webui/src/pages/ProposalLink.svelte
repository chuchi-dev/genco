<script lang="ts" module>
	import { ApiError, get, type Proposal } from '@/lib/api';

	export const isPublic = true;

	export const KEY = 'genco-proposal-link';

	export type LinkView = {
		proposal: Proposal;
		/** the admin of the link */
		name: string;
		/** approving asks for the code of the authenticator app */
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

	// the link of a proposal's mail: the admin it went to approves or
	// rejects it without logging in, also on a phone. Opening it changes
	// nothing, mail scanners open links too.
	let { token, link }: { token: string; link: LinkView | null } = $props();

	// the link's token doesn't stay in the address bar and the history
	history.replaceState(history.state, '', '/proposal');

	const touch = matchMedia('(pointer: coarse)').matches;
	let view = $state<'formatted' | 'original'>('formatted');
	let rejecting = $state(false);
	let code = $state('');
	let reason = $state('');
	let comment = $state('');
	// what happened, or that the link doesn't work (anymore)
	let done = $state<string | null>(null);
	let gone = $state(false);

	/** Decides, a link that stopped working meanwhile says so. */
	async function decide(action: string, body: object, text: string) {
		try {
			await post(`/proposal-links/${token}/${action}`, body);
		} catch (e) {
			if (!(e instanceof ApiError && e.type === 'NOT_FOUND')) throw e;
			gone = true;
		}
		if (!gone) done = text;
		sessionStorage.removeItem(KEY);
	}

	const approve = () =>
		decide(
			'approve',
			{ code, comment },
			`Approved: genco's agent works on it, its answer goes back to ${link!.proposal.from}.`,
		);

	const reject = () =>
		decide(
			'reject',
			{ message: reason },
			`Rejected, ${link!.proposal.from} was told.`,
		);
</script>

<div class="page">
	<Window title="Proposal" icon="agent">
		<div class="content">
			{#if gone || !link}
				<p>
					This link no longer works (the proposal was decided or
					expired). Open the panel to see it.
				</p>
			{:else if done}
				<p>{done}</p>
			{:else}
				<ProposalText proposal={link.proposal} {view} />
				{#if rejecting}
					<Form
						onsubmit={reject}
						oncancel={() => (rejecting = false)}
						ok="Reject"
					>
						<label for="pl-reason">Reason:</label>
						<!-- svelte-ignore a11y_autofocus -->
						<textarea
							id="pl-reason"
							rows="4"
							maxlength="2000"
							autofocus
							placeholder="optional, sent to {link.proposal.from}"
							spellcheck={!touch}
							bind:value={reason}
						></textarea>
					</Form>
				{:else}
					<p class="hint">
						Approved, genco's agent works on this task as if you ({link.name})
						asked. Nothing happens without an approval.
					</p>
					<Form onsubmit={approve} ok="Approve">
						<label for="pl-comment">Comment:</label>
						<textarea
							id="pl-comment"
							rows="3"
							maxlength="2000"
							placeholder="optional, for genco's agent, e.g. keep the old DNS for now"
							spellcheck={!touch}
							bind:value={comment}
						></textarea>
						{#if link.totp}
							<label for="pl-code">Two-factor code:</label>
							<input
								id="pl-code"
								inputmode="numeric"
								autocomplete="one-time-code"
								bind:value={code}
							/>
						{/if}
						{#snippet extra()}
							<Button onclick={() => (rejecting = true)}>
								Reject…
							</Button>
							{#if link.proposal.rewrittenBy}
								<Button
									onclick={() =>
										(view =
											view === 'original'
												? 'formatted'
												: 'original')}
								>
									{view === 'original'
										? 'Rewritten'
										: 'Original'}
								</Button>
							{/if}
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
