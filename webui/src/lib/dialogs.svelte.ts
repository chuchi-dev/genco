// the desktop's dialog, pages open one with `dialogs.open = 'group'`
export type DialogKind =
	| 'client'
	| 'group'
	| 'domain'
	| 'database'
	| 'mail'
	| 'users'
	| 'password'
	| 'apply'
	| 'settings'
	| 'totp'
	| 'tokens'
	| 'activity'
	| 'passwords';

export const dialogs = $state<{
	open: DialogKind | null;
	/** the password the Passwords dialog selects when it opens */
	password: number | null;
}>({ open: null, password: null });

/** what MessageBox shows: a message, a yes/no question or an input */
type Box = {
	title: string;
	text: string;
	kind: 'message' | 'confirm' | 'ask' | 'password';
	/** the input's value of an `ask` */
	value: string;
	done: (answer: string | null) => void;
};

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
		// a box still open is answered with Cancel, its caller would wait
		// forever otherwise
		this.current?.done(null);
		return new Promise(done => {
			this.current = { kind, title, text, value, done };
		});
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
	 * The admin's own password for what a stolen session alone mustn't do
	 * (invites, tokens), null for Cancel.
	 */
	password(title: string, what: string): Promise<string | null> {
		return this.open('password', title, `Your password, to ${what}:`);
	}

	/** closes the box with an answer, null is No or Cancel */
	answer(value: string | null) {
		const box = this.current;
		this.current = null;
		box?.done(value);
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
	let body: object | null = null;
	if (o.password) {
		const password = await message.password(o.title, o.password);
		if (password === null) return false;
		body = { password };
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
 * confirm it, the message box says so.
 */
export async function requestDelete(path: string, what: string) {
	const { del } = await import('./api');
	const yes = await message.confirm(
		'Delete',
		`Delete ${what}? Every admin gets a mail to confirm it.`,
	);
	if (!yes) return;
	try {
		message.show('Waiting for Confirmation', await del(path));
	} catch (e: any) {
		message.show('Not Deleted', e.message);
	}
}
