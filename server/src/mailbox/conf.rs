//! The files mail runs on: Postfix's maps and settings, Dovecot's whole
//! config, the groups' SMTP logins.

use super::*;

pub(super) fn map_file(name: &str, lines: Vec<String>) -> File {
	let mut content =
		"# written by genco, changes here are overwritten\n".to_string();
	for l in lines {
		content.push_str(&l);
		content.push('\n');
	}
	File {
		path: PathBuf::from(MAPS).join(name),
		content,
		private_to: None,
	}
}

/// Postfix's maps, Dovecot's users and config, the mail host's nginx site.
/// `cert`: the mail host has a certificate.
/// `smtpd_sender_login_maps`: who may send as an address. A mailbox
/// itself, and the SMTP login of its group for every address of the
/// group's mail domains.
pub(super) fn senders(groups: &[Group]) -> Vec<String> {
	let mut lines = vec![];
	for g in groups {
		let login = g.smtp.as_ref().map(|_| smtp_user(&g.name));
		for d in &g.mail_domains {
			for b in &d.mailboxes {
				match &login {
					Some(l) => lines.push(format!("{0} {0},{l}", b.address)),
					None => lines.push(format!("{0} {0}", b.address)),
				}
			}
			if let Some(l) = &login {
				lines.push(format!("@{} {l}", d.name));
			}
		}
	}
	lines
}

pub fn files(cfg: &Config, groups: &[Group], cert: bool) -> Vec<File> {
	let domains: Vec<&MailDomain> = domains(groups).collect();
	let boxes = || domains.iter().flat_map(|d| &d.mailboxes);

	let mut forwards: BTreeMap<String, Vec<String>> = BTreeMap::new();
	// a mailbox maps to itself: Postfix looks up forwards first, a
	// catch-all (@domain) would take its mail otherwise
	for b in boxes() {
		forwards.insert(b.address.clone(), vec![b.address.clone()]);
	}
	for d in &domains {
		for f in &d.forwards {
			// the catch-all of postfix is written @domain
			let from = f.address.strip_prefix('*').unwrap_or(&f.address);
			forwards.insert(from.to_string(), f.targets.clone());
		}
	}

	let mut files = vec![
		map_file(
			"domains",
			domains.iter().map(|d| format!("{} OK", d.name)).collect(),
		),
		map_file(
			"mailboxes",
			boxes().map(|b| format!("{0} {0}", b.address)).collect(),
		),
		// a mailbox sends as itself only, a group's SMTP login as any
		// address of the group's mail domains
		map_file("senders", senders(groups)),
		map_file(
			"forwards",
			forwards
				.iter()
				.map(|(from, to)| format!("{from} {}", to.join(",")))
				.collect(),
		),
	];

	let mut passwd =
		"# written by genco, changes here are overwritten\n".to_string();
	for b in boxes() {
		passwd.push_str(&format!(
			"{}:{}::::::userdb_quota_storage_size={}M\n",
			b.address, b.password_hash, b.quota_mb
		));
	}
	files.push(File {
		path: PathBuf::from(MAPS).join("passwd"),
		content: passwd,
		private_to: Some("dovecot".into()),
	});

	// the groups' SMTP logins: a passdb without a userdb, they can send
	// but never open a mailbox
	let mut smtp =
		"# written by genco, changes here are overwritten\n".to_string();
	for g in groups.iter().filter(|g| !g.mail_domains.is_empty()) {
		let Some((password, hash)) = &g.smtp else {
			continue;
		};
		smtp.push_str(&format!("{}:{hash}\n", smtp_user(&g.name)));
		files.push(File {
			path: smtp_password_path(&g.name),
			content: format!("{password}\n"),
			private_to: Some(crate::render::group_user(&g.name)),
		});
	}
	files.push(File {
		path: PathBuf::from(MAPS).join("smtp-passwd"),
		content: smtp,
		private_to: Some("dovecot".into()),
	});

	let host = host(cfg);
	let imap = cert && !domains.is_empty();
	files.push(File {
		path: DOVECOT_CONF.into(),
		content: dovecot_conf(host.as_deref().filter(|_| imap)),
		private_to: None,
	});
	if let Some(host) = host.as_deref().filter(|_| !domains.is_empty()) {
		files.extend(crate::webmail::files(host, cert));
	}
	files.extend(crate::mailfilter::files(cfg, groups));
	files
}

/// Dovecot's whole config: only genco's mailboxes log in, never the
/// server's users. Without a certificate it only delivers (LMTP).
pub(super) fn dovecot_conf(host: Option<&str>) -> String {
	let ssl = match host {
		Some(h) => format!(
			"protocols = imap lmtp
ssl = required
ssl_server_cert_file = /etc/letsencrypt/live/{h}/fullchain.pem
ssl_server_key_file = /etc/letsencrypt/live/{h}/privkey.pem
ssl_min_protocol = TLSv1.2

service imap-login {{
	# IMAP only with TLS from the start, port 993
	inet_listener imap {{
		port = 0
	}}
}}
"
		),
		None => "protocols = lmtp\nssl = no\n".into(),
	};
	format!(
		"# written by genco, changes here are overwritten
dovecot_config_version = 2.4.0
dovecot_storage_version = 2.4.0
!include_try /usr/share/dovecot/protocols.d/*.protocol

{ssl}
mail_driver = maildir
mail_home = {VMAIL}/%{{user | domain}}/%{{user | username}}
mail_path = ~/Maildir
mail_inbox_path =

auth_mechanisms = plain login
auth_username_format = %{{user | lower}}

passdb passwd-file {{
	passwd_file_path = {MAPS}/passwd
}}

# the groups' SMTP logins, no userdb: no IMAP for them
passdb smtp {{
	driver = passwd-file
	passwd_file_path = {MAPS}/smtp-passwd
}}

userdb passwd-file {{
	passwd_file_path = {MAPS}/passwd
	fields {{
		uid = vmail
		gid = vmail
		home = {VMAIL}/%{{user | domain}}/%{{user | username}}
	}}
}}

mail_plugins {{
	quota = yes
}}

quota \"User quota\" {{
}}

protocol imap {{
	mail_plugins {{
		imap_quota = yes
	}}
}}

namespace inbox {{
	inbox = yes
	mailbox Drafts {{
		special_use = \\Drafts
		auto = subscribe
	}}
	mailbox Junk {{
		special_use = \\Junk
		auto = subscribe
	}}
	mailbox Trash {{
		special_use = \\Trash
		auto = subscribe
	}}
	mailbox Sent {{
		special_use = \\Sent
		auto = subscribe
	}}
}}

service lmtp {{
	unix_listener /var/spool/postfix/private/dovecot-lmtp {{
		mode = 0600
		user = postfix
		group = postfix
	}}
}}

service auth {{
	unix_listener /var/spool/postfix/private/auth {{
		mode = 0660
		user = postfix
		group = postfix
	}}
}}
{sieve}",
		sieve = crate::mailfilter::dovecot_sieve()
	)
}

/// Postfix's main.cf values for mail, and whether a change needs a
/// restart instead of a reload.
pub fn postfix_settings(
	cfg: &Config,
	groups: &[Group],
	cert: bool,
) -> Vec<(&'static str, String)> {
	let on = enabled(groups);
	let host = host(cfg).unwrap_or_else(|| "localhost".into());
	let panel = cfg.domain.clone().unwrap_or_else(|| "localhost".into());
	let mut s = vec![
		(
			"inet_interfaces",
			if on { "all" } else { "loopback-only" }.to_string(),
		),
		("myhostname", if on { host.clone() } else { panel }),
		(
			"virtual_mailbox_domains",
			format!("texthash:{MAPS}/domains"),
		),
		("virtual_mailbox_maps", format!("texthash:{MAPS}/mailboxes")),
		("virtual_alias_maps", format!("texthash:{MAPS}/forwards")),
		(
			"virtual_transport",
			"lmtp:unix:private/dovecot-lmtp".to_string(),
		),
		(
			"smtpd_sender_login_maps",
			format!("texthash:{MAPS}/senders"),
		),
		("disable_vrfy_command", "yes".into()),
		("smtpd_helo_required", "yes".into()),
		("message_size_limit", "52428800".into()),
	];
	s.extend(crate::mailfilter::postfix_settings(
		on && crate::mailfilter::installed(),
	));
	if cert && on {
		s.push((
			"smtpd_tls_cert_file",
			format!("/etc/letsencrypt/live/{host}/fullchain.pem"),
		));
		s.push((
			"smtpd_tls_key_file",
			format!("/etc/letsencrypt/live/{host}/privkey.pem"),
		));
		s.push(("smtpd_tls_security_level", "may".into()));
	}
	s
}

/// Sending from a mail program: 587 with STARTTLS and 465 with TLS, only
/// after a login, only as the mailbox's own address.
pub const SUBMISSION: &[(&str, &[(&str, &str)])] = &[
	("submission", &[]),
	("submissions", &[("smtpd_tls_wrappermode", "yes")]),
];

pub const SUBMISSION_OPTIONS: &[(&str, &str)] = &[
	("smtpd_tls_security_level", "encrypt"),
	("smtpd_sasl_auth_enable", "yes"),
	("smtpd_sasl_type", "dovecot"),
	("smtpd_sasl_path", "private/auth"),
	(
		"smtpd_client_restrictions",
		"permit_sasl_authenticated,reject",
	),
	("smtpd_sender_restrictions", "reject_sender_login_mismatch"),
	(
		"smtpd_recipient_restrictions",
		"permit_sasl_authenticated,reject",
	),
];

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn dovecot_without_certificate_only_delivers() {
		let c = dovecot_conf(None);
		assert!(c.contains("protocols = lmtp\n"));
		assert!(!c.contains("imap lmtp"));
		let c = dovecot_conf(Some("mail.x.ch"));
		assert!(c.contains("protocols = imap lmtp"));
		assert!(c.contains("/etc/letsencrypt/live/mail.x.ch/fullchain.pem"));
		// only genco's passwd file, never the system's users
		assert!(!c.contains("pam"));
	}
}
