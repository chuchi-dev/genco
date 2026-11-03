//! The activity log: who did what, from the panel, an API token, the
//! terminal or the agent. Every change through the API is logged by the
//! middleware `record`, logins and the CLI log themselves.

use axum::body::{Body, to_bytes};
use axum::extract::{Query, Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;
use crate::db::{Db, now};
use crate::error::Result;
use crate::tokens;
use crate::users::Authed;

/// how long entries stay
const KEEP_MS: i64 = 365 * 24 * 60 * 60 * 1000;
/// JSON bodies up to this size are read for the names in them
const MAX_BODY: usize = 64 * 1024;
/// what axum takes for JSON anyway
const MAX_JSON: usize = 2 * 1024 * 1024;
/// fields of a request body that tell what an action was about, never
/// passwords or contents
const FIELDS: &[&str] =
	&["name", "email", "path", "from", "to", "ref", "address"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
	pub id: i64,
	pub at: i64,
	/// an admin, `token <name>`, `AI agent <name>`, `agent`, `terminal`, `systemd`, `genco`
	pub actor: String,
	pub action: String,
	/// the client, group, domain, … it was about
	pub target: String,
	pub detail: String,
	/// `None` when the outcome isn't known
	pub ok: Option<bool>,
}

pub fn log(
	conn: &Connection,
	actor: &str,
	action: &str,
	target: &str,
	detail: &str,
	ok: Option<bool>,
) {
	let res = conn.execute(
		"INSERT INTO activity (at, actor, action, target, detail, ok)
		VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
		params![now(), actor, action, target, detail, ok],
	);
	if let Err(e) = res {
		tracing::error!("activity log: {e}");
	}
}

/// Who runs a `genco` command.
pub fn cli_actor() -> &'static str {
	if std::env::var_os("GENCO_AGENT").is_some() {
		"agent"
	} else if std::env::var_os("INVOCATION_ID").is_some() {
		"systemd"
	} else {
		"terminal"
	}
}

pub fn prune(conn: &Connection) {
	let _ =
		conn.execute("DELETE FROM activity WHERE at < ?1", [now() - KEEP_MS]);
}

/// What a request does, `None` for reads that aren't worth a line.
fn describe(method: &Method, path: &str) -> Option<String> {
	// a download's ticket (the download itself is logged), the build
	// runners' polls and logs (the deploy's log has them)
	if path == "/users/tickets" || path.starts_with("/runner/") {
		return None;
	}
	const ACTIONS: &[(&str, &str, &str)] = &[
		("POST", "/apply", "Apply changes"),
		("POST", "/clients", "Add client"),
		("POST", "/clients/*", "Change client"),
		("DELETE", "/clients/*", "Delete client"),
		("POST", "/groups", "Add group"),
		("POST", "/groups/*", "Change group"),
		("DELETE", "/groups/*", "Ask to delete group"),
		("POST", "/domains", "Add domain"),
		("POST", "/domains/*", "Change domain"),
		("DELETE", "/domains/*", "Ask to delete domain"),
		("POST", "/databases", "Add database"),
		("DELETE", "/databases/*", "Ask to delete database"),
		("GET", "/databases/*/dump", "Download database"),
		("PUT", "/databases/*/import", "Import database"),
		("POST", "/actions/*/confirm", "Confirm deletion"),
		("POST", "/backups", "Back up now"),
		("POST", "/groups/*/backup", "Back up group"),
		("POST", "/deploy", "Deploy (CI)"),
		("POST", "/groups/*/deploy", "Deploy"),
		("POST", "/groups/*/rollback", "Roll back"),
		("GET", "/groups/*/files/read", "Open file"),
		("PUT", "/groups/*/files/write", "Save file"),
		("POST", "/groups/*/files/mkdir", "New folder"),
		("POST", "/groups/*/files/create", "New file"),
		("POST", "/groups/*/files/rename", "Rename"),
		("POST", "/groups/*/files/copy", "Copy"),
		("POST", "/groups/*/files/extract", "Unzip"),
		("POST", "/groups/*/files/delete", "Delete file"),
		("GET", "/groups/*/files/download", "Download files"),
		("POST", "/conversations", "New conversation"),
		("POST", "/conversations/*/messages", "Message to the agent"),
		("POST", "/conversations/*/stop", "Stop the agent"),
		("POST", "/conversations/*/resolve", "Mark resolved"),
		("POST", "/routines/*/run", "Run routine"),
		("POST", "/users/add", "Add admin"),
		("POST", "/users/invite", "Invite admin"),
		("POST", "/users/*/invite", "Send invite again"),
		("DELETE", "/users/*", "Delete admin"),
		("POST", "/users/password", "Change password"),
		("POST", "/users/account", "Change email"),
		("POST", "/users/email/*", "Confirm new email"),
		("POST", "/users/logout", "Log out"),
		("POST", "/users/totp/enable", "Turn on two-factor login"),
		("POST", "/users/totp/disable", "Turn off two-factor login"),
		("POST", "/mail-domains", "Add mail domain"),
		("DELETE", "/mail-domains/*", "Ask to delete mail domain"),
		("POST", "/mail-domains/*/mailboxes", "Add mailbox"),
		("POST", "/mail-domains/*/forwards", "Set forward"),
		("POST", "/mailboxes/*", "Change mailbox"),
		("DELETE", "/mailboxes/*", "Ask to delete mailbox"),
		("DELETE", "/forwards/*", "Delete forward"),
		("POST", "/secrets", "Save password"),
		("POST", "/secrets/*", "Change password entry"),
		("DELETE", "/secrets/*", "Delete password"),
		("GET", "/secrets/*/value", "Show password"),
		("POST", "/firewall/blocks", "Block address"),
		("DELETE", "/firewall/blocks/*", "Unblock address"),
		("POST", "/tokens", "Create API token"),
		("DELETE", "/tokens/*", "Delete API token"),
		("POST", "/settings/mail", "Change mail settings"),
		("POST", "/settings/mail/test", "Send test mail"),
		("POST", "/settings/notify", "Change notifications"),
		("POST", "/settings/model", "Change the agent's model"),
		("POST", "/domains/*/env", "Change environment variables"),
		("GET", "/groups/*/smtp", "Show SMTP login"),
		("POST", "/runners", "Add runner"),
		("DELETE", "/runners/*", "Remove runner"),
		("PUT", "/deploy/artifact", "Deploy upload"),
		("POST", "/peers", "Add AI agent"),
		("POST", "/peers/*", "Change AI agent"),
		("DELETE", "/peers/*", "Delete AI agent"),
		("POST", "/peers/*/key", "New key for AI agent"),
		("POST", "/peers/*/test", "Test webhook"),
		("POST", "/proposals", "Propose a task"),
		("POST", "/proposals/*/approve", "Approve proposal"),
		("POST", "/proposals/*/reject", "Reject proposal"),
		("POST", "/update/check", "Check for updates"),
		("POST", "/update/install", "Start update"),
		("POST", "/update/auto", "Change automatic updates"),
	];

	let segments: Vec<&str> = path.split('/').collect();
	let found = ACTIONS.iter().find(|(m, pattern, _)| {
		let p: Vec<&str> = pattern.split('/').collect();
		*m == method.as_str()
			&& p.len() == segments.len()
			&& p.iter().zip(&segments).all(|(p, s)| *p == "*" || p == s)
	});
	match found {
		Some((_, _, action)) => Some(action.to_string()),
		// a change genco doesn't know a name for yet
		None if *method != Method::GET => {
			// long segments may be tokens of links
			let path: Vec<&str> = segments
				.iter()
				.map(|s| if s.len() > 20 { "…" } else { s })
				.collect();
			Some(format!("{method} {}", path.join("/")))
		}
		None => None,
	}
}

/// The name of what `/<kind>/<id>/...` is about.
fn target(conn: &Connection, path: &str) -> String {
	let mut segments = path.split('/').skip(1);
	let (Some(kind), Some(id)) = (segments.next(), segments.next()) else {
		return String::new();
	};
	let Ok(id) = id.parse::<i64>() else {
		// names in the path, e.g. a database dump or a routine
		return match kind {
			"databases" | "routines" => id.to_string(),
			"firewall" => segments
				.next()
				.and_then(|id| id.parse().ok())
				.and_then(|id| crate::firewall::address(conn, id))
				.unwrap_or_default(),
			_ => String::new(),
		};
	};
	let sql = match kind {
		"clients" => "SELECT name FROM clients WHERE id = ?1",
		"groups" => "SELECT name FROM domain_groups WHERE id = ?1",
		"domains" => "SELECT name FROM domains WHERE id = ?1",
		"databases" => "SELECT name FROM databases WHERE id = ?1",
		"users" => "SELECT email FROM users WHERE id = ?1",
		"tokens" => "SELECT name FROM api_tokens WHERE id = ?1",
		"secrets" => "SELECT name FROM secrets WHERE id = ?1",
		"mail-domains" => "SELECT name FROM mail_domains WHERE id = ?1",
		"mailboxes" => {
			"SELECT b.local_part || '@' || d.name FROM mailboxes b
			JOIN mail_domains d ON d.id = b.mail_domain_id WHERE b.id = ?1"
		}
		"forwards" => {
			"SELECT f.local_part || '@' || d.name FROM mail_forwards f
			JOIN mail_domains d ON d.id = f.mail_domain_id WHERE f.id = ?1"
		}
		"conversations" => "SELECT title FROM conversations WHERE id = ?1",
		"peers" => "SELECT name FROM peers WHERE id = ?1",
		"proposals" => {
			"SELECT p.name FROM proposals r JOIN peers p ON p.id = r.peer_id
			WHERE r.id = ?1"
		}
		_ => return String::new(),
	};
	conn.query_row(sql, [id], |r| r.get(0))
		.optional()
		.ok()
		.flatten()
		.unwrap_or_default()
}

/// The admin of a session or the API token of a request.
fn actor(conn: &Connection, req: &Request) -> Option<String> {
	let header = |name| req.headers().get(name).and_then(|v| v.to_str().ok());
	let query = |name: &str| {
		req.uri().query().and_then(|q| {
			q.split('&').find_map(|pair| {
				pair.strip_prefix(name)?
					.strip_prefix('=')
					.map(str::to_string)
			})
		})
	};
	// a download names its session through its ticket
	let session = header("session-token").map(tokens::hash).or_else(|| {
		query("ticket").and_then(|t| crate::users::ticket_session(&t))
	});

	if let Some(hashed) = session {
		return conn
			.query_row(
				"SELECT u.name FROM sessions s
				JOIN users u ON u.id = s.user_id WHERE s.token = ?1",
				[hashed],
				|r| r.get(0),
			)
			.optional()
			.ok()
			.flatten();
	}
	let bearer = tokens::hash(tokens::bearer(req.headers())?);
	let token = conn
		.query_row(
			"SELECT t.name, u.name FROM api_tokens t
			LEFT JOIN users u ON u.id = t.user_id WHERE t.token_hash = ?1",
			[&bearer],
			|r| {
				let name: String = r.get(0)?;
				let user: Option<String> = r.get(1)?;
				Ok(match user {
					Some(u) => format!("token {name} ({u})"),
					None => format!("token {name}"),
				})
			},
		)
		.optional()
		.ok()
		.flatten();
	token.or_else(|| {
		conn.query_row(
			"SELECT name FROM peers WHERE key_hash = ?1",
			[&bearer],
			|r| Ok(format!("AI agent {}", r.get::<_, String>(0)?)),
		)
		.optional()
		.ok()
		.flatten()
	})
}

/// `path=` and the names of a JSON body, nothing secret.
fn detail(query: Option<&str>, body: &[u8]) -> String {
	let mut parts = vec![];
	for pair in query.unwrap_or("").split('&') {
		if let Some(p) = pair.strip_prefix("path=") {
			parts.push(format!("path {}", decode(p)));
		}
	}
	if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) {
		for field in FIELDS {
			match map.get(*field) {
				Some(Value::String(s)) if !s.is_empty() => {
					parts.push(format!("{field} {s}"))
				}
				Some(Value::Array(a)) if *field == "path" => {
					parts.push(format!("path {}", Value::Array(a.clone())))
				}
				_ => {}
			}
		}
	}
	parts.join(", ")
}

/// Percent decoding for the query, `+` is a space.
fn decode(s: &str) -> String {
	let bytes = s.as_bytes();
	let mut out = vec![];
	let mut i = 0;
	while i < bytes.len() {
		match bytes[i] {
			b'%' if i + 2 < bytes.len() => {
				let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
				match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
					Some(b) => {
						out.push(b);
						i += 3;
						continue;
					}
					None => out.push(b'%'),
				}
			}
			b'+' => out.push(b' '),
			b => out.push(b),
		}
		i += 1;
	}
	String::from_utf8_lossy(&out).into_owned()
}

/// Middleware of the API: logs every change and every download.
pub async fn record(
	State(db): State<Db>,
	req: Request,
	next: Next,
) -> Response {
	// below /api; the login and invites log themselves, with the email
	let path = req.uri().path().to_string();
	let own = path == "/users/login" || path.starts_with("/users/invite/");
	let Some(action) = describe(req.method(), &path).filter(|_| !own) else {
		return next.run(req).await;
	};

	let (actor, target) = {
		let conn = db.conn();
		(actor(&conn, &req), target(&conn, &path))
	};

	// the body is read for the names in it, file uploads are skipped
	let json = req
		.headers()
		.get("content-type")
		.is_some_and(|v| v.as_bytes().starts_with(b"application/json"));
	let query = req.uri().query().map(str::to_string);
	let (req, body) = if json {
		let (parts, body) = req.into_parts();
		let Ok(bytes) = to_bytes(body, MAX_JSON).await else {
			return axum::http::StatusCode::PAYLOAD_TOO_LARGE.into_response();
		};
		let names = if bytes.len() <= MAX_BODY {
			bytes.clone()
		} else {
			Default::default()
		};
		(Request::from_parts(parts, Body::from(bytes)), names)
	} else {
		(req, Default::default())
	};
	let detail = detail(query.as_deref(), &body);

	let resp = next.run(req).await;
	let status = resp.status();

	// strangers knocking without a login are no activity
	let actor = match actor {
		Some(a) => a,
		None if status.as_u16() == 401 || status.as_u16() == 403 => {
			return resp;
		}
		None if path.starts_with("/actions/") => "mail link".into(),
		None => "anonymous".into(),
	};
	log(
		&db.conn(),
		&actor,
		&action,
		&target,
		&detail,
		Some(status.is_success()),
	);
	resp
}

#[derive(Debug, Deserialize)]
struct ListQuery {
	/// entries older than this id
	before: Option<i64>,
}

pub fn list(
	conn: &Connection,
	before: Option<i64>,
	limit: i64,
) -> rusqlite::Result<Vec<Entry>> {
	let mut stmt = conn.prepare(
		"SELECT id, at, actor, action, target, detail, ok FROM activity
		WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
	)?;
	stmt.query_map(params![before.unwrap_or(i64::MAX), limit], |r| {
		Ok(Entry {
			id: r.get(0)?,
			at: r.get(1)?,
			actor: r.get(2)?,
			action: r.get(3)?,
			target: r.get(4)?,
			detail: r.get(5)?,
			ok: r.get(6)?,
		})
	})?
	.collect()
}

async fn list_route(
	_authed: Authed,
	State(db): State<Db>,
	Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Entry>>> {
	Ok(Json(list(&db.conn(), q.before, 200)?))
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/activity", get(list_route))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn describes_changes_and_downloads_but_not_reads() {
		let d = |m: Method, p| describe(&m, p);
		assert_eq!(d(Method::POST, "/groups/3/deploy").unwrap(), "Deploy");
		assert_eq!(
			d(Method::GET, "/databases/shop/dump").unwrap(),
			"Download database"
		);
		assert_eq!(d(Method::GET, "/groups/3/files"), None);
		assert_eq!(d(Method::POST, "/new/thing").unwrap(), "POST /new/thing");
		assert_eq!(
			d(Method::POST, &format!("/new/{}", "ab".repeat(32))).unwrap(),
			"POST /new/…"
		);
	}

	#[test]
	fn detail_has_names_but_no_secrets() {
		let body = br#"{"email":"a@b.c","password":"secret","name":"A"}"#;
		let d = detail(Some("path=web%2Fa+b.txt&token=x"), body);
		assert_eq!(d, "path web/a b.txt, name A, email a@b.c");
		assert!(!d.contains("secret"));
	}

	#[test]
	fn target_is_the_name_behind_the_id() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute(
			"INSERT INTO clients (name, created_at) VALUES ('Acme', 0)",
			[],
		)
		.unwrap();
		assert_eq!(target(&conn, "/clients/1"), "Acme");
		assert_eq!(target(&conn, "/databases/shop/dump"), "shop");
		assert_eq!(target(&conn, "/apply"), "");
	}
}
