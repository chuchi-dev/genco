//! Build runners: machines that build steps marked `on = "runner"` in
//! genco.toml, so a long Rust build doesn't slow down the live sites. A
//! runner is the same genco binary (`genco runner`, see runner.rs) on a
//! build box; it connects out to its genco servers with a token, takes
//! build jobs, runs them in a container (podman or docker) of an image
//! for this server's Debian release, sends the log and the artifacts
//! back. No ssh, it works behind NAT.
//!
//! A deploy queues a job and waits until a runner takes it, oldest job
//! first; several deploys of different groups wait at once and their logs
//! show their place in the queue. The step never runs on this server
//! instead (it has no toolchain for it, operator): no runner takes it in
//! the queue timeout (Settings → Runners, 2 h by default, `wait` of the
//! step), or no runner is online for `OFFLINE`, and the deploy fails. Only
//! a detected step (no genco.toml) builds here, when no runner is
//! registered at all. Cancelling the deploy takes the job out of the
//! queue. The source and the artifacts go through root-only files in
//! `DIR`; packing and unpacking them runs as the group's user.

mod api;
mod job;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{
	DefaultBodyLimit, FromRef, FromRequestParts, Path as UrlPath, State,
};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use crate::AppState;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::job::Log;
use crate::users::Authed;
use crate::util::sh;

pub use api::RunnerInfo;
use api::*;
pub use job::{Outcome, Request, build, cancel};

const DIR: &str = "/var/lib/genco/builds";
/// how long a deploy waits for a runner to take its job, by default
const QUEUE_MINUTES: u32 = 120;
/// no runner online this long while a job waits: the deploy fails
const OFFLINE: Duration = Duration::from_secs(15 * 60);
/// a waiting deploy says where it is in the queue this often, without
/// a change too
const REMIND: Duration = Duration::from_secs(10 * 60);
/// a runner is online when it asked for jobs this recently
const ONLINE_MS: i64 = 90 * 1000;
/// a running job whose runner was silent this long failed
const SILENT_MS: i64 = 2 * 60 * 1000;
/// a job a runner took but said nothing about for this long goes back
const UNHEARD_MS: i64 = 60 * 1000;
/// what a runner long-polls for a job
const POLL: Duration = Duration::from_secs(25);
const MAX_LOG: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runner {
	pub id: i64,
	pub name: String,
	pub created_at: i64,
	pub last_seen: Option<i64>,
	/// what it told on its last poll: host, engine, arch, cpus
	pub info: serde_json::Value,
	pub online: bool,
	/// the job it builds right now
	pub job: Option<i64>,
}

pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Runner>> {
	let mut stmt = conn.prepare(
		"SELECT r.id, r.name, r.created_at, r.last_seen, r.info,
			(SELECT j.id FROM build_jobs j
			WHERE j.runner_id = r.id AND j.status = 'running')
		FROM runners r ORDER BY r.name",
	)?;
	let at = now();
	stmt.query_map([], |r| {
		let last_seen: Option<i64> = r.get(3)?;
		Ok(Runner {
			id: r.get(0)?,
			name: r.get(1)?,
			created_at: r.get(2)?,
			last_seen,
			info: serde_json::from_str(&r.get::<_, String>(4)?)
				.unwrap_or_default(),
			online: last_seen.is_some_and(|s| at - s < ONLINE_MS),
			job: r.get(5)?,
		})
	})?
	.collect()
}

/// A new runner, returns its token (shown this once).
pub fn create(conn: &Connection, name: &str) -> Result<String> {
	let name = name.trim();
	let ok = !name.is_empty()
		&& name.len() <= 40
		&& name
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
	if !ok {
		return Err(Error::Invalid(
			"a runner's name is like performance or build-1".into(),
		));
	}
	let token = format!("genco_runner_{}", crate::util::random_hex(24));
	conn.execute(
		"INSERT INTO runners (name, token_hash, created_at) VALUES (?1, ?2, ?3)",
		params![name, crate::tokens::hash(&token), now()],
	)
	.map_err(|e| match e {
		rusqlite::Error::SqliteFailure(f, _)
			if f.code == rusqlite::ErrorCode::ConstraintViolation =>
		{
			Error::Invalid(format!("there is a runner {name} already"))
		}
		e => e.into(),
	})?;
	Ok(token)
}

pub fn remove(conn: &Connection, name: &str) -> Result<()> {
	// its running job fails at the next check of its deploy
	conn.execute(
		"UPDATE build_jobs SET status = 'failed', message = 'the runner was \
		removed', finished_at = ?2 WHERE status = 'running' AND runner_id = \
		(SELECT id FROM runners WHERE name = ?1)",
		params![name, now()],
	)?;
	let n = conn.execute("DELETE FROM runners WHERE name = ?1", [name])?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	Ok(())
}

fn any_registered(conn: &Connection) -> rusqlite::Result<bool> {
	conn.query_row("SELECT EXISTS (SELECT 1 FROM runners)", [], |r| r.get(0))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
	/// how long a deploy waits for a runner to take its build
	#[serde(default = "default_queue_minutes")]
	pub queue_minutes: u32,
}

impl Default for Settings {
	fn default() -> Self {
		Self {
			queue_minutes: QUEUE_MINUTES,
		}
	}
}

fn default_queue_minutes() -> u32 {
	QUEUE_MINUTES
}

pub fn settings(conn: &Connection) -> Settings {
	crate::db::get_json(conn, "runners").unwrap_or_default()
}

/// What a queue timeout may be, in Settings and genco.toml.
pub fn check_queue_minutes(minutes: u32) -> std::result::Result<(), String> {
	if !(1..=24 * 60).contains(&minutes) {
		return Err("a queue timeout is 1 to 1440 minutes".into());
	}
	Ok(())
}

/// What a build for this server has to match.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
	/// `debian`
	pub os: String,
	/// `13`
	pub version: String,
	/// `trixie`
	pub codename: String,
	/// `2.41`
	pub glibc: String,
	/// `x86_64`
	pub arch: String,
}

pub async fn target() -> Target {
	let release =
		std::fs::read_to_string("/etc/os-release").unwrap_or_default();
	let field = |name: &str| {
		release
			.lines()
			.find_map(|l| l.strip_prefix(&format!("{name}=")))
			.unwrap_or("")
			.trim_matches('"')
			.to_string()
	};
	let glibc = sh("ldd --version | head -n1 | awk '{print $NF}'")
		.await
		.unwrap_or_default();
	Target {
		os: field("ID"),
		version: field("VERSION_ID"),
		codename: field("VERSION_CODENAME"),
		glibc,
		arch: std::env::consts::ARCH.into(),
	}
}

/// The image for a step: its own (`{codename}` becomes this server's
/// release), or one for what the folder holds.
fn image(own: Option<&str>, dir: &Path, codename: &str) -> String {
	let image = match own {
		Some(i) => i.to_string(),
		None if dir.join("Cargo.toml").exists() => "rust:1-{codename}".into(),
		None if dir.join("package.json").exists() => {
			"node:24-{codename}".into()
		}
		None => "debian:{codename}".into(),
	};
	qualified(&image.replace("{codename}", codename))
}

/// podman wants the registry in the name.
fn qualified(image: &str) -> String {
	let first = image.split('/').next().unwrap_or("");
	match image.matches('/').count() {
		0 => format!("docker.io/library/{image}"),
		_ if first.contains('.')
			|| first.contains(':')
			|| first == "localhost" =>
		{
			image.to_string()
		}
		_ => format!("docker.io/{image}"),
	}
}

/// An image name ends up in the runner's command line.
pub fn check_image(image: &str) -> std::result::Result<(), String> {
	let ok = !image.is_empty()
		&& image.len() <= 200
		&& !image.starts_with('-')
		&& image
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || "._-/:@{}".contains(c));
	if ok {
		Ok(())
	} else {
		Err(format!("{image:?} is no image name"))
	}
}

/// Where the runner keeps a step's source and caches between builds: per
/// group and folder, only plain names.
fn cache_key(group: &str, dir: &str) -> String {
	let clean = |s: &str| -> String {
		s.chars()
			.map(|c| {
				if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
					c
				} else {
					'_'
				}
			})
			.collect()
	};
	let dir = if dir == "." {
		"_root".into()
	} else {
		clean(dir)
	};
	format!("{}/{dir}", clean(group))
}

/// What a runner gets to build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
	pub id: i64,
	pub image: String,
	pub run: String,
	pub artifacts: Vec<String>,
	pub minutes: u32,
	pub cache_key: String,
}

fn job_dir(id: i64) -> PathBuf {
	Path::new(DIR).join(id.to_string())
}

async fn list_route(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<serde_json::Value>> {
	let runners = list(&db.conn())?;
	Ok(Json(serde_json::json!({
		"runners": runners,
		"target": target().await,
		"version": crate::update::VERSION,
	})))
}

async fn get_settings(_authed: Authed, State(db): State<Db>) -> Json<Settings> {
	Json(settings(&db.conn()))
}

async fn save_settings(
	_authed: Authed,
	State(db): State<Db>,
	Json(new): Json<Settings>,
) -> Result<Json<()>> {
	check_queue_minutes(new.queue_minutes).map_err(Error::Invalid)?;
	crate::db::set_setting(
		&db.conn(),
		"runners",
		&serde_json::to_string(&new).unwrap(),
	)?;
	Ok(Json(()))
}

#[derive(Deserialize)]
struct CreateReq {
	name: String,
	/// the admin's password and code: a runner token takes the groups'
	/// sources
	#[serde(flatten)]
	auth: crate::users::PasswordOnly,
}

async fn create_route(
	authed: Authed,
	State(db): State<Db>,
	Json(req): Json<CreateReq>,
) -> Result<Json<String>> {
	crate::users::verify_password_and_code(&db, &authed.user, &req.auth)
		.await?;
	Ok(Json(create(&db.conn(), &req.name)?))
}

async fn delete_route(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(name): UrlPath<String>,
) -> Result<Json<()>> {
	remove(&db.conn(), &name)?;
	Ok(Json(()))
}

/// What a CI build has to match, for the admins' API tokens.
async fn deploy_target(
	token: crate::tokens::ApiToken,
) -> crate::error::Result<Json<Target>> {
	if token.peer.is_some() {
		return Err(crate::error::Error::Forbidden);
	}
	Ok(Json(target().await))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/runners", get(list_route).post(create_route))
		.route("/runners/{name}", delete(delete_route))
		.route("/settings/runners", get(get_settings).post(save_settings))
		.route("/deploy/target", get(deploy_target))
		.route("/runner/hello", get(hello))
		.route("/runner/jobs/next", post(next_job))
		.route("/runner/jobs/{id}/source", get(source))
		.route("/runner/jobs/{id}/log", post(append_log))
		.route(
			"/runner/jobs/{id}/artifacts",
			put(artifacts).layer(DefaultBodyLimit::max(4 << 30)),
		)
		.route("/runner/jobs/{id}/finish", post(finish))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn images_get_their_registry() {
		assert_eq!(
			qualified("rust:1-trixie"),
			"docker.io/library/rust:1-trixie"
		);
		assert_eq!(qualified("me/img:1"), "docker.io/me/img:1");
		assert_eq!(qualified("ghcr.io/me/img"), "ghcr.io/me/img");
		assert!(check_image("rust:1-{codename}").is_ok());
		assert!(check_image("x; rm -rf ~").is_err());
		assert!(check_image("--privileged").is_err());
	}

	#[test]
	fn cache_keys_are_plain_names() {
		assert_eq!(cache_key("shop", "cms"), "shop/cms");
		assert_eq!(cache_key("b", "."), "b/_root");
		assert_eq!(cache_key("b", "x/$(y)"), "b/x___y_");
	}
}
