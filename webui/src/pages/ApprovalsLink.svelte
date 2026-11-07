<script lang="ts" module>
	import { dialogs } from '@/lib/dialogs.svelte';

	// the panel's link in the mails of proposals and deletions: opens
	// Start → Approvals over the server's page
	export function loadProps(
		_: object,
		lp: { setRedirect: (path: string) => void },
	) {
		dialogs.open = 'approvals';
		lp.setRedirect('/');
	}
</script>
