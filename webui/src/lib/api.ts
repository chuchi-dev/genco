import { session } from './session.svelte';

// most components take the formats from here, with the requests
export { formatAgo, formatBytes, formatTime } from './format';

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

export type Kind = 'static' | 'php' | 'node' | 'rust' | 'proxy' | 'redirect';
export const KINDS: Kind[] = [
	'static',
	'php',
	'node',
	'rust',
	'proxy',
	'redirect',
];
export const KIND_NAMES: Record<Kind, string> = {
	static: 'Static files',
	php: 'PHP',
	node: 'Node',
	rust: 'Rust binary',
	proxy: 'Proxy',
	redirect: 'Redirect',
};
/** 301 and 308 are permanent, 308 and 307 keep a POST a POST */
export const REDIRECT_STATUSES = [301, 308, 302, 307];
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
	/** of a redirect: where it sends every request */
	target: string | null;
	/** of a redirect: the request's path and query follow the target */
	keepPath: boolean;
	/** of a redirect: 301, 302, 307 or 308 */
	redirectStatus: number;
	/** www.<name> redirects to it (a redirect's to its target) */
	www: boolean;
	/** of an app: restarts start the new process beside the old one */
	zeroDowntime: boolean;
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
	/** where clients reach it, a note: packages.example.com:5426 */
	address: string;
	source: Source;
	/** its unit's ActiveState (active, failed, …), only from /tree */
	state: string | null;
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
	limits: Limits & { defaults: LimitDefaults };
};

/** what a group's slice may use, null is the default */
export type Limits = {
	memoryMaxMb: number | null;
	/** two thirds of the max by default */
	memoryHighMb: number | null;
	cpuPercent: number | null;
	tasksMax: number | null;
};

/** what each of a group's limits is when it is empty, the others kept */
export type LimitDefaults = {
	[K in keyof Limits]: number;
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
	/** the inbox is elsewhere, apps here only send from it */
	sendOnly: boolean;
	mailboxes: Mailbox[];
	forwards: Forward[];
	sendOnlyAddresses: SendOnly[];
};

/** an app's own SMTP login, it sends only as its address */
export type SendOnly = {
	id: number;
	mailDomainId: number;
	address: string;
	note: string;
	createdAt: number;
};

/** what an app needs to send as a send-only address */
export type SendOnlySettings = {
	host: string;
	port: number;
	user: string;
	password: string;
	from: string;
	zippSmtpUrl: string;
	smtpUrl: string;
	/** the mail host has its certificate */
	ready: boolean;
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

/** the mail of an address in the last days, from Postfix's log */
export type AddressCount = {
	/** `*@domain` the catch-all, `g-<group>` the group's SMTP login */
	address: string;
	/** received and forwarded; sent of a send-only address and the
	 * login; rejected of an unknown address */
	count: number;
	last: number | null;
};

export type MailCounts = {
	/** when counting started */
	since: number | null;
	days: number;
	received: number;
	forwarded: number;
	rejected: number;
	bounced: number;
	mailboxes: AddressCount[];
	/** what the mailboxes sent with their login, same order */
	mailboxesSent: AddressCount[];
	forwards: AddressCount[];
	sendOnly: AddressCount[];
	login: AddressCount | null;
	/** rejected addresses without a mailbox or forward, `(others)@domain`
	 * beyond the daily cap */
	unknown: AddressCount[];
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
	/** a drop-in someone else wrote, it overrides genco's unit */
	foreign: boolean;
};

export function fileState(
	f: ConfigFile,
): 'ok' | 'missing' | 'changed' | 'foreign' {
	if (f.foreign) return 'foreign';
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
	/** its build waits for a runner at this place in the queue */
	queue: number | null;
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

/** a version of a proposal's plan, written by genco's planner */
export type Plan = {
	id: number;
	/** 1 for the first plan of the proposal */
	version: number;
	/** the plan exactly as genco's agent gets it; with `refused` why the
	 * planner wrote none */
	text: string;
	refused: boolean;
	/** invisible and control characters removed from the planner's text */
	removedChars: number;
	/** look-alikes of ASCII letters may hide in it */
	mixedScripts: boolean;
	/** the deletions it confirms, e.g. "database shop_old" */
	deletes: string[];
	/** `Delete:` lines genco doesn't understand, those still ask */
	otherDeletes: string[];
	/** the admin's change that led to this version */
	note: string | null;
	noteBy: string | null;
	createdAt: number;
};

export type Proposal = {
	id: number;
	from: string;
	/** the agent's text, cleaned: only the planner reads it */
	text: string;
	/** invisible and control characters removed from it */
	removedChars: number;
	/** letters outside the Latin script, look-alikes may hide in it */
	mixedScripts: boolean;
	/** oldest first, the newest is the one an admin decides on */
	plans: Plan[];
	/** the plan an admin approved */
	planId: number | null;
	/** the change the planner works on now */
	changeNote: string | null;
	changeBy: string | null;
	/** the admin who decides, null for any */
	approver: string | null;
	approverEmail: string | null;
	status:
		| 'planning'
		| 'pending'
		| 'approved'
		| 'answered'
		| 'rejected'
		| 'expired';
	decidedBy: string | null;
	conversationId: number | null;
	/** what went back: the agent's answer, or the reason of a rejection */
	answer: string | null;
	createdAt: number;
	/** when an undecided one expires */
	expiresAt: number;
};

/** a deletion or another change that waits for an admin (actions.rs) */
export type PendingAction = {
	id: number;
	/** "A12", apart from the proposals' "#12" */
	number: string;
	kind:
		| 'delete_domain'
		| 'delete_database'
		| 'delete_group'
		| 'delete_mail_domain'
		| 'delete_mailbox'
		| 'backup_remote_set'
		| 'backup_remote_off';
	description: string;
	requestedBy: string;
	/** the conversation of genco's agent that asked for it */
	conversationId: number | null;
	status: 'pending' | 'running' | 'done' | 'failed' | 'rejected' | 'expired';
	createdAt: number;
	expiresAt: number;
	decidedBy: string | null;
	decidedAt: number | null;
	/** the admin's reason of a rejection */
	reason: string | null;
	/** nothing was mailed (mail off), the password confirms it */
	byPassword: boolean;
	deletion: boolean;
	/** of a database to delete */
	sizeBytes?: number;
};

/** what waits in Start → Approvals, and what was decided in 30 days */
export type Approval =
	| { kind: 'proposal'; proposal: Proposal }
	| { kind: 'action'; action: PendingAction };

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

export type TopPage = {
	path: string;
	requests: number;
	/** answers 400 to 499 */
	clientErrors: number;
	/** 500 and above */
	serverErrors: number;
	bytes: number;
};

export type TopAddress = {
	ip: string;
	requests: number;
	clientErrors: number;
	serverErrors: number;
	/** its most requested path */
	path: string;
};

export type TopPages = {
	pages: TopPage[];
	/** the most requested paths that answered 404 */
	notFound: TopPage[];
	/** the clients with the most requests */
	addresses: TopAddress[];
	/** the oldest request read when the log doesn't cover the period */
	from: number | null;
};

export type Snapshot = {
	id: string;
	time: string;
	tags: string[];
	bytes: number;
};

/** the copy of the backups off the server, GET /settings/backup */
export type BackupSettings = {
	remote: {
		provider: 'b2' | 's3';
		bucket: string;
		path: string;
		endpoint: string;
		region: string;
		keyId: string;
		keep: { daily: number; weekly: number; monthly: number };
		/** KiB/s */
		limitUpload: number | null;
		configuredAt: number;
		location: string;
	} | null;
	state: {
		lastOk: number | null;
		lastError: string | null;
		lastErrorAt: number | null;
		size: number | null;
		sizeAt: number | null;
	};
	defaultPath: string;
	/** genco made a repository password on the agent's request, unseen */
	newPassword: boolean;
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
	| { type: 'approvals' }
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

/** A single-use ticket that lets in the request to `path` (with its query). */
export function ticket(path: string): Promise<string> {
	return post('/users/tickets', { path });
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
	const t = await ticket(target);
	const sep = q ? '&' : '?';
	window.location.href = `${target}${sep}ticket=${encodeURIComponent(t)}`;
}

/**
 * Saves a group: the server takes all its fields, the General and the
 * Deploy tab each change only theirs. The limits only when given; the
 * warning says when the groups' memory limits are more than the server has.
 */
export function saveGroup(
	group: Group,
	change: Partial<
		Pick<Group, 'clientId' | 'notes' | 'repo' | 'branch' | 'php'> & {
			limits: Limits;
		}
	>,
): Promise<{ warning: string | null }> {
	const { clientId, notes, repo, branch, php } = { ...group, ...change };
	return post('/groups/' + group.id, {
		clientId,
		notes,
		repo,
		branch,
		php,
		limits: change.limits,
	});
}
