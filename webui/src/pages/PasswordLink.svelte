<script lang="ts" module>
	import { dialogs } from '@/lib/dialogs.svelte';

	// a link to a password (the agent writes them): opens Start →
	// Passwords on it over the server's page
	export function loadProps(
		{ id }: { id: string },
		lp: { setRedirect: (path: string) => void },
	) {
		dialogs.password = Number(id);
		dialogs.open = 'passwords';
		lp.setRedirect('/');
	}
</script>
