//! Passwords the admins need: logins the agent set up (a Matomo admin, a
//! database user of an app) or that admins keep for a client. The value
//! is encrypted with a key outside the database, the panel shows it only
//! on request and the activity log records who did.

use std::path::Path;

use axum::extract::{Path as UrlPath, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::users::Authed;

/// 32 random bytes, only root reads it. Without it the values are lost.
const KEY_FILE: &str = "/etc/genco/secrets.key";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Secret {
	pub id: i64,
	pub name: String,
	pub group_id: Option<i64>,
	pub username: String,
	pub url: String,
	pub note: String,
	pub created_by: String,
	pub updated_at: i64,
}

fn key(path: &Path) -> Result<Key> {
	use std::io::Write as _;
	use std::os::unix::fs::OpenOptionsExt as _;

	// made once and only readable by root from the start; the panel and
	// the CLI may try at the same moment, the first one wins
	let new = std::fs::OpenOptions::new()
		.write(true)
		.create_new(true)
		.mode(0o600)
		.open(path);
	match new {
		Ok(mut f) => {
			let bytes: [u8; 32] = rand::random();
			f.write_all(&bytes).map_err(|e| {
				Error::Internal(format!("{}: {e}", path.display()))
			})?;
		}
		Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
		Err(e) => {
			return Err(Error::Internal(format!("{}: {e}", path.display())));
		}
	}
	let bytes = std::fs::read(path)
		.map_err(|e| Error::Internal(format!("{}: {e}", path.display())))?;
	if bytes.len() != 32 {
		return Err(Error::Internal(format!("{} is no key", path.display())));
	}
	Ok(*Key::from_slice(&bytes))
}

/// Nonce and ciphertext in one blob.
fn seal(key: &Key, plain: &str) -> Result<Vec<u8>> {
	let nonce: [u8; 12] = rand::random();
	let mut out = nonce.to_vec();
	let sealed = ChaCha20Poly1305::new(key)
		.encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
		.map_err(|_| Error::Internal("encrypting failed".into()))?;
	out.extend(sealed);
	Ok(out)
}

fn open(key: &Key, blob: &[u8]) -> Result<String> {
	if blob.len() < 12 {
		return Err(Error::Internal("broken secret".into()));
	}
	let (nonce, sealed) = blob.split_at(12);
	let plain = ChaCha20Poly1305::new(key)
		.decrypt(Nonce::from_slice(nonce), sealed)
		.map_err(|_| {
			Error::Internal(format!(
				"can't decrypt, is {KEY_FILE} the right key?"
			))
		})?;
	String::from_utf8(plain).map_err(|e| Error::Internal(e.to_string()))
}

const COLUMNS: &str =
	"id, name, group_id, username, url, note, created_by, updated_at";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Secret> {
	Ok(Secret {
		id: r.get(0)?,
		name: r.get(1)?,
		group_id: r.get(2)?,
		username: r.get(3)?,
		url: r.get(4)?,
		note: r.get(5)?,
		created_by: r.get(6)?,
		updated_at: r.get(7)?,
	})
}

fn list(conn: &Connection) -> rusqlite::Result<Vec<Secret>> {
	let mut stmt =
		conn.prepare(&format!("SELECT {COLUMNS} FROM secrets ORDER BY name"))?;
	stmt.query_map([], from_row)?.collect()
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretReq {
	pub name: String,
	pub group_id: Option<i64>,
	#[serde(default)]
	pub username: String,
	#[serde(default)]
	pub url: String,
	#[serde(default)]
	pub note: String,
	/// empty keeps the value when changing one
	#[serde(default)]
	pub value: String,
}

/// Adds a secret, or replaces the one with the same name.
fn save(
	conn: &Connection,
	key_file: &Path,
	req: &SecretReq,
	by: &str,
) -> Result<i64> {
	let name = req.name.trim();
	check(req)?;
	let existing: Option<i64> = conn
		.query_row("SELECT id FROM secrets WHERE name = ?1", [name], |r| {
			r.get(0)
		})
		.optional()?;
	if existing.is_none() && req.value.is_empty() {
		return Err(Error::Invalid("the password is empty".into()));
	}
	let value = match req.value.as_str() {
		"" => None,
		v => Some(seal(&key(key_file)?, v)?),
	};

	match existing {
		Some(id) => {
			conn.execute(
				"UPDATE secrets SET group_id = ?1, username = ?2, url = ?3,
				note = ?4, value = coalesce(?5, value), updated_at = ?6
				WHERE id = ?7",
				params![
					req.group_id,
					req.username.trim(),
					req.url.trim(),
					req.note.trim(),
					value,
					now(),
					id
				],
			)?;
		}
		None => {
			conn.execute(
				"INSERT INTO secrets (name, group_id, username, url, note,
				value, created_by, created_at, updated_at)
				VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
				params![
					name,
					req.group_id,
					req.username.trim(),
					req.url.trim(),
					req.note.trim(),
					value,
					by,
					now()
				],
			)?;
		}
	}
	Ok(conn.query_row(
		"SELECT id FROM secrets WHERE name = ?1",
		[name],
		|r| r.get(0),
	)?)
}

/// A name, and a URL only with http(s): the panel makes it a link.
fn check(req: &SecretReq) -> Result<()> {
	if req.name.trim().is_empty() {
		return Err(Error::Invalid("a password needs a name".into()));
	}
	let url = req.url.trim();
	if !url.is_empty()
		&& !url.starts_with("https://")
		&& !url.starts_with("http://")
	{
		return Err(Error::Invalid("the URL starts with https://".into()));
	}
	Ok(())
}

fn reveal(conn: &Connection, key_file: &Path, id: i64) -> Result<String> {
	let blob: Vec<u8> =
		conn.query_row("SELECT value FROM secrets WHERE id = ?1", [id], |r| {
			r.get(0)
		})?;
	open(&key(key_file)?, &blob)
}

// -------------------------------------------------------------------- cli

#[derive(Debug, clap::Parser)]
pub enum SecretCmd {
	/// List the passwords (the link path first), without their values
	List,
	/// Add or replace a password, its value is read from stdin
	Add {
		name: String,
		#[clap(long)]
		group: Option<String>,
		#[clap(long, default_value = "")]
		user: String,
		#[clap(long, default_value = "")]
		url: String,
		#[clap(long, default_value = "")]
		note: String,
	},
	/// Print a password, for the terminal when the panel is down
	Show { name: String },
	/// Delete a password
	Delete { name: String },
}

pub fn cli(
	conn: &Connection,
	cmd: SecretCmd,
	value: impl FnOnce() -> std::result::Result<String, String>,
) -> Result<String> {
	let key_file = Path::new(KEY_FILE);
	let by_name = |name: &str| -> Result<i64> {
		conn.query_row("SELECT id FROM secrets WHERE name = ?1", [name], |r| {
			r.get(0)
		})
		.optional()?
		.ok_or_else(|| Error::Invalid(format!("no password {name}")))
	};
	match cmd {
		SecretCmd::List => Ok(list(conn)?
			.iter()
			.map(|s| {
				// the path links it in the agent's answers
				format!(
					"/passwords/{}\t{}\t{}\t{}\t{}",
					s.id, s.name, s.username, s.url, s.note
				)
			})
			.collect::<Vec<_>>()
			.join("\n")),
		SecretCmd::Add {
			name,
			group,
			user,
			url,
			note,
		} => {
			let group_id = match group {
				Some(g) => Some(
					crate::hosting::group_by_name(conn, &g)?
						.ok_or_else(|| Error::Invalid(format!("no group {g}")))?
						.id,
				),
				None => None,
			};
			let req = SecretReq {
				name: name.clone(),
				group_id,
				username: user,
				url,
				note,
				value: value().map_err(Error::Invalid)?,
			};
			let id = save(conn, key_file, &req, crate::activity::cli_actor())?;
			Ok(format!(
				"saved the password {name}, admins find it under Start → \
				Passwords; link to it in your answer as [{name}](/passwords/{id})"
			))
		}
		SecretCmd::Show { name } => reveal(conn, key_file, by_name(&name)?),
		SecretCmd::Delete { name } => {
			conn.execute(
				"DELETE FROM secrets WHERE id = ?1",
				[by_name(&name)?],
			)?;
			Ok(format!("deleted the password {name}"))
		}
	}
}

// -------------------------------------------------------------------- api

async fn list_route(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<Secret>>> {
	Ok(Json(list(&db.conn())?))
}

async fn create(
	authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Json(req): Json<SecretReq>,
) -> Result<Json<i64>> {
	let id = save(&db.conn(), Path::new(KEY_FILE), &req, &authed.user.name)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

async fn update(
	authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<SecretReq>,
) -> Result<Json<()>> {
	check(&req)?;
	let conn = db.conn();
	let name: String =
		conn.query_row("SELECT name FROM secrets WHERE id = ?1", [id], |r| {
			r.get(0)
		})?;
	// renaming is a new name on the same row
	if name != req.name.trim() {
		conn.execute(
			"UPDATE secrets SET name = ?1 WHERE id = ?2",
			params![req.name.trim(), id],
		)?;
	}
	save(&conn, Path::new(KEY_FILE), &req, &authed.user.name)?;
	Ok(Json(()))
}

async fn delete_route(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<()>> {
	let n = db
		.conn()
		.execute("DELETE FROM secrets WHERE id = ?1", [id])?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	Ok(Json(()))
}

/// The value, logged in the activity log by its middleware.
async fn value(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<String>> {
	Ok(Json(reveal(&db.conn(), Path::new(KEY_FILE), id)?))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/secrets", get(list_route).post(create))
		.route("/secrets/{id}", post(update).delete(delete_route))
		.route("/secrets/{id}/value", get(value))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn values_are_sealed_and_open_again() {
		let dir = std::env::temp_dir()
			.join(format!("genco-key-{}", rand::random::<u32>()));
		let db = Db::memory();
		let conn = db.conn();
		let req = SecretReq {
			name: "Matomo".into(),
			value: "s3cret!".into(),
			..Default::default()
		};
		let id = save(&conn, &dir, &req, "test").unwrap();

		let blob: Vec<u8> = conn
			.query_row("SELECT value FROM secrets WHERE id = ?1", [id], |r| {
				r.get(0)
			})
			.unwrap();
		assert!(!String::from_utf8_lossy(&blob).contains("s3cret"));
		assert_eq!(reveal(&conn, &dir, id).unwrap(), "s3cret!");

		// a change without a value keeps it
		let req = SecretReq {
			name: "Matomo".into(),
			username: "admin".into(),
			..Default::default()
		};
		assert_eq!(save(&conn, &dir, &req, "test").unwrap(), id);
		assert_eq!(reveal(&conn, &dir, id).unwrap(), "s3cret!");
		std::fs::remove_file(&dir).unwrap();
	}
}
