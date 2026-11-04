//! Where genco's reports go: a mail to the admins and the webhooks of the
//! AI agents (peers.rs). Deletion confirmations and invites always go by
//! mail, a person acts on them. With mail off only the webhooks and the
//! panel get them.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::config::Config;
use crate::db::{Db, set_setting};
use crate::error::Result;
use crate::users::Authed;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifySettings {
	/// reports that need attention go to every admin by mail
	#[serde(default = "crate::util::yes")]
	pub mail: bool,
}

impl Default for NotifySettings {
	fn default() -> Self {
		Self { mail: true }
	}
}

fn settings(db: &Db) -> NotifySettings {
	crate::db::get_json(&db.conn(), "notify").unwrap_or_default()
}

/// What happened, for the mail and the webhooks alike.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
	/// "report" (routines and alerts), "update", "proposal", "test"
	pub kind: &'static str,
	pub attention: bool,
	pub title: String,
	pub text: String,
	/// where to look at it in the panel
	pub url: Option<String>,
	/// the proposal a "proposal" notice answers
	#[serde(skip_serializing_if = "Option::is_none")]
	pub proposal: Option<i64>,
}

/// Sends `n` to the admins and the agents; errors are logged, a report
/// must not fail because a webhook is down.
pub async fn send(cfg: &Config, db: &Db, n: Notice) {
	if n.attention
		&& settings(db).mail
		&& !crate::mail::off(db)
		&& let Err(e) = mail(cfg, db, &n).await
	{
		tracing::error!("mail about {}: {e}", n.title);
	}
	crate::peers::notify(cfg, db, &n).await;
}

async fn mail(cfg: &Config, db: &Db, n: &Notice) -> Result<()> {
	let to = crate::mail::admins(db)?;
	if to.is_empty() {
		return Ok(());
	}
	let mut body = n.text.clone();
	if let Some(url) = &n.url {
		body.push_str(&format!("\n\n{url}\n"));
	}
	let subject = match n.kind {
		"update" => format!("genco: {}", n.title),
		_ => format!("genco needs attention: {}", n.title),
	};
	crate::mail::send(cfg, db, &to, &subject, &body).await
}

async fn get_settings(
	_authed: Authed,
	State(db): State<Db>,
) -> Json<NotifySettings> {
	Json(settings(&db))
}

async fn save_settings(
	_authed: Authed,
	State(db): State<Db>,
	Json(new): Json<NotifySettings>,
) -> Result<Json<()>> {
	set_setting(&db.conn(), "notify", &serde_json::to_string(&new).unwrap())?;
	Ok(Json(()))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/settings/notify", get(get_settings).post(save_settings))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn mail_is_on_without_settings() {
		assert!(settings(&Db::memory()).mail);
	}
}
