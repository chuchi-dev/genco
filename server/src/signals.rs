//! Signs that a group may be compromised, for the minute check
//! (checks.rs): they only wake genco's agent with an alert, it looks into
//! them and decides (it may put the group in quarantine, quarantine.rs).
//! Cheap and without the llm:
//!
//! - a group uses (nearly) all of its CPU limit for 10 minutes, outside a
//!   deploy: a miner
//! - a process of a group runs a program from /tmp, /var/tmp, /dev/shm or
//!   one that was deleted (outside /usr, where updates replace them)
//! - a shell, netcat, perl or python under the group's PHP-FPM for two
//!   minutes: a web shell
//! - a group's connection to a port mining pools and IRC bots use
//! - a group's SMTP login sends far more mail today than usual: spam
//! - new PHP files in a group's upload folders, every 15 minutes
//!
//! A sign's key holds what was found (the program, the file, the port):
//! a new one alerts at once, not only after the `REALERT` of the same
//! group's last one.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::checks::Problem;
use crate::hosting::Group;
use crate::stats::StatsDb;

/// Of its CPU limit, for `CPU_MINUTES`.
const CPU_SHARE: f64 = 0.9;
const CPU_MINUTES: u32 = 10;
/// Programs a web shell runs under PHP-FPM.
const SHELLS: &[&str] = &[
	"sh", "bash", "dash", "zsh", "nc", "ncat", "netcat", "socat", "perl",
	"python", "python3",
];
/// Ports of mining pools (stratum) and IRC bots.
const POOL_PORTS: &[u16] = &[
	3333, 4444, 5555, 6666, 6667, 7777, 9999, 14433, 14444, 45560, 45700,
];
/// Mail of a group's SMTP login today: at least this many, and this many
/// times the most of a day in the week before.
const MAIL_MIN: i64 = 300;
const MAIL_FACTOR: i64 = 5;
/// Rounds between the searches for PHP files in upload folders, and the
/// minutes a file counts as new.
const UPLOAD_ROUNDS: u32 = 15;
const UPLOAD_MINUTES: u32 = 20;

const DAY_MS: i64 = 24 * 3600 * 1000;

#[derive(Default)]
pub struct Signals {
	/// CPU time of each group's slice in µs at the last round
	cpu: HashMap<String, (u64, Instant)>,
	round: u32,
}

impl Signals {
	/// The signs of this round; groups in quarantine are stopped already.
	pub async fn find(
		&mut self,
		groups: &[Group],
		stats: Option<&StatsDb>,
	) -> Vec<Problem> {
		let groups: Vec<&Group> =
			groups.iter().filter(|g| g.quarantine.is_none()).collect();
		let users = group_users(&groups);
		let mut found = self.cpu(&groups);
		found.extend(processes(&users));
		found.extend(connections(&users));
		if let Some(stats) = stats {
			found.extend(mail(&groups, stats));
		}
		self.round += 1;
		if self.round.is_multiple_of(UPLOAD_ROUNDS) {
			found.extend(uploads(&groups).await);
		}
		found
	}

	/// A group whose slice used nearly its whole CPU limit since the last
	/// round, not while it deploys.
	fn cpu(&mut self, groups: &[&Group]) -> Vec<Problem> {
		let mut found = vec![];
		for g in groups {
			let Some(usage) = slice_usage(&g.name) else {
				continue;
			};
			let at = Instant::now();
			let last = self.cpu.insert(g.name.clone(), (usage, at));
			let Some((before, then)) = last else {
				continue;
			};
			let secs = at.duration_since(then).as_secs_f64();
			let deploying =
				!crate::job::is_free(&crate::deploy::lock_path(&g.name));
			if secs < 1.0 || deploying {
				continue;
			}
			// percent of one core, like the limit
			let used = usage.saturating_sub(before) as f64 / secs / 1e4;
			let limit = g.limits.cpu() as f64;
			if used >= limit * CPU_SHARE {
				found.push(
					Problem::new(
						format!("signal:cpu:{}", g.name),
						format!(
							"the group {} uses {used:.0}% CPU of its limit \
							{limit:.0}% for {CPU_MINUTES} minutes, outside a \
							deploy (a miner?)",
							g.name
						),
					)
					.minutes(CPU_MINUTES),
				);
			}
		}
		found
	}
}

/// The CPU time of a group's slice in µs.
fn slice_usage(group: &str) -> Option<u64> {
	let path = format!(
		"/sys/fs/cgroup/genco.slice/{}/cpu.stat",
		crate::render::slice(group)
	);
	let stat = std::fs::read_to_string(path).ok()?;
	stat.lines()
		.find_map(|l| l.strip_prefix("usage_usec "))?
		.trim()
		.parse()
		.ok()
}

/// The uid of each group's user, to its group's name.
fn group_users(groups: &[&Group]) -> HashMap<u32, String> {
	let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
	users_of(&passwd, groups)
}

fn users_of(passwd: &str, groups: &[&Group]) -> HashMap<u32, String> {
	passwd
		.lines()
		.filter_map(|l| {
			let mut f = l.split(':');
			let name = f.next()?;
			let uid = f.nth(1)?.parse().ok()?;
			let g = groups.iter().find(|g| g.user == name)?;
			Some((uid, g.name.clone()))
		})
		.collect()
}

/// A process as the checks need it.
#[derive(Debug)]
struct Proc {
	pid: u32,
	ppid: u32,
	uid: u32,
	comm: String,
	exe: String,
}

fn read_proc(pid: u32) -> Option<Proc> {
	let dir = Path::new("/proc").join(pid.to_string());
	let status = std::fs::read_to_string(dir.join("status")).ok()?;
	let field = |name: &str| {
		status
			.lines()
			.find_map(|l| l.strip_prefix(name))
			.map(|v| v.trim().to_string())
	};
	Some(Proc {
		pid,
		ppid: field("PPid:")?.parse().ok()?,
		uid: field("Uid:")?.split_whitespace().next()?.parse().ok()?,
		comm: field("Name:")?,
		exe: std::fs::read_link(dir.join("exe"))
			.map(|p| p.display().to_string())
			.unwrap_or_default(),
	})
}

/// Programs from temporary folders or deleted ones, and shells under
/// PHP-FPM, of the groups' users.
fn processes(users: &HashMap<u32, String>) -> Vec<Problem> {
	let procs: Vec<Proc> = std::fs::read_dir("/proc")
		.into_iter()
		.flatten()
		.flatten()
		.filter_map(|e| e.file_name().to_str()?.parse().ok())
		.filter_map(read_proc)
		.collect();
	suspicious(&procs, users)
}

fn suspicious(procs: &[Proc], users: &HashMap<u32, String>) -> Vec<Problem> {
	let by_pid: HashMap<u32, &Proc> =
		procs.iter().map(|p| (p.pid, p)).collect();
	let mut found: HashMap<String, Problem> = HashMap::new();
	for p in procs {
		let Some(group) = users.get(&p.uid) else {
			continue;
		};
		let odd_exe = ["/tmp/", "/var/tmp/", "/dev/shm/"]
			.iter()
			.any(|dir| p.exe.starts_with(dir))
			|| (p.exe.ends_with(" (deleted)") && !p.exe.starts_with("/usr/"));
		if odd_exe {
			let key = format!("signal:exe:{group}:{}", p.exe);
			found.entry(key.clone()).or_insert_with(|| {
				Problem::new(
					key,
					format!(
						"a process of the group {group} runs {} (pid {}, {})",
						p.exe, p.pid, p.comm
					),
				)
			});
		}
		let parent = by_pid.get(&p.ppid).map(|p| p.comm.as_str());
		if SHELLS.contains(&p.comm.as_str())
			&& parent.is_some_and(|c| c.starts_with("php-fpm"))
		{
			let key = format!("signal:shell:{group}:{}", p.comm);
			found.entry(key.clone()).or_insert_with(|| {
				Problem::new(
					key,
					format!(
						"{} runs under the PHP-FPM of the group {group} \
						(pid {}, a web shell?)",
						p.comm, p.pid
					),
				)
			});
		}
	}
	let mut found: Vec<Problem> = found.into_values().collect();
	found.sort_by(|a, b| a.key.cmp(&b.key));
	found
}

/// Connections of the groups' users to the ports of mining pools.
fn connections(users: &HashMap<u32, String>) -> Vec<Problem> {
	let mut tables = String::new();
	for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
		tables.push_str(&std::fs::read_to_string(f).unwrap_or_default());
	}
	pool_connections(&tables, users)
}

fn pool_connections(
	tables: &str,
	users: &HashMap<u32, String>,
) -> Vec<Problem> {
	let mut found: Vec<Problem> = vec![];
	for line in tables.lines() {
		let f: Vec<&str> = line.split_whitespace().collect();
		// established, the remote port
		if f.len() < 8 || f[3] != "01" {
			continue;
		}
		let Some(port) = f[2]
			.rsplit(':')
			.next()
			.and_then(|p| u16::from_str_radix(p, 16).ok())
		else {
			continue;
		};
		let Some(group) = f[7].parse().ok().and_then(|uid| users.get(&uid))
		else {
			continue;
		};
		let key = format!("signal:pool:{group}:{port}");
		if POOL_PORTS.contains(&port) && !found.iter().any(|p| p.key == key) {
			found.push(Problem::new(
				key,
				format!(
					"the group {group} keeps a connection to port {port}, \
					which mining pools and IRC bots use"
				),
			));
		}
	}
	found
}

/// A group's SMTP login sending far more today than any day of the week
/// before.
fn mail(groups: &[&Group], stats: &StatsDb) -> Vec<Problem> {
	let today = crate::db::now() / DAY_MS * DAY_MS;
	let conn = stats.conn();
	let mut found = vec![];
	for g in groups.iter().filter(|g| !g.mail_domains.is_empty()) {
		let sent = |from: i64, to: i64| -> i64 {
			conn.query_row(
				"SELECT coalesce(max(count), 0) FROM mail_counts
				WHERE address = ?1 AND kind = 'sent' AND day >= ?2
				AND day < ?3",
				rusqlite::params![g.user, from, to],
				|r| r.get(0),
			)
			.unwrap_or(0)
		};
		let now = sent(today, today + DAY_MS);
		let before = sent(today - 7 * DAY_MS, today);
		if burst(now, before) {
			found.push(
				Problem::new(
					format!("signal:mail:{}", g.name),
					format!(
						"the group {} sent {now} mails today through its SMTP \
						login, at most {before} a day in the week before \
						(spam?)",
						g.name
					),
				)
				.minutes(1),
			);
		}
	}
	found
}

fn burst(today: i64, most_before: i64) -> bool {
	today >= MAIL_MIN && today > most_before * MAIL_FACTOR
}

/// New PHP files in the groups' upload folders, not while a group
/// deploys; at the lowest priority.
async fn uploads(groups: &[&Group]) -> Vec<Problem> {
	let mut found = vec![];
	for g in groups {
		if !crate::job::is_free(&crate::deploy::lock_path(&g.name)) {
			continue;
		}
		let dir = g.dir.replace('\'', "'\\''");
		let script = format!(
			"nice -n 19 ionice -c3 find '{dir}' -xdev \
			\\( -name vendor -o -name node_modules -o -name .git \\) -prune \
			-o \\( -path '*/upload/*' -o -path '*/uploads/*' \
			-o -path '*/media/*' -o -path '*/files/*' \\) \
			\\( -iname '*.php*' -o -iname '*.phtml' -o -iname '*.phar' \\) \
			-type f -mmin -{UPLOAD_MINUTES} -print 2>/dev/null | head -5",
		);
		let out = crate::util::sh(&script).await.unwrap_or_default();
		let files: Vec<&str> = out.lines().collect();
		if let Some(first) = files.first() {
			found.push(
				Problem::new(
					format!("signal:upload:{}:{first}", g.name),
					format!(
						"new PHP files in an upload folder of the group {}: \
						{first}{}",
						g.name,
						if files.len() > 1 { " and more" } else { "" }
					),
				)
				.minutes(1),
			);
		}
	}
	found
}

#[cfg(test)]
mod tests {
	use super::*;

	fn group(name: &str) -> Group {
		Group {
			name: name.into(),
			user: format!("g-{name}"),
			..Default::default()
		}
	}

	#[test]
	fn uids_map_to_their_groups() {
		let shop = group("shop");
		let passwd = "root:x:0:0::/root:/bin/bash\n\
			g-shop:x:20001:20001::/srv/genco/shop:/usr/sbin/nologin\n\
			g-other:x:20002:20002::/srv/genco/other:/usr/sbin/nologin\n";
		let users = users_of(passwd, &[&shop]);
		assert_eq!(users, HashMap::from([(20001, "shop".to_string())]));
	}

	#[test]
	fn odd_programs_and_shells_under_php_are_signs() {
		let users = HashMap::from([(20001, "shop".to_string())]);
		let p = |pid, ppid, uid, comm: &str, exe: &str| Proc {
			pid,
			ppid,
			uid,
			comm: comm.into(),
			exe: exe.into(),
		};
		let procs = [
			p(10, 1, 0, "php-fpm8.4", "/usr/sbin/php-fpm8.4"),
			p(
				11,
				10,
				20001,
				"php-fpm8.4",
				"/usr/sbin/php-fpm8.4 (deleted)",
			),
			p(12, 11, 20001, "sh", "/usr/bin/dash"),
			p(13, 1, 20001, "xmrig", "/tmp/.x/xmrig"),
			p(14, 1, 20001, "app", "/srv/genco/shop/current/app"),
			p(15, 1, 33, "kworker", "/dev/shm/k"),
			p(16, 1, 20001, "node", "/usr/bin/node"),
		];
		let found = suspicious(&procs, &users);
		let keys: Vec<&str> = found.iter().map(|p| p.key.as_str()).collect();
		assert_eq!(
			keys,
			["signal:exe:shop:/tmp/.x/xmrig", "signal:shell:shop:sh"]
		);
		assert!(found[0].detail.contains("/tmp/.x/xmrig"));
		assert!(found[1].detail.contains("sh runs under the PHP-FPM"));
	}

	#[test]
	fn connections_to_pool_ports_are_signs() {
		let users = HashMap::from([(20001, "shop".to_string())]);
		let tables = "  sl  local_address rem_address   st tx rx tr tm retr uid\n\
			0: 0100007F:2C44 00000000:0000 0A 0:0 00:0 0 20001\n\
			1: 0A00000A:D3F2 5DB8D822:0D05 01 0:0 00:0 0 20001\n\
			2: 0A00000A:D3F3 5DB8D822:01BB 01 0:0 00:0 0 20001\n\
			3: 0A00000A:D3F4 5DB8D822:0D05 01 0:0 00:0 0 33\n";
		let found = pool_connections(tables, &users);
		assert_eq!(found.len(), 1);
		assert!(found[0].detail.contains("port 3333"));
	}

	#[test]
	fn a_mail_burst_is_far_above_the_usual() {
		assert!(!burst(299, 0));
		assert!(burst(300, 0));
		assert!(burst(600, 100));
		assert!(!burst(500, 100));
	}
}
