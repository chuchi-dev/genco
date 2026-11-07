//! Mail per address, to see which mailboxes, forwards and catch-alls are
//! used: every minute the new lines of Postfix in the journal are counted
//! per day and address. For the receiving mail domains how a mail ended
//! (delivered to a mailbox, forwarded, rejected, bounced), for the SMTP
//! logins what they sent. Only counts and the time of the last mail, in
//! the stats database for 90 days; never senders, subjects or message
//! ids.

use std::collections::{HashMap, HashSet};

use crate::stats::StatsDb;

use super::*;

const DAY: i64 = 24 * 3600 * 1000;
/// how long the counts per day stay
const KEEP_DAYS: i64 = 90 * DAY;
/// the last mail of an address stays longer, "never" would be wrong;
/// the last rejected one only as long as the counts (spammers try
/// thousands of addresses)
const KEEP_LAST: i64 = 400 * DAY;
/// what the panel and `genco mail stats` sum up
const SHOWN_DAYS: i64 = 30;
const UNKNOWN_SHOWN: usize = 10;
/// rejected addresses a domain keeps per day, the rest of the spammers'
/// tries count as `(others)@domain`
const REJECTED_PER_DAY: i64 = 200;
const OTHERS: &str = "(others)";
/// the domain of an address, a login without `@` is itself; the same
/// expression as in the indexes, so the panel doesn't scan all addresses
const DOMAIN: &str = "substr(address, instr(address, '@') + 1)";

pub const SCHEMA: &str = "
	CREATE TABLE IF NOT EXISTS mail_counts (
		day INTEGER NOT NULL,
		address TEXT NOT NULL,
		kind TEXT NOT NULL,
		count INTEGER NOT NULL,
		PRIMARY KEY (day, address, kind)
	);
	CREATE TABLE IF NOT EXISTS mail_last (
		address TEXT NOT NULL,
		kind TEXT NOT NULL,
		at INTEGER NOT NULL,
		PRIMARY KEY (address, kind)
	);
	CREATE INDEX IF NOT EXISTS mail_counts_domain
		ON mail_counts (substr(address, instr(address, '@') + 1), day);
	CREATE INDEX IF NOT EXISTS mail_last_domain
		ON mail_last (substr(address, instr(address, '@') + 1));
	CREATE TABLE IF NOT EXISTS mail_state (
		key TEXT PRIMARY KEY,
		value TEXT NOT NULL
	);
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Kind {
	/// delivered to a mailbox here
	Received,
	/// sent on to another server
	Forwarded,
	/// refused while it came in: an unknown address, spam
	Rejected,
	Bounced,
	/// sent with an SMTP login
	Sent,
}

impl Kind {
	fn as_str(self) -> &'static str {
		match self {
			Kind::Received => "received",
			Kind::Forwarded => "forwarded",
			Kind::Rejected => "rejected",
			Kind::Bounced => "bounced",
			Kind::Sent => "sent",
		}
	}
}

/// What a line of Postfix tells about a mail.
#[derive(Debug, PartialEq)]
enum Line<'a> {
	/// `to=` of a delivery agent, `address` is the original recipient
	/// (before forwards and the catch-all)
	Delivery {
		queue: &'a str,
		address: &'a str,
		kind: Kind,
	},
	/// refused for good (5xx) at the SMTP dialog or by the spam filter
	Rejected { address: &'a str },
	/// a mail that came in with an SMTP login
	Login { user: &'a str },
	/// the mail left the queue
	Removed { queue: &'a str },
}

/// `name` (`to=<`) of the fields separated by spaces or commas, without
/// the `<>`.
fn field<'a>(s: &'a str, name: &str) -> Option<&'a str> {
	s.split([' ', ','])
		.find_map(|t| t.strip_prefix(name))
		.map(|v| v.trim_end_matches('>'))
}

/// A short queue id is hex, a long one has no vowels: `warning` or
/// `NOQUEUE` at the start of a line are none.
fn queue_id(msg: &str) -> Option<(&str, &str)> {
	let (id, rest) = msg.split_once(": ")?;
	let hex = id.chars().all(|c| matches!(c, '0'..='9' | 'A'..='F'));
	let long = id.chars().all(|c| c.is_ascii_alphanumeric())
		&& !id.contains(['a', 'e', 'i', 'o', 'u', 'A', 'E', 'I', 'O', 'U']);
	(id.len() >= 6 && (hex || long)).then_some((id, rest))
}

/// The code after the client (`... from host[1.2.3.4]: 550 5.1.1 ...`)
/// says whether the client may try again.
fn permanent(reject: &str) -> bool {
	reject
		.split_once("]: ")
		.is_some_and(|(_, code)| code.starts_with('5'))
}

fn rejected(reject: &str) -> Option<Line<'_>> {
	if !permanent(reject) {
		return None;
	}
	Some(Line::Rejected {
		address: field(reject, "to=<")?,
	})
}

/// `ident` is the program (`postfix/lmtp`, `postfix/submission/smtpd`).
fn parse<'a>(ident: &str, msg: &'a str) -> Option<Line<'a>> {
	let program = ident.rsplit('/').next()?;

	if let Some(reject) = msg.strip_prefix("NOQUEUE: reject: ") {
		return rejected(reject);
	}

	let (queue, rest) = queue_id(msg)?;
	if let Some(reject) = rest.strip_prefix("milter-reject: ") {
		return rejected(reject);
	}
	if rest == "removed" {
		return Some(Line::Removed { queue });
	}
	if program == "smtpd" && rest.starts_with("client=") {
		return Some(Line::Login {
			user: field(rest, "sasl_username=")?,
		});
	}

	// `to=<a>, orig_to=<b>, relay=..., status=sent (...)`
	let to = rest.strip_prefix("to=<")?.split_once('>')?.0;
	let address = field(rest, "orig_to=<").unwrap_or(to);
	let kind = match field(rest, "status=")? {
		"sent" if matches!(program, "lmtp" | "local" | "virtual") => {
			Kind::Received
		}
		"sent" => Kind::Forwarded,
		"bounced" => Kind::Bounced,
		// tried again later, counted when it ends
		_ => return None,
	};
	Some(Line::Delivery {
		queue,
		address,
		kind,
	})
}

/// One mail counted for an address.
#[derive(Debug, PartialEq)]
struct Count {
	/// milliseconds, of the log line
	at: i64,
	address: String,
	kind: Kind,
}

/// What the collector remembers between two minutes.
#[derive(Default)]
pub struct Counter {
	/// per mail in the queue the addresses already counted: a forward
	/// to two targets is one mail
	counted: HashMap<String, HashSet<(String, Kind)>>,
}

impl Counter {
	/// `lines`: time, program and message. Mail is counted for the
	/// addresses of `domains` (the receiving ones), logins always.
	fn count(
		&mut self,
		lines: &[(i64, String, String)],
		domains: &HashSet<&str>,
	) -> Vec<Count> {
		let ours = |address: &str| {
			address
				.rsplit_once('@')
				.is_some_and(|(_, d)| domains.contains(d))
		};
		let mut counts = vec![];
		for (at, ident, msg) in lines {
			let (address, kind) = match parse(ident, msg) {
				Some(Line::Delivery {
					queue,
					address,
					kind,
				}) => {
					let address = address.to_lowercase();
					let new = self
						.counted
						.entry(queue.to_string())
						.or_default()
						.insert((address.clone(), kind));
					if !new || !ours(&address) {
						continue;
					}
					(address, kind)
				}
				Some(Line::Rejected { address }) => {
					let address = address.to_lowercase();
					if !ours(&address) {
						continue;
					}
					(address, Kind::Rejected)
				}
				Some(Line::Login { user }) => (user.to_lowercase(), Kind::Sent),
				Some(Line::Removed { queue }) => {
					self.counted.remove(queue);
					continue;
				}
				None => continue,
			};
			counts.push(Count {
				at: *at,
				address,
				kind,
			});
		}
		// a `removed` genco didn't see (a restart) must not keep them
		if self.counted.len() > 10_000 {
			self.counted.clear();
		}
		counts
	}

	/// Counts the lines since the last round. The first round on a
	/// server starts now, the lines before aren't counted.
	pub async fn round(&mut self, groups: &[Group], stats: &StatsDb) {
		let (cursor, from) = {
			let conn = stats.conn();
			if state(&conn, "since").is_none() {
				let t = now().to_string();
				let _ = set_state(&conn, "since", &t);
				let _ = set_state(&conn, "from", &t);
			}
			(state(&conn, "cursor"), state(&conn, "from"))
		};

		let mut args = vec![
			"journalctl",
			"-q",
			"--no-pager",
			"-o",
			"json",
			"--output-fields=MESSAGE,SYSLOG_IDENTIFIER",
		];
		args.extend(crate::mail::POSTFIX_UNITS);
		let after;
		let since;
		match &cursor {
			Some(c) => {
				after = format!("--after-cursor={c}");
				args.push(&after);
			}
			None => {
				let from: i64 = from.and_then(|f| f.parse().ok()).unwrap_or(0);
				since = format!("--since=@{}", from / 1000);
				args.push(&since);
			}
		}
		let out = match crate::util::run(&args).await {
			Ok(out) => out,
			// journalctl refuses the cursor: on from now
			Err(e) if bad_cursor(&e, &args) => {
				tracing::warn!("mail counts: {e}, counting on from now");
				let conn = stats.conn();
				let _ = conn
					.execute("DELETE FROM mail_state WHERE key = 'cursor'", []);
				let _ = set_state(&conn, "from", &now().to_string());
				return;
			}
			// anything else: the same lines again next minute
			Err(e) => {
				tracing::warn!("mail counts: {e}");
				return;
			}
		};
		let (lines, last) = journal_lines(&out);

		let domains: HashSet<&str> = domains(groups)
			.filter(|d| !d.send_only)
			.map(|d| d.name.as_str())
			.collect();
		let counts = self.count(&lines, &domains);

		let conn = stats.conn();
		let res = store(&conn, &counts, last.as_deref())
			.and_then(|()| prune(&conn, now()));
		if let Err(e) = res {
			tracing::error!("mail counts: {e}");
		}
	}
}

/// Whether journalctl failed on the cursor itself (`Failed to seek to
/// cursor: Invalid argument`); only its message after the command line
/// counts, which holds the cursor too. A well-formed cursor of vacuumed
/// lines isn't an error, journalctl starts at the oldest line left.
fn bad_cursor(err: &str, args: &[&str]) -> bool {
	let prefix = format!("{} failed: ", args.join(" "));
	err.strip_prefix(&prefix)
		.is_some_and(|msg| msg.to_lowercase().contains("cursor"))
}

/// Time, program and message of journalctl's JSON lines, and the cursor
/// of the last one.
fn journal_lines(out: &str) -> (Vec<(i64, String, String)>, Option<String>) {
	let mut lines = vec![];
	let mut last = None;
	for l in out.lines() {
		let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else {
			continue;
		};
		if let Some(c) = v["__CURSOR"].as_str() {
			last = Some(c.to_string());
		}
		// a message that isn't UTF-8 is an array of bytes, none of Postfix
		let (Some(ident), Some(msg)) =
			(v["SYSLOG_IDENTIFIER"].as_str(), v["MESSAGE"].as_str())
		else {
			continue;
		};
		let at = v["__REALTIME_TIMESTAMP"]
			.as_str()
			.and_then(|t| t.parse::<i64>().ok())
			.map_or_else(now, |us| us / 1000);
		lines.push((at, ident.to_string(), msg.to_string()));
	}
	(lines, last)
}

fn state(conn: &Connection, key: &str) -> Option<String> {
	conn.query_row("SELECT value FROM mail_state WHERE key = ?1", [key], |r| {
		r.get(0)
	})
	.ok()
}

fn set_state(
	conn: &Connection,
	key: &str,
	value: &str,
) -> rusqlite::Result<()> {
	conn.execute(
		"INSERT INTO mail_state (key, value) VALUES (?1, ?2)
		ON CONFLICT (key) DO UPDATE SET value = excluded.value",
		params![key, value],
	)?;
	Ok(())
}

/// Adds the counts to their days and moves the cursor on, together: a
/// crash counts nothing twice.
fn store(
	conn: &Connection,
	counts: &[Count],
	cursor: Option<&str>,
) -> rusqlite::Result<()> {
	let tx = conn.unchecked_transaction()?;
	for c in counts {
		let day = c.at / DAY * DAY;
		let address = match c.kind {
			Kind::Rejected => rejected_address(&tx, day, &c.address)?,
			_ => c.address.clone(),
		};
		tx.execute(
			"INSERT INTO mail_counts (day, address, kind, count)
			VALUES (?1, ?2, ?3, 1)
			ON CONFLICT (day, address, kind) DO UPDATE SET count = count + 1",
			params![day, address, c.kind.as_str()],
		)?;
		tx.execute(
			"INSERT INTO mail_last (address, kind, at) VALUES (?1, ?2, ?3)
			ON CONFLICT (address, kind) DO UPDATE SET at = max(at, excluded.at)",
			params![address, c.kind.as_str(), c.at],
		)?;
	}
	if let Some(c) = cursor {
		set_state(&tx, "cursor", c)?;
	}
	tx.commit()
}

/// The address a rejected mail counts for: its own, or `(others)@domain`
/// once the domain has `REJECTED_PER_DAY` others that day.
fn rejected_address(
	conn: &Connection,
	day: i64,
	address: &str,
) -> rusqlite::Result<String> {
	let Some((_, domain)) = address.rsplit_once('@') else {
		return Ok(address.into());
	};
	let others = format!("{OTHERS}@{domain}");
	let known: bool = conn.query_row(
		"SELECT EXISTS (SELECT 1 FROM mail_counts WHERE day = ?1
		AND address = ?2 AND kind = 'rejected')",
		params![day, address],
		|r| r.get(0),
	)?;
	if known {
		return Ok(address.into());
	}
	let n: i64 = conn.query_row(
		&format!(
			"SELECT count(*) FROM mail_counts WHERE {DOMAIN} = ?1
			AND day = ?2 AND kind = 'rejected' AND address != ?3"
		),
		params![domain, day, others],
		|r| r.get(0),
	)?;
	Ok(if n < REJECTED_PER_DAY {
		address.into()
	} else {
		others
	})
}

fn prune(conn: &Connection, at: i64) -> rusqlite::Result<()> {
	conn.execute(
		"DELETE FROM mail_counts WHERE day < ?1",
		[at / DAY * DAY - KEEP_DAYS],
	)?;
	conn.execute(
		"DELETE FROM mail_last WHERE at < ?1
		OR (kind = 'rejected' AND at < ?2)",
		params![at - KEEP_LAST, at - KEEP_DAYS],
	)?;
	Ok(())
}

/// The mail of an address in the last 30 days.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressCount {
	/// `*@domain` the catch-all, `g-<group>` the group's login
	pub address: String,
	/// received and forwarded; for a send-only address and the login
	/// sent; for an unknown address rejected
	pub count: i64,
	/// milliseconds, None if none since counting started
	pub last: Option<i64>,
}

#[derive(Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainCounts {
	/// when counting started, None before the first round
	pub since: Option<i64>,
	pub days: i64,
	pub received: i64,
	pub forwarded: i64,
	pub rejected: i64,
	pub bounced: i64,
	pub mailboxes: Vec<AddressCount>,
	/// what the mailboxes sent with their login, in the same order
	pub mailboxes_sent: Vec<AddressCount>,
	/// with the catch-all: the addresses without a mailbox or forward
	pub forwards: Vec<AddressCount>,
	pub send_only: Vec<AddressCount>,
	/// what the group's SMTP login sent, as any of its addresses
	pub login: Option<AddressCount>,
	/// refused addresses without a mailbox or forward, the most first;
	/// `(others)@domain` sums up those beyond the daily cap
	pub unknown: Vec<AddressCount>,
}

/// Per address and kind: count in the last days and the last mail.
type Totals = HashMap<String, HashMap<String, (i64, Option<i64>)>>;

fn totals(
	conn: &Connection,
	domain: &str,
	login: &str,
	at: i64,
) -> rusqlite::Result<Totals> {
	let since = at / DAY * DAY - (SHOWN_DAYS - 1) * DAY;
	let mut totals: Totals = HashMap::new();

	// the login has no `@`, its "domain" is the login itself
	let mut stmt = conn.prepare(&format!(
		"SELECT address, kind, sum(count) FROM mail_counts
		WHERE {DOMAIN} IN (?2, ?3) AND day >= ?1
		GROUP BY address, kind"
	))?;
	let rows = stmt.query_map(params![since, domain, login], |r| {
		Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?))
	})?;
	for row in rows {
		let (address, kind, count) = row?;
		totals
			.entry(address)
			.or_default()
			.entry(kind)
			.or_default()
			.0 = count;
	}

	let mut stmt = conn.prepare(&format!(
		"SELECT address, kind, at FROM mail_last WHERE {DOMAIN} IN (?1, ?2)"
	))?;
	let rows = stmt.query_map(params![domain, login], |r| {
		Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?))
	})?;
	for row in rows {
		let (address, kind, at) = row?;
		totals
			.entry(address)
			.or_default()
			.entry(kind)
			.or_default()
			.1 = Some(at);
	}
	Ok(totals)
}

/// The count and last mail of `kinds` summed up over `addresses`.
fn sum<'a>(
	totals: &Totals,
	address: &str,
	addresses: impl Iterator<Item = &'a str>,
	kinds: &[Kind],
) -> AddressCount {
	let mut c = AddressCount {
		address: address.to_string(),
		..Default::default()
	};
	for a in addresses {
		for k in kinds {
			if let Some((count, last)) =
				totals.get(a).and_then(|t| t.get(k.as_str()))
			{
				c.count += count;
				c.last = c.last.max(*last);
			}
		}
	}
	c
}

/// The counts of a mail domain's addresses, the group's login and the
/// unknown addresses tried.
pub fn domain_counts(
	conn: &Connection,
	domain: &MailDomain,
	login: Option<&str>,
	at: i64,
) -> rusqlite::Result<DomainCounts> {
	let totals = totals(conn, &domain.name, login.unwrap_or(""), at)?;
	let incoming = [Kind::Received, Kind::Forwarded];
	let one = |address: &str, kinds: &[Kind]| {
		sum(&totals, address, std::iter::once(address), kinds)
	};

	let known: HashSet<&str> = domain
		.mailboxes
		.iter()
		.map(|b| b.address.as_str())
		.chain(domain.forwards.iter().map(|f| f.address.as_str()))
		.collect();
	// the rest of the domain's mail went to the catch-all
	let at_domain = format!("@{}", domain.name);
	let others = || {
		totals
			.keys()
			.map(|a| a.as_str())
			.filter(|a| !known.contains(a) && a.ends_with(&at_domain))
	};
	let catch_all = format!("*@{}", domain.name);

	let mut unknown: Vec<AddressCount> = if known.contains(catch_all.as_str()) {
		vec![]
	} else {
		others()
			.map(|a| one(a, &[Kind::Rejected]))
			.filter(|c| c.count > 0)
			.collect()
	};
	unknown.sort_by(|a, b| {
		b.count
			.cmp(&a.count)
			.then_with(|| a.address.cmp(&b.address))
	});
	unknown.truncate(UNKNOWN_SHOWN);

	let domain_sum = |kind: Kind| -> i64 {
		totals
			.iter()
			.filter(|(a, _)| a.ends_with(&at_domain))
			.filter_map(|(_, t)| t.get(kind.as_str()).map(|c| c.0))
			.sum()
	};

	Ok(DomainCounts {
		since: state(conn, "since").and_then(|s| s.parse().ok()),
		days: SHOWN_DAYS,
		received: domain_sum(Kind::Received),
		forwarded: domain_sum(Kind::Forwarded),
		rejected: domain_sum(Kind::Rejected),
		bounced: domain_sum(Kind::Bounced),
		mailboxes: domain
			.mailboxes
			.iter()
			.map(|b| one(&b.address, &incoming))
			.collect(),
		mailboxes_sent: domain
			.mailboxes
			.iter()
			.map(|b| one(&b.address, &[Kind::Sent]))
			.collect(),
		forwards: domain
			.forwards
			.iter()
			.map(|f| {
				if f.address == catch_all {
					sum(&totals, &f.address, others(), &incoming)
				} else {
					one(&f.address, &incoming)
				}
			})
			.collect(),
		send_only: domain
			.send_only_addresses
			.iter()
			.map(|s| one(&s.address, &[Kind::Sent]))
			.collect(),
		login: login.map(|l| one(l, &[Kind::Sent])),
		unknown,
	})
}

/// The counts of `domain` with its group's SMTP login.
pub(super) fn counts(
	stats: &StatsDb,
	groups: &[Group],
	domain: &MailDomain,
) -> Result<DomainCounts> {
	let login = groups
		.iter()
		.find(|g| g.id == domain.group_id && g.smtp.is_some())
		.map(|g| smtp_user(&g.name));
	Ok(domain_counts(
		&stats.conn(),
		domain,
		login.as_deref(),
		now(),
	)?)
}

/// `genco mail stats [<domain>]`
pub(super) async fn stats_text(
	stats: &StatsDb,
	groups: &[Group],
	domain: Option<&str>,
) -> Result<String> {
	let list = match domain {
		Some(name) => vec![cli::find_domain(groups, name)?],
		None => domains(groups).cloned().collect(),
	};
	let mut out = String::new();
	for d in &list {
		let c = counts(stats, groups, d)?;
		let since = match c.since {
			Some(s) => crate::util::format_time(s).await,
			None => "not yet".into(),
		};
		out.push_str(&format!(
			"{} in the last {} days (counted since {since}): {} received, \
			{} forwarded, {} rejected, {} bounced\n",
			d.name, c.days, c.received, c.forwarded, c.rejected, c.bounced
		));
		let rows = [
			("mailbox", "mails", &c.mailboxes),
			("mailbox", "sent", &c.mailboxes_sent),
			("forward", "mails", &c.forwards),
			("send-only", "sent", &c.send_only),
			("login", "sent", &c.login.iter().cloned().collect()),
			("unknown", "rejected", &c.unknown),
		];
		for (what, unit, list) in rows {
			for a in list {
				let last = match a.last {
					Some(t) => crate::util::format_time(t).await,
					None => "never".into(),
				};
				out.push_str(&format!(
					"  {:<10} {:<36} {:>6} {unit:<8}  last {last}\n",
					what, a.address, a.count
				));
			}
		}
	}
	if out.is_empty() {
		out = "no mail domains".into();
	}
	Ok(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Postfix 3.10 with Dovecot's LMTP, postsrsd and Rspamd, from a
	/// Debian 13 test server: shop.ch and other.ch receive, info@ goes to
	/// two other servers, team@ to anna@, gone@ bounces, other.ch has a
	/// catch-all to anna@; noreply@ and g-shop send with a login.
	const LOG: &str = "\
postfix/smtpd[6854]: DDD1611A4F5B: client=localhost[127.0.0.1]
postfix/cleanup[6858]: DDD1611A4F5B: message-id=<20261104035246.006853@25c712f0215b>
postfix/qmgr[6829]: DDD1611A4F5B: from=<SRS0=daFd=IZ=sender.example.org=someone@mail.panel.test>, size=487, nrcpt=1 (queue active)
postfix/lmtp[6861]: DDD1611A4F5B: to=<anna@shop.ch>, relay=mail.panel.test[private/dovecot-lmtp], delay=1.2, delays=1.2/0.01/0.01/0.01, dsn=2.0.0, status=sent (250 2.0.0 <anna@shop.ch> qkiWBBCs6mrPGgAA8oM8Ig Saved)
postfix/qmgr[6829]: DDD1611A4F5B: removed
postfix/smtp[6866]: 1BECD11A4F5B: to=<x@ext.test>, orig_to=<info@shop.ch>, relay=none, delay=0.06, delays=0.05/0.01/0/0, dsn=4.4.1, status=deferred (connect to 127.0.0.1[127.0.0.1]:2525: Connection refused)
postfix/smtp[6866]: 1BECD11A4F5B: to=<x@ext.test>, orig_to=<info@shop.ch>, relay=127.0.0.1[127.0.0.1]:2525, delay=36, delays=36/0/0/0, dsn=2.0.0, status=sent (250 2.0.0 Ok)
postfix/smtp[6866]: 1BECD11A4F5B: to=<y@ext2.test>, orig_to=<info@shop.ch>, relay=127.0.0.1[127.0.0.1]:2525, delay=36, delays=36/0/0/0, dsn=2.0.0, status=sent (250 2.0.0 Ok)
postfix/qmgr[6829]: 1BECD11A4F5B: removed
postfix/lmtp[6861]: 36A3511A4FD5: to=<anna@shop.ch>, orig_to=<team@shop.ch>, relay=mail.panel.test[private/dovecot-lmtp], delay=0.06, delays=0.06/0/0/0, dsn=2.0.0, status=sent (250 2.0.0 <anna@shop.ch> oXmyEBCs6mrPGgAA8oM8Ig Saved)
postfix/qmgr[6829]: 36A3511A4FD5: removed
postfix/lmtp[6861]: 5071111A4FD5: to=<anna@shop.ch>, orig_to=<random@other.ch>, relay=mail.panel.test[private/dovecot-lmtp], delay=0.07, delays=0.06/0/0/0, dsn=2.0.0, status=sent (250 2.0.0 <anna@shop.ch> iQ9HFxCs6mrPGgAA8oM8Ig Saved)
postfix/qmgr[6829]: 5071111A4FD5: removed
postfix/smtpd[6854]: NOQUEUE: reject: RCPT from localhost[127.0.0.1]: 550 5.1.1 <nobody@shop.ch>: Recipient address rejected: User unknown in virtual mailbox table; from=<someone@sender.example.org> to=<nobody@shop.ch> proto=ESMTP helo=<client.example.org>
postfix/smtpd[6854]: NOQUEUE: reject: RCPT from localhost[127.0.0.1]: 450 4.7.1 <nobody@shop.ch>: Recipient address rejected: try again; from=<someone@sender.example.org> to=<nobody@shop.ch> proto=ESMTP helo=<client.example.org>
postfix/smtpd[6854]: NOQUEUE: reject: RCPT from localhost[127.0.0.1]: 454 4.7.1 <x@notours.test>: Relay access denied; from=<someone@sender.example.org> to=<x@notours.test> proto=ESMTP helo=<client.example.org>
postfix/smtp[6866]: 76B3D11A4FD5: to=<nobody@bounce.test>, orig_to=<gone@shop.ch>, relay=127.0.0.1[127.0.0.1]:2526, delay=35, delays=35/0/0/0.01, dsn=5.3.0, status=bounced (host 127.0.0.1[127.0.0.1] said: 500 5.3.0 Error: command failed (in reply to RCPT TO command))
postfix/bounce[6874]: 76B3D11A4FD5: sender non-delivery notification: E7FF811A4FD3
postfix/smtp[6920]: E7FF811A4FD3: to=<someone@sender.example.org>, orig_to=<SRS0=daFd=IZ=sender.example.org=someone@mail.panel.test>, relay=none, delay=0.01, delays=0/0/0/0, dsn=5.4.4, status=bounced (Host or domain name not found. Name service error for name=sender.example.org type=AAAA: Host found but no data record of requested type)
postfix/cleanup[6858]: AB6DB11A4FDB: milter-reject: END-OF-MESSAGE from localhost[127.0.0.1]: 5.7.1 Gtube pattern; from=<SRS0=daFd=IZ=sender.example.org=someone@mail.panel.test> to=<anna@shop.ch> proto=ESMTP helo=<client.example.org>
postfix/cleanup[6858]: AB6DB11A4FDB: removed (canceled)
postfix/submission/smtpd[6877]: DE3C111A4FE1: client=localhost[127.0.0.1], sasl_method=PLAIN, sasl_username=noreply@shop.ch
postfix/smtp[6920]: DE3C111A4FE1: to=<x@ext.test>, relay=127.0.0.1[127.0.0.1]:2525, delay=35, delays=35/0/0/0, dsn=2.0.0, status=sent (250 2.0.0 Ok)
postfix/submission/smtpd[6877]: 0C0C211A4FE5: client=localhost[127.0.0.1], sasl_method=LOGIN, sasl_username=g-shop
postfix/submission/smtpd[6877]: warning: localhost[127.0.0.1]: SASL PLAIN authentication failed: (reason unavailable), sasl_username=noreply@shop.ch
postfix/pickup[6828]: 2AA3111A4FEA: uid=0 from=<root>
postfix/lmtp[6861]: 2AA3111A4FEA: to=<Anna@shop.ch>, relay=mail.panel.test[private/dovecot-lmtp], delay=0.03, delays=0.03/0/0/0, dsn=2.0.0, status=sent (250 2.0.0 <anna@shop.ch> iTeyCxOs6mrPGgAA8oM8Ig Saved)
";

	fn lines(log: &str) -> Vec<(i64, String, String)> {
		log.lines()
			.enumerate()
			.map(|(i, l)| {
				let (ident, msg) = l.split_once(": ").unwrap();
				let ident = ident.split('[').next().unwrap();
				(i as i64 * 1000, ident.to_string(), msg.to_string())
			})
			.collect()
	}

	fn counted(log: &str) -> Vec<(String, Kind)> {
		let domains = HashSet::from(["shop.ch", "other.ch"]);
		Counter::default()
			.count(&lines(log), &domains)
			.into_iter()
			.map(|c| (c.address, c.kind))
			.collect()
	}

	fn c(address: &str, kind: Kind) -> (String, Kind) {
		(address.to_string(), kind)
	}

	#[test]
	fn postfix_lines_count_per_original_address() {
		assert_eq!(
			counted(LOG),
			vec![
				c("anna@shop.ch", Kind::Received),
				// two targets, one mail; the deferred try isn't counted
				c("info@shop.ch", Kind::Forwarded),
				c("team@shop.ch", Kind::Received),
				c("random@other.ch", Kind::Received),
				// only for good, and only for our domains
				c("nobody@shop.ch", Kind::Rejected),
				c("gone@shop.ch", Kind::Bounced),
				// Rspamd's milter
				c("anna@shop.ch", Kind::Rejected),
				c("noreply@shop.ch", Kind::Sent),
				c("g-shop", Kind::Sent),
				c("anna@shop.ch", Kind::Received),
			]
		);
	}

	#[test]
	fn queue_ids_short_and_long() {
		assert_eq!(
			queue_id("DDD1611A4F5B: removed").unwrap().0,
			"DDD1611A4F5B"
		);
		assert_eq!(
			queue_id("4Tz2Rk1Fc5zJ8x: removed").unwrap().0,
			"4Tz2Rk1Fc5zJ8x"
		);
		assert!(queue_id("warning: localhost[127.0.0.1]: x").is_none());
		assert!(queue_id("NOQUEUE: reject: x").is_none());
	}

	#[test]
	fn a_mail_in_the_queue_counts_once_until_removed() {
		let mut counter = Counter::default();
		let domains = HashSet::from(["shop.ch"]);
		let line = "postfix/smtp[1]: 1BECD11A4F5B: to=<x@ext.test>, orig_to=<info@shop.ch>, relay=a[1.2.3.4]:25, status=sent (250 Ok)";
		assert_eq!(counter.count(&lines(line), &domains).len(), 1);
		assert_eq!(counter.count(&lines(line), &domains).len(), 0);
		counter
			.count(&lines("postfix/qmgr[1]: 1BECD11A4F5B: removed"), &domains);
		assert_eq!(counter.count(&lines(line), &domains).len(), 1);
	}

	#[test]
	fn journal_json_lines_and_the_last_cursor() {
		let out = r#"{"SYSLOG_IDENTIFIER":"postfix/qmgr","__REALTIME_TIMESTAMP":"1793764694241853","__CURSOR":"s=a;i=1","MESSAGE":"DDD1611A4F5B: removed"}
{"SYSLOG_IDENTIFIER":"postfix/smtpd","__REALTIME_TIMESTAMP":"1793764694241999","__CURSOR":"s=a;i=2","MESSAGE":[255,0]}
"#;
		let (lines, last) = journal_lines(out);
		assert_eq!(
			lines,
			vec![(
				1793764694241,
				"postfix/qmgr".to_string(),
				"DDD1611A4F5B: removed".to_string()
			)]
		);
		assert_eq!(last.as_deref(), Some("s=a;i=2"));
	}

	fn count(at: i64, address: &str, kind: Kind) -> Count {
		Count {
			at,
			address: address.into(),
			kind,
		}
	}

	#[test]
	fn counts_add_up_per_day_and_the_cursor_moves_with_them() {
		let stats = StatsDb::memory();
		let conn = stats.conn();
		let day = 20_000 * DAY;
		store(
			&conn,
			&[
				count(day + 1000, "anna@shop.ch", Kind::Received),
				count(day + 5000, "anna@shop.ch", Kind::Received),
				count(day + DAY, "anna@shop.ch", Kind::Received),
			],
			Some("s=a;i=1"),
		)
		.unwrap();
		store(&conn, &[], None).unwrap();

		let days: Vec<(i64, i64)> = conn
			.prepare("SELECT day, count FROM mail_counts ORDER BY day")
			.unwrap()
			.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
			.unwrap()
			.collect::<rusqlite::Result<_>>()
			.unwrap();
		assert_eq!(days, vec![(day, 2), (day + DAY, 1)]);
		let last: i64 = conn
			.query_row("SELECT at FROM mail_last", [], |r| r.get(0))
			.unwrap();
		assert_eq!(last, day + DAY);
		assert_eq!(state(&conn, "cursor").as_deref(), Some("s=a;i=1"));
	}

	#[test]
	fn old_counts_are_pruned() {
		let stats = StatsDb::memory();
		let conn = stats.conn();
		let today = 20_000 * DAY;
		store(
			&conn,
			&[
				count(today - 100 * DAY, "old@shop.ch", Kind::Received),
				count(today - 100 * DAY, "tried@shop.ch", Kind::Rejected),
				count(today - 10 * DAY, "anna@shop.ch", Kind::Received),
			],
			None,
		)
		.unwrap();
		prune(&conn, today).unwrap();

		let n: i64 = conn
			.query_row("SELECT count(*) FROM mail_counts", [], |r| r.get(0))
			.unwrap();
		assert_eq!(n, 1);
		// the last mail stays longer, a rejected one doesn't
		let last: Vec<String> = conn
			.prepare("SELECT address FROM mail_last ORDER BY address")
			.unwrap()
			.query_map([], |r| r.get(0))
			.unwrap()
			.collect::<rusqlite::Result<_>>()
			.unwrap();
		assert_eq!(last, ["anna@shop.ch", "old@shop.ch"]);
	}

	fn domain() -> MailDomain {
		let mailbox = Mailbox {
			id: 1,
			mail_domain_id: 1,
			address: "anna@shop.ch".into(),
			quota_mb: 100,
			password_hash: String::new(),
		};
		let forward = |id, address: &str| Forward {
			id,
			mail_domain_id: 1,
			address: address.into(),
			targets: vec!["x@ext.test".into()],
		};
		MailDomain {
			id: 1,
			group_id: 1,
			name: "shop.ch".into(),
			send_only: false,
			mailboxes: vec![mailbox],
			forwards: vec![
				forward(1, "info@shop.ch"),
				forward(2, "old@shop.ch"),
			],
			send_only_addresses: vec![],
		}
	}

	#[test]
	fn the_panel_sums_up_the_last_30_days() {
		let stats = StatsDb::memory();
		let conn = stats.conn();
		set_state(&conn, "since", "5").unwrap();
		let today = 20_000 * DAY;
		let lines = lines(LOG);
		let mut counts =
			Counter::default().count(&lines, &HashSet::from(["shop.ch"]));
		for c in &mut counts {
			c.at += today;
		}
		counts.push(count(today - 40 * DAY, "old@shop.ch", Kind::Forwarded));
		counts.push(count(today, "anna@shop.ch", Kind::Sent));
		store(&conn, &counts, None).unwrap();

		let mut d = domain();
		let c = domain_counts(&conn, &d, Some("g-shop"), today).unwrap();
		assert_eq!(c.since, Some(5));
		assert_eq!(
			(c.received, c.forwarded, c.rejected, c.bounced),
			(3, 1, 2, 1)
		);
		assert_eq!(c.mailboxes[0].count, 2);
		// what the mailbox sent with its login
		assert_eq!(c.mailboxes_sent[0].count, 1);
		assert_eq!(c.mailboxes_sent[0].address, "anna@shop.ch");
		assert_eq!(c.forwards[0].count, 1);
		// a month ago is too long, but its last mail stays
		assert_eq!(c.forwards[1].count, 0);
		assert!(c.forwards[1].last.is_some());
		assert_eq!(c.login.unwrap().count, 1);
		let unknown: Vec<_> =
			c.unknown.iter().map(|u| u.address.as_str()).collect();
		// gone@ bounced, nobody@ was refused
		assert_eq!(unknown, ["nobody@shop.ch"]);

		// a catch-all took team@ and gone@, and nothing is unknown
		d.forwards.push(Forward {
			id: 3,
			mail_domain_id: 1,
			address: "*@shop.ch".into(),
			targets: vec!["anna@shop.ch".into()],
		});
		let c = domain_counts(&conn, &d, None, today).unwrap();
		assert_eq!(c.forwards[2].count, 1);
		assert!(c.unknown.is_empty());
		assert!(c.login.is_none());
	}

	#[test]
	fn only_a_refused_cursor_starts_over() {
		let args = ["journalctl", "--after-cursor=s=a;i=1"];
		let failed = |msg: &str| format!("{} failed: {msg}", args.join(" "));
		assert!(bad_cursor(
			&failed("Failed to seek to cursor: Invalid argument"),
			&args
		));
		// the cursor in the command line doesn't count
		assert!(!bad_cursor(
			&failed("Failed to open journal: Too many open files"),
			&args
		));
		assert!(!bad_cursor("journalctl: No such file or directory", &args));
	}

	#[test]
	fn rejected_addresses_are_capped_per_domain_and_day() {
		let stats = StatsDb::memory();
		let conn = stats.conn();
		let day = 20_000 * DAY;
		let mut counts: Vec<Count> = (0..REJECTED_PER_DAY + 5)
			.map(|i| count(day, &format!("spam{i}@shop.ch"), Kind::Rejected))
			.collect();
		// one kept before still counts as itself, other domains apart
		counts.push(count(day, "spam0@shop.ch", Kind::Rejected));
		counts.push(count(day, "x@other.ch", Kind::Rejected));
		store(&conn, &counts, None).unwrap();

		let n = |sql: &str| -> i64 {
			conn.query_row(sql, [], |r| r.get(0)).unwrap()
		};
		assert_eq!(
			n(
				"SELECT count(*) FROM mail_counts WHERE address LIKE '%@shop.ch'"
			),
			REJECTED_PER_DAY + 1
		);
		assert_eq!(
			n("SELECT count FROM mail_counts WHERE address = 'spam0@shop.ch'"),
			2
		);
		assert_eq!(
			n("SELECT count FROM mail_counts
			WHERE address = '(others)@shop.ch'"),
			5
		);
		assert_eq!(n("SELECT count(*) FROM mail_last"), REJECTED_PER_DAY + 2);

		// the next day starts afresh, the totals have them all
		store(
			&conn,
			&[count(day + DAY, "new@shop.ch", Kind::Rejected)],
			None,
		)
		.unwrap();
		assert_eq!(
			n("SELECT count(*) FROM mail_counts WHERE address = 'new@shop.ch'"),
			1
		);
		drop(conn);
		let c =
			domain_counts(&stats.conn(), &domain(), None, day + DAY).unwrap();
		assert_eq!(c.rejected, REJECTED_PER_DAY + 7);
		assert_eq!(c.unknown[0].address, "(others)@shop.ch");
	}

	#[test]
	fn the_totals_use_the_domain_indexes() {
		let stats = StatsDb::memory();
		let conn = stats.conn();
		let plan = |sql: String| -> String {
			conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
				.unwrap()
				.query_map(params!["shop.ch", "g-shop"], |r| {
					r.get::<_, String>(3)
				})
				.unwrap()
				.collect::<rusqlite::Result<Vec<_>>>()
				.unwrap()
				.join("\n")
		};
		let counts = plan(format!(
			"SELECT * FROM mail_counts WHERE {DOMAIN} IN (?1, ?2) AND day >= 0"
		));
		assert!(counts.contains("INDEX mail_counts_domain"), "{counts}");
		let last = plan(format!(
			"SELECT * FROM mail_last WHERE {DOMAIN} IN (?1, ?2)"
		));
		assert!(last.contains("INDEX mail_last_domain"), "{last}");
	}
}
