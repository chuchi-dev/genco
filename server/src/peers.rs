//! Other AI agents that look after the server too (Start → AI Agents). An
//! agent has one key, not tied to an admin: it reads the status, downloads
//! databases and proposes tasks to genco's agent (proposals.rs) with it,
//! and genco sends it with the webhook (reports, answers to proposals) so
//! the agent knows the call is genco's. The key is stored for that, not
//! only its hash.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::{self, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::notify::Notice;
use crate::tokens;
use crate::users::Authed;

/// What an agent's key may do, the scopes of an API token.
pub const SCOPES: &[&str] = &[tokens::READ, tokens::DATABASES, tokens::PROPOSE];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
	pub id: i64,
	pub name: String,
	/// empty for none: the agent only calls genco
	pub webhook_url: String,
	/// the webhook gets the OK reports too, not only the ones that need
	/// attention
	pub webhook_all: bool,
	pub created_at: i64,
	pub last_used: Option<i64>,
	#[serde(skip)]
	key: String,
}

const COLUMNS: &str =
	"id, name, webhook_url, webhook_all, created_at, last_used, key";

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Peer> {
	Ok(Peer {
		id: row.get(0)?,
		name: row.get(1)?,
		webhook_url: row.get(2)?,
		webhook_all: row.get(3)?,
		created_at: row.get(4)?,
		last_used: row.get(5)?,
		key: row.get(6)?,
	})
}

pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Peer>> {
	let mut stmt =
		conn.prepare(&format!("SELECT {COLUMNS} FROM peers ORDER BY name"))?;
	stmt.query_map([], from_row)?.collect()
}

pub fn get(conn: &Connection, id: i64) -> Result<Peer> {
	conn.query_row(
		&format!("SELECT {COLUMNS} FROM peers WHERE id = ?1"),
		[id],
		from_row,
	)
	.optional()?
	.ok_or(Error::NotFound)
}

/// The agent a key belongs to, marks it used.
pub fn by_key(conn: &Connection, key: &str) -> Result<Option<(i64, String)>> {
	let found = conn
		.query_row(
			"SELECT id, name FROM peers WHERE key_hash = ?1",
			[tokens::hash(key)],
			|row| Ok((row.get(0)?, row.get(1)?)),
		)
		.optional()?;
	if let Some((id, _)) = &found {
		conn.execute(
			"UPDATE peers SET last_used = ?1 WHERE id = ?2",
			params![now(), id],
		)?;
	}
	Ok(found)
}

fn check(name: &str, webhook_url: &str) -> Result<()> {
	if name.trim().is_empty() {
		return Err(Error::Invalid("the agent needs a name".into()));
	}
	let ok = webhook_url.is_empty()
		|| (webhook_url.starts_with("https://")
			&& webhook_url.len() > "https://".len()
			&& !webhook_url
				.chars()
				.any(|c| c.is_whitespace() || c.is_control()));
	if !ok {
		return Err(Error::Invalid(
			"the webhook is an https:// address".into(),
		));
	}
	Ok(())
}

/// Returns the id and the key, the key is shown only this once.
fn create(
	conn: &Connection,
	name: &str,
	webhook_url: &str,
	webhook_all: bool,
	key: Option<String>,
) -> Result<(i64, String)> {
	check(name, webhook_url)?;
	let key = key.unwrap_or_else(tokens::new_token);
	let res = conn.execute(
		"INSERT INTO peers (name, key, key_hash, webhook_url, webhook_all,
		created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
		params![
			name.trim(),
			key,
			tokens::hash(&key),
			webhook_url,
			webhook_all,
			now()
		],
	);
	res?;
	Ok((conn.last_insert_rowid(), key))
}

/// The webhook of genco 0.2 (Settings → Notifications) becomes an agent
/// whose key is the webhook's token, so it keeps working.
pub fn adopt_old_webhook(db: &Db) -> Result<()> {
	let conn = db.conn();
	let Some(old) = crate::db::get_json::<serde_json::Value>(&conn, "notify")
	else {
		return Ok(());
	};
	let field = |name: &str| old[name].as_str().unwrap_or("").to_string();
	let url = field("webhookUrl");
	if url.is_empty() {
		return Ok(());
	}

	let token = field("webhookToken");
	let key = (!token.is_empty()).then_some(token);
	let all = old["webhookAll"].as_bool().unwrap_or(false);
	create(&conn, "Webhook", &url, all, key)?;
	let mail = old["mail"].as_bool().unwrap_or(true);
	crate::db::set_setting(
		&conn,
		"notify",
		&serde_json::json!({ "mail": mail }).to_string(),
	)?;
	tracing::info!("the notification webhook is the AI agent Webhook now");
	Ok(())
}

#[derive(Serialize)]
struct Payload<'a> {
	/// the panel's domain, which server it is about
	server: &'a str,
	/// "ok" or "attention"
	status: &'static str,
	#[serde(flatten)]
	notice: &'a Notice,
}

/// A POST to the agent's webhook with its key as the bearer token.
pub async fn send(cfg: &Config, peer: &Peer, n: &Notice) -> Result<()> {
	let payload = Payload {
		server: cfg.domain.as_deref().unwrap_or("localhost"),
		status: if n.attention { "attention" } else { "ok" },
		notice: n,
	};
	let res = reqwest::Client::new()
		.post(&peer.webhook_url)
		.timeout(Duration::from_secs(20))
		.bearer_auth(&peer.key)
		.json(&payload)
		.send()
		.await
		.map_err(|e| Error::Internal(format!("webhook: {e}")))?;
	if !res.status().is_success() {
		return Err(Error::Internal(format!(
			"webhook answered {}",
			res.status()
		)));
	}
	Ok(())
}

/// A report to every agent with a webhook that wants it; errors are
/// logged, a report must not fail because a webhook is down.
pub async fn notify(cfg: &Config, db: &Db, n: &Notice) {
	let peers = match list(&db.conn()) {
		Ok(p) => p,
		Err(e) => {
			tracing::error!("AI agents: {e}");
			return;
		}
	};
	for p in peers {
		if p.webhook_url.is_empty() || !(n.attention || p.webhook_all) {
			continue;
		}
		if let Err(e) = send(cfg, &p, n).await {
			tracing::error!("webhook of {} about {}: {e}", p.name, n.title);
		}
	}
}

async fn list_route(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<Peer>>> {
	Ok(Json(list(&db.conn())?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateReq {
	name: String,
	#[serde(default)]
	webhook_url: String,
	#[serde(default)]
	webhook_all: bool,
	/// the admin's password and code: a key is lasting access
	#[serde(flatten)]
	auth: crate::users::PasswordOnly,
}

async fn create_route(
	authed: Authed,
	State(db): State<Db>,
	Json(req): Json<CreateReq>,
) -> Result<Json<String>> {
	crate::users::verify_password_and_code(&db, &authed.user, &req.auth)
		.await?;
	let (_, key) = create(
		&db.conn(),
		&req.name,
		req.webhook_url.trim(),
		req.webhook_all,
		None,
	)?;
	Ok(Json(key))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangeReq {
	#[serde(default)]
	webhook_url: String,
	#[serde(default)]
	webhook_all: bool,
	/// the admin's, and the 2FA code, when the webhook changes: genco
	/// sends the key there
	#[serde(default)]
	password: String,
	#[serde(default)]
	code: String,
}

async fn change_route(
	authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
	Json(req): Json<ChangeReq>,
) -> Result<Json<()>> {
	let peer = get(&db.conn(), id)?;
	let url = req.webhook_url.trim();
	check(&peer.name, url)?;
	if url != peer.webhook_url {
		crate::users::verify_password(&db, &authed.user, &req.password).await?;
		crate::users::verify_code(&db, &authed.user, &req.code).await?;
	}
	let conn = db.conn();
	conn.execute(
		"UPDATE peers SET webhook_url = ?1, webhook_all = ?2 WHERE id = ?3",
		params![url, req.webhook_all, id],
	)?;
	Ok(Json(()))
}

/// A new key, the old one stops working.
async fn new_key_route(
	authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
	Json(req): Json<crate::users::PasswordOnly>,
) -> Result<Json<String>> {
	crate::users::verify_password_and_code(&db, &authed.user, &req).await?;
	let key = tokens::new_token();
	let changed = db.conn().execute(
		"UPDATE peers SET key = ?1, key_hash = ?2 WHERE id = ?3",
		params![key, tokens::hash(&key), id],
	)?;
	if changed == 0 {
		return Err(Error::NotFound);
	}
	Ok(Json(key))
}

async fn delete_route(
	_authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	db.conn().execute("DELETE FROM peers WHERE id = ?1", [id])?;
	Ok(Json(()))
}

/// A test to the agent's webhook, its error goes back to the browser.
async fn test_route(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	let peer = get(&db.conn(), id)?;
	if peer.webhook_url.is_empty() {
		return Err(Error::Invalid("the agent has no webhook".into()));
	}
	let n = Notice {
		kind: "test",
		attention: false,
		title: "Test".into(),
		text: format!(
			"{} sent this test from genco. Nothing to do.",
			authed.user.name
		),
		url: None,
		proposal: None,
	};
	send(&cfg, &peer, &n).await?;
	Ok(Json(()))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/peers", routing::get(list_route).post(create_route))
		.route("/peers/{id}", post(change_route).delete(delete_route))
		.route("/peers/{id}/key", post(new_key_route))
		.route("/peers/{id}/test", post(test_route))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn webhooks_only_over_https() {
		assert!(check("a", "").is_ok());
		assert!(check("a", "https://noah.example/api/x").is_ok());
		assert!(check("a", "http://noah.example/api/x").is_err());
		assert!(check("a", "https://").is_err());
		assert!(check("a", "https://a b").is_err());
		assert!(check(" ", "").is_err());
	}

	#[test]
	fn the_key_finds_its_agent_and_never_goes_to_the_browser() {
		let db = Db::memory();
		let conn = db.conn();
		let (id, key) = create(&conn, "Corleone", "", false, None).unwrap();
		assert_eq!(by_key(&conn, &key).unwrap(), Some((id, "Corleone".into())));
		assert_eq!(by_key(&conn, "genco_other").unwrap(), None);
		assert!(get(&conn, id).unwrap().last_used.is_some());

		let json = serde_json::to_string(&list(&conn).unwrap()).unwrap();
		assert!(!json.contains(&key));
		assert!(create(&conn, "Corleone", "", false, None).is_err());
	}

	#[test]
	fn the_old_webhook_becomes_an_agent() {
		let db = Db::memory();
		crate::db::set_setting(
			&db.conn(),
			"notify",
			r#"{"mail":false,"webhookUrl":"https://a.example/hook",
			"webhookToken":"tok","webhookAll":true}"#,
		)
		.unwrap();
		adopt_old_webhook(&db).unwrap();
		adopt_old_webhook(&db).unwrap();

		let conn = db.conn();
		let peers = list(&conn).unwrap();
		assert_eq!(peers.len(), 1);
		assert_eq!(peers[0].webhook_url, "https://a.example/hook");
		assert!(peers[0].webhook_all);
		assert!(by_key(&conn, "tok").unwrap().is_some());
		let notify: serde_json::Value =
			crate::db::get_json(&conn, "notify").unwrap();
		assert_eq!(notify, serde_json::json!({ "mail": false }));
	}
}
