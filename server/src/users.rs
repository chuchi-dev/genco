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
const MAX_CODE_FAILURES: u32 = 5;
const MAX_PASSWORD_FAILURES: u32 = 10;
const CODE_LOCK_MS: i64 = 1000 * 60 * 15;

/// wrong 2FA codes (`code:<user id>`) and passwords (`password:<email>`):
/// how many and when the last was. nginx limits per IP, this per account,
/// also for a client that reaches genco without nginx.
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
	let email = email.trim().to_lowercase();
	if !email.contains('@') {
		return Err(Error::Invalid("invalid email".into()));
	}

	conn.execute(
		"INSERT INTO users (email, name, password_hash, created_at)
		VALUES (?1, ?2, ?3, ?4)",
		params![email, name.trim(), hash, now()],
	)?;

	Ok(by_id(conn, conn.last_insert_rowid())?)
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
/// `session-token` header. The `token` query param only counts for what a
/// browser opens itself: the event stream and downloads.
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
		let user = conn.query_row(
			&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
			[user_id],
			User::from_row,
		)?;

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
/// event stream (EventSource can't send a header) from the query, or for
/// a download from its ticket (the token stays out of the browser's
/// download list).
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
	if path == "/api/events" {
		return Ok(tokens::hash(value("token")?));
	}
	if !is_download(&path) {
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

/// A download link: a database dump or files.
fn is_download(path: &str) -> bool {
	path.starts_with("/api/")
		&& (path.ends_with("/files/download") || path.ends_with("/dump"))
}

/// A single-use link to one download, for a minute.
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
	/// for it, `/api/groups/1/files/download?path=a.txt`
	path: String,
}

async fn create_ticket(
	authed: Authed,
	Json(req): Json<TicketReq>,
) -> Result<Json<String>> {
	let path = req.path.split('?').next().unwrap_or("");
	if !is_download(path) {
		return Err(Error::Invalid("no download".into()));
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
	// genco only listens on localhost, nginx appends the real client last
	let ip = header("x-forwarded-for")
		.and_then(|v| v.rsplit(',').next())
		.map(str::trim);
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
	let step = attempts_allowed(&key, MAX_CODE_FAILURES)
		.then(|| check_code(secret, email, code, last_step))
		.flatten();
	let Some(step) = step else {
		let why = "Login, wrong two-factor code";
		return Err(reject(db, email, from, why, &key, MAX_CODE_FAILURES).await);
	};

	CODE_FAILURES.lock().unwrap().remove(&key);
	db.conn().execute(
		"UPDATE users SET totp_step = ?1 WHERE id = ?2",
		params![step, user_id],
	)?;
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
	let user = conn.query_row(
		&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
		[user_id],
		User::from_row,
	)?;
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
	Json(req): Json<PasswordOnly>,
) -> Result<Json<()>> {
	if id == authed.user.id {
		return Err(Error::Invalid("you can't delete yourself".into()));
	}
	verify_password(&db, &authed.user, &req.password).await?;
	db.conn().execute("DELETE FROM users WHERE id = ?1", [id])?;
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
struct PasswordReq {
	current: String,
	password: String,
}

/// The admin's own password, for what a stolen session alone mustn't do
/// (two-factor, invites, tokens). It counts against the same limit as the
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

/// What a dangerous request sends along: the admin's password.
#[derive(Debug, Deserialize)]
pub struct PasswordOnly {
	pub password: String,
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

/// how long the link of an invite works
const INVITE_MS: i64 = 3 * 24 * 60 * 60 * 1000;

#[derive(Debug, Deserialize)]
struct InviteReq {
	email: String,
	name: String,
	/// the inviting admin's: a new admin is lasting access
	password: String,
}

/// Adds an admin without a password, the mail has a link to set one.
async fn invite(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<InviteReq>,
) -> Result<Json<User>> {
	verify_password(&db, &authed.user, &req.password).await?;
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

/// A new link for an admin who lost the first one or the password.
async fn reinvite(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Path(id): Path<i64>,
	Json(req): Json<PasswordOnly>,
) -> Result<Json<()>> {
	verify_password(&db, &authed.user, &req.password).await?;
	let user = db.conn().query_row(
		&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
		[id],
		User::from_row,
	)?;
	send_invite(&cfg, &db, &user, &authed.user.name).await?;
	Ok(Json(()))
}

async fn send_invite(
	cfg: &Config,
	db: &Db,
	user: &User,
	by: &str,
) -> Result<()> {
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
fn attempts_allowed(key: &str, max: u32) -> bool {
	match CODE_FAILURES.lock().unwrap().get(key) {
		Some((n, last)) => *n < max || last + CODE_LOCK_MS < now(),
		None => true,
	}
}

fn attempt_failed(key: &str, max: u32) {
	let mut failures = CODE_FAILURES.lock().unwrap();
	// unknown emails would grow the map without end
	failures.retain(|_, (_, last)| *last + CODE_LOCK_MS > now());
	let entry = failures.entry(key.to_string()).or_insert((0, 0));
	// after the wait the count starts over
	if entry.0 >= max {
		entry.0 = 0;
	}
	*entry = (entry.0 + 1, now());
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

#[derive(Debug, Deserialize)]
struct DisableTotpReq {
	code: String,
	password: String,
}

async fn disable_totp(
	authed: Authed,
	State(db): State<Db>,
	Json(req): Json<DisableTotpReq>,
) -> Result<Json<()>> {
	verify_password(&db, &authed.user, &req.password).await?;
	let secret: Option<String> = db.conn().query_row(
		"SELECT totp_secret FROM users WHERE id = ?1",
		[authed.user.id],
		|r| r.get(0),
	)?;
	let ok = secret.is_some_and(|s| {
		check_code(&s, &authed.user.email, &req.code, 0).is_some()
	});
	if !ok {
		return Err(Error::Invalid("the code doesn't match".into()));
	}
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
		.route("/totp/setup", post(setup_totp))
		.route("/totp/enable", post(enable_totp))
		.route("/totp/disable", post(disable_totp))
		.route("/", get(list_users))
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
		assert!(is_download("/api/groups/1/files/download"));
		assert!(!is_download("/api/events"));
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
