//! What the runners call, with their token: take a job, fetch its
//! source, send the log (also the heartbeat), the artifacts, the end.

use super::*;

/// A runner, from `Authorization: Bearer genco_runner_…`.
pub struct RunnerAuth {
	pub id: i64,
	pub name: String,
}

impl<S> FromRequestParts<S> for RunnerAuth
where
	Db: FromRef<S>,
	S: Send + Sync,
{
	type Rejection = Error;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
		let token = crate::tokens::bearer(&parts.headers)
			.ok_or(Error::MissingSessionToken)?;
		let db = Db::from_ref(state);
		let conn = db.conn();
		let (id, name) = conn
			.query_row(
				"SELECT id, name FROM runners WHERE token_hash = ?1",
				[crate::tokens::hash(token.trim())],
				|r| Ok((r.get(0)?, r.get(1)?)),
			)
			.optional()?
			.ok_or(Error::InvalidToken)?;
		conn.execute(
			"UPDATE runners SET last_seen = ?1 WHERE id = ?2",
			params![now(), id],
		)?;
		Ok(Self { id, name })
	}
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerInfo {
	#[serde(default)]
	pub host: String,
	#[serde(default)]
	pub engine: String,
	#[serde(default)]
	pub arch: String,
	#[serde(default)]
	pub cpus: u32,
	#[serde(default)]
	pub version: String,
}

/// Who the server is, for `genco runner register`.
pub(super) async fn hello(
	me: RunnerAuth,
	State(cfg): State<std::sync::Arc<crate::config::Config>>,
) -> Json<serde_json::Value> {
	Json(serde_json::json!({
		"runner": me.name,
		"server": cfg.domain,
		"target": target().await,
	}))
}

/// The oldest queued job, waits up to `POLL` for one.
pub(super) async fn next_job(
	me: RunnerAuth,
	State(db): State<Db>,
	Json(info): Json<RunnerInfo>,
) -> Result<(StatusCode, Json<Option<Job>>)> {
	db.conn().execute(
		"UPDATE runners SET info = ?1 WHERE id = ?2",
		params![serde_json::to_string(&info).unwrap(), me.id],
	)?;
	// a build for another architecture wouldn't run here
	if info.arch != std::env::consts::ARCH {
		return Ok((StatusCode::NO_CONTENT, Json(None)));
	}
	let started = Instant::now();
	while started.elapsed() < POLL {
		if let Some(job) = claim(&db, me.id)? {
			return Ok((StatusCode::OK, Json(Some(job))));
		}
		tokio::time::sleep(Duration::from_secs(1)).await;
		db.conn().execute(
			"UPDATE runners SET last_seen = ?1 WHERE id = ?2",
			params![now(), me.id],
		)?;
	}
	Ok((StatusCode::NO_CONTENT, Json(None)))
}

pub(super) fn claim(db: &Db, runner: i64) -> Result<Option<Job>> {
	let conn = db.conn();
	let found = conn
		.query_row(
			"SELECT id, image, run, artifacts, minutes, cache_key
			FROM build_jobs WHERE status = 'queued' ORDER BY id LIMIT 1",
			[],
			|r| {
				Ok(Job {
					id: r.get(0)?,
					image: r.get(1)?,
					run: r.get(2)?,
					artifacts: serde_json::from_str(&r.get::<_, String>(3)?)
						.unwrap_or_default(),
					minutes: r.get(4)?,
					cache_key: r.get(5)?,
				})
			},
		)
		.optional()?;
	let Some(job) = found else {
		return Ok(None);
	};
	let at = now();
	let n = conn.execute(
		"UPDATE build_jobs SET status = 'running', runner_id = ?2,
		started_at = ?3, heartbeat_at = ?3 WHERE id = ?1 AND status = 'queued'
		AND EXISTS (SELECT 1 FROM runners WHERE id = ?2)",
		params![job.id, runner, at],
	)?;
	Ok((n == 1).then_some(job))
}

/// The job if this runner builds it right now, else 410: the deploy gave
/// up on it, the runner stops.
pub(super) fn mine(db: &Db, me: &RunnerAuth, id: i64) -> Result<()> {
	let ok: bool = db.conn().query_row(
		"SELECT EXISTS (SELECT 1 FROM build_jobs WHERE id = ?1
		AND runner_id = ?2 AND status = 'running')",
		params![id, me.id],
		|r| r.get(0),
	)?;
	if ok { Ok(()) } else { Err(Error::Gone) }
}

pub(super) async fn source(
	me: RunnerAuth,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Body> {
	mine(&db, &me, id)?;
	let file = tokio::fs::File::open(job_dir(id).join("source.tar"))
		.await
		.map_err(|_| Error::NotFound)?;
	Ok(Body::from_stream(tokio_util::io::ReaderStream::new(file)))
}

/// Log lines, also the runner's heartbeat.
pub(super) async fn append_log(
	me: RunnerAuth,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	body: String,
) -> Result<Json<()>> {
	mine(&db, &me, id)?;
	db.conn().execute(
		"UPDATE build_jobs SET heartbeat_at = ?1 WHERE id = ?2",
		params![now(), id],
	)?;
	let path = job_dir(id).join("log");
	let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
	if size < MAX_LOG && !body.is_empty() {
		let mut f = tokio::fs::OpenOptions::new()
			.create(true)
			.append(true)
			.open(&path)
			.await
			.map_err(|e| Error::Internal(e.to_string()))?;
		let text = if size + body.len() as u64 >= MAX_LOG {
			format!("{body}\n… more output, not logged\n")
		} else {
			body
		};
		f.write_all(text.as_bytes())
			.await
			.map_err(|e| Error::Internal(e.to_string()))?;
	}
	Ok(Json(()))
}

pub(super) async fn artifacts(
	me: RunnerAuth,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	body: Body,
) -> Result<Json<()>> {
	mine(&db, &me, id)?;
	let path = job_dir(id).join("artifacts.tar");
	crate::util::body_to_file(body, &path)
		.await
		.map_err(|e| Error::Invalid(format!("upload: {e}")))?;
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub(super) struct Finish {
	pub(super) ok: bool,
	#[serde(default)]
	pub(super) message: String,
}

pub(super) async fn finish(
	me: RunnerAuth,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Json(f): Json<Finish>,
) -> Result<Json<()>> {
	mine(&db, &me, id)?;
	let message: String = f.message.chars().take(500).collect();
	db.conn().execute(
		"UPDATE build_jobs SET status = ?2, message = ?3, finished_at = ?4
		WHERE id = ?1 AND status = 'running'",
		params![id, if f.ok { "done" } else { "failed" }, message, now()],
	)?;
	Ok(Json(()))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_job_goes_to_one_runner() {
		let db = Db::memory();
		let token = create(&db.conn(), "perf").unwrap();
		assert!(token.starts_with("genco_runner_"));
		assert!(create(&db.conn(), "perf").is_err());
		assert!(create(&db.conn(), "a b").is_err());
		let id = db
			.conn()
			.query_row("SELECT id FROM runners", [], |r| r.get(0))
			.unwrap();
		db.conn()
			.execute(
				"INSERT INTO build_jobs (deploy_id, cache_key, image, run,
				artifacts, minutes, status, created_at)
				VALUES (1, 'g/x', 'img', 'make', '[\"a\"]', 5, 'queued', 0)",
				[],
			)
			.unwrap();
		let job = claim(&db, id).unwrap().unwrap();
		assert_eq!(job.artifacts, ["a"]);
		assert!(claim(&db, id).unwrap().is_none());
		remove(&db.conn(), "perf").unwrap();
		let status: String = db
			.conn()
			.query_row("SELECT status FROM build_jobs", [], |r| r.get(0))
			.unwrap();
		assert_eq!(status, "failed");
	}
}
