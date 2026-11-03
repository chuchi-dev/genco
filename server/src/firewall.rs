//! The blocklist: addresses and ranges that can't reach the sites, the
//! mail and the workers' public ports. ssh stays open to them, so an
//! admin who blocked themselves can always get in and unblock. An entry
//! may end, the minute check removes it then.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::users::Authed;

/// the broadest ranges that may be blocked
const MIN_V4: u8 = 16;
const MIN_V6: u8 = 32;
/// an admin who logged in from an address this recently is protected
const ADMIN_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
	pub id: i64,
	pub address: String,
	pub note: String,
	pub created_by: String,
	pub created_at: i64,
	/// it stops blocking then, `None` blocks until it is removed
	pub until: Option<i64>,
}

/// An address or a range of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Net {
	addr: IpAddr,
	len: u8,
}

impl Net {
	fn max_len(addr: IpAddr) -> u8 {
		if addr.is_ipv4() { 32 } else { 128 }
	}

	/// The address on the left of 128 bits, v4 and v6 alike.
	fn bits(addr: IpAddr) -> u128 {
		match addr {
			IpAddr::V4(a) => (u32::from(a) as u128) << 96,
			IpAddr::V6(a) => u128::from(a),
		}
	}

	fn mask(len: u8) -> u128 {
		if len == 0 {
			0
		} else {
			u128::MAX << (128 - len as u32)
		}
	}

	/// `203.0.113.7` or `203.0.113.0/24`; host bits are cleared.
	pub fn parse(s: &str) -> std::result::Result<Self, String> {
		let s = s.trim();
		let (addr, len) = match s.split_once('/') {
			Some((a, l)) => (a, Some(l)),
			None => (s, None),
		};
		let addr: IpAddr = addr
			.parse()
			.map_err(|_| format!("{s:?} is no IP address"))?;
		let max = Self::max_len(addr);
		let len = match len {
			Some(l) => l
				.parse::<u8>()
				.ok()
				.filter(|l| *l <= max)
				.ok_or_else(|| format!("{s:?}: /{l} is no prefix length"))?,
			None => max,
		};

		let bits = Self::bits(addr) & Self::mask(len);
		let addr = match addr {
			IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::from((bits >> 96) as u32)),
			IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from(bits)),
		};
		Ok(Self { addr, len })
	}

	pub fn contains(&self, ip: IpAddr) -> bool {
		if ip.is_ipv4() != self.addr.is_ipv4() {
			return false;
		}
		Self::bits(ip) & Self::mask(self.len) == Self::bits(self.addr)
	}

	fn overlaps(&self, other: &Net) -> bool {
		self.contains(other.addr) || other.contains(self.addr)
	}
}

impl fmt::Display for Net {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		if self.len == Self::max_len(self.addr) {
			write!(f, "{}", self.addr)
		} else {
			write!(f, "{}/{}", self.addr, self.len)
		}
	}
}

/// The entry as stored, or why it can't be blocked: too broad, loopback,
/// or one of the `protected` addresses (this server's, the admins').
fn check(
	input: &str,
	protected: &[(IpAddr, &str)],
) -> std::result::Result<Net, String> {
	let net = Net::parse(input)?;
	let min = if net.addr.is_ipv4() { MIN_V4 } else { MIN_V6 };
	if net.len < min {
		return Err(format!(
			"{net} is too broad, the broadest is a /{min} for IPv{}",
			if net.addr.is_ipv4() { 4 } else { 6 }
		));
	}
	if net.addr.is_unspecified() {
		return Err(format!("{net} is no address to block"));
	}
	let loopback = [Net::parse("127.0.0.0/8")?, Net::parse("::1")?];
	if loopback.iter().any(|l| l.overlaps(&net)) {
		return Err(format!("{net} is loopback"));
	}
	if let Some((ip, whose)) =
		protected.iter().find(|(ip, _)| net.contains(*ip))
	{
		if net.len == Net::max_len(*ip) {
			return Err(format!("{ip} is {whose}"));
		}
		return Err(format!("{net} contains {ip}, {whose}"));
	}
	Ok(net)
}

/// `30m`, `24h` or `7d` in milliseconds.
fn parse_duration(s: &str) -> std::result::Result<i64, String> {
	let s = s.trim();
	let err = || format!("{s:?} is no duration like 30m, 24h or 7d");
	let unit = match s.chars().last() {
		Some('m') => 60_000,
		Some('h') => 60 * 60_000,
		Some('d') => 24 * 60 * 60_000,
		_ => return Err(err()),
	};
	let n: i64 = s[..s.len() - 1].parse().map_err(|_| err())?;
	if !(1..=365 * 24 * 60).contains(&n) {
		return Err(err());
	}
	Ok(n * unit)
}

/// This server's addresses and those admins logged in from lately.
async fn protected(db: &Db) -> Vec<(IpAddr, String)> {
	let mut out: Vec<(IpAddr, String)> = crate::dns::own_ips()
		.await
		.iter()
		.filter_map(|ip| ip.parse().ok())
		.map(|ip| (ip, "this server's address".to_string()))
		.collect();
	out.extend(admin_ips(&db.conn(), now()).unwrap_or_default());
	out
}

fn admin_ips(
	conn: &Connection,
	now: i64,
) -> rusqlite::Result<Vec<(IpAddr, String)>> {
	let mut stmt = conn.prepare(
		"SELECT s.ip, u.name FROM sessions s JOIN users u ON u.id = s.user_id
		WHERE s.ip IS NOT NULL AND s.last_seen > ?1",
	)?;
	let rows = stmt.query_map([now - ADMIN_MS], |r| {
		Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
	})?;
	let mut out = vec![];
	for row in rows {
		let (ips, name) = row?;
		// x-forwarded-for may be a list
		for ip in ips.split(',').filter_map(|ip| ip.trim().parse().ok()) {
			out.push((ip, format!("an address {name} logged in from lately")));
		}
	}
	Ok(out)
}

const COLUMNS: &str = "id, address, note, created_by, created_at, until";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Block> {
	Ok(Block {
		id: r.get(0)?,
		address: r.get(1)?,
		note: r.get(2)?,
		created_by: r.get(3)?,
		created_at: r.get(4)?,
		until: r.get(5)?,
	})
}

fn list(conn: &Connection) -> rusqlite::Result<Vec<Block>> {
	let mut stmt = conn.prepare(&format!(
		"SELECT {COLUMNS} FROM firewall_blocks ORDER BY created_at DESC, id DESC"
	))?;
	stmt.query_map([], from_row)?.collect()
}

/// The addresses that block at `now`, for the firewall.
pub fn active(conn: &Connection, now: i64) -> rusqlite::Result<Vec<String>> {
	let mut stmt = conn.prepare(
		"SELECT address FROM firewall_blocks
		WHERE until IS NULL OR until > ?1 ORDER BY address",
	)?;
	stmt.query_map([now], |r| r.get(0))?.collect()
}

/// Blocks `net`, or changes its note and end when it is blocked already.
fn add(
	conn: &Connection,
	net: Net,
	note: &str,
	until: Option<i64>,
	by: &str,
) -> rusqlite::Result<()> {
	conn.execute(
		"INSERT INTO firewall_blocks (address, note, created_by, created_at,
		until) VALUES (?1, ?2, ?3, ?4, ?5)
		ON CONFLICT (address) DO UPDATE SET note = ?2, created_by = ?3,
		created_at = ?4, until = ?5",
		params![net.to_string(), note.trim(), by, now(), until],
	)?;
	Ok(())
}

/// Removes the entries that ended before `now`, returns their addresses.
fn remove_expired(
	conn: &Connection,
	now: i64,
) -> rusqlite::Result<Vec<String>> {
	let mut stmt = conn.prepare(
		"DELETE FROM firewall_blocks WHERE until <= ?1 RETURNING address",
	)?;
	stmt.query_map([now], |r| r.get(0))?.collect()
}

/// Renders genco's firewall rules and loads them, without a full apply.
async fn reload(cfg: &Config, db: &Db) -> Result<()> {
	let report = crate::apply::apply_firewall(cfg, db).await;
	if !report.ok {
		return Err(Error::Internal(format!(
			"the firewall wasn't loaded: {}",
			report.log.join("; ")
		)));
	}
	Ok(())
}

/// Every minute: entries that ended stop blocking.
pub async fn due(cfg: &Config, db: &Db) {
	let expired = match remove_expired(&db.conn(), now()) {
		Ok(expired) => expired,
		Err(e) => {
			tracing::error!("firewall blocks: {e}");
			return;
		}
	};
	if expired.is_empty() {
		return;
	}
	for address in &expired {
		crate::activity::log(
			&db.conn(),
			"genco",
			"Unblock address",
			address,
			"its time ended",
			None,
		);
	}
	if let Err(e) = reload(cfg, db).await {
		tracing::error!("{e}");
	}
}

/// Checks and blocks an address, then loads the firewall.
async fn block(
	cfg: &Config,
	db: &Db,
	address: &str,
	note: &str,
	duration: Option<&str>,
	by: &str,
) -> Result<Net> {
	let until = match duration {
		Some(d) => Some(now() + parse_duration(d).map_err(Error::Invalid)?),
		None => None,
	};
	let protected = protected(db).await;
	let protected: Vec<(IpAddr, &str)> =
		protected.iter().map(|(ip, w)| (*ip, w.as_str())).collect();
	let net = check(address, &protected).map_err(Error::Invalid)?;

	add(&db.conn(), net, note, until, by)?;
	reload(cfg, db).await?;
	Ok(net)
}

// -------------------------------------------------------------------- cli

#[derive(Debug, clap::Parser)]
pub enum FirewallCmd {
	/// Block an address or range from the sites, mail and workers' public
	/// ports (never ssh)
	Block {
		/// e.g. 203.0.113.7 or 203.0.113.0/24
		address: String,
		/// it stops blocking after this: 30m, 24h, 7d
		#[clap(long = "for")]
		duration: Option<String>,
		/// why, for the admins
		#[clap(long, default_value = "")]
		note: String,
	},
	/// Remove an address or range from the blocklist
	Unblock { address: String },
	/// The blocked addresses
	List,
}

pub async fn cli(cfg: &Config, db: &Db, cmd: FirewallCmd) -> Result<String> {
	let by = crate::activity::cli_actor();
	match cmd {
		FirewallCmd::Block {
			address,
			duration,
			note,
		} => {
			let net = block(cfg, db, &address, &note, duration.as_deref(), by)
				.await?;
			let until = match &duration {
				Some(d) => format!(" for {d}"),
				None => " until it is unblocked".into(),
			};
			Ok(format!("blocked {net}{until}"))
		}
		FirewallCmd::Unblock { address } => {
			let net = Net::parse(&address).map_err(Error::Invalid)?;
			let n = db.conn().execute(
				"DELETE FROM firewall_blocks WHERE address = ?1",
				[net.to_string()],
			)?;
			if n == 0 {
				return Err(Error::Invalid(format!("{net} isn't blocked")));
			}
			reload(cfg, db).await?;
			Ok(format!("unblocked {net}"))
		}
		FirewallCmd::List => {
			let mut lines = vec![];
			let blocks = list(&db.conn())?;
			for b in blocks {
				let until = match b.until {
					Some(t) => crate::util::format_time(t).await,
					None => "-".into(),
				};
				lines.push(format!(
					"{}\tuntil {until}\tby {}\t{}",
					b.address, b.created_by, b.note
				));
			}
			if lines.is_empty() {
				return Ok("no blocked addresses".into());
			}
			Ok(lines.join("\n"))
		}
	}
}

// -------------------------------------------------------------------- api

async fn list_route(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<Block>>> {
	Ok(Json(list(&db.conn())?))
}

#[derive(Debug, Deserialize)]
struct BlockReq {
	address: String,
	#[serde(default)]
	note: String,
	/// 30m, 24h, 7d; none blocks until it is removed
	duration: Option<String>,
}

async fn create(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<BlockReq>,
) -> Result<Json<()>> {
	let by = &authed.user.name;
	let duration = req.duration.as_deref().filter(|d| !d.is_empty());
	block(&cfg, &db, &req.address, &req.note, duration, by).await?;
	Ok(Json(()))
}

async fn delete_route(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	let n = db
		.conn()
		.execute("DELETE FROM firewall_blocks WHERE id = ?1", [id])?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	reload(&cfg, &db).await?;
	Ok(Json(()))
}

/// The address of an entry, for the activity log.
pub fn address(conn: &Connection, id: i64) -> Option<String> {
	conn.query_row(
		"SELECT address FROM firewall_blocks WHERE id = ?1",
		[id],
		|r| r.get(0),
	)
	.optional()
	.ok()
	.flatten()
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/firewall/blocks", get(list_route).post(create))
		.route("/firewall/blocks/{id}", delete(delete_route))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ok(s: &str) -> String {
		check(s, &[]).unwrap().to_string()
	}

	#[test]
	fn addresses_and_ranges_are_parsed_and_cleaned() {
		assert_eq!(ok("203.0.113.7"), "203.0.113.7");
		assert_eq!(ok(" 203.0.113.7/32 "), "203.0.113.7");
		assert_eq!(ok("203.0.113.7/24"), "203.0.113.0/24");
		assert_eq!(ok("198.51.0.0/16"), "198.51.0.0/16");
		assert_eq!(ok("2001:db8::1"), "2001:db8::1");
		assert_eq!(ok("2001:db8:1:2::5/64"), "2001:db8:1:2::/64");
		assert_eq!(ok("2001:db8::/32"), "2001:db8::/32");
	}

	#[test]
	fn broad_odd_and_own_addresses_are_refused() {
		for bad in [
			"",
			"example.com",
			"203.0.113.7/33",
			"203.0.113.7/",
			"0.0.0.0/0",
			"::/0",
			"10.0.0.0/8",
			"2001:db8::/31",
			"0.0.0.0",
			"::",
			"127.0.0.1",
			"127.0.0.0/16",
			"::1",
		] {
			assert!(check(bad, &[]).is_err(), "{bad} was accepted");
		}

		let own = [("203.0.113.10".parse().unwrap(), "this server")];
		assert!(check("203.0.113.0/24", &own).is_err());
		assert!(check("203.0.113.10", &own).is_err());
		assert!(check("203.0.113.11", &own).is_ok());
		assert!(check("2001:db8::10", &own).is_ok());
	}

	#[test]
	fn durations() {
		assert_eq!(parse_duration("30m"), Ok(30 * 60_000));
		assert_eq!(parse_duration("24h"), Ok(24 * 60 * 60_000));
		assert_eq!(parse_duration("7d"), Ok(7 * 24 * 60 * 60_000));
		for bad in ["", "h", "0h", "-1d", "7", "1w", "1.5h"] {
			assert!(parse_duration(bad).is_err(), "{bad} was accepted");
		}
	}

	#[test]
	fn admins_addresses_are_protected() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute(
			"INSERT INTO users (id, email, name, password_hash, created_at)
			VALUES (1, 'a@example.com', 'Anna', 'x', 0)",
			[],
		)
		.unwrap();
		conn.execute(
			"INSERT INTO sessions (token, id, user_id, ip, created_at,
			last_seen) VALUES ('t', 's', 1, '198.51.100.4, 10.0.0.1', 0, ?1)",
			[now()],
		)
		.unwrap();
		let ips = admin_ips(&conn, now()).unwrap();
		let ips: Vec<(IpAddr, &str)> =
			ips.iter().map(|(ip, w)| (*ip, w.as_str())).collect();
		let err = check("198.51.100.0/24", &ips).unwrap_err();
		assert!(err.contains("Anna"), "{err}");

		// a month later it is no admin's any more
		assert!(admin_ips(&conn, now() + ADMIN_MS + 1).unwrap().is_empty());
	}

	#[test]
	fn ended_blocks_stop_blocking_and_are_removed() {
		let db = Db::memory();
		let conn = db.conn();
		let net = |s| Net::parse(s).unwrap();
		add(
			&conn,
			net("203.0.113.7"),
			"scan",
			Some(now() + 1000),
			"agent",
		)
		.unwrap();
		add(&conn, net("198.51.100.0/24"), "flood", None, "Anna").unwrap();

		let later = now() + 2000;
		assert_eq!(active(&conn, later).unwrap(), ["198.51.100.0/24"]);
		assert_eq!(active(&conn, now()).unwrap().len(), 2);
		assert_eq!(remove_expired(&conn, later).unwrap(), ["203.0.113.7"]);
		assert_eq!(list(&conn).unwrap().len(), 1);

		// blocking again changes the entry
		add(&conn, net("198.51.100.9/24"), "still", None, "Anna").unwrap();
		let all = list(&conn).unwrap();
		assert_eq!(all.len(), 1);
		assert_eq!(all[0].note, "still");
	}
}
