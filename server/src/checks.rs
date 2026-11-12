//! Checks without the llm every minute: failed units, sites not answering,
//! disk, load, memory, certificates, backups and deploys. A new problem
//! starts an alert conversation, the agent looks into it.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension as _, params};
use tokio::process::Command;

use crate::agent::Runner;
use crate::conversations::{self as convs, Kind, MessageData};
use crate::db::{Db, now};
use crate::hosting::{self, Group};
use crate::stats::StatsDb;
use crate::util::sh;

const ALERT: &str = include_str!("agent/alert.md");

const HOUR: i64 = 60 * 60 * 1000;
/// a problem that comes back alerts again only after this
const REALERT: i64 = 4 * HOUR;
/// cleared problems are forgotten after this
const FORGET: i64 = 7 * 24 * HOUR;
/// an alert is solved once its problems are cleared this long: one that
/// comes back before `REALERT` doesn't alert again
const SOLVED: i64 = 15 * 60 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
	/// the same problem has the same key every minute
	pub key: String,
	pub detail: String,
	/// minutes in a row it has to be seen before it counts
	pub minutes: u32,
}

impl Problem {
	pub fn new(key: impl Into<String>, detail: impl Into<String>) -> Self {
		Self {
			key: key.into(),
			detail: detail.into(),
			minutes: 2,
		}
	}

	pub fn minutes(mut self, minutes: u32) -> Self {
		self.minutes = minutes;
		self
	}
}

/// How many minutes in a row each problem was seen, and the state of the
/// signs of a compromised group.
#[derive(Default)]
pub struct Checker {
	seen: HashMap<String, u32>,
	signals: crate::signals::Signals,
	/// the OOM kills of each group's slice in the last round
	oom_kills: HashMap<String, u64>,
}

impl Checker {
	/// One round: runs the checks, records the problems and starts an
	/// alert for the new ones.
	pub async fn round(&mut self, db: &Db, stats: &StatsDb, runner: &Runner) {
		let groups = hosting::groups(&db.conn()).unwrap_or_default();
		let mut found = find(db, &groups).await;
		found.extend(self.signals.find(&groups, stats).await);
		found.extend(oom_kills(&groups, &mut self.oom_kills));
		let keys: HashSet<&str> =
			found.iter().map(|p| p.key.as_str()).collect();
		self.seen.retain(|k, _| keys.contains(k.as_str()));

		let mut confirmed = vec![];
		for p in &found {
			let n = self.seen.entry(p.key.clone()).or_default();
			*n += 1;
			if *n >= p.minutes {
				confirmed.push(p.clone());
			}
		}

		let new = match record(&db.conn(), &confirmed, &keys, now()) {
			Ok(new) => new,
			Err(e) => {
				tracing::error!("recording problems: {e}");
				return;
			}
		};
		if !new.is_empty() {
			alert(db, runner, &new);
		}

		let older = superseded_routines(&db.conn()).unwrap_or_else(|e| {
			tracing::error!("finding superseded routines: {e}");
			vec![]
		});
		for (id, newer) in older {
			runner.resolve(
				id,
				&format!(
					"a newer run of this routine (conversation {newer}) has \
					its own report."
				),
			);
		}

		let gone = solved_alerts(&db.conn(), now()).unwrap_or_else(|e| {
			tracing::error!("finding solved alerts: {e}");
			vec![]
		});
		for id in gone {
			runner.resolve(
				id,
				"the minute check doesn't find the problems of this alert \
				any more.",
			);
		}
	}
}

/// Runs of a routine that need attention while a newer run of it has
/// its own report: that one says how things are now. With the newest
/// such run.
fn superseded_routines(conn: &Connection) -> rusqlite::Result<Vec<(i64, i64)>> {
	conn.prepare(
		"SELECT c.id, (SELECT max(n.id) FROM conversations n
			WHERE n.kind = 'routine' AND n.title = c.title AND n.id > c.id
			AND n.report IS NOT NULL)
		FROM conversations c
		WHERE c.kind = 'routine' AND c.report = 'attention'
		AND c.status != 'running' AND EXISTS (SELECT 1 FROM conversations n
			WHERE n.kind = 'routine' AND n.title = c.title AND n.id > c.id
			AND n.report IS NOT NULL)
		ORDER BY c.id",
	)?
	.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
	.collect()
}

/// Alerts that need attention but whose problems are gone since `SOLVED`,
/// not while the agent still works on them. Not signs of a compromised group: they
/// are gone once it is in quarantine or between the upload searches, an
/// admin decides when it is clean.
fn solved_alerts(conn: &Connection, now: i64) -> rusqlite::Result<Vec<i64>> {
	conn.prepare(
		"SELECT c.id FROM conversations c
		WHERE c.kind = 'alert' AND c.report = 'attention'
		AND c.status != 'running'
		AND EXISTS (SELECT 1 FROM alert_problems a
			WHERE a.conversation_id = c.id)
		AND NOT EXISTS (SELECT 1 FROM alert_problems a
			WHERE a.conversation_id = c.id AND a.key LIKE 'signal:%')
		AND NOT EXISTS (SELECT 1 FROM alert_problems a
			JOIN problems p ON p.key = a.key
			WHERE a.conversation_id = c.id
			AND (p.cleared_at IS NULL OR p.cleared_at > ?1))
		ORDER BY c.id",
	)?
	.query_map([now - SOLVED], |r| r.get(0))?
	.collect()
}

/// Stores the problems. Returns the ones to alert: new ones, and ones
/// that came back after `REALERT`. Problems not `seen` any more are
/// cleared.
fn record(
	conn: &Connection,
	confirmed: &[Problem],
	seen: &HashSet<&str>,
	now: i64,
) -> rusqlite::Result<Vec<Problem>> {
	let mut new = vec![];
	for p in confirmed {
		let row: Option<(Option<i64>, Option<i64>)> = conn
			.query_row(
				"SELECT alerted_at, cleared_at FROM problems WHERE key = ?1",
				[&p.key],
				|r| Ok((r.get(0)?, r.get(1)?)),
			)
			.optional()?;

		match row {
			None => {
				conn.execute(
					"INSERT INTO problems (key, detail, since, alerted_at)
					VALUES (?1, ?2, ?3, ?3)",
					params![p.key, p.detail, now],
				)?;
				new.push(p.clone());
			}
			Some((_, None)) => {}
			Some((alerted, Some(_))) => {
				let again = alerted.is_none_or(|a| a < now - REALERT);
				conn.execute(
					"UPDATE problems SET detail = ?1, since = ?2,
					cleared_at = NULL,
					alerted_at = CASE WHEN ?3 THEN ?2 ELSE alerted_at END
					WHERE key = ?4",
					params![p.detail, now, again, p.key],
				)?;
				if again {
					new.push(p.clone());
				}
			}
		}
	}

	let active: Vec<String> = conn
		.prepare("SELECT key FROM problems WHERE cleared_at IS NULL")?
		.query_map([], |r| r.get(0))?
		.collect::<rusqlite::Result<_>>()?;
	for key in active.iter().filter(|k| !seen.contains(k.as_str())) {
		conn.execute(
			"UPDATE problems SET cleared_at = ?1 WHERE key = ?2",
			params![now, key],
		)?;
	}
	conn.execute("DELETE FROM problems WHERE cleared_at < ?1", [now - FORGET])?;

	Ok(new)
}

fn alert(db: &Db, runner: &Runner, problems: &[Problem]) {
	let title = match problems {
		[p] => format!("Alert: {}", p.detail),
		[p, rest @ ..] => {
			format!("Alert: {} and {} more", p.detail, rest.len())
		}
		[] => return,
	};
	let list: String = problems
		.iter()
		.map(|p| format!("- {}\n", p.detail))
		.collect();
	let text = ALERT.replace("{{problems}}", &list);

	let res = (|| {
		let conn = db.conn();
		let id = convs::create_kind(&conn, &title, Kind::Alert)?;
		convs::insert_message(&conn, id, MessageData::System { text })?;
		for p in problems {
			conn.execute(
				"INSERT OR IGNORE INTO alert_problems (conversation_id, key)
				VALUES (?1, ?2)",
				params![id, p.key],
			)?;
		}
		Ok::<_, rusqlite::Error>(id)
	})();
	match res {
		Ok(id) => {
			crate::activity::log(&db.conn(), "genco", "Alert", "", &list, None);
			runner.start_report(id);
		}
		Err(e) => tracing::error!("starting an alert: {e}"),
	}
}

/// Runs every check, the problems found right now.
async fn find(db: &Db, groups: &[Group]) -> Vec<Problem> {
	let mut found = failed_units().await;
	found.extend(sites(groups).await);
	found.extend(disks().await);
	found.extend(load());
	found.extend(memory());
	found.extend(limits(groups));
	found.extend(memory_pressure());
	found.extend(certificates(groups).await);
	found.extend(jobs(&db.conn()).unwrap_or_default());
	found
}

async fn failed_units() -> Vec<Problem> {
	let out = sh("systemctl list-units --failed --plain --no-legend \
		| awk '{print $1}'")
	.await
	.unwrap_or_default();
	out.lines()
		.map(|u| Problem::new(format!("unit:{u}"), format!("{u} failed")))
		.collect()
}

/// A site counts as down when nginx can't be reached or it answers with
/// an error 500 or above for three minutes.
async fn sites(groups: &[Group]) -> Vec<Problem> {
	// an app before its first deploy answers 502, nobody waits for it yet;
	// a group in quarantine answers 503 on purpose
	let domains =
		groups
			.iter()
			.filter(|g| g.quarantine.is_none())
			.flat_map(|g| {
				g.domains.iter().filter(|d| {
					!crate::render::has_app(d)
						|| crate::render::app_ready(&g.name, d)
				})
			});
	let checks = domains.map(|d| async move {
		let (scheme, port) = if crate::host::has_cert(&d.name) {
			("https", 443)
		} else {
			("http", 80)
		};
		let out = Command::new("curl")
			.args(["-sk", "-o", "/dev/null", "-w", "%{http_code}"])
			.args(["--max-time", "15", "--resolve"])
			.arg(format!("{}:{port}:127.0.0.1", d.name))
			.arg(format!("{scheme}://{}/", d.name))
			.output()
			.await
			.ok()?;
		let code: u16 = String::from_utf8_lossy(&out.stdout).parse().ok()?;
		let detail = match code {
			0 => format!("{} doesn't answer", d.name),
			500.. => format!("{} answers {code}", d.name),
			_ => return None,
		};
		Some(Problem::new(format!("site:{}", d.name), detail).minutes(3))
	});
	futures::future::join_all(checks)
		.await
		.into_iter()
		.flatten()
		.collect()
}

async fn disks() -> Vec<Problem> {
	let out = sh("df -P -x tmpfs -x devtmpfs -x overlay -x squashfs \
		-x efivarfs | tail -n +2")
	.await
	.unwrap_or_default();
	out.lines()
		.filter_map(|l| {
			let cols: Vec<&str> = l.split_whitespace().collect();
			let used: u32 = cols.get(4)?.trim_end_matches('%').parse().ok()?;
			let mount = cols.get(5)?;
			(used >= 90).then(|| {
				Problem::new(
					format!("disk:{mount}"),
					format!("disk {mount} is {used}% full"),
				)
			})
		})
		.collect()
}

/// The load over 15 minutes above one and a half times the cores.
fn load() -> Option<Problem> {
	let avg = std::fs::read_to_string("/proc/loadavg").ok()?;
	let load: f64 = avg.split_whitespace().nth(2)?.parse().ok()?;
	let cores = std::thread::available_parallelism().ok()?.get() as f64;
	(load > cores * 1.5).then(|| {
		Problem::new(
			"load",
			format!("load {load:.1} over 15 minutes on {cores} cores"),
		)
	})
}

/// Less than a tenth of the memory available for five minutes.
fn memory() -> Option<Problem> {
	let info = std::fs::read_to_string("/proc/meminfo").ok()?;
	let field = |name: &str| -> Option<f64> {
		info.lines()
			.find(|l| l.starts_with(name))?
			.split_whitespace()
			.nth(1)?
			.parse()
			.ok()
	};
	let available = field("MemAvailable:")? / field("MemTotal:")?;
	(available < 0.1).then(|| {
		Problem::new(
			"memory",
			format!("only {:.0}% of the memory available", available * 100.0),
		)
		.minutes(5)
	})
}

/// Drop-ins that override genco's units. How the groups' memory limits
/// add up doesn't matter, they share the cap of genco.slice; what counts
/// is real pressure and processes killed (`memory_pressure`, `oom_kills`).
fn limits(groups: &[Group]) -> Vec<Problem> {
	let mut found = vec![];
	let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
	for d in crate::limits::foreign_here(&names) {
		found.push(Problem::new(
			format!("dropin:{}", d.path.display()),
			d.text(),
		));
	}
	found
}

/// Of the last minute, the share in percent all tasks waited for memory
/// before it counts.
const PRESSURE_FULL: f64 = 10.0;

/// The server stalls on memory for 5 minutes: everything waits while the
/// kernel reclaims or swaps.
fn memory_pressure() -> Option<Problem> {
	let psi = std::fs::read_to_string("/proc/pressure/memory").ok()?;
	let full = pressure_full(&psi)?;
	(full >= PRESSURE_FULL).then(|| {
		Problem::new(
			"memory:pressure",
			format!(
				"the server stalls on memory: in {full:.0}% of the last \
				minute every task waited for it"
			),
		)
		.minutes(5)
	})
}

/// `full avg60=` of /proc/pressure/memory.
fn pressure_full(psi: &str) -> Option<f64> {
	psi.lines()
		.find_map(|l| l.strip_prefix("full "))?
		.split_whitespace()
		.find_map(|f| f.strip_prefix("avg60="))?
		.parse()
		.ok()
}

/// Processes the kernel killed in a group because it ran out of memory,
/// its own limit or that of all groups, since the last round: the
/// slice's count only grows (`last` holds it), a round without new kills
/// clears the problem.
fn oom_kills(
	groups: &[Group],
	last: &mut HashMap<String, u64>,
) -> Vec<Problem> {
	let mut found = vec![];
	for g in groups {
		let path = std::path::Path::new(crate::stats::CGROUPS)
			.join(crate::render::slice(&g.name))
			.join("memory.events");
		let Ok(events) = std::fs::read_to_string(path) else {
			continue;
		};
		let kills = oom_kill_count(&events);
		let before = last.insert(g.name.clone(), kills);
		let new = kills.saturating_sub(before.unwrap_or(kills));
		if new > 0 {
			found.push(
				Problem::new(
					format!("oom:{}", g.name),
					format!(
						"the kernel killed {new} processes of the group {} \
						for lack of memory (its limit {} MB, or all groups' \
						cap)",
						g.name,
						g.limits.memory_max()
					),
				)
				.minutes(1),
			);
		}
	}
	found
}

fn oom_kill_count(events: &str) -> u64 {
	events
		.lines()
		.find_map(|l| l.strip_prefix("oom_kill "))
		.and_then(|n| n.trim().parse().ok())
		.unwrap_or(0)
}

/// Certbot renews 30 days ahead, 14 days left means renewing fails.
async fn certificates(groups: &[Group]) -> Vec<Problem> {
	let mut found = vec![];
	for d in groups.iter().flat_map(|g| &g.domains) {
		if !crate::host::has_cert(&d.name) {
			continue;
		}
		let path = format!("/etc/letsencrypt/live/{}/fullchain.pem", d.name);
		let valid = Command::new("openssl")
			.args(["x509", "-noout", "-checkend", "1209600", "-in", &path])
			.output()
			.await
			.is_ok_and(|o| o.status.success());
		if !valid {
			found.push(Problem::new(
				format!("cert:{}", d.name),
				format!(
					"the certificate of {} expires in under 14 days",
					d.name
				),
			));
		}
	}
	found
}

/// The last backup failed or is more than a day old, the same for the
/// copy off the server (two days), deploys that failed
/// in the last day without a good one after them.
fn jobs(conn: &Connection) -> rusqlite::Result<Vec<Problem>> {
	let mut found = vec![];

	let last: Option<(String, i64)> = conn
		.query_row(
			"SELECT status, created_at FROM backups
			WHERE status != 'running' ORDER BY id DESC LIMIT 1",
			[],
			|r| Ok((r.get(0)?, r.get(1)?)),
		)
		.optional()?;
	let last_done: Option<i64> = conn.query_row(
		"SELECT max(created_at) FROM backups
		WHERE status = 'done' AND group_id IS NULL",
		[],
		|r| r.get(0),
	)?;
	// a day after the install, also when none ever finished
	let installed: Option<i64> =
		conn.query_row("SELECT min(created_at) FROM users", [], |r| r.get(0))?;
	let since = last_done.or(installed);
	if last.as_ref().is_some_and(|(status, _)| status == "failed") {
		found.push(
			Problem::new("backup:failed", "the last backup failed").minutes(1),
		);
	} else if since.is_some_and(|t| t < now() - 26 * HOUR) {
		found.push(
			Problem::new("backup:old", "no full backup for more than a day")
				.minutes(1),
		);
	}
	if let Some((key, detail)) = crate::backup::remote::problem(conn, now()) {
		found.push(Problem::new(key, detail).minutes(1));
	}

	let mut stmt = conn.prepare(
		"SELECT d.id, g.name, d.ref FROM deploys d
		JOIN domain_groups g ON g.id = d.group_id
		WHERE d.status = 'failed' AND d.finished_at > ?1
		AND NOT EXISTS (SELECT 1 FROM deploys n WHERE n.group_id = d.group_id
			AND n.id > d.id AND n.status = 'done')",
	)?;
	let failed = stmt.query_map([now() - 24 * HOUR], |r| {
		Ok(Problem::new(
			format!("deploy:{}", r.get::<_, i64>(0)?),
			format!(
				"the deploy of {} ({}) failed",
				r.get::<_, String>(1)?,
				r.get::<_, String>(2)?
			),
		)
		.minutes(1))
	})?;
	for p in failed {
		found.push(p?);
	}

	Ok(found)
}

/// Every minute, the first round a minute after the start.
pub fn spawn(
	cfg: std::sync::Arc<crate::config::Config>,
	db: Db,
	stats: StatsDb,
	runner: Runner,
	events: crate::events::Events,
) {
	tokio::spawn(async move {
		let mut checker = Checker::default();
		let mut tick = tokio::time::interval(Duration::from_secs(60));
		tick.tick().await;
		loop {
			tick.tick().await;
			checker.round(&db, &stats, &runner).await;
			crate::activity::prune(&db.conn());
			crate::job::prune(&db);
			crate::routines::due(&db, &runner).await;
			crate::update::due(&db).await;
			crate::proposals::due_mails(&cfg, &db).await;
			crate::proposals::due_expiries(&cfg, &db, &events);
			crate::actions::due(&db, &events);
			crate::firewall::due(&cfg, &db).await;
		}
	});
}

#[cfg(test)]
mod tests {
	use super::*;

	fn keys(problems: &[Problem]) -> Vec<&str> {
		problems.iter().map(|p| p.key.as_str()).collect()
	}

	#[test]
	fn a_newer_run_supersedes_the_routine_s_old_attention() {
		let db = Db::memory();
		let conn = db.conn();
		let run = |title: &str, report: Option<convs::Report>| {
			let id = convs::create_kind(&conn, title, Kind::Routine).unwrap();
			convs::set_status(&conn, id, convs::Status::Idle).unwrap();
			if let Some(r) = report {
				convs::set_report(&conn, id, r).unwrap();
			}
			id
		};
		let old = run("Daily health check", Some(convs::Report::Attention));
		run("Daily health check", Some(convs::Report::Ok));
		run("Weekly update round", Some(convs::Report::Attention));
		let last = run("Daily health check", Some(convs::Report::Attention));
		// a run without its report yet doesn't count
		run("Daily health check", None);
		assert_eq!(superseded_routines(&conn).unwrap(), [(old, last)]);
	}

	#[test]
	fn an_alert_is_solved_once_all_its_problems_are_gone() {
		let db = Db::memory();
		let conn = db.conn();
		let alert = |keys: &[&str]| {
			let id = convs::create_kind(&conn, "Alert", Kind::Alert).unwrap();
			convs::set_report(&conn, id, convs::Report::Attention).unwrap();
			convs::set_status(&conn, id, convs::Status::Idle).unwrap();
			for k in keys {
				conn.execute(
					"INSERT INTO alert_problems (conversation_id, key)
					VALUES (?1, ?2)",
					params![id, k],
				)
				.unwrap();
			}
			id
		};
		let both = alert(&["unit:a", "unit:b"]);
		let one = alert(&["unit:b"]);
		// an alert from before the problems were linked stays
		alert(&[]);
		// so does a sign of a compromised group
		alert(&["signal:cpu:x"]);

		let a = Problem::new("unit:a", "a failed");
		let b = Problem::new("unit:b", "b failed");
		fn seen<'a>(ps: &[&'a Problem]) -> HashSet<&'a str> {
			ps.iter().map(|p| p.key.as_str()).collect()
		}
		record(&conn, &[a.clone(), b.clone()], &seen(&[&a, &b]), 0).unwrap();
		assert!(solved_alerts(&conn, 0).unwrap().is_empty());

		// b is gone: only the alert about b alone is solved, once it
		// stays away
		record(&conn, &[a.clone()], &seen(&[&a]), 60_000).unwrap();
		assert!(solved_alerts(&conn, 60_000).unwrap().is_empty());
		let later = 60_000 + SOLVED + 1;
		assert_eq!(solved_alerts(&conn, later).unwrap(), [one]);

		record(&conn, &[], &seen(&[]), later).unwrap();
		let later = later + SOLVED + 1;
		assert_eq!(solved_alerts(&conn, later).unwrap(), [both, one]);

		// not while the agent works on it
		convs::set_status(&conn, both, convs::Status::Running).unwrap();
		assert_eq!(solved_alerts(&conn, later).unwrap(), [one]);
	}

	#[test]
	fn memory_pressure_and_kills_are_read() {
		let psi = "some avg10=1.00 avg60=20.50 avg300=3.00 total=1\n\
			full avg10=0.50 avg60=12.25 avg300=1.00 total=2\n";
		assert_eq!(pressure_full(psi), Some(12.25));
		assert_eq!(pressure_full(""), None);
		let events = "low 0\nhigh 12\nmax 3\noom 2\noom_kill 2\n\
			oom_group_kill 0\n";
		assert_eq!(oom_kill_count(events), 2);
		assert_eq!(oom_kill_count("low 0\n"), 0);
	}

	#[test]
	fn a_problem_alerts_once_and_again_after_a_while() {
		let db = Db::memory();
		let conn = db.conn();
		let p = Problem::new("unit:x", "x failed");
		let seen = HashSet::from(["unit:x"]);
		let none = HashSet::new();

		let new = record(&conn, &[p.clone()], &seen, 0).unwrap();
		assert_eq!(keys(&new), ["unit:x"]);
		assert!(record(&conn, &[p.clone()], &seen, 1).unwrap().is_empty());

		// gone and back within the hours: quiet
		record(&conn, &[], &none, 2).unwrap();
		assert!(record(&conn, &[p.clone()], &seen, 3).unwrap().is_empty());

		// gone and back later: alerts again
		record(&conn, &[], &none, 4).unwrap();
		let later = REALERT + 10;
		assert_eq!(
			keys(&record(&conn, &[p], &seen, later).unwrap()),
			["unit:x"]
		);
	}

	#[test]
	fn cleared_problems_are_forgotten() {
		let db = Db::memory();
		let conn = db.conn();
		let p = Problem::new("load", "load");
		record(&conn, &[p], &HashSet::from(["load"]), 0).unwrap();
		record(&conn, &[], &HashSet::new(), 1).unwrap();
		record(&conn, &[], &HashSet::new(), FORGET + 2).unwrap();
		let n: i64 = conn
			.query_row("SELECT count(*) FROM problems", [], |r| r.get(0))
			.unwrap();
		assert_eq!(n, 0);
	}
}
