//! Daily backups with restic: per group its files and a dump of every
//! database, and the server's config (/etc, genco's database). Snapshots
//! are tagged `group:<name>` and `files`, `db:<name>` or `system`, each
//! kind keeps its own history. The repository is local for now.

use std::path::PathBuf;
use std::process::Stdio;

use axum::extract::{Path as UrlPath, State};
use axum::routing;
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::AppState;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::hosting::{self, Group};
use crate::job::{self, Log};
use crate::render;
use crate::users::Authed;

const REPOSITORY: &str = "/var/backups/genco/restic";
pub const PASSWORD_FILE: &str = "/etc/genco/restic-password";
pub(crate) const LOG_DIR: &str = "/var/lib/genco/backups";
pub const LOCK: &str = "/var/lib/genco/backup.lock";
/// a consistent copy of genco's database, the live one may change while
/// restic reads it
const DB_COPY: &str = "/var/lib/genco/genco-backup.db";

/// Rebuilt from git or downloaded again, not worth the space.
const EXCLUDES: &[&str] = &[
	"node_modules",
	".cache",
	".npm",
	".local",
	".config",
	"repo.git",
];

/// How long snapshots stay, per group and kind.
const KEEP: &[&str] = &[
	// a backup by hand doesn't push out last night's
	"--keep-last",
	"3",
	"--keep-daily",
	"7",
	"--keep-weekly",
	"4",
	"--keep-monthly",
	"6",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
	pub id: i64,
	/// running, done, failed
	pub status: String,
	pub started_by: String,
	pub created_at: i64,
	pub finished_at: Option<i64>,
	/// a backup of only this group
	pub group_id: Option<i64>,
	pub group_name: Option<String>,
}

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Backup> {
	Ok(Backup {
		id: row.get(0)?,
		status: row.get(1)?,
		started_by: row.get(2)?,
		created_at: row.get(3)?,
		finished_at: row.get(4)?,
		group_id: row.get(5)?,
		group_name: row.get(6)?,
	})
}

const COLUMNS: &str = "b.id, b.status, b.started_by, b.created_at, \
	b.finished_at, b.group_id, g.name";
const FROM: &str = "backups b LEFT JOIN domain_groups g ON g.id = b.group_id";

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Backup> {
	conn.query_row(
		&format!("SELECT {COLUMNS} FROM {FROM} WHERE b.id = ?1"),
		[id],
		from_row,
	)
}

fn list(conn: &Connection) -> rusqlite::Result<Vec<Backup>> {
	let mut stmt = conn.prepare(&format!(
		"SELECT {COLUMNS} FROM {FROM} ORDER BY b.id DESC LIMIT 20"
	))?;
	stmt.query_map([], from_row)?.collect()
}

/// restic with genco's repository and password
pub fn restic() -> Command {
	let mut c = Command::new("restic");
	c.env("RESTIC_REPOSITORY", REPOSITORY)
		.env("RESTIC_PASSWORD_FILE", PASSWORD_FILE)
		.env("RESTIC_CACHE_DIR", "/var/cache/restic")
		.stdin(Stdio::null());
	c
}

pub struct Ctx {
	pub db: Db,
	pub events: Events,
	/// `genco backup` prints the log too
	pub print: bool,
}

impl Ctx {
	fn status(&self, id: i64, status: &str) {
		let conn = self.db.conn();
		let finished = (status != "running").then(now);
		let res = conn
			.execute(
				"UPDATE backups SET status = ?1, finished_at = ?2 WHERE id = ?3",
				params![status, finished, id],
			)
			.and_then(|_| get(&conn, id));
		match res {
			Ok(backup) => self.events.send(Event::Backup { backup }),
			Err(e) => tracing::error!("backup {id}: {e}"),
		}
	}
}

/// Runs restic with its errors in the log, false if it failed.
async fn restic_log(log: &mut Log, args: &[&str]) -> bool {
	let mut cmd = Command::new("sh");
	cmd.args(["-c", r#"exec restic "$@" 2>&1"#, "sh"])
		.args(args)
		.env("RESTIC_REPOSITORY", REPOSITORY)
		.env("RESTIC_PASSWORD_FILE", PASSWORD_FILE)
		.env("RESTIC_CACHE_DIR", "/var/cache/restic")
		.stdin(Stdio::null());
	match log.run(cmd).await {
		Ok(()) => true,
		Err(e) => {
			log.error(&format!("restic {}: {e}", args[0])).await;
			false
		}
	}
}

/// Starts a backup run of everything or of one group, returns its id.
pub fn create(
	conn: &Connection,
	started_by: &str,
	group_id: Option<i64>,
) -> rusqlite::Result<i64> {
	conn.execute(
		"INSERT INTO backups (status, started_by, created_at, group_id)
		VALUES ('running', ?1, ?2, ?3)",
		params![started_by, now(), group_id],
	)?;
	Ok(conn.last_insert_rowid())
}

pub async fn run(ctx: &Ctx, id: i64) {
	let mut log = match Log::create(LOG_DIR, id, ctx.print).await {
		Ok(log) => log,
		Err(e) => {
			tracing::error!("backup {id}: {e}");
			ctx.status(id, "failed");
			return;
		}
	};
	ctx.status(id, "running");

	match job::lock(LOCK).await {
		Ok(lock) => {
			let only = get(&ctx.db.conn(), id).ok().and_then(|b| b.group_id);
			backup(ctx, &mut log, only).await;
			drop(lock);
		}
		Err(e) => log.error(&e).await,
	}

	let status = if log.ok { "done" } else { "failed" };
	log.line(status).await;
	ctx.status(id, status);
}

/// Everything, or only the group `only` without the system and cleanups.
async fn backup(ctx: &Ctx, log: &mut Log, only: Option<i64>) {
	if !PathBuf::from(REPOSITORY).join("config").exists() {
		log.line(&format!("creating the repository {REPOSITORY}"))
			.await;
		let _ = std::fs::create_dir_all(REPOSITORY);
		if !restic_log(log, &["init"]).await {
			return;
		}
	}

	// a killed restic leaves its lock behind, genco's own lock makes sure
	// no other backup runs; `unlock` only removes stale locks
	restic_log(log, &["unlock", "--quiet"]).await;

	let groups = hosting::groups(&ctx.db.conn());
	let groups = match groups {
		Ok(g) => g,
		Err(e) => {
			log.error(&e.to_string()).await;
			return;
		}
	};
	if let Some(id) = only {
		let Some(g) = groups.iter().find(|g| g.id == id) else {
			log.error("the group is gone").await;
			return;
		};
		group(log, g).await;
		let tag = format!("group:{}", g.name);
		let mut args =
			vec!["forget", "--quiet", "--tag", &tag, "--group-by", "tags"];
		args.extend(KEEP);
		args.push("--prune");
		restic_log(log, &args).await;
		return;
	}

	for g in &groups {
		group(log, g).await;
	}
	system(ctx, log).await;

	// what the file manager deleted, after 30 days; as the group's user,
	// root must not follow links the group can swap in
	for g in &groups {
		let trash = render::group_dir(&g.name).join(crate::files::TRASH);
		// most groups never deleted anything
		if std::fs::symlink_metadata(&trash).is_err() {
			continue;
		}
		let _ = Command::new("runuser")
			.args(["-u", &render::group_user(&g.name), "--", "find"])
			.arg(&trash)
			.args(["-mindepth", "1", "-maxdepth", "1", "-mtime", "+30"])
			.args(["-exec", "rm", "-rf", "{}", "+"])
			.status()
			.await;
	}
	for path in crate::actions::clean_trash() {
		log.line(&format!("removed {path} from the trash")).await;
	}

	log.line("removing old snapshots").await;
	let mut args = vec![
		"forget",
		"--quiet",
		"--tag",
		"genco",
		"--group-by",
		"tags",
		"--prune",
	];
	args.extend(KEEP);
	restic_log(log, &args).await;
}

/// Where the consistent copies of a group's SQLite files go for the
/// files snapshot, `shared/cms/data/zipp.db` as `shared_cms_data_zipp.db`.
const SQLITE_COPIES: &str = ".genco-sqlite";

/// A live SQLite file copied as it is may be torn in the middle of a write
/// (and its -wal holds the rest): `sqlite3 .backup` copies each one
/// consistently, as the group's user, before the files go into restic.
/// Releases (code) and caches are left out.
const SQLITE_SCRIPT: &str = r#"cd "$1" || exit 0
rm -rf .genco-sqlite
find . -xdev \( -path ./releases -o -path ./.genco-sqlite -o -path ./repo.git \
	-o -path ./.cache -o -name node_modules \) -prune -o -type f \
	\( -name '*.db' -o -name '*.sqlite' -o -name '*.sqlite3' \) -size +0 -print \
| head -n 200 | while IFS= read -r f; do
	[ "$(head -c 15 "$f")" = "SQLite format 3" ] || continue
	command -v sqlite3 >/dev/null || { echo "sqlite3 is missing, $f is copied as it is"; continue; }
	mkdir -p -m 700 .genco-sqlite
	out=".genco-sqlite/$(printf '%s' "${f#./}" | tr "/'\"" '___')"
	if sqlite3 "$f" ".timeout 10000" ".backup \"$out\"" >/dev/null 2>&1; then
		echo "SQLite: copied $f"
	else
		echo "error: SQLite: copying $f failed"
	fi
done"#;

async fn sqlite_copies(log: &mut Log, g: &Group, dir: &str) {
	let mut cmd = Command::new("runuser");
	cmd.args(["-u", &render::group_user(&g.name), "--", "bash", "-c"])
		.args([SQLITE_SCRIPT, "bash", dir])
		.stdin(std::process::Stdio::null());
	if let Err(e) = log.run(cmd).await {
		log.error(&format!("copying the SQLite files: {e}")).await;
	}
}

async fn group(log: &mut Log, g: &Group) {
	let dir = render::group_dir(&g.name);
	let tag = format!("group:{}", g.name);
	log.line(&format!("== {}: files", g.name)).await;
	let dir = dir.display().to_string();
	sqlite_copies(log, g, &dir).await;
	let mut args = vec![
		"backup",
		"--quiet",
		"--host",
		"genco",
		"--tag",
		"genco",
		"--tag",
		&tag,
		"--tag",
		"files",
		"--skip-if-unchanged",
	];
	for e in EXCLUDES {
		args.extend(["--exclude", e]);
	}
	args.push(&dir);
	restic_log(log, &args).await;
	// the snapshot has them, the next backup makes new ones
	let _ = Command::new("runuser")
		.args(["-u", &render::group_user(&g.name), "--", "rm", "-rf", "--"])
		.arg(format!("{dir}/{SQLITE_COPIES}"))
		.status()
		.await;

	let mail: Vec<String> = g
		.mail_domains
		.iter()
		.map(|d| format!("{}/{}", crate::mailbox::VMAIL, d.name))
		.filter(|p| std::path::Path::new(p).exists())
		.collect();
	if !mail.is_empty() {
		log.line(&format!("== {}: mail", g.name)).await;
		let mut args = vec![
			"backup",
			"--quiet",
			"--host",
			"genco",
			"--tag",
			"genco",
			"--tag",
			&tag,
			"--tag",
			"mail",
			"--skip-if-unchanged",
		];
		args.extend(mail.iter().map(String::as_str));
		restic_log(log, &args).await;
	}

	for d in &g.databases {
		log.line(&format!("== {}: database {}", g.name, d.name))
			.await;
		let db_tag = format!("db:{}", d.name);
		let file = format!("{}.sql", d.name);
		let dump = format!("exec {}", d.dump_command());
		restic_log(
			log,
			&[
				"backup",
				"--quiet",
				"--host",
				"genco",
				"--tag",
				"genco",
				"--tag",
				&tag,
				"--tag",
				&db_tag,
				"--stdin-filename",
				&file,
				"--stdin-from-command",
				"--",
				"sh",
				"-c",
				&dump,
			],
		)
		.await;
	}
}

async fn system(ctx: &Ctx, log: &mut Log) {
	log.line("== system: /etc and genco's database").await;
	let _ = std::fs::remove_file(DB_COPY);
	let copied = ctx
		.db
		.conn()
		.execute("VACUUM INTO ?1", [DB_COPY])
		.map_err(|e| e.to_string());
	if let Err(e) = copied {
		log.error(&format!("copying genco's database: {e}")).await;
	}
	restic_log(
		log,
		&[
			"backup",
			"--quiet",
			"--host",
			"genco",
			"--tag",
			"genco",
			"--tag",
			"system",
			"--skip-if-unchanged",
			"--exclude",
			"/var/lib/genco/genco.db*",
			// usage stats, big and not worth restoring
			"--exclude",
			"/var/lib/genco/stats.db*",
			"/etc",
			"/var/lib/genco",
		],
	)
	.await;
	let _ = std::fs::remove_file(DB_COPY);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
	pub id: String,
	pub time: String,
	pub tags: Vec<String>,
	/// size of the backed up data
	pub bytes: u64,
}

/// The snapshots with this tag, newest first.
pub async fn snapshots(tag: &str) -> Result<Vec<Snapshot>> {
	if !PathBuf::from(REPOSITORY).join("config").exists() {
		return Ok(vec![]);
	}
	let out = restic()
		.args(["snapshots", "--json", "--no-lock", "--tag", tag])
		.output()
		.await
		.map_err(|e| Error::Internal(format!("restic: {e}")))?;
	if !out.status.success() {
		return Err(Error::Internal(
			String::from_utf8_lossy(&out.stderr).trim().to_string(),
		));
	}
	parse_snapshots(&out.stdout)
}

fn parse_snapshots(json: &[u8]) -> Result<Vec<Snapshot>> {
	#[derive(Deserialize)]
	struct Raw {
		short_id: String,
		time: String,
		#[serde(default)]
		tags: Vec<String>,
		summary: Option<Summary>,
	}
	#[derive(Deserialize)]
	struct Summary {
		total_bytes_processed: u64,
	}

	let raw: Vec<Raw> = serde_json::from_slice(json)
		.map_err(|e| Error::Internal(format!("restic snapshots: {e}")))?;
	let mut snaps: Vec<Snapshot> = raw
		.into_iter()
		.map(|r| Snapshot {
			id: r.short_id,
			time: r.time,
			tags: r.tags.into_iter().filter(|t| t != "genco").collect(),
			bytes: r.summary.map(|s| s.total_bytes_processed).unwrap_or(0),
		})
		.collect();
	snaps.reverse();
	Ok(snaps)
}

fn start(
	state: &AppState,
	started_by: &str,
	group_id: Option<i64>,
) -> Result<i64> {
	let id = create(&state.db.conn(), started_by, group_id)?;
	let ctx = Ctx {
		db: state.db.clone(),
		events: state.events.clone(),
		print: false,
	};
	tokio::spawn(async move { run(&ctx, id).await });
	Ok(id)
}

async fn backup_now(
	authed: Authed,
	State(state): State<AppState>,
) -> Result<Json<i64>> {
	Ok(Json(start(&state, &authed.user.name, None)?))
}

async fn backup_group(
	authed: Authed,
	State(state): State<AppState>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<i64>> {
	state.db.conn().query_row(
		"SELECT id FROM domain_groups WHERE id = ?1",
		[id],
		|r| r.get::<_, i64>(0),
	)?;
	Ok(Json(start(&state, &authed.user.name, Some(id))?))
}

async fn list_backups(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<Backup>>> {
	Ok(Json(list(&db.conn())?))
}

async fn backup_log(
	_authed: Authed,
	UrlPath(id): UrlPath<i64>,
) -> Result<String> {
	Ok(job::read(LOG_DIR, id).await)
}

async fn group_snapshots(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<Vec<Snapshot>>> {
	let group = hosting::find_group(&db.conn(), id)?;
	Ok(Json(snapshots(&format!("group:{}", group.name)).await?))
}

async fn system_snapshots(_authed: Authed) -> Result<Json<Vec<Snapshot>>> {
	Ok(Json(snapshots("system").await?))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/backups", routing::get(list_backups).post(backup_now))
		.route("/backups/system", routing::get(system_snapshots))
		.route("/backups/{id}/log", routing::get(backup_log))
		.route("/groups/{id}/snapshots", routing::get(group_snapshots))
		.route("/groups/{id}/backup", routing::post(backup_group))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn snapshots_newest_first_without_the_genco_tag() {
		let json = br#"[
			{"short_id":"aaa","time":"2026-11-01T03:00:00Z","tags":["genco","group:shop","files"],
			 "summary":{"total_bytes_processed":100}},
			{"short_id":"bbb","time":"2026-11-02T03:00:00Z","tags":["genco","group:shop","db:shop"]}
		]"#;
		let snaps = parse_snapshots(json).unwrap();
		assert_eq!(snaps[0].id, "bbb");
		assert_eq!(snaps[0].tags, ["group:shop", "db:shop"]);
		assert_eq!(snaps[1].bytes, 100);
	}
}
