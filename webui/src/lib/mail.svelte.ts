import { get } from './api';

// the mail mode of Settings → Mail: with "off" genco sends no mail, the
// pages offer what works without it
class MailStore {
	mode = $state('direct');
	private loading: Promise<void> | null = null;

	get off(): boolean {
		return this.mode === 'off';
	}

	/** once per page load, Settings sets it after a save */
	load() {
		this.loading ??= get('/settings/mail')
			.then(s => {
				this.mode = s.mode;
			})
			.catch(e => console.error('mail settings', e));
		return this.loading;
	}
}

export const mail = new MailStore();
