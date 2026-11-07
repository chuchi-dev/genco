import { session } from './session.svelte';

// the desktop's dialog, pages open one with `dialogs.open = 'group'`
export type DialogKind =
	| 'client'
	| 'group'
	| 'domain'
	| 'database'
	| 'mail'
	| 'users'
	| 'password'
	| 'account'
	| 'apply'
	| 'settings'
	| 'totp'
	| 'tokens'
	| 'agents'
	| 'approvals'
	| 'find'
	| 'activity'
	| 'passwords';

export const dialogs = $state<{
	open: DialogKind | null;
	/** the password the Passwords dialog selects when it opens */
	password: number | null;
	/** what Approvals selects when it opens: "p12", "a3" */
	approval: string | null;
}>({ open: null, password: null, approval: null });

/** what MessageBox shows: a message, a yes/no question or an input */
type Box = {
	title: string;
	text: string;
	kind: 'message' | 'confirm' | 'ask' | 'password';
	/** the input's value of an `ask` */
	value: string;
	/** a `password` box asks for the two-factor code too */
	code: boolean;
	done: (answer: string | null, code: string) => void;
};

/** what a dangerous request sends along, see `message.password` */
export type Auth = { password: string; code: string };

/**
 * Message boxes like Windows': `show` tells something, `confirm` and `ask`
 * wait for the answer instead of the browser's own confirm() and prompt().
 */
class Message {
	current = $state<Box | null>(null);

	private open(
		kind: Box['kind'],
		title: string,
		text: string,
		value = '',
	): Promise<string | null> {
		return new Promise(done =>
			this.box({ kind, title, text, value, done }),
		);
	}

	private box(b: Omit<Box, 'code'> & { code?: boolean }) {
		// a box still open is answered with Cancel, its caller would wait
		// forever otherwise
		this.current?.done(null, '');
		this.current = { code: false, ...b };
	}

	show(title: string, text: string) {
		this.open('message', title, text);
	}

	/** true for Yes */
	async confirm(title: string, text: string): Promise<boolean> {
		return (await this.open('confirm', title, text)) !== null;
	}

	/** the typed text, null for Cancel */
	ask(title: string, text: string, value = ''): Promise<string | null> {
		return this.open('ask', title, text, value);
	}

	/**
	 * The admin's own password, and the two-factor code when 2FA is on,
	 * for what a stolen session and password mustn't do (invites, tokens,
	 * mail settings), null for Cancel.
	 */
	password(title: string, what: string): Promise<Auth | null> {
		const code = session.user?.totp ?? false;
		const text = code
			? `Your password and a two-factor code, to ${what}:`
			: `Your password, to ${what}:`;
		return new Promise(done =>
			this.box({
				kind: 'password',
				title,
				text,
				value: '',
				code,
				done: (password, code) =>
					done(password === null ? null : { password, code }),
			}),
		);
	}

	/** closes the box with an answer, null is No or Cancel */
	answer(value: string | null, code = '') {
		const box = this.current;
		this.current = null;
		box?.done(value, code);
	}
}

export const message = new Message();

/**
 * Deletes something right away after a Yes (and the admin's password when
 * `password` says what for); false if it wasn't deleted, the reason shows
 * in a message box.
 */
export async function confirmDelete(o: {
	title: string;
	text: string;
	path: string;
	password?: string;
}): Promise<boolean> {
	const { del } = await import('./api');
	if (!(await message.confirm(o.title, o.text))) return false;
	let body: Auth | null = null;
	if (o.password) {
		body = await message.password(o.title, o.password);
		if (body === null) return false;
	}
	try {
		await del(o.path, body);
		return true;
	} catch (e: any) {
		message.show('Not Deleted', e.message);
		return false;
	}
}

/**
 * Asks the server to delete something. It only mails the admins a link to
 * confirm it, the message box says so; with mail off the confirmation
 * page opens, the admin confirms with the password.
 */
export async function requestDelete(path: string, what: string) {
	const { del } = await import('./api');
	const { mail } = await import('./mail.svelte');
	const { go } = await import('./router.svelte');
	const yes = await message.confirm(
		'Delete',
		mail.off
			? `Delete ${what}? You confirm it with your password next.`
			: `Delete ${what}? Every admin gets a mail to confirm it, it waits in Start → Approvals.`,
	);
	if (!yes) return;
	try {
		const req: { message: string; confirm: string | null } =
			await del(path);
		if (req.confirm) go(req.confirm);
		else message.show('Waiting for Confirmation', req.message);
	} catch (e: any) {
		message.show('Not Deleted', e.message);
	}
}
