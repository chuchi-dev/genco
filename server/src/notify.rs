//! Where genco's reports go: a mail to the admins, a webhook, or both
//! (Settings → Notifications). A webhook gets a JSON POST with a bearer
//! token, for example to another agent that looks after the server.
//! Deletion confirmations and invites always go by mail, a person acts
//! on them.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::config::Config;
use crate::db::{Db, set_setting};
use crate::error::{Error, Result};
use crate::users::Authed;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifySettings {
	/// reports that need attention go to every admin by mail
	#[serde(default = "yes")]
	pub mail: bool,
	/// empty for none, https only
	#[serde(default)]
	pub webhook_url: String,
	/// sent as `Authorization: Bearer`, never to the browser; an empty one
	/// keeps the stored one
	#[serde(default, skip_serializing)]
	pub webhook_token: String,
	#[serde(default, skip_deserializing)]
	pub has_token: bool,
	/// the webhook gets the OK reports too, not only the ones that need
	/// attention
	#[serde(default)]
	pub webhook_all: bool,
}

fn yes() -> bool {
	true
}

impl Default for NotifySettings {
	fn default() -> Self {
		Self {
			mail: true,
			webhook_url: String::new(),
			webhook_token: String::new(),
			has_token: false,
			webhook_all: false,
		}
	}
}

fn settings(db: &Db) -> Result<NotifySettings> {
	let mut s: NotifySettings =
		crate::db::get_json(&db.conn(), "notify").unwrap_or_default();
	s.has_token = !s.webhook_token.is_empty();
	Ok(s)
}

/// What happened, for the mail and the webhook alike.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
	/// "report" (routines and alerts), "update", "test"
	pub kind: &'static str,
	pub attention: bool,
	pub title: String,
	pub text: String,
	/// where to look at it in the panel
	pub url: Option<String>,
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

/// Sends `n` where the settings say; errors are logged, a report must not
/// fail because a webhook is down.
pub async fn send(cfg: &Config, db: &Db, n: Notice) {
	let s = match settings(db) {
		Ok(s) => s,
		Err(e) => {
			tracing::error!("notification settings: {e}");
			NotifySettings::default()
		}
	};
	if n.attention
		&& s.mail
		&& let Err(e) = mail(cfg, db, &n).await
	{
		tracing::error!("mail about {}: {e}", n.title);
	}
	if !s.webhook_url.is_empty()
		&& (n.attention || s.webhook_all)
		&& let Err(e) = webhook(cfg, &s, &n).await
	{
		tracing::error!("webhook about {}: {e}", n.title);
	}
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
	crate::mail::send(cfg, &to, &subject, &body).await
}

async fn webhook(cfg: &Config, s: &NotifySettings, n: &Notice) -> Result<()> {
	let payload = Payload {
		server: cfg.domain.as_deref().unwrap_or("localhost"),
		status: if n.attention { "attention" } else { "ok" },
		notice: n,
	};
	let mut req = reqwest::Client::new()
		.post(&s.webhook_url)
		.timeout(Duration::from_secs(20))
		.json(&payload);
	if !s.webhook_token.is_empty() {
		req = req.bearer_auth(&s.webhook_token);
	}
	let res = req
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

fn check_url(url: &str) -> Result<()> {
	let ok = url.is_empty()
		|| (url.starts_with("https://")
			&& url.len() > "https://".len()
			&& !url.chars().any(|c| c.is_whitespace() || c.is_control()));
	if !ok {
		return Err(Error::Invalid(
			"the webhook is an https:// address".into(),
		));
	}
	Ok(())
}

async fn get_settings(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<NotifySettings>> {
	Ok(Json(settings(&db)?))
}

async fn save_settings(
	_authed: Authed,
	State(db): State<Db>,
	Json(mut new): Json<NotifySettings>,
) -> Result<Json<()>> {
	new.webhook_url = new.webhook_url.trim().into();
	check_url(&new.webhook_url)?;
	let old = settings(&db)?;
	if new.webhook_token.is_empty() && new.webhook_url == old.webhook_url {
		new.webhook_token = old.webhook_token;
	}
	if new
		.webhook_token
		.chars()
		.any(|c| c.is_whitespace() || c.is_control())
	{
		return Err(Error::Invalid("the token has no spaces".into()));
	}
	// with the token, the struct skips it only when serializing for the
	// browser
	let stored = serde_json::json!({
		"mail": new.mail,
		"webhookUrl": new.webhook_url,
		"webhookToken": new.webhook_token,
		"webhookAll": new.webhook_all,
	});
	set_setting(&db.conn(), "notify", &stored.to_string())?;
	Ok(Json(()))
}

/// A test to the webhook, its error goes back to the browser.
async fn test_webhook(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
) -> Result<Json<()>> {
	let s = settings(&db)?;
	if s.webhook_url.is_empty() {
		return Err(Error::Invalid("no webhook saved".into()));
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
	};
	webhook(&cfg, &s, &n).await?;
	Ok(Json(()))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/settings/notify", get(get_settings).post(save_settings))
		.route("/settings/notify/test", post(test_webhook))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn webhooks_only_over_https() {
		assert!(check_url("").is_ok());
		assert!(check_url("https://noah.example/api/x").is_ok());
		assert!(check_url("http://noah.example/api/x").is_err());
		assert!(check_url("https://").is_err());
		assert!(check_url("https://a b").is_err());
	}

	#[test]
	fn mail_is_on_without_settings() {
		let db = Db::memory();
		let s = settings(&db).unwrap();
		assert!(s.mail);
		assert!(s.webhook_url.is_empty());
	}

	#[test]
	fn the_token_never_goes_to_the_browser() {
		let s = NotifySettings {
			webhook_token: "secret".into(),
			has_token: true,
			..Default::default()
		};
		let json = serde_json::to_string(&s).unwrap();
		assert!(!json.contains("secret"));
		assert!(json.contains("\"hasToken\":true"));
	}
}
