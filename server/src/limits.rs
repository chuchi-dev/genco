//! What a group's slice may use: memory, CPU and tasks, the defaults where
//! the group has none set. Also the drop-ins other tools (or an agent)
//! put beside genco's units, which override what genco writes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The defaults, what every group had before limits were a setting.
pub const MEMORY_MAX_MB: u32 = 1536;
pub const CPU_PERCENT: u32 = 150;
pub const TASKS_MAX: u32 = 512;

const SYSTEMD: &str = "/etc/systemd/system";
/// Where `systemctl set-property` writes without `--runtime`; its files
/// win over those of the same name in /run/systemd/system.control, where
/// genco sets the limits on the running slice.
const SYSTEM_CONTROL: &str = "/etc/systemd/system.control";

/// `None` is the default.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase", default)]
pub struct Limits {
	/// MemoryMax, the main setting: above it the kernel kills
	pub memory_max_mb: Option<u32>,
	/// MemoryHigh, the kernel slows the group down and reclaims above it;
	/// two thirds of the max by default
	pub memory_high_mb: Option<u32>,
	/// CPUQuota, 100 is one core
	pub cpu_percent: Option<u32>,
	pub tasks_max: Option<u32>,
}

impl Limits {
	pub fn memory_max(&self) -> u32 {
		self.memory_max_mb.unwrap_or(MEMORY_MAX_MB)
	}

	pub fn memory_high(&self) -> u32 {
		self.memory_high_mb.unwrap_or(self.memory_max() * 2 / 3)
	}

	pub fn cpu(&self) -> u32 {
		self.cpu_percent.unwrap_or(CPU_PERCENT)
	}

	pub fn tasks(&self) -> u32 {
		self.tasks_max.unwrap_or(TASKS_MAX)
	}

	/// The slice's settings, in its unit and for `systemctl set-property`.
	pub fn unit_properties(&self) -> [String; 4] {
		[
			format!("MemoryHigh={}", unit_size(self.memory_high())),
			format!("MemoryMax={}", unit_size(self.memory_max())),
			format!("TasksMax={}", self.tasks()),
			format!("CPUQuota={}%", self.cpu()),
		]
	}

	/// What each setting is when it is empty and the others stay: the
	/// panel's placeholders.
	pub fn defaults(&self) -> Defaults {
		let high = Limits {
			memory_high_mb: None,
			..*self
		};
		Defaults {
			memory_max_mb: MEMORY_MAX_MB,
			memory_high_mb: high.memory_high(),
			cpu_percent: CPU_PERCENT,
			tasks_max: TASKS_MAX,
		}
	}

	/// Within what the server has: `cap_mb`, what all groups may use
	/// together (`groups_cap`), and `cores`. A memory limit the group
	/// already has (`kept_mb`) stays allowed, the cap of all groups holds
	/// it anyway.
	pub fn check(
		&self,
		cap_mb: u32,
		cores: u32,
		kept_mb: Option<u32>,
	) -> Result<()> {
		let max = self.memory_max();
		if self.memory_max_mb.is_some_and(|m| Some(m) != kept_mb)
			&& !(128..=cap_mb).contains(&max)
		{
			return Err(Error::Invalid(format!(
				"the memory limit is 128 MB to {}, what all groups may use \
				together (`genco memory`)",
				size(cap_mb)
			)));
		}
		if !(64..=max).contains(&self.memory_high()) {
			return Err(Error::Invalid(format!(
				"the soft memory limit is 64 MB to the memory limit ({})",
				size(max)
			)));
		}
		if !(10..=100 * cores).contains(&self.cpu()) {
			return Err(Error::Invalid(format!(
				"CPU is 10 to {}% ({cores} cores)",
				100 * cores
			)));
		}
		if !(64..=65536).contains(&self.tasks()) {
			return Err(Error::Invalid("tasks are 64 to 65536".into()));
		}
		Ok(())
	}

	/// For the CLI and the agent: "memory 2.5 GB (slows down from 2 GB),
	/// CPU 200%, 1024 tasks".
	pub fn describe(&self) -> String {
		format!(
			"memory {} (slows down from {}), CPU {}%, {} tasks",
			size(self.memory_max()),
			size(self.memory_high()),
			self.cpu(),
			self.tasks()
		)
	}

	/// ImageMagick's memory for pixels per process: a quarter of
	/// MemoryHigh, 256 MiB at most; beyond it, it uses its disk cache
	/// instead of pushing the group over its limit. The map limit is twice
	/// that.
	pub fn magick_memory_mb(&self) -> u32 {
		(self.memory_high() / 4).min(256)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
	pub memory_max_mb: u32,
	pub memory_high_mb: u32,
	pub cpu_percent: u32,
	pub tasks_max: u32,
}

/// A group's limits for the panel, with their defaults.
pub fn with_defaults<S: serde::Serializer>(
	limits: &Limits,
	s: S,
) -> std::result::Result<S::Ok, S::Error> {
	#[derive(Serialize)]
	struct Shown<'a> {
		#[serde(flatten)]
		limits: &'a Limits,
		defaults: Defaults,
	}
	Shown {
		limits,
		defaults: limits.defaults(),
	}
	.serialize(s)
}

/// "2.5 GB", "512 MB"
fn size(mb: u32) -> String {
	if mb < 1024 {
		return format!("{mb} MB");
	}
	let gb = format!("{:.1}", mb as f64 / 1024.0);
	format!("{} GB", gb.trim_end_matches(".0"))
}

/// As systemd takes it: 1G, 1536M.
fn unit_size(mb: u32) -> String {
	if mb.is_multiple_of(1024) {
		format!("{}G", mb / 1024)
	} else {
		format!("{mb}M")
	}
}

/// "2.5G", "2.5 GB", "1536M", "1536" (MB), or "default" for `None`.
pub fn parse_size(s: &str) -> Result<Option<u32>> {
	parse(s, |s| {
		let s = s.to_ascii_lowercase();
		let (num, factor) = if let Some(n) = strip_any(&s, &["gib", "gb", "g"])
		{
			(n, 1024.0)
		} else if let Some(n) = strip_any(&s, &["mib", "mb", "m"]) {
			(n, 1.0)
		} else {
			(s.as_str(), 1.0)
		};
		let n: f64 = num.trim().parse().ok()?;
		(n > 0.0 && n.is_finite()).then(|| (n * factor).round() as u32)
	})
	.map_err(|()| {
		Error::Invalid(format!(
			"{s} is no size: 2.5G, 512M, 512 (MB) or default"
		))
	})
}

/// "200%", "200" or "default".
pub fn parse_percent(s: &str) -> Result<Option<u32>> {
	parse(s, |s| s.trim_end_matches('%').trim().parse().ok()).map_err(|()| {
		Error::Invalid(format!("{s} is no percentage: 200% or default"))
	})
}

/// "1024" or "default".
pub fn parse_count(s: &str) -> Result<Option<u32>> {
	parse(s, |s| s.parse().ok()).map_err(|()| {
		Error::Invalid(format!("{s} is no number: 1024 or default"))
	})
}

fn parse(
	s: &str,
	value: impl Fn(&str) -> Option<u32>,
) -> std::result::Result<Option<u32>, ()> {
	let s = s.trim();
	if s.eq_ignore_ascii_case("default") {
		return Ok(None);
	}
	value(s).map(Some).ok_or(())
}

fn strip_any<'a>(s: &'a str, suffixes: &[&str]) -> Option<&'a str> {
	suffixes.iter().find_map(|x| s.strip_suffix(x))
}

/// RAM and swap in MB, from /proc/meminfo.
pub fn server_memory() -> (u32, u32) {
	let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
	let mb = |field| {
		crate::stats::meminfo(&info, field).map_or(0, |b| (b >> 20) as u32)
	};
	(mb("MemTotal:"), mb("SwapTotal:"))
}

fn cores() -> u32 {
	std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

/// Checks the limits against this server with what all groups may use
/// (`cap_mb`), `kept_mb` is the memory limit the group has now.
pub fn check_here(
	limits: &Limits,
	cap_mb: u32,
	kept_mb: Option<u32>,
) -> Result<()> {
	limits.check(cap_mb, cores(), kept_mb)
}

/// The least an admin or the agent may keep for the system.
pub const MIN_RESERVE_MB: u32 = 512;

/// The memory genco keeps for the system (the databases, nginx, mail,
/// genco itself) unless set (memory.rs): a quarter of the RAM, at least
/// 768 MB, at most half.
pub fn default_reserve(ram_mb: u32) -> u32 {
	(ram_mb / 4).max(768).min(ram_mb / 2)
}

/// What a reserve may be: `MIN_RESERVE_MB` to half the RAM.
pub fn reserve_bounds(ram_mb: u32) -> (u32, u32) {
	(MIN_RESERVE_MB.min(ram_mb / 2), ram_mb / 2)
}

/// What all groups together may use (`genco.slice`): the RAM without the
/// reserve. Their own limits may add up to much more, they rarely peak
/// together; when they do, a group runs out, never the system.
pub fn groups_cap(ram_mb: u32, reserve_mb: u32) -> u32 {
	ram_mb.saturating_sub(reserve_mb)
}

/// The unit of the slice all groups' slices are in.
pub fn groups_slice_unit(cap_mb: u32) -> String {
	format!(
		"{}: genco memory reserve changes it
[Unit]
Description=All groups of genco

[Slice]
MemoryMax={}
",
		crate::render::HEADER,
		unit_size(cap_mb)
	)
}

/// A file in the drop-in folder of one of genco's units: systemd reads
/// it after genco's unit, its settings win. genco writes no drop-ins
/// there, every one is someone else's. Those in /run/systemd/system.control
/// are genco's, the limits on the running slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropIn {
	pub path: PathBuf,
	/// `None` for a prefix drop-in (genco-.slice.d), it changes every group
	pub group: Option<String>,
	/// `genco-<group>.slice` or one of its services, `genco-.slice`
	pub unit: String,
}

impl DropIn {
	fn is_slice(&self) -> bool {
		self.unit.ends_with(".slice")
	}

	pub fn text(&self) -> String {
		let path = self.path.display();
		match (&self.group, self.is_slice()) {
			(Some(g), true) => format!(
				"{path} overrides genco's limits of {g}; move it into the \
				group's limits"
			),
			(Some(g), false) => format!(
				"{path} overrides genco's unit {} of {g}; move it into \
				genco's settings",
				self.unit
			),
			(None, true) => format!(
				"{path} overrides genco's limits of every group; set them \
				per group and remove it"
			),
			(None, false) => format!(
				"{path} overrides all of genco's services; move it into \
				genco's settings"
			),
		}
	}
}

/// The drop-ins among `paths` (files under /etc/systemd/system and
/// /etc/systemd/system.control) that change a unit of one of the
/// `groups`, or of all of them.
fn foreign(paths: &[PathBuf], groups: &[&str]) -> Vec<DropIn> {
	let mut found: Vec<DropIn> = paths
		.iter()
		.filter_map(|path| {
			// systemd only reads .conf
			if path.extension()? != "conf" {
				return None;
			}
			let dir = path.parent()?.file_name()?.to_str()?;
			let unit = dir.strip_suffix(".d")?;
			if !unit.ends_with(".slice") && !unit.ends_with(".service") {
				return None;
			}
			let rest = unit.strip_prefix("genco-")?;
			// genco-.slice.d is read for every genco-*.slice
			let group = if rest.starts_with('.') {
				None
			} else {
				// group names have no "-" and no "."
				let group = rest.split(['-', '.']).next()?;
				Some(groups.contains(&group).then(|| group.to_string())?)
			};
			(!groups.is_empty()).then(|| DropIn {
				path: path.clone(),
				group,
				unit: unit.into(),
			})
		})
		.collect();
	found.sort_by(|a, b| a.path.cmp(&b.path));
	found
}

/// Every file in the drop-in folders of genco's units.
fn dropin_files() -> Vec<PathBuf> {
	let mut files = vec![];
	for base in [SYSTEMD, SYSTEM_CONTROL] {
		let Ok(entries) = std::fs::read_dir(base) else {
			continue;
		};
		for dir in entries.flatten() {
			let name = dir.file_name().to_string_lossy().to_string();
			if !name.starts_with("genco-") || !name.ends_with(".d") {
				continue;
			}
			let Ok(inner) = std::fs::read_dir(dir.path()) else {
				continue;
			};
			files.extend(inner.flatten().map(|e| e.path()));
		}
	}
	files
}

/// The drop-ins on this server that change the units of `groups`.
pub fn foreign_here(groups: &[&str]) -> Vec<DropIn> {
	foreign(&dropin_files(), groups)
}

/// `limits` with the settings of a slice drop-in on top. Refuses one with
/// other settings: removing it would lose them.
pub fn adopt(mut limits: Limits, content: &str) -> Result<Limits> {
	for line in content.lines().map(str::trim) {
		if line.is_empty() || line.starts_with(['#', ';']) || line == "[Slice]"
		{
			continue;
		}
		let bad = || {
			Error::Invalid(format!(
				"can't adopt \"{line}\", move it by hand or set the limits \
				with genco group set"
			))
		};
		let (key, value) = line.split_once('=').ok_or_else(bad)?;
		let value = value.trim();
		match key.trim() {
			"MemoryMax" => {
				limits.memory_max_mb =
					Some(systemd_size(value).ok_or_else(bad)?)
			}
			"MemoryHigh" => {
				limits.memory_high_mb =
					Some(systemd_size(value).ok_or_else(bad)?)
			}
			"CPUQuota" => {
				// set-property writes 150.00%
				let v = value.strip_suffix('%').ok_or_else(bad)?;
				let v: f64 = v.parse().map_err(|_| bad())?;
				if !v.is_finite() || v < 0.0 {
					return Err(bad());
				}
				limits.cpu_percent = Some(v.round() as u32)
			}
			"TasksMax" => {
				limits.tasks_max = Some(value.parse().map_err(|_| bad())?)
			}
			_ => return Err(bad()),
		}
	}
	Ok(limits)
}

/// A size as systemd reads it, in MB: bytes, or K, M, G, T (base 1024).
/// `None` for infinity and percentages.
fn systemd_size(value: &str) -> Option<u32> {
	let (num, factor) = match value.chars().last()? {
		'K' => (&value[..value.len() - 1], 1.0 / 1024.0),
		'M' => (&value[..value.len() - 1], 1.0),
		'G' => (&value[..value.len() - 1], 1024.0),
		'T' => (&value[..value.len() - 1], 1024.0 * 1024.0),
		_ => (value, 1.0 / 1024.0 / 1024.0),
	};
	// Rust reads "infinity" as a float too
	let n: f64 = num.parse().ok().filter(|n: &f64| n.is_finite())?;
	Some((n * factor).round() as u32)
}

/// The drop-in files of a group's slice, also those of `systemctl
/// set-property`, in the order systemd reads them.
pub fn slice_dropins(group: &str) -> Vec<PathBuf> {
	let dir = format!("{}.d", crate::render::slice(group));
	let files: Vec<PathBuf> = [SYSTEMD, SYSTEM_CONTROL]
		.into_iter()
		.flat_map(|base| {
			std::fs::read_dir(Path::new(base).join(&dir))
				.map(|e| e.flatten().map(|e| e.path()).collect())
				.unwrap_or_else(|_| vec![])
		})
		.collect();
	read_order(foreign(&files, &[group]))
}

/// By file name, whatever the folder; of two with the same name the one in
/// system.control wins, it comes last.
fn read_order(dropins: Vec<DropIn>) -> Vec<PathBuf> {
	let mut paths: Vec<PathBuf> = dropins.into_iter().map(|d| d.path).collect();
	paths.sort_by_key(|p| {
		(
			p.file_name().map(|n| n.to_owned()),
			p.starts_with(SYSTEM_CONTROL),
		)
	});
	paths
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn defaults_are_the_old_fixed_values() {
		let l = Limits::default();
		assert_eq!(
			(l.memory_max(), l.memory_high(), l.cpu(), l.tasks()),
			(1536, 1024, 150, 512)
		);
		assert_eq!(unit_size(l.memory_high()), "1G");
		assert_eq!(unit_size(l.memory_max()), "1536M");

		// the soft limit follows the max unless it is set
		let l = Limits {
			memory_max_mb: Some(2560),
			..Limits::default()
		};
		assert_eq!(l.memory_high(), 1706);
		let l = Limits {
			memory_high_mb: Some(2048),
			..l
		};
		assert_eq!(l.memory_high(), 2048);
	}

	#[test]
	fn sizes_percentages_and_counts() {
		assert_eq!(parse_size("2.5G").unwrap(), Some(2560));
		assert_eq!(parse_size("2.5 GB").unwrap(), Some(2560));
		assert_eq!(parse_size("2g").unwrap(), Some(2048));
		assert_eq!(parse_size("1536M").unwrap(), Some(1536));
		assert_eq!(parse_size("768MiB").unwrap(), Some(768));
		assert_eq!(parse_size("512").unwrap(), Some(512));
		assert_eq!(parse_size("default").unwrap(), None);
		assert!(parse_size("lots").is_err());
		assert!(parse_size("-1G").is_err());
		assert!(parse_size("").is_err());
		assert!(parse_size("inf").is_err());

		assert_eq!(parse_percent("200%").unwrap(), Some(200));
		assert_eq!(parse_percent("50").unwrap(), Some(50));
		assert_eq!(parse_percent("Default").unwrap(), None);
		assert!(parse_percent("half").is_err());

		assert_eq!(parse_count("1024").unwrap(), Some(1024));
		assert!(parse_count("1k").is_err());

		assert_eq!(size(512), "512 MB");
		assert_eq!(size(2560), "2.5 GB");
		assert_eq!(size(2048), "2 GB");
		assert_eq!(size(3993), "3.9 GB");
	}

	#[test]
	fn bounds_follow_the_server() {
		let ok = |l: Limits| l.check(3000, 2, None).is_ok();
		assert!(ok(Limits::default()));
		let mem = |mb| Limits {
			memory_max_mb: Some(mb),
			..Limits::default()
		};
		assert!(ok(mem(128)));
		assert!(!ok(mem(100)));
		// what all groups may use together, a quarter stays for the system
		assert!(ok(mem(3000)));
		assert!(!ok(mem(3001)));
		// one it had before the cap stays
		assert!(mem(3500).check(3000, 2, Some(3500)).is_ok());
		assert!(mem(3600).check(3000, 2, Some(3500)).is_err());
		// a soft limit above the max
		assert!(!ok(Limits {
			memory_high_mb: Some(2048),
			..Limits::default()
		}));
		let cpu = |p| Limits {
			cpu_percent: Some(p),
			..Limits::default()
		};
		assert!(ok(cpu(200)));
		assert!(!ok(cpu(201)));
		assert!(!ok(cpu(5)));
		let tasks = |n| Limits {
			tasks_max: Some(n),
			..Limits::default()
		};
		assert!(ok(tasks(64)));
		assert!(!ok(tasks(63)));
		assert!(!ok(tasks(65537)));
	}

	#[test]
	fn the_system_keeps_a_reserve() {
		assert_eq!(default_reserve(4000), 1000);
		assert_eq!(groups_cap(4000, 1000), 3000);
		assert_eq!(default_reserve(2048), 768);
		assert_eq!(default_reserve(1024), 512);
		assert_eq!(default_reserve(16384), 4096);
		assert_eq!(reserve_bounds(4000), (512, 2000));
		assert_eq!(reserve_bounds(800), (400, 400));
		assert!(groups_slice_unit(3000).contains("MemoryMax=3000M\n"));
		assert!(groups_slice_unit(3072).contains("MemoryMax=3G\n"));
	}

	#[test]
	fn foreign_dropins_of_known_groups() {
		let paths: Vec<PathBuf> = [
			"/etc/systemd/system/genco-puresport.slice.d/memory.conf",
			"/etc/systemd/system/genco-shop-php8.4.service.d/env.conf",
			"/etc/systemd/system/genco-shop-worker-q.service.d/x.conf",
			// not read by systemd
			"/etc/systemd/system/genco-shop.slice.d/notes.txt",
			// no group of genco
			"/etc/systemd/system/genco-backup.service.d/override.conf",
			"/etc/systemd/system/genco-other.slice.d/a.conf",
			// a timer has no limits
			"/etc/systemd/system/genco-shop-cron-x.timer.d/a.conf",
			// systemctl set-property without --runtime
			"/etc/systemd/system.control/genco-shop.slice.d/50-MemoryMax.conf",
			// for every group's slice and every genco service
			"/etc/systemd/system/genco-.slice.d/memory.conf",
			"/etc/systemd/system/genco-.service.d/env.conf",
		]
		.into_iter()
		.map(PathBuf::from)
		.collect();
		let found = foreign(&paths, &["puresport", "shop"]);
		let units: Vec<(Option<&str>, &str)> = found
			.iter()
			.map(|d| (d.group.as_deref(), d.unit.as_str()))
			.collect();
		assert_eq!(
			units,
			[
				(None, "genco-.service"),
				(None, "genco-.slice"),
				(Some("puresport"), "genco-puresport.slice"),
				(Some("shop"), "genco-shop-php8.4.service"),
				(Some("shop"), "genco-shop-worker-q.service"),
				(Some("shop"), "genco-shop.slice"),
			]
		);
		assert!(found[5].path.starts_with(SYSTEM_CONTROL));
		assert_eq!(
			found[1].text(),
			"/etc/systemd/system/genco-.slice.d/memory.conf overrides \
			genco's limits of every group; set them per group and remove it"
		);
		// without groups nothing is overridden
		assert!(foreign(&paths, &[]).is_empty());
		let found = &found[2..];
		assert_eq!(
			found[0].text(),
			"/etc/systemd/system/genco-puresport.slice.d/memory.conf \
			overrides genco's limits of puresport; move it into the group's \
			limits"
		);
		assert!(found[1].text().contains("unit genco-shop-php8.4.service"));
	}

	#[test]
	fn set_property_files_come_after_those_of_the_same_name() {
		let d = |path: &str| DropIn {
			path: path.into(),
			group: Some("shop".into()),
			unit: "genco-shop.slice".into(),
		};
		let order = read_order(vec![
			d(
				"/etc/systemd/system.control/genco-shop.slice.d/50-MemoryMax.conf",
			),
			d("/etc/systemd/system/genco-shop.slice.d/50-MemoryMax.conf"),
			d("/etc/systemd/system/genco-shop.slice.d/90-cpu.conf"),
			d("/etc/systemd/system/genco-shop.slice.d/10-tasks.conf"),
		]);
		let order: Vec<String> =
			order.iter().map(|p| p.display().to_string()).collect();
		assert_eq!(
			order,
			[
				"/etc/systemd/system/genco-shop.slice.d/10-tasks.conf",
				"/etc/systemd/system/genco-shop.slice.d/50-MemoryMax.conf",
				"/etc/systemd/system.control/genco-shop.slice.d/\
				50-MemoryMax.conf",
				"/etc/systemd/system/genco-shop.slice.d/90-cpu.conf",
			]
		);
	}

	#[test]
	fn the_slice_s_properties() {
		assert_eq!(
			Limits::default().unit_properties(),
			[
				"MemoryHigh=1G",
				"MemoryMax=1536M",
				"TasksMax=512",
				"CPUQuota=150%"
			]
		);
		let l = Limits {
			memory_max_mb: Some(2560),
			cpu_percent: Some(200),
			tasks_max: Some(1024),
			..Limits::default()
		};
		assert_eq!(
			l.unit_properties(),
			[
				"MemoryHigh=1706M",
				"MemoryMax=2560M",
				"TasksMax=1024",
				"CPUQuota=200%"
			]
		);
	}

	#[test]
	fn defaults_are_what_an_empty_field_means() {
		let l = Limits {
			memory_max_mb: Some(3072),
			memory_high_mb: Some(1024),
			cpu_percent: Some(300),
			tasks_max: None,
		};
		assert_eq!(
			l.defaults(),
			Defaults {
				memory_max_mb: 1536,
				memory_high_mb: 2048,
				cpu_percent: 150,
				tasks_max: 512,
			}
		);
		#[derive(Serialize)]
		struct G {
			#[serde(serialize_with = "with_defaults")]
			limits: Limits,
		}
		let json = serde_json::to_value(G { limits: l }).unwrap();
		assert_eq!(json["limits"]["memoryMaxMb"], 3072);
		assert_eq!(json["limits"]["tasksMax"], serde_json::Value::Null);
		assert_eq!(json["limits"]["defaults"]["memoryHighMb"], 2048);
	}

	#[test]
	fn adopts_a_slice_dropin() {
		let l = adopt(
			Limits::default(),
			"# more memory for Craft\n[Slice]\nMemoryHigh=2G\n\
			MemoryMax=2.5G\nCPUQuota=200%\nTasksMax=1024\n",
		)
		.unwrap();
		assert_eq!(
			l,
			Limits {
				memory_max_mb: Some(2560),
				memory_high_mb: Some(2048),
				cpu_percent: Some(200),
				tasks_max: Some(1024),
			}
		);
		// bytes and K, the rest stays
		let l = adopt(l, "[Slice]\nMemoryMax=3221225472\nMemoryHigh=2097152K")
			.unwrap();
		assert_eq!(
			(l.memory_max_mb, l.memory_high_mb),
			(Some(3072), Some(2048))
		);
		assert_eq!(l.tasks_max, Some(1024));
		// as systemctl set-property writes it
		let l = adopt(
			l,
			"# This is a drop-in unit file extension, created via \
			\"systemctl set-property\"\n[Slice]\nCPUQuota=150.00%\n",
		)
		.unwrap();
		assert_eq!(l.cpu_percent, Some(150));

		// what it can't take over stays a manual job
		assert!(adopt(l, "[Slice]\nMemoryMax=infinity").is_err());
		assert!(adopt(l, "[Slice]\nMemoryMax=50%").is_err());
		assert!(adopt(l, "[Slice]\nCPUWeight=50").is_err());
		assert!(adopt(l, "[Slice]\nCPUQuota=2").is_err());
	}

	#[test]
	fn imagemagick_gets_a_share_of_the_soft_limit() {
		assert_eq!(Limits::default().magick_memory_mb(), 256);
		let small = Limits {
			memory_max_mb: Some(512),
			..Limits::default()
		};
		assert_eq!(small.magick_memory_mb(), 85);
	}
}
