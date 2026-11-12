//! Conversations with the maintenance agent and their messages.

use axum::extract::{Path, Query, State};
use axum::routing::{self, post};
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::sync::Arc;

use crate::AppState;
use crate::agent::Runner;
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::users::Authed;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
	Idle,
	Running,
	Failed,
}

impl Status {
	fn as_str(self) -> &'static str {
		match self {
			Self::Idle => "idle",
			Self::Running => "running",
			Self::Failed => "failed",
		}
	}

	fn parse(s: &str) -> Self {
		match s {
			"running" => Self::Running,
			"failed" => Self::Failed,
			_ => Self::Idle,
		}
	}
}

/// Conversations of admins, routines on a schedule and alerts of the
/// minute check. The last two end with a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
	Chat,
	Routine,
	Alert,
}

impl Kind {
	fn as_str(self) -> &'static str {
		match self {
			Self::Chat => "chat",
			Self::Routine => "routine",
			Self::Alert => "alert",
		}
	}

	fn parse(s: &str) -> Self {
		match s {
			"routine" => Self::Routine,
			"alert" => Self::Alert,
			_ => Self::Chat,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Report {
	Ok,
	Attention,
}

impl Report {
	fn as_str(self) -> &'static str {
		match self {
			Self::Ok => "ok",
			Self::Attention => "attention",
		}
	}

	fn parse(s: &str) -> Option<Self> {
		match s {
			"ok" => Some(Self::Ok),
			"attention" => Some(Self::Attention),
			_ => None,
		}
	}

	/// The last `STATUS: OK` or `STATUS: ATTENTION` line of a text.
	pub fn find(text: &str) -> Option<Self> {
		text.lines().rev().find_map(|l| {
			match l.trim().trim_matches('*').trim() {
				"STATUS: OK" => Some(Self::Ok),
				"STATUS: ATTENTION" => Some(Self::Attention),
				_ => None,
			}
		})
	}
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
	pub id: i64,
	pub title: String,
	pub status: Status,
	pub kind: Kind,
	/// how the last run of a routine or alert ended
	pub report: Option<Report>,
	/// the last message the agent has read
	pub seen: i64,
	pub created_at: i64,
	pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MessageData {
	#[serde(rename_all = "camelCase")]
	User {
		text: String,
		/// who wrote it, `None` from the terminal
		#[serde(default, skip_serializing_if = "Option::is_none")]
		user_name: Option<String>,
		/// attached files, see attachments.rs
		#[serde(default, skip_serializing_if = "Vec::is_empty")]
		files: Vec<crate::attachments::Attachment>,
	},
	/// Messages genco gives the agent, for example after a restart.
	System {
		text: String,
	},
	Assistant {
		text: String,
	},
	#[serde(rename_all = "camelCase")]
	ToolCall {
		call_id: String,
		name: String,
		input: Value,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		context: Option<String>,
	},
	#[serde(rename_all = "camelCase")]
	ToolOutput {
		call_id: String,
		output: String,
		is_error: bool,
	},
	Error {
		text: String,
	},
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
	pub id: i64,
	pub conversation_id: i64,
	pub created_at: i64,
	#[serde(flatten)]
	pub data: MessageData,
}

const COLUMNS: &str =
	"id, title, status, seen, created_at, updated_at, kind, report";

impl Conversation {
	fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
		Ok(Self {
			id: row.get(0)?,
			title: row.get(1)?,
			status: Status::parse(&row.get::<_, String>(2)?),
			seen: row.get(3)?,
			created_at: row.get(4)?,
			updated_at: row.get(5)?,
			kind: Kind::parse(&row.get::<_, String>(6)?),
			report: row
				.get::<_, Option<String>>(7)?
				.and_then(|r| Report::parse(&r)),
		})
	}
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Conversation> {
	conn.query_row(
		&format!("SELECT {COLUMNS} FROM conversations WHERE id = ?1"),
		[id],
		Conversation::from_row,
	)
}

pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Conversation>> {
	let mut stmt = conn.prepare(&format!(
		"SELECT {COLUMNS} FROM conversations ORDER BY updated_at DESC"
	))?;
	stmt.query_map([], Conversation::from_row)?.collect()
}

pub fn with_status(
	conn: &Connection,
	status: Status,
) -> rusqlite::Result<Vec<i64>> {
	let mut stmt =
		conn.prepare("SELECT id FROM conversations WHERE status = ?1")?;
	stmt.query_map([status.as_str()], |row| row.get(0))?
		.collect()
}

/// The title is the start of the first message.
pub fn create(conn: &Connection, first: &str) -> rusqlite::Result<i64> {
	create_kind(conn, first, Kind::Chat)
}

pub fn create_kind(
	conn: &Connection,
	first: &str,
	kind: Kind,
) -> rusqlite::Result<i64> {
	let title = crate::util::first_line(first, 60);
	conn.execute(
		"INSERT INTO conversations (title, status, kind, created_at, updated_at)
		VALUES (?1, 'idle', ?2, ?3, ?3)",
		params![title, kind.as_str(), now()],
	)?;
	Ok(conn.last_insert_rowid())
}

pub fn set_report(
	conn: &Connection,
	id: i64,
	report: Report,
) -> rusqlite::Result<Conversation> {
	conn.execute(
		"UPDATE conversations SET report = ?1 WHERE id = ?2",
		params![report.as_str(), id],
	)?;
	get(conn, id)
}

/// A routine or alert that needed attention doesn't any more: its report
/// turns OK and a system message says why.
pub fn resolve_by_genco(
	conn: &Connection,
	id: i64,
	why: &str,
) -> rusqlite::Result<(Conversation, Message)> {
	let message = insert_message(
		conn,
		id,
		MessageData::System {
			text: format!("Resolved by genco: {why}"),
		},
	)?;
	Ok((set_report(conn, id, Report::Ok)?, message))
}

/// `genco attention`: the routines and alerts that still need attention,
/// genco's agent goes over them in the daily health check.
#[derive(Debug, clap::Parser)]
pub enum AttentionCmd {
	/// List them with their last report, oldest first
	List,
	/// Clear one that is taken care of; the reason goes into it
	Resolve {
		id: i64,
		/// why it needs no admin any more
		#[clap(long)]
		why: String,
	},
}

/// The longest report `genco attention list` prints, in characters.
const LIST_REPORT: usize = 600;

pub fn attention(conn: &Connection, cmd: AttentionCmd) -> Result<String> {
	match cmd {
		AttentionCmd::List => {
			let mut out = String::new();
			for c in list(conn)?
				.into_iter()
				.filter(|c| c.report == Some(Report::Attention))
				.rev()
			{
				let report = messages(conn, c.id)?
					.into_iter()
					.rev()
					.find_map(|m| match m.data {
						MessageData::Assistant { text } => Some(text),
						_ => None,
					})
					.unwrap_or_default();
				let report: String = report.chars().take(LIST_REPORT).collect();
				let kind = format!("{:?}", c.kind).to_lowercase();
				out.push_str(&format!(
					"#{} {kind}: {}\n{}\n\n",
					c.id,
					c.title,
					report.trim()
				));
			}
			if out.is_empty() {
				out.push_str("nothing needs attention");
			}
			Ok(out.trim_end().into())
		}
		AttentionCmd::Resolve { id, why } => {
			let why = why.trim();
			if why.is_empty() {
				return Err(Error::Invalid("say why with --why".into()));
			}
			let c = get(conn, id)?;
			if c.report != Some(Report::Attention) {
				return Err(Error::Invalid(format!(
					"#{id} doesn't need attention"
				)));
			}
			resolve_by_genco(conn, id, why)?;
			Ok(format!("resolved #{id} {}", c.title))
		}
	}
}

pub fn set_status(
	conn: &Connection,
	id: i64,
	status: Status,
) -> rusqlite::Result<Conversation> {
	conn.execute(
		"UPDATE conversations SET status = ?1, updated_at = ?2 WHERE id = ?3",
		params![status.as_str(), now(), id],
	)?;
	get(conn, id)
}

pub fn set_seen(conn: &Connection, id: i64, seen: i64) -> rusqlite::Result<()> {
	conn.execute(
		"UPDATE conversations SET seen = ?1 WHERE id = ?2",
		params![seen, id],
	)?;
	Ok(())
}

pub fn insert_message(
	conn: &Connection,
	conversation_id: i64,
	data: MessageData,
) -> rusqlite::Result<Message> {
	let created_at = now();
	conn.execute(
		"INSERT INTO messages (conversation_id, data, created_at)
		VALUES (?1, ?2, ?3)",
		params![
			conversation_id,
			serde_json::to_string(&data).unwrap(),
			created_at
		],
	)?;
	conn.execute(
		"UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
		params![created_at, conversation_id],
	)?;

	Ok(Message {
		id: conn.last_insert_rowid(),
		conversation_id,
		created_at,
		data,
	})
}

/// Whether an admin or genco wrote after message `after`.
pub fn has_input_after(
	conn: &Connection,
	conversation_id: i64,
	after: i64,
) -> rusqlite::Result<bool> {
	conn.query_row(
		"SELECT EXISTS (SELECT 1 FROM messages
		WHERE conversation_id = ?1 AND id > ?2
		AND json_extract(data, '$.kind') IN ('user', 'system'))",
		params![conversation_id, after],
		|r| r.get(0),
	)
}

pub fn messages(
	conn: &Connection,
	conversation_id: i64,
) -> rusqlite::Result<Vec<Message>> {
	let mut stmt = conn.prepare(
		"SELECT id, data, created_at FROM messages
		WHERE conversation_id = ?1 ORDER BY id",
	)?;
	stmt.query_map([conversation_id], |row| {
		let data: String = row.get(1)?;
		Ok(Message {
			id: row.get(0)?,
			conversation_id,
			created_at: row.get(2)?,
			data: serde_json::from_str(&data).unwrap_or(MessageData::Error {
				text: "unreadable message".into(),
			}),
		})
	})?
	.collect()
}

async fn list_conversations(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<Conversation>>> {
	Ok(Json(list(&db.conn())?))
}

#[derive(Debug, Serialize)]
struct ConversationResp {
	conversation: Conversation,
	messages: Vec<Message>,
}

async fn get_conversation(
	_authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<ConversationResp>> {
	let conn = db.conn();
	Ok(Json(ConversationResp {
		conversation: get(&conn, id)?,
		messages: messages(&conn, id)?,
	}))
}

#[derive(Debug, Deserialize)]
struct SendReq {
	#[serde(default)]
	text: String,
	/// ids of uploaded attachments
	#[serde(default)]
	files: Vec<String>,
}

impl SendReq {
	/// A message has text or files.
	fn check(&self) -> Result<()> {
		if self.text.trim().is_empty() && self.files.is_empty() {
			return Err(Error::Invalid("empty message".into()));
		}
		Ok(())
	}
}

/// Starts a new conversation with the first message.
async fn create_conversation(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	State(runner): State<Runner>,
	Json(req): Json<SendReq>,
) -> Result<Json<Conversation>> {
	req.check()?;
	let files = crate::attachments::resolve(&cfg, &req.files)?;
	// a message with only files is titled after them
	let title = if req.text.trim().is_empty() {
		files
			.iter()
			.map(|f| f.name.as_str())
			.collect::<Vec<_>>()
			.join(", ")
	} else {
		req.text.clone()
	};
	let id = create(&db.conn(), &title)?;
	send_message(&db, &events, id, req.text, files, authed.user.name)?;
	runner.start(id);
	Ok(Json(get(&db.conn(), id)?))
}

async fn post_message(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	State(runner): State<Runner>,
	Path(id): Path<i64>,
	Json(req): Json<SendReq>,
) -> Result<Json<()>> {
	req.check()?;
	let files = crate::attachments::resolve(&cfg, &req.files)?;
	get(&db.conn(), id)?;
	send_message(&db, &events, id, req.text, files, authed.user.name)?;
	runner.start(id);
	Ok(Json(()))
}

fn send_message(
	db: &Db,
	events: &Events,
	id: i64,
	text: String,
	files: Vec<crate::attachments::Attachment>,
	user_name: String,
) -> Result<()> {
	let message = insert_message(
		&db.conn(),
		id,
		MessageData::User {
			text,
			user_name: Some(user_name),
			files,
		},
	)?;
	events.send(Event::Message { message });
	Ok(())
}

async fn stop(
	_authed: Authed,
	State(runner): State<Runner>,
	Path(id): Path<i64>,
) -> Json<()> {
	runner.stop(id);
	Json(())
}

/// An admin took care of a routine or alert that needed attention.
async fn resolve(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	let conversation = set_report(&db.conn(), id, Report::Ok)?;
	events.send(Event::Conversation { conversation });
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
	q: String,
}

/// Conversations whose title or messages contain the text, newest first.
fn search(conn: &Connection, q: &str) -> rusqlite::Result<Vec<i64>> {
	let mut stmt = conn.prepare(
		"SELECT c.id FROM conversations c WHERE instr(lower(c.title), ?1)
		OR EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id = c.id
			AND instr(lower(json_extract(m.data, '$.text')), ?1))
		ORDER BY c.updated_at DESC LIMIT 100",
	)?;
	stmt.query_map([q.trim().to_lowercase()], |r| r.get(0))?
		.collect()
}

async fn search_route(
	_authed: Authed,
	State(db): State<Db>,
	Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<i64>>> {
	if q.q.trim().is_empty() {
		return Ok(Json(vec![]));
	}
	Ok(Json(search(&db.conn(), &q.q)?))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route(
			"/",
			routing::get(list_conversations).post(create_conversation),
		)
		.route("/search", routing::get(search_route))
		.route("/{id}", routing::get(get_conversation))
		.route("/{id}/messages", post(post_message))
		.route("/{id}/stop", post(stop))
		.route("/{id}/resolve", post(resolve))
		.merge(crate::attachments::routes())
}

#[cfg(test)]
mod tests {
	#[test]
	fn genco_attention_lists_and_resolves() {
		let db = Db::memory();
		let conn = db.conn();
		let id = create_kind(&conn, "Alert: x failed", Kind::Alert).unwrap();
		set_report(&conn, id, Report::Attention).unwrap();
		insert_message(
			&conn,
			id,
			MessageData::Assistant {
				text: "x is down\n\nSTATUS: ATTENTION".into(),
			},
		)
		.unwrap();
		let list = attention(&conn, AttentionCmd::List).unwrap();
		assert!(list.starts_with("#1 alert: Alert: x failed\nx is down"));

		let no_why = AttentionCmd::Resolve {
			id,
			why: " ".into(),
		};
		assert!(attention(&conn, no_why).is_err());
		let why = AttentionCmd::Resolve {
			id,
			why: "x runs again".into(),
		};
		attention(&conn, why).unwrap();
		assert_eq!(get(&conn, id).unwrap().report, Some(Report::Ok));
		assert_eq!(
			attention(&conn, AttentionCmd::List).unwrap(),
			"nothing needs attention"
		);
	}

	use super::*;

	#[test]
	fn input_after_a_message() {
		let db = Db::memory();
		let conn = db.conn();
		let id = create(&conn, "a").unwrap();
		let first = insert_message(
			&conn,
			id,
			MessageData::Assistant { text: "x".into() },
		)
		.unwrap();
		assert!(!has_input_after(&conn, id, 0).unwrap());

		insert_message(&conn, id, MessageData::System { text: "y".into() })
			.unwrap();
		assert!(has_input_after(&conn, id, first.id).unwrap());
	}

	#[test]
	fn report_is_the_last_status_line() {
		assert_eq!(Report::find("all fine\nSTATUS: OK"), Some(Report::Ok));
		assert_eq!(
			Report::find("STATUS: OK\nthen\n**STATUS: ATTENTION**"),
			Some(Report::Attention)
		);
		assert_eq!(Report::find("the STATUS: OK is inline"), None);
	}

	#[test]
	fn search_finds_titles_and_message_texts() {
		let db = Db::memory();
		let conn = db.conn();
		let a = create(&conn, "Set up shop").unwrap();
		let b = create(&conn, "Something else").unwrap();
		insert_message(
			&conn,
			b,
			MessageData::Assistant {
				text: "The SHOP deploy works".into(),
			},
		)
		.unwrap();
		create(&conn, "Unrelated").unwrap();

		let mut found = search(&conn, "Shop").unwrap();
		found.sort();
		assert_eq!(found, [a, b]);
	}

	#[test]
	fn long_first_message_is_cut_for_the_title() {
		let db = Db::memory();
		let conn = db.conn();
		let id = create(&conn, &"x".repeat(100)).unwrap();
		assert_eq!(get(&conn, id).unwrap().title.chars().count(), 60);
	}
}
