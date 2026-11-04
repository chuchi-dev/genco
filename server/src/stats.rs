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
use crate::users::Authed;
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

pub fn path(cfg: &crate::config::Config) -> PathBuf {
	cfg.data_dir.join("stats.db")
}

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
		conn.execute_batch(crate::mailbox::COUNTS_SCHEMA)
			.map_err(|e| e.to_string())?;
		Ok(Self(Arc::new(Mutex::new(conn))))
	}

	#[cfg(test)]
	pub fn memory() -> Self {
		let conn = Connection::open_in_memory().unwrap();
		conn.execute_batch(SCHEMA).unwrap();
		conn.execute_batch(crate::mailbox::COUNTS_SCHEMA).unwrap();
		Self(Arc::new(Mutex::new(conn)))
	}

	pub fn conn(&self) -> MutexGuard<'_, Connection> {
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
	mail: crate::mailbox::Counter,
}

impl Collector {
	async fn round(&mut self, db: &Db, stats: &StatsDb) {
		let groups = hosting::groups(&db.conn()).unwrap_or_default();
		let at = now() / MINUTE * MINUTE;
		if now() - self.disks_at >= DISK_EVERY {
			self.disks = group_disks(&groups).await;
			self.disks_at = now();
			self.own = own_addresses().await;
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
						.join(render::live_unit(&g.name, d));
					(domain.cpu, domain.memory) = self.cgroup(&unit);
				}
				group.requests = add(group.requests, domain.requests);
				group.errors = add(group.errors, domain.errors);
				group.tx = add(group.tx, domain.tx);
				rows.push((format!("domain:{}", d.name), domain));
			}
			rows.push((format!("group:{}", g.name), group));
		}

		let res = {
			let conn = stats.conn();
			store(&conn, 60, &rows).and_then(|()| rollup(&conn, at))
		};
		if let Err(e) = res {
			tracing::error!("stats: {e}");
		}

		self.mail.round(&groups, stats).await;
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
		let path = access_log(domain);
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

fn access_log(domain: &str) -> String {
	format!("{NGINX_LOGS}/{domain}.access.log")
}

/// The server's addresses, its requests don't count in the usage and the
/// top pages.
async fn own_addresses() -> Vec<String> {
	crate::dns::own_ips()
		.await
		.into_iter()
		.chain(["127.0.0.1".into(), "::1".into()])
		.collect()
}

/// Lines in nginx's combined format, see `Line`. Requests from the
/// server itself (genco's checks, apps calling a site) don't count.
fn count_requests(lines: &str, own: &[String]) -> Sample {
	let mut s = Sample {
		requests: Some(0),
		errors: Some(0),
		tx: Some(0),
		..Default::default()
	};
	for line in lines.lines().filter_map(Line::parse) {
		if own.iter().any(|o| o == line.ip) {
			continue;
		}
		*s.requests.as_mut().unwrap() += 1;
		*s.tx.as_mut().unwrap() += line.bytes;
		if line.status >= 500 {
			*s.errors.as_mut().unwrap() += 1;
		}
	}
	s
}

/// The fields of a line in nginx's combined format: `ip - user [time]
/// "request" status bytes ...`.
struct Line<'a> {
	ip: &'a str,
	time: &'a str,
	request: &'a str,
	status: u16,
	bytes: i64,
}

impl<'a> Line<'a> {
	/// Nothing for a cut line or another format.
	fn parse(line: &'a str) -> Option<Self> {
		let ip = line.split(' ').next()?;
		// the visitor picks the user (Basic auth) and nginx keeps its
		// brackets, but escapes `"`: the time is right before the request
		let (head, rest) = line.split_once('"')?;
		let time = head.trim_end().strip_suffix(']')?.rsplit_once('[')?.1;
		let (request, rest) = rest.split_once('"')?;
		let mut fields = rest.split_whitespace();
		let status = fields.next()?.parse().ok()?;
		let bytes = fields.next()?.parse().unwrap_or(0);
		Some(Self {
			ip,
			time,
			request,
			status,
			bytes,
		})
	}
}

/// A request of an access log line.
#[derive(Debug, PartialEq)]
struct Hit<'a> {
	ip: &'a str,
	/// milliseconds
	at: i64,
	/// without the query, at most `MAX_PATH` bytes
	path: &'a str,
	status: u16,
	bytes: i64,
}

/// Longer paths are cut, they are bots' garbage anyway.
const MAX_PATH: usize = 200;

/// Nothing for a line that isn't a request (a TLS handshake on the HTTP
/// port, a cut line).
fn parse_line(line: &str) -> Option<Hit<'_>> {
	let Line {
		ip,
		time,
		request,
		status,
		bytes,
	} = Line::parse(line)?;

	// `GET /a b HTTP/1.1`: the path may have spaces
	let target = request.split_once(' ')?.1;
	let target = match target.rsplit_once(' ') {
		Some((t, v)) if v.starts_with("HTTP/") => t,
		_ => target,
	};
	if !target.starts_with('/') {
		return None;
	}
	let mut path = target.split('?').next()?;
	if path.len() > MAX_PATH {
		let mut end = MAX_PATH;
		while !path.is_char_boundary(end) {
			end -= 1;
		}
		path = &path[..end];
	}

	Some(Hit {
		ip,
		at: log_time(time)?,
		path,
		status,
		bytes,
	})
}

/// `01/Nov/2026:17:37:10 +0100` in milliseconds.
fn log_time(time: &str) -> Option<i64> {
	let (date, zone) = time.split_once(' ')?;
	let mut parts = date.split([':', '/']);
	// small types: no overflow below, whatever the line says
	let day: u8 = parts.next()?.parse().ok()?;
	let month = parts.next()?;
	let month = [
		"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
		"Nov", "Dec",
	]
	.iter()
	.position(|m| *m == month)? as i64
		+ 1;
	let year: u16 = parts.next()?.parse().ok()?;
	let mut clock = [0i64; 3];
	for c in &mut clock {
		*c = parts.next()?.parse::<u8>().ok()?.into();
	}
	let (day, year) = (i64::from(day), i64::from(year));

	// days since 1970 of a proleptic Gregorian date (Howard Hinnant)
	let y = if month <= 2 { year - 1 } else { year };
	let era = y.div_euclid(400);
	let yoe = y - era * 400;
	let mp = (month + 9) % 12;
	let doy = (153 * mp + 2) / 5 + day - 1;
	let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
	let days = era * 146_097 + doe - 719_468;

	let sign = if zone.starts_with('-') { -1 } else { 1 };
	let offset = zone.get(1..3)?.parse::<u8>().ok()? as i64 * 3600
		+ zone.get(3..5)?.parse::<u8>().ok()? as i64 * 60;
	let secs = days * 86_400 + clock[0] * 3600 + clock[1] * 60 + clock[2];
	Some((secs - sign * offset) * 1000)
}

/// At most this much of the logs is read for the top pages.
const TOP_READ: u64 = 50 << 20;
/// Different paths counted, the rest (bots trying thousands) is left out.
const TOP_PATHS: usize = 50_000;
const TOP_LEN: usize = 50;

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Page {
	/// in `top_pages`' count per address the address
	path: String,
	requests: i64,
	/// answers 400 to 499
	client_errors: i64,
	/// 500 and above
	server_errors: i64,
	bytes: i64,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TopResp {
	pages: Vec<Page>,
	/// the most requested paths that answered 404
	not_found: Vec<Page>,
	/// the clients with the most requests
	addresses: Vec<Address>,
	/// the oldest request read when the logs don't go back to the start
	/// of the period (rotated, or more than `TOP_READ`)
	from: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Address {
	ip: String,
	requests: i64,
	client_errors: i64,
	server_errors: i64,
	/// its most requested path
	path: String,
}

/// The requests since `since` in the newest `TOP_READ` bytes of the
/// logs, newest file first. Requests of the server itself don't count,
/// like in the usage.
async fn top_pages(files: &[PathBuf], since: i64, own: &[String]) -> TopResp {
	use tokio::io::AsyncBufReadExt as _;

	let mut pages: HashMap<String, Page> = HashMap::new();
	let mut missing: HashMap<String, Page> = HashMap::new();
	let mut ips: HashMap<String, Page> = HashMap::new();
	// requests per address and path, for each address's top path
	let mut ip_paths: HashMap<(String, String), i64> = HashMap::new();
	let mut budget = TOP_READ;
	let mut oldest = None;
	let mut complete = false;

	for path in files {
		let Ok(mut f) = tokio::fs::File::open(path).await else {
			continue;
		};
		let size = f.metadata().await.map(|m| m.len()).unwrap_or(0);
		let start = size.saturating_sub(budget);
		let _ = f.seek(std::io::SeekFrom::Start(start)).await;
		budget -= size - start;
		let mut r = tokio::io::BufReader::new(f.take(size - start));

		let mut line = vec![];
		// the first line is cut unless it is the file's
		let mut skip = start > 0;
		let mut first = true;
		loop {
			line.clear();
			match r.read_until(b'\n', &mut line).await {
				Ok(0) | Err(_) => break,
				Ok(_) => {}
			}
			if std::mem::take(&mut skip) {
				continue;
			}
			let text = String::from_utf8_lossy(&line);
			let Some(hit) = parse_line(text.trim_end()) else {
				continue;
			};
			if first {
				first = false;
				oldest = Some(oldest.map_or(hit.at, |o: i64| o.min(hit.at)));
				complete |= hit.at < since;
			}
			if hit.at < since || own.iter().any(|o| o == hit.ip) {
				continue;
			}
			count(&mut pages, hit.path, &hit);
			if hit.status == 404 {
				count(&mut missing, hit.path, &hit);
			}
			count(&mut ips, hit.ip, &hit);
			let key = (hit.ip.to_string(), hit.path.to_string());
			if ip_paths.len() < TOP_PATHS || ip_paths.contains_key(&key) {
				*ip_paths.entry(key).or_default() += 1;
			}
		}
		if complete || budget == 0 {
			break;
		}
	}

	let addresses = top(ips)
		.into_iter()
		.map(|a| {
			let path = ip_paths
				.iter()
				.filter(|((ip, _), _)| *ip == a.path)
				.max_by(|(x, a), (y, b)| a.cmp(b).then_with(|| y.1.cmp(&x.1)))
				.map(|((_, path), _)| path.clone())
				.unwrap_or_default();
			Address {
				ip: a.path,
				requests: a.requests,
				client_errors: a.client_errors,
				server_errors: a.server_errors,
				path,
			}
		})
		.collect();

	TopResp {
		pages: top(pages),
		not_found: top(missing),
		addresses,
		from: oldest.filter(|_| !complete).map(|o| o.max(since)),
	}
}

/// Counts the request under `key`, a path or an address.
fn count(pages: &mut HashMap<String, Page>, key: &str, hit: &Hit) {
	if pages.len() >= TOP_PATHS && !pages.contains_key(key) {
		return;
	}
	let p = pages.entry(key.to_string()).or_insert_with(|| Page {
		path: key.to_string(),
		..Default::default()
	});
	p.requests += 1;
	p.bytes += hit.bytes;
	match hit.status {
		400..=499 => p.client_errors += 1,
		500.. => p.server_errors += 1,
		_ => {}
	}
}

fn top(pages: HashMap<String, Page>) -> Vec<Page> {
	let mut pages: Vec<Page> = pages.into_values().collect();
	pages.sort_by(|a, b| {
		b.requests
			.cmp(&a.requests)
			.then_with(|| a.path.cmp(&b.path))
	});
	pages.truncate(TOP_LEN);
	pages
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

#[derive(Debug, Deserialize)]
struct TopQuery {
	hours: i64,
}

/// The most requested paths of a domain, from its access log. Only for
/// admins: anyone can put text into a path (an AI agent's key must not
/// read it) and IPs are personal data.
async fn top_route(
	_authed: Authed,
	State(db): State<Db>,
	axum::extract::Path(id): axum::extract::Path<i64>,
	Query(q): Query<TopQuery>,
) -> Result<Json<TopResp>> {
	if !(1..=30 * 24).contains(&q.hours) {
		return Err(Error::Invalid("hours is 1 to 720".into()));
	}
	let (_, d) = hosting::find_domain(&db.conn(), |d| d.id == id)?;

	let log = access_log(&d.name);
	let files = [PathBuf::from(&log), PathBuf::from(format!("{log}.1"))];
	let own = own_addresses().await;
	let since = now() - q.hours * HOUR;
	Ok(Json(top_pages(&files, since, &own).await))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/stats", get(stats_route))
		.route("/domains/{id}/top", get(top_route))
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
	fn parses_a_line_without_the_query() {
		let line = "1.2.3.4 - - [01/Nov/2026:17:37:10 +0100] \"GET /a/b?token=x HTTP/2.0\" 404 153 \"-\" \"x\"";
		let hit = parse_line(line).unwrap();
		assert_eq!(hit.ip, "1.2.3.4");
		assert_eq!(hit.path, "/a/b");
		assert_eq!(hit.status, 404);
		assert_eq!(hit.bytes, 153);
		// 2026-11-01 16:37:10 UTC
		assert_eq!(hit.at, 1_793_551_030_000);
		assert_eq!(log_time("01/Jan/1970:01:00:00 +0100"), Some(0),);
	}

	#[test]
	fn paths_with_spaces_long_ones_and_garbage() {
		let line = "1.2.3.4 - - [01/Nov/2026:17:37:10 +0100] \"GET /a b HTTP/1.1\" 400 0 \"-\" \"x\"";
		assert_eq!(parse_line(line).unwrap().path, "/a b");

		let long = format!(
			"1.2.3.4 - - [01/Nov/2026:17:37:10 +0100] \"GET /{} HTTP/1.1\" 200 1 \"-\" \"x\"",
			"ä".repeat(300)
		);
		assert!(parse_line(&long).unwrap().path.len() <= MAX_PATH);

		for garbage in [
			"",
			"nonsense",
			"1.2.3.4 - - [01/Nov/2026:17:37:10 +0100] \"\\x16\\x03\\x01\" 400 157 \"-\" \"-\"",
			"1.2.3.4 - - [bad time] \"GET / HTTP/1.1\" 200 1 \"-\" \"x\"",
			"1.2.3.4 - - [01/Nov/2026:17:37:10 +0100] \"GET / HTTP/1.1",
			"1.2.3.4 - - [01/Nov/99999999999999:17:37:10 +0100] \"GET / HTTP/1.1\" 200 1",
			"1.2.3.4 - - [01/Nov/2026:17:37:10 +aä00] \"GET / HTTP/1.1\" 200 1",
		] {
			assert_eq!(parse_line(garbage), None, "{garbage}");
		}
	}

	#[test]
	fn the_user_cant_forge_the_time() {
		let line = "1.2.3.4 - [01/Jan/2000:00:00:00 +0000] [01/Nov/2026:17:37:10 +0100] \"GET / HTTP/1.1\" 200 1 \"-\" \"x\"";
		assert_eq!(parse_line(line).unwrap().at, 1_793_551_030_000);
	}

	#[tokio::test]
	async fn top_pages_of_the_period_from_both_logs() {
		let dir = std::env::temp_dir()
			.join(format!("genco-top-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let line = |time: &str, path: &str, status: u16| {
			format!(
				"1.2.3.4 - - [{time} +0000] \"GET {path} HTTP/2.0\" {status} 10 \"-\" \"x\"\n"
			)
		};
		let old = line("01/Nov/2026:08:00:00", "/old", 200)
			+ &line("01/Nov/2026:11:00:00", "/a", 200);
		let new = line("01/Nov/2026:12:00:00", "/a?x=1", 200)
			+ &line("01/Nov/2026:12:00:01", "/missing", 404)
			+ &line("01/Nov/2026:12:00:02", "/a", 502)
			+ "garbage\n"
			+ "127.0.0.1 - - [01/Nov/2026:12:00:03 +0000] \"GET /self HTTP/2.0\" 200 1 \"-\" \"x\"\n";
		let files = [dir.join("log"), dir.join("log.1")];
		std::fs::write(&files[0], new).unwrap();
		std::fs::write(&files[1], old).unwrap();

		// since 10:00 UTC: the old file is read up to its start
		let since = log_time("01/Nov/2026:10:00:00 +0000").unwrap();
		let own = ["127.0.0.1".to_string()];
		let top = top_pages(&files, since, &own).await;

		let a = Page {
			path: "/a".into(),
			requests: 3,
			client_errors: 0,
			server_errors: 1,
			bytes: 30,
		};
		assert_eq!(top.pages[0], a);
		assert_eq!(top.pages.len(), 2);
		assert_eq!(top.not_found.len(), 1);
		assert_eq!(top.not_found[0].path, "/missing");
		assert_eq!(top.not_found[0].client_errors, 1);
		assert_eq!(top.from, None);
		let addresses = top.addresses;
		assert_eq!(addresses.len(), 1);
		assert_eq!(addresses[0].ip, "1.2.3.4");
		assert_eq!(addresses[0].requests, 4);
		assert_eq!(addresses[0].client_errors, 1);
		assert_eq!(addresses[0].server_errors, 1);
		assert_eq!(addresses[0].path, "/a");

		// a period before the logs start
		let top = top_pages(&files[..1], since, &own).await;
		assert_eq!(top.from, log_time("01/Nov/2026:12:00:00 +0000"));
		std::fs::remove_dir_all(&dir).unwrap();
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
