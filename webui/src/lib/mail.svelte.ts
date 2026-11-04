import { get } from './api';

// the mail mode of Settings → Mail: with "off" genco sends no mail, the
// pages offer what works without it; without hosting no new mail domains
class MailStore {
	mode = $state('direct');
	hosting = $state(true);
	private loading: Promise<void> | null = null;

	get off(): boolean {
		return this.mode === 'off';
	}

	/** no new mail domains, mailboxes, forwards or send-only addresses */
	get hostingOff(): boolean {
		return this.off || !this.hosting;
	}

	/** once per page load, Settings sets it after a save */
	load() {
		this.loading ??= get('/settings/mail')
			.then(s => {
				this.mode = s.mode;
				this.hosting = s.hosting;
			})
			.catch(e => console.error('mail settings', e));
		return this.loading;
	}
}

export const mail = new MailStore();
