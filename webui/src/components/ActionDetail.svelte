<script lang="ts">
	import { formatBytes, formatTime, type PendingAction } from '@/lib/api';

	// a pending action in Start → Approvals: what exactly happens when it
	// is confirmed, and who decided
	let { action, by }: { action: PendingAction; by: string } = $props();

	const STATUS: Record<PendingAction['status'], string> = {
		pending: 'Waits for an admin to confirm or reject it.',
		running: 'Confirmed, it runs.',
		done: 'Done.',
		failed: 'It failed, the activity log has the error.',
		rejected: 'Rejected, it didn’t run.',
		expired: 'Nobody confirmed it within 24 hours, it didn’t run.',
	};
</script>

<p><b>{by}</b> asked for this ({action.number}):</p>
<div class="text selectable">
	<p>{action.description}</p>
	{#if action.sizeBytes !== undefined}
		<p>The database has {formatBytes(action.sizeBytes)} now.</p>
	{/if}
</div>
<p class="hint">
	Requested {formatTime(action.createdAt)}{action.status === 'pending'
		? `, expires ${formatTime(action.expiresAt)}`
		: ''}.
	{STATUS[action.status]}
	{#if action.decidedBy}
		{action.status === 'rejected' ? 'Rejected' : 'Confirmed'} by
		{action.decidedBy}{action.decidedAt
			? ` at ${formatTime(action.decidedAt)}`
			: ''}.
	{/if}
</p>
{#if action.reason}
	<p>
		The reason{action.conversationId ? ', sent to genco’s agent' : ''}:
	</p>
	<div class="text selectable"><p>{action.reason}</p></div>
{/if}

<style lang="scss">
	@use 'styles/win' as *;

	p {
		margin: 8px 0 4px;
	}

	.text {
		@include sunken;
		flex: 0 1 auto;
		overflow: auto;
		padding: 0 8px;
		background: #fff;

		p {
			white-space: pre-wrap;
		}
	}
</style>
