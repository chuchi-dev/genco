//! Mail hosting (genco's own mail is `mail`): mail domains of a group
//! with mailboxes (IMAP, stored by Dovecot under /var/vmail) and forwards. Postfix receives on 25 and
//! takes mail from mailboxes on 587/465; everything runs under one host
//! name, `mail.<panel domain>`. genco writes the maps and Dovecot's whole
//! config, the DNS check tells what the domain's records should be.
//! A send-only mail domain has its inbox elsewhere: genco receives
//! nothing for it, apps only send from it, signed, with send-only
//! addresses (`send_only`).

mod cli;
mod conf;
mod dns;
mod routes;
mod send_only;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::hosting::{self, Group};
use crate::render::File;
use crate::users::Authed;
use crate::util;

pub use cli::SendOnlyCmd;
pub use cli::{MailCmd, cli, dns_text, records_text};
pub use conf::{
	POSTFIX_AFTER_DOCKER, SUBMISSION, SUBMISSION_OPTIONS, docker_bridges,
	docker_installed, files, postfix_settings,
};
use dns::*;
pub use dns::{DnsRecord, panel_dns};
pub use routes::routes;
pub use send_only::SendOnly;

/// Where Dovecot keeps the mail, `<domain>/<local part>/Maildir`.
pub const VMAIL: &str = "/var/vmail";
const MAPS: &str = "/etc/genco/mail";
pub const DOVECOT_CONF: &str = "/etc/dovecot/dovecot.conf";
const DEFAULT_QUOTA_MB: i64 = 2048;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailDomain {
	pub id: i64,
	pub group_id: i64,
	pub name: String,
	/// the inbox is elsewhere: no mailboxes or forwards, genco only sends
	pub send_only: bool,
	pub mailboxes: Vec<Mailbox>,
	pub forwards: Vec<Forward>,
	pub send_only_addresses: Vec<SendOnly>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
	pub id: i64,
	pub mail_domain_id: i64,
	pub address: String,
	pub quota_mb: i64,
	/// for Dovecot's passwd file, never sent to the browser
	#[serde(skip)]
	pub password_hash: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
	pub id: i64,
	pub mail_domain_id: i64,
	/// `*@domain` is the catch-all
	pub address: String,
	pub targets: Vec<String>,
}

/// Every mail domain with its mailboxes, forwards and send-only
/// addresses.
pub fn load(conn: &Connection) -> rusqlite::Result<Vec<MailDomain>> {
	let mut stmt = conn.prepare(
		"SELECT b.id, b.mail_domain_id, b.local_part || '@' || d.name,
		b.quota_mb, b.password_hash FROM mailboxes b
		JOIN mail_domains d ON d.id = b.mail_domain_id ORDER BY b.local_part",
	)?;
	let boxes: Vec<Mailbox> = stmt
		.query_map([], |r| {
			Ok(Mailbox {
				id: r.get(0)?,
				mail_domain_id: r.get(1)?,
				address: r.get(2)?,
				quota_mb: r.get(3)?,
				password_hash: r.get(4)?,
			})
		})?
		.collect::<rusqlite::Result<_>>()?;

	let mut stmt = conn.prepare(
		"SELECT f.id, f.mail_domain_id, f.local_part || '@' || d.name,
		f.targets FROM mail_forwards f
		JOIN mail_domains d ON d.id = f.mail_domain_id ORDER BY f.local_part",
	)?;
	let forwards: Vec<Forward> = stmt
		.query_map([], |r| {
			let targets: String = r.get(3)?;
			Ok(Forward {
				id: r.get(0)?,
				mail_domain_id: r.get(1)?,
				address: r.get(2)?,
				targets: targets.split_whitespace().map(String::from).collect(),
			})
		})?
		.collect::<rusqlite::Result<_>>()?;

	let send_only = send_only::load(conn)?;

	let mut stmt = conn.prepare(
		"SELECT id, group_id, name, send_only FROM mail_domains ORDER BY name",
	)?;
	stmt.query_map([], |r| {
		let id: i64 = r.get(0)?;
		Ok(MailDomain {
			id,
			group_id: r.get(1)?,
			name: r.get(2)?,
			send_only: r.get(3)?,
			mailboxes: util::matching(&boxes, |b| b.mail_domain_id == id),
			forwards: util::matching(&forwards, |f| f.mail_domain_id == id),
			send_only_addresses: util::matching(&send_only, |s| {
				s.mail_domain_id == id
			}),
		})
	})?
	.collect()
}

fn domains(groups: &[Group]) -> impl Iterator<Item = &MailDomain> {
	groups.iter().flat_map(|g| &g.mail_domains)
}

/// Mail runs once a group has a mail domain.
pub fn enabled(groups: &[Group]) -> bool {
	domains(groups).next().is_some()
}

/// The host name of IMAP, SMTP and the webmail, `mail.<panel domain>`.
pub fn host(cfg: &Config) -> Option<String> {
	cfg.mail_host
		.clone()
		.or_else(|| cfg.domain.as_ref().map(|d| format!("mail.{d}")))
}

/// Letters, digits and `.`, `_`, `+`, `-`, not at the ends.
fn check_local_part(local: &str) -> Result<()> {
	let ok = !local.is_empty()
		&& local.len() <= 64
		&& !local.starts_with(['.', '-', '+'])
		&& !local.ends_with('.')
		&& !local.contains("..")
		&& local.chars().all(|c| {
			c.is_ascii_lowercase()
				|| c.is_ascii_digit()
				|| matches!(c, '.' | '_' | '+' | '-')
		});
	if !ok {
		return Err(Error::Invalid(format!(
			"{local} is no valid address before the @ (a-z, 0-9, . _ + -)"
		)));
	}
	Ok(())
}

fn check_address(address: &str) -> Result<()> {
	let Some((local, domain)) = address.rsplit_once('@') else {
		return Err(Error::Invalid(format!("{address} is no mail address")));
	};
	check_local_part(local)?;
	hosting::check_domain_name(domain)
}

/// Dovecot reads bcrypt as BLF-CRYPT.
async fn hash(password: String) -> Result<String> {
	if password.chars().count() < 10 {
		return Err(Error::Invalid(
			"the password needs at least 10 characters".into(),
		));
	}
	let hash = tokio::task::spawn_blocking(move || bcrypt::hash(password, 12))
		.await
		.map_err(|e| Error::Internal(e.to_string()))?
		.map_err(|e| Error::Internal(e.to_string()))?;
	Ok(format!("{{BLF-CRYPT}}{hash}"))
}

/// The SMTP login of a group's sites: `g-<group>`, it may send as any
/// address of the group's mail domains, and that mail is DKIM signed.
/// Plain mail() over localhost:25 stays unsigned, any group could claim
/// any sender there.
fn smtp_user(group: &str) -> String {
	crate::render::group_user(group)
}

/// Where the group's processes read their SMTP password.
fn smtp_password_path(group: &str) -> PathBuf {
	PathBuf::from("/etc/genco/smtp").join(smtp_user(group))
}

/// Makes the SMTP login of every group with mail domains that has none.
pub async fn ensure_smtp_logins(db: &Db) -> Result<()> {
	let missing: Vec<(i64, String)> = {
		let conn = db.conn();
		let mut stmt = conn.prepare(
			"SELECT g.id, g.name FROM domain_groups g WHERE g.smtp_hash IS NULL
			AND EXISTS (SELECT 1 FROM mail_domains m WHERE m.group_id = g.id)",
		)?;
		stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
			.collect::<rusqlite::Result<_>>()?
	};
	for (id, _name) in missing {
		let password = crate::util::random_hex(16);
		let hash = hash(password.clone()).await?;
		db.conn().execute(
			"UPDATE domain_groups SET smtp_password = ?1, smtp_hash = ?2
			WHERE id = ?3",
			params![password, hash, id],
		)?;
	}
	Ok(())
}

/// Not with mail hosting off. `send_only`: the inbox stays elsewhere,
/// apps here only send from it.
fn add_domain(
	conn: &Connection,
	group_id: i64,
	name: &str,
	send_only: bool,
) -> Result<i64> {
	check_hosting(conn, "no new mail domains")?;
	let name = name.trim().to_lowercase();
	hosting::check_domain_name(&name)?;
	conn.execute(
		"INSERT INTO mail_domains (group_id, name, send_only, created_at)
		VALUES (?1, ?2, ?3, ?4)",
		params![group_id, name, send_only, now()],
	)?;
	Ok(conn.last_insert_rowid())
}

/// Receiving or send-only, not with mail hosting off. A domain becomes
/// send-only only without mailboxes and forwards: their mail would no
/// longer arrive.
fn set_mode(conn: &Connection, domain_id: i64, send_only: bool) -> Result<()> {
	check_hosting(conn, "mail domains keep their mode")?;
	let (name, boxes, forwards): (String, i64, i64) = conn.query_row(
		"SELECT name,
		(SELECT count(*) FROM mailboxes WHERE mail_domain_id = d.id),
		(SELECT count(*) FROM mail_forwards WHERE mail_domain_id = d.id)
		FROM mail_domains d WHERE id = ?1",
		[domain_id],
		|r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
	)?;
	if send_only && boxes + forwards > 0 {
		return Err(Error::Invalid(format!(
			"{name} has {boxes} mailboxes and {forwards} forwards, delete \
			them first: a send-only domain receives no mail here"
		)));
	}
	conn.execute(
		"UPDATE mail_domains SET send_only = ?1 WHERE id = ?2",
		params![send_only, domain_id],
	)?;
	Ok(())
}

/// Nothing new to host with mail hosting off (or mail off: nothing
/// could send), what exists stays.
fn check_hosting(conn: &Connection, what: &str) -> Result<()> {
	if crate::mail::hosting_off(conn) {
		return Err(crate::mail::hosting_off_error(what));
	}
	Ok(())
}

/// Mailboxes and forwards only on receiving domains.
fn check_receiving(conn: &Connection, domain_id: i64) -> Result<()> {
	let (name, send_only): (String, bool) = conn.query_row(
		"SELECT name, send_only FROM mail_domains WHERE id = ?1",
		[domain_id],
		|r| Ok((r.get(0)?, r.get(1)?)),
	)?;
	if send_only {
		return Err(Error::Invalid(format!(
			"{name} is send-only, its inbox is elsewhere: no mailboxes or \
			forwards here"
		)));
	}
	Ok(())
}

pub async fn add_mailbox(
	db: &Db,
	domain_id: i64,
	local: &str,
	password: String,
	quota_mb: Option<i64>,
) -> Result<i64> {
	let local = local.trim().to_lowercase();
	check_local_part(&local)?;
	let hash = hash(password).await?;
	let conn = db.conn();
	check_hosting(&conn, "no new mailboxes")?;
	check_receiving(&conn, domain_id)?;
	if let Some(kind) = address_in_use(&conn, domain_id, &local)? {
		return Err(Error::Invalid(format!("{local}@ is a {kind} already")));
	}
	conn.execute(
		"INSERT INTO mailboxes (mail_domain_id, local_part, password_hash,
		quota_mb, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
		params![
			domain_id,
			local,
			hash,
			quota_mb.unwrap_or(DEFAULT_QUOTA_MB).max(10),
			now()
		],
	)?;
	Ok(conn.last_insert_rowid())
}

/// A new password and/or quota.
pub async fn set_mailbox(
	db: &Db,
	id: i64,
	password: Option<String>,
	quota_mb: Option<i64>,
) -> Result<()> {
	let hash = match password {
		Some(p) => Some(hash(p).await?),
		None => None,
	};
	let n = db.conn().execute(
		"UPDATE mailboxes SET password_hash = coalesce(?1, password_hash),
		quota_mb = coalesce(?2, quota_mb) WHERE id = ?3",
		params![hash, quota_mb.map(|q| q.max(10)), id],
	)?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	Ok(())
}

/// What an address of a domain is already: "mailbox", "forward" or
/// "send-only address". One address is only one of them.
fn address_in_use(
	conn: &Connection,
	domain_id: i64,
	local: &str,
) -> Result<Option<&'static str>> {
	let (mailbox, forward, send_only): (bool, bool, bool) = conn.query_row(
		"SELECT
		EXISTS (SELECT 1 FROM mailboxes
			WHERE mail_domain_id = ?1 AND local_part = ?2),
		EXISTS (SELECT 1 FROM mail_forwards
			WHERE mail_domain_id = ?1 AND local_part = ?2),
		EXISTS (SELECT 1 FROM mail_senders
			WHERE mail_domain_id = ?1 AND local_part = ?2)",
		params![domain_id, local],
		|r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
	)?;
	Ok(match (mailbox, forward, send_only) {
		(true, _, _) => Some("mailbox"),
		(_, true, _) => Some("forward"),
		(_, _, true) => Some("send-only address"),
		_ => None,
	})
}

/// A forward to one or more addresses; `*` is the catch-all of the domain.
fn set_forward(
	conn: &Connection,
	domain_id: i64,
	local: &str,
	targets: &[String],
) -> Result<i64> {
	let local = local.trim().to_lowercase();
	check_receiving(conn, domain_id)?;
	if local != "*" {
		check_local_part(&local)?;
	}
	match address_in_use(conn, domain_id, &local)? {
		Some("mailbox") => {
			return Err(Error::Invalid(format!(
				"{local}@ is a mailbox, give the mailbox a forward instead"
			)));
		}
		Some("send-only address") => {
			return Err(Error::Invalid(format!(
				"{local}@ is a send-only address already"
			)));
		}
		// a forward gets its new targets, also with hosting off
		Some(_) => {}
		None => check_hosting(conn, "no new forwards")?,
	}
	let targets: Vec<String> = targets
		.iter()
		.map(|t| t.trim().to_lowercase())
		.filter(|t| !t.is_empty())
		.collect();
	if targets.is_empty() {
		return Err(Error::Invalid(
			"a forward needs an address to go to".into(),
		));
	}
	for t in &targets {
		check_address(t)?;
	}

	conn.execute(
		"INSERT INTO mail_forwards (mail_domain_id, local_part, targets,
		created_at) VALUES (?1, ?2, ?3, ?4)
		ON CONFLICT (mail_domain_id, local_part)
		DO UPDATE SET targets = excluded.targets",
		params![domain_id, local, targets.join(" "), now()],
	)?;
	Ok(conn.query_row(
		"SELECT id FROM mail_forwards WHERE mail_domain_id = ?1
		AND local_part = ?2",
		params![domain_id, local],
		|r| r.get(0),
	)?)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn local_parts() {
		assert!(check_local_part("info").is_ok());
		assert!(check_local_part("first.last+news").is_ok());
		assert!(check_local_part("").is_err());
		assert!(check_local_part(".x").is_err());
		assert!(check_local_part("a..b").is_err());
		assert!(check_local_part("a b").is_err());
		assert!(check_local_part("Ä").is_err());
	}

	#[test]
	fn no_mail_domain_with_mail_off() {
		let db = crate::db::Db::memory();
		let conn = db.conn();
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
				VALUES ('shop', 1, 0);",
		)
		.unwrap();
		crate::db::set_setting(&conn, "mail", r#"{"mode":"off"}"#).unwrap();
		let refused = add_domain(&conn, 1, "shop.ch", false).unwrap_err();
		assert!(refused.to_string().contains("mail hosting is off"));
		let refused = add_domain(&conn, 1, "shop.ch", true).unwrap_err();
		assert!(refused.to_string().contains("mail hosting is off"));

		crate::db::set_setting(&conn, "mail", r#"{"mode":"direct"}"#).unwrap();
		add_domain(&conn, 1, "shop.ch", false).unwrap();
	}

	pub(super) fn group_with_domains(conn: &Connection) {
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
				VALUES ('shop', 1, 0);",
		)
		.unwrap();
		add_domain(conn, 1, "shop.ch", false).unwrap();
		add_domain(conn, 1, "textrem.ch", true).unwrap();
	}

	#[tokio::test]
	async fn send_only_domains_take_no_mailboxes_or_forwards() {
		let db = crate::db::Db::memory();
		group_with_domains(&db.conn());
		let to = vec!["a@b.ch".to_string()];
		let refused = set_forward(&db.conn(), 2, "info", &to).unwrap_err();
		assert!(refused.to_string().contains("send-only"), "{refused}");
		let refused =
			add_mailbox(&db, 2, "info", "long enough pw".into(), None)
				.await
				.unwrap_err();
		assert!(refused.to_string().contains("send-only"), "{refused}");
	}

	#[test]
	fn only_an_empty_domain_becomes_send_only() {
		let db = crate::db::Db::memory();
		group_with_domains(&db.conn());
		let conn = db.conn();
		set_forward(&conn, 1, "info", &["a@b.ch".into()]).unwrap();
		let refused = set_mode(&conn, 1, true).unwrap_err();
		assert!(refused.to_string().contains("1 forwards"), "{refused}");

		conn.execute("DELETE FROM mail_forwards", []).unwrap();
		set_mode(&conn, 1, true).unwrap();
		// and back, always
		set_mode(&conn, 1, false).unwrap();
		set_mode(&conn, 2, false).unwrap();
		let d = load(&conn).unwrap();
		assert!(d.iter().all(|d| !d.send_only));
	}

	#[test]
	fn no_mode_change_with_mail_off() {
		let db = crate::db::Db::memory();
		group_with_domains(&db.conn());
		let conn = db.conn();
		crate::db::set_setting(&conn, "mail", r#"{"mode":"off"}"#).unwrap();
		let refused = set_mode(&conn, 1, true).unwrap_err();
		assert!(
			refused.to_string().contains("mail hosting is off"),
			"{refused}"
		);
	}

	#[tokio::test]
	async fn nothing_new_with_hosting_off_under_any_mode() {
		let db = crate::db::Db::memory();
		group_with_domains(&db.conn());
		set_forward(&db.conn(), 1, "info", &["a@b.ch".into()]).unwrap();

		for mode in ["direct", "relay", "off"] {
			let conn = db.conn();
			let mail = format!(r#"{{"mode":"{mode}","hosting":false}}"#);
			crate::db::set_setting(&conn, "mail", &mail).unwrap();
			let off = |r: Result<i64>| {
				let e = r.unwrap_err().to_string();
				assert!(e.contains("mail hosting is off"), "{mode}: {e}");
			};
			off(add_domain(&conn, 1, "new.ch", false));
			off(add_domain(&conn, 1, "new.ch", true));
			off(set_forward(&conn, 1, "sales", &["a@b.ch".into()]));
			drop(conn);
			off(add_mailbox(&db, 1, "anna", "long enough pw".into(), None)
				.await);
			assert!(set_mode(&db.conn(), 2, false).is_err());

			// what exists stays, a forward gets new targets
			let conn = db.conn();
			assert_eq!(load(&conn).unwrap().len(), 2);
			set_forward(&conn, 1, "info", &["c@d.ch".into()]).unwrap();
		}

		let conn = db.conn();
		crate::db::set_setting(&conn, "mail", r#"{"mode":"relay"}"#).unwrap();
		add_domain(&conn, 1, "new.ch", false).unwrap();
	}

	#[tokio::test]
	async fn an_address_is_only_one_thing() {
		let db = crate::db::Db::memory();
		group_with_domains(&db.conn());
		let in_use = |local| address_in_use(&db.conn(), 1, local).unwrap();
		assert_eq!(in_use("info"), None);

		set_forward(&db.conn(), 1, "info", &["a@b.ch".into()]).unwrap();
		add_mailbox(&db, 1, "anna", "long enough pw".into(), None)
			.await
			.unwrap();
		db.conn()
			.execute(
				"INSERT INTO mail_senders (mail_domain_id, local_part,
				password, password_hash, note, created_at)
				VALUES (1, 'noreply', x'', '', '', 0)",
				[],
			)
			.unwrap();
		assert_eq!(in_use("info"), Some("forward"));
		assert_eq!(in_use("anna"), Some("mailbox"));
		assert_eq!(in_use("noreply"), Some("send-only address"));
		// another domain's
		assert_eq!(address_in_use(&db.conn(), 2, "anna").unwrap(), None);

		// a forward gets new targets, a mailbox never a second time
		set_forward(&db.conn(), 1, "info", &["c@d.ch".into()]).unwrap();
		let refused =
			set_forward(&db.conn(), 1, "anna", &["c@d.ch".into()]).unwrap_err();
		assert!(refused.to_string().contains("is a mailbox"), "{refused}");
		let refused =
			add_mailbox(&db, 1, "anna", "long enough pw".into(), None)
				.await
				.unwrap_err();
		assert!(
			refused.to_string().contains("a mailbox already"),
			"{refused}"
		);
	}
}
