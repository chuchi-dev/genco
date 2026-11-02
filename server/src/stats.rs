//! Usage stats: every minute a sample of the server (CPU, memory, disk,
//! network), each group (CPU and memory of its slice, disk of its files
//! and databases, the requests of its domains) and each domain (requests,
//! errors and traffic from its nginx log, CPU and memory of an app). They
//! go into their own database, minutes for two days, hours for a year.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

use crate::AppState;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::hosting::{self, Group};
use crate::render;
use crate::tokens::Reader;
use crate::util::sh;

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
/// how long minute samples and hourly ones stay
const KEEP_MINUTES: i64 = 48 * HOUR;
const KEEP_HOURS: i64 = 400 * 24 * HOUR;
/// disk usage of the groups takes a `du`, once an hour is enough
const DISK_EVERY: i64 = HOUR;
const CGROUPS: &str = "/sys/fs/cgroup/genco.slice";
const NGINX_LOGS: &str = "/var/log/nginx/genco";

/// The stats database, apart from genco's: it is big, changes every
/// minute and needs no backup.
#[derive(Clone)]
pub struct StatsDb(Arc<Mutex<Connection>>);

impl StatsDb {
	pub fn open(path: &Path) -> std::result::Result<Self, String> {
		let conn = Connection::open(path).map_err(|e| {
			format!("failed to open the stats {}: {e}", path.display())
		})?;
		conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
		Ok(Self(Arc::new(Mutex::new(conn))))
	}

	#[cfg(test)]
	fn memory() -> Self {
		let conn = Connection::open_in_memory().unwrap();
		conn.execute_batch(SCHEMA).unwrap();
		Self(Arc::new(Mutex::new(conn)))
	}

	fn conn(&self) -> MutexGuard<'_, Connection> {
		self.0.lock().unwrap()
	}
}

// `period` is 60 for minutes, 3600 for hours; counters (rx, tx, requests,
// errors) are totals of the period, the rest averages
const SCHEMA: &str = "
	PRAGMA journal_mode = WAL;
	CREATE TABLE IF NOT EXISTS samples (
		at INTEGER NOT NULL,
		period INTEGER NOT NULL,
		scope TEXT NOT NULL,
		cpu REAL,
		memory INTEGER,
		disk INTEGER,
		rx INTEGER,
		tx INTEGER,
		requests INTEGER,
		errors INTEGER
	);
	CREATE INDEX IF NOT EXISTS samples_scope
		ON samples(scope, period, at);
";

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
	pub at: i64,
	/// percent of all cores
	pub cpu: Option<f64>,
	/// bytes
	pub memory: Option<i64>,
	pub disk: Option<i64>,
	/// bytes received and sent in the period
	pub rx: Option<i64>,
	pub tx: Option<i64>,
	pub requests: Option<i64>,
	/// answers 500 and above
	pub errors: Option<i64>,
}

/// What the collector remembers between two minutes.
#[derive(Default)]
struct Collector {
	/// busy and total jiffies of all cores
	cpu: Option<(u64, u64)>,
	/// microseconds of CPU per cgroup
	usage: HashMap<PathBuf, (u64, i64)>,
	net: Option<(u64, u64)>,
	/// read position per access log: inode and offset
	logs: HashMap<String, (u64, u64)>,
	/// group -> bytes, and when it was measured
	disks: HashMap<String, i64>,
	disks_at: i64,
	/// the server's own addresses: its apps calling its sites (crelte's
	/// queries) are no visitors
	own: Vec<String>,
}

impl Collector {
	async fn round(&mut self, db: &Db, stats: &StatsDb) {
		let groups = hosting::groups(&db.conn()).unwrap_or_default();
		let at = now() / MINUTE * MINUTE;
		if now() - self.disks_at >= DISK_EVERY {
			self.disks = group_disks(&groups).await;
			self.disks_at = now();
			self.own = crate::dns::own_ips()
				.await
				.into_iter()
				.chain(["127.0.0.1".into(), "::1".into()])
				.collect();
		}

		let mut rows = vec![("server".to_string(), self.server(at).await)];
		for g in &groups {
			let mut group = Sample {
				at,
				disk: self.disks.get(&g.name).copied(),
				..Default::default()
			};
			(group.cpu, group.memory) =
				self.cgroup(&PathBuf::from(CGROUPS).join(slice(&g.name)));

			for d in &g.domains {
				let mut domain = self.log(&d.name).await;
				domain.at = at;
				if render::has_app(d) {
					let unit = PathBuf::from(CGROUPS)
						.join(slice(&g.name))
						.join(render::app_unit(&g.name, &d.name));
					(domain.cpu, domain.memory) = self.cgroup(&unit);
				}
				group.requests = add(group.requests, domain.requests);
				group.errors = add(group.errors, domain.errors);
				group.tx = add(group.tx, domain.tx);
				rows.push((format!("domain:{}", d.name), domain));
			}
			rows.push((format!("group:{}", g.name), group));
		}

		let conn = stats.conn();
		let res = store(&conn, 60, &rows).and_then(|()| rollup(&conn, at));
		drop(conn);
		if let Err(e) = res {
			tracing::error!("stats: {e}");
		}
	}

	async fn server(&mut self, at: i64) -> Sample {
		let mut s = Sample {
			at,
			..Default::default()
		};

		let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
		if let Some((busy, total)) = cpu_jiffies(&stat) {
			if let Some((b0, t0)) = self.cpu
				&& total > t0
			{
				s.cpu = Some((busy - b0) as f64 / (total - t0) as f64 * 100.0);
			}
			self.cpu = Some((busy, total));
		}

		let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
		if let (Some(total), Some(available)) =
			(meminfo(&info, "MemTotal:"), meminfo(&info, "MemAvailable:"))
		{
			s.memory = Some(total - available);
		}

		s.disk = sh("df -B1 --output=used / | tail -1")
			.await
			.ok()
			.and_then(|o| o.trim().parse().ok());

		let dev = std::fs::read_to_string("/proc/net/dev").unwrap_or_default();
		let (rx, tx) = net_bytes(&dev);
		if let Some((rx0, tx0)) = self.net {
			s.rx = Some(rx.saturating_sub(rx0) as i64);
			s.tx = Some(tx.saturating_sub(tx0) as i64);
		}
		self.net = Some((rx, tx));
		s
	}

	/// CPU in percent of all cores since the last minute and the memory of
	/// a cgroup, nothing if it doesn't exist (a stopped app).
	fn cgroup(&mut self, path: &Path) -> (Option<f64>, Option<i64>) {
		let memory = std::fs::read_to_string(path.join("memory.current"))
			.ok()
			.and_then(|m| m.trim().parse().ok());
		let usage = std::fs::read_to_string(path.join("cpu.stat"))
			.ok()
			.and_then(|s| {
				s.lines()
					.find_map(|l| l.strip_prefix("usage_usec "))
					.and_then(|v| v.trim().parse::<u64>().ok())
			});
		let Some(usage) = usage else {
			self.usage.remove(path);
			return (None, memory);
		};

		let cores = std::thread::available_parallelism()
			.map(|n| n.get())
			.unwrap_or(1) as f64;
		let t = now();
		let cpu = self.usage.get(path).and_then(|(u0, t0)| {
			let ms = (t - t0) as f64;
			(ms > 0.0 && usage >= *u0)
				.then(|| (usage - u0) as f64 / 1000.0 / ms / cores * 100.0)
		});
		self.usage.insert(path.to_path_buf(), (usage, t));
		(cpu, memory)
	}

	/// Requests, errors and bytes sent since the last minute, from the new
	/// lines of the domain's access log.
	async fn log(&mut self, domain: &str) -> Sample {
		let path = format!("{NGINX_LOGS}/{domain}.access.log");
		let Ok(meta) = tokio::fs::metadata(&path).await else {
			return Sample::default();
		};
		let (inode, size) = (meta.ino(), meta.len());

		let start = match self.logs.get(domain) {
			// rotated, or a new file
			Some((i, offset)) if *i == inode && *offset <= size => *offset,
			Some(_) => 0,
			// genco just started: the lines before are no new requests
			None => {
				self.logs.insert(domain.into(), (inode, size));
				return Sample::default();
			}
		};

		let mut new = vec![];
		if let Ok(mut f) = tokio::fs::File::open(&path).await {
			let _ = f.seek(std::io::SeekFrom::Start(start)).await;
			// at most 64 MB a minute, the rest is read next time
			let _ = f.take(64 << 20).read_to_end(&mut new).await;
		}
		// a line nginx is still writing stays for the next round
		let complete =
			new.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
		self.logs
			.insert(domain.into(), (inode, start + complete as u64));

		count_requests(&String::from_utf8_lossy(&new[..complete]), &self.own)
	}
}

fn slice(group: &str) -> String {
	format!("genco-{group}.slice")
}

fn add(a: Option<i64>, b: Option<i64>) -> Option<i64> {
	match (a, b) {
		(None, None) => None,
		(a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
	}
}

/// Busy and total jiffies from the `cpu` line of /proc/stat.
fn cpu_jiffies(stat: &str) -> Option<(u64, u64)> {
	let line = stat.lines().find(|l| l.starts_with("cpu "))?;
	let v: Vec<u64> = line
		.split_whitespace()
		.skip(1)
		.filter_map(|n| n.parse().ok())
		.collect();
	if v.len() < 5 {
		return None;
	}
	// user nice system idle iowait irq softirq steal
	let total: u64 = v.iter().take(8).sum();
	let idle = v[3] + v[4];
	Some((total - idle, total))
}

/// A field of /proc/meminfo in bytes.
pub fn meminfo(info: &str, field: &str) -> Option<i64> {
	let kb: i64 = info
		.lines()
		.find(|l| l.starts_with(field))?
		.split_whitespace()
		.nth(1)?
		.parse()
		.ok()?;
	Some(kb * 1024)
}

/// Bytes received and sent by every interface but loopback.
fn net_bytes(dev: &str) -> (u64, u64) {
	let mut rx = 0;
	let mut tx = 0;
	for line in dev.lines().skip(2) {
		let Some((name, rest)) = line.split_once(':') else {
			continue;
		};
		if name.trim() == "lo" {
			continue;
		}
		let v: Vec<u64> = rest
			.split_whitespace()
			.filter_map(|n| n.parse().ok())
			.collect();
		if v.len() >= 9 {
			rx += v[0];
			tx += v[8];
		}
	}
	(rx, tx)
}

/// Lines in nginx's combined format: `ip - - [time] "request" status
/// bytes ...`. Requests from the server itself (genco's checks, apps
/// calling a site) don't count.
fn count_requests(lines: &str, own: &[String]) -> Sample {
	let mut s = Sample {
		requests: Some(0),
		errors: Some(0),
		tx: Some(0),
		..Default::default()
	};
	for line in lines.lines() {
		let ip = line.split(' ').next().unwrap_or("");
		if own.iter().any(|o| o == ip) {
			continue;
		}
		let Some(after) = line.split('"').nth(2) else {
			continue;
		};
		let mut fields = after.split_whitespace();
		let status: u16 =
			fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
		let bytes: i64 =
			fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
		*s.requests.as_mut().unwrap() += 1;
		*s.tx.as_mut().unwrap() += bytes;
		if status >= 500 {
			*s.errors.as_mut().unwrap() += 1;
		}
	}
	s
}

/// Bytes of each group: its directory and its databases.
async fn group_disks(groups: &[Group]) -> HashMap<String, i64> {
	let sizes = sh("mariadb -N -B -e 'SELECT table_schema, \
		SUM(data_length + index_length) FROM information_schema.tables \
		GROUP BY table_schema'")
	.await
	.unwrap_or_default();
	let mut databases: HashMap<String, i64> = sizes
		.lines()
		.filter_map(|l| {
			let (name, size) = l.split_once('\t')?;
			Some((name.to_string(), size.trim().parse().ok()?))
		})
		.collect();
	databases.extend(crate::pg::sizes().await);

	let mut disks = HashMap::new();
	for g in groups {
		let dir = render::group_dir(&g.name);
		let files: i64 = sh(&format!("du -sb {} | cut -f1", dir.display()))
			.await
			.ok()
			.and_then(|o| o.trim().parse().ok())
			.unwrap_or(0);
		let dbs: i64 = g
			.databases
			.iter()
			.filter_map(|d| databases.get(&d.name))
			.sum();
		disks.insert(g.name.clone(), files + dbs);
	}
	disks
}

fn store(
	conn: &Connection,
	period: i64,
	rows: &[(String, Sample)],
) -> rusqlite::Result<()> {
	let mut stmt = conn.prepare(
		"INSERT INTO samples (at, period, scope, cpu, memory, disk, rx, tx,
		requests, errors) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
	)?;
	for (scope, s) in rows {
		stmt.execute(params![
			s.at, period, scope, s.cpu, s.memory, s.disk, s.rx, s.tx,
			s.requests, s.errors
		])?;
	}
	Ok(())
}

/// Sums up the last full hour once, and forgets old samples.
/// Sums up every full hour since the last one summed up (also after
/// genco was down), and forgets old samples.
fn rollup(conn: &Connection, at: i64) -> rusqlite::Result<()> {
	let last_full = at / HOUR * HOUR - HOUR;
	let newest: Option<i64> = conn.query_row(
		"SELECT max(at) FROM samples WHERE period = 3600",
		[],
		|r| r.get(0),
	)?;
	let first = newest
		.map(|n| n + HOUR)
		.unwrap_or(last_full)
		.max(at - KEEP_MINUTES / HOUR * HOUR);
	let mut hour = first;
	while hour <= last_full {
		conn.execute(
			"INSERT INTO samples (at, period, scope, cpu, memory, disk, rx,
			tx, requests, errors)
			SELECT ?1, 3600, scope, avg(cpu), avg(memory), max(disk),
			sum(rx), sum(tx), sum(requests), sum(errors)
			FROM samples WHERE period = 60 AND at >= ?1 AND at < ?2
			GROUP BY scope",
			params![hour, hour + HOUR],
		)?;
		hour += HOUR;
	}
	conn.execute(
		"DELETE FROM samples WHERE (period = 60 AND at < ?1)
		OR (period = 3600 AND at < ?2)",
		params![at - KEEP_MINUTES, at - KEEP_HOURS],
	)?;
	Ok(())
}

/// Every minute; the first round only remembers the counters.
pub fn spawn(db: Db, stats: StatsDb) {
	tokio::spawn(async move {
		let mut collector = Collector::default();
		let mut tick = tokio::time::interval(Duration::from_secs(60));
		loop {
			tick.tick().await;
			collector.round(&db, &stats).await;
		}
	});
}

#[derive(Debug, Deserialize)]
struct StatsQuery {
	/// `server`, `group:<id>` or `domain:<id>`
	scope: String,
	/// 1h, 24h, 7d or 30d
	range: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsResp {
	/// seconds per point
	period: i64,
	/// what memory and disk can reach, for the scale
	memory_limit: Option<i64>,
	disk_limit: Option<i64>,
	points: Vec<Sample>,
	/// of a group: its domains over the range
	domains: Vec<DomainTotal>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DomainTotal {
	name: String,
	requests: i64,
	errors: i64,
	tx: i64,
}

fn range(range: &str) -> Result<(i64, i64)> {
	match range {
		"1h" => Ok((60, HOUR)),
		"24h" => Ok((60, 24 * HOUR)),
		"7d" => Ok((3600, 7 * 24 * HOUR)),
		"30d" => Ok((3600, 30 * 24 * HOUR)),
		_ => Err(Error::Invalid("range is 1h, 24h, 7d or 30d".into())),
	}
}

fn points(
	conn: &Connection,
	scope: &str,
	period: i64,
	since: i64,
) -> rusqlite::Result<Vec<Sample>> {
	let mut stmt = conn.prepare(
		"SELECT at, cpu, memory, disk, rx, tx, requests, errors FROM samples
		WHERE scope = ?1 AND period = ?2 AND at >= ?3 ORDER BY at",
	)?;
	stmt.query_map(params![scope, period, since], |r| {
		Ok(Sample {
			at: r.get(0)?,
			cpu: r.get(1)?,
			memory: r.get(2)?,
			disk: r.get(3)?,
			rx: r.get(4)?,
			tx: r.get(5)?,
			requests: r.get(6)?,
			errors: r.get(7)?,
		})
	})?
	.collect()
}

fn totals(
	conn: &Connection,
	domains: &[String],
	period: i64,
	since: i64,
) -> rusqlite::Result<Vec<DomainTotal>> {
	let mut stmt = conn.prepare(
		"SELECT coalesce(sum(requests), 0), coalesce(sum(errors), 0),
		coalesce(sum(tx), 0) FROM samples
		WHERE scope = ?1 AND period = ?2 AND at >= ?3",
	)?;
	domains
		.iter()
		.map(|name| {
			stmt.query_row(
				params![format!("domain:{name}"), period, since],
				|r| {
					Ok(DomainTotal {
						name: name.clone(),
						requests: r.get(0)?,
						errors: r.get(1)?,
						tx: r.get(2)?,
					})
				},
			)
		})
		.collect()
}

async fn stats_route(
	_reader: Reader,
	State(db): State<Db>,
	State(stats): State<StatsDb>,
	Query(q): Query<StatsQuery>,
) -> Result<Json<StatsResp>> {
	let (period, span) = range(&q.range)?;
	let groups = hosting::groups(&db.conn())?;
	let id = |prefix: &str| -> Option<i64> {
		q.scope.strip_prefix(prefix)?.parse().ok()
	};

	let (scope, memory_limit, disk_limit, domains) = if q.scope == "server" {
		let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
		let disk = sh("df -B1 --output=size / | tail -1")
			.await
			.ok()
			.and_then(|o| o.trim().parse().ok());
		(
			"server".to_string(),
			meminfo(&info, "MemTotal:"),
			disk,
			vec![],
		)
	} else if let Some(g) =
		id("group:").and_then(|id| groups.iter().find(|g| g.id == id))
	{
		let max = std::fs::read_to_string(
			PathBuf::from(CGROUPS)
				.join(slice(&g.name))
				.join("memory.max"),
		)
		.ok()
		.and_then(|m| m.trim().parse().ok());
		let names = g.domains.iter().map(|d| d.name.clone()).collect();
		(format!("group:{}", g.name), max, None, names)
	} else if let Some(d) = id("domain:").and_then(|id| {
		groups.iter().flat_map(|g| &g.domains).find(|d| d.id == id)
	}) {
		(format!("domain:{}", d.name), None, None, vec![])
	} else {
		return Err(Error::NotFound);
	};

	let since = now() - span;
	let conn = stats.conn();
	Ok(Json(StatsResp {
		period,
		memory_limit,
		disk_limit,
		points: points(&conn, &scope, period, since)?,
		domains: totals(&conn, &domains, period, since)?,
	}))
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/stats", get(stats_route))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn counts_requests_errors_and_bytes_but_not_the_server_itself() {
		let log = "\
1.2.3.4 - - [01/Nov/2026:17:37:10 +0100] \"GET / HTTP/2.0\" 200 1000 \"-\" \"x\"
1.2.3.4 - - [01/Nov/2026:17:37:11 +0100] \"GET /a HTTP/2.0\" 502 50 \"-\" \"x\"
127.0.0.1 - - [01/Nov/2026:17:37:12 +0100] \"GET / HTTP/2.0\" 401 12 \"-\" \"curl\"
5.6.7.8 - - [01/Nov/2026:17:37:13 +0100] \"POST /queries/global HTTP/1.1\" 200 9 \"-\" \"node\"
";
		let own = ["127.0.0.1".to_string(), "5.6.7.8".to_string()];
		let s = count_requests(log, &own);
		assert_eq!(s.requests, Some(2));
		assert_eq!(s.errors, Some(1));
		assert_eq!(s.tx, Some(1050));
	}

	#[test]
	fn cpu_and_network_from_proc() {
		let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3 4\n";
		assert_eq!(cpu_jiffies(stat), Some((150, 1000)));

		let dev = "Inter-|\n face |\n    lo: 5 1 0 0 0 0 0 0 5 1 0 0 0 0 0 0\n  ens3: 100 1 0 0 0 0 0 0 40 1 0 0 0 0 0 0\n";
		assert_eq!(net_bytes(dev), (100, 40));
	}

	#[test]
	fn missed_hours_are_summed_up_later() {
		let stats = StatsDb::memory();
		let conn = stats.conn();
		for h in [8, 9] {
			let rows = vec![(
				"server".to_string(),
				Sample {
					at: h * HOUR + MINUTE,
					requests: Some(1),
					..Default::default()
				},
			)];
			store(&conn, 60, &rows).unwrap();
		}
		rollup(&conn, 9 * HOUR).unwrap();
		// genco was down from 9 to 11
		rollup(&conn, 11 * HOUR + MINUTE).unwrap();
		let hours = points(&conn, "server", 3600, 0).unwrap();
		let at: Vec<i64> = hours.iter().map(|h| h.at / HOUR).collect();
		// 9 was missed while genco was down, 10 had no samples
		assert_eq!(at, [8, 9]);
	}

	#[test]
	fn the_hour_is_summed_up_once() {
		let stats = StatsDb::memory();
		let conn = stats.conn();
		let at = 10 * HOUR;
		let rows = |cpu, req| {
			vec![(
				"server".to_string(),
				Sample {
					at: at - 30 * MINUTE,
					cpu: Some(cpu),
					requests: Some(req),
					..Default::default()
				},
			)]
		};
		store(&conn, 60, &rows(10.0, 5)).unwrap();
		store(&conn, 60, &rows(30.0, 7)).unwrap();

		rollup(&conn, at).unwrap();
		rollup(&conn, at).unwrap();
		let hours = points(&conn, "server", 3600, 0).unwrap();
		assert_eq!(hours.len(), 1);
		assert_eq!(hours[0].cpu, Some(20.0));
		assert_eq!(hours[0].requests, Some(12));
	}
}
