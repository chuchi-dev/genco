<script lang="ts" module>
	export const isPublic = true;
</script>

<script lang="ts">
	import { ApiError, post } from '@/lib/api';
	import { go } from '@/lib/router.svelte';
	import { session } from '@/lib/session.svelte';
	import Window from '@/components/win/Window.svelte';
	import Form from '@/components/win/Form.svelte';
	import Icon from '@/components/win/Icon.svelte';

	let email = $state('');
	let password = $state('');
	// asked for once the password was right and the user has 2FA
	let code = $state<string | null>(null);

	async function login() {
		try {
			const resp = await post('/users/login', { email, password, code });
			session.login(resp.token, resp.user);
			// back to the page that asked for the login
			const url = new URLSearchParams(location.search).get('url');
			go(url?.startsWith('/') ? url : '/');
		} catch (e: any) {
			if (e instanceof ApiError && e.type === 'TOTP_REQUIRED') {
				code = '';
			} else {
				if (code === null) password = '';
				else code = '';
				// the form shows it
				throw e;
			}
		}
	}
</script>

<div class="login">
	<Window title="Welcome to genco" icon="key">
		<div class="content">
			<div class="icon"><Icon name="key" size={32} /></div>
			<div>
				<p>
					Type your email and password to log on to {location.hostname}.
				</p>
				<Form onsubmit={login}>
					<label for="email">Email:</label>
					<!-- svelte-ignore a11y_autofocus -->
					<input
						id="email"
						type="email"
						autocomplete="username"
						autofocus
						bind:value={email}
					/>
					<label for="password">Password:</label>
					<input
						id="password"
						type="password"
						autocomplete="current-password"
						bind:value={password}
					/>
					{#if code !== null}
						<label for="code">Code:</label>
						<!-- svelte-ignore a11y_autofocus -->
						<input
							id="code"
							inputmode="numeric"
							autocomplete="one-time-code"
							placeholder="from the authenticator app"
							autofocus
							bind:value={code}
						/>
					{/if}
				</Form>
			</div>
		</div>
	</Window>
</div>

<style lang="scss">
	.login {
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
