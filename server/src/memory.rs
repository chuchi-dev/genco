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
pub fn reserve(conn: &Connection) -> u32 {
	let (ram, _) = limits::server_memory();
	let set: Setting = crate::db::get_json(conn, "memory").unwrap_or_default();
	let (min, max) = limits::reserve_bounds(ram);
	set.reserve_mb
		.map(|r| r.clamp(min, max))
		.unwrap_or_else(|| limits::default_reserve(ram))
}

/// What all groups together may use on this server.
pub fn groups_cap(conn: &Connection) -> u32 {
	limits::groups_cap(limits::server_memory().0, reserve(conn))
}

/// Sets the reserve, `None` for the default; apply makes it hold.
pub fn set_reserve(conn: &Connection, reserve_mb: Option<u32>) -> Result<()> {
	let (ram, _) = limits::server_memory();
	let (min, max) = limits::reserve_bounds(ram);
	if let Some(r) = reserve_mb
		&& !(min..=max).contains(&r)
	{
		return Err(Error::Invalid(format!(
			"the reserve is {min} MB to {max} MB, half the RAM"
		)));
	}
	let set = Setting { reserve_mb };
	set_setting(conn, "memory", &serde_json::to_string(&set).unwrap())?;
	Ok(())
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
	pub groups_now_mb: Option<u32>,
	/// since the groups' slice started (the last boot, mostly)
	pub groups_peak_mb: Option<u32>,
	/// everything outside the groups: the RAM in use without them
	pub system_now_mb: Option<u32>,
}

pub fn status(conn: &Connection) -> Result<Status> {
	let (ram, swap) = limits::server_memory();
	let set: Setting = crate::db::get_json(conn, "memory").unwrap_or_default();
	let reserve_mb = reserve(conn);
	let (min, max) = limits::reserve_bounds(ram);
	let read = |file: &str| -> Option<u32> {
		let v =
			std::fs::read_to_string(format!("{GROUPS_CGROUP}/{file}")).ok()?;
		Some((v.trim().parse::<u64>().ok()? >> 20) as u32)
	};
	let groups_now = read("memory.current");
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
		groups_peak_mb: read("memory.peak"),
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

/// `genco memory`: the split, or a new reserve.
#[derive(Debug, clap::Parser)]
pub enum MemoryCmd {
	/// Show the split between the groups and the system and what each uses
	Show,
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
	fn the_reserve_is_a_setting_within_bounds() {
		let db = Db::memory();
		let conn = db.conn();
		let (ram, _) = limits::server_memory();
		assert_eq!(reserve(&conn), limits::default_reserve(ram));

		let (min, max) = limits::reserve_bounds(ram);
		assert!(set_reserve(&conn, Some(min - 1)).is_err());
		assert!(set_reserve(&conn, Some(max + 1)).is_err());
		set_reserve(&conn, Some(min)).unwrap();
		assert_eq!(reserve(&conn), min);
		assert!(!status(&conn).unwrap().default);
		set_reserve(&conn, None).unwrap();
		assert!(status(&conn).unwrap().default);
	}
}
