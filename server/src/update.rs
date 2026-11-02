//! Updates of genco itself from its GitHub releases. Every release is
//! signed (ssh-keygen -Y sign, the key's public half is in
//! `update/allowed_signers`); an update checks the signature and that the
//! new binary runs, saves the old binary and database, runs `genco setup`
//! and restarts. If genco isn't back within a minute the old binary and
//! database come back. A daily check at night installs patch releases if
//! the server allows it (Settings → Updates); the weekly update round
//! reports the rest. A test server is in testing mode instead: it never
//! updates itself, its builds come with `genco update --file`, unsigned
//! (only root can run it), with the same backup and rollback.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::AppState;
use crate::config::Config;
use crate::db::{Db, get_setting, now, set_setting};
use crate::error::{Error, Result};
use crate::users::Authed;
use crate::util::sh;

const REPO: &str = "chuchi-dev/genco";
const DIR: &str = "/var/lib/genco/updates";
const BIN: &str = "/usr/local/bin/genco";
const UNIT: &str = "genco-update";
const SIGNERS: &str = include_str!("update/allowed_signers");
/// when the nightly check runs, local time
const NIGHT: &str = "04:00";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u32, u32, u32);

impl Version {
	/// `1.2.3` or `v1.2.3`
	pub fn parse(s: &str) -> Option<Self> {
		let mut parts = s.trim().trim_start_matches('v').split('.');
		let v = Version(
			parts.next()?.parse().ok()?,
			parts.next()?.parse().ok()?,
			parts.next()?.parse().ok()?,
		);
		parts.next().is_none().then_some(v)
	}

	/// Whether `newer` only fixes things: the same major and minor.
	fn is_patch(self, newer: Version) -> bool {
		newer > self && (newer.0, newer.1) == (self.0, self.1)
	}
}

impl std::fmt::Display for Version {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}.{}.{}", self.0, self.1, self.2)
	}
}

fn current() -> Version {
	Version::parse(VERSION).expect("the crate's version")
}

/// What the panel shows under Settings → Updates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
	#[serde(default)]
	pub current: String,
	/// the newest release at the last check
	pub latest: Option<String>,
	pub checked_at: Option<i64>,
	/// why the last check failed
	pub error: Option<String>,
	/// patch releases install themselves at night
	#[serde(default)]
	pub auto: bool,
	/// "release", or "testing" for a test server that gets its builds
	/// with `genco update --file`
	#[serde(default)]
	pub mode: String,
	/// an update runs right now
	#[serde(default)]
	pub running: bool,
	pub last: Option<LastUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastUpdate {
	pub from: String,
	pub to: String,
	pub ok: bool,
	pub at: i64,
	pub message: String,
}

fn status(db: &Db) -> Status {
	let conn = db.conn();
	let mut s: Status =
		crate::db::get_json(&conn, "update:check").unwrap_or_default();
	s.current = VERSION.into();
	s.auto = auto(&conn);
	s.mode = mode(&conn).into();
	s.running = !crate::job::is_free(&lock_path());
	s.last = crate::db::get_json(&conn, "update:last");
	s
}

fn auto(conn: &rusqlite::Connection) -> bool {
	get_setting(conn, "update:auto").ok().flatten().as_deref() != Some("off")
}

fn mode(conn: &rusqlite::Connection) -> &'static str {
	match get_setting(conn, "update:mode").ok().flatten().as_deref() {
		Some("testing") => "testing",
		_ => "release",
	}
}

/// Only on the terminal (`genco update --mode`): a panel login can't turn
/// off the signed releases.
fn set_mode(conn: &rusqlite::Connection, value: &str) -> Result<()> {
	if !["release", "testing"].contains(&value) {
		return Err(Error::Invalid("the mode is release or testing".into()));
	}
	set_setting(conn, "update:mode", value)?;
	Ok(())
}

fn lock_path() -> String {
	format!("{DIR}/update.lock")
}

/// The newest release on GitHub.
async fn latest() -> std::result::Result<Version, String> {
	#[derive(Deserialize)]
	struct Release {
		tag_name: String,
	}
	let res = reqwest::Client::new()
		.get(format!(
			"https://api.github.com/repos/{REPO}/releases/latest"
		))
		.header("user-agent", format!("genco/{VERSION}"))
		.timeout(Duration::from_secs(30))
		.send()
		.await
		.map_err(|e| format!("GitHub: {e}"))?;
	if res.status() == reqwest::StatusCode::NOT_FOUND {
		return Err("no release on GitHub yet".into());
	}
	let release: Release = res
		.error_for_status()
		.map_err(|e| format!("GitHub: {e}"))?
		.json()
		.await
		.map_err(|e| format!("GitHub: {e}"))?;
	Version::parse(&release.tag_name)
		.ok_or_else(|| format!("odd release tag {}", release.tag_name))
}

/// Asks GitHub for the newest release and stores the answer.
async fn check(db: &Db) -> std::result::Result<Version, String> {
	let found = latest().await;
	let mut s = status(db);
	s.checked_at = Some(now());
	match &found {
		Ok(v) => {
			s.latest = Some(v.to_string());
			s.error = None;
		}
		Err(e) => s.error = Some(e.clone()),
	}
	let stored = serde_json::json!({
		"latest": s.latest,
		"checkedAt": s.checked_at,
		"error": s.error,
	});
	if let Err(e) = set_setting(&db.conn(), "update:check", &stored.to_string())
	{
		tracing::error!("storing the update check: {e}");
	}
	found
}

/// Starts `genco update` as its own unit, it outlives genco's restart.
async fn start(
	version: Option<Version>,
	file: Option<&Path>,
) -> std::result::Result<(), String> {
	if !crate::job::is_free(&lock_path()) {
		return Err("an update runs already".into());
	}
	let mut cmd = Command::new("systemd-run");
	cmd.args([
		"--unit",
		UNIT,
		"--collect",
		"--quiet",
		BIN,
		"update",
		"--yes",
	]);
	if let Some(v) = version {
		cmd.args(["--version", &v.to_string()]);
	}
	if let Some(f) = file {
		cmd.arg("--file").arg(f);
	}
	let out = cmd
		.output()
		.await
		.map_err(|e| format!("systemd-run: {e}"))?;
	if !out.status.success() {
		return Err(format!(
			"systemd-run: {}",
			String::from_utf8_lossy(&out.stderr).trim()
		));
	}
	Ok(())
}

/// Called every minute: the nightly check, which installs a patch release
/// if the server allows it.
pub async fn due(db: &Db) {
	let Ok(now) = sh("date '+%F %H:%M'").await else {
		return;
	};
	let Some((today, time)) = now.split_once(' ') else {
		return;
	};
	// a test server gets its builds by hand
	if mode(&db.conn()) == "testing" {
		return;
	}
	let checked = get_setting(&db.conn(), "update:night").ok().flatten();
	if time < NIGHT || checked.as_deref() == Some(today) {
		return;
	}
	if let Err(e) = set_setting(&db.conn(), "update:night", today) {
		tracing::error!("update check: {e}");
		return;
	}
	let Ok(latest) = check(db).await else {
		return;
	};
	if auto(&db.conn()) && current().is_patch(latest) {
		tracing::info!("installing the patch release {latest}");
		if let Err(e) = start(Some(latest), None).await {
			tracing::error!("update: {e}");
		}
	}
}

/// Whether this process runs inside genco's own service, whose restart
/// would end it halfway (the agent's commands do).
fn inside_genco() -> bool {
	std::fs::read_to_string("/proc/self/cgroup")
		.is_ok_and(|c| c.contains("/genco.service"))
}

/// What `genco update` was asked for.
pub struct Args {
	pub check: bool,
	pub yes: bool,
	pub version: Option<String>,
	/// a local build instead of a release
	pub file: Option<PathBuf>,
	/// sets the update mode
	pub mode: Option<String>,
}

/// `genco update`: checks, or installs the newest (or the given) release
/// or a local build.
pub async fn main(cfg: &Config, args: Args) -> std::result::Result<(), String> {
	let db = Db::open(&cfg.db_path())?;
	if let Some(m) = &args.mode {
		set_mode(&db.conn(), m).map_err(|e| e.to_string())?;
		println!("update mode: {m}");
		return Ok(());
	}
	let me = current();
	let (latest, source) = match &args.file {
		Some(file) => {
			let (v, file) = local_build(&db, file).await?;
			(v, Source::File(file))
		}
		None => match pick_release(&db, &args, me).await? {
			Some(v) => (v, Source::Release),
			None => return Ok(()),
		},
	};
	if args.check {
		return Ok(());
	}
	if !args.yes {
		return Err("add --yes to install it".into());
	}
	if inside_genco() {
		let file = match &source {
			Source::File(f) => Some(f.as_path()),
			Source::Release => None,
		};
		start(Some(latest), file).await?;
		println!(
			"started as {UNIT}.service, it restarts genco; the log is \
			{DIR}/update.log"
		);
		return Ok(());
	}

	// root's alone: it holds a copy of the database
	let lock = crate::job::try_lock(&lock_path())
		.map_err(|e| e.to_string())?
		.ok_or("an update runs already")?;
	let mut log = UpdateLog::create()?;
	let local = matches!(source, Source::File(_));
	let res = install(cfg, &db, latest, source, &mut log).await;
	// a rollback put the old database file in place
	drop(db);
	let message = match &res {
		Ok(()) if local => format!("installed a build of {latest} over {me}"),
		Ok(()) => format!("updated from {me} to {latest}"),
		Err(e) => e.clone(),
	};
	log.line(&message);
	record(cfg, me, latest, res.is_ok(), &message, &mut log).await?;
	drop(lock);
	res
}

/// The version of a local build (`--file`): only on a test server, only a
/// root-owned file outside the groups' folders, run without a shell.
async fn local_build(
	db: &Db,
	file: &Path,
) -> std::result::Result<(Version, PathBuf), String> {
	if mode(&db.conn()) != "testing" {
		return Err(
			"--file is for test servers: genco update --mode testing first"
				.into(),
		);
	}
	let file = std::fs::canonicalize(file)
		.map_err(|e| format!("{}: {e}", file.display()))?;
	let meta = std::fs::metadata(&file).map_err(|e| e.to_string())?;
	{
		use std::os::unix::fs::MetadataExt as _;
		if meta.uid() != 0 || file.starts_with("/srv") {
			return Err(format!(
				"{} has to belong to root, outside the groups' folders",
				file.display()
			));
		}
	}
	let out = Command::new(&file)
		.arg("--version")
		.output()
		.await
		.map_err(|e| format!("{}: {e}", file.display()))?;
	let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
	let v = said
		.strip_prefix("genco ")
		.and_then(Version::parse)
		.ok_or_else(|| format!("{} isn't a genco binary", file.display()))?;
	println!("installed:   {}\nlocal build: {v}", current());
	Ok((v, file))
}

/// The release to install (`--version` or the newest), after telling what
/// it is; `None` when there is nothing to do.
async fn pick_release(
	db: &Db,
	args: &Args,
	me: Version,
) -> std::result::Result<Option<Version>, String> {
	let wanted = match &args.version {
		Some(v) => Some(
			Version::parse(v).ok_or_else(|| format!("{v} isn't a version"))?,
		),
		None => None,
	};
	let latest = match wanted {
		Some(v) => v,
		None => check(db).await?,
	};
	let kind = match latest {
		v if v <= me => "",
		v if me.is_patch(v) => " (a patch release)",
		v if v.0 == me.0 => " (a minor release)",
		_ => " (a major release, read its notes first)",
	};
	println!("installed: {me}\nlatest:    {latest}{kind}");
	println!("https://github.com/{REPO}/releases/tag/v{latest}");
	if mode(&db.conn()) == "testing" {
		println!("this server is in testing mode, it doesn't update itself");
	}
	// an older one only when asked for by its version
	let wanted_older = latest < me && wanted.is_some();
	if args.check || latest > me || wanted_older {
		return Ok(Some(latest));
	}
	if latest == me {
		println!("nothing to do");
	}
	Ok(None)
}

/// The result for the panel (Settings → Updates), the activity log and
/// the admins.
async fn record(
	cfg: &Config,
	from: Version,
	to: Version,
	ok: bool,
	message: &str,
	log: &mut UpdateLog,
) -> std::result::Result<(), String> {
	// opened again: a rollback put the old database file in place
	let db = Db::open(&cfg.db_path())?;
	let last = LastUpdate {
		from: from.to_string(),
		to: to.to_string(),
		ok,
		at: now(),
		message: message.into(),
	};
	let stored = serde_json::to_string(&last).unwrap();
	if let Err(e) = set_setting(&db.conn(), "update:last", &stored) {
		log.line(&format!("storing the result: {e}"));
	}
	crate::activity::log(
		&db.conn(),
		"genco",
		"Update",
		&to.to_string(),
		message,
		Some(ok),
	);
	let notice = crate::notify::Notice {
		kind: "update",
		attention: !ok,
		title: if ok {
			format!("updated to {to}")
		} else {
			format!("the update to {to} failed")
		},
		text: format!("{message}\n\nThe log is {DIR}/update.log."),
		url: cfg.domain.as_ref().map(|d| format!("https://{d}/")),
		proposal: None,
	};
	crate::notify::send(cfg, &db, notice).await;
	Ok(())
}

/// The update's lines, in `DIR/update.log` and on the terminal.
struct UpdateLog(std::fs::File);

impl UpdateLog {
	fn create() -> std::result::Result<Self, String> {
		std::fs::File::create(Path::new(DIR).join("update.log"))
			.map(Self)
			.map_err(|e| format!("update.log: {e}"))
	}

	fn line(&mut self, text: &str) {
		use std::io::Write as _;
		println!("{text}");
		let _ = writeln!(self.0, "{text}");
	}
}

async fn run(log: &mut UpdateLog, what: &str, cmd: &mut Command) -> bool {
	log.line(&format!("==> {what}"));
	match cmd.output().await {
		Ok(out) => {
			for l in String::from_utf8_lossy(&out.stdout)
				.lines()
				.chain(String::from_utf8_lossy(&out.stderr).lines())
			{
				log.line(l);
			}
			out.status.success()
		}
		Err(e) => {
			log.line(&format!("{what}: {e}"));
			false
		}
	}
}

/// Where the new binary comes from.
enum Source {
	/// GitHub, signed
	Release,
	/// a build root put on the server, unsigned
	File(PathBuf),
}

async fn install(
	cfg: &Config,
	db: &Db,
	version: Version,
	source: Source,
	log: &mut UpdateLog,
) -> std::result::Result<(), String> {
	let dir = PathBuf::from(DIR).join(version.to_string());
	std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
	let name = format!("genco-{}-linux", std::env::consts::ARCH);
	let bin = dir.join(&name);
	match source {
		Source::Release => download(&dir, &name, version, log).await?,
		Source::File(file) => {
			log.line(&format!("==> a local build from {}", file.display()));
			std::fs::copy(&file, &bin)
				.map_err(|e| format!("{}: {e}", file.display()))?;
		}
	}

	crate::util::set_mode(&bin, 0o755).map_err(|e| e.to_string())?;
	let said = sh(&format!("{} --version", bin.display())).await;
	if said.as_deref() != Ok(&format!("genco {version}")) {
		return Err(format!(
			"the new binary doesn't run here ({}), nothing changed",
			said.unwrap_or_else(|e| e)
		));
	}

	// the database as it is, the new genco may migrate it
	log.line("==> saving the binary and the database");
	let saved_db = dir.join("genco-before.db");
	let _ = std::fs::remove_file(&saved_db);
	db.conn()
		.execute("VACUUM INTO ?1", [saved_db.display().to_string()])
		.map_err(|e| format!("saving the database: {e}"))?;
	std::fs::copy(BIN, format!("{BIN}.previous"))
		.map_err(|e| format!("saving the binary: {e}"))?;

	std::fs::copy(&bin, format!("{BIN}.new"))
		.and_then(|_| std::fs::rename(format!("{BIN}.new"), BIN))
		.map_err(|e| format!("installing: {e}"))?;

	let mut setup = Command::new(BIN);
	setup.arg("setup");
	let mut restart = Command::new("systemctl");
	restart.args(["restart", "genco"]);
	let ok = run(log, "genco setup", &mut setup).await
		&& run(log, "restarting genco", &mut restart).await
		&& healthy(cfg, log).await;
	if ok {
		clean_old(&dir);
		return Ok(());
	}

	log.line("==> going back to the version before");
	let back = rollback(cfg, &saved_db).await;
	match back {
		Ok(()) => Err(format!(
			"genco {version} didn't come up, {VERSION} runs again"
		)),
		Err(e) => Err(format!(
			"genco {version} didn't come up and going back failed: {e}"
		)),
	}
}

/// The release and its signature into `dir`, checked.
async fn download(
	dir: &Path,
	name: &str,
	version: Version,
	log: &mut UpdateLog,
) -> std::result::Result<(), String> {
	// another place to test updates from, the signature still counts
	let base = std::env::var("GENCO_RELEASE_URL").unwrap_or_else(|_| {
		format!("https://github.com/{REPO}/releases/download")
	});
	let url = format!("{base}/v{version}/{name}");
	let bin = dir.join(name);
	let sig = dir.join(format!("{name}.sig"));

	let mut download = Command::new("curl");
	download
		.args(["-fsSL", "-o"])
		.arg(&bin)
		.arg(&url)
		.arg("-o")
		.arg(&sig)
		.arg(format!("{url}.sig"));
	if !run(log, &format!("downloading {url}"), &mut download).await {
		return Err("the download failed".into());
	}

	let signers = dir.join("allowed_signers");
	std::fs::write(&signers, SIGNERS).map_err(|e| e.to_string())?;
	let file = std::fs::File::open(&bin).map_err(|e| e.to_string())?;
	let mut verify = Command::new("ssh-keygen");
	verify
		.args(["-Y", "verify", "-I", "release@genco", "-n", "genco-release"])
		.arg("-f")
		.arg(&signers)
		.arg("-s")
		.arg(&sig)
		.stdin(file);
	if !run(log, "checking the signature", &mut verify).await {
		return Err(
			"the release's signature doesn't match, nothing changed".into()
		);
	}

	Ok(())
}

/// Up and answering on the panel's port, and still up a little later.
async fn healthy(cfg: &Config, log: &mut UpdateLog) -> bool {
	let check = format!(
		"systemctl is-active -q genco && curl -s -o /dev/null -m 5 \
		-w '%{{http_code}}' http://127.0.0.1:{}/api/users/me",
		cfg.panel_port()
	);
	for _ in 0..30 {
		tokio::time::sleep(Duration::from_secs(2)).await;
		let up = sh(&check)
			.await
			.is_ok_and(|code| code.starts_with(['2', '3', '4']));
		if up {
			tokio::time::sleep(Duration::from_secs(10)).await;
			if sh("systemctl is-active -q genco").await.is_ok() {
				log.line("genco is up");
				return true;
			}
		}
	}
	log.line("genco didn't come up within a minute");
	false
}

async fn rollback(
	cfg: &Config,
	saved_db: &Path,
) -> std::result::Result<(), String> {
	let db = cfg.db_path().display().to_string();
	sh(&format!(
		"systemctl stop genco \
		&& cp {BIN}.previous {BIN}.new && mv {BIN}.new {BIN} \
		&& cp '{}' '{db}.restore' && rm -f '{db}-wal' '{db}-shm' \
		&& mv '{db}.restore' '{db}' \
		&& systemctl start genco",
		saved_db.display()
	))
	.await
	.map(|_| ())
}

/// Only the newest download stays, the database copy in it too.
fn clean_old(keep: &Path) {
	for entry in std::fs::read_dir(DIR).into_iter().flatten().flatten() {
		let path = entry.path();
		if path.is_dir() && path != keep {
			let _ = std::fs::remove_dir_all(&path);
		}
	}
}

async fn get_status(_authed: Authed, State(db): State<Db>) -> Json<Status> {
	Json(status(&db))
}

async fn check_now(_authed: Authed, State(db): State<Db>) -> Json<Status> {
	let _ = check(&db).await;
	Json(status(&db))
}

async fn install_now(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<()>> {
	let latest = status(&db)
		.latest
		.and_then(|v| Version::parse(&v))
		.filter(|v| *v > current())
		.ok_or_else(|| Error::Invalid("no newer release".into()))?;
	start(Some(latest), None).await.map_err(Error::Invalid)?;
	Ok(Json(()))
}

#[derive(Deserialize)]
struct AutoReq {
	auto: bool,
}

async fn set_auto(
	_authed: Authed,
	State(db): State<Db>,
	Json(req): Json<AutoReq>,
) -> Result<Json<()>> {
	let value = if req.auto { "patch" } else { "off" };
	set_setting(&db.conn(), "update:auto", value)?;
	Ok(Json(()))
}

async fn get_log(_authed: Authed) -> String {
	tokio::fs::read_to_string(Path::new(DIR).join("update.log"))
		.await
		.unwrap_or_default()
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/update", get(get_status))
		.route("/update/check", post(check_now))
		.route("/update/install", post(install_now))
		.route("/update/auto", post(set_auto))
		.route("/update/log", get(get_log))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn versions() {
		let v = Version::parse("v0.1.2").unwrap();
		assert_eq!(v, Version(0, 1, 2));
		assert!(Version::parse("0.1").is_none());
		assert!(Version::parse("0.1.2.3").is_none());
		assert!(Version::parse("0.1.x").is_none());
		assert!(v.is_patch(Version(0, 1, 3)));
		assert!(!v.is_patch(Version(0, 2, 0)));
		assert!(!v.is_patch(Version(0, 1, 2)));
		assert!(!v.is_patch(Version(0, 1, 1)));
		assert!(Version(0, 10, 0) > Version(0, 9, 9));
	}

	#[test]
	fn signers_has_the_release_key() {
		assert!(SIGNERS.starts_with("release@genco "));
		assert!(SIGNERS.contains("namespaces=\"genco-release\""));
	}

	#[test]
	fn install_sh_trusts_the_same_key() {
		let script = include_str!("../../scripts/install.sh");
		assert!(script.contains(SIGNERS.trim()));
	}

	#[test]
	fn auto_updates_are_on_unless_turned_off() {
		let db = Db::memory();
		assert!(auto(&db.conn()));
		set_setting(&db.conn(), "update:auto", "off").unwrap();
		assert!(!auto(&db.conn()));
	}
}
