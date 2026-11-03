//! Mail hosting (genco's own mail is `mail`): mail domains of a group
//! with mailboxes (IMAP, stored by Dovecot under /var/vmail) and forwards. Postfix receives on 25 and
//! takes mail from mailboxes on 587/465; everything runs under one host
//! name, `mail.<panel domain>`. genco writes the maps and Dovecot's whole
//! config, the DNS check tells what the domain's records should be.

mod cli;
mod conf;
mod dns;
mod routes;

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

pub use cli::{MailCmd, cli};
pub use conf::{
	SUBMISSION, SUBMISSION_OPTIONS, docker_bridges, files, postfix_settings,
};
use dns::*;
pub use dns::{DnsRecord, panel_dns};
pub use routes::routes;

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
	pub mailboxes: Vec<Mailbox>,
	pub forwards: Vec<Forward>,
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

/// Every mail domain with its mailboxes and forwards.
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

	let mut stmt = conn
		.prepare("SELECT id, group_id, name FROM mail_domains ORDER BY name")?;
	stmt.query_map([], |r| {
		let id: i64 = r.get(0)?;
		Ok(MailDomain {
			id,
			group_id: r.get(1)?,
			name: r.get(2)?,
			mailboxes: util::matching(&boxes, |b| b.mail_domain_id == id),
			forwards: util::matching(&forwards, |f| f.mail_domain_id == id),
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

fn add_domain(conn: &Connection, group_id: i64, name: &str) -> Result<i64> {
	let name = name.trim().to_lowercase();
	hosting::check_domain_name(&name)?;
	conn.execute(
		"INSERT INTO mail_domains (group_id, name, created_at)
		VALUES (?1, ?2, ?3)",
		params![group_id, name, now()],
	)?;
	Ok(conn.last_insert_rowid())
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
	taken_by_forward(&conn, domain_id, &local)?;
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

fn taken_by_forward(
	conn: &Connection,
	domain_id: i64,
	local: &str,
) -> Result<()> {
	let taken: bool = conn.query_row(
		"SELECT EXISTS (SELECT 1 FROM mail_forwards
		WHERE mail_domain_id = ?1 AND local_part = ?2)",
		params![domain_id, local],
		|r| r.get(0),
	)?;
	if taken {
		return Err(Error::Invalid(format!("{local}@ is a forward already")));
	}
	Ok(())
}

/// A forward to one or more addresses; `*` is the catch-all of the domain.
fn set_forward(
	conn: &Connection,
	domain_id: i64,
	local: &str,
	targets: &[String],
) -> Result<i64> {
	let local = local.trim().to_lowercase();
	if local != "*" {
		check_local_part(&local)?;
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
	let mailbox: bool = conn.query_row(
		"SELECT EXISTS (SELECT 1 FROM mailboxes
		WHERE mail_domain_id = ?1 AND local_part = ?2)",
		params![domain_id, local],
		|r| r.get(0),
	)?;
	if mailbox {
		return Err(Error::Invalid(format!(
			"{local}@ is a mailbox, give the mailbox a forward instead"
		)));
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
}
