import { session } from './session.svelte';

// most components take the formats from here, with the requests
export { formatBytes, formatTime } from './format';

export class ApiError extends Error {
	type: string;

	constructor(message: string, type: string) {
		super(message);
		this.type = type;
	}
}

export async function request(
	method: string,
	path: string,
	data: object | null = null,
	text = false,
): Promise<any> {
	const headers: Record<string, string> = {
		'content-type': 'application/json',
	};
	if (session.token) headers['session-token'] = session.token;

	let resp: Response;
	try {
		resp = await fetch('/api' + path, {
			method,
			headers,
			body: method === 'GET' ? undefined : JSON.stringify(data),
		});
	} catch (e: any) {
		throw new ApiError('The server is not reachable.', 'REQUEST');
	}

	if (!resp.ok) {
		let err: ApiError;
		try {
			const obj = await resp.json();
			err = new ApiError(obj.message, obj.error.type);
		} catch (_e) {
			err = new ApiError(resp.statusText, 'INTERNAL');
		}

		// the session is gone, start over
		if (err.type === 'INVALID_SESSION_TOKEN') {
			session.logout();
			window.location.href = '/login';
		}

		throw err;
	}

	return text ? await resp.text() : await resp.json();
}

export const get = (path: string) => request('GET', path);
export const post = (path: string, data: object | null = null) =>
	request('POST', path, data);
export const del = (path: string, data: object | null = null) =>
	request('DELETE', path, data);

export type User = {
	id: number;
	email: string;
	name: string;
	createdAt: number;
	totp: boolean;
	/** hasn't set a password from the invite yet */
	invited: boolean;
};

/** a line of the activity log */
export type Activity = {
	id: number;
	at: number;
	/** an admin, `token <name>`, agent, terminal, systemd, genco */
	actor: string;
	action: string;
	target: string;
	detail: string;
	/** null when the outcome isn't known */
	ok: boolean | null;
};

export type Kind = 'static' | 'php' | 'node' | 'rust' | 'proxy';
export const KINDS: Kind[] = ['static', 'php', 'node', 'rust', 'proxy'];
export const KIND_NAMES: Record<Kind, string> = {
	static: 'Static files',
	php: 'PHP',
	node: 'Node',
	rust: 'Rust binary',
	proxy: 'Proxy',
};
export const PHP_VERSIONS = ['8.2', '8.3', '8.4', '8.5'];

export type Domain = {
	id: number;
	groupId: number;
	name: string;
	kind: Kind;
	phpVersion: string | null;
	root: string;
	command: string | null;
	/** of an app: files nginx serves itself, relative to the group */
	publicDir: string | null;
	nginxExtra: string;
	/** of a proxy: where it forwards to */
	upstream: string | null;
	/** www.<name> redirects to it */
	www: boolean;
};

/** 'repo': from genco.toml, changes only with the repository */
export type Source = 'panel' | 'repo';

/** a command of a group that runs all the time */
export type Worker = {
	id: number;
	groupId: number;
	name: string;
	dir: string;
	command: string;
	/** open to the internet, the worker gets it as PORT */
	port: number | null;
	source: Source;
};

/** a command of a group that runs at times */
export type Schedule = {
	id: number;
	groupId: number;
	name: string;
	dir: string;
	command: string;
	/** systemd's OnCalendar */
	every: string;
	/** minutes */
	timeout: number;
	source: Source;
};

export type Database = {
	id: number;
	groupId: number;
	name: string;
	engine: 'mariadb' | 'postgres';
};

export type Group = {
	id: number;
	name: string;
	clientId: number;
	notes: string;
	user: string;
	dir: string;
	repo: string | null;
	branch: string;
	domains: Domain[];
	databases: Database[];
	mailDomains: MailDomain[];
	workers: Worker[];
	schedules: Schedule[];
	php: PhpSettings;
};

/** what a group's PHP may use */
export type PhpSettings = {
	uploadMb: number;
	memoryMb: number;
	maxExecutionS: number;
	maxInputVars: number;
};

export type MailDomain = {
	id: number;
	groupId: number;
	name: string;
	mailboxes: Mailbox[];
	forwards: Forward[];
};

export type Mailbox = {
	id: number;
	mailDomainId: number;
	address: string;
	quotaMb: number;
};

export type Forward = {
	id: number;
	mailDomainId: number;
	/** `*@domain` is the catch-all */
	address: string;
	targets: string[];
};

export type DnsRecord = {
	name: string;
	kind: string;
	/** what to enter */
	value: string;
	/** what the DNS answers now */
	found: string[];
	status: 'ok' | 'missing' | 'differs' | 'optional';
	note: string;
};

export type Client = {
	id: number;
	name: string;
	contact: string;
	notes: string;
	groups: Group[];
};

export type Unit = {
	unit: string;
	load: string;
	active: string;
	sub: string;
	description: string;
};

/** a file as genco writes it and as it is on the disk */
export type ConfigFile = {
	path: string;
	expected: string;
	actual: string | null;
};

export function fileState(f: ConfigFile): 'ok' | 'missing' | 'changed' {
	if (f.actual === null) return 'missing';
	return f.actual === f.expected ? 'ok' : 'changed';
}

export type ApplyReport = { ok: boolean; log: string[] };

export type DatabaseInfo = {
	name: string;
	exists: boolean;
	sizeBytes: number;
	tables: number;
};

export type Conversation = {
	id: number;
	title: string;
	status: 'idle' | 'running' | 'failed';
	kind: 'chat' | 'routine' | 'alert';
	/** how the last run of a routine or alert ended */
	report: 'ok' | 'attention' | null;
	seen: number;
	createdAt: number;
	updatedAt: number;
};

export type Routine = {
	name: string;
	title: string;
	/** local time of the server, HH:MM */
	time: string;
	/** 1 is Monday, null every day */
	weekday: number | null;
};

/** a file an admin attached to a message, the agent reads it at `path` */
export type Attachment = {
	id: string;
	name: string;
	size: number;
	path: string;
};

export type MessageData =
	| { kind: 'user'; text: string; userName?: string; files?: Attachment[] }
	| { kind: 'system'; text: string }
	| { kind: 'assistant'; text: string }
	| { kind: 'toolCall'; callId: string; name: string; input: any }
	| {
			kind: 'toolOutput';
			callId: string;
			output: string;
			isError: boolean;
	  }
	| { kind: 'error'; text: string };

export type Message = {
	id: number;
	conversationId: number;
	createdAt: number;
} & MessageData;

export type Deploy = {
	id: number;
	groupId: number;
	ref: string;
	commitId: string | null;
	release: string | null;
	status: 'queued' | 'running' | 'done' | 'failed';
	startedBy: string;
	createdAt: number;
	finishedAt: number | null;
	/** its release is the one that runs, set by the list only */
	live: boolean;
};

export type Token = {
	id: number;
	name: string;
	scopes: string[];
	userName: string | null;
	createdAt: number;
	lastUsed: number | null;
};

export type Peer = {
	id: number;
	name: string;
	webhookUrl: string;
	webhookAll: boolean;
	createdAt: number;
	lastUsed: number | null;
};

export type Proposal = {
	id: number;
	from: string;
	text: string;
	/** the admin who decides, null for any */
	approver: string | null;
	approverEmail: string | null;
	status: 'pending' | 'approved' | 'answered' | 'rejected' | 'expired';
	decidedBy: string | null;
	conversationId: number | null;
	answer: string | null;
	createdAt: number;
};

export type Backup = {
	id: number;
	status: 'running' | 'done' | 'failed';
	startedBy: string;
	createdAt: number;
	finishedAt: number | null;
	/** a backup of only this group */
	groupId: number | null;
	groupName: string | null;
};

/** one point of the usage stats; counters are totals of the period */
export type Sample = {
	at: number;
	/** percent of all cores */
	cpu: number | null;
	memory: number | null;
	disk: number | null;
	rx: number | null;
	tx: number | null;
	requests: number | null;
	errors: number | null;
};

export type Stats = {
	/** seconds per point */
	period: number;
	memoryLimit: number | null;
	diskLimit: number | null;
	points: Sample[];
	/** of a group: each domain over the range */
	domains: { name: string; requests: number; errors: number; tx: number }[];
};

export type Snapshot = {
	id: string;
	time: string;
	tags: string[];
	bytes: number;
};

/** what a snapshot holds: files, database x or system */
export function snapshotKind(s: Snapshot): string {
	const db = s.tags.find(t => t.startsWith('db:'));
	if (db) return 'database ' + db.slice(3);
	if (s.tags.includes('system')) return 'system';
	if (s.tags.includes('mail')) return 'mail';
	return 'files';
}

export type Event =
	| { type: 'hello'; boot: number }
	| { type: 'tree' }
	| { type: 'proposals' }
	| { type: 'conversation'; conversation: Conversation }
	| { type: 'message'; message: Message }
	| { type: 'deploy'; deploy: Deploy }
	| { type: 'backup'; backup: Backup }
	| { type: 'delta'; conversationId: number; text: string }
	| { type: 'showPage'; conversationId: number; path: string };

/**
 * For loads that run again (on events, on a new tab): only the answer of
 * the newest load counts, a slow older one doesn't overwrite it.
 */
export function newest() {
	let seq = 0;
	return <T>(
		load: Promise<T>,
		then: (value: T) => void,
		onerror: (e: any) => void = () => {},
	) => {
		const n = ++seq;
		load.then(
			v => n === seq && then(v),
			e => n === seq && onerror(e),
		);
	};
}

/**
 * Downloads what `path` returns. The browser opens it itself (a link
 * would go through the router), with a single-use ticket in the query:
 * the session token stays out of the URL and the download list.
 */
export async function download(
	path: string,
	params: Record<string, string> = {},
) {
	// the ticket is for this path with exactly these parameters
	const q = new URLSearchParams(params).toString();
	const target = q ? `/api${path}?${q}` : `/api${path}`;
	const ticket: string = await post('/users/tickets', { path: target });
	const sep = q ? '&' : '?';
	window.location.href = `${target}${sep}ticket=${encodeURIComponent(ticket)}`;
}

/**
 * Saves a group: the server takes all its fields, the General and the
 * Deploy tab each change only theirs.
 */
export function saveGroup(
	group: Group,
	change: Partial<
		Pick<Group, 'clientId' | 'notes' | 'repo' | 'branch' | 'php'>
	>,
) {
	const { clientId, notes, repo, branch, php } = { ...group, ...change };
	return post('/groups/' + group.id, { clientId, notes, repo, branch, php });
}
