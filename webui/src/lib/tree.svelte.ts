import {
	get,
	type Client,
	type Database,
	type Domain,
	type Group,
	type MailDomain,
} from './api';

// clients with their groups, domains and databases, reloaded on `tree`
// events
class TreeStore {
	clients = $state<Client[]>([]);
	loaded = $state(false);
	error = $state<string | null>(null);

	// events come in bursts, an older answer must not win; every call
	// still finishes, the first page waits for it
	private seq = 0;

	async load() {
		const n = ++this.seq;
		try {
			const clients: Client[] = await get('/tree');
			if (n === this.seq) {
				this.clients = clients;
				this.error = null;
			}
		} catch (e: any) {
			if (n === this.seq) this.error = e.message;
		}
		this.loaded = true;
	}

	get groups(): Group[] {
		return this.clients.flatMap(c => c.groups);
	}

	client(id: number): Client | undefined {
		return this.clients.find(c => c.id === id);
	}

	group(id: number): Group | undefined {
		return this.groups.find(g => g.id === id);
	}

	domain(id: number): Domain | undefined {
		return this.groups.flatMap(g => g.domains).find(d => d.id === id);
	}

	mailDomain(id: number): MailDomain | undefined {
		return this.groups.flatMap(g => g.mailDomains).find(d => d.id === id);
	}

	database(id: number): Database | undefined {
		return this.groups.flatMap(g => g.databases).find(d => d.id === id);
	}
}

export const tree = new TreeStore();
