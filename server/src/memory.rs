//! How the RAM is split between the groups and the system: all groups
//! share `genco.slice`, capped at the RAM without the reserve for the
//! system (apply/memory.rs). The reserve is a setting (Settings → Memory,
//! `genco memory reserve`), a quarter of the RAM by default; admins and
//! genco's agent move it within `limits::reserve_bounds` when the split
//! no longer fits what the groups and the system really use.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::apply;
use crate::config::Config;
use crate::db::{Db, set_setting};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::limits;
use crate::users::Authed;

const GROUPS_CGROUP: &str = "/sys/fs/cgroup/genco.slice";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Setting {
	/// `None` is the default
	reserve_mb: Option<u32>,
}

/// The reserve for the system on this server, set or the default.
fn reserve(conn: &Connection) -> u32 {
	reserve_of(limits::server_memory().0, &setting(conn))
}

fn setting(conn: &Connection) -> Setting {
	crate::db::get_json(conn, "memory").unwrap_or_default()
}

/// A reserve that no longer fits the RAM (a resized VM) is clamped.
fn reserve_of(ram_mb: u32, set: &Setting) -> u32 {
	let (min, max) = limits::reserve_bounds(ram_mb);
	set.reserve_mb
		.map(|r| r.clamp(min, max))
		.unwrap_or_else(|| limits::default_reserve(ram_mb))
}

/// What all groups together may use on this server.
pub fn groups_cap(conn: &Connection) -> u32 {
	limits::groups_cap(limits::server_memory().0, reserve(conn))
}

/// Sets the reserve, `None` for the default; apply makes it hold.
pub fn set_reserve(conn: &Connection, reserve_mb: Option<u32>) -> Result<()> {
	let (ram, _) = limits::server_memory();
	check_reserve(ram, reserve_mb, groups_used())?;
	let set = Setting { reserve_mb };
	set_setting(conn, "memory", &serde_json::to_string(&set).unwrap())?;
	Ok(())
}

/// Within the bounds, and the new cap leaves the groups what they use now
/// without their page cache: below that the kernel kills in the groups
/// right away.
fn check_reserve(
	ram_mb: u32,
	reserve_mb: Option<u32>,
	groups_used_mb: Option<u32>,
) -> Result<()> {
	let Some(r) = reserve_mb else {
		return Ok(());
	};
	let (min, max) = limits::reserve_bounds(ram_mb);
	if !(min..=max).contains(&r) {
		return Err(Error::Invalid(format!(
			"the reserve is {min} MB to {max} MB, half the RAM"
		)));
	}
	let cap = limits::groups_cap(ram_mb, r);
	if let Some(used) = groups_used_mb
		&& cap < used
	{
		return Err(Error::Invalid(format!(
			"the groups use {used} MB now, a reserve of {r} MB leaves them \
			{cap} MB; at most {} MB",
			(ram_mb - used).min(max)
		)));
	}
	Ok(())
}

fn read_groups(file: &str) -> Option<String> {
	std::fs::read_to_string(format!("{GROUPS_CGROUP}/{file}")).ok()
}

/// What the groups use now without their page cache, which the kernel
/// takes back first, in MB.
fn groups_used() -> Option<u32> {
	let current: u64 = read_groups("memory.current")?.trim().parse().ok()?;
	let stat = read_groups("memory.stat")?;
	let file: u64 = stat
		.lines()
		.find_map(|l| l.strip_prefix("file "))?
		.trim()
		.parse()
		.ok()?;
	Some((current.saturating_sub(file) >> 20) as u32)
}

/// The split and what the groups and the system use, for the panel, the
/// CLI and the agent's decisions.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
	pub ram_mb: u32,
	pub swap_mb: u32,
	pub reserve_mb: u32,
	/// the reserve is the default, not set
	pub default: bool,
	pub default_reserve_mb: u32,
	pub min_reserve_mb: u32,
	pub max_reserve_mb: u32,
	/// what all groups may use together
	pub cap_mb: u32,
	/// the groups' own limits added up (the default for those without)
	pub limits_mb: u32,
	/// without their page cache, like the system's number
	pub groups_now_mb: Option<u32>,
	/// since the groups' slice started (the last boot, mostly)
	pub groups_peak_mb: Option<u32>,
	/// everything outside the groups: the RAM in use without them
	pub system_now_mb: Option<u32>,
}

pub fn status(conn: &Connection) -> Result<Status> {
	let (ram, swap) = limits::server_memory();
	let set = setting(conn);
	let reserve_mb = reserve_of(ram, &set);
	let (min, max) = limits::reserve_bounds(ram);
	let groups_now = groups_used();
	let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
	let used = crate::stats::meminfo(&info, "MemTotal:")
		.zip(crate::stats::meminfo(&info, "MemAvailable:"))
		.map(|(t, a)| ((t - a) >> 20) as u32);
	Ok(Status {
		ram_mb: ram,
		swap_mb: swap,
		reserve_mb,
		default: set.reserve_mb.is_none(),
		default_reserve_mb: limits::default_reserve(ram),
		min_reserve_mb: min,
		max_reserve_mb: max,
		cap_mb: limits::groups_cap(ram, reserve_mb),
		limits_mb: crate::hosting::groups(conn)?
			.iter()
			.map(|g| g.limits.memory_max())
			.sum(),
		groups_now_mb: groups_now,
		groups_peak_mb: read_groups("memory.peak")
			.and_then(|v| v.trim().parse::<u64>().ok())
			.map(|v| (v >> 20) as u32),
		system_now_mb: used.zip(groups_now).map(|(u, g)| u.saturating_sub(g)),
	})
}

impl Status {
	/// For `genco memory`.
	pub fn text(&self) -> String {
		let mb = |v: Option<u32>| v.map_or("?".into(), |v| format!("{v} MB"));
		format!(
			"RAM {} MB, swap {} MB\n\
			reserve for the system {} MB ({}; {} to {} MB)\n\
			all groups together at most {} MB, their own limits add up to \
			{} MB\n\
			groups now {}, at most {} since their slice started\n\
			system now {}",
			self.ram_mb,
			self.swap_mb,
			self.reserve_mb,
			if self.default {
				"the default".to_string()
			} else {
				format!("set, the default is {} MB", self.default_reserve_mb)
			},
			self.min_reserve_mb,
			self.max_reserve_mb,
			self.cap_mb,
			self.limits_mb,
			mb(self.groups_now_mb),
			mb(self.groups_peak_mb),
			mb(self.system_now_mb),
		)
	}
}

/// `genco memory reserve`: a new reserve.
#[derive(Debug, clap::Parser)]
pub enum MemoryCmd {
	/// Keep this much for the system (2G, 1536M, 1536) or `default`;
	/// applies at once
	Reserve { size: String },
}

async fn get_route(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Status>> {
	Ok(Json(status(&db.conn())?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetReq {
	/// `None` for the default
	reserve_mb: Option<u32>,
}

/// Sets the reserve and applies, genco.slice gets the new cap at once.
async fn set_route(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	Json(req): Json<SetReq>,
) -> Result<Json<apply::Report>> {
	set_reserve(&db.conn(), req.reserve_mb)?;
	let report = apply::apply(&cfg, &db).await;
	events.send(Event::Tree);
	Ok(Json(report))
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/settings/memory", get(get_route).post(set_route))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_reserve_stays_within_bounds() {
		assert!(check_reserve(4000, None, Some(3900)).is_ok());
		assert!(check_reserve(4000, Some(511), None).is_err());
		assert!(check_reserve(4000, Some(2001), None).is_err());
		assert!(check_reserve(4000, Some(512), None).is_ok());
		assert!(check_reserve(4000, Some(2000), None).is_ok());
		// the groups use 2500 MB without their cache
		assert!(check_reserve(4000, Some(1500), Some(2500)).is_ok());
		assert!(check_reserve(4000, Some(1600), Some(2500)).is_err());
	}

	#[test]
	fn a_set_reserve_is_clamped_to_the_ram() {
		let set = |r| Setting { reserve_mb: r };
		assert_eq!(reserve_of(4000, &set(None)), 1000);
		assert_eq!(reserve_of(4000, &set(Some(1500))), 1500);
		// the VM shrank to 2 GB
		assert_eq!(reserve_of(2000, &set(Some(1500))), 1000);

		let db = Db::memory();
		set_setting(&db.conn(), "memory", r#"{"reserveMb":700}"#).unwrap();
		assert_eq!(setting(&db.conn()).reserve_mb, Some(700));
	}
}
