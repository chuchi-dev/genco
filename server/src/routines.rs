//! Agent routines on a schedule in the server's local time. A run is a
//! conversation that ends with a report, see `Runner::report`.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::AppState;
use crate::agent::Runner;
use crate::conversations::{self as convs, Conversation, Kind, MessageData};
use crate::db::{Db, get_setting, set_setting};
use crate::error::{Error, Result};
use crate::users::Authed;
use crate::util::sh;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
	pub name: &'static str,
	pub title: &'static str,
	/// local time, `HH:MM`
	pub time: &'static str,
	/// 1 is Monday, `None` every day
	pub weekday: Option<u8>,
	#[serde(skip)]
	prompt: &'static str,
}

const ROUTINES: &[Routine] = &[
	Routine {
		name: "health",
		title: "Daily health check",
		time: "07:00",
		weekday: None,
		prompt: include_str!("agent/routines/health.md"),
	},
	Routine {
		name: "updates",
		title: "Weekly update round",
		time: "07:15",
		weekday: Some(1),
		prompt: include_str!("agent/routines/updates.md"),
	},
	Routine {
		name: "security",
		title: "Weekly security review",
		time: "07:30",
		weekday: Some(1),
		prompt: include_str!("agent/routines/security.md"),
	},
];

/// Starts a run of the routine, returns its conversation.
fn run(db: &Db, runner: &Runner, r: &Routine) -> rusqlite::Result<i64> {
	let text = r.prompt.replace("{{inventory}}", crate::setup::INVENTORY);
	let id = {
		let conn = db.conn();
		let id = convs::create_kind(&conn, r.title, Kind::Routine)?;
		convs::insert_message(&conn, id, MessageData::System { text })?;
		id
	};
	crate::activity::log(&db.conn(), "genco", "Routine", r.title, "", None);
	runner.start_report(id);
	Ok(id)
}

/// Whether it is `r`'s day and its time passed.
fn is_due(r: &Routine, time: &str, weekday: u8) -> bool {
	r.weekday.is_none_or(|d| d == weekday) && time >= r.time
}

/// Called every minute, starts the routines that are due. A routine whose
/// time already passed when genco came first starts the next time.
pub async fn due(db: &Db, runner: &Runner) {
	let Ok(now) = sh("date '+%F %H:%M %u'").await else {
		return;
	};
	let mut parts = now.split(' ');
	let (Some(today), Some(time), Some(weekday)) =
		(parts.next(), parts.next(), parts.next())
	else {
		return;
	};
	let weekday: u8 = weekday.parse().unwrap_or(0);

	for r in ROUTINES {
		let key = format!("routine:{}", r.name);
		let last = get_setting(&db.conn(), &key).ok().flatten();
		let due = is_due(r, time, weekday);
		// the first minute after the install: a routine whose time passed
		// today starts next time, a later one today
		if last.is_none() {
			let mark = if due { today } else { "waiting" };
			if let Err(e) = set_setting(&db.conn(), &key, mark) {
				tracing::error!("routine {}: {e}", r.name);
			}
			continue;
		}
		if last.as_deref() == Some(today) || !due {
			continue;
		}
		if let Err(e) = set_setting(&db.conn(), &key, today) {
			tracing::error!("routine {}: {e}", r.name);
			continue;
		}

		tracing::info!("starting the routine {}", r.name);
		if let Err(e) = run(db, runner, r) {
			tracing::error!("routine {}: {e}", r.name);
		}
	}
}

async fn list(_authed: Authed) -> Json<&'static [Routine]> {
	Json(ROUTINES)
}

/// Runs a routine now, besides its schedule.
async fn run_now(
	_authed: Authed,
	State(db): State<Db>,
	State(runner): State<Runner>,
	Path(name): Path<String>,
) -> Result<Json<Conversation>> {
	let r = ROUTINES
		.iter()
		.find(|r| r.name == name)
		.ok_or(Error::NotFound)?;
	let id = run(&db, &runner, r)?;
	Ok(Json(convs::get(&db.conn(), id)?))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/routines", get(list))
		.route("/routines/{name}/run", post(run_now))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn weekly_routine_only_on_its_day_after_its_time() {
		let weekly = ROUTINES.iter().find(|r| r.name == "security").unwrap();
		assert!(!is_due(weekly, "07:29", 1));
		assert!(is_due(weekly, "07:30", 1));
		assert!(!is_due(weekly, "09:00", 2));
		assert!(is_due(&ROUTINES[0], "23:59", 2));
	}
}
