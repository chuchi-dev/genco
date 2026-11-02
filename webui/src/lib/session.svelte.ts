import { get, type User } from './api';
import { disconnect } from './events.svelte';

const STORAGE_KEY = 'GENCO_TOKEN';

class Session {
	token = $state<string | null>(localStorage.getItem(STORAGE_KEY));
	user = $state<User | null>(null);

	/** Loads the user of a stored token, a rejected one logs out. */
	async init() {
		if (!this.token) return;
		try {
			this.user = await get('/users/me');
		} catch (e) {
			// only a rejected token logs out (see request), not a restart
			console.error('session check failed', e);
		}
	}

	login(token: string, user: User) {
		this.token = token;
		this.user = user;
		localStorage.setItem(STORAGE_KEY, token);
	}

	logout() {
		disconnect();
		this.token = null;
		this.user = null;
		localStorage.removeItem(STORAGE_KEY);
	}
}

export const session = new Session();
