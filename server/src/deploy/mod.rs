//! Deploys from git. Every deploy builds a new release in
//! `releases/<stamp>/` next to `shared/` (`.env`, uploads, storage), then
//! switches `current` to it: PHP serves the new code at once, the steps
//! after the switch run (`craft up`), then the apps restart. A failed
//! build leaves the live site alone. One deploy at a time per group, and
//! one waiting: a newer one takes its place.
//! Build steps may run on a build runner (see runners.rs), and a CI can
//! upload a release it built (`PUT /deploy/artifact`) instead of genco
//! fetching the repository. Before the switch, the release's binaries
//! have to find their libraries here (glibc).

mod build;
mod config;
mod routes;
mod shell;
mod slot;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use axum::extract::{Path as UrlPath, State};
use axum::routing::{self, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::AppState;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::hosting::{self, Group};
use crate::job::{self, Log};
use crate::procs::{self, Schedule, Worker};
use crate::render;
use crate::tokens::{self, ApiToken, Caller};
use crate::users::Authed;
use crate::util;

use build::*;
use config::*;
pub use routes::routes;
use shell::*;

/// steps of a deploy fail with what to show in its log
type Res<T> = std::result::Result<T, String>;

const KEEP_RELEASES: usize = 5;
pub(crate) const LOG_DIR: &str = "/var/lib/genco/deploys";
/// One deploy or rollback of a group at a time, other groups go on.
pub fn lock_path(group: &str) -> String {
	format!("{LOG_DIR}/{group}.lock")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deploy {
	pub id: i64,
	pub group_id: i64,
	#[serde(rename = "ref")]
	pub git_ref: String,
	pub commit_id: Option<String>,
	pub release: Option<String>,
	/// queued, running, done, failed
	pub status: String,
	pub started_by: String,
	pub created_at: i64,
	pub finished_at: Option<i64>,
	/// its release is the one `current` points to; also a failed deploy
	/// whose step after the switch (`craft up`) failed
	pub live: bool,
	/// its build waits for a runner at this place in the queue (1 next)
	pub queue: Option<i64>,
}

const COLUMNS: &str = "id, group_id, ref, commit_id, release, status, \
	started_by, created_at, finished_at, \
	(SELECT count(*) FROM build_jobs q WHERE q.status = 'queued' \
	AND q.id <= (SELECT max(j.id) FROM build_jobs j \
	WHERE j.deploy_id = deploys.id AND j.status = 'queued'))";

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Deploy> {
	Ok(Deploy {
		id: row.get(0)?,
		group_id: row.get(1)?,
		git_ref: row.get(2)?,
		commit_id: row.get(3)?,
		release: row.get(4)?,
		status: row.get(5)?,
		started_by: row.get(6)?,
		created_at: row.get(7)?,
		finished_at: row.get(8)?,
		live: false,
		queue: Some(row.get(9)?).filter(|n| *n > 0),
	})
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Deploy> {
	conn.query_row(
		&format!("SELECT {COLUMNS} FROM deploys WHERE id = ?1"),
		[id],
		from_row,
	)
}

fn list(conn: &Connection, group_id: i64) -> rusqlite::Result<Vec<Deploy>> {
	let mut stmt = conn.prepare(&format!(
		"SELECT {COLUMNS} FROM deploys WHERE group_id = ?1
		ORDER BY id DESC LIMIT 30"
	))?;
	stmt.query_map([group_id], from_row)?.collect()
}

/// A queued deploy: `new` unless one waited already and got the ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Queued {
	pub id: i64,
	pub new: bool,
}

/// Queues a deploy of `git_ref` (a branch, tag or commit, the group's
/// branch by default). `run` does it, if the deploy is new: one that waits
/// already only gets the newer ref.
pub fn queue(
	conn: &Connection,
	group: &Group,
	git_ref: Option<&str>,
	started_by: &str,
) -> Result<Queued> {
	if group.repo.is_none() {
		return Err(Error::Invalid(format!(
			"the group {} has no git repository",
			group.name
		)));
	}
	let git_ref = git_ref.unwrap_or(&group.branch);
	check_ref(git_ref)?;
	// a CI pushing fast queues one deploy, the newest ref wins
	// unless it started in this moment
	if let Some(id) = waiting(conn, group.id)? {
		let n = conn.execute(
			"UPDATE deploys SET ref = ?1, started_by = ?2
			WHERE id = ?3 AND status = 'queued'",
			params![git_ref, started_by, id],
		)?;
		if n == 1 {
			return Ok(Queued { id, new: false });
		}
	}
	conn.execute(
		"INSERT INTO deploys (group_id, ref, status, started_by, created_at)
		VALUES (?1, ?2, 'queued', ?3, ?4)",
		params![group.id, git_ref, started_by, now()],
	)?;
	Ok(Queued {
		id: conn.last_insert_rowid(),
		new: true,
	})
}

/// The deploy of the group that waits for its turn, if any (a rollback
/// has its release already and doesn't count). A waiting upload counts
/// too: its file can't be swapped.
fn waiting(conn: &Connection, group_id: i64) -> Result<Option<i64>> {
	let found: Option<i64> = conn
		.query_row(
			"SELECT id FROM deploys WHERE group_id = ?1 AND status = 'queued'
			AND release IS NULL ORDER BY id LIMIT 1",
			[group_id],
			|r| r.get(0),
		)
		.optional()?;
	match found {
		Some(id) if upload_path(id).exists() => Err(Error::Invalid(format!(
			"an uploaded release of the group waits already (#{id})"
		))),
		found => Ok(found),
	}
}

/// A branch, tag or commit; it ends up in logs, alerts and the agent's
/// conversation, a CI token may send anything.
fn check_ref(git_ref: &str) -> Result<()> {
	let ok = !git_ref.is_empty()
		&& git_ref.len() <= 100
		&& !git_ref.starts_with(['-', '/', '.'])
		&& !git_ref.contains("..")
		&& git_ref.chars().all(|c| {
			c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-')
		});
	if !ok {
		return Err(Error::Invalid(format!("invalid ref {git_ref:?}")));
	}
	Ok(())
}

/// Queues a deploy of a release built elsewhere; its tarball goes to
/// `upload_path` before `run`. `label` names it (a tag, a commit).
pub fn queue_upload(
	conn: &Connection,
	group: &Group,
	label: Option<&str>,
	commit: Option<&str>,
	started_by: &str,
) -> Result<i64> {
	let label = label.unwrap_or("upload");
	check_ref(label)?;
	if let Some(id) = waiting(conn, group.id)? {
		return Err(Error::Invalid(format!(
			"a deploy of the group waits already (#{id}), upload again \
			once it ran"
		)));
	}
	if let Some(c) = commit {
		let ok = (7..=64).contains(&c.len())
			&& c.chars().all(|c| c.is_ascii_hexdigit());
		if !ok {
			return Err(Error::Invalid(format!("invalid commit {c:?}")));
		}
	}
	conn.execute(
		"INSERT INTO deploys (group_id, ref, commit_id, status, started_by,
		created_at)
		VALUES (?1, ?2, ?3, 'queued', ?4, ?5)",
		params![group.id, label, commit, started_by, now()],
	)?;
	Ok(conn.last_insert_rowid())
}

/// Where an uploaded release waits for its deploy.
pub fn upload_path(id: i64) -> PathBuf {
	Path::new(LOG_DIR).join(format!("{id}.upload"))
}

/// Writes the deploy's log and tells the browsers about status changes.
pub struct Ctx {
	pub cfg: std::sync::Arc<crate::config::Config>,
	pub db: Db,
	pub events: Events,
	/// `genco deploy` prints the log too
	pub print: bool,
}

impl Ctx {
	/// Runs `sql` on the deploy, then tells the browsers.
	fn set(&self, id: i64, sql: &str, params: impl rusqlite::Params) {
		let conn = self.db.conn();
		let res = conn.execute(sql, params).and_then(|_| get(&conn, id));
		match res {
			Ok(deploy) => self.events.send(Event::Deploy { deploy }),
			Err(e) => tracing::error!("deploy {id}: {e}"),
		}
	}

	pub fn status(&self, id: i64, status: &str) {
		let finished = matches!(status, "done" | "failed").then(now);
		self.set(
			id,
			"UPDATE deploys SET status = ?1,
			finished_at = coalesce(?2, finished_at) WHERE id = ?3",
			params![status, finished, id],
		);
	}

	/// Tells the browsers about the deploy as it is.
	fn changed(&self, id: i64) {
		match get(&self.db.conn(), id) {
			Ok(deploy) => self.events.send(Event::Deploy { deploy }),
			Err(e) => tracing::error!("deploy {id}: {e}"),
		}
	}

	fn set_commit(&self, id: i64, commit: &str) {
		self.set(
			id,
			"UPDATE deploys SET commit_id = ?1 WHERE id = ?2",
			params![commit, id],
		);
	}

	fn set_release(&self, id: i64, stamp: &str) {
		self.set(
			id,
			"UPDATE deploys SET release = ?1 WHERE id = ?2",
			params![stamp, id],
		);
	}
}

/// What deploys and rollbacks share: the log, the group's lock (waits for
/// the run before), the status from running to done or failed. `work`
/// returns the last line of a run that worked.
async fn with_job(
	ctx: &Ctx,
	id: i64,
	group: Res<String>,
	work: impl AsyncFnOnce(&mut Log) -> Res<&'static str>,
) {
	let mut log = match Log::create(LOG_DIR, id, ctx.print).await {
		Ok(log) => log,
		Err(e) => {
			tracing::error!("deploy {id}: {e}");
			ctx.status(id, "failed");
			return;
		}
	};
	let lock = match group {
		Ok(group) => job::lock(lock_path(&group)).await,
		Err(e) => Err(e),
	};
	let lock = match lock {
		Ok(lock) => lock,
		Err(e) => {
			log.error(&e).await;
			ctx.status(id, "failed");
			return;
		}
	};
	ctx.status(id, "running");
	let res = work(&mut log).await;
	drop(lock);
	match res {
		Ok(done) => {
			log.line(done).await;
			ctx.status(id, "done");
		}
		Err(e) => {
			log.error(&e).await;
			ctx.status(id, "failed");
		}
	}
}

/// Runs a queued deploy, waits for others of the group to finish first.
pub async fn run(ctx: &Ctx, id: i64) {
	// one connection: a second db.conn() in the statement waits forever
	let group = {
		let conn = ctx.db.conn();
		get(&conn, id)
			.map_err(Error::from)
			.and_then(|d| hosting::find_group(&conn, d.group_id))
			.map(|g| g.name)
			.map_err(|e| e.to_string())
	};
	with_job(ctx, id, group, async |log: &mut Log| {
		deploy(ctx, id, log).await.map(|()| "deployed")
	})
	.await;
	let _ = std::fs::remove_file(upload_path(id));
}

/// Queues the switch of `current` back to the release of an earlier
/// deploy, `run_rollback` does it. The database stays as it is.
fn queue_rollback(
	conn: &Connection,
	group: &Group,
	to: &Deploy,
	started_by: &str,
) -> Result<i64> {
	let release = to
		.release
		.clone()
		.ok_or(Error::Invalid("that deploy has no release".into()))?;
	let base = render::group_dir(&group.name);
	if to.status != "done" || !base.join("releases").join(&release).exists() {
		return Err(Error::Invalid(
			"that release is not there anymore or failed".into(),
		));
	}

	conn.execute(
		"INSERT INTO deploys (group_id, ref, commit_id, release, status,
		started_by, created_at)
		VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6)",
		params![
			group.id,
			format!("rollback to #{}", to.id),
			to.commit_id,
			release,
			started_by,
			now()
		],
	)?;
	Ok(conn.last_insert_rowid())
}

/// Runs a queued rollback after the deploys of the group before it.
async fn run_rollback(ctx: &Ctx, id: i64, group: &Group) {
	let release = get(&ctx.db.conn(), id).ok().and_then(|d| d.release);
	with_job(ctx, id, Ok(group.name.clone()), async |log: &mut Log| {
		let release = release.ok_or("the rollback has no release")?;
		let base = render::group_dir(&group.name);
		// a deploy before it may have pruned that release meanwhile
		if !base.join("releases").join(&release).is_dir() {
			return Err(format!(
				"the release {release} is gone, nothing changed"
			));
		}
		switch(log, &base, &release, &render::group_user(&group.name)).await?;
		restart(ctx, log, group).await?;
		Ok("rolled back, the database stays as it is")
	})
	.await;
}

/// The public key the group fetches its repository with, read as the
/// group's user: the group could link it to a file only root may read.
pub async fn deploy_key(group: &Group) -> Option<String> {
	let path = render::deploy_key_path(&group.name).with_extension("pub");
	let out = Command::new("runuser")
		.args(["-u", &render::group_user(&group.name), "--", "cat", "--"])
		.arg(path)
		.output()
		.await
		.ok()?;
	out.status
		.success()
		.then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn one_deploy_waits_per_group_with_the_newest_ref() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0)",
			[],
		)
		.unwrap();
		conn.execute(
			"INSERT INTO domain_groups (name, client_id, created_at)
			VALUES ('g', 1, 0)",
			[],
		)
		.unwrap();
		let group = Group {
			id: 1,
			name: "g".into(),
			repo: Some("git@x:y.git".into()),
			branch: "main".into(),
			..Group::default()
		};
		let a = queue(&conn, &group, None, "a").unwrap();
		assert!(a.new);
		let b = queue(&conn, &group, Some("v2"), "b").unwrap();
		assert_eq!(b, Queued { new: false, ..a });
		let a = a.id;
		assert_eq!(get(&conn, a).unwrap().git_ref, "v2");
		// once it runs, the next one waits on its own
		conn.execute("UPDATE deploys SET status = 'running'", [])
			.unwrap();
		let c = queue(&conn, &group, None, "c").unwrap();
		assert!(c.new && c.id != a);
	}
}
