//! Tasks an AI agent (peers.rs) proposes to genco's agent. A proposal waits
//! until an admin approves it, then it becomes a conversation. The agent
//! may name the admin who decides (only they get the mail and may decide),
//! otherwise every admin gets the mail and any decides. When the agent is done, its last answer goes back to the
//! proposing agent's webhook; the agent can also poll the proposal.

use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::agent::Runner;
use crate::config::Config;
use crate::conversations::{self as convs, Message, MessageData};
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::notify::Notice;
use crate::peers;
use crate::tokens::{self, ApiToken};
use crate::users::Authed;

/// A proposal nobody decided on in this time expires.
const VALID_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const MAX_TEXT: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
	pub id: i64,
	/// the agent's name
	pub from: String,
	pub text: String,
	/// the admin who decides, `None` for any
	pub approver: Option<String>,
	/// pending, approved (the agent works on it), answered, rejected,
	/// expired
	pub status: String,
	pub decided_by: Option<String>,
	pub conversation_id: Option<i64>,
	pub answer: Option<String>,
	pub created_at: i64,
	#[serde(skip)]
	peer_id: i64,
	#[serde(skip)]
	approver_id: Option<i64>,
}

const SELECT: &str = "SELECT r.id, p.name, r.text, r.status, r.decided_by,
	r.conversation_id, r.answer, r.created_at, r.peer_id, u.name, u.email,
	r.approver_id
	FROM proposals r JOIN peers p ON p.id = r.peer_id
	LEFT JOIN users u ON u.id = r.approver_id";

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Proposal> {
	let status: String = row.get(3)?;
	let created_at: i64 = row.get(7)?;
	let expired = status == "pending" && created_at + VALID_MS < now();
	Ok(Proposal {
		id: row.get(0)?,
		from: row.get(1)?,
		text: row.get(2)?,
		status: if expired { "expired".into() } else { status },
		decided_by: row.get(4)?,
		conversation_id: row.get(5)?,
		answer: row.get(6)?,
		created_at,
		peer_id: row.get(8)?,
		approver: row
			.get::<_, Option<String>>(9)?
			.zip(row.get::<_, Option<String>>(10)?)
			.map(|(name, email)| format!("{name} <{email}>")),
		approver_id: row.get(11)?,
	})
}

fn list(conn: &Connection) -> rusqlite::Result<Vec<Proposal>> {
	let mut stmt =
		conn.prepare(&format!("{SELECT} ORDER BY r.id DESC LIMIT 200"))?;
	stmt.query_map([], from_row)?.collect()
}

fn get_one(conn: &Connection, id: i64) -> Result<Proposal> {
	conn.query_row(&format!("{SELECT} WHERE r.id = ?1"), [id], from_row)
		.optional()?
		.ok_or(Error::NotFound)
}

/// The admin with this email, the agent names who decides.
fn approver(conn: &Connection, email: &str) -> Result<i64> {
	conn.query_row(
		"SELECT id FROM users WHERE lower(email) = lower(?1)",
		[email.trim()],
		|r| r.get(0),
	)
	.optional()?
	.ok_or_else(|| Error::Invalid(format!("no admin has the email {email}")))
}

fn insert(
	conn: &Connection,
	peer_id: i64,
	text: &str,
	approver_id: Option<i64>,
) -> Result<i64> {
	if text.trim().is_empty() {
		return Err(Error::Invalid("the proposal has no text".into()));
	}
	if text.len() > MAX_TEXT {
		return Err(Error::Invalid(format!(
			"a proposal has at most {MAX_TEXT} bytes"
		)));
	}
	conn.execute(
		"INSERT INTO proposals (peer_id, text, approver_id, status,
		created_at) VALUES (?1, ?2, ?3, 'pending', ?4)",
		params![peer_id, text.trim(), approver_id, now()],
	)?;
	Ok(conn.last_insert_rowid())
}

/// Moves a pending proposal to `status`: its approver decides, or any
/// admin when it has none; only one decision counts.
fn decide(
	conn: &Connection,
	id: i64,
	status: &str,
	by: &crate::users::User,
) -> Result<()> {
	let p = get_one(conn, id)?;
	if p.approver_id.is_some_and(|a| a != by.id) {
		return Err(Error::Invalid(format!(
			"only {} decides on this proposal",
			p.approver.unwrap_or_default()
		)));
	}
	if p.status != "pending" {
		return Err(Error::Invalid(format!(
			"the proposal is {} already",
			p.status
		)));
	}
	let changed = conn.execute(
		"UPDATE proposals SET status = ?1, decided_by = ?2, decided_at = ?3
		WHERE id = ?4 AND status = 'pending'",
		params![status, by.name, now(), id],
	)?;
	if changed != 1 {
		return Err(Error::Invalid("the proposal is decided already".into()));
	}
	Ok(())
}

/// The first line, shortened for a title or a subject.
fn first_line(text: &str) -> String {
	let line = text.lines().next().unwrap_or("");
	if line.chars().count() > 60 {
		line.chars().take(57).collect::<String>() + "..."
	} else {
		line.into()
	}
}

/// Tells the proposing agent, errors are logged.
async fn tell(cfg: &Config, db: &Db, p: &Proposal, text: String) {
	let Ok(peer) = peers::get(&db.conn(), p.peer_id) else {
		return;
	};
	if peer.webhook_url.is_empty() {
		return;
	}
	let n = Notice {
		kind: "proposal",
		attention: false,
		title: first_line(&p.text),
		text,
		url: (cfg.domain.as_ref())
			.zip(p.conversation_id)
			.map(|(d, c)| format!("https://{d}/?chat={c}")),
		proposal: Some(p.id),
	};
	if let Err(e) = peers::send(cfg, &peer, &n).await {
		tracing::error!("answer to {} about proposal {}: {e}", peer.name, p.id);
	}
}

/// A conversation ended: if it came from a proposal that wasn't answered
/// yet, the agent's last words go back to the proposing agent. Later runs
/// of the conversation (an admin goes on) aren't sent.
pub async fn answer(cfg: &Config, db: &Db, conversation: i64, failed: bool) {
	let found = (|| -> Result<Option<(Proposal, Vec<Message>)>> {
		let conn = db.conn();
		let id: Option<i64> = conn
			.query_row(
				"SELECT id FROM proposals
				WHERE conversation_id = ?1 AND status = 'approved'",
				[conversation],
				|r| r.get(0),
			)
			.optional()?;
		let Some(id) = id else { return Ok(None) };
		Ok(Some((
			get_one(&conn, id)?,
			convs::messages(&conn, conversation)?,
		)))
	})();
	let (p, messages) = match found {
		Ok(Some(found)) => found,
		Ok(None) => return,
		Err(e) => {
			tracing::error!("proposal of conversation {conversation}: {e}");
			return;
		}
	};

	let last = messages.iter().rev().find_map(|m| match &m.data {
		MessageData::Assistant { text } => Some(text.clone()),
		MessageData::Error { text } if failed => {
			Some(format!("genco's agent couldn't finish: {text}"))
		}
		_ => None,
	});
	let text = last.unwrap_or_else(|| "genco's agent gave no answer.".into());
	let stored = db.conn().execute(
		"UPDATE proposals SET status = 'answered', answer = ?1,
		answered_at = ?2 WHERE id = ?3",
		params![text, now(), p.id],
	);
	if let Err(e) = stored {
		tracing::error!("proposal {}: {e}", p.id);
	}
	tell(cfg, db, &p, text).await;
}

#[derive(Debug, Deserialize)]
struct ProposeReq {
	text: String,
	/// the email of the admin who decides, empty for any
	#[serde(default)]
	approver: String,
}

#[derive(Debug, Serialize)]
struct Created {
	id: i64,
	status: &'static str,
}

/// An agent proposes a task, its approver or every admin gets a mail.
async fn propose(
	token: ApiToken,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<ProposeReq>,
) -> Result<Json<Created>> {
	token.require(tokens::PROPOSE)?;
	let peer = token.peer.ok_or(Error::Forbidden)?;
	let (id, to) = {
		let conn = db.conn();
		let approver_id = match req.approver.trim() {
			"" => None,
			email => Some(approver(&conn, email)?),
		};
		let id = insert(&conn, peer, &req.text, approver_id)?;
		let to = match get_one(&conn, id)?.approver_id {
			Some(a) => vec![crate::users::by_id(&conn, a)?.email],
			None => crate::users::list(&conn)?
				.into_iter()
				.map(|u| u.email)
				.collect(),
		};
		(id, to)
	};

	let link = cfg
		.domain
		.as_ref()
		.map(|d| format!("https://{d}/agents\n\n"))
		.unwrap_or_default();
	let body = format!(
		"The AI agent {} proposes this task for genco's agent:\n\n{}\n\n\
		To approve or reject it, open Start → AI Agents → Proposals:\n\n\
		{link}Nothing happens without an approval. It expires in 7 days.\n",
		token.name,
		req.text.trim()
	);
	let subject = format!(
		"Proposal from {}: {}",
		token.name,
		first_line(req.text.trim())
	);
	if let Err(e) = crate::mail::send(&cfg, &to, &subject, &body).await {
		tracing::error!("mail about proposal {id}: {e}");
	}
	Ok(Json(Created {
		id,
		status: "pending",
	}))
}

/// An admin in the panel, or `Some(agent)` for an agent's key.
struct Viewer(Option<i64>);

impl<S> FromRequestParts<S> for Viewer
where
	S: Send + Sync,
	Db: FromRef<S>,
{
	type Rejection = Error;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
		if parts.headers.contains_key("authorization") {
			let token = ApiToken::from_request_parts(parts, state).await?;
			return Ok(Self(Some(token.peer.ok_or(Error::Forbidden)?)));
		}
		Authed::from_request_parts(parts, state).await?;
		Ok(Self(None))
	}
}

/// An admin or the agent that proposed it.
async fn get_route(
	viewer: Viewer,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<Proposal>> {
	let p = get_one(&db.conn(), id)?;
	if viewer.0.is_some_and(|peer| peer != p.peer_id) {
		return Err(Error::NotFound);
	}
	Ok(Json(p))
}

async fn list_route(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<Proposal>>> {
	Ok(Json(list(&db.conn())?))
}

/// The proposal becomes a conversation, returns its id.
async fn approve(
	authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	State(runner): State<Runner>,
	Path(id): Path<i64>,
) -> Result<Json<i64>> {
	let by = authed.user.name.clone();
	let conversation = {
		let conn = db.conn();
		decide(&conn, id, "approved", &authed.user)?;
		let p = get_one(&conn, id)?;
		let c = convs::create(
			&conn,
			&format!("{}: {}", p.from, first_line(&p.text)),
		)?;
		conn.execute(
			"UPDATE proposals SET conversation_id = ?1 WHERE id = ?2",
			params![c, id],
		)?;
		let intro = format!(
			"The AI agent {} proposed this task through its API key, {by} \
			approved it. Your last message goes back to {} as the answer.",
			p.from, p.from
		);
		for data in [
			MessageData::System { text: intro },
			MessageData::User {
				text: p.text,
				user_name: Some(format!("{} (AI agent)", p.from)),
				files: vec![],
			},
		] {
			let message = convs::insert_message(&conn, c, data)?;
			events.send(Event::Message { message });
		}
		c
	};
	runner.start(conversation);
	Ok(Json(conversation))
}

async fn reject(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	let by = authed.user.name.clone();
	let p = {
		let conn = db.conn();
		decide(&conn, id, "rejected", &authed.user)?;
		get_one(&conn, id)?
	};
	tell(&cfg, &db, &p, format!("{by} rejected the proposal.")).await;
	Ok(Json(()))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/proposals", get(list_route).post(propose))
		.route("/proposals/{id}", get(get_route))
		.route("/proposals/{id}/approve", post(approve))
		.route("/proposals/{id}/reject", post(reject))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn peer(conn: &Connection) -> i64 {
		conn.execute(
			"INSERT INTO peers (name, key, key_hash, created_at)
			VALUES ('Corleone', 'k', 'h', 0)",
			[],
		)
		.unwrap();
		conn.last_insert_rowid()
	}

	fn admin(conn: &Connection, name: &str) -> crate::users::User {
		conn.execute(
			"INSERT INTO users (email, name, password_hash, created_at)
			VALUES (?1, ?2, '', 0)",
			[format!("{name}@example.ch"), name.into()],
		)
		.unwrap();
		crate::users::by_id(conn, conn.last_insert_rowid()).unwrap()
	}

	#[test]
	fn only_the_named_approver_decides() {
		let db = Db::memory();
		let conn = db.conn();
		let (a, b) = (admin(&conn, "a"), admin(&conn, "b"));
		assert!(approver(&conn, "x@example.ch").is_err());
		let named = approver(&conn, " B@example.ch").unwrap();
		let id =
			insert(&conn, peer(&conn), "Update Craft", Some(named)).unwrap();
		assert_eq!(
			get_one(&conn, id).unwrap().approver.as_deref(),
			Some("b <b@example.ch>")
		);

		assert!(decide(&conn, id, "approved", &a).is_err());
		decide(&conn, id, "approved", &b).unwrap();

		// a deleted approver leaves it to any admin
		let id = insert(&conn, 1, "Update PHP", Some(named)).unwrap();
		conn.execute("DELETE FROM users WHERE id = ?1", [named])
			.unwrap();
		decide(&conn, id, "approved", &a).unwrap();
	}

	#[test]
	fn only_one_decision() {
		let db = Db::memory();
		let conn = db.conn();
		let id = insert(&conn, peer(&conn), "Update Craft", None).unwrap();
		assert_eq!(get_one(&conn, id).unwrap().status, "pending");

		let (a, b) = (admin(&conn, "a"), admin(&conn, "b"));
		decide(&conn, id, "rejected", &a).unwrap();
		assert!(decide(&conn, id, "approved", &b).is_err());
		let p = get_one(&conn, id).unwrap();
		assert_eq!(p.status, "rejected");
		assert_eq!(p.decided_by.as_deref(), Some("a"));
		assert_eq!(p.from, "Corleone");
	}

	#[test]
	fn old_proposals_expire() {
		let db = Db::memory();
		let conn = db.conn();
		let id = insert(&conn, peer(&conn), "Update Craft", None).unwrap();
		conn.execute("UPDATE proposals SET created_at = 0", [])
			.unwrap();
		assert_eq!(get_one(&conn, id).unwrap().status, "expired");
		assert!(decide(&conn, id, "approved", &admin(&conn, "a")).is_err());
	}

	#[test]
	fn empty_or_huge_text_is_refused() {
		let db = Db::memory();
		let conn = db.conn();
		let p = peer(&conn);
		assert!(insert(&conn, p, "  ", None).is_err());
		assert!(insert(&conn, p, &"x".repeat(MAX_TEXT + 1), None).is_err());
	}

	#[test]
	fn proposals_go_with_their_agent() {
		let db = Db::memory();
		let conn = db.conn();
		let p = peer(&conn);
		insert(&conn, p, "Update Craft", None).unwrap();
		conn.execute("DELETE FROM peers", []).unwrap();
		assert!(list(&conn).unwrap().is_empty());
	}
}
