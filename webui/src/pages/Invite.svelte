<script lang="ts" module>
	import { get, type User } from '@/lib/api';

	export const isPublic = true;

	export const KEY = 'genco-invite';

	// the token goes out of the address bar, a reload takes it from the
	// tab's storage
	export async function loadProps({ token }: { token?: string }) {
		token ||= sessionStorage.getItem(KEY) ?? '';
		if (!token) throw new Error('This invite link is incomplete.');
		sessionStorage.setItem(KEY, token);
		return {
			token,
			user: (await get(`/users/invite/${token}`)) as User,
		};
	}
</script>

<script lang="ts">
	import { post } from '@/lib/api';
	import { go } from '@/lib/router.svelte';
	import Window from '@/components/win/Window.svelte';
	import Form from '@/components/win/Form.svelte';
	import Icon from '@/components/win/Icon.svelte';

	// the link of an invite mail: the new admin sets a password
	let { token, user }: { token: string; user: User } = $props();

	// the link's token doesn't stay in the address bar and the history
	history.replaceState(history.state, '', '/invite');

	let password = $state('');
	let repeat = $state('');

	async function save() {
		if (password !== repeat) throw new Error("The passwords don't match");
		await post(`/users/invite/${token}`, { password });
		sessionStorage.removeItem(KEY);
		go('/login');
	}
</script>

<div class="invite">
	<Window title="Welcome to genco" icon="key">
		<div class="content">
			<div class="icon"><Icon name="key" size={32} /></div>
			<div>
				<p>
					Hi {user.name}, choose a password for {user.email} on {location.hostname}.
					At least 10 characters.
				</p>
				<Form onsubmit={save} ok="Save">
					<label for="password">Password:</label>
					<!-- svelte-ignore a11y_autofocus -->
					<input
						id="password"
						type="password"
						autocomplete="new-password"
						autofocus
						bind:value={password}
					/>
					<label for="repeat">Repeat:</label>
					<input
						id="repeat"
						type="password"
						autocomplete="new-password"
						bind:value={repeat}
					/>
				</Form>
			</div>
		</div>
	</Window>
</div>

<style lang="scss">
	.invite {
		position: fixed;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 8px;

		:global(.window) {
			width: 420px;
			max-width: 100%;
		}
	}

	.content {
		display: flex;
		gap: 14px;
		padding: 12px 10px 8px;
	}

	.icon {
		padding-top: 2px;
	}

	p {
		margin: 0 0 14px;
	}
</style>
