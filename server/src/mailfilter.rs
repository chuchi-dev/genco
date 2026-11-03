//! What mail passes through: Rspamd filters spam (milter of Postfix,
//! Redis for its data) and signs outgoing mail with a DKIM key per mail
//! domain; Dovecot files spam into Junk; postsrsd rewrites the sender of
//! forwarded mail (SRS) so it passes SPF at the next server.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::hosting::Group;
use crate::mailbox;
use crate::render::File;
use crate::util::sh;

/// Private keys `<domain>.genco.key`, only root and Rspamd read them.
const DKIM_DIR: &str = "/etc/genco/dkim";
pub const SELECTOR: &str = "genco";
const RSPAMD_USER: &str = "_rspamd";
const RSPAMD_LOCAL: &str = "/etc/rspamd/local.d";
pub const SPAM_SIEVE: &str = "/etc/genco/mail/spam.sieve";
pub const SRS_DEFAULTS: &str = "/etc/default/postsrsd";

pub fn installed() -> bool {
	Path::new("/usr/bin/rspamd").exists()
}

pub fn key_path(domain: &str) -> PathBuf {
	Path::new(DKIM_DIR).join(format!("{domain}.{SELECTOR}.key"))
}

fn local(name: &str, content: &str) -> File {
	File {
		path: Path::new(RSPAMD_LOCAL).join(name),
		content: format!(
			"# written by genco, changes here are overwritten\n{content}"
		),
		private_to: None,
	}
}

/// Rspamd's settings, the Junk rule and postsrsd's, while there are mail
/// domains and Rspamd is installed.
pub fn files(cfg: &Config, groups: &[Group]) -> Vec<File> {
	if !installed() || !mailbox::enabled(groups) {
		return vec![];
	}
	let mut files = vec![
		local(
			"worker-proxy.inc",
			"bind_socket = \"127.0.0.1:11332\";
milter = yes;
timeout = 120s;
upstream \"local\" {
	default = yes;
	self_scan = yes;
}
",
		),
		local("redis.conf", "servers = \"127.0.0.1\";\n"),
		// greylisting delays the first mail of every new sender
		local("greylist.conf", "enabled = false;\n"),
		local(
			"milter_headers.conf",
			"use = [\"x-spamd-bar\", \"x-spam-level\", \"authentication-results\"];
authenticated_headers = [\"authentication-results\"];
",
		),
		// a key file decides which domains are signed
		local(
			"dkim_signing.conf",
			&format!(
				"path = \"{DKIM_DIR}/$domain.$selector.key\";
selector = \"{SELECTOR}\";
# mail with a login is signed for its envelope sender's domain, which
# postfix allowed for that login (a mailbox its own address, a group's
# SMTP login its mail domains); mail of the sites over localhost:25
# without a login is not signed: any group could send as any domain there
allow_username_mismatch = true;
sign_authenticated = true;
sign_local = false;
# postfix checks no owner for the empty sender <>, so it signs nothing
allow_envfrom_empty = false;
use_domain = \"envelope\";
# the whole domain, mail domains are often subdomains
use_esld = false;
"
			),
		),
		File {
			path: SPAM_SIEVE.into(),
			content: "# written by genco, changes here are overwritten
require [\"fileinto\"];
if header :is \"X-Spam\" \"Yes\" {
	fileinto \"Junk\";
	stop;
}
"
			.into(),
			private_to: None,
		},
	];

	// the server's own domains keep their sender, only forwarded mail of
	// others gets the mail host's
	let mut own: Vec<String> = groups
		.iter()
		.flat_map(|g| {
			g.mail_domains
				.iter()
				.map(|d| d.name.clone())
				.chain(g.domains.iter().map(|d| d.name.clone()))
		})
		.collect();
	own.extend(cfg.domain.clone());
	let host = mailbox::host(cfg).unwrap_or_else(|| "localhost".into());
	own.push(host.clone());
	own.sort();
	own.dedup();
	files.push(File {
		path: SRS_DEFAULTS.into(),
		content: format!(
			"# written by genco, changes here are overwritten
SRS_DOMAIN={host}
SRS_EXCLUDE_DOMAINS={}
SRS_SEPARATOR==
SRS_SECRET=/etc/postsrsd.secret
SRS_HASHLENGTH=4
SRS_HASHMIN=4
SRS_FORWARD_PORT=10001
SRS_REVERSE_PORT=10002
RUN_AS=postsrsd
SRS_LISTEN_ADDR=127.0.0.1
CHROOT=/var/lib/postsrsd
",
			own.join(",")
		),
		private_to: None,
	});
	files
}

/// Postfix's settings for the filter and SRS, empty values remove them.
/// The canonical classes go back to postfix's defaults instead: postfix
/// 3.10 (Debian 13) refuses an empty one and its cleanup dies on every
/// mail.
pub fn postfix_settings(on: bool) -> Vec<(&'static str, String)> {
	let or = |s: &str, default: &str| {
		if on {
			s.to_string()
		} else {
			default.to_string()
		}
	};
	vec![
		("smtpd_milters", or("inet:127.0.0.1:11332", "")),
		("non_smtpd_milters", or("inet:127.0.0.1:11332", "")),
		("milter_default_action", "accept".into()),
		("milter_protocol", "6".into()),
		("sender_canonical_maps", or("tcp:127.0.0.1:10001", "")),
		(
			"sender_canonical_classes",
			or("envelope_sender", "envelope_sender, header_sender"),
		),
		("recipient_canonical_maps", or("tcp:127.0.0.1:10002", "")),
		(
			"recipient_canonical_classes",
			or(
				"envelope_recipient,header_recipient",
				"envelope_recipient, header_recipient",
			),
		),
	]
}

/// The Sieve part of Dovecot's config: spam goes to Junk.
pub fn dovecot_sieve() -> String {
	if !Path::new("/usr/lib/dovecot/modules/lib90_sieve_plugin.so").exists() {
		return String::new();
	}
	format!(
		"
protocol lmtp {{
	mail_plugins {{
		sieve = yes
	}}
}}

sieve_script spam {{
	type = after
	driver = file
	path = {SPAM_SIEVE}
}}
"
	)
}

/// A 2048 bit key for every mail domain that has none.
pub async fn ensure_keys(groups: &[Group]) -> Result<Vec<String>, String> {
	if !installed() {
		return Ok(vec![]);
	}
	sh(&format!(
		"mkdir -p {DKIM_DIR} && chown root:{RSPAMD_USER} {DKIM_DIR} \
		&& chmod 750 {DKIM_DIR}"
	))
	.await?;
	let mut made = vec![];
	for name in signed(groups) {
		let path = key_path(name);
		if path.exists() {
			continue;
		}
		make_key(&path, Some(RSPAMD_USER)).await?;
		made.push(name.to_string());
	}
	Ok(made)
}

/// The domains that get a key and so are signed: every mail domain,
/// send-only ones too.
fn signed(groups: &[Group]) -> Vec<&str> {
	let domains = groups.iter().flat_map(|g| &g.mail_domains);
	domains.map(|d| d.name.as_str()).collect()
}

/// A 2048 bit RSA key in PKCS#1 (lettre reads only that, Rspamd both),
/// only root reads it, and `group` if given.
pub async fn make_key(path: &Path, group: Option<&str>) -> Result<(), String> {
	let p = path.display();
	let share = match group {
		Some(g) => format!("&& chown root:{g} {p}.new && chmod 640 {p}.new"),
		None => String::new(),
	};
	sh(&format!(
		"mkdir -p {DKIM_DIR} && umask 077 \
		&& openssl genrsa -traditional -out {p}.new 2048 2>/dev/null \
		{share} && mv {p}.new {p}"
	))
	.await
	.map(|_| ())
}

/// The DKIM record's value of a domain, from its key.
pub async fn dkim_record(domain: &str) -> Option<String> {
	dkim_record_of(&key_path(domain)).await
}

/// The DKIM record's value of a key file.
pub async fn dkim_record_of(path: &Path) -> Option<String> {
	if !path.exists() {
		return None;
	}
	let key = sh(&format!(
		"openssl rsa -in {} -pubout -outform DER 2>/dev/null | base64 -w0",
		path.display()
	))
	.await
	.ok()?;
	(!key.is_empty()).then(|| format!("v=DKIM1; k=rsa; p={key}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn send_only_domains_are_signed() {
		let domain = |name: &str, send_only| crate::mailbox::MailDomain {
			id: 0,
			group_id: 1,
			name: name.into(),
			send_only,
			mailboxes: vec![],
			forwards: vec![],
			senders: vec![],
		};
		let groups = [Group {
			mail_domains: vec![
				domain("shop.ch", false),
				domain("textrem.ch", true),
			],
			..Default::default()
		}];
		assert_eq!(signed(&groups), ["shop.ch", "textrem.ch"]);
	}

	#[test]
	fn canonical_classes_are_never_empty() {
		for on in [true, false] {
			for (name, value) in postfix_settings(on) {
				if name.ends_with("_classes") {
					assert!(!value.is_empty(), "{name} empty with on={on}");
				}
			}
		}
	}

	#[test]
	fn filter_settings_go_away_without_mail() {
		let off = postfix_settings(false);
		let milters = off.iter().find(|(k, _)| *k == "smtpd_milters").unwrap();
		assert_eq!(milters.1, "");
		let on = postfix_settings(true);
		let srs = on
			.iter()
			.find(|(k, _)| *k == "sender_canonical_maps")
			.unwrap();
		assert_eq!(srs.1, "tcp:127.0.0.1:10001");
	}
}
