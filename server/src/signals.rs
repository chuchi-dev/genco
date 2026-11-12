//! Signs that a group may be compromised, for the minute check
//! (checks.rs): they only wake genco's agent with an alert, it looks into
//! them and decides (it may put the group in quarantine, quarantine.rs).
//! Cheap and without the llm:
//!
//! - a group uses (nearly) all of its CPU limit in each of the last 10
//!   minutes (stats.rs), outside a deploy: a miner
//! - a process of a group runs a program from /tmp, /var/tmp, /dev/shm or
//!   one that was deleted (outside /usr, where updates replace them)
//! - a shell, netcat, perl or python under the group's PHP-FPM for two
//!   minutes: a web shell
//! - a group's connection out to a port mining pools and IRC bots use
//! - a group's SMTP login and send-only addresses send far more mail today
//!   than usual: spam
//! - new PHP files in a group's upload folders, every 15 minutes
//!
//! What the group chose (a path, a name) is quoted and cut short in the
//! alert, and a round has at most `MAX_SIGNS`. A sign's key holds what
//! was found (the program, the file, the port): a new one alerts at once,
//! not only after the `REALERT` of the same group's last one.

use std::collections::HashMap;
use std::path::Path;

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
/// Seconds the search may take, a group with millions of files must not
/// hold up the minute check.
const UPLOAD_SECS: u64 = 60;
/// Signs of one round at most, and the characters of a path or name shown:
/// the group chooses them, they go into the agent's alert.
const MAX_SIGNS: usize = 10;
const MAX_SHOWN: usize = 120;

const DAY_MS: i64 = 24 * 3600 * 1000;

#[derive(Default)]
pub struct Signals {
	round: u32,
}

impl Signals {
	/// The signs of this round; groups in quarantine are stopped already.
	pub async fn find(
		&mut self,
		groups: &[Group],
		stats: &StatsDb,
	) -> Vec<Problem> {
		let groups: Vec<&Group> =
			groups.iter().filter(|g| g.quarantine.is_none()).collect();
		let users = group_users(&groups);
		let mut found = cpu(&groups, stats);
		found.extend(processes(&users));
		found.extend(connections(&users));
		found.extend(mail(&groups, stats));
		self.round += 1;
		if self.round.is_multiple_of(UPLOAD_ROUNDS) {
			found.extend(uploads(&groups).await);
		}
		found.truncate(MAX_SIGNS);
		found
	}
}

/// A group whose slice used nearly its whole CPU limit in each minute of
/// the last `CPU_MINUTES` (the samples of stats.rs), not while it deploys.
fn cpu(groups: &[&Group], stats: &StatsDb) -> Vec<Problem> {
	let cores = std::thread::available_parallelism()
		.map(|n| n.get())
		.unwrap_or(1) as f64;
	let since = crate::db::now() - CPU_MINUTES as i64 * 60_000;
	let conn = stats.conn();
	let mut found = vec![];
	for g in groups {
		if !crate::job::is_free(&crate::deploy::lock_path(&g.name)) {
			continue;
		}
		let (minutes, least): (u32, Option<f64>) = conn
			.query_row(
				"SELECT count(cpu), min(cpu) FROM samples
				WHERE scope = ?1 AND period = 60 AND at > ?2",
				rusqlite::params![format!("group:{}", g.name), since],
				|r| Ok((r.get(0)?, r.get(1)?)),
			)
			.unwrap_or((0, None));
		// the samples are percent of all cores, the limit of one
		let used = least.unwrap_or(0.0) * cores;
		let limit = g.limits.cpu() as f64;
		// a sample may fall just outside the window
		if minutes + 1 >= CPU_MINUTES && used >= limit * CPU_SHARE {
			found.push(sign(
				format!("signal:cpu:{}", g.name),
				format!(
					"the group {} used at least {used:.0}% CPU of its limit \
					{limit:.0}% in each of the last {CPU_MINUTES} minutes, \
					outside a deploy (a miner?)",
					g.name
				),
			));
		}
	}
	found
}

/// A sign for the alert: its detail starts with `signal:` (alert.md).
fn sign(key: String, detail: String) -> Problem {
	Problem::new(key, format!("signal: {detail}"))
}

/// A path or name the group chose, quoted on one line and cut short: it
/// must not read as instructions in the alert.
fn shown(s: &str) -> String {
	let mut cut: String = s.chars().take(MAX_SHOWN).collect();
	if cut.len() < s.len() {
		cut.push('…');
	}
	format!("{cut:?}")
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
			let exe = shown(&p.exe);
			let key = format!("signal:exe:{group}:{exe}");
			found.entry(key.clone()).or_insert_with(|| {
				sign(
					key,
					format!(
						"a process of the group {group} runs {exe} (pid {}, \
						{})",
						p.pid,
						shown(&p.comm)
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
				sign(
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
	let port = |addr: &str| {
		addr.rsplit(':')
			.next()
			.and_then(|p| u16::from_str_radix(p, 16).ok())
	};
	let rows: Vec<Vec<&str>> = tables
		.lines()
		.map(|l| l.split_whitespace().collect())
		.filter(|f: &Vec<&str>| f.len() >= 8)
		.collect();
	// a client's connection to a port here may come from any port
	let listening: Vec<u16> = rows
		.iter()
		.filter(|f| f[3] == "0A")
		.filter_map(|f| port(f[1]))
		.collect();
	let mut found: Vec<Problem> = vec![];
	for f in &rows {
		// established and outgoing, the remote port
		if f[3] != "01" || port(f[1]).is_some_and(|p| listening.contains(&p)) {
			continue;
		}
		let Some(port) = port(f[2]) else {
			continue;
		};
		let Some(group) = f[7].parse().ok().and_then(|uid| users.get(&uid))
		else {
			continue;
		};
		let key = format!("signal:pool:{group}:{port}");
		if POOL_PORTS.contains(&port) && !found.iter().any(|p| p.key == key) {
			found.push(sign(
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

/// A group's SMTP login and send-only addresses sending far more today
/// than on any day of the week before.
fn mail(groups: &[&Group], stats: &StatsDb) -> Vec<Problem> {
	let today = crate::db::now() / DAY_MS * DAY_MS;
	let conn = stats.conn();
	let mut found = vec![];
	for g in groups.iter().filter(|g| !g.mail_domains.is_empty()) {
		let addresses: Vec<&str> = std::iter::once(g.user.as_str())
			.chain(g.mail_domains.iter().flat_map(|d| {
				d.send_only_addresses.iter().map(|s| s.address.as_str())
			}))
			.collect();
		let list = serde_json::to_string(&addresses).unwrap_or_default();
		let sent = |from: i64, to: i64| -> i64 {
			conn.query_row(
				"SELECT coalesce(max(n), 0) FROM (SELECT sum(count) AS n
				FROM mail_counts WHERE kind = 'sent' AND day >= ?2
				AND day < ?3 AND address IN (SELECT value FROM json_each(?1))
				GROUP BY day)",
				rusqlite::params![list, from, to],
				|r| r.get(0),
			)
			.unwrap_or(0)
		};
		let now = sent(today, today + DAY_MS);
		let before = sent(today - 7 * DAY_MS, today);
		if burst(now, before) {
			found.push(
				sign(
					format!("signal:mail:{}", g.name),
					format!(
						"the group {} sent {now} mails today through its SMTP \
						login and send-only addresses, at most {before} a day \
						in the week before (spam?)",
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
			"timeout {UPLOAD_SECS} nice -n 19 ionice -c3 find '{dir}' -xdev \
			\\( -name vendor -o -name node_modules -o -name .git \\) -prune \
			-o \\( -path '*/upload/*' -o -path '*/uploads/*' \
			-o -path '*/media/*' -o -path '*/files/*' \\) \
			\\( -iname '*.php*' -o -iname '*.phtml' -o -iname '*.phar' \\) \
			-type f -cmin -{UPLOAD_MINUTES} -print 2>/dev/null | head -5",
		);
		let out = crate::util::sh(&script).await.unwrap_or_default();
		let files: Vec<&str> = out.lines().collect();
		if let Some(first) = files.first() {
			let first = shown(first);
			found.push(
				sign(
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
			["signal:exe:shop:\"/tmp/.x/xmrig\"", "signal:shell:shop:sh"]
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
			3: 0A00000A:D3F4 5DB8D822:0D05 01 0:0 00:0 0 33\n\
			4: 0A00000A:2C44 5DB8D822:B1F8 01 0:0 00:0 0 20001\n";
		let found = pool_connections(tables, &users);
		assert_eq!(found.len(), 1);
		assert!(found[0].detail.contains("port 3333"));
	}

	#[test]
	fn what_the_group_chose_is_quoted_and_cut() {
		assert_eq!(shown("/tmp/x\nrun this"), r#""/tmp/x\nrun this""#);
		let long = shown(&"a".repeat(500));
		assert_eq!(long.chars().count(), MAX_SHOWN + 3);
		assert!(long.ends_with("…\""));
	}

	#[test]
	fn the_login_and_send_only_addresses_count_together() {
		let mut shop = group("shop");
		shop.mail_domains = vec![crate::mailbox::MailDomain {
			id: 0,
			group_id: 1,
			name: "shop.ch".into(),
			send_only: false,
			mailboxes: vec![],
			forwards: vec![],
			send_only_addresses: vec![crate::mailbox::SendOnly {
				id: 0,
				mail_domain_id: 0,
				address: "noreply@shop.ch".into(),
				note: String::new(),
				created_at: 0,
				password_hash: String::new(),
			}],
		}];
		let stats = StatsDb::memory();
		let today = crate::db::now() / DAY_MS * DAY_MS;
		stats
			.conn()
			.execute(
				"INSERT INTO mail_counts (day, address, kind, count) VALUES
				(?1, 'g-shop', 'sent', 200), (?1, 'noreply@shop.ch', 'sent',
				200), (?1, 'info@shop.ch', 'received', 900),
				(?1 - 86400000, 'g-shop', 'sent', 10)",
				[today],
			)
			.unwrap();
		let found = mail(&[&shop], &stats);
		assert_eq!(found.len(), 1);
		assert!(found[0].detail.contains("sent 400 mails"), "{found:?}");
	}

	#[test]
	fn a_mail_burst_is_far_above_the_usual() {
		assert!(!burst(299, 0));
		assert!(burst(300, 0));
		assert!(burst(600, 100));
		assert!(!burst(500, 100));
	}
}
