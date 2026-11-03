//! Mail goes through the local postfix, which delivers it directly or
//! relays it through an external SMTP server (Settings → Mail). The sites
//! use the same postfix for their mails. With mail off (port 25 blocked
//! and no relay) genco sends none: deletions are confirmed with the
//! password and reports only show in the panel.

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
	/// through `relay_host`, "off": genco sends no mail
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
	/// From and envelope sender of genco's own mails through the relay,
	/// which often only sends as its account; empty is genco@<panel>
	#[serde(default)]
	pub sender: String,
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

/// Whether genco sends no mail at all.
pub fn off(db: &Db) -> bool {
	is_off(&db.conn())
}

pub fn is_off(conn: &rusqlite::Connection) -> bool {
	crate::db::get_json::<MailSettings>(conn, "mail")
		.is_some_and(|s| s.mode == "off")
}

/// The error of what needs a mail while mail is off.
pub fn off_error(what: &str) -> Error {
	Error::Invalid(format!("mail is off in Settings → Mail, {what}"))
}

/// genco's From, also the envelope sender bounces go to: the relay's
/// sender if set, else genco@<panel>.
fn sender(domain: Option<&str>, s: &MailSettings) -> Result<Mailbox> {
	let address = if s.mode == "relay" && !s.sender.is_empty() {
		s.sender.clone()
	} else {
		format!("genco@{}", domain.unwrap_or("localhost"))
	};
	format!("genco <{address}>")
		.parse()
		.map_err(|e| Error::Internal(format!("sender: {e}")))
}

/// The domain genco signs as: only the panel's own, its key is for no
/// other.
fn signs_as<'a>(domain: Option<&'a str>, from: &Mailbox) -> Option<&'a str> {
	domain.filter(|d| from.email.domain().eq_ignore_ascii_case(d))
}

/// Sends a plain text mail through the local postfix.
pub async fn send(
	cfg: &Config,
	db: &Db,
	to: &[String],
	subject: &str,
	body: &str,
) -> Result<()> {
	queue(cfg, db, to, subject, body).await.map(|_| ())
}

/// Hands the mail to postfix, returns its queue id if postfix said it.
async fn queue(
	cfg: &Config,
	db: &Db,
	to: &[String],
	subject: &str,
	body: &str,
) -> Result<Option<String>> {
	let s = settings(db)?;
	if s.mode == "off" {
		return Err(off_error("genco sends no mail"));
	}
	let from = sender(cfg.domain.as_deref(), &s)?;
	let signer = signs_as(cfg.domain.as_deref(), &from).map(str::to_string);
	let mut msg = Message::builder().from(from).subject(subject);
	for t in to {
		let mb: Mailbox =
			t.parse().map_err(|e| Error::Invalid(format!("{t}: {e}")))?;
		msg = msg.to(mb);
	}
	let mut msg = msg
		.body(body.to_string())
		.map_err(|e| Error::Internal(e.to_string()))?;
	if let Some(domain) = signer {
		sign(&domain, &mut msg);
	}

	let transport =
		AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("127.0.0.1")
			.port(25)
			.build();
	let reply = transport
		.send(msg)
		.await
		.map_err(|e| Error::Internal(format!("sending the mail: {e}")))?;
	Ok(queue_id(&reply.message().collect::<Vec<_>>().join(" ")))
}

/// postfix's "250 2.0.0 Ok: queued as 4dXy1z2AbC".
fn queue_id(reply: &str) -> Option<String> {
	let (_, rest) = reply.split_once("queued as ")?;
	let id: String = rest
		.chars()
		.take_while(|c| c.is_ascii_alphanumeric())
		.collect();
	(!id.is_empty()).then_some(id)
}

#[derive(Debug, PartialEq)]
enum Delivery {
	/// the receiving server or the relay took it
	Sent(String),
	/// postfix tries again later, e.g. the relay's login failed
	Deferred(String),
	Bounced(String),
}

/// What became of the mail `id`, from postfix's log lines and its queue
/// (`postqueue -j`); None while it is still on the way.
fn delivery(id: &str, log: &str, queue: &str) -> Option<Delivery> {
	let ours = format!("{id}: ");
	// the reason in brackets after the status
	let reason = |line: &str, status: &str| {
		let after = line.split_once(status)?.1.trim();
		Some(
			after
				.strip_prefix('(')
				.and_then(|r| r.strip_suffix(')'))
				.unwrap_or(after)
				.to_string(),
		)
	};
	for line in log.lines().rev().filter(|l| l.contains(&ours)) {
		if let Some(r) = reason(line, "status=sent") {
			return Some(Delivery::Sent(r));
		}
		if let Some(r) = reason(line, "status=bounced") {
			return Some(Delivery::Bounced(r));
		}
		if let Some(r) = reason(line, "status=deferred") {
			return Some(Delivery::Deferred(r));
		}
	}
	queue
		.lines()
		.filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
		.find(|m| m["queue_id"] == id && m["queue_name"] == "deferred")
		.map(|m| {
			let reason = m["recipients"][0]["delay_reason"]
				.as_str()
				.unwrap_or("no reason given");
			Delivery::Deferred(reason.to_string())
		})
}

/// Waits up to 15 s for postfix to deliver the mail or give its reason.
async fn wait_for_delivery(id: &str) -> Option<Delivery> {
	for _ in 0..15 {
		tokio::time::sleep(std::time::Duration::from_secs(1)).await;
		let log = crate::util::run(&[
			"journalctl",
			"-q",
			"--no-pager",
			"-o",
			"cat",
			"--since",
			"-5min",
		])
		.await
		.unwrap_or_default();
		let queue = crate::util::run(&["postqueue", "-j"])
			.await
			.unwrap_or_default();
		if let Some(d) = delivery(id, &log, &queue) {
			return Some(d);
		}
	}
	None
}

/// Mail servers that always listen on 25: if neither answers, the host
/// blocks outgoing mail.
const MX_HOSTS: [&str; 2] =
	["gmail-smtp-in.l.google.com", "mail.protonmail.ch"];

async fn reaches(host: &str) -> bool {
	let connect = tokio::net::TcpStream::connect((host, 25));
	matches!(
		tokio::time::timeout(std::time::Duration::from_secs(5), connect).await,
		Ok(Ok(_))
	)
}

/// Whether outgoing port 25 is blocked (common at cloud hosts).
pub async fn port25_blocked() -> bool {
	let (a, b) = tokio::join!(reaches(MX_HOSTS[0]), reaches(MX_HOSTS[1]));
	!a && !b
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct Port25 {
	blocked: bool,
	checked_at: i64,
}

/// the last check, it takes up to 5 s
static PORT25: std::sync::Mutex<Option<Port25>> = std::sync::Mutex::new(None);

/// genco's own key: postfix signs only mail with a login, genco's comes
/// over localhost:25 without one. Its own selector, the panel's domain
/// may be a mail domain with a key of its own too.
pub const PANEL_SELECTOR: &str = "genco-panel";
const PANEL_KEY: &str = "/etc/genco/dkim/panel.key";

/// Signs genco's mail as its domain when the key is there; without it
/// the mail goes unsigned, a report must not fail on the key.
fn sign(domain: &str, msg: &mut Message) {
	use lettre::message::dkim::{
		DkimConfig, DkimSigningAlgorithm, DkimSigningKey,
	};
	let Ok(pem) = std::fs::read_to_string(PANEL_KEY) else {
		return;
	};
	match DkimSigningKey::new(&pem, DkimSigningAlgorithm::Rsa) {
		Ok(key) => msg.sign(&DkimConfig::default_config(
			PANEL_SELECTOR.into(),
			domain.into(),
			key,
		)),
		Err(e) => tracing::error!("DKIM key of genco's mail: {e}"),
	}
}

/// Makes genco's DKIM key if there is none, true if it did. PKCS#1, what
/// lettre reads; only root.
pub async fn ensure_panel_key() -> std::result::Result<bool, String> {
	if std::path::Path::new(PANEL_KEY).exists() {
		return Ok(false);
	}
	crate::mailfilter::make_key(std::path::Path::new(PANEL_KEY), None).await?;
	Ok(true)
}

/// The DKIM record of genco's key.
pub async fn panel_dkim_record() -> Option<String> {
	crate::mailfilter::dkim_record_of(std::path::Path::new(PANEL_KEY)).await
}

/// Every admin's address.
pub fn admins(db: &Db) -> Result<Vec<String>> {
	Ok(users::list(&db.conn())?
		.into_iter()
		.map(|u| u.email)
		.collect())
}

const POSTCONF: &str = "/usr/sbin/postconf";

/// postfix logs in to a relay only with its PLAIN and LOGIN modules,
/// without them it fails with "no mechanism available".
pub const SASL_PACKAGE: &str = "libsasl2-modules";

/// Installs the SASL modules if they are missing, true if it did; a
/// server set up by an older genco has none.
pub async fn ensure_sasl() -> std::result::Result<bool, String> {
	let status =
		crate::util::run(&["dpkg-query", "-W", "-f=${Status}", SASL_PACKAGE])
			.await
			.unwrap_or_default();
	if status.ends_with("ok installed") {
		return Ok(false);
	}
	crate::util::run(&[
		"sh",
		"-c",
		&format!(
			"DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
			{SASL_PACKAGE} >/dev/null"
		),
	])
	.await?;
	Ok(true)
}

/// Writes the relay into postfix's config and reloads it; direct and off
/// take it out.
async fn configure_postfix(s: &MailSettings) -> Result<()> {
	let run = |args: Vec<String>| async move {
		let args: Vec<&str> = args.iter().map(String::as_str).collect();
		crate::util::run(&args)
			.await
			.map(|_| ())
			.map_err(Error::Internal)
	};

	if s.mode == "relay" {
		ensure_sasl().await.map_err(Error::Internal)?;
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
	} else if s.mode == "off" && !std::path::Path::new(POSTCONF).exists() {
		// without mail domains it needs no postfix, nothing to undo
		return Ok(());
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
		"direct" | "off" => {}
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
	new.sender = check_sender(&new.sender)?;
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
		"sender": new.sender,
	});
	set_setting(&db.conn(), "mail", &stored.to_string())?;
	Ok(Json(()))
}

/// The relay's sender, a plain address like ie@dunkel.cc or empty.
fn check_sender(sender: &str) -> Result<String> {
	let sender = sender.trim();
	if sender.is_empty() {
		return Ok(String::new());
	}
	sender
		.parse::<lettre::Address>()
		.map(|a| a.to_string())
		.map_err(|_| {
			Error::Invalid(format!(
				"the sender {sender} is no address like ie@example.ch"
			))
		})
}

#[derive(Debug, Deserialize)]
struct TestReq {
	to: String,
}

/// Sends the test and waits for postfix: a deferred or bounced mail
/// (the relay's login failed) is an error with postfix's reason.
async fn test_mail(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<TestReq>,
) -> Result<Json<String>> {
	let id = queue(
		&cfg,
		&db,
		std::slice::from_ref(&req.to),
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
	let to = req.to;
	let Some(id) = id else {
		return Ok(Json(format!(
			"postfix took it for {to}, check the inbox (and the spam folder)."
		)));
	};
	match wait_for_delivery(&id).await {
		Some(Delivery::Sent(reply)) => Ok(Json(format!(
			"Delivered for {to} ({reply}), check the inbox (and the spam \
			folder)."
		))),
		Some(Delivery::Deferred(reason)) => Err(Error::Invalid(format!(
			"Not delivered, postfix tries again later: {reason}"
		))),
		Some(Delivery::Bounced(reason)) => {
			Err(Error::Invalid(format!("Bounced: {reason}")))
		}
		None => Ok(Json(format!(
			"Still in postfix's queue as {id}, see `mailq` and \
			`journalctl -u postfix` on the server."
		))),
	}
}

#[derive(Debug, Deserialize)]
struct Port25Query {
	/// check again instead of the last result
	#[serde(default)]
	again: bool,
}

/// Whether the server reaches mail servers on port 25, checked once.
async fn port25_route(
	_authed: Authed,
	axum::extract::Query(q): axum::extract::Query<Port25Query>,
) -> Json<Port25> {
	let cached = *PORT25.lock().unwrap();
	if let Some(p) = cached.filter(|_| !q.again) {
		return Json(p);
	}
	let p = Port25 {
		blocked: port25_blocked().await,
		checked_at: crate::db::now(),
	};
	*PORT25.lock().unwrap() = Some(p);
	Json(p)
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/settings/mail", get(get_settings).post(save_settings))
		.route("/settings/mail/test", post(test_mail))
		.route("/settings/mail/port25", get(port25_route))
		.route("/settings/mail/dns", get(dns_route))
}

/// The DNS records genco's own mails need, sent from genco@<panel>.
async fn dns_route(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
) -> Result<Json<Vec<crate::mailbox::DnsRecord>>> {
	let groups = crate::hosting::groups(&db.conn())?;
	Ok(Json(crate::mailbox::panel_dns(&cfg, &groups).await?))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn relay(sender: &str) -> MailSettings {
		MailSettings {
			mode: "relay".into(),
			sender: sender.into(),
			..Default::default()
		}
	}

	#[test]
	fn postfix_says_the_queue_id() {
		assert_eq!(
			queue_id("2.0.0 Ok: queued as 4dXy1z2AbC").as_deref(),
			Some("4dXy1z2AbC")
		);
		assert_eq!(queue_id("2.0.0 Ok"), None);
	}

	#[test]
	fn the_log_and_queue_say_what_became_of_it() {
		let deferred = "4AB: to=<a@x.ch>, relay=mail.infomaniak.com[1.2.3.4]:587, \
			delay=1, dsn=4.7.0, status=deferred (SASL authentication failed; \
			cannot authenticate to server mail.infomaniak.com[1.2.3.4]: no \
			mechanism available)";
		assert_eq!(
			delivery("4AB", deferred, ""),
			Some(Delivery::Deferred(
				"SASL authentication failed; cannot authenticate to server \
				mail.infomaniak.com[1.2.3.4]: no mechanism available"
					.into()
			))
		);
		// another mail's line doesn't count, the last line of ours does
		let log = "4AB: to=<a@x.ch>, status=sent (250 2.0.0 Ok)\n\
			4ABC: to=<b@x.ch>, status=bounced (550 no such user)";
		assert_eq!(
			delivery("4AB", log, ""),
			Some(Delivery::Sent("250 2.0.0 Ok".into()))
		);
		assert_eq!(
			delivery("4ABC", log, ""),
			Some(Delivery::Bounced("550 no such user".into()))
		);

		let queue = r#"{"queue_name": "deferred", "queue_id": "4AB",
			"recipients": [{"address": "a@x.ch", "delay_reason": "connect to x timed out"}]}"#
			.replace('\n', " ");
		assert_eq!(
			delivery("4AB", "", &queue),
			Some(Delivery::Deferred("connect to x timed out".into()))
		);
		let active = r#"{"queue_name": "active", "queue_id": "4AB"}"#;
		assert_eq!(delivery("4AB", "4AB: uid=0 from=<genco>", active), None);
	}

	#[test]
	fn the_sender_is_an_address() {
		assert_eq!(check_sender(" ").unwrap(), "");
		assert_eq!(check_sender(" ie@dunkel.cc\n").unwrap(), "ie@dunkel.cc");
		for bad in ["ie", "ie@", "a b@x.ch", "genco <ie@dunkel.cc>", "@x.ch"] {
			assert!(check_sender(bad).is_err(), "{bad:?}");
		}
	}

	#[test]
	fn the_relays_sender_is_from_and_envelope_unsigned() {
		let panel = Some("panel.example.ch");
		let own = sender(panel, &relay("")).unwrap();
		assert_eq!(own.email.to_string(), "genco@panel.example.ch");
		assert_eq!(signs_as(panel, &own), panel);

		let from = sender(panel, &relay("ie@dunkel.cc")).unwrap();
		assert_eq!(from.email.to_string(), "ie@dunkel.cc");
		assert_eq!(signs_as(panel, &from), None);
		let msg = Message::builder()
			.from(from)
			.to("a@example.ch".parse().unwrap())
			.body(String::new())
			.unwrap();
		assert_eq!(
			msg.envelope().from().map(|a| a.to_string()).as_deref(),
			Some("ie@dunkel.cc")
		);

		// set once with the relay, direct delivery sends as the panel
		let direct = MailSettings {
			mode: "direct".into(),
			..relay("ie@dunkel.cc")
		};
		let own = sender(panel, &direct).unwrap();
		assert_eq!(own.email.to_string(), "genco@panel.example.ch");
	}
}
