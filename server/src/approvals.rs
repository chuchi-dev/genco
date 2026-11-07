//! Start → Approvals: what waits for an admin in one list, the AI agents'
//! proposals (proposals.rs, "#12") and the pending actions (actions.rs,
//! "A12": deletions, the agent's remote backup changes), newest first,
//! with the decided and expired ones of the last 30 days below. Both
//! confirm in the panel the same way: the 2FA code
//! (`users::verify_approval`).

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::AppState;
use crate::actions::{self, Action};
use crate::db::Db;
use crate::error::Result;
use crate::proposals::{self, Proposal};
use crate::users::Authed;

/// How long the decided ones stay in the list.
const HISTORY_DAYS: i64 = 30;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Approval {
	Proposal { proposal: Proposal },
	Action { action: Action },
}

impl Approval {
	fn created_at(&self) -> i64 {
		match self {
			Self::Proposal { proposal } => proposal.created_at,
			Self::Action { action } => action.created_at,
		}
	}
}

/// Both kinds, newest first.
fn merge(proposals: Vec<Proposal>, actions: Vec<Action>) -> Vec<Approval> {
	let mut list: Vec<Approval> = proposals
		.into_iter()
		.map(|proposal| Approval::Proposal { proposal })
		.chain(
			actions
				.into_iter()
				.map(|action| Approval::Action { action }),
		)
		.collect();
	list.sort_by_key(|a| std::cmp::Reverse(a.created_at()));
	list
}

async fn list(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<Approval>>> {
	let (proposals, mut actions) = {
		let conn = db.conn();
		(
			proposals::recent(&conn, HISTORY_DAYS)?,
			actions::recent(&conn, HISTORY_DAYS)?,
		)
	};
	actions::add_sizes(&db, &mut actions).await;
	Ok(Json(merge(proposals, actions)))
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/approvals", get(list))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn both_kinds_in_one_list_newest_first() {
		let db = Db::memory();
		let conn = db.conn();
		let day = 24 * 60 * 60 * 1000;
		let now = crate::db::now();
		conn.execute(
			"INSERT INTO peers (name, key, key_hash, created_at)
			VALUES ('Corleone', 'k', 'h', 0)",
			[],
		)
		.unwrap();
		// a proposal of yesterday, one decided 40 days ago, an action of
		// today and one that waited long ago
		for (status, at) in
			[("pending", now - day), ("rejected", now - 40 * day)]
		{
			conn.execute(
				"INSERT INTO proposals (peer_id, text, status, created_at,
				removed_chars) VALUES (1, 'Update Craft', ?1, ?2, 0)",
				rusqlite::params![status, at],
			)
			.unwrap();
		}
		for (token, at) in [("a", now), ("b", now - 40 * day)] {
			conn.execute(
				"INSERT INTO pending_actions (kind, target_id, description,
				token_hash, requested_by, status, created_at)
				VALUES ('delete_domain', 1, 'x', ?1, 'a', 'pending', ?2)",
				rusqlite::params![token, at],
			)
			.unwrap();
		}

		let list = merge(
			proposals::recent(&conn, HISTORY_DAYS).unwrap(),
			actions::recent(&conn, HISTORY_DAYS).unwrap(),
		);
		let json = serde_json::to_value(&list).unwrap();
		let got: Vec<(String, String, String)> = json
			.as_array()
			.unwrap()
			.iter()
			.map(|a| {
				let kind = a["kind"].as_str().unwrap().to_string();
				let item = &a[kind.as_str()];
				let n = item["number"]
					.as_str()
					.map(String::from)
					.unwrap_or_else(|| format!("#{}", item["id"]));
				(kind, n, item["status"].as_str().unwrap().into())
			})
			.collect();
		// the old action still waits in the table, it shows as expired
		assert_eq!(
			got,
			[
				("action".into(), "A1".into(), "pending".into()),
				("proposal".into(), "#1".into(), "pending".into()),
				("action".into(), "A2".into(), "expired".into()),
			]
		);
	}
}
