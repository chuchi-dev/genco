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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
	/// mail domains with mailboxes, forwards and send-only addresses on
	/// this server; off keeps genco's own mails. As saved, kept while
	/// mail is off
	#[serde(default = "crate::util::yes")]
	pub hosting: bool,
	/// whether hosting is on now: saved on and mail not off
	#[serde(default, skip_deserializing)]
	pub hosting_on: bool,
}

impl Default for MailSettings {
	fn default() -> Self {
		Self {
			mode: String::new(),
			relay_host: String::new(),
			relay_port: 0,
			relay_user: String::new(),
			relay_password: String::new(),
			has_password: false,
			sender: String::new(),
			hosting: true,
			hosting_on: false,
		}
	}
}

/// As saved, `hosting` without the "off" mode in it.
fn stored(conn: &rusqlite::Connection) -> MailSettings {
	crate::db::get_json(conn, "mail").unwrap_or_default()
}

/// Whether mail domains can be added: hosting on and mail not off.
fn hosting_on(s: &MailSettings) -> bool {
	s.mode != "off" && s.hosting
}

pub fn settings(db: &Db) -> Result<MailSettings> {
	let mut s = stored(&db.conn());
	if s.mode.is_empty() {
		s.mode = "direct".into();
	}
	s.has_password = !s.relay_password.is_empty();
	s.hosting_on = hosting_on(&s);
	Ok(s)
}

/// Whether genco sends no mail at all.
pub fn off(db: &Db) -> bool {
	is_off(&db.conn())
}

pub fn is_off(conn: &rusqlite::Connection) -> bool {
	stored(conn).mode == "off"
}

/// The error of what needs a mail while mail is off.
pub fn off_error(what: &str) -> Error {
	Error::Invalid(format!("mail is off in Settings → Mail, {what}"))
}

/// Whether no new mail domains, mailboxes, forwards or send-only
/// addresses can be made here, also with mail off: nothing could send.
pub fn hosting_off(conn: &rusqlite::Connection) -> bool {
	!hosting_on(&stored(conn))
}

/// The error of what needs mail hosting while it is off.
pub fn hosting_off_error(what: &str) -> Error {
	Error::Invalid(format!("mail hosting is off in Settings → Mail, {what}"))
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

/// postfix's delivery agents and queue manager, only they write the
/// status of a mail; any other program could log a line like theirs. Its
/// unit is set by journald, the name (`-t`) by the program that logs.
const POSTFIX_UNITS: [&str; 2] = [
	"_SYSTEMD_UNIT=postfix.service",
	"_SYSTEMD_UNIT=postfix@-.service",
];
const POSTFIX_LOGS: [&str; 6] = [
	"postfix/smtp",
	"postfix/qmgr",
	"postfix/bounce",
	"postfix/lmtp",
	"postfix/local",
	"postfix/error",
];

/// Waits up to 15 s for postfix to deliver the mail or give its reason,
/// reading postfix's log lines since the mail was queued at `since`.
async fn wait_for_delivery(id: &str, since: i64) -> Option<Delivery> {
	// a second earlier: journalctl's --since has whole seconds
	let since = format!("@{}", since / 1000 - 1);
	let mut args = vec!["journalctl", "-q", "--no-pager", "-o", "cat"];
	args.extend(POSTFIX_UNITS);
	for t in POSTFIX_LOGS {
		args.extend(["-t", t]);
	}
	args.extend(["--since", &since]);
	for _ in 0..15 {
		tokio::time::sleep(std::time::Duration::from_secs(1)).await;
		let log = crate::util::run(&args).await.unwrap_or_default();
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

#[derive(Debug, Deserialize)]
struct SaveReq {
	#[serde(flatten)]
	settings: MailSettings,
	/// hosting has its own route, here it is refused instead of ignored
	#[serde(default)]
	hosting: Option<serde_json::Value>,
	/// the admin's password and code: a relay of a stolen session would
	/// get the mails that confirm deletions
	#[serde(flatten)]
	auth: crate::users::PasswordOnly,
}

async fn save_settings(
	authed: Authed,
	State(db): State<Db>,
	Json(req): Json<SaveReq>,
) -> Result<Json<()>> {
	if req.hosting.is_some() {
		return Err(Error::Invalid(
			"hosting is switched with POST /settings/mail/hosting".into(),
		));
	}
	crate::users::verify_password_and_code(&db, &authed.user, &req.auth)
		.await?;
	let mut new = req.settings;
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
	store_settings(&db.conn(), &new)?;
	Ok(Json(()))
}

/// Saves all but hosting, read and written under one lock: hosting is
/// switched on its own and mustn't be lost in between.
fn store_settings(
	conn: &rusqlite::Connection,
	new: &MailSettings,
) -> Result<()> {
	// with the password, the struct skips it only when serializing for
	// the browser
	let stored = serde_json::json!({
		"mode": new.mode,
		"relayHost": new.relay_host,
		"relayPort": new.relay_port,
		"relayUser": new.relay_user,
		"relayPassword": new.relay_password,
		"sender": new.sender,
		// its own switch, kept also while mail is off
		"hosting": stored(conn).hosting,
	});
	set_setting(conn, "mail", &stored.to_string())?;
	Ok(())
}

#[derive(Debug, Deserialize)]
struct HostingReq {
	hosting: bool,
}

/// Mail hosting on or off, without the password: it changes no mail
/// genco sends. Existing mail domains stay, the answer is how many.
async fn save_hosting(
	_authed: Authed,
	State(db): State<Db>,
	Json(req): Json<HostingReq>,
) -> Result<Json<i64>> {
	set_hosting(&db.conn(), req.hosting)?;
	let domains =
		db.conn()
			.query_row("SELECT count(*) FROM mail_domains", [], |r| r.get(0))?;
	Ok(Json(domains))
}

fn set_hosting(conn: &rusqlite::Connection, hosting: bool) -> Result<()> {
	if hosting && is_off(conn) {
		return Err(off_error("nothing could send; turn mail on first"));
	}
	let mut stored: serde_json::Value = crate::db::get_json(conn, "mail")
		.unwrap_or_else(|| serde_json::json!({ "mode": "direct" }));
	stored["hosting"] = hosting.into();
	set_setting(conn, "mail", &stored.to_string())?;
	Ok(())
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
	Ok(Json(test(&cfg, &db, &req.to, &authed.user.name).await?))
}

/// The test mail of the panel and `genco mail test`, sent by `by`.
pub async fn test(cfg: &Config, db: &Db, to: &str, by: &str) -> Result<String> {
	let since = crate::db::now();
	let id = queue(
		cfg,
		db,
		&[to.to_string()],
		"genco test mail",
		&format!(
			"{by} sent this test from genco on {}.\n\nIf it is in the spam \
			folder, use a relay or add SPF and DKIM for the sender's \
			domain.\n",
			cfg.domain.as_deref().unwrap_or("this server")
		),
	)
	.await?;
	let Some(id) = id else {
		return Ok(format!(
			"postfix took it for {to}, check the inbox (and the spam folder)."
		));
	};
	match wait_for_delivery(&id, since).await {
		Some(Delivery::Sent(reply)) => Ok(format!(
			"Delivered for {to} ({reply}), check the inbox (and the spam \
			folder)."
		)),
		Some(Delivery::Deferred(reason)) => Err(Error::Invalid(format!(
			"Not delivered, postfix tries again later: {reason}"
		))),
		Some(Delivery::Bounced(reason)) => {
			Err(Error::Invalid(format!("Bounced: {reason}")))
		}
		None => Ok(format!(
			"Still in postfix's queue as {id}, see `mailq` and \
			`journalctl -u postfix` on the server."
		)),
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
		.route("/settings/mail/hosting", post(save_hosting))
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

	#[tokio::test]
	async fn saving_needs_the_password_and_code() {
		let db = Db::memory();
		let hash = crate::users::hash_password("admin password").unwrap();
		let a = crate::users::create(&db.conn(), "m@example.ch", "A", &hash)
			.unwrap();
		let secret = totp_rs::Secret::generate_secret().to_encoded();
		db.conn()
			.execute(
				"UPDATE users SET id = 703, totp_secret = ?1 WHERE id = ?2",
				rusqlite::params![secret.to_string(), a.id],
			)
			.unwrap();
		let authed = || Authed {
			token: String::new(),
			user: crate::users::by_id(&db.conn(), 703).unwrap(),
		};
		let save = |password: &str| {
			let req = serde_json::json!({
				"mode": "relay",
				"relayHost": "evil.example.ch",
				"relayPort": 587,
				"password": password,
			});
			save_settings(
				authed(),
				State(db.clone()),
				Json(serde_json::from_value(req).unwrap()),
			)
		};

		let wrong = save("wrong").await.unwrap_err().to_string();
		assert!(wrong.contains("password is wrong"), "{wrong}");
		let no_code = save("admin password").await.unwrap_err().to_string();
		assert!(no_code.contains("two-factor"), "{no_code}");
		assert_ne!(settings(&db).unwrap().relay_host, "evil.example.ch");
	}

	#[test]
	fn hosting_is_its_own_switch() {
		let db = Db::memory();
		let conn = db.conn();
		// on for servers from before the switch
		assert!(stored(&conn).hosting && !hosting_off(&conn));
		set_setting(&conn, "mail", r#"{"mode":"relay","relayPassword":"pw"}"#)
			.unwrap();
		assert!(!hosting_off(&conn));

		set_hosting(&conn, false).unwrap();
		assert!(hosting_off(&conn) && !is_off(&conn));
		let s = stored(&conn);
		assert_eq!(
			(s.mode.as_str(), s.relay_password.as_str()),
			("relay", "pw")
		);
		drop(conn);
		assert!(!settings(&db).unwrap().hosting);

		db.conn().execute("DELETE FROM settings", []).unwrap();
		set_hosting(&db.conn(), false).unwrap();
		assert_eq!(settings(&db).unwrap().mode, "direct");
		set_hosting(&db.conn(), true).unwrap();
		assert!(settings(&db).unwrap().hosting);
	}

	#[test]
	fn no_mail_means_no_hosting() {
		let db = Db::memory();
		let conn = db.conn();
		set_setting(&conn, "mail", r#"{"mode":"off","hosting":true}"#).unwrap();
		assert!(hosting_off(&conn));
		let refused = set_hosting(&conn, true).unwrap_err().to_string();
		assert!(refused.contains("mail is off"), "{refused}");
		// turning it off is fine, the choice stays for when mail is back
		set_hosting(&conn, false).unwrap();
		drop(conn);
		assert!(!settings(&db).unwrap().hosting);
	}

	#[test]
	fn the_browser_gets_saved_and_effective_hosting() {
		let db = Db::memory();
		set_setting(&db.conn(), "mail", r#"{"mode":"off","hosting":true}"#)
			.unwrap();
		let json = serde_json::to_value(settings(&db).unwrap()).unwrap();
		assert_eq!(json["hosting"], true);
		assert_eq!(json["hostingOn"], false);

		// mail back on: saving the mode keeps the saved hosting
		let direct = MailSettings {
			mode: "direct".into(),
			..Default::default()
		};
		store_settings(&db.conn(), &direct).unwrap();
		let json = serde_json::to_value(settings(&db).unwrap()).unwrap();
		assert_eq!(
			(&json["hosting"], &json["hostingOn"]),
			(&true.into(), &true.into())
		);
		assert!(!hosting_off(&db.conn()));
	}

	#[tokio::test]
	async fn the_general_save_refuses_hosting() {
		let db = Db::memory();
		set_setting(&db.conn(), "mail", r#"{"mode":"direct","hosting":true}"#)
			.unwrap();
		let req: SaveReq = serde_json::from_value(serde_json::json!({
			"mode": "direct",
			"hosting": false,
			"password": "x",
		}))
		.unwrap();
		let hash = crate::users::hash_password("admin password").unwrap();
		let user = crate::users::create(&db.conn(), "m@example.ch", "A", &hash)
			.unwrap();
		let authed = Authed {
			token: String::new(),
			user,
		};
		let err = save_settings(authed, State(db.clone()), Json(req))
			.await
			.unwrap_err()
			.to_string();
		assert!(err.contains("/settings/mail/hosting"), "{err}");
		assert!(settings(&db).unwrap().hosting);
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
