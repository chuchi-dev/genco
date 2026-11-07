//! Admins of the panel and their sessions. Every user is an admin.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::extract::OriginalUri;
use axum::extract::{FromRef, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use subtle::ConstantTimeEq as _;
use totp_rs::{Algorithm, Secret, TOTP};

use crate::AppState;
use crate::activity;
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::mail;
use crate::tokens;
use crate::util;

const SESSION_DURATION_MS: i64 = 1000 * 60 * 60 * 24 * 30;
/// a session not used for this long ends before its 30 days
const IDLE_MS: i64 = 1000 * 60 * 60 * 24 * 3;
/// wrong 2FA codes, or passwords, in a row before the account waits
pub const MAX_CODE_FAILURES: u32 = 5;
pub const MAX_PASSWORD_FAILURES: u32 = 10;
const CODE_LOCK_MS: i64 = 1000 * 60 * 15;

/// wrong 2FA codes (`code:<user id>`) and passwords (`password:<email>`)
/// per account, links of proposal mails that don't work per client address
/// (`proposal-link:<ip>`): how many and when the last was. nginx limits
/// the login per IP, the accounts' limits hold also for a client that
/// reaches genco without nginx, or changes its address.
static CODE_FAILURES: LazyLock<Mutex<HashMap<String, (u32, i64)>>> =
	LazyLock::new(Default::default);

/// compared against for unknown emails, a login takes as long either way
static DUMMY_HASH: LazyLock<String> =
	LazyLock::new(|| bcrypt::hash("genco", bcrypt::DEFAULT_COST).unwrap());

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
	pub id: i64,
	pub email: String,
	pub name: String,
	pub created_at: i64,
	/// logs in with a code of an authenticator app too
	pub totp: bool,
	/// hasn't set a password from the invite yet
	pub invited: bool,
}

impl User {
	fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
		Ok(Self {
			id: row.get(0)?,
			email: row.get(1)?,
			name: row.get(2)?,
			created_at: row.get(3)?,
			totp: row.get(4)?,
			invited: row.get(5)?,
		})
	}
}

const USER_COLUMNS: &str = "id, email, name, created_at, totp_secret IS NOT NULL, \
	invite_hash IS NOT NULL";

pub fn list(conn: &Connection) -> rusqlite::Result<Vec<User>> {
	let mut stmt = conn
		.prepare(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY name"))?;
	stmt.query_map([], User::from_row)?.collect()
}

/// bcrypt takes a quarter of a second: hash before taking the database,
/// in the server on a blocking thread (`hash_password_async`).
pub fn hash_password(password: &str) -> Result<String> {
	check_password(password)?;
	bcrypt::hash(password, bcrypt::DEFAULT_COST)
		.map_err(|e| Error::Internal(e.to_string()))
}

async fn hash_password_async(password: String) -> Result<String> {
	tokio::task::spawn_blocking(move || hash_password(&password))
		.await
		.map_err(|e| Error::Internal(e.to_string()))?
}

/// `hash` from `hash_password`
pub fn create(
	conn: &Connection,
	email: &str,
	name: &str,
	hash: &str,
) -> Result<User> {
	let (email, name) = normalize(email, name)?;

	conn.execute(
		"INSERT INTO users (email, name, password_hash, created_at)
		VALUES (?1, ?2, ?3, ?4)",
		params![email, name, hash, now()],
	)?;

	Ok(by_id(conn, conn.last_insert_rowid())?)
}

/// The email trimmed and lowercase, the name trimmed. The email must be
/// one mail can go to: a broken one breaks every mail to the admins.
fn normalize(email: &str, name: &str) -> Result<(String, String)> {
	let email = email.trim().to_lowercase();
	if email.parse::<lettre::Address>().is_err() {
		return Err(Error::Invalid(format!("invalid email {email:?}")));
	}
	let name = name.trim();
	if name.is_empty() {
		return Err(Error::Invalid("the name is empty".into()));
	}
	Ok((email, name.into()))
}

pub fn by_id(conn: &Connection, id: i64) -> rusqlite::Result<User> {
	conn.query_row(
		&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
		[id],
		User::from_row,
	)
}

/// `hash` from `hash_password`
pub fn set_password(conn: &Connection, user_id: i64, hash: &str) -> Result<()> {
	conn.execute(
		"UPDATE users SET password_hash = ?1 WHERE id = ?2",
		params![hash, user_id],
	)?;
	Ok(())
}

fn check_password(password: &str) -> Result<()> {
	if password.chars().count() < 10 {
		return Err(Error::Invalid(
			"the password needs at least 10 characters".into(),
		));
	}
	Ok(())
}

/// Extractor that only succeeds with a valid session token, sent as
/// `session-token` header. What a browser opens itself (the event stream
/// and downloads) is let in by a ticket in the query instead.
pub struct Authed {
	/// the hash of the session's token, as stored
	pub token: String,
	pub user: User,
}

impl<S> FromRequestParts<S> for Authed
where
	S: Send + Sync,
	Db: FromRef<S>,
{
	type Rejection = Error;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
		let hashed = session_hash(parts)?;
		let db = Db::from_ref(state);
		let conn = db.conn();
		let session: Option<(i64, i64, i64)> = conn
			.query_row(
				"SELECT created_at, last_seen, user_id FROM sessions
				WHERE token = ?1",
				[&hashed],
				|r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
			)
			.optional()?;
		let at = now();
		let Some((_, _, user_id)) = session.filter(|(created, seen, _)| {
			created + SESSION_DURATION_MS > at && seen + IDLE_MS > at
		}) else {
			return Err(Error::InvalidSessionToken);
		};
		let user = by_id(&conn, user_id)?;

		// at most once a minute, most requests only read
		conn.execute(
			"UPDATE sessions SET last_seen = ?1
			WHERE token = ?2 AND last_seen < ?1 - 60000",
			params![at, hashed],
		)?;

		Ok(Self {
			token: hashed,
			user,
		})
	}
}

/// The hashed session token of a request: from its header, or for the
/// event stream (EventSource can't send a header) and downloads from a
/// ticket, the token stays out of URLs (proxy logs, the download list).
fn session_hash(parts: &Parts) -> Result<String> {
	if let Some(token) = parts
		.headers
		.get("session-token")
		.and_then(|v| v.to_str().ok())
	{
		return Ok(tokens::hash(token));
	}
	let path = parts
		.extensions
		.get::<OriginalUri>()
		.map(|u| u.path().to_string())
		.unwrap_or_else(|| parts.uri.path().to_string());
	let pairs: Vec<&str> = parts
		.uri
		.query()
		.unwrap_or("")
		.split('&')
		.filter(|p| !p.is_empty())
		.collect();
	let value = |name: &str| {
		pairs
			.iter()
			.find_map(|p| p.strip_prefix(name)?.strip_prefix('='))
			.ok_or(Error::MissingSessionToken)
	};
	if parts.method != Method::GET {
		return Err(Error::MissingSessionToken);
	}
	if !takes_ticket(&path) {
		return Err(Error::MissingSessionToken);
	}
	// the ticket is for this path with exactly these parameters
	let ticket = value("ticket")?;
	let rest: Vec<&str> = pairs
		.iter()
		.filter(|p| !p.starts_with("ticket="))
		.copied()
		.collect();
	let target = if rest.is_empty() {
		path
	} else {
		format!("{path}?{}", rest.join("&"))
	};
	redeem(ticket, &target).ok_or(Error::InvalidSessionToken)
}

/// The event stream or a download link (a database dump or files).
fn takes_ticket(path: &str) -> bool {
	path == "/api/events"
		|| path.starts_with("/api/")
			&& (path.ends_with("/files/download") || path.ends_with("/dump"))
}

/// A single-use link to one download or event stream, for a minute.
struct Ticket {
	/// the session's hashed token
	session: String,
	path: String,
	expires: std::time::Instant,
	used: bool,
}

static TICKETS: LazyLock<Mutex<HashMap<String, Ticket>>> =
	LazyLock::new(Default::default);

const TICKET_LIFE: Duration = Duration::from_secs(60);

fn new_ticket(session: &str, path: &str) -> String {
	let ticket = crate::util::random_hex(24);
	let mut tickets = TICKETS.lock().unwrap();
	let at = std::time::Instant::now();
	tickets.retain(|_, t| t.expires > at);
	tickets.insert(
		ticket.clone(),
		Ticket {
			session: session.into(),
			path: path.into(),
			expires: at + TICKET_LIFE,
			used: false,
		},
	);
	ticket
}

/// The session of an unused ticket for `path`, it is used then.
fn redeem(ticket: &str, path: &str) -> Option<String> {
	let mut tickets = TICKETS.lock().unwrap();
	let t = tickets.get_mut(ticket)?;
	if t.used || t.path != path || t.expires < std::time::Instant::now() {
		return None;
	}
	t.used = true;
	Some(t.session.clone())
}

/// The session of a ticket, used or not, for the activity log.
pub fn ticket_session(ticket: &str) -> Option<String> {
	TICKETS
		.lock()
		.unwrap()
		.get(ticket)
		.map(|t| t.session.clone())
}

#[derive(Debug, Deserialize)]
struct TicketReq {
	/// the download's path with its parameters, as the browser will ask
	/// for it, `/api/groups/1/files/download?path=a.txt`, or `/api/events`
	path: String,
}

async fn create_ticket(
	authed: Authed,
	Json(req): Json<TicketReq>,
) -> Result<Json<String>> {
	let path = req.path.split('?').next().unwrap_or("");
	if !takes_ticket(path) {
		return Err(Error::Invalid("no download or event stream".into()));
	}
	Ok(Json(new_ticket(&authed.token, &req.path)))
}

#[derive(Debug, Deserialize)]
struct LoginReq {
	email: String,
	password: String,
	/// of the authenticator app, if the user has 2FA
	code: Option<String>,
}

#[derive(Debug, Serialize)]
struct LoginResp {
	token: String,
	user: User,
}

async fn login(
	State(db): State<Db>,
	headers: HeaderMap,
	Json(req): Json<LoginReq>,
) -> Result<Json<LoginResp>> {
	let email = req.email.trim().to_lowercase();
	let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
	let ip = client_ip(&headers);
	let from = format!("from {}", ip.unwrap_or("?"));

	let (user_id, secret, last_step) =
		check_credentials(&db, &email, req.password, &from).await?;

	// the password was right, the browser asks for the code now
	if let Some(secret) = secret {
		let code = req.code.as_deref().filter(|c| !c.is_empty());
		check_totp(&db, &email, user_id, &secret, last_step, code, &from)
			.await?;
	}

	let resp = create_session(&db, user_id, header("user-agent"), ip, &from)?;
	Ok(Json(resp))
}

/// The address of the client: genco only listens on localhost, nginx
/// appends the real client last.
pub fn client_ip(headers: &HeaderMap) -> Option<&str> {
	headers
		.get("x-forwarded-for")
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.rsplit(',').next())
		.map(str::trim)
}

/// The user id, 2FA secret and last used 2FA step of a right email and
/// password.
async fn check_credentials(
	db: &Db,
	email: &str,
	password: String,
	from: &str,
) -> Result<(i64, Option<String>, u64)> {
	let found: Option<(i64, String, Option<String>, u64)> = db
		.conn()
		.query_row(
			"SELECT id, password_hash, totp_secret, totp_step FROM users
			WHERE email = ?1",
			[email],
			|row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
		)
		.optional()?;

	// an unknown email takes as long as a wrong password
	let hash = found
		.as_ref()
		.map(|f| f.1.clone())
		.unwrap_or_else(|| DUMMY_HASH.clone());
	let right = tokio::task::spawn_blocking(move || {
		bcrypt::verify(&password, &hash).unwrap_or(false)
	})
	.await
	.unwrap_or(false);

	let key = format!("password:{email}");
	let allowed = attempts_allowed(&key, MAX_PASSWORD_FAILURES);
	let Some((id, _, secret, step)) = found.filter(|_| right && allowed) else {
		let why = if allowed {
			"Login, wrong email or password"
		} else {
			"Login, too many wrong passwords, waiting"
		};
		return Err(
			reject(db, email, from, why, &key, MAX_PASSWORD_FAILURES).await
		);
	};

	CODE_FAILURES.lock().unwrap().remove(&key);
	Ok((id, secret, step))
}

/// Checks the code of the authenticator app and uses it up.
async fn check_totp(
	db: &Db,
	email: &str,
	user_id: i64,
	secret: &str,
	last_step: u64,
	code: Option<&str>,
	from: &str,
) -> Result<()> {
	let Some(code) = code else {
		return Err(Error::TotpRequired);
	};
	let key = format!("code:{user_id}");
	let used = attempts_allowed(&key, MAX_CODE_FAILURES)
		.then(|| check_code(secret, email, code, last_step))
		.flatten()
		.map(|step| use_step(db, user_id, step))
		.transpose()?;
	if used != Some(true) {
		let why = "Login, wrong two-factor code";
		return Err(reject(db, email, from, why, &key, MAX_CODE_FAILURES).await);
	}

	CODE_FAILURES.lock().unwrap().remove(&key);
	Ok(())
}

fn create_session(
	db: &Db,
	user_id: i64,
	user_agent: Option<&str>,
	ip: Option<&str>,
	from: &str,
) -> Result<LoginResp> {
	let token = util::random_hex(32);
	let id = util::random_hex(8);

	let conn = db.conn();
	conn.execute(
		"INSERT INTO sessions
		(token, id, user_id, user_agent, ip, created_at, last_seen)
		VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
		params![tokens::hash(&token), id, user_id, user_agent, ip, now()],
	)?;
	let user = by_id(&conn, user_id)?;
	activity::log(&conn, &user.name, "Login", "", from, Some(true));

	Ok(LoginResp { token, user })
}

/// A failed login: logged, counted against `key` and slowed down.
async fn reject(
	db: &Db,
	email: &str,
	from: &str,
	why: &str,
	key: &str,
	max: u32,
) -> Error {
	activity::log(&db.conn(), email, why, "", from, Some(false));
	attempt_failed(key, max);
	// slow down guessing
	tokio::time::sleep(Duration::from_secs(1)).await;
	Error::LoginIncorrect
}

async fn me(authed: Authed) -> Json<User> {
	Json(authed.user)
}

async fn logout(State(db): State<Db>, authed: Authed) -> Result<Json<()>> {
	db.conn()
		.execute("DELETE FROM sessions WHERE token = ?1", [authed.token])?;
	Ok(Json(()))
}

async fn list_users(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<User>>> {
	Ok(Json(list(&db.conn())?))
}

async fn delete_user(
	authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
	Json(req): Json<StepUp>,
) -> Result<Json<()>> {
	if id == authed.user.id {
		return Err(Error::Invalid("you can't delete yourself".into()));
	}
	verify_step_up(&db, &authed.user, &req).await?;
	db.conn().execute("DELETE FROM users WHERE id = ?1", [id])?;
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
struct PasswordReq {
	current: String,
	password: String,
}

/// The admin's own password, for what a stolen session alone mustn't do
/// (two-factor, invites, tokens), and approving through a proposal mail's
/// link without 2FA. It counts against the same limit as the
/// login, the try before the check: parallel guesses don't get around it.
pub async fn verify_password(
	db: &Db,
	user: &User,
	password: &str,
) -> Result<()> {
	let key = format!("password:{}", user.email);
	if !attempts_allowed(&key, MAX_PASSWORD_FAILURES) {
		return Err(Error::Invalid(
			"too many wrong passwords, wait a quarter of an hour".into(),
		));
	}
	attempt_failed(&key, MAX_PASSWORD_FAILURES);
	let hash: String = db.conn().query_row(
		"SELECT password_hash FROM users WHERE id = ?1",
		[user.id],
		|r| r.get(0),
	)?;
	let password = password.to_string();
	let right = tokio::task::spawn_blocking(move || {
		bcrypt::verify(&password, &hash).unwrap_or(false)
	})
	.await
	.unwrap_or(false);
	if !right {
		tokio::time::sleep(Duration::from_secs(1)).await;
		return Err(Error::Invalid("the current password is wrong".into()));
	}
	CODE_FAILURES.lock().unwrap().remove(&key);
	Ok(())
}

/// What a dangerous request sends along: the 2FA code when the admin
/// has 2FA, else the password (`verify_step_up`).
#[derive(Debug, Default, Deserialize)]
pub struct StepUp {
	#[serde(default)]
	pub password: String,
	#[serde(default)]
	pub code: String,
}

/// Needs the current password, a stolen session can't lock the admin
/// out; the other sessions of the admin end.
async fn change_password(
	authed: Authed,
	State(db): State<Db>,
	Json(req): Json<PasswordReq>,
) -> Result<Json<()>> {
	verify_password(&db, &authed.user, &req.current).await?;

	let hash = hash_password_async(req.password).await?;
	let conn = db.conn();
	set_password(&conn, authed.user.id, &hash)?;
	conn.execute(
		"DELETE FROM sessions WHERE user_id = ?1 AND token != ?2",
		params![authed.user.id, authed.token],
	)?;
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
struct AccountReq {
	email: String,
	name: String,
	#[serde(flatten)]
	auth: StepUp,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountResp {
	user: User,
	/// the new email, mailed a link to confirm it
	confirm_sent: Option<String>,
}

/// An admin changes their own name, and email: the new one only counts
/// once the link mailed to it is opened, then the old address hears of
/// it. With the step-up a stolen session alone can't move the account
/// and its confirmation mails away. With mail off no link can go, the
/// step-up changes it right away.
async fn change_account(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<AccountReq>,
) -> Result<Json<AccountResp>> {
	let user = authed.user;
	verify_step_up(&db, &user, &req.auth).await?;
	let (email, name) = normalize(&req.email, &req.name)?;

	let confirm_sent = if email == user.email {
		None
	} else if mail::off(&db) {
		set_email(&db.conn(), user.id, &email)?;
		None
	} else {
		send_email_link(&cfg, &db, &user, &email, &name).await?;
		Some(email)
	};

	db.conn().execute(
		"UPDATE users SET name = ?1 WHERE id = ?2",
		params![name, user.id],
	)?;
	let user = by_id(&db.conn(), user.id)?;
	Ok(Json(AccountResp { user, confirm_sent }))
}

/// how long the link to confirm a new email works
const EMAIL_LINK_MS: i64 = 24 * 60 * 60 * 1000;

/// Stores `email` as the admin's next one, the token of its link
/// replaces an earlier one.
fn request_email(
	conn: &Connection,
	user_id: i64,
	email: &str,
) -> Result<String> {
	email_free(conn, email)?;
	let token = util::random_hex(32);
	conn.execute(
		"UPDATE users SET new_email = ?1, new_email_hash = ?2,
		new_email_expires = ?3 WHERE id = ?4",
		params![email, tokens::hash(&token), now() + EMAIL_LINK_MS, user_id],
	)?;
	Ok(token)
}

/// The admin's email without a link, a pending one is dropped.
fn set_email(conn: &Connection, user_id: i64, email: &str) -> Result<()> {
	email_free(conn, email)?;
	conn.execute(
		"UPDATE users SET email = ?1, new_email = NULL, new_email_hash = NULL,
		new_email_expires = NULL WHERE id = ?2",
		params![email, user_id],
	)?;
	Ok(())
}

fn email_free(conn: &Connection, email: &str) -> Result<()> {
	let taken: bool = conn.query_row(
		"SELECT EXISTS (SELECT 1 FROM users WHERE email = ?1)",
		[email],
		|r| r.get(0),
	)?;
	if taken {
		return Err(Error::Invalid(format!("{email} is another admin's")));
	}
	Ok(())
}

async fn send_email_link(
	cfg: &Config,
	db: &Db,
	user: &User,
	email: &str,
	name: &str,
) -> Result<()> {
	let domain = cfg
		.domain
		.as_deref()
		.ok_or(Error::Invalid("the panel has no domain".into()))?;
	let token = request_email(&db.conn(), user.id, email)?;

	let body = format!(
		"Hi {name}\n\n\
		Confirm {email} as the email of your genco admin account on \
		{domain} with this link, it works for a day:\n\n\
		https://{domain}/email/{token}\n\n\
		Until then you log in with {old}. If you didn't ask for this, \
		ignore this mail.\n",
		old = user.email,
	);
	let subject = format!("Confirm your admin email on {domain}");
	if let Err(e) = mail::send(cfg, db, &[email.into()], &subject, &body).await
	{
		db.conn().execute(
			"UPDATE users SET new_email = NULL, new_email_hash = NULL,
			new_email_expires = NULL WHERE id = ?1",
			[user.id],
		)?;
		return Err(e);
	}
	Ok(())
}

/// The admin's next email of a link that still works.
fn email_of_link(
	conn: &Connection,
	user_id: i64,
	token: &str,
) -> Result<String> {
	conn.query_row(
		"SELECT new_email FROM users
		WHERE id = ?1 AND new_email_hash = ?2 AND new_email_expires > ?3",
		params![user_id, tokens::hash(token), now()],
		|r| r.get(0),
	)
	.map_err(|_| Error::Invalid("the link is wrong or expired".into()))
}

/// The email of the link becomes the admin's, the link is used up; not
/// when another admin took the address in between.
fn confirm_email(conn: &Connection, user_id: i64, token: &str) -> Result<User> {
	let email = email_of_link(conn, user_id, token)?;
	email_free(conn, &email)?;
	conn.execute(
		"UPDATE users SET email = new_email, new_email = NULL,
		new_email_hash = NULL, new_email_expires = NULL WHERE id = ?1",
		[user_id],
	)?;
	Ok(by_id(conn, user_id)?)
}

/// What the page of the link shows: the email it confirms.
async fn get_email_link(
	authed: Authed,
	State(db): State<Db>,
	Path(token): Path<String>,
) -> Result<Json<String>> {
	Ok(Json(email_of_link(&db.conn(), authed.user.id, &token)?))
}

/// Opened logged in as the admin, the link alone changes nothing.
async fn confirm_email_link(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Path(token): Path<String>,
) -> Result<Json<User>> {
	let old = authed.user.email;
	let user = confirm_email(&db.conn(), authed.user.id, &token)?;

	let domain = cfg.domain.as_deref().unwrap_or("this server");
	let body = format!(
		"Hi {name}\n\n\
		The email of your genco admin account on {domain} was changed to \
		{email}. Log in with it from now on.\n\n\
		If that wasn't you, tell another admin right away.\n",
		name = user.name,
		email = user.email,
	);
	let subject = format!("Your admin email on {domain} was changed");
	if !mail::off(&db)
		&& let Err(e) = mail::send(&cfg, &db, &[old], &subject, &body).await
	{
		tracing::error!("mail about the changed email of {}: {e}", user.id);
	}
	Ok(Json(user))
}

/// how long the link of an invite works
const INVITE_MS: i64 = 3 * 24 * 60 * 60 * 1000;

#[derive(Debug, Deserialize)]
struct InviteReq {
	email: String,
	name: String,
	/// the inviting admin's step-up: a new admin is lasting
	/// access
	#[serde(flatten)]
	auth: StepUp,
}

/// Adds an admin without a password, the mail has a link to set one.
async fn invite(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<InviteReq>,
) -> Result<Json<User>> {
	if mail::off(&db) {
		return Err(mail::off_error("add the admin with a password"));
	}
	verify_step_up(&db, &authed.user, &req.auth).await?;
	// nobody knows this password, the invite sets the real one
	let hash = hash_password_async(util::random_hex(32)).await?;
	let user = create(&db.conn(), &req.email, &req.name, &hash)?;
	if let Err(e) = send_invite(&cfg, &db, &user, &authed.user.name).await {
		db.conn()
			.execute("DELETE FROM users WHERE id = ?1", [user.id])?;
		return Err(e);
	}
	Ok(Json(User {
		invited: true,
		..user
	}))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddReq {
	email: String,
	name: String,
	/// the new admin's
	new_password: String,
	/// the adding admin's, as for an invite
	#[serde(flatten)]
	auth: StepUp,
}

/// Adds an admin with a password, who can log in right away, for a
/// panel without mail or an admin sitting next to you. Nobody gets a
/// mail about it, so the other admins do.
async fn add(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<AddReq>,
) -> Result<Json<User>> {
	verify_step_up(&db, &authed.user, &req.auth).await?;
	let hash = hash_password_async(req.new_password).await?;
	let user = create(&db.conn(), &req.email, &req.name, &hash)?;

	let others: Vec<String> = list(&db.conn())?
		.into_iter()
		.filter(|u| u.id != authed.user.id && u.id != user.id)
		.map(|u| u.email)
		.collect();
	// without a domain there is no address to send from
	if let Some(domain) = cfg
		.domain
		.as_deref()
		.filter(|_| !others.is_empty() && !mail::off(&db))
	{
		let body = format!(
			"{by} added {email} ({name}) as an admin on {domain}.\n\n\
			If that's news to you, ask {by} about it.\n",
			by = authed.user.name,
			email = user.email,
			name = user.name,
		);
		let subject = format!("New admin on {domain}");
		if let Err(e) = mail::send(&cfg, &db, &others, &subject, &body).await {
			tracing::error!("mail about the added admin {}: {e}", user.id);
		}
	}
	Ok(Json(user))
}

/// A new link for an admin who lost the first one or the password.
async fn reinvite(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Path(id): Path<i64>,
	Json(req): Json<StepUp>,
) -> Result<Json<()>> {
	verify_step_up(&db, &authed.user, &req).await?;
	let user = by_id(&db.conn(), id)?;
	send_invite(&cfg, &db, &user, &authed.user.name).await?;
	Ok(Json(()))
}

async fn send_invite(
	cfg: &Config,
	db: &Db,
	user: &User,
	by: &str,
) -> Result<()> {
	if mail::off(db) {
		return Err(mail::off_error(
			"set a password with `genco user password`",
		));
	}
	let domain = cfg
		.domain
		.as_deref()
		.ok_or(Error::Invalid("the panel has no domain".into()))?;
	let token = util::random_hex(32);
	db.conn().execute(
		"UPDATE users SET invite_hash = ?1, invite_expires = ?2 WHERE id = ?3",
		params![tokens::hash(&token), now() + INVITE_MS, user.id],
	)?;

	let body = format!(
		"Hi {name}\n\n{by} invited you as an admin of the genco panel on \
		{domain}.\n\nSet your password with this link, it works for 3 \
		days:\n\nhttps://{domain}/invite/{token}\n\nThen log in with \
		{email} and turn on two-factor login under Start.\n",
		name = user.name,
		email = user.email,
	);
	mail::send(
		cfg,
		db,
		std::slice::from_ref(&user.email),
		&format!("Your admin account for {domain}"),
		&body,
	)
	.await
}

/// The admin an invite link belongs to, if it still works.
fn invited(conn: &Connection, token: &str) -> Result<User> {
	conn.query_row(
		&format!(
			"SELECT {USER_COLUMNS} FROM users
			WHERE invite_hash = ?1 AND invite_expires > ?2"
		),
		params![tokens::hash(token), now()],
		User::from_row,
	)
	.map_err(|_| Error::Invalid("the link is wrong or expired".into()))
}

async fn get_invite(
	State(db): State<Db>,
	Path(token): Path<String>,
) -> Result<Json<User>> {
	Ok(Json(invited(&db.conn(), &token)?))
}

#[derive(Debug, Deserialize)]
struct AcceptReq {
	password: String,
}

async fn accept_invite(
	State(db): State<Db>,
	Path(token): Path<String>,
	Json(req): Json<AcceptReq>,
) -> Result<Json<()>> {
	// a new invite is how an admin who lost everything gets back in:
	// the new password and no old authenticator
	let user = invited(&db.conn(), &token)?;
	let hash = hash_password_async(req.password).await?;

	let conn = db.conn();
	set_password(&conn, user.id, &hash)?;
	conn.execute(
		"UPDATE users SET invite_hash = NULL, invite_expires = NULL,
		totp_secret = NULL, totp_step = 0 WHERE id = ?1",
		[user.id],
	)?;
	conn.execute("DELETE FROM sessions WHERE user_id = ?1", [user.id])?;
	activity::log(
		&conn,
		&user.name,
		"Set password from invite",
		"",
		"",
		Some(true),
	);
	Ok(Json(()))
}

/// false while the account waits after `max` wrong tries
pub fn attempts_allowed(key: &str, max: u32) -> bool {
	match CODE_FAILURES.lock().unwrap().get(key) {
		Some((n, last)) => *n < max || last + CODE_LOCK_MS < now(),
		None => true,
	}
}

pub fn attempt_failed(key: &str, max: u32) {
	let mut failures = CODE_FAILURES.lock().unwrap();
	// unknown emails would grow the map without end
	failures.retain(|_, (_, last)| *last + CODE_LOCK_MS > now());
	let entry = failures.entry(key.to_string()).or_insert((0, 0));
	// a try while it waits neither restarts the count nor the wait; after
	// the wait the entry is gone and the count starts over
	if entry.0 < max {
		*entry = (entry.0 + 1, now());
	}
}

fn totp(secret: &str, email: &str) -> Option<TOTP> {
	let bytes = Secret::Encoded(secret.into()).to_bytes().ok()?;
	TOTP::new(
		Algorithm::SHA1,
		6,
		1,
		30,
		bytes,
		Some("genco".into()),
		email.replace(':', ""),
	)
	.ok()
}

/// The time step of the code if it is valid (now, or 30 s before or after)
/// and newer than `after`: a code can't be used twice.
fn check_code(
	secret: &str,
	email: &str,
	code: &str,
	after: u64,
) -> Option<u64> {
	let totp = totp(secret, email)?;
	let now = (now() / 1000) as u64;
	let code = code.trim();
	[now - 30, now, now + 30].into_iter().find_map(|t| {
		let step = t / 30;
		let matches: bool =
			totp.generate(t).as_bytes().ct_eq(code.as_bytes()).into();
		(matches && step > after).then_some(step)
	})
}

/// The code of the admin's authenticator app when 2FA is on, used up as
/// at the login; wrong ones count against the same limit.
pub async fn verify_code(db: &Db, user: &User, code: &str) -> Result<()> {
	if !user.totp {
		return Ok(());
	}
	let (secret, last_step): (String, u64) = db.conn().query_row(
		"SELECT totp_secret, totp_step FROM users WHERE id = ?1",
		[user.id],
		|r| Ok((r.get(0)?, r.get(1)?)),
	)?;
	let key = format!("code:{}", user.id);
	let used = attempts_allowed(&key, MAX_CODE_FAILURES)
		.then(|| check_code(&secret, &user.email, code, last_step))
		.flatten()
		.map(|step| use_step(db, user.id, step))
		.transpose()?;
	if used != Some(true) {
		attempt_failed(&key, MAX_CODE_FAILURES);
		tokio::time::sleep(Duration::from_secs(1)).await;
		return Err(Error::Invalid("the two-factor code is wrong".into()));
	}

	CODE_FAILURES.lock().unwrap().remove(&key);
	Ok(())
}

/// Uses up the code of `step`, false if it or a later one was used in
/// between: parallel requests with the same code don't both get in.
fn use_step(db: &Db, user_id: i64, step: u64) -> rusqlite::Result<bool> {
	let changed = db.conn().execute(
		"UPDATE users SET totp_step = ?1 WHERE id = ?2 AND totp_step < ?1",
		params![step, user_id],
	)?;
	Ok(changed == 1)
}

/// A step-up for what a stolen session alone mustn't do: the session
/// already proves the password, so one more factor. With 2FA on that is
/// a code of the authenticator app, else the password; the other field
/// is ignored.
pub async fn verify_step_up(db: &Db, user: &User, req: &StepUp) -> Result<()> {
	if user.totp {
		verify_code(db, user, &req.code).await
	} else {
		verify_password(db, user, &req.password).await
	}
}

/// Confirming in the panel (Approvals) what a mail link confirms too: a
/// proposal or a pending action, with a step-up (`verify_step_up`).
/// Without 2FA the panel only rejects and the mail's link confirms; with
/// nothing `mailed` (mail is off) the password does.
pub async fn verify_approval(
	db: &Db,
	user: &User,
	req: &StepUp,
	mailed: bool,
) -> Result<()> {
	if !user.totp && mailed {
		return Err(Error::Invalid(NO_2FA.into()));
	}
	verify_step_up(db, user, req).await
}

pub const NO_2FA: &str = "Turn on two-factor login to confirm here, or use \
	the link in the mail.";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TotpSetup {
	secret: String,
	/// the qr code as png, base64
	qr: String,
	url: String,
}

/// A new secret for the authenticator app, stored when `enable_totp` gets
/// a valid code for it.
async fn setup_totp(authed: Authed) -> Result<Json<TotpSetup>> {
	let secret = Secret::generate_secret().to_encoded().to_string();
	let totp = totp(&secret, &authed.user.email)
		.ok_or(Error::Internal("totp".into()))?;
	Ok(Json(TotpSetup {
		qr: totp.get_qr_base64().map_err(Error::Internal)?,
		url: totp.get_url(),
		secret,
	}))
}

#[derive(Debug, Deserialize)]
struct EnableTotpReq {
	secret: String,
	code: String,
	password: String,
}

async fn enable_totp(
	authed: Authed,
	State(db): State<Db>,
	Json(req): Json<EnableTotpReq>,
) -> Result<Json<()>> {
	verify_password(&db, &authed.user, &req.password).await?;
	let Some(step) = check_code(&req.secret, &authed.user.email, &req.code, 0)
	else {
		return Err(Error::Invalid(
			"the code doesn't match, check the time of the phone".into(),
		));
	};
	// the code is used up, the next login needs a new one
	db.conn().execute(
		"UPDATE users SET totp_secret = ?1, totp_step = ?2 WHERE id = ?3",
		params![req.secret, step, authed.user.id],
	)?;
	Ok(Json(()))
}

async fn disable_totp(
	authed: Authed,
	State(db): State<Db>,
	Json(req): Json<StepUp>,
) -> Result<Json<()>> {
	if !authed.user.totp {
		return Err(Error::Invalid("two-factor login is off".into()));
	}
	verify_step_up(&db, &authed.user, &req).await?;
	db.conn().execute(
		"UPDATE users SET totp_secret = NULL WHERE id = ?1",
		[authed.user.id],
	)?;
	Ok(Json(()))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/login", post(login))
		.route("/me", get(me))
		.route("/logout", post(logout))
		.route("/tickets", post(create_ticket))
		.route("/password", post(change_password))
		.route("/account", post(change_account))
		.route(
			"/email/{token}",
			get(get_email_link).post(confirm_email_link),
		)
		.route("/totp/setup", post(setup_totp))
		.route("/totp/enable", post(enable_totp))
		.route("/totp/disable", post(disable_totp))
		.route("/", get(list_users))
		.route("/add", post(add))
		.route("/invite", post(invite))
		.route("/invite/{token}", get(get_invite).post(accept_invite))
		.route("/{id}/invite", post(reinvite))
		.route("/{id}", delete(delete_user))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_ticket_opens_one_download_once() {
		let path = "/api/databases/shop/dump";
		let t = new_ticket("session-hash", path);
		assert!(redeem(&t, "/api/databases/other/dump").is_none());
		assert_eq!(redeem(&t, path).as_deref(), Some("session-hash"));
		assert!(redeem(&t, path).is_none());
		// the activity log still finds who it was
		assert_eq!(ticket_session(&t).as_deref(), Some("session-hash"));
		assert!(takes_ticket("/api/groups/1/files/download"));
		assert!(takes_ticket("/api/events"));
		assert!(!takes_ticket("/api/users/me"));
	}

	fn parts(uri: &str) -> Parts {
		axum::http::Request::get(uri)
			.body(())
			.unwrap()
			.into_parts()
			.0
	}

	#[test]
	fn a_ticket_opens_the_event_stream_once() {
		let t = new_ticket("session-hash", "/api/events");
		let url = format!("/api/events?ticket={t}");
		assert!(
			session_hash(&parts(&format!("/api/users/me?ticket={t}"))).is_err()
		);
		assert_eq!(session_hash(&parts(&url)).unwrap(), "session-hash");
		assert!(session_hash(&parts(&url)).is_err());

		// an expired one doesn't
		let t = new_ticket("session-hash", "/api/events");
		TICKETS.lock().unwrap().get_mut(&t).unwrap().expires =
			std::time::Instant::now() - Duration::from_secs(1);
		assert!(
			session_hash(&parts(&format!("/api/events?ticket={t}"))).is_err()
		);

		// the session token itself stays out of URLs
		assert!(session_hash(&parts("/api/events?token=abc")).is_err());
	}

	#[test]
	fn emails_are_ones_mail_can_go_to() {
		let ok = normalize(" A.B@Example.ch\n", " Anna ").unwrap();
		assert_eq!(ok, ("a.b@example.ch".into(), "Anna".into()));
		for bad in ["a", "a b@example.ch", "a@example.ch\nb@x.ch", "@x.ch"] {
			assert!(normalize(bad, "A").is_err(), "{bad:?}");
		}
		assert!(normalize("a@example.ch", " ").is_err());
	}

	fn auth(password: &str, code: &str) -> StepUp {
		StepUp {
			password: password.into(),
			code: code.into(),
		}
	}

	/// An admin with 2FA and its own id, wrong codes of other tests
	/// don't count against it.
	fn admin_with_2fa(db: &Db, id: i64, email: &str) -> (User, String) {
		let hash = hash_password("admin password").unwrap();
		let a = create(&db.conn(), email, "A", &hash).unwrap();
		let secret = Secret::generate_secret().to_encoded().to_string();
		db.conn()
			.execute(
				"UPDATE users SET id = ?1, totp_secret = ?2 WHERE id = ?3",
				params![id, secret, a.id],
			)
			.unwrap();
		(by_id(&db.conn(), id).unwrap(), secret)
	}

	/// the code of 30 s before, now or after: three codes to use up
	fn code_at(secret: &str, email: &str, offset: i64) -> String {
		let t = (now() / 1000 + offset) as u64;
		totp(secret, email).unwrap().generate(t)
	}

	fn add_req(email: &str, new: &str) -> AddReq {
		AddReq {
			email: email.into(),
			name: "New".into(),
			new_password: new.into(),
			auth: auth("admin password", ""),
		}
	}

	#[tokio::test]
	async fn an_admin_adds_one_with_a_password() {
		let db = Db::memory();
		let cfg: Arc<Config> = Arc::new(toml::from_str("").unwrap());
		let hash = hash_password("admin password").unwrap();
		let admin = create(&db.conn(), "admin@example.ch", "Admin", &hash);
		let authed = || Authed {
			token: String::new(),
			user: admin.as_ref().unwrap().clone(),
		};
		let add_one = |req| {
			add(authed(), State(cfg.clone()), State(db.clone()), Json(req))
		};

		let weak = add_one(add_req("a@example.ch", "short"));
		assert!(weak.await.is_err());
		let mut wrong = add_req("a@example.ch", "long enough 1");
		wrong.auth.password = "wrong".into();
		assert!(add_one(wrong).await.is_err());
		let broken = add_req("a b@example.ch", "long enough 1");
		assert!(add_one(broken).await.is_err());
		assert_eq!(list(&db.conn()).unwrap().len(), 1);

		let user = add_one(add_req(" A@example.ch", "long enough 1"))
			.await
			.unwrap()
			.0;
		assert_eq!(user.email, "a@example.ch");
		assert!(!user.invited);
		let found = check_credentials(
			&db,
			"a@example.ch",
			"long enough 1".into(),
			"test",
		)
		.await
		.unwrap();
		assert_eq!(found.0, user.id);

		let twice = add_req("a@example.ch", "long enough 2");
		assert!(add_one(twice).await.is_err());
	}

	#[test]
	fn a_new_email_counts_after_its_link() {
		let db = Db::memory();
		let conn = db.conn();
		let hash = hash_password("admin password").unwrap();
		let a = create(&conn, "a@example.ch", "A", &hash).unwrap();
		let b = create(&conn, "b@example.ch", "B", &hash).unwrap();

		assert!(request_email(&conn, a.id, "b@example.ch").is_err());

		// only once, and only for the admin who asked
		let token = request_email(&conn, a.id, "new@example.ch").unwrap();
		assert_eq!(by_id(&conn, a.id).unwrap().email, "a@example.ch");
		assert!(confirm_email(&conn, b.id, &token).is_err());
		assert!(confirm_email(&conn, a.id, "wrong").is_err());
		let user = confirm_email(&conn, a.id, &token).unwrap();
		assert_eq!(user.email, "new@example.ch");
		assert!(confirm_email(&conn, a.id, &token).is_err());

		// a later link replaces the earlier one
		let first = request_email(&conn, a.id, "x@example.ch").unwrap();
		let token = request_email(&conn, a.id, "y@example.ch").unwrap();
		assert!(confirm_email(&conn, a.id, &first).is_err());

		// expired
		conn.execute("UPDATE users SET new_email_expires = ?1", [now() - 1])
			.unwrap();
		assert!(confirm_email(&conn, a.id, &token).is_err());

		// taken by another admin in between
		let token = request_email(&conn, a.id, "z@example.ch").unwrap();
		create(&conn, "z@example.ch", "Z", &hash).unwrap();
		assert!(confirm_email(&conn, a.id, &token).is_err());
		assert_eq!(by_id(&conn, a.id).unwrap().email, "new@example.ch");
	}

	#[tokio::test]
	async fn a_new_email_needs_the_code_with_2fa() {
		let db = Db::memory();
		let hash = hash_password("admin password").unwrap();
		let a = create(&db.conn(), "a@example.ch", "A", &hash).unwrap();
		verify_code(&db, &a, "").await.unwrap();

		let secret = Secret::generate_secret().to_encoded().to_string();
		db.conn()
			.execute("UPDATE users SET totp_secret = ?1", [&secret])
			.unwrap();
		let a = by_id(&db.conn(), a.id).unwrap();
		assert!(verify_code(&db, &a, "").await.is_err());
		let code = totp(&secret, &a.email)
			.unwrap()
			.generate((now() / 1000) as u64);
		verify_code(&db, &a, &code).await.unwrap();
		// used up
		assert!(verify_code(&db, &a, &code).await.is_err());
	}

	#[tokio::test]
	async fn without_mail_no_invite_and_the_email_changes_directly() {
		let db = Db::memory();
		let cfg: Arc<Config> =
			Arc::new(toml::from_str("domain = 'panel.example.ch'").unwrap());
		crate::db::set_setting(&db.conn(), "mail", r#"{"mode":"off"}"#)
			.unwrap();
		let hash = hash_password("admin password").unwrap();
		let admin = create(&db.conn(), "a@example.ch", "A", &hash).unwrap();
		let authed = || Authed {
			token: String::new(),
			user: admin.clone(),
		};

		let req = InviteReq {
			email: "b@example.ch".into(),
			name: "B".into(),
			auth: auth("admin password", ""),
		};
		let invited =
			invite(authed(), State(cfg.clone()), State(db.clone()), Json(req));
		assert!(
			invited
				.await
				.unwrap_err()
				.to_string()
				.contains("mail is off")
		);
		assert_eq!(list(&db.conn()).unwrap().len(), 1);
		let resent = send_invite(&cfg, &db, &admin, "A").await;
		assert!(resent.is_err());

		let req = AccountReq {
			email: "new@example.ch".into(),
			name: "A".into(),
			auth: auth("admin password", ""),
		};
		let resp =
			change_account(authed(), State(cfg), State(db.clone()), Json(req))
				.await
				.unwrap()
				.0;
		assert_eq!(resp.confirm_sent, None);
		assert_eq!(resp.user.email, "new@example.ch");
	}

	#[tokio::test]
	async fn sensitive_actions_need_the_code_with_2fa() {
		let db = Db::memory();
		let cfg: Arc<Config> = Arc::new(toml::from_str("").unwrap());
		let (admin, secret) = admin_with_2fa(&db, 701, "2fa@example.ch");
		// the codes count on one 30 s step: start early in a step, the
		// password hashes take a few seconds
		let into_step = now() / 1000 % 30;
		if into_step >= 15 {
			let rest = (30 - into_step) as u64;
			tokio::time::sleep(std::time::Duration::from_secs(rest)).await;
		}
		let code = |offset| code_at(&secret, &admin.email, offset);
		let authed = || Authed {
			token: String::new(),
			user: admin.clone(),
		};
		let is_code_error = |r: Result<Json<()>>| {
			r.unwrap_err()
				.to_string()
				.contains("two-factor code is wrong")
		};

		let mut req = add_req("new@example.ch", "long enough 1");
		let added =
			add(authed(), State(cfg.clone()), State(db.clone()), Json(req));
		assert!(added.await.unwrap_err().to_string().contains("two-factor"));
		assert_eq!(list(&db.conn()).unwrap().len(), 1);
		req = add_req("new@example.ch", "long enough 1");
		req.auth.code = code(-30);
		let new =
			add(authed(), State(cfg.clone()), State(db.clone()), Json(req))
				.await
				.unwrap()
				.0;

		let deleted = delete_user(
			authed(),
			State(db.clone()),
			Path(new.id),
			Json(auth("admin password", "")),
		);
		assert!(is_code_error(deleted.await));
		let req = InviteReq {
			email: "b@example.ch".into(),
			name: "B".into(),
			auth: auth("admin password", ""),
		};
		let invited =
			invite(authed(), State(cfg.clone()), State(db.clone()), Json(req));
		assert!(
			invited
				.await
				.unwrap_err()
				.to_string()
				.contains("two-factor")
		);
		let resent = reinvite(
			authed(),
			State(cfg.clone()),
			State(db.clone()),
			Path(new.id),
			Json(auth("admin password", "")),
		);
		assert!(is_code_error(resent.await));
		assert_eq!(list(&db.conn()).unwrap().len(), 2);

		delete_user(
			authed(),
			State(db.clone()),
			Path(new.id),
			Json(auth("", &code(0))),
		)
		.await
		.unwrap()
		.0;
		assert_eq!(list(&db.conn()).unwrap().len(), 1);

		let off = |code: String| {
			disable_totp(
				authed(),
				State(db.clone()),
				Json(auth("admin password", &code)),
			)
		};
		assert!(is_code_error(off(String::new()).await));
		// used up by the deletion
		assert!(is_code_error(off(code(0)).await));
		off(code(30)).await.unwrap().0;
		assert!(!by_id(&db.conn(), admin.id).unwrap().totp);
	}

	#[tokio::test]
	async fn a_step_up_asks_one_factor() {
		let db = Db::memory();
		let (admin, secret) = admin_with_2fa(&db, 711, "step@example.ch");
		let code = code_at(&secret, &admin.email, 0);

		// with 2FA the code alone, the password doesn't help
		let wrong = auth("admin password", "");
		assert!(verify_step_up(&db, &admin, &wrong).await.is_err());
		let failures = CODE_FAILURES.lock().unwrap().get("code:711").copied();
		assert_eq!(failures.map(|f| f.0), Some(1));
		verify_step_up(&db, &admin, &auth("wrong", &code))
			.await
			.unwrap();
		assert!(!CODE_FAILURES.lock().unwrap().contains_key("code:711"));
		// used up
		let again = auth("", &code);
		assert!(verify_step_up(&db, &admin, &again).await.is_err());

		// without 2FA the password, the code doesn't matter
		let hash = hash_password("other password").unwrap();
		let b = create(&db.conn(), "b@example.ch", "B", &hash).unwrap();
		let wrong = auth("", &code);
		assert!(verify_step_up(&db, &b, &wrong).await.is_err());
		verify_step_up(&db, &b, &auth("other password", ""))
			.await
			.unwrap();
	}

	#[test]
	fn tries_while_locked_dont_restart_the_count() {
		let key = "test:lock";
		for _ in 0..MAX_CODE_FAILURES {
			attempt_failed(key, MAX_CODE_FAILURES);
		}
		assert!(!attempts_allowed(key, MAX_CODE_FAILURES));
		// a refused try counts too, the lock holds
		attempt_failed(key, MAX_CODE_FAILURES);
		assert!(!attempts_allowed(key, MAX_CODE_FAILURES));
		CODE_FAILURES.lock().unwrap().remove(key);
	}

	#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
	async fn parallel_requests_cant_use_a_code_twice() {
		let db = Db::memory();
		let (admin, secret) = admin_with_2fa(&db, 702, "par@example.ch");
		let code = code_at(&secret, &admin.email, 0);

		let tries: Vec<_> = (0..8)
			.map(|_| {
				let (db, admin, code) =
					(db.clone(), admin.clone(), code.clone());
				tokio::spawn(
					async move { verify_code(&db, &admin, &code).await },
				)
			})
			.collect();
		let mut ok = 0;
		for t in tries {
			ok += t.await.unwrap().is_ok() as u32;
		}
		assert_eq!(ok, 1);

		// the database refuses the step a request checked against an old
		// state
		let step = check_code(&secret, &admin.email, &code, 0).unwrap();
		assert!(!use_step(&db, admin.id, step).unwrap());
		assert!(use_step(&db, admin.id, step + 1).unwrap());
	}

	#[test]
	fn a_code_works_once() {
		let secret = Secret::generate_secret().to_encoded().to_string();
		let t = totp(&secret, "a@b.ch").unwrap();
		let code = t.generate((now() / 1000) as u64);

		let step = check_code(&secret, "a@b.ch", &code, 0).unwrap();
		assert!(check_code(&secret, "a@b.ch", &code, step).is_none());
		assert!(check_code(&secret, "a@b.ch", "000000x", 0).is_none());
	}
}
