//! The personal link of each admin in a proposal's mail: it shows the
//! proposal without a login and approves or rejects it, easy on a phone.
//! Approving needs a second factor besides the mailbox, the approval and
//! its comment steer genco's agent: the 2FA code, or the admin's password
//! without 2FA, both counted per account like at the login. Rejecting
//! needs none, it can't make anything run. Opening a link changes nothing,
//! mail scanners fetch links. The links of a proposal stop working once it
//! is decided, any way, or expired. They work only on the panel's domain,
//! and links that don't work count per client address (`MAX_FAILURES`).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};

use super::{Proposal, RejectReq, admin_note, approved, get_one, reject_as};
use crate::AppState;
use crate::activity;
use crate::agent::Runner;
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::Events;
use crate::tokens;
use crate::users::{self, User};
use crate::util;

/// wrong links from one address before it waits
const MAX_FAILURES: u32 = 20;

/// A new link of the admin for the proposal, returns its token.
pub(super) fn create(
	conn: &Connection,
	proposal_id: i64,
	user_id: i64,
) -> Result<String> {
	let token = util::random_hex(32);
	conn.execute(
		"INSERT INTO proposal_links (proposal_id, user_id, token_hash,
		created_at) VALUES (?1, ?2, ?3, ?4)",
		params![proposal_id, user_id, tokens::hash(&token), now()],
	)?;
	Ok(token)
}

/// The waiting proposal of a link and its admin; `NotFound` for a link
/// that doesn't (or no longer) work.
fn find(conn: &Connection, token: &str) -> Result<(Proposal, User)> {
	let (proposal_id, user_id): (i64, i64) = conn
		.query_row(
			"SELECT proposal_id, user_id FROM proposal_links
			WHERE token_hash = ?1",
			[tokens::hash(token)],
			|r| Ok((r.get(0)?, r.get(1)?)),
		)
		.optional()?
		.ok_or(Error::NotFound)?;
	let p = get_one(conn, proposal_id)?;
	if p.status != "pending" {
		return Err(Error::NotFound);
	}
	Ok((p, users::by_id(conn, user_id)?))
}

/// `find` for a request: only on the panel's domain, and an address with
/// too many wrong links waits.
async fn check(
	cfg: &Config,
	db: &Db,
	headers: &HeaderMap,
	token: &str,
) -> Result<(Proposal, User)> {
	let host = headers.get("host").and_then(|v| v.to_str().ok());
	if host.is_none() || host != cfg.domain.as_deref() {
		return Err(Error::NotFound);
	}
	let key =
		format!("proposal-link:{}", users::client_ip(headers).unwrap_or("?"));
	if !users::attempts_allowed(&key, MAX_FAILURES) {
		return Err(Error::Invalid(
			"Too many links that don't work, try again in 15 minutes.".into(),
		));
	}
	let found = find(&db.conn(), token);
	if let Err(Error::NotFound) = found {
		users::attempt_failed(&key, MAX_FAILURES);
		// slow down guessing
		tokio::time::sleep(Duration::from_secs(1)).await;
	}
	found
}

/// Logs a decision through the link.
fn log(conn: &Connection, by: &User, action: &str, p: &Proposal, ok: bool) {
	let detail = format!("proposal #{}", p.id);
	activity::log(conn, &by.name, action, &p.from, &detail, Some(ok));
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkView {
	proposal: Proposal,
	/// the admin of the link
	name: String,
	/// approving asks for the code of the authenticator app, otherwise for
	/// the admin's password
	totp: bool,
}

/// What the link's page shows: the proposal, nothing else of the server.
async fn show(
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	headers: HeaderMap,
	Path(token): Path<String>,
) -> Result<Json<LinkView>> {
	let (proposal, user) = check(&cfg, &db, &headers, &token).await?;
	Ok(Json(LinkView {
		proposal,
		name: user.name,
		totp: user.totp,
	}))
}

#[derive(Debug, Default, Deserialize)]
struct ApproveReq {
	/// the admin's change to the task, empty for none
	#[serde(default)]
	comment: String,
	/// of the authenticator app, when the admin has 2FA
	#[serde(default)]
	code: String,
	/// the admin's own, when the admin has no 2FA
	#[serde(default)]
	password: String,
}

/// Approves as the link's admin with the second factor, returns the
/// conversation for the runner. The comment is checked first: a comment
/// too long mustn't use up the code.
async fn approve_link(
	cfg: &Config,
	db: &Db,
	events: &Events,
	headers: &HeaderMap,
	token: &str,
	req: &ApproveReq,
) -> Result<i64> {
	let (p, user) = check(cfg, db, headers, token).await?;
	let comment = admin_note(&req.comment)?;
	let action = "Approve proposal via the mail link";
	let verified = if user.totp {
		users::verify_code(db, &user, &req.code).await
	} else {
		users::verify_password(db, &user, &req.password).await
	};
	if let Err(e) = verified {
		log(&db.conn(), &user, action, &p, false);
		return Err(e);
	}

	let conn = db.conn();
	let conversation =
		approved(&conn, events, p.id, &user, comment.as_deref())?;
	log(&conn, &user, action, &p, true);
	Ok(conversation)
}

async fn approve(
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	State(runner): State<Runner>,
	headers: HeaderMap,
	Path(token): Path<String>,
	req: Option<Json<ApproveReq>>,
) -> Result<Json<()>> {
	let req = req.unwrap_or_default().0;
	let c = approve_link(&cfg, &db, &events, &headers, &token, &req).await?;
	runner.start(c);
	Ok(Json(()))
}

/// Rejects as the link's admin, no code needed: a rejection can't do harm.
async fn reject_link(
	cfg: &Config,
	db: &Db,
	events: &Events,
	headers: &HeaderMap,
	token: &str,
	message: &str,
) -> Result<()> {
	let (p, user) = check(cfg, db, headers, token).await?;
	reject_as(cfg, db, events, p.id, &user, message).await?;
	log(
		&db.conn(),
		&user,
		"Reject proposal via the mail link",
		&p,
		true,
	);
	Ok(())
}

async fn reject(
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	headers: HeaderMap,
	Path(token): Path<String>,
	req: Option<Json<RejectReq>>,
) -> Result<Json<()>> {
	let message = req.unwrap_or_default().0.message;
	reject_link(&cfg, &db, &events, &headers, &token, &message).await?;
	Ok(Json(()))
}

pub(super) fn routes() -> Router<AppState> {
	Router::new()
		.route("/proposal-links/{token}", get(show))
		.route("/proposal-links/{token}/approve", post(approve))
		.route("/proposal-links/{token}/reject", post(reject))
}

#[cfg(test)]
mod tests {
	use totp_rs::{Algorithm, Secret, TOTP};

	use super::*;
	use crate::proposals::{MAX_NOTE, expire, insert, rewrite::Draft};

	const DOMAIN: &str = "panel.example.ch";
	const PASSWORD: &str = "the admin's password";

	fn cfg() -> Config {
		toml::from_str(&format!("domain = \"{DOMAIN}\"")).unwrap()
	}

	/// A request on the panel's domain, from its own address: wrong links
	/// of other tests don't count against it.
	fn headers(ip: &str) -> HeaderMap {
		let mut h = HeaderMap::new();
		h.insert("host", DOMAIN.parse().unwrap());
		h.insert("x-forwarded-for", ip.parse().unwrap());
		h
	}

	/// An admin with `PASSWORD`; tests that count wrong ones use names of
	/// their own, wrong passwords count per email.
	fn admin(conn: &Connection, id: i64, name: &str) -> User {
		// the lowest cost, a test needn't wait for bcrypt
		let hash = bcrypt::hash(PASSWORD, 4).unwrap();
		conn.execute(
			"INSERT INTO users (id, email, name, password_hash, created_at)
			VALUES (?1, ?2, ?3, ?4, 0)",
			params![id, format!("{name}@links.example.ch"), name, hash],
		)
		.unwrap();
		users::by_id(conn, id).unwrap()
	}

	/// Turns on 2FA for the admin, returns a function for its codes.
	fn with_2fa(db: &Db, user: &User) -> impl Fn() -> String + use<> {
		let secret = Secret::generate_secret().to_encoded().to_string();
		db.conn()
			.execute(
				"UPDATE users SET totp_secret = ?1 WHERE id = ?2",
				params![secret, user.id],
			)
			.unwrap();
		let bytes = Secret::Encoded(secret).to_bytes().unwrap();
		let totp = TOTP::new(
			Algorithm::SHA1,
			6,
			1,
			30,
			bytes,
			Some("genco".into()),
			user.email.clone(),
		)
		.unwrap();
		move || totp.generate((now() / 1000) as u64)
	}

	/// A waiting proposal of the agent, for `approver` or every admin.
	fn proposal(conn: &Connection, approver: Option<i64>) -> i64 {
		conn.execute(
			"INSERT OR IGNORE INTO peers (id, name, key, key_hash, created_at)
			VALUES (1, 'Corleone', 'k', 'h', 0)",
			[],
		)
		.unwrap();
		let flights = super::super::InFlight(Default::default());
		let draft = Draft::new("Update Craft").unwrap();
		insert(conn, flights.reserve(conn, 1).unwrap(), &draft, approver)
			.unwrap()
	}

	fn code(code: &str) -> ApproveReq {
		ApproveReq {
			code: code.into(),
			..Default::default()
		}
	}

	fn password(password: &str) -> ApproveReq {
		ApproveReq {
			password: password.into(),
			..Default::default()
		}
	}

	fn status(db: &Db, id: i64) -> String {
		get_one(&db.conn(), id).unwrap().status
	}

	fn links(db: &Db) -> i64 {
		db.conn()
			.query_row("SELECT count(*) FROM proposal_links", [], |r| r.get(0))
			.unwrap()
	}

	#[tokio::test]
	async fn every_recipient_gets_their_own_link() {
		let db = Db::memory();
		let (cfg, h) = (cfg(), headers("10.0.0.1"));
		let (id, named) = {
			let conn = db.conn();
			admin(&conn, 801, "a");
			admin(&conn, 802, "b");
			(proposal(&conn, None), proposal(&conn, Some(802)))
		};

		let ta = create(&db.conn(), id, 801).unwrap();
		let tb = create(&db.conn(), id, 802).unwrap();
		assert_eq!(ta.len(), 64);
		assert_ne!(ta, tb);
		let (p, user) = check(&cfg, &db, &h, &tb).await.unwrap();
		assert_eq!((p.id, user.name.as_str()), (id, "b"));
		// only the hash is stored
		let stored: i64 = db
			.conn()
			.query_row(
				"SELECT count(*) FROM proposal_links WHERE token_hash = ?1",
				[&ta],
				|r| r.get(0),
			)
			.unwrap();
		assert_eq!(stored, 0);

		let tn = create(&db.conn(), named, 802).unwrap();
		let (p, _) = check(&cfg, &db, &h, &tn).await.unwrap();
		assert_eq!(p.id, named);
	}

	#[tokio::test]
	async fn opening_the_link_changes_nothing() {
		let db = Db::memory();
		let (cfg, h) = (Arc::new(cfg()), headers("10.0.0.2"));
		let id = proposal(&db.conn(), None);
		admin(&db.conn(), 811, "a");
		let token = create(&db.conn(), id, 811).unwrap();

		for _ in 0..2 {
			let view = show(
				State(cfg.clone()),
				State(db.clone()),
				h.clone(),
				Path(token.clone()),
			)
			.await
			.unwrap();
			assert_eq!(view.proposal.text, "Update Craft");
			assert_eq!(view.name, "a");
			assert!(!view.totp);
		}
		assert_eq!(status(&db, id), "pending");
		assert_eq!(links(&db), 1);
	}

	#[tokio::test]
	async fn the_link_approves_once_and_dies_with_the_decision() {
		let db = Db::memory();
		let (cfg, h, events) = (cfg(), headers("10.0.0.3"), Events::new());
		let id = proposal(&db.conn(), None);
		let a = admin(&db.conn(), 821, "a");
		admin(&db.conn(), 822, "b");
		let ta = create(&db.conn(), id, 821).unwrap();
		let tb = create(&db.conn(), id, 822).unwrap();

		let c = approve_link(&cfg, &db, &events, &h, &tb, &password(PASSWORD))
			.await
			.unwrap();
		let p = get_one(&db.conn(), id).unwrap();
		assert_eq!(p.status, "approved");
		assert_eq!(p.decided_by.as_deref(), Some("b"));
		assert_eq!(p.conversation_id, Some(c));
		let logged: String = db
			.conn()
			.query_row(
				"SELECT actor || ': ' || action || ' ' || detail FROM activity",
				[],
				|r| r.get(0),
			)
			.unwrap();
		assert_eq!(logged, "b: Approve proposal via the mail link proposal #1");

		// neither link works anymore, nor a decision in the panel
		for t in [&ta, &tb] {
			let again =
				approve_link(&cfg, &db, &events, &h, t, &password(PASSWORD))
					.await;
			assert!(matches!(again, Err(Error::NotFound)));
			let again = reject_link(&cfg, &db, &events, &h, t, "").await;
			assert!(matches!(again, Err(Error::NotFound)));
		}
		assert_eq!(links(&db), 0);
		assert!(approved(&db.conn(), &events, id, &a, None).is_err());
	}

	#[tokio::test]
	async fn a_decision_in_the_panel_or_the_expiry_ends_the_links() {
		let db = Db::memory();
		let (cfg, h, events) = (cfg(), headers("10.0.0.4"), Events::new());
		let a = admin(&db.conn(), 831, "a");
		let decided = proposal(&db.conn(), None);
		let token = create(&db.conn(), decided, 831).unwrap();
		approved(&db.conn(), &events, decided, &a, None).unwrap();
		assert!(matches!(
			check(&cfg, &db, &h, &token).await,
			Err(Error::NotFound)
		));

		// refused when it's too old, before and after it is marked expired
		let old = proposal(&db.conn(), None);
		let token = create(&db.conn(), old, 831).unwrap();
		db.conn()
			.execute("UPDATE proposals SET created_at = 0 WHERE id = ?1", [old])
			.unwrap();
		let tried =
			approve_link(&cfg, &db, &events, &h, &token, &password(PASSWORD))
				.await;
		assert!(matches!(tried, Err(Error::NotFound)));
		expire(&db.conn()).unwrap();
		assert_eq!(status(&db, old), "expired");
		assert_eq!(links(&db), 0);
	}

	#[tokio::test]
	async fn the_link_rejects_with_a_reason() {
		let db = Db::memory();
		let (cfg, h, events) = (cfg(), headers("10.0.0.5"), Events::new());
		let id = proposal(&db.conn(), None);
		admin(&db.conn(), 841, "a");
		let token = create(&db.conn(), id, 841).unwrap();
		reject_link(&cfg, &db, &events, &h, &token, " Not now. ")
			.await
			.unwrap();
		let p = get_one(&db.conn(), id).unwrap();
		assert_eq!(p.status, "rejected");
		assert_eq!(p.decided_by.as_deref(), Some("a"));
		assert_eq!(p.answer.as_deref(), Some("Not now."));
		assert_eq!(links(&db), 0);
	}

	#[tokio::test]
	async fn without_2fa_approving_needs_the_password() {
		let db = Db::memory();
		let (cfg, h, events) = (cfg(), headers("10.0.0.6"), Events::new());
		let id = proposal(&db.conn(), None);
		admin(&db.conn(), 851, "pw");
		let token = create(&db.conn(), id, 851).unwrap();
		let (_, user) = check(&cfg, &db, &h, &token).await.unwrap();
		assert!(!user.totp);

		// the mailbox alone doesn't approve
		for req in [password(""), password("wrong"), code("123456")] {
			let tried = approve_link(&cfg, &db, &events, &h, &token, &req);
			assert!(tried.await.is_err());
			assert_eq!(status(&db, id), "pending");
		}
		let failed: i64 = db
			.conn()
			.query_row("SELECT count(*) FROM activity WHERE ok = 0", [], |r| {
				r.get(0)
			})
			.unwrap();
		assert_eq!(failed, 3);
		approve_link(&cfg, &db, &events, &h, &token, &password(PASSWORD))
			.await
			.unwrap();
		assert_eq!(status(&db, id), "approved");
	}

	#[tokio::test]
	async fn wrong_passwords_count_like_at_the_login() {
		let db = Db::memory();
		let (cfg, h, events) = (cfg(), headers("10.0.0.10"), Events::new());
		let id = proposal(&db.conn(), None);
		let a = admin(&db.conn(), 852, "pw-count");
		let token = create(&db.conn(), id, a.id).unwrap();
		let key = format!("password:{}", a.email);
		for _ in 0..users::MAX_PASSWORD_FAILURES - 2 {
			users::attempt_failed(&key, users::MAX_PASSWORD_FAILURES);
		}

		for _ in 0..2 {
			let req = password("wrong");
			let tried = approve_link(&cfg, &db, &events, &h, &token, &req);
			assert!(tried.await.is_err());
		}
		// the two through the link used up the rest
		let req = password(PASSWORD);
		let tried = approve_link(&cfg, &db, &events, &h, &token, &req).await;
		assert!(
			matches!(tried, Err(Error::Invalid(e)) if e.contains("too many"))
		);
		assert_eq!(status(&db, id), "pending");
	}

	#[tokio::test]
	async fn with_2fa_approving_needs_the_code() {
		let db = Db::memory();
		let (cfg, h, events) = (cfg(), headers("10.0.0.11"), Events::new());
		let id = proposal(&db.conn(), None);
		let a = admin(&db.conn(), 853, "totp");
		let now_code = with_2fa(&db, &a);
		let token = create(&db.conn(), id, a.id).unwrap();
		let (_, user) = check(&cfg, &db, &h, &token).await.unwrap();
		assert!(user.totp);

		// the password doesn't do instead of the code
		for req in [code(""), code("000000"), password(PASSWORD)] {
			let tried = approve_link(&cfg, &db, &events, &h, &token, &req);
			assert!(tried.await.is_err());
			assert_eq!(status(&db, id), "pending");
		}
		approve_link(&cfg, &db, &events, &h, &token, &code(&now_code()))
			.await
			.unwrap();
		assert_eq!(status(&db, id), "approved");
	}

	#[tokio::test]
	async fn wrong_codes_count_like_at_the_login() {
		let db = Db::memory();
		let (cfg, h, events) = (cfg(), headers("10.0.0.12"), Events::new());
		let id = proposal(&db.conn(), None);
		let a = admin(&db.conn(), 854, "totp-count");
		let now_code = with_2fa(&db, &a);
		let token = create(&db.conn(), id, a.id).unwrap();
		let key = format!("code:{}", a.id);
		for _ in 0..users::MAX_CODE_FAILURES - 2 {
			users::attempt_failed(&key, users::MAX_CODE_FAILURES);
		}

		for _ in 0..2 {
			let req = code("000000");
			let tried = approve_link(&cfg, &db, &events, &h, &token, &req);
			assert!(tried.await.is_err());
		}
		let req = code(&now_code());
		let tried = approve_link(&cfg, &db, &events, &h, &token, &req).await;
		assert!(tried.is_err());
		assert_eq!(status(&db, id), "pending");
	}

	#[tokio::test]
	async fn a_long_comment_doesnt_use_up_the_code() {
		let db = Db::memory();
		let (cfg, h, events) = (cfg(), headers("10.0.0.13"), Events::new());
		let id = proposal(&db.conn(), None);
		let a = admin(&db.conn(), 855, "totp-note");
		let now_code = with_2fa(&db, &a);
		let token = create(&db.conn(), id, a.id).unwrap();

		let mut req = ApproveReq {
			comment: "x".repeat(MAX_NOTE + 1),
			code: now_code(),
			..Default::default()
		};
		let tried = approve_link(&cfg, &db, &events, &h, &token, &req).await;
		assert!(
			matches!(tried, Err(Error::Invalid(e)) if e.contains("characters"))
		);
		assert_eq!(status(&db, id), "pending");

		// the same code still works
		req.comment = "Keep the old DNS.".into();
		approve_link(&cfg, &db, &events, &h, &token, &req)
			.await
			.unwrap();
		let p = get_one(&db.conn(), id).unwrap();
		assert_eq!(p.status, "approved");
		assert_eq!(p.comment.as_deref(), Some("Keep the old DNS."));
	}

	#[tokio::test]
	async fn links_work_only_on_the_panel_domain() {
		let db = Db::memory();
		let cfg = cfg();
		let id = proposal(&db.conn(), None);
		admin(&db.conn(), 861, "a");
		let token = create(&db.conn(), id, 861).unwrap();

		let mut other = headers("10.0.0.7");
		other.insert("host", "old.example.ch".parse().unwrap());
		assert!(check(&cfg, &db, &other, &token).await.is_err());
		other.remove("host");
		assert!(check(&cfg, &db, &other, &token).await.is_err());
	}

	#[tokio::test]
	async fn wrong_links_make_the_address_wait() {
		let db = Db::memory();
		let cfg = cfg();
		let id = proposal(&db.conn(), None);
		admin(&db.conn(), 871, "a");
		let token = create(&db.conn(), id, 871).unwrap();
		let h = headers("10.0.0.8");
		// counted like the waits of the login, without their seconds
		for _ in 0..MAX_FAILURES {
			users::attempt_failed("proposal-link:10.0.0.8", MAX_FAILURES);
		}
		assert!(matches!(
			check(&cfg, &db, &h, &token).await,
			Err(Error::Invalid(_))
		));
		assert!(check(&cfg, &db, &headers("10.0.0.9"), &token).await.is_ok());
		assert!(matches!(
			check(&cfg, &db, &headers("10.0.0.9"), "00").await,
			Err(Error::NotFound)
		));
	}
}
