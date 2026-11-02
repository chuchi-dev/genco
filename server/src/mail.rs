//! Mail goes through the local postfix, which delivers it directly or
//! relays it through an external SMTP server (Settings → Mail). The sites
//! use the same postfix for their mails.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::config::Config;
use crate::db::{Db, set_setting};
use crate::error::{Error, Result};
use crate::users::{self, Authed};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailSettings {
	/// "direct": postfix delivers to the recipients' servers, "relay":
	/// through `relay_host`
	pub mode: String,
	#[serde(default)]
	pub relay_host: String,
	#[serde(default)]
	pub relay_port: u16,
	#[serde(default)]
	pub relay_user: String,
	/// never sent to the browser, an empty one keeps the stored one
	#[serde(default, skip_serializing)]
	pub relay_password: String,
	/// what the browser sees instead of the password
	#[serde(default, skip_deserializing)]
	pub has_password: bool,
}

pub fn settings(db: &Db) -> Result<MailSettings> {
	let mut s: MailSettings =
		crate::db::get_json(&db.conn(), "mail").unwrap_or_default();
	if s.mode.is_empty() {
		s.mode = "direct".into();
	}
	s.has_password = !s.relay_password.is_empty();
	Ok(s)
}

fn sender(cfg: &Config) -> Result<Mailbox> {
	let domain = cfg.domain.as_deref().unwrap_or("localhost");
	format!("genco <genco@{domain}>")
		.parse()
		.map_err(|e| Error::Internal(format!("sender: {e}")))
}

/// Sends a plain text mail through the local postfix.
pub async fn send(
	cfg: &Config,
	to: &[String],
	subject: &str,
	body: &str,
) -> Result<()> {
	let mut msg = Message::builder().from(sender(cfg)?).subject(subject);
	for t in to {
		let mb: Mailbox =
			t.parse().map_err(|e| Error::Invalid(format!("{t}: {e}")))?;
		msg = msg.to(mb);
	}
	let msg = msg
		.body(body.to_string())
		.map_err(|e| Error::Internal(e.to_string()))?;

	let transport =
		AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("127.0.0.1")
			.port(25)
			.build();
	transport
		.send(msg)
		.await
		.map_err(|e| Error::Internal(format!("sending the mail: {e}")))?;
	Ok(())
}

/// Every admin's address.
pub fn admins(db: &Db) -> Result<Vec<String>> {
	Ok(users::list(&db.conn())?
		.into_iter()
		.map(|u| u.email)
		.collect())
}

/// Writes the relay into postfix's config and reloads it.
async fn configure_postfix(s: &MailSettings) -> Result<()> {
	let run = |args: Vec<String>| async move {
		let args: Vec<&str> = args.iter().map(String::as_str).collect();
		crate::util::run(&args)
			.await
			.map(|_| ())
			.map_err(Error::Internal)
	};

	if s.mode == "relay" {
		let relay = format!("[{}]:{}", s.relay_host, s.relay_port);
		let passwd = format!("{relay} {}:{}\n", s.relay_user, s.relay_password);
		// made 0600, never readable for a moment
		{
			use std::io::Write as _;
			use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
			std::fs::OpenOptions::new()
				.write(true)
				.create(true)
				.truncate(true)
				.mode(0o600)
				.open("/etc/postfix/sasl_passwd")
				.and_then(|mut f| {
					f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
					f.write_all(passwd.as_bytes())
				})
				.map_err(|e| Error::Internal(format!("sasl_passwd: {e}")))?;
		}
		run(vec!["postmap".into(), "/etc/postfix/sasl_passwd".into()]).await?;
		run(vec![
			"postconf".into(),
			"-e".into(),
			format!("relayhost = {relay}"),
			"smtp_sasl_auth_enable = yes".into(),
			"smtp_sasl_password_maps = hash:/etc/postfix/sasl_passwd".into(),
			"smtp_sasl_security_options = noanonymous".into(),
			"smtp_tls_security_level = encrypt".into(),
		])
		.await?;
	} else {
		run(vec![
			"postconf".into(),
			"-e".into(),
			"relayhost =".into(),
			"smtp_sasl_auth_enable = no".into(),
			"smtp_tls_security_level = may".into(),
		])
		.await?;
	}
	run(vec!["systemctl".into(), "reload".into(), "postfix".into()]).await
}

async fn get_settings(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<MailSettings>> {
	Ok(Json(settings(&db)?))
}

async fn save_settings(
	_authed: Authed,
	State(db): State<Db>,
	Json(mut new): Json<MailSettings>,
) -> Result<Json<()>> {
	match new.mode.as_str() {
		"direct" => {}
		"relay" => {
			if new.relay_host.trim().is_empty() || new.relay_port == 0 {
				return Err(Error::Invalid(
					"the relay needs a host and a port (587 or 465)".into(),
				));
			}
			if new.relay_password.is_empty() {
				new.relay_password = settings(&db)?.relay_password;
			}
		}
		_ => return Err(Error::Invalid("unknown mode".into())),
	}
	new.relay_host = new.relay_host.trim().into();
	new.relay_user = new.relay_user.trim().into();
	// they go into main.cf and sasl_passwd, one line each
	let host_ok = new
		.relay_host
		.chars()
		.all(|c| c.is_ascii_alphanumeric() || ".-".contains(c));
	let line_ok =
		|v: &str| !v.chars().any(|c| c.is_whitespace() || c.is_control());
	if !host_ok || !line_ok(&new.relay_user) || !line_ok(&new.relay_password) {
		return Err(Error::Invalid(
			"the host is a name like mail.example.ch, user and password \
			have no spaces"
				.into(),
		));
	}

	configure_postfix(&new).await?;
	// with the password, the struct skips it only when serializing for
	// the browser
	let stored = serde_json::json!({
		"mode": new.mode,
		"relayHost": new.relay_host,
		"relayPort": new.relay_port,
		"relayUser": new.relay_user,
		"relayPassword": new.relay_password,
	});
	set_setting(&db.conn(), "mail", &stored.to_string())?;
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
struct TestReq {
	to: String,
}

async fn test_mail(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	Json(req): Json<TestReq>,
) -> Result<Json<()>> {
	send(
		&cfg,
		&[req.to],
		"genco test mail",
		&format!(
			"{} sent this test from genco on {}.\n\nIf it is in the spam \
			folder, use a relay or add SPF and DKIM for the sender's \
			domain.\n",
			authed.user.name,
			cfg.domain.as_deref().unwrap_or("this server")
		),
	)
	.await?;
	Ok(Json(()))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/settings/mail", get(get_settings).post(save_settings))
		.route("/settings/mail/test", post(test_mail))
}
