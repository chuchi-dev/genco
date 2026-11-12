//! A group in quarantine, when it looks compromised (a miner, a web shell,
//! spam): apply stops and disables its services, workers and schedules,
//! its sites answer 503 with a maintenance page (redirects stay), its SMTP
//! login and send-only addresses stop working, and deploys are refused.
//! Its files, databases and mailboxes stay as they are, so the cause can
//! be found and cleaned. Lifting it brings everything back with the next
//! apply. Admins and genco's agent both may set and lift it.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::apply;
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::hosting;
use crate::users::Authed;

/// The longest reason, in characters.
const MAX_REASON: usize = 2000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quarantine {
	pub reason: String,
	/// an admin, `agent`, `terminal`
	pub by: String,
	pub at: i64,
}

/// The quarantine of a group's row, from its columns `reason`, `by`, `at`
/// at `i`.
pub fn from_row(
	row: &rusqlite::Row,
	i: usize,
) -> rusqlite::Result<Option<Quarantine>> {
	let reason: Option<String> = row.get(i)?;
	Ok(match reason {
		Some(reason) => Some(Quarantine {
			reason,
			by: row.get::<_, Option<String>>(i + 1)?.unwrap_or_default(),
			at: row.get::<_, Option<i64>>(i + 2)?.unwrap_or_default(),
		}),
		None => None,
	})
}

/// Puts the group in quarantine; apply does the rest.
pub fn set(
	conn: &Connection,
	group: i64,
	reason: &str,
	by: &str,
) -> Result<()> {
	let reason = reason.trim();
	if reason.is_empty() {
		return Err(Error::Invalid("say why the group is quarantined".into()));
	}
	if reason.chars().count() > MAX_REASON {
		return Err(Error::Invalid(format!(
			"the reason has more than {MAX_REASON} characters"
		)));
	}
	let changed = conn.execute(
		"UPDATE domain_groups SET quarantine_reason = ?1,
		quarantined_by = ?2, quarantined_at = ?3 WHERE id = ?4",
		params![reason, by, now(), group],
	)?;
	if changed != 1 {
		return Err(Error::NotFound);
	}
	Ok(())
}

/// Lifts the quarantine; false when the group wasn't in one.
pub fn lift(conn: &Connection, group: i64) -> Result<bool> {
	let changed = conn.execute(
		"UPDATE domain_groups SET quarantine_reason = NULL,
		quarantined_by = NULL, quarantined_at = NULL
		WHERE id = ?1 AND quarantine_reason IS NOT NULL",
		[group],
	)?;
	Ok(changed == 1)
}

/// A deploy of a group in quarantine is refused.
pub fn check_deploy(group: &hosting::Group) -> std::result::Result<(), String> {
	match &group.quarantine {
		Some(q) => Err(format!(
			"the group {} is in quarantine ({}), lift it first",
			group.name, q.reason
		)),
		None => Ok(()),
	}
}

#[derive(Debug, Deserialize)]
struct SetReq {
	reason: String,
}

async fn set_route(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(req): Json<SetReq>,
) -> Result<Json<apply::Report>> {
	set(&db.conn(), id, &req.reason, &authed.user.name)?;
	Ok(Json(applied(&cfg, &db, &events).await))
}

async fn lift_route(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
) -> Result<Json<apply::Report>> {
	if !lift(&db.conn(), id)? {
		return Err(Error::Invalid("the group isn't in quarantine".into()));
	}
	Ok(Json(applied(&cfg, &db, &events).await))
}

/// The change takes effect at once, the panel shows apply's report.
async fn applied(cfg: &Config, db: &Db, events: &Events) -> apply::Report {
	let report = apply::apply(cfg, db).await;
	events.send(Event::Tree);
	report
}

pub fn routes() -> Router<AppState> {
	Router::new().route(
		"/groups/{id}/quarantine",
		post(set_route).delete(lift_route),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_group_goes_into_quarantine_and_out() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
				VALUES ('shop', 1, 0);",
		)
		.unwrap();
		let group = || hosting::groups(&conn).unwrap().remove(0);
		assert_eq!(group().quarantine, None);
		assert!(check_deploy(&group()).is_ok());

		assert!(set(&conn, 1, " ", "a").is_err());
		assert!(set(&conn, 7, "miner", "a").is_err());
		set(&conn, 1, " a miner in uploads/x.php ", "agent").unwrap();
		let q = group().quarantine.unwrap();
		assert_eq!(
			(q.reason.as_str(), q.by.as_str()),
			("a miner in uploads/x.php", "agent")
		);
		assert!(check_deploy(&group()).unwrap_err().contains("quarantine"));

		assert!(lift(&conn, 1).unwrap());
		assert!(!lift(&conn, 1).unwrap());
		assert_eq!(group().quarantine, None);
	}
}
