<script lang="ts" module>
	import { dialogs } from '@/lib/dialogs.svelte';

	// the link of a proposal's mail: opens Start → AI Agents over the
	// server's page
	export function loadProps(
		_: object,
		lp: { setRedirect: (path: string) => void },
	) {
		dialogs.open = 'agents';
		lp.setRedirect('/');
	}
</script>
