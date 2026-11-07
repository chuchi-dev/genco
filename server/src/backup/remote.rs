//! A copy of the backups off the server: after each full backup its new
//! snapshots go with `restic copy` into a repository at Backblaze B2 or an
//! S3-compatible storage, which keeps its own, longer history. Key and
//! repository password are sealed with the secrets key, the API only says
//! whether they are set, and restic gets them through its environment.
//! Where the copy goes decides where all data goes: the panel asks for the
//! step-up (the 2FA code, or the password), the agent's changes wait for
//! an admin's confirmation (actions.rs), and every change or stop is sent
//! to all admins.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::{PASSWORD_FILE, REPOSITORY};
use crate::AppState;
use crate::config::Config;
use crate::db::{Db, get_json, now, set_setting};
use crate::error::{Error, Result};
use crate::job::Log;
use crate::users::Authed;

const SETTING: &str = "backup:remote";
/// the last copy and the size, written by the backup
const STATE: &str = "backup:remote-state";
/// when and by whom the remote was turned off, a problem for a week
const OFF: &str = "backup:remote-off";
/// the repository password genco made on a confirmed request of the agent,
/// sealed until an admin looks at it
const NEW_PASSWORD: &str = "backup:remote-new-password";
const DAY: i64 = 24 * 60 * 60 * 1000;
const OFF_SHOWN: i64 = 7 * DAY;
/// the size reads the whole index, once a week is enough
const SIZE_EVERY: i64 = 7 * DAY;
/// a problem when no copy worked for this long
const OLD_AFTER: i64 = 2 * DAY;
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keep {
	pub daily: u32,
	pub weekly: u32,
	pub monthly: u32,
}

impl Default for Keep {
	fn default() -> Self {
		Self {
			daily: 14,
			weekly: 8,
			monthly: 12,
		}
	}
}

/// As stored, the secrets sealed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
	/// b2 or s3
	provider: String,
	bucket: String,
	/// inside the bucket, without slashes around it
	path: String,
	/// s3 only
	endpoint: String,
	region: String,
	/// B2's keyID or the S3 access key
	key_id: String,
	keep: Keep,
	/// KiB/s
	limit_upload: Option<u32>,
	configured_at: i64,
	secret: Vec<u8>,
	password: Vec<u8>,
}

/// What the panel and `genco backup remote show` see, no secrets.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
	provider: String,
	bucket: String,
	path: String,
	endpoint: String,
	region: String,
	key_id: String,
	keep: Keep,
	limit_upload: Option<u32>,
	configured_at: i64,
	pub location: String,
}

impl From<&Stored> for View {
	fn from(s: &Stored) -> Self {
		Self {
			provider: s.provider.clone(),
			bucket: s.bucket.clone(),
			path: s.path.clone(),
			endpoint: s.endpoint.clone(),
			region: s.region.clone(),
			key_id: s.key_id.clone(),
			keep: s.keep,
			limit_upload: s.limit_upload,
			configured_at: s.configured_at,
			location: repository(s),
		}
	}
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteState {
	pub last_ok: Option<i64>,
	pub last_error: Option<String>,
	pub last_error_at: Option<i64>,
	/// bytes in the remote repository, compressed and deduplicated
	pub size: Option<u64>,
	pub size_at: Option<i64>,
}

/// A change of the settings, from the panel or the CLI; sealed in the
/// request of the agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
	/// off, b2 or s3
	pub provider: String,
	#[serde(default)]
	pub bucket: String,
	/// empty is genco/<panel name>
	#[serde(default)]
	pub path: String,
	#[serde(default)]
	pub endpoint: String,
	#[serde(default)]
	pub region: String,
	#[serde(default)]
	pub key_id: String,
	/// empty keeps the stored one
	#[serde(default)]
	pub secret: String,
	/// the password of an existing remote repository (a new server that
	/// restores); empty keeps the stored one or makes a new one
	#[serde(default)]
	pub repo_password: String,
	#[serde(default)]
	pub keep: Keep,
	#[serde(default)]
	pub limit_upload: Option<u32>,
}

/// The remote with its secrets open, for restic.
pub struct Opened {
	stored: Stored,
	secret: String,
	password: String,
}

fn stored(conn: &Connection) -> Option<Stored> {
	get_json(conn, SETTING)
}

pub fn state(conn: &Connection) -> RemoteState {
	get_json(conn, STATE).unwrap_or_default()
}

fn set_state(conn: &Connection, state: &RemoteState) -> Result<()> {
	set_setting(conn, STATE, &serde_json::to_string(state).unwrap())?;
	Ok(())
}

pub fn view(conn: &Connection) -> Option<View> {
	stored(conn).as_ref().map(View::from)
}

#[derive(Debug, Serialize, Deserialize)]
struct Off {
	at: i64,
	by: String,
}

/// The remote with its secrets, an error when there is none.
pub fn required(db: &Db) -> Result<Opened> {
	open(&db.conn(), crate::secrets::key_file())?
		.ok_or_else(|| Error::Invalid("no remote backup is set up".into()))
}

/// The remote with its secrets, `None` when there is none.
pub fn open(conn: &Connection, key_file: &Path) -> Result<Option<Opened>> {
	let Some(stored) = stored(conn) else {
		return Ok(None);
	};
	let secret = crate::secrets::open_with(key_file, &stored.secret)?;
	let password = crate::secrets::open_with(key_file, &stored.password)?;
	Ok(Some(Opened {
		stored,
		secret,
		password,
	}))
}

/// `genco/<panel name>`, the hostname without one.
pub fn default_path(cfg: &Config) -> String {
	let name = cfg.domain.clone().unwrap_or_else(|| {
		std::fs::read_to_string("/etc/hostname")
			.map(|h| h.trim().to_string())
			.unwrap_or_else(|_| "server".into())
	});
	format!("genco/{name}")
}

fn repository(s: &Stored) -> String {
	url(&s.provider, &s.endpoint, &s.bucket, &s.path)
}

fn url(provider: &str, endpoint: &str, bucket: &str, path: &str) -> String {
	match provider {
		"b2" => format!("b2:{bucket}:{path}"),
		_ => format!("s3:{}/{bucket}/{path}", endpoint.trim_end_matches('/')),
	}
}

/// restic's environment for the remote: location, key and password.
fn env(o: &Opened) -> Vec<(&'static str, String)> {
	let s = &o.stored;
	let mut env = vec![
		("RESTIC_REPOSITORY", repository(s)),
		("RESTIC_PASSWORD", o.password.clone()),
	];
	match s.provider.as_str() {
		"b2" => {
			env.push(("B2_ACCOUNT_ID", s.key_id.clone()));
			env.push(("B2_ACCOUNT_KEY", o.secret.clone()));
		}
		_ => {
			env.push(("AWS_ACCESS_KEY_ID", s.key_id.clone()));
			env.push(("AWS_SECRET_ACCESS_KEY", o.secret.clone()));
			if !s.region.is_empty() {
				env.push(("AWS_DEFAULT_REGION", s.region.clone()));
			}
		}
	}
	env
}

/// The upload limit, a global flag of every restic command.
fn limit_args(s: &Stored) -> Vec<String> {
	match s.limit_upload {
		Some(kib) => vec!["--limit-upload".into(), kib.to_string()],
		None => vec![],
	}
}

/// How many snapshots the remote keeps, per group and kind.
fn keep_args(keep: &Keep) -> Vec<String> {
	let mut args = vec![];
	for (flag, n) in [
		("--keep-daily", keep.daily),
		("--keep-weekly", keep.weekly),
		("--keep-monthly", keep.monthly),
	] {
		if n > 0 {
			args.extend([flag.to_string(), n.to_string()]);
		}
	}
	args
}

impl Opened {
	/// The remote's environment on `cmd`; nothing of the local repository
	/// comes along from genco's own.
	fn setup(&self, cmd: &mut Command) {
		cmd.env_remove("RESTIC_PASSWORD_FILE")
			.env_remove("RESTIC_PASSWORD_COMMAND")
			.envs(env(self))
			.env("RESTIC_CACHE_DIR", "/var/cache/restic")
			.stdin(Stdio::null());
	}

	/// restic on the remote repository.
	pub fn restic(&self) -> Command {
		let mut cmd = Command::new("restic");
		self.setup(&mut cmd);
		cmd.args(limit_args(&self.stored));
		cmd
	}

	/// The key and password as ***, e.g. in restic's errors. Short ones
	/// would mask ordinary words, no real key is that short.
	pub fn hide(&self, text: &str) -> String {
		let mut text = text.to_string();
		for s in [&self.secret, &self.password] {
			if s.trim().len() >= 8 {
				text = text.replace(s.as_str(), "***");
			}
		}
		text
	}

	pub fn location(&self) -> String {
		repository(&self.stored)
	}

	/// restic's output, its error with the secrets hidden. It stops when
	/// the caller gives up, e.g. a browser that left.
	pub(super) async fn output(
		&self,
		args: &[&str],
	) -> std::result::Result<Vec<u8>, String> {
		let out = self
			.restic()
			.args(args)
			.kill_on_drop(true)
			.output()
			.await
			.map_err(|e| format!("restic: {e}"))?;
		if out.status.success() {
			return Ok(out.stdout);
		}
		Err(self.hide(&super::restic_error(&out.stderr)))
	}

	/// Creates the remote repository if there is none yet, true then. With
	/// the local one's chunker settings the copy deduplicates like it.
	async fn ensure(&self) -> std::result::Result<bool, String> {
		let read = self
			.restic()
			.args(["cat", "config"])
			.stdout(Stdio::null())
			.kill_on_drop(true)
			.output();
		let out = self
			.answer(read)
			.await?
			.map_err(|e| format!("restic: {e}"))?;
		if out.status.success() {
			return Ok(false);
		}
		let err = self.hide(String::from_utf8_lossy(&out.stderr).trim());
		if !missing(out.status.code(), &err) {
			return Err(err);
		}

		let mut args = vec!["init"];
		if PathBuf::from(REPOSITORY).join("config").exists() {
			args.extend([
				"--copy-chunker-params",
				"--from-repo",
				REPOSITORY,
				"--from-password-file",
				PASSWORD_FILE,
			]);
		}
		self.output(&args).await?;
		Ok(true)
	}

	/// `run` if the remote answers in a minute: restic retries a missing
	/// bucket or a dead endpoint for minutes.
	pub(super) async fn answer<T>(
		&self,
		run: impl Future<Output = T>,
	) -> std::result::Result<T, String> {
		within(CONNECT_TIMEOUT, &self.location(), run).await
	}

	/// Runs restic on the remote with its output and errors in the log.
	async fn log_run(
		&self,
		log: &mut Log,
		args: &[String],
	) -> std::result::Result<(), String> {
		let mut cmd = Command::new("sh");
		cmd.args(["-c", r#"exec restic "$@" 2>&1"#, "sh"]);
		self.setup(&mut cmd);
		cmd.args(limit_args(&self.stored)).args(args);
		log.run(cmd)
			.await
			.map_err(|e| format!("restic {}: {e}", args[0]))
	}
}

async fn within<T>(
	limit: std::time::Duration,
	location: &str,
	run: impl Future<Output = T>,
) -> std::result::Result<T, String> {
	tokio::time::timeout(limit, run).await.map_err(|_| {
		format!(
			"no answer from {location} in {} seconds: does the bucket exist, \
			is the key allowed to use it?",
			limit.as_secs()
		)
	})
}

/// Whether restic's error says there is no repository yet (exit code 10
/// since restic 0.17, the message before).
fn missing(code: Option<i32>, err: &str) -> bool {
	code == Some(10)
		|| err.contains("Is there a repository at the following location?")
}

fn strings(args: &[&str]) -> Vec<String> {
	args.iter().map(|a| a.to_string()).collect()
}

/// The snapshots of the local repository the remote doesn't have yet.
fn copy_args() -> Vec<String> {
	strings(&[
		"copy",
		"--quiet",
		"--tag",
		"genco",
		"--from-repo",
		REPOSITORY,
		"--from-password-file",
		PASSWORD_FILE,
	])
}

fn forget_args(keep: &Keep) -> Vec<String> {
	let mut args = strings(&[
		"forget",
		"--quiet",
		"--tag",
		"genco",
		"--group-by",
		"tags",
		"--prune",
	]);
	args.extend(keep_args(keep));
	args
}

/// After a good full backup: its new snapshots to the remote, then the
/// remote's old ones removed. A failure goes into the log and the
/// remote's state (a problem of its own), the local backup stays done.
pub async fn copy(db: &Db, log: &mut Log) {
	let opened = open(&db.conn(), crate::secrets::key_file());
	let opened = match opened {
		Ok(Some(o)) => o,
		Ok(None) => return,
		Err(e) => {
			failed(db, log, &e.to_string()).await;
			return;
		}
	};
	log.hide(&opened.secret);
	log.hide(&opened.password);
	log.line(&format!("== remote copy to {}", opened.location()))
		.await;

	if let Err(e) = copy_to(&opened, log).await {
		failed(db, log, &e).await;
		return;
	}

	let mut state = state(&db.conn());
	state.last_ok = Some(now());
	state.last_error = None;
	state.last_error_at = None;
	if state.size_at.is_none_or(|at| at < now() - SIZE_EVERY) {
		match opened.size().await {
			Ok(size) => {
				state.size = Some(size);
				state.size_at = Some(now());
			}
			Err(e) => log.line(&format!("remote size: {e}")).await,
		}
	}
	let saved = set_state(&db.conn(), &state);
	if let Err(e) = saved {
		log.line(&format!("remote state: {e}")).await;
	}
	log.line("remote copy done").await;
}

async fn copy_to(o: &Opened, log: &mut Log) -> std::result::Result<(), String> {
	if o.ensure().await? {
		log.line("created the remote repository").await;
	}
	// a killed restic leaves its lock behind there too
	let _ = o.log_run(log, &strings(&["unlock", "--quiet"])).await;
	o.log_run(log, &copy_args()).await?;
	log.line("removing old snapshots on the remote").await;
	o.log_run(log, &forget_args(&o.stored.keep)).await
}

async fn failed(db: &Db, log: &mut Log, e: &str) {
	log.line(&format!("remote copy failed: {e}")).await;
	let saved = {
		let conn = db.conn();
		let mut state = state(&conn);
		// the problem shows it, one line is enough
		let first = e.lines().next().unwrap_or(e);
		state.last_error = Some(first.chars().take(300).collect());
		state.last_error_at = Some(now());
		set_state(&conn, &state)
	};
	if let Err(e) = saved {
		log.line(&format!("remote state: {e}")).await;
	}
}

impl Opened {
	/// Bytes the remote repository takes.
	async fn size(&self) -> std::result::Result<u64, String> {
		#[derive(Deserialize)]
		struct Stats {
			total_size: u64,
		}
		let out = self
			.output(&["stats", "--mode", "raw-data", "--json", "--no-lock"])
			.await?;
		serde_json::from_slice::<Stats>(&out)
			.map(|s| s.total_size)
			.map_err(|e| format!("restic stats: {e}"))
	}

	/// Connects, creates the repository if there is none, says what is in
	/// it.
	pub async fn test(&self) -> std::result::Result<String, String> {
		let created = self.ensure().await?;
		let location = self.location();
		if created {
			return Ok(format!("created the repository {location}"));
		}
		let out = self.output(&["snapshots", "--json", "--no-lock"]).await?;
		let snaps = super::parse_snapshots(&out).map_err(|e| e.to_string())?;
		Ok(format!(
			"connected to {location}, {} snapshots",
			snaps.len()
		))
	}
}

// ------------------------------------------------------------- settings

/// Letters, digits and `extra`, for what goes into the repository URL.
fn plain(v: &str, extra: &str) -> bool {
	!v.is_empty()
		&& v.chars()
			.all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// No spaces or control characters, which can't be in a key.
fn one_word(v: &str) -> bool {
	!v.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The settings cleaned up, or what is wrong with them.
fn check(mut s: Settings, default_path: &str) -> Result<Settings> {
	let invalid = |m: &str| Err(Error::Invalid(m.into()));
	s.bucket = s.bucket.trim().into();
	s.key_id = s.key_id.trim().into();
	s.endpoint = s.endpoint.trim().trim_end_matches('/').into();
	s.region = s.region.trim().into();
	s.path = s.path.trim().trim_matches('/').into();
	if s.path.is_empty() {
		s.path = default_path.into();
	}

	match s.provider.as_str() {
		"b2" => {
			s.endpoint.clear();
			s.region.clear();
		}
		"s3" => {
			let host = s
				.endpoint
				.strip_prefix("https://")
				.or_else(|| s.endpoint.strip_prefix("http://"));
			if !host.is_some_and(|h| plain(h, ".-:")) {
				return invalid(
					"the endpoint is a URL like https://s3.example.com",
				);
			}
			if !s.region.is_empty() && !plain(&s.region, "-_") {
				return invalid("the region is a name like eu-central-1");
			}
		}
		_ => return invalid("the provider is b2, s3 or off"),
	}
	if s.bucket.len() < 3 || s.bucket.len() > 63 || !plain(&s.bucket, ".-") {
		return invalid(
			"the bucket is a name of 3 to 63 letters, digits, - and .",
		);
	}
	let path_ok = plain(&s.path, "._-/")
		&& s.path.split('/').all(|p| !p.is_empty() && p != "..");
	if !path_ok {
		return invalid("the path is like genco/server, without spaces");
	}
	if s.key_id.is_empty() || !one_word(&s.key_id) {
		return invalid("the key ID is missing");
	}
	if !one_word(&s.secret) || s.repo_password.chars().any(char::is_control) {
		return invalid("the key and password have no spaces or line breaks");
	}
	let k = s.keep;
	if k.daily + k.weekly + k.monthly == 0 {
		return invalid("the remote keeps at least one snapshot");
	}
	if k.daily > 1000 || k.weekly > 1000 || k.monthly > 1000 {
		return invalid("at most 1000 snapshots per kind");
	}
	if s.limit_upload == Some(0) {
		s.limit_upload = None;
	}
	Ok(s)
}

/// Stores the settings, `off` forgets them (the repository stays in the
/// bucket). Returns the repository password when it made a new one: it
/// is shown this once, the admin keeps it off the server.
pub fn save(
	conn: &Connection,
	key_file: &Path,
	s: Settings,
	default_path: &str,
) -> Result<Option<String>> {
	if s.provider == "off" {
		conn.execute(
			"DELETE FROM settings WHERE key IN (?1, ?2)",
			[SETTING, STATE],
		)?;
		return Ok(None);
	}
	let s = check(s, default_path)?;
	let old = stored(conn);

	// the same key keeps its secret when none is sent
	let same_key = old
		.as_ref()
		.filter(|o| o.provider == s.provider && o.key_id == s.key_id);
	let secret = match same_key {
		_ if !s.secret.is_empty() => {
			crate::secrets::seal_with(key_file, &s.secret)?
		}
		Some(o) => o.secret.clone(),
		None => {
			return Err(Error::Invalid(
				"the application key (secret) is missing".into(),
			));
		}
	};
	let mut generated = None;
	let password = match &old {
		_ if !s.repo_password.is_empty() => {
			crate::secrets::seal_with(key_file, &s.repo_password)?
		}
		Some(o) => o.password.clone(),
		None => {
			let p = crate::util::random_hex(24);
			let sealed = crate::secrets::seal_with(key_file, &p)?;
			generated = Some(p);
			sealed
		}
	};

	let mut new = Stored {
		provider: s.provider,
		bucket: s.bucket,
		path: s.path,
		endpoint: s.endpoint,
		region: s.region,
		key_id: s.key_id,
		keep: s.keep,
		limit_upload: s.limit_upload,
		configured_at: now(),
		secret,
		password,
	};
	// another place starts over, the old one's last copy says nothing
	match &old {
		Some(o) if repository(o) == repository(&new) => {
			new.configured_at = o.configured_at;
		}
		_ => {
			conn.execute("DELETE FROM settings WHERE key = ?1", [STATE])?;
		}
	}
	set_setting(conn, SETTING, &serde_json::to_string(&new).unwrap())?;
	Ok(generated)
}

/// Saves the settings like `save` and tells every admin and agent when the
/// backups go somewhere else or stop leaving the server, so it can't
/// happen silently. `by` did it.
pub async fn change(
	cfg: &Config,
	db: &Db,
	key_file: &Path,
	s: Settings,
	by: &str,
) -> Result<Option<String>> {
	let (generated, notice) = {
		let conn = db.conn();
		let before = view(&conn).map(|v| v.location);
		let generated = save(&conn, key_file, s, &default_path(cfg))?;
		let after = view(&conn).map(|v| v.location);
		(generated, changed(&conn, before, after, by, now())?)
	};
	if let Some(n) = notice {
		crate::notify::send(cfg, db, n).await;
	}
	Ok(generated)
}

/// The notice when the location changed; a stop is remembered for the
/// problem of the week after.
fn changed(
	conn: &Connection,
	before: Option<String>,
	after: Option<String>,
	by: &str,
	now: i64,
) -> Result<Option<crate::notify::Notice>> {
	if before == after {
		return Ok(None);
	}
	let check = "If none of you did this, look at the activity log and \
		change it back in Settings → Backups.";
	let (title, text) = match (&before, &after) {
		(Some(old), None) => {
			let off = Off {
				at: now,
				by: by.into(),
			};
			set_setting(conn, OFF, &serde_json::to_string(&off).unwrap())?;
			(
				"remote backups turned off".to_string(),
				format!(
					"{by} turned off the remote backups: no copy leaves the \
					server any more. The repository at {old} stays. {check}"
				),
			)
		}
		(_, Some(new)) => {
			conn.execute("DELETE FROM settings WHERE key = ?1", [OFF])?;
			let was = before
				.as_ref()
				.map(|o| format!(" instead of {o}"))
				.unwrap_or_default();
			(
				format!("remote backups now go to {new}"),
				format!(
					"{by} set the remote backups: after each night the \
					backups of all groups and of the server are copied to \
					{new}{was}. {check}"
				),
			)
		}
		(None, None) => return Ok(None),
	};
	Ok(Some(crate::notify::Notice {
		kind: "report",
		attention: true,
		title,
		text,
		url: None,
		proposal: None,
	}))
}

/// Asks an admin to confirm the agent's new settings: they are sealed in
/// the request, the agent's output only gets the request's message.
async fn request_set(
	cfg: &Config,
	db: &Db,
	key_file: &Path,
	s: Settings,
	by: &str,
) -> Result<String> {
	let s = check(s, &default_path(cfg))?;
	let new_password =
		stored(&db.conn()).is_none() && s.repo_password.is_empty();
	let description = format!(
		"Copy the backups of all groups and of the server to {} (key ID {}) \
		after each night.",
		url(&s.provider, &s.endpoint, &s.bucket, &s.path),
		s.key_id
	);
	let sealed = crate::secrets::seal_with(
		key_file,
		&serde_json::to_string(&s).unwrap(),
	)?;
	let req = crate::actions::ask(
		cfg,
		db,
		crate::actions::Kind::RemoteSet,
		0,
		&description,
		Some(sealed),
		by,
	)
	.await?;
	let mut msg = req.message;
	if new_password {
		msg += "\nOnce confirmed, the new remote repository password is \
			shown to the admins in Settings → Backups, not here.";
	}
	Ok(msg)
}

/// Runs a confirmed request of the agent: the sealed settings, `None`
/// turns the remote off.
pub async fn confirmed(
	cfg: &Config,
	db: &Db,
	key_file: &Path,
	sealed: Option<&[u8]>,
	by: &str,
	log: &mut Vec<String>,
) -> std::result::Result<(), String> {
	let s = match sealed {
		Some(sealed) => {
			let json = crate::secrets::open_with(key_file, sealed)
				.map_err(|e| e.to_string())?;
			serde_json::from_str(&json).map_err(|e| e.to_string())?
		}
		None => Settings {
			provider: "off".into(),
			..Default::default()
		},
	};
	let generated = change(cfg, db, key_file, s, by)
		.await
		.map_err(|e| e.to_string())?;
	match view(&db.conn()) {
		Some(v) => log
			.push(format!("the backups go to {} after each night", v.location)),
		None => log.push(
			"no more remote copies; the repository stays in the bucket".into(),
		),
	}
	if let Some(p) = generated {
		let sealed = crate::secrets::seal_with(key_file, &p)
			.map_err(|e| e.to_string())?;
		set_setting(
			&db.conn(),
			NEW_PASSWORD,
			&serde_json::to_string(&sealed).unwrap(),
		)
		.map_err(|e| e.to_string())?;
		log.push(
			"genco made the remote repository's password: open Settings → \
			Backups to see it once and keep it outside this server"
				.into(),
		);
	}
	Ok(())
}

/// What the minute check reports: the last copy failed, or none worked
/// for two days, or the remote was turned off in the last week. Key and
/// text of the problem.
pub fn problem(conn: &Connection, now: i64) -> Option<(&'static str, String)> {
	let Some(s) = stored(conn) else {
		let off: Off = get_json(conn, OFF)?;
		return (off.at > now - OFF_SHOWN).then(|| {
			(
				"backup:remote-off",
				format!(
					"the remote backups were turned off by {}, no copy \
					leaves the server",
					off.by
				),
			)
		});
	};
	let st = state(conn);
	if let (Some(at), Some(e)) = (st.last_error_at, &st.last_error)
		&& st.last_ok.is_none_or(|ok| ok < at)
	{
		return Some((
			"backup:remote-failed",
			format!("the copy to the remote backup failed: {e}"),
		));
	}
	let since = st.last_ok.unwrap_or(s.configured_at);
	(since < now - OLD_AFTER).then(|| {
		(
			"backup:remote-old",
			"no copy to the remote backup for more than 2 days".into(),
		)
	})
}

/// For the server's status: where the copy goes, when it last worked.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
	provider: String,
	last_copy: Option<i64>,
	last_error_at: Option<i64>,
	size: Option<u64>,
}

pub fn summary(conn: &Connection) -> Option<Summary> {
	let s = stored(conn)?;
	let st = state(conn);
	Some(Summary {
		provider: s.provider,
		last_copy: st.last_ok,
		last_error_at: st.last_error_at,
		size: st.size,
	})
}

// ------------------------------------------------------------------- api

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
	remote: Option<View>,
	state: RemoteState,
	default_path: String,
	/// genco made a repository password on the agent's request, an admin
	/// hasn't seen it yet
	new_password: bool,
}

async fn get_settings(
	_authed: Authed,
	State(cfg): State<std::sync::Arc<Config>>,
	State(db): State<Db>,
) -> Result<Json<SettingsView>> {
	let conn = db.conn();
	Ok(Json(SettingsView {
		remote: view(&conn),
		state: state(&conn),
		default_path: default_path(&cfg),
		new_password: get_json::<Vec<u8>>(&conn, NEW_PASSWORD).is_some(),
	}))
}

#[derive(Debug, Deserialize)]
struct SaveReq {
	#[serde(flatten)]
	settings: Settings,
	/// the admin's step-up: it decides where all data goes
	#[serde(flatten)]
	auth: crate::users::StepUp,
}

async fn save_settings(
	authed: Authed,
	State(cfg): State<std::sync::Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<SaveReq>,
) -> Result<Json<Option<String>>> {
	crate::users::verify_step_up(&db, &authed.user, &req.auth).await?;
	let key_file = crate::secrets::key_file();
	let by = &authed.user.name;
	let generated = change(&cfg, &db, key_file, req.settings, by).await?;
	Ok(Json(generated))
}

/// The repository password genco made on the agent's request, shown this
/// once.
async fn new_password(
	authed: Authed,
	State(db): State<Db>,
	Json(auth): Json<crate::users::StepUp>,
) -> Result<Json<String>> {
	crate::users::verify_step_up(&db, &authed.user, &auth).await?;
	let conn = db.conn();
	let sealed: Vec<u8> = get_json(&conn, NEW_PASSWORD)
		.ok_or_else(|| Error::Invalid("it was shown already".into()))?;
	let password =
		crate::secrets::open_with(crate::secrets::key_file(), &sealed)?;
	conn.execute("DELETE FROM settings WHERE key = ?1", [NEW_PASSWORD])?;
	Ok(Json(password))
}

async fn test_route(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<String>> {
	required(&db)?
		.test()
		.await
		.map(Json)
		.map_err(Error::Invalid)
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/settings/backup", get(get_settings).post(save_settings))
		.route("/settings/backup/new-password", post(new_password))
		.route("/settings/backup/test", post(test_route))
}

// ------------------------------------------------------------------- cli

#[derive(Debug, clap::Subcommand)]
pub enum RemoteCmd {
	/// Copy the backups to Backblaze B2 or an S3-compatible storage after
	/// each night. stdin: the application key (B2) or secret key (S3); a
	/// second line is the password of an existing remote repository (a
	/// new server restoring), without one genco makes a new one and
	/// prints it once. Run by the agent, an admin confirms it first.
	Set {
		#[clap(long, required_unless_present = "s3")]
		b2: bool,
		#[clap(long, conflicts_with = "b2")]
		s3: bool,
		/// B2's keyID or the S3 access key
		#[clap(long)]
		key_id: String,
		#[clap(long)]
		bucket: String,
		/// inside the bucket, genco/<panel name> by default
		#[clap(long)]
		path: Option<String>,
		/// S3 only: https://s3.example.com
		#[clap(long, default_value = "")]
		endpoint: String,
		#[clap(long, default_value = "")]
		region: String,
		#[clap(long, default_value_t = 14)]
		keep_daily: u32,
		#[clap(long, default_value_t = 8)]
		keep_weekly: u32,
		#[clap(long, default_value_t = 12)]
		keep_monthly: u32,
		/// KiB/s
		#[clap(long)]
		limit_upload: Option<u32>,
	},
	/// Connect and create the remote repository if there is none
	Test,
	/// The settings and the last copy, without key or password
	Show,
	/// Stop copying; the repository stays in the bucket. Run by the agent,
	/// an admin confirms it first.
	Off,
}

/// `actor` runs it (`activity::cli_actor`): the agent's `set` and `off`
/// wait for an admin's confirmation, root in a terminal changes it
/// directly. Every admin hears of it either way.
pub async fn cli(
	cfg: &Config,
	db: &Db,
	cmd: RemoteCmd,
	stdin: impl FnOnce() -> std::result::Result<String, String>,
	actor: &str,
	key_file: &Path,
) -> Result<String> {
	let agent = actor == "agent";
	let by = format!("the {actor}");
	match cmd {
		RemoteCmd::Set {
			b2: _,
			s3,
			key_id,
			bucket,
			path,
			endpoint,
			region,
			keep_daily,
			keep_weekly,
			keep_monthly,
			limit_upload,
		} => {
			let input = stdin().map_err(Error::Invalid)?;
			let mut lines = input.lines();
			let secret = lines.next().unwrap_or("").trim().to_string();
			let repo_password = lines.next().unwrap_or("").to_string();
			let settings = Settings {
				provider: if s3 { "s3" } else { "b2" }.into(),
				bucket,
				path: path.unwrap_or_default(),
				endpoint,
				region,
				key_id,
				secret,
				repo_password,
				keep: Keep {
					daily: keep_daily,
					weekly: keep_weekly,
					monthly: keep_monthly,
				},
				limit_upload,
			};
			if agent {
				return request_set(cfg, db, key_file, settings, &by).await;
			}
			let generated = change(cfg, db, key_file, settings, &by).await?;
			let location = view(&db.conn()).map(|v| v.location);
			let mut msg = format!(
				"the backups go to {} after each night",
				location.unwrap_or_default()
			);
			if let Some(p) = generated {
				msg += &format!(
					"\nthe remote repository's password, shown only this \
					once:\n{p}\nkeep it outside this server (a password \
					manager): without it the remote copy can't be read when \
					the server is lost"
				);
			}
			Ok(msg)
		}
		RemoteCmd::Test => required(db)?.test().await.map_err(Error::Invalid),
		RemoteCmd::Show => {
			// one lock at a time: both in one expression deadlocks
			let Some(v) = view(&db.conn()) else {
				return Ok("no remote backup".into());
			};
			let st = state(&db.conn());
			let time = |t: Option<i64>| async move {
				match t {
					Some(t) => crate::util::format_time(t).await,
					None => "never".into(),
				}
			};
			let mut out = format!(
				"{}\nkey ID {}\nkeeps {} daily, {} weekly, {} monthly\n\
				upload limit {}\nlast copy {}",
				v.location,
				v.key_id,
				v.keep.daily,
				v.keep.weekly,
				v.keep.monthly,
				v.limit_upload
					.map_or("none".into(), |k| format!("{k} KiB/s")),
				time(st.last_ok).await,
			);
			if let Some(e) = &st.last_error {
				out += &format!(
					"\nlast failure {}: {e}",
					time(st.last_error_at).await
				);
			}
			if let Some(size) = st.size {
				out += &format!("\nsize {} MiB", size / (1024 * 1024));
			}
			Ok(out)
		}
		RemoteCmd::Off if agent => {
			let req = crate::actions::request(
				cfg,
				db,
				crate::actions::Kind::RemoteOff,
				0,
				&by,
			)
			.await?;
			Ok(req.message)
		}
		RemoteCmd::Off => {
			if view(&db.conn()).is_none() {
				return Ok("no remote backup".into());
			}
			let s = Settings {
				provider: "off".into(),
				..Default::default()
			};
			change(cfg, db, key_file, s, &by).await?;
			Ok(
				"no more remote copies, the admins are told; the repository \
				stays in the bucket"
					.into(),
			)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn key_file() -> PathBuf {
		std::env::temp_dir()
			.join(format!("genco-remote-key-{}", rand::random::<u32>()))
	}

	fn b2() -> Settings {
		Settings {
			provider: "b2".into(),
			bucket: "acme-backups".into(),
			key_id: "0012ab".into(),
			secret: "K001secret".into(),
			..Default::default()
		}
	}

	#[test]
	fn settings_are_checked_and_cleaned() {
		let s = check(b2(), "genco/panel.example.ch").unwrap();
		assert_eq!(s.path, "genco/panel.example.ch");

		let mut s = b2();
		s.path = "/backups/x/".into();
		s.limit_upload = Some(0);
		let s = check(s, "d").unwrap();
		assert_eq!(s.path, "backups/x");
		assert_eq!(s.limit_upload, None);

		let bad = [
			Settings {
				provider: "ftp".into(),
				..b2()
			},
			Settings {
				path: "a/../b".into(),
				..b2()
			},
			Settings {
				bucket: "a b".into(),
				..b2()
			},
			Settings {
				bucket: "acme_backups".into(),
				..b2()
			},
			Settings {
				secret: "two\nlines".into(),
				..b2()
			},
			Settings {
				key_id: "".into(),
				..b2()
			},
			Settings {
				keep: Keep {
					daily: 0,
					weekly: 0,
					monthly: 0,
				},
				..b2()
			},
			Settings {
				provider: "s3".into(),
				endpoint: "s3.example.com".into(),
				..b2()
			},
		];
		for s in bad {
			assert!(check(s.clone(), "d").is_err(), "{s:?}");
		}
	}

	#[test]
	fn secrets_are_sealed_and_never_in_the_view() {
		let db = Db::memory();
		let conn = db.conn();
		let key = key_file();
		let generated = save(&conn, &key, b2(), "genco/x").unwrap();
		let password = generated.expect("a new password");

		let raw = crate::db::get_setting(&conn, SETTING).unwrap().unwrap();
		assert!(!raw.contains("K001secret") && !raw.contains(&password));
		let json = serde_json::to_string(&view(&conn).unwrap()).unwrap();
		assert!(!json.contains("K001secret") && !json.contains(&password));
		assert!(json.contains(r#""keyId":"0012ab""#), "{json}");

		// saved again without the secret: both kept, no new password
		let again = Settings {
			secret: "".into(),
			..b2()
		};
		assert_eq!(save(&conn, &key, again, "genco/x").unwrap(), None);
		let o = open(&conn, &key).unwrap().unwrap();
		assert_eq!(o.secret, "K001secret");
		assert_eq!(o.password, password);

		// another key needs its secret
		let other = Settings {
			key_id: "other".into(),
			secret: "".into(),
			..b2()
		};
		assert!(save(&conn, &key, other, "genco/x").is_err());

		// an existing repository's password replaces it
		let restore = Settings {
			repo_password: "from the safe".into(),
			..b2()
		};
		save(&conn, &key, restore, "genco/x").unwrap();
		let o = open(&conn, &key).unwrap().unwrap();
		assert_eq!(o.password, "from the safe");

		let off = Settings {
			provider: "off".into(),
			..Default::default()
		};
		save(&conn, &key, off, "").unwrap();
		assert!(view(&conn).is_none());
		std::fs::remove_file(&key).unwrap();
	}

	fn opened(provider: &str) -> Opened {
		Opened {
			stored: Stored {
				provider: provider.into(),
				bucket: "acme".into(),
				path: "genco/x".into(),
				endpoint: "https://s3.example.com".into(),
				region: "eu-1".into(),
				key_id: "id".into(),
				keep: Keep::default(),
				limit_upload: Some(500),
				configured_at: 0,
				secret: vec![],
				password: vec![],
			},
			secret: "sec".into(),
			password: "pw".into(),
		}
	}

	#[test]
	fn restic_env_for_b2_and_s3() {
		let b2 = env(&opened("b2"));
		assert!(b2.contains(&("RESTIC_REPOSITORY", "b2:acme:genco/x".into())));
		assert!(b2.contains(&("B2_ACCOUNT_ID", "id".into())));
		assert!(b2.contains(&("B2_ACCOUNT_KEY", "sec".into())));
		assert!(b2.contains(&("RESTIC_PASSWORD", "pw".into())));
		assert!(!b2.iter().any(|(k, _)| k.starts_with("AWS")));

		let s3 = env(&opened("s3"));
		assert!(s3.contains(&(
			"RESTIC_REPOSITORY",
			"s3:https://s3.example.com/acme/genco/x".into()
		)));
		assert!(s3.contains(&("AWS_ACCESS_KEY_ID", "id".into())));
		assert!(s3.contains(&("AWS_SECRET_ACCESS_KEY", "sec".into())));
		assert!(s3.contains(&("AWS_DEFAULT_REGION", "eu-1".into())));

		assert_eq!(limit_args(&opened("b2").stored), ["--limit-upload", "500"]);
	}

	#[test]
	fn only_real_secrets_are_hidden() {
		let mut o = opened("b2");
		o.secret = "K001secretkey".into();
		o.password = "0123456789abcdef".into();
		assert_eq!(
			o.hide("bad K001secretkey and 0123456789abcdef"),
			"bad *** and ***"
		);
		// too short to be a key, it would mask ordinary words
		o.secret = "sec".into();
		o.password = "        ".into();
		assert_eq!(o.hide("a second  sec"), "a second  sec");
	}

	#[test]
	fn required_says_when_there_is_no_remote() {
		let db = Db::memory();
		let err = required(&db).err().unwrap().to_string();
		assert!(err.contains("no remote backup is set up"), "{err}");
	}

	#[tokio::test]
	async fn a_remote_that_doesnt_answer_is_given_up() {
		let limit = std::time::Duration::from_millis(10);
		let never = std::future::pending::<()>();
		let err = within(limit, "b2:acme:x", never).await.unwrap_err();
		assert!(err.contains("no answer from b2:acme:x"), "{err}");
		assert_eq!(within(limit, "b2:acme:x", async { 1 }).await, Ok(1));
	}

	#[test]
	fn changes_and_stops_are_told_and_a_stop_stays_a_week() {
		let db = Db::memory();
		let conn = db.conn();
		let t = now();
		let loc = Some("b2:acme:x".to_string());
		assert!(
			changed(&conn, loc.clone(), loc.clone(), "A", t)
				.unwrap()
				.is_none()
		);

		let n = changed(&conn, None, loc.clone(), "the terminal", t).unwrap();
		let n = n.unwrap();
		assert!(n.attention && n.title.contains("b2:acme:x"), "{n:?}");
		assert!(n.text.starts_with("the terminal set"), "{n:?}");

		let n =
			changed(&conn, loc.clone(), None, "the agent, confirmed by A", t)
				.unwrap()
				.unwrap();
		assert!(n.title.contains("turned off"), "{n:?}");
		let (key, detail) = problem(&conn, t + DAY).unwrap();
		assert_eq!(key, "backup:remote-off");
		assert!(detail.contains("the agent, confirmed by A"), "{detail}");
		assert_eq!(problem(&conn, t + OFF_SHOWN + 1), None);

		// set again, the stop is no problem any more
		changed(&conn, None, loc, "A", t).unwrap();
		assert_eq!(problem(&conn, t + DAY), None);
	}

	fn set_cmd() -> RemoteCmd {
		RemoteCmd::Set {
			b2: true,
			s3: false,
			key_id: "0012ab".into(),
			bucket: "acme-backups".into(),
			path: None,
			endpoint: "".into(),
			region: "".into(),
			keep_daily: 14,
			keep_weekly: 8,
			keep_monthly: 12,
			limit_upload: None,
		}
	}

	#[tokio::test]
	async fn the_agent_asks_an_admin_the_terminal_sets_it() {
		let db = Db::memory();
		// the password confirms, nothing is mailed
		set_setting(&db.conn(), "mail", r#"{"mode":"off"}"#).unwrap();
		let cfg: Config =
			toml::from_str("domain = 'panel.example.ch'").unwrap();
		let key = key_file();
		let stdin = || Ok("K001secret".to_string());
		let pending = || {
			db.conn()
				.query_row(
					"SELECT count(*) FROM pending_actions WHERE status = 'pending'",
					[],
					|r| r.get::<_, i64>(0),
				)
				.unwrap()
		};

		let msg = cli(&cfg, &db, set_cmd(), stdin, "agent", &key)
			.await
			.unwrap();
		assert!(msg.contains("Waiting for confirmation"), "{msg}");
		assert!(msg.contains("Settings → Backups, not here"), "{msg}");
		assert!(!msg.contains("K001secret"), "{msg}");
		assert!(view(&db.conn()).is_none());
		assert_eq!(pending(), 1);
		let sealed: Vec<u8> = db
			.conn()
			.query_row("SELECT payload FROM pending_actions", [], |r| r.get(0))
			.unwrap();
		let json = crate::secrets::open_with(&key, &sealed).unwrap();
		assert!(json.contains("K001secret"));

		let msg = cli(&cfg, &db, set_cmd(), stdin, "terminal", &key)
			.await
			.unwrap();
		assert!(msg.contains("shown only this once"), "{msg}");
		assert!(view(&db.conn()).is_some());
		assert_eq!(pending(), 1);

		let msg = cli(&cfg, &db, RemoteCmd::Off, stdin, "agent", &key)
			.await
			.unwrap();
		assert!(msg.contains("Waiting for confirmation"), "{msg}");
		assert!(view(&db.conn()).is_some());
		assert_eq!(pending(), 2);

		cli(&cfg, &db, RemoteCmd::Off, stdin, "terminal", &key)
			.await
			.unwrap();
		assert!(view(&db.conn()).is_none());
		let (k, _) = problem(&db.conn(), now()).unwrap();
		assert_eq!(k, "backup:remote-off");
		std::fs::remove_file(&key).unwrap();
	}

	#[test]
	fn retention_skips_kinds_kept_at_zero() {
		let args = forget_args(&Keep {
			daily: 14,
			weekly: 0,
			monthly: 12,
		});
		assert!(args.ends_with(&strings(&[
			"--prune",
			"--keep-daily",
			"14",
			"--keep-monthly",
			"12"
		])));
		assert!(args.contains(&"genco".to_string()));
	}

	#[tokio::test]
	async fn the_copy_is_skipped_without_a_remote() {
		let db = Db::memory();
		let dir = std::env::temp_dir()
			.join(format!("genco-remote-log-{}", rand::random::<u32>()));
		let dir = dir.to_str().unwrap();
		let mut log = Log::create(dir, 1, false).await.unwrap();
		copy(&db, &mut log).await;
		assert!(log.ok);
		assert_eq!(crate::job::read(dir, 1).await, "");
		assert_eq!(state(&db.conn()), RemoteState::default());
		std::fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn a_problem_after_a_failure_or_two_days() {
		let db = Db::memory();
		let conn = db.conn();
		let key = key_file();
		assert_eq!(problem(&conn, now()), None);
		save(&conn, &key, b2(), "genco/x").unwrap();
		let t = now();
		assert_eq!(problem(&conn, t), None);
		assert_eq!(
			problem(&conn, t + OLD_AFTER + 1).unwrap().0,
			"backup:remote-old"
		);

		let mut st = RemoteState {
			last_ok: Some(t),
			..Default::default()
		};
		set_state(&conn, &st).unwrap();
		assert_eq!(problem(&conn, t + DAY), None);
		st.last_error = Some("no network".into());
		st.last_error_at = Some(t + 1);
		set_state(&conn, &st).unwrap();
		let (k, detail) = problem(&conn, t + 2).unwrap();
		assert_eq!(k, "backup:remote-failed");
		assert!(detail.contains("no network"));
		std::fs::remove_file(&key).unwrap();
	}

	#[tokio::test]
	async fn a_confirmed_request_keeps_the_new_password_for_the_panel() {
		let db = Db::memory();
		set_setting(&db.conn(), "mail", r#"{"mode":"off"}"#).unwrap();
		let cfg: Config =
			toml::from_str("domain = 'panel.example.ch'").unwrap();
		let key = key_file();
		let sealed = crate::secrets::seal_with(
			&key,
			&serde_json::to_string(&b2()).unwrap(),
		)
		.unwrap();

		let mut log = vec![];
		confirmed(&cfg, &db, &key, Some(&sealed), "the agent", &mut log)
			.await
			.unwrap();
		assert!(view(&db.conn()).is_some());
		assert!(log.join("\n").contains("Settings → Backups"), "{log:?}");
		let kept: Vec<u8> = get_json(&db.conn(), NEW_PASSWORD).unwrap();
		let password = crate::secrets::open_with(&key, &kept).unwrap();
		assert_eq!(open(&db.conn(), &key).unwrap().unwrap().password, password);

		confirmed(&cfg, &db, &key, None, "the agent", &mut log)
			.await
			.unwrap();
		assert!(view(&db.conn()).is_none());
		std::fs::remove_file(&key).unwrap();
	}
}
