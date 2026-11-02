//! API tokens for CI jobs and scripts. A token belongs to the admin who
//! created it and has scopes: `deploy:<group id>` deploys a group,
//! `databases` downloads every database of the server. Only its hash is
//! stored. The keys of AI agents (peers.rs) are accepted here too, with
//! the scopes `read`, `databases` and `propose`.

use axum::extract::{FromRef, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::routing::{delete, get};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::users::Authed;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Token {
	pub id: i64,
	pub name: String,
	pub scopes: Vec<String>,
	/// the admin it belongs to
	pub user_name: Option<String>,
	pub created_at: i64,
	pub last_used: Option<i64>,
}

pub fn hash(token: &str) -> String {
	Sha256::digest(token.as_bytes())
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect()
}

pub fn deploy_scope(group_id: i64) -> String {
	format!("deploy:{group_id}")
}

/// Downloads every database of the server.
pub const DATABASES: &str = "databases";
/// Reads the status and usage stats, changes nothing (an AI agent).
pub const READ: &str = "read";
/// Proposes a task to genco's agent (an AI agent).
pub const PROPOSE: &str = "propose";

/// Scopes an admin's token may have, others are refused.
fn check_scope(scope: &str) -> Result<()> {
	match scope.split_once(':') {
		Some(("deploy", id)) if id.parse::<i64>().is_ok() => Ok(()),
		None if scope == DATABASES => Ok(()),
		_ => Err(Error::Invalid(format!("unknown scope {scope}"))),
	}
}

fn list(conn: &Connection) -> rusqlite::Result<Vec<Token>> {
	let mut stmt = conn.prepare(
		"SELECT t.id, t.name, t.scopes, u.name, t.created_at, t.last_used
		FROM api_tokens t LEFT JOIN users u ON u.id = t.user_id
		ORDER BY t.id",
	)?;
	stmt.query_map([], |row| {
		Ok(Token {
			id: row.get(0)?,
			name: row.get(1)?,
			scopes: serde_json::from_str(&row.get::<_, String>(2)?)
				.unwrap_or_default(),
			user_name: row.get(3)?,
			created_at: row.get(4)?,
			last_used: row.get(5)?,
		})
	})?
	.collect()
}

/// Returns the token, it is shown only this once.
fn create(
	conn: &Connection,
	name: &str,
	scopes: &[String],
	user_id: i64,
) -> Result<String> {
	if name.trim().is_empty() {
		return Err(Error::Invalid("the token needs a name".into()));
	}
	for s in scopes {
		check_scope(s)?;
	}

	let token = format!("genco_{}", crate::util::random_hex(24));
	conn.execute(
		"INSERT INTO api_tokens (name, token_hash, scopes, created_at,
		user_id) VALUES (?1, ?2, ?3, ?4, ?5)",
		params![
			name.trim(),
			hash(&token),
			serde_json::to_string(scopes).unwrap(),
			now(),
			user_id
		],
	)?;
	Ok(token)
}

/// The token of `Authorization: Bearer <token>`, trimmed.
pub fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
	headers
		.get("authorization")
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.strip_prefix("Bearer "))
		.map(str::trim)
}

/// Extractor for `Authorization: Bearer <token>`, the handler checks the
/// scope it needs with `require`.
pub struct ApiToken {
	pub name: String,
	scopes: Vec<String>,
	/// the AI agent whose key it is
	pub peer: Option<i64>,
}

impl ApiToken {
	pub fn require(&self, scope: &str) -> Result<()> {
		if self.scopes.iter().any(|s| s == scope) {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
}

/// An admin in the panel or a token with `scope`: who it is, for logs.
pub async fn admin_or_token<S>(
	parts: &mut Parts,
	state: &S,
	scope: &str,
) -> Result<String>
where
	S: Send + Sync,
	Db: FromRef<S>,
{
	if parts.headers.contains_key("authorization") {
		let token = ApiToken::from_request_parts(parts, state).await?;
		token.require(scope)?;
		return Ok(format!("token {}", token.name));
	}
	let authed = Authed::from_request_parts(parts, state).await?;
	Ok(authed.user.name)
}

/// An admin or an AI agent's key.
pub struct Reader {
	/// a token sees no free text (problem details, report titles): what
	/// comes from logs and the agent stays with the admins
	pub token: bool,
}

impl<S> FromRequestParts<S> for Reader
where
	S: Send + Sync,
	Db: FromRef<S>,
{
	type Rejection = Error;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
		let who = admin_or_token(parts, state, READ).await?;
		Ok(Self {
			token: who.starts_with("token "),
		})
	}
}

impl<S> FromRequestParts<S> for ApiToken
where
	S: Send + Sync,
	Db: FromRef<S>,
{
	type Rejection = Error;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
		let token = bearer(&parts.headers).ok_or(Error::MissingSessionToken)?;

		let db = Db::from_ref(state);
		let conn = db.conn();
		let found: Option<(i64, String, String)> = conn
			.query_row(
				"SELECT id, name, scopes FROM api_tokens WHERE token_hash = ?1",
				[hash(token.trim())],
				|row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
			)
			.optional()?;
		let Some((id, name, scopes)) = found else {
			let (peer, name) = crate::peers::by_key(&conn, token)?
				.ok_or(Error::InvalidToken)?;
			return Ok(Self {
				name,
				scopes: crate::peers::SCOPES
					.iter()
					.map(|s| s.to_string())
					.collect(),
				peer: Some(peer),
			});
		};

		conn.execute(
			"UPDATE api_tokens SET last_used = ?1 WHERE id = ?2",
			params![now(), id],
		)?;
		Ok(Self {
			name,
			scopes: serde_json::from_str(&scopes).unwrap_or_default(),
			peer: None,
		})
	}
}

async fn list_tokens(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<Token>>> {
	Ok(Json(list(&db.conn())?))
}

#[derive(Debug, Deserialize)]
struct CreateReq {
	name: String,
	scopes: Vec<String>,
	/// the admin's: a token is lasting access
	password: String,
}

async fn create_token(
	authed: Authed,
	State(db): State<Db>,
	Json(req): Json<CreateReq>,
) -> Result<Json<String>> {
	crate::users::verify_password(&db, &authed.user, &req.password).await?;
	Ok(Json(create(
		&db.conn(),
		&req.name,
		&req.scopes,
		authed.user.id,
	)?))
}

async fn delete_token(
	_authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	db.conn()
		.execute("DELETE FROM api_tokens WHERE id = ?1", [id])?;
	Ok(Json(()))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/", get(list_tokens).post(create_token))
		.route("/{id}", delete(delete_token))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_known_scopes() {
		assert!(check_scope("deploy:3").is_ok());
		assert!(check_scope("deploy:x").is_err());
		assert!(check_scope("databases").is_ok());
		assert!(check_scope("read").is_err());
		assert!(check_scope("admin").is_err());
	}

	#[test]
	fn stores_only_the_hash_and_goes_with_its_admin() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute(
			"INSERT INTO users (email, name, password_hash, created_at)
			VALUES ('a@b.ch', 'a', '', 0)",
			[],
		)
		.unwrap();

		let token = create(&conn, "ci", &["deploy:1".into()], 1).unwrap();
		let stored: String = conn
			.query_row("SELECT token_hash FROM api_tokens", [], |r| r.get(0))
			.unwrap();
		assert_ne!(stored, token);
		assert_eq!(stored, hash(&token));
		assert_eq!(list(&conn).unwrap()[0].user_name.as_deref(), Some("a"));

		conn.execute("DELETE FROM users", []).unwrap();
		assert!(list(&conn).unwrap().is_empty());
	}
}
