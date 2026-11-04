//! A copy of the backups off the server: after each full backup its new
//! snapshots go with `restic copy` into a repository at Backblaze B2 or an
//! S3-compatible storage, which keeps its own, longer history. Key and
//! repository password are sealed with the secrets key, the API only says
//! whether they are set, and restic gets them through its environment.

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
const DAY: i64 = 24 * 60 * 60 * 1000;
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
	location: String,
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

/// A change of the settings, from the panel or the CLI.
#[derive(Debug, Clone, Default, Deserialize)]
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
	match s.provider.as_str() {
		"b2" => format!("b2:{}:{}", s.bucket, s.path),
		_ => format!(
			"s3:{}/{}/{}",
			s.endpoint.trim_end_matches('/'),
			s.bucket,
			s.path
		),
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

	/// The key and password as ***, e.g. in restic's errors.
	pub fn hide(&self, text: &str) -> String {
		text.replace(&self.secret, "***")
			.replace(&self.password, "***")
	}

	pub fn location(&self) -> String {
		repository(&self.stored)
	}

	/// restic's output, its error with the secrets hidden.
	async fn output(
		&self,
		args: &[&str],
	) -> std::result::Result<Vec<u8>, String> {
		let out = self
			.restic()
			.args(args)
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
		// restic retries a missing bucket or a dead endpoint for minutes
		let read = self
			.restic()
			.args(["cat", "config"])
			.stdout(Stdio::null())
			.kill_on_drop(true)
			.output();
		let out = tokio::time::timeout(CONNECT_TIMEOUT, read)
			.await
			.map_err(|_| {
				format!(
					"no answer from {} in a minute: does the bucket exist, \
					is the key allowed to use it?",
					self.location()
				)
			})?
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
	if s.bucket.len() < 3 || s.bucket.len() > 63 || !plain(&s.bucket, ".-_") {
		return invalid("the bucket is a name of 3 to 63 letters, digits, -");
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

/// What the minute check reports: the last copy failed, or none worked
/// for two days. Key and text of the problem.
pub fn problem(conn: &Connection, now: i64) -> Option<(&'static str, String)> {
	let s = stored(conn)?;
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
	}))
}

#[derive(Debug, Deserialize)]
struct SaveReq {
	#[serde(flatten)]
	settings: Settings,
	/// the admin's password and code: it decides where all data goes
	#[serde(flatten)]
	auth: crate::users::PasswordOnly,
}

async fn save_settings(
	authed: Authed,
	State(cfg): State<std::sync::Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<SaveReq>,
) -> Result<Json<Option<String>>> {
	crate::users::verify_password_and_code(&db, &authed.user, &req.auth)
		.await?;
	let generated = save(
		&db.conn(),
		crate::secrets::key_file(),
		req.settings,
		&default_path(&cfg),
	)?;
	Ok(Json(generated))
}

async fn test_route(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<String>> {
	let opened = open(&db.conn(), crate::secrets::key_file())?
		.ok_or_else(|| Error::Invalid("no remote backup is set up".into()))?;
	opened.test().await.map(Json).map_err(Error::Invalid)
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/settings/backup", get(get_settings).post(save_settings))
		.route("/settings/backup/test", post(test_route))
}

// ------------------------------------------------------------------- cli

#[derive(Debug, clap::Subcommand)]
pub enum RemoteCmd {
	/// Copy the backups to Backblaze B2 or an S3-compatible storage after
	/// each night. stdin: the application key (B2) or secret key (S3); a
	/// second line is the password of an existing remote repository (a
	/// new server restoring), without one genco makes a new one and
	/// prints it once
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
	/// Stop copying; the repository stays in the bucket
	Off,
}

pub async fn cli(
	cfg: &Config,
	db: &Db,
	cmd: RemoteCmd,
	stdin: impl FnOnce() -> std::result::Result<String, String>,
) -> Result<String> {
	let key_file = crate::secrets::key_file();
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
			let generated =
				save(&db.conn(), key_file, settings, &default_path(cfg))?;
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
		RemoteCmd::Test => {
			let opened = open(&db.conn(), key_file)?.ok_or_else(|| {
				Error::Invalid("no remote backup is set up".into())
			})?;
			opened.test().await.map_err(Error::Invalid)
		}
		RemoteCmd::Show => {
			let (Some(v), st) = (view(&db.conn()), state(&db.conn())) else {
				return Ok("no remote backup".into());
			};
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
		RemoteCmd::Off => {
			let s = Settings {
				provider: "off".into(),
				..Default::default()
			};
			save(&db.conn(), key_file, s, "")?;
			Ok("no more remote copies; the repository stays in the bucket"
				.into())
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
		assert_eq!(opened("b2").hide("bad sec and pw"), "bad *** and ***");
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
}
