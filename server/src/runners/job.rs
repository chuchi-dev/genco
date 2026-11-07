//! The deploy's side of a build job: pack the step's folder, wait in the
//! queue until a runner takes it, judge its state each second, copy its
//! log, unpack the artifacts.

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
	/// minutes it may wait for a runner, else the setting
	pub wait: Option<u32>,
	/// a detected step (no genco.toml): it builds here when this server
	/// has no runner at all
	pub fallback: bool,
	/// its place in the queue changed, for the deploy's view
	pub moved: &'a (dyn Fn() + Sync),
}

pub enum Outcome {
	/// the artifacts are in the release
	Built,
	/// a detected step without a runner, build it here
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
	let (registered, settings) = {
		let conn = req.db.conn();
		(
			any_registered(&conn).map_err(|e| e.to_string())?,
			settings(&conn),
		)
	};
	if !registered && req.fallback {
		return Ok(Outcome::Here("this server has no runner".into()));
	}
	if !registered {
		return Err("the step has on = \"runner\", but this server has no \
			runner (Settings → Runners)"
			.into());
	}
	let limit = queue_limit(req.wait, settings.queue_minutes);
	let limit = Duration::from_secs(u64::from(limit) * 60);
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
	let wait = Wait {
		limit,
		moved: req.moved,
	};
	let res = run_job(req.db, id, &dir, &image, wait, log, pack, unpack).await;
	if let Err(e) = &res {
		let _ = fail(req.db, id, e);
	}
	let _ = std::fs::remove_dir_all(&dir);
	res
}

/// How the job waits in the queue.
pub(super) struct Wait<'a> {
	pub(super) limit: Duration,
	pub(super) moved: &'a (dyn Fn() + Sync),
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_job(
	db: &Db,
	id: i64,
	dir: &Path,
	image: &str,
	wait: Wait<'_>,
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
	log.line(&format!("queued for a runner (image {image})"))
		.await;

	let queued = Instant::now();
	let mut offline_since: Option<Instant> = None;
	// what the log said about the queue last, and when
	let mut said = (String::new(), Instant::now());
	let mut position = None;
	let mut logged = 0u64;
	loop {
		tokio::time::sleep(Duration::from_secs(1)).await;
		logged = copy_log(dir, logged, log).await;
		let job = JobRow::read(db, id)?;

		if job.status == "queued" {
			let q = Queue::read(db, id)?;
			let at = now();
			offline_since = match q.any_online(at) {
				true => None,
				false => offline_since.or(Some(Instant::now())),
			};
			let offline = offline_since.map(|s| s.elapsed());
			if let Some(why) =
				give_up(&q, at, queued.elapsed(), wait.limit, offline)
			{
				// unless a runner took it in this very moment
				if drop_queued(db, id, &why)? {
					return Err(why);
				}
				continue;
			}
			let (key, text) = q.describe(at);
			if key != said.0 || said.1.elapsed() > REMIND {
				log.line(&text).await;
				said = (key, Instant::now());
			}
			if position != Some(q.position) {
				position = Some(q.position);
				(wait.moved)();
			}
			continue;
		}
		if position.take().is_some() {
			(wait.moved)();
		}

		let runner = job.runner.clone().unwrap_or_default();
		match verdict(&job, now()) {
			Verdict::Wait => {}
			Verdict::Cancelled => {
				let by = job.message.unwrap_or_else(|| "cancelled".into());
				return Err(format!("{by} while it waited for a runner"));
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

/// The queue as a waiting job sees it.
#[derive(Debug)]
pub(super) struct Queue {
	/// 1 for the next a runner takes
	pub(super) position: i64,
	pub(super) runners: Vec<QueueRunner>,
}

#[derive(Debug)]
pub(super) struct QueueRunner {
	pub(super) name: String,
	pub(super) last_seen: Option<i64>,
	/// whether it builds something right now; never for which group, the
	/// log goes to the waiting group's deploy token
	pub(super) busy: bool,
}

impl Queue {
	pub(super) fn read(db: &Db, id: i64) -> std::result::Result<Self, String> {
		let conn = db.conn();
		let read = || -> rusqlite::Result<Self> {
			let position = position(&conn, id)?;
			let mut stmt = conn.prepare(
				"SELECT r.name, r.last_seen,
				EXISTS (SELECT 1 FROM build_jobs j
				WHERE j.runner_id = r.id AND j.status = 'running')
				FROM runners r ORDER BY r.name",
			)?;
			let runners = stmt
				.query_map([], |r| {
					Ok(QueueRunner {
						name: r.get(0)?,
						last_seen: r.get(1)?,
						busy: r.get(2)?,
					})
				})?
				.collect::<rusqlite::Result<_>>()?;
			Ok(Self { position, runners })
		};
		read().map_err(|e| e.to_string())
	}

	pub(super) fn any_online(&self, now: i64) -> bool {
		self.runners.iter().any(|r| r.online(now))
	}

	/// What the runners do, for the log and the errors.
	pub(super) fn runners_text(&self, now: i64) -> String {
		if self.runners.is_empty() {
			return "no runner registered".into();
		}
		let list: Vec<String> =
			self.runners.iter().map(|r| r.text(now, true)).collect();
		list.join(", ")
	}

	/// The log's line, and what has to change for a new one (no times).
	pub(super) fn describe(&self, now: i64) -> (String, String) {
		let line = |times: bool| {
			let list: Vec<String> =
				self.runners.iter().map(|r| r.text(now, times)).collect();
			format!(
				"waiting for a runner (position {} in the queue, {})",
				self.position,
				list.join(", ")
			)
		};
		(line(false), line(true))
	}
}

impl QueueRunner {
	fn online(&self, now: i64) -> bool {
		self.last_seen.is_some_and(|s| now - s < ONLINE_MS)
	}

	fn text(&self, now: i64, times: bool) -> String {
		let name = &self.name;
		match (self.busy, self.last_seen) {
			(true, _) => format!("{name} busy with another build"),
			_ if self.online(now) => format!("{name} online"),
			(false, Some(seen)) if times => {
				format!("{name} offline for {}", span(now - seen))
			}
			(false, Some(_)) => format!("{name} offline"),
			(false, None) => format!("{name} never connected"),
		}
	}
}

/// The place of a queued job, 1 for the next one; 0 when it isn't queued.
pub(super) fn position(conn: &Connection, id: i64) -> rusqlite::Result<i64> {
	conn.query_row(
		"SELECT count(*) FROM build_jobs WHERE status = 'queued' AND id <= ?1
		AND EXISTS (SELECT 1 FROM build_jobs WHERE id = ?1
		AND status = 'queued')",
		[id],
		|r| r.get(0),
	)
}

/// The minutes a job waits in the queue: a step's `wait` only shortens
/// the admin's setting, a repo can't hold the queue longer than that.
fn queue_limit(wait: Option<u32>, setting: u32) -> u32 {
	wait.map_or(setting, |w| w.min(setting))
}

/// Milliseconds like `2 h`, `1 h 5 min` or `40 min`.
pub(super) fn span(ms: i64) -> String {
	let minutes = (ms / 60_000).max(1);
	match (minutes / 60, minutes % 60) {
		(0, m) => format!("{m} min"),
		(h, 0) => format!("{h} h"),
		(h, m) => format!("{h} h {m} min"),
	}
}

/// Why a queued job fails, if it waited long enough: no runner online for
/// `OFFLINE` (a box that is off won't come back by itself), or none took
/// it within `limit`.
pub(super) fn give_up(
	q: &Queue,
	now: i64,
	waited: Duration,
	limit: Duration,
	offline: Option<Duration>,
) -> Option<String> {
	if offline.is_some_and(|o| o > OFFLINE) {
		return Some(format!(
			"no runner online for {} ({}): start one (systemctl start \
			genco-runner on the build box) and deploy again",
			span(OFFLINE.as_millis() as i64),
			q.runners_text(now)
		));
	}
	if waited > limit {
		return Some(format!(
			"no runner took the build in {} ({})",
			span(limit.as_millis() as i64),
			q.runners_text(now)
		));
	}
	None
}

/// Fails a job that is still queued, false if a runner took it.
pub(super) fn drop_queued(
	db: &Db,
	id: i64,
	why: &str,
) -> std::result::Result<bool, String> {
	let n = db
		.conn()
		.execute(
			"UPDATE build_jobs SET status = 'failed', message = ?2,
			finished_at = ?3 WHERE id = ?1 AND status = 'queued'",
			params![id, why, now()],
		)
		.map_err(|e| e.to_string())?;
	Ok(n == 1)
}

/// Takes the deploy's job out of the queue; an error if it doesn't wait
/// for a runner (anymore). Its deploy fails at its next look. The deploy
/// log, which the group's deploy token reads, names no one; the activity
/// log keeps who.
pub fn cancel(conn: &Connection, deploy_id: i64) -> Result<()> {
	let n = conn.execute(
		"UPDATE build_jobs SET status = 'cancelled', message = ?2,
		finished_at = ?3 WHERE deploy_id = ?1 AND status = 'queued'",
		params![deploy_id, "cancelled by an admin", now()],
	)?;
	if n == 0 {
		return Err(Error::Invalid(
			"the deploy doesn't wait for a runner".into(),
		));
	}
	Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
	/// nothing to do yet
	Wait,
	/// an admin took it out of the queue
	Cancelled,
	/// taken but never heard of: the answer to the runner's poll got lost
	Requeue,
	/// the runner went quiet or the step ran too long
	Fail(&'static str),
	Done,
	/// the runner says it failed
	Failed,
}

/// What the waiting deploy does with its job once it left the queue.
pub(super) fn verdict(job: &JobRow, now: i64) -> Verdict {
	match job.status.as_str() {
		"queued" => Verdict::Wait,
		"cancelled" => Verdict::Cancelled,
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
		// queued, it waits as long as it takes: no fallback after minutes
		assert_eq!(verdict(&job("queued", None, None), 0), Verdict::Wait);
		assert_eq!(
			verdict(&job("cancelled", None, None), 0),
			Verdict::Cancelled
		);
		// taken a while ago, never heard of
		let taken = job("running", Some(1000), Some(1000));
		assert_eq!(verdict(&taken, 1000 + UNHEARD_MS - 1), Verdict::Wait);
		assert_eq!(verdict(&taken, 1000 + UNHEARD_MS + 1), Verdict::Requeue);
		// it spoke, then went quiet
		let quiet = job("running", Some(5000), Some(1000));
		assert_eq!(verdict(&quiet, 5000 + SILENT_MS - 1), Verdict::Wait);
		assert!(matches!(
			verdict(&quiet, 5000 + SILENT_MS + 1),
			Verdict::Fail(_)
		));
		// speaks all along, but past its minutes and five more
		let at = 1000 + 15 * 60 * 1000 + 1;
		let long = job("running", Some(at), Some(1000));
		assert!(matches!(verdict(&long, at), Verdict::Fail(_)));
		assert_eq!(verdict(&job("done", None, None), 0), Verdict::Done);
		assert_eq!(verdict(&job("failed", None, None), 0), Verdict::Failed);
	}

	/// A deploy of group `group` with a build job in `status`, on `runner`.
	fn add_job(db: &Db, group: &str, status: &str, runner: Option<i64>) -> i64 {
		let conn = db.conn();
		conn.execute(
			"INSERT OR IGNORE INTO clients (name, created_at) VALUES ('c', 0)",
			[],
		)
		.unwrap();
		conn.execute(
			"INSERT OR IGNORE INTO domain_groups (name, client_id, created_at)
			VALUES (?1, 1, 0)",
			[group],
		)
		.unwrap();
		conn.execute(
			"INSERT INTO deploys (group_id, ref, status, started_by,
			created_at) VALUES ((SELECT id FROM domain_groups
			WHERE name = ?1), 'main', 'running', 'x', 0)",
			[group],
		)
		.unwrap();
		conn.execute(
			"INSERT INTO build_jobs (deploy_id, cache_key, image, run,
			artifacts, minutes, status, runner_id, created_at)
			VALUES (?1, 'g/x', 'img', 'make', '[]', 5, ?2, ?3, 0)",
			params![conn.last_insert_rowid(), status, runner],
		)
		.unwrap();
		conn.last_insert_rowid()
	}

	#[test]
	fn queued_jobs_know_their_place() {
		let db = Db::memory();
		create(&db.conn(), "perf").unwrap();
		db.conn()
			.execute("UPDATE runners SET last_seen = 1000", [])
			.unwrap();
		let busy = add_job(&db, "shop", "running", Some(1));
		let second = add_job(&db, "blog", "queued", None);
		let third = add_job(&db, "cms", "queued", None);

		let q = Queue::read(&db, third).unwrap();
		assert_eq!(q.position, 2);
		assert_eq!(
			q.describe(2000).1,
			"waiting for a runner (position 2 in the queue, perf busy with \
			another build)"
		);
		// no other group's name in the log, also not when giving up
		let why = give_up(&q, 2000, Duration::MAX, Duration::ZERO, None);
		assert!(!why.unwrap().contains("shop"));
		assert_eq!(position(&db.conn(), busy).unwrap(), 0);
		// the deploy's view shows it (deploy ids are the job ids here)
		let deploy = |id| crate::deploy::get(&db.conn(), id).unwrap().queue;
		assert_eq!(deploy(third), Some(2));
		assert_eq!(deploy(busy), None);

		// the runner takes the oldest, the third moves up
		let next = api::claim(&db, 1).unwrap().unwrap();
		assert_eq!(next.id, second);
		assert_eq!(Queue::read(&db, third).unwrap().position, 1);
	}

	#[test]
	fn a_queued_job_gives_up_late_and_says_why() {
		let hour = 60 * 60 * 1000;
		let q = Queue {
			position: 1,
			runners: vec![QueueRunner {
				name: "performance".into(),
				last_seen: Some(0),
				busy: false,
			}],
		};
		let limit = Duration::from_secs(2 * 60 * 60);
		let minutes = |m: u64| Duration::from_secs(m * 60);
		// an hour, and the runner was offline only a while
		assert_eq!(
			give_up(&q, hour, minutes(60), limit, Some(minutes(5))),
			None
		);
		assert_eq!(
			give_up(&q, 2 * hour, minutes(121), limit, None).unwrap(),
			"no runner took the build in 2 h (performance offline for 2 h)"
		);
		// all offline for a quarter of an hour: no need to wait two
		let why = give_up(&q, hour, minutes(16), limit, Some(minutes(16)));
		assert!(why.unwrap().starts_with(
			"no runner online for 15 min (performance offline for 1 h)"
		));
		let none = Queue {
			position: 1,
			runners: vec![],
		};
		let why = give_up(&none, 0, minutes(16), limit, Some(minutes(16)));
		assert!(why.unwrap().contains("(no runner registered)"));
	}

	#[test]
	fn a_steps_wait_only_shortens_the_setting() {
		assert_eq!(queue_limit(None, 120), 120);
		assert_eq!(queue_limit(Some(30), 120), 30);
		assert_eq!(queue_limit(Some(600), 120), 120);
	}

	#[test]
	fn a_cancel_takes_the_job_out_of_the_queue() {
		let db = Db::memory();
		let id = add_job(&db, "shop", "queued", None);
		let deploy: i64 = db
			.conn()
			.query_row("SELECT deploy_id FROM build_jobs", [], |r| r.get(0))
			.unwrap();
		cancel(&db.conn(), deploy).unwrap();
		let job = JobRow::read(&db, id).unwrap();
		assert_eq!(verdict(&job, 0), Verdict::Cancelled);
		assert_eq!(job.message.as_deref(), Some("cancelled by an admin"));
		// no runner gets it, and twice is an error
		create(&db.conn(), "perf").unwrap();
		assert!(api::claim(&db, 1).unwrap().is_none());
		assert!(cancel(&db.conn(), deploy).is_err());
		// a job a runner took already isn't cancelled
		add_job(&db, "blog", "running", Some(1));
		assert!(cancel(&db.conn(), deploy + 1).is_err());
	}

	#[tokio::test]
	async fn without_runners_only_a_detected_step_builds_here() {
		let db = Db::memory();
		let dir = std::env::temp_dir().join("genco-test-no-runner");
		let mut log =
			Log::create(dir.to_str().unwrap(), 1, false).await.unwrap();
		let moved = || {};
		let req = |fallback| Request {
			db: &db,
			deploy_id: 1,
			group: "shop",
			dir: Path::new("/nonexistent"),
			rel_dir: ".",
			image: None,
			run: "cargo build",
			artifacts: &[],
			minutes: 5,
			wait: None,
			fallback,
			moved: &moved,
		};
		let cmd = || tokio::process::Command::new("false");
		let res = build(req(false), &mut log, cmd(), |_| cmd()).await;
		assert!(res.err().unwrap().contains("this server has no runner"));
		let res = build(req(true), &mut log, cmd(), |_| cmd()).await;
		assert!(matches!(res, Ok(Outcome::Here(_))));
		let _ = std::fs::remove_dir_all(dir);
	}
}
