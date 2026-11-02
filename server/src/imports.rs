//! Imports a dump into a group's database: a `.sql`, `.sql.gz` or a zip
//! with one. The current content goes to the trash first. The dump runs
//! as the group's own database user, so whatever it contains (`USE`, other
//! databases, `DEFINER`s) reaches only the group's databases.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::Serialize;
use tokio::process::Command;

use crate::AppState;
use crate::actions::TRASH;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::hosting;
use crate::job::{self, Log};
use crate::render;
use crate::users::Authed;

const DIR: &str = "/var/lib/genco/imports";
/// like nginx lets through to the panel
const MAX_UPLOAD: usize = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Import {
	pub id: i64,
	pub database_id: i64,
	/// running, done, failed
	pub status: String,
	pub started_by: String,
	pub created_at: i64,
	pub finished_at: Option<i64>,
}

fn get_import(conn: &Connection, id: i64) -> rusqlite::Result<Import> {
	conn.query_row(
		"SELECT id, database_id, status, started_by, created_at, finished_at
		FROM db_imports WHERE id = ?1",
		[id],
		|r| {
			Ok(Import {
				id: r.get(0)?,
				database_id: r.get(1)?,
				status: r.get(2)?,
				started_by: r.get(3)?,
				created_at: r.get(4)?,
				finished_at: r.get(5)?,
			})
		},
	)
}

fn set_status(db: &Db, id: i64, status: &str) {
	let res = db.conn().execute(
		"UPDATE db_imports SET status = ?1, finished_at = ?2 WHERE id = ?3",
		params![status, (status != "running").then(now), id],
	);
	if let Err(e) = res {
		tracing::error!("import {id}: {e}");
	}
}

/// The database and its group's name.
fn target(
	conn: &Connection,
	database_id: i64,
) -> Result<(hosting::Database, String)> {
	let (g, d) = hosting::find_database(conn, |d| d.id == database_id)?;
	Ok((d, g.name))
}

/// A new import of `database_id`, its file goes to `upload_path`. The
/// lock marks it as running from the upload to the end, pass it to `run`.
pub fn create(
	conn: &Connection,
	database_id: i64,
	started_by: &str,
) -> Result<(i64, std::fs::File)> {
	target(conn, database_id)?;
	let running: Vec<i64> = conn
		.prepare(
			"SELECT id FROM db_imports WHERE database_id = ?1
			AND status = 'running'",
		)?
		.query_map([database_id], |r| r.get(0))?
		.collect::<rusqlite::Result<_>>()?;
	for id in running {
		// its process holds the lock while it runs; a free one was killed
		// (genco restarted, the terminal closed)
		if !job::is_free(&lock_path(id)) {
			return Err(Error::Invalid("an import of it runs already".into()));
		}
		conn.execute(
			"UPDATE db_imports SET status = 'failed', finished_at = ?1
			WHERE id = ?2",
			params![now(), id],
		)?;
	}
	conn.execute(
		"INSERT INTO db_imports (database_id, status, started_by, created_at)
		VALUES (?1, 'running', ?2, ?3)",
		params![database_id, started_by, now()],
	)?;
	let id = conn.last_insert_rowid();
	let lock = job::try_lock(&lock_path(id))
		.map_err(|e| Error::Internal(format!("import lock: {e}")))?
		.ok_or_else(|| Error::Internal("import lock: held".into()))?;
	Ok((id, lock))
}

fn lock_path(id: i64) -> String {
	format!("{DIR}/{id}.lock")
}

pub fn upload_path(id: i64) -> PathBuf {
	Path::new(DIR).join(format!("{id}.upload"))
}

/// The import itself, its log in DIR.
pub async fn run(db: &Db, id: i64, lock: std::fs::File, print: bool) -> bool {
	let upload = upload_path(id);
	let mut log = match Log::create(DIR, id, print).await {
		Ok(log) => log,
		Err(e) => {
			tracing::error!("import {id}: {e}");
			set_status(db, id, "failed");
			let _ = std::fs::remove_file(&upload);
			return false;
		}
	};
	let ok = import(db, id, &upload, &mut log).await;
	let _ = std::fs::remove_file(&upload);
	let status = if ok && log.ok { "done" } else { "failed" };
	log.line(status).await;
	set_status(db, id, status);
	drop(lock);
	let _ = std::fs::remove_file(lock_path(id));
	status == "done"
}

async fn import(db: &Db, id: i64, upload: &Path, log: &mut Log) -> bool {
	// one lock for both, a second db.conn() in the same statement waits
	// for the first forever
	let found = {
		let conn = db.conn();
		get_import(&conn, id)
			.map_err(Error::from)
			.and_then(|i| target(&conn, i.database_id))
	};
	let (database, group) = match found {
		Ok(t) => t,
		Err(e) => {
			log.error(&e.to_string()).await;
			return false;
		}
	};

	// what it was, in case the dump was the wrong one
	let _ = std::fs::create_dir_all(TRASH);
	let name = database.name.clone();
	let backup = format!(
		"{TRASH}/db-{name}-before-import-{}.sql.gz",
		crate::util::stamp().await
	);
	log.line(&format!("saving the current content to {backup}"))
		.await;
	let mut dump = Command::new("bash");
	dump.args([
		"-c",
		&format!(
			"set -o pipefail; exec 2>&1; {} | gzip > \"$1\"",
			database.dump_command()
		),
		"bash",
		&backup,
	]);
	if let Err(e) = log.run(dump).await {
		log.error(&format!("saving the current content: {e}")).await;
		return false;
	}

	// zip, gzip or plain by its first bytes; `CREATE DATABASE` and `USE`
	// go, the dump lands in this database whatever it was called before;
	// DEFINERs need rights the group doesn't have
	log.line(&format!(
		"importing into {name} as {}",
		render::group_user(&group)
	))
	.await;
	let script = r#"set -o pipefail; exec 2>&1
case "$(head -c 2 "$1" | od -An -tx1 | tr -d ' ')" in
	1f8b) read=(gzip -dc -- "$1") ;;
	504b) read=(unzip -p -- "$1" '*.sql') ;;
	*) read=(cat -- "$1") ;;
esac
"${read[@]}" \
	| sed -E -e '/^CREATE DATABASE /d' -e '/^USE `/d' \
		-e 's/DEFINER=`[^`]*`@`[^`]*`//g' \
	| runuser -u "$2" -- mariadb --database="$3"
"#;
	let script = match database.engine {
		hosting::Engine::Mariadb => script,
		hosting::Engine::Postgres => PG_SCRIPT,
	};
	let mut cmd = Command::new("bash");
	cmd.args(["-c", script, "bash"])
		.arg(upload)
		.args([render::group_user(&group), name.clone()]);
	if let Err(e) = log.run(cmd).await {
		log.error(&format!("import: {e}; the old content is in {backup}"))
			.await;
		return false;
	}
	log.line("imported").await;
	true
}

/// A plain SQL dump into a PostgreSQL database as the group's role, in
/// one transaction: everything the role owns there goes first (all its
/// schemas, not only `public`), a failure leaves
/// everything as it was. What only a superuser may do (owners, extension
/// comments, `\connect`) goes. The file has to unpack completely first,
/// half a dump would be committed.
const PG_SCRIPT: &str = r#"set -o pipefail; exec 2>&1
case "$(head -c 2 "$1" | od -An -tx1 | tr -d ' ')" in
	1f8b) read=(gzip -dc -- "$1") ;;
	504b) read=(unzip -p -- "$1" '*.sql') ;;
	*) read=(cat -- "$1") ;;
esac
"${read[@]}" >/dev/null || { echo "the file doesn't unpack"; exit 3; }
{
	echo 'DROP OWNED BY CURRENT_USER; CREATE SCHEMA IF NOT EXISTS public;'
	"${read[@]}" | sed -E -e '/^CREATE DATABASE /d' -e '/^\\c(onnect)? /d' \
		-e '/ OWNER TO /d' -e '/^COMMENT ON EXTENSION /d'
} | runuser -u "$2" -- psql -qX -v ON_ERROR_STOP=1 --single-transaction --dbname="$3""#;

/// The upload goes to a file first, then the import starts.
async fn import_route(
	authed: Authed,
	State(db): State<Db>,
	UrlPath(database_id): UrlPath<i64>,
	body: Body,
) -> Result<Json<i64>> {
	let (id, lock) = create(&db.conn(), database_id, &authed.user.name)?;
	let path = upload_path(id);
	if let Err(e) =
		crate::util::body_to_file(body, &path, MAX_UPLOAD as u64).await
	{
		set_status(&db, id, "failed");
		return Err(Error::Invalid(format!("upload: {e}")));
	}

	let db2 = db.clone();
	tokio::spawn(async move { run(&db2, id, lock, false).await });
	Ok(Json(id))
}

async fn import_get(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<Import>> {
	Ok(Json(get_import(&db.conn(), id)?))
}

async fn import_log(_authed: Authed, UrlPath(id): UrlPath<i64>) -> String {
	job::read(DIR, id).await
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route(
			"/databases/{id}/import",
			put(import_route).layer(DefaultBodyLimit::max(MAX_UPLOAD)),
		)
		.route("/imports/{id}", get(import_get))
		.route("/imports/{id}/log", get(import_log))
}
