//! The deploy's side of a build job: pack the step's folder, wait for a
//! runner to take it, judge its state each second, copy its log, unpack
//! the artifacts; or build here when no runner takes it.

use super::*;

/// A step of a deploy for a runner.
pub struct Request<'a> {
	pub db: &'a Db,
	pub deploy_id: i64,
	pub group: &'a str,
	/// the step's folder in the release
	pub dir: &'a Path,
	/// relative, for the cache
	pub rel_dir: &'a str,
	pub image: Option<&'a str>,
	pub run: &'a str,
	pub artifacts: &'a [String],
	pub minutes: u32,
}

pub enum Outcome {
	/// the artifacts are in the release
	Built,
	/// no runner took it, build it here
	Here(String),
}

/// Packs a command's stdout (the group's user packing its files) into a
/// root-only file.
pub(super) async fn pack_into(
	mut cmd: tokio::process::Command,
	file: &Path,
) -> std::result::Result<(), String> {
	let mut child = cmd
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.map_err(|e| e.to_string())?;
	let mut out = child.stdout.take().unwrap();
	let mut f = tokio::fs::File::create(file)
		.await
		.map_err(|e| e.to_string())?;
	tokio::io::copy(&mut out, &mut f)
		.await
		.map_err(|e| e.to_string())?;
	f.flush().await.map_err(|e| e.to_string())?;
	let status = child.wait().await.map_err(|e| e.to_string())?;
	if !status.success() {
		return Err(format!("packing failed ({status})"));
	}
	Ok(())
}

/// Builds a step on a runner: queues the job, copies its log into the
/// deploy's, unpacks the artifacts with `unpack` (the group's user, stdin
/// is the tarball). `pack` packs the step's folder.
pub async fn build(
	req: Request<'_>,
	log: &mut Log,
	pack: tokio::process::Command,
	unpack: impl FnOnce(std::fs::File) -> tokio::process::Command,
) -> std::result::Result<Outcome, String> {
	if !any_online(&req.db.conn()).map_err(|e| e.to_string())? {
		return Ok(Outcome::Here("no runner is online".into()));
	}
	let target = target().await;
	let image = image(req.image, req.dir, &target.codename);
	check_image(&image)?;
	let artifacts = serde_json::to_string(req.artifacts).unwrap();
	let id = {
		let conn = req.db.conn();
		conn.execute(
			"INSERT INTO build_jobs (deploy_id, cache_key, image, run,
			artifacts, minutes, status, created_at)
			VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'packing', ?7)",
			params![
				req.deploy_id,
				cache_key(req.group, req.rel_dir),
				image,
				req.run,
				artifacts,
				req.minutes,
				now()
			],
		)
		.map_err(|e| e.to_string())?;
		conn.last_insert_rowid()
	};
	let dir = job_dir(id);
	let res = run_job(req.db, id, &dir, &image, log, pack, unpack).await;
	if let Err(e) = &res {
		let _ = fail(req.db, id, e);
	}
	let _ = std::fs::remove_dir_all(&dir);
	res
}

pub(super) async fn run_job(
	db: &Db,
	id: i64,
	dir: &Path,
	image: &str,
	log: &mut Log,
	pack: tokio::process::Command,
	unpack: impl FnOnce(std::fs::File) -> tokio::process::Command,
) -> std::result::Result<Outcome, String> {
	std::fs::create_dir_all(dir)
		.and_then(|()| crate::util::set_mode(Path::new(DIR), 0o700))
		.map_err(|e| format!("{}: {e}", dir.display()))?;
	pack_into(pack, &dir.join("source.tar"))
		.await
		.map_err(|e| format!("packing the source for the runner: {e}"))?;
	set_status(db, id, "packing", "queued")?;
	log.line(&format!("waiting for a runner (image {image})"))
		.await;

	let queued = Instant::now();
	let mut logged = 0u64;
	loop {
		tokio::time::sleep(Duration::from_secs(1)).await;
		logged = copy_log(dir, logged, log).await;
		let job = JobRow::read(db, id)?;
		let runner = job.runner.clone().unwrap_or_default();
		match verdict(&job, now(), queued.elapsed()) {
			Verdict::Wait => {}
			// unless a runner took it in this very moment
			Verdict::GiveUp => {
				if set_status(db, id, "queued", "gone").is_ok() {
					return Ok(Outcome::Here(format!(
						"no runner took it within {} minutes",
						WAIT.as_secs() / 60
					)));
				}
			}
			Verdict::Requeue => {
				let _ = db.conn().execute(
					"UPDATE build_jobs SET status = 'queued', runner_id = NULL,
					started_at = NULL, heartbeat_at = NULL
					WHERE id = ?1 AND status = 'running'
					AND heartbeat_at = started_at",
					[id],
				);
			}
			Verdict::Fail(why) => {
				let _ = fail(db, id, why);
				return Err(format!("{why} ({runner})"));
			}
			Verdict::Done => {
				copy_log(dir, logged, log).await;
				return take_artifacts(dir, log, unpack, job.artifacts)
					.await
					.map(|()| Outcome::Built);
			}
			Verdict::Failed => {
				copy_log(dir, logged, log).await;
				return Err(format!(
					"the build on the runner {runner} failed: {}",
					job.message.unwrap_or_default()
				));
			}
		}
	}
}

/// A build job as the deploy waiting for it sees it each second.
pub(super) struct JobRow {
	pub(super) status: String,
	pub(super) message: Option<String>,
	pub(super) runner: Option<String>,
	pub(super) heartbeat: Option<i64>,
	pub(super) minutes: i64,
	pub(super) started: Option<i64>,
	/// it should bring artifacts back
	pub(super) artifacts: bool,
}

impl JobRow {
	pub(super) fn read(db: &Db, id: i64) -> std::result::Result<Self, String> {
		db.conn()
			.query_row(
				"SELECT j.status, j.message, r.name, j.heartbeat_at, j.minutes,
				j.started_at, j.artifacts != '[]' FROM build_jobs j
				LEFT JOIN runners r ON r.id = j.runner_id WHERE j.id = ?1",
				[id],
				|r| {
					Ok(Self {
						status: r.get(0)?,
						message: r.get(1)?,
						runner: r.get(2)?,
						heartbeat: r.get(3)?,
						minutes: r.get(4)?,
						started: r.get(5)?,
						artifacts: r.get(6)?,
					})
				},
			)
			.map_err(|e| e.to_string())
	}
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
	/// nothing to do yet
	Wait,
	/// no runner took it in time, it builds here
	GiveUp,
	/// taken but never heard of: the answer to the runner's poll got lost
	Requeue,
	/// the runner went quiet or the step ran too long
	Fail(&'static str),
	Done,
	/// the runner says it failed
	Failed,
}

/// What the waiting deploy does with its job, `waited` since it queued.
pub(super) fn verdict(job: &JobRow, now: i64, waited: Duration) -> Verdict {
	match job.status.as_str() {
		"queued" if waited > WAIT => Verdict::GiveUp,
		"queued" => Verdict::Wait,
		"running" => {
			// a runner says something right after it took a job
			let unheard =
				job.heartbeat.is_some() && job.heartbeat == job.started;
			if unheard && job.started.is_some_and(|s| now - s > UNHEARD_MS) {
				return Verdict::Requeue;
			}
			if job.heartbeat.is_some_and(|h| now - h > SILENT_MS) {
				return Verdict::Fail("the runner stopped answering");
			}
			let limit = (job.minutes + 5) * 60 * 1000;
			if job.started.is_some_and(|s| now - s > limit) {
				return Verdict::Fail("the step took too long");
			}
			Verdict::Wait
		}
		"done" => Verdict::Done,
		_ => Verdict::Failed,
	}
}

/// The runner's new log lines into the deploy's log.
pub(super) async fn copy_log(dir: &Path, from: u64, log: &mut Log) -> u64 {
	let Ok(mut f) = tokio::fs::File::open(dir.join("log")).await else {
		return from;
	};
	if f.seek(std::io::SeekFrom::Start(from)).await.is_err() {
		return from;
	}
	let mut buf = Vec::new();
	if f.read_to_end(&mut buf).await.is_err() {
		return from;
	}
	// only whole lines, the rest comes with the next chunk; a runner that
	// sends no line break at all gets its text through anyway
	let end = match buf.iter().rposition(|b| *b == b'\n') {
		Some(end) => end,
		None if buf.len() > 64 * 1024 => buf.len() - 1,
		None => return from,
	};
	for line in String::from_utf8_lossy(&buf[..=end]).lines() {
		// no terminal escapes from the runner into logs and terminals
		let line: String = line
			.chars()
			.filter(|c| *c == '\t' || !c.is_control())
			.collect();
		log.line(&line).await;
	}
	from + end as u64 + 1
}

pub(super) async fn take_artifacts(
	dir: &Path,
	log: &mut Log,
	unpack: impl FnOnce(std::fs::File) -> tokio::process::Command,
	wanted: bool,
) -> std::result::Result<(), String> {
	let path = dir.join("artifacts.tar");
	let Ok(file) = std::fs::File::open(&path) else {
		if wanted {
			return Err("the runner sent no artifacts".into());
		}
		return Ok(());
	};
	log.run(unpack(file))
		.await
		.map_err(|e| format!("unpacking the artifacts {e}"))
}

/// Moves a job from one status to the next, an error if it wasn't in
/// `from` anymore.
pub(super) fn set_status(
	db: &Db,
	id: i64,
	from: &str,
	to: &str,
) -> std::result::Result<(), String> {
	let n = db
		.conn()
		.execute(
			"UPDATE build_jobs SET status = ?3 WHERE id = ?1 AND status = ?2",
			params![id, from, to],
		)
		.map_err(|e| e.to_string())?;
	if n == 0 {
		return Err(format!("build job {id} isn't {from} anymore"));
	}
	Ok(())
}

pub(super) fn fail(db: &Db, id: i64, message: &str) -> rusqlite::Result<usize> {
	db.conn().execute(
		"UPDATE build_jobs SET status = 'failed', message = ?2,
		finished_at = ?3
		WHERE id = ?1 AND status IN ('packing', 'queued', 'running')",
		params![id, message, now()],
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn job(
		status: &str,
		heartbeat: Option<i64>,
		started: Option<i64>,
	) -> JobRow {
		JobRow {
			status: status.into(),
			message: None,
			runner: None,
			heartbeat,
			minutes: 10,
			started,
			artifacts: false,
		}
	}

	#[test]
	fn the_waiting_deploy_judges_its_job() {
		let soon = Duration::from_secs(10);
		let late = WAIT + Duration::from_secs(1);
		assert_eq!(verdict(&job("queued", None, None), 0, soon), Verdict::Wait);
		assert_eq!(
			verdict(&job("queued", None, None), 0, late),
			Verdict::GiveUp
		);
		// taken a while ago, never heard of
		let taken = job("running", Some(1000), Some(1000));
		assert_eq!(verdict(&taken, 1000 + UNHEARD_MS - 1, soon), Verdict::Wait);
		assert_eq!(
			verdict(&taken, 1000 + UNHEARD_MS + 1, soon),
			Verdict::Requeue
		);
		// it spoke, then went quiet
		let quiet = job("running", Some(5000), Some(1000));
		assert_eq!(verdict(&quiet, 5000 + SILENT_MS - 1, soon), Verdict::Wait);
		assert!(matches!(
			verdict(&quiet, 5000 + SILENT_MS + 1, soon),
			Verdict::Fail(_)
		));
		// speaks all along, but past its minutes and five more
		let at = 1000 + 15 * 60 * 1000 + 1;
		let long = job("running", Some(at), Some(1000));
		assert!(matches!(verdict(&long, at, soon), Verdict::Fail(_)));
		assert_eq!(verdict(&job("done", None, None), 0, soon), Verdict::Done);
		assert_eq!(
			verdict(&job("failed", None, None), 0, soon),
			Verdict::Failed
		);
	}
}
