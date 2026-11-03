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

	/// Within what the server has: `ram_mb` and `cores`.
	pub fn check(&self, ram_mb: u32, cores: u32) -> Result<()> {
		let max = self.memory_max();
		if self.memory_max_mb.is_some() && !(128..=ram_mb).contains(&max) {
			return Err(Error::Invalid(format!(
				"the memory limit is 128 MB to the server's {}",
				size(ram_mb)
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

/// "2.5 GB", "512 MB"
pub fn size(mb: u32) -> String {
	if mb < 1024 {
		return format!("{mb} MB");
	}
	let gb = format!("{:.1}", mb as f64 / 1024.0);
	format!("{} GB", gb.trim_end_matches(".0"))
}

/// As systemd takes it: 1G, 1536M.
pub fn unit_size(mb: u32) -> String {
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

pub fn cores() -> u32 {
	std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

/// Checks the limits against this server.
pub fn check_here(limits: &Limits) -> Result<()> {
	limits.check(server_memory().0, cores())
}

/// When the groups' memory limits add up to more than the RAM: together
/// they may push the server out of memory before one hits its own.
pub fn overcommit(
	limits: impl IntoIterator<Item = Limits>,
	ram_mb: u32,
	swap_mb: u32,
) -> Option<String> {
	let sum: u32 = limits.into_iter().map(|l| l.memory_max()).sum();
	if sum <= ram_mb {
		return None;
	}
	let swap = match swap_mb {
		0 => "no swap".to_string(),
		mb => format!("{} swap", size(mb)),
	};
	Some(format!(
		"Group memory limits add up to {}, the server has {} and {swap}",
		size(sum),
		size(ram_mb)
	))
}

/// The minute check's problem with the groups' memory limits. Defaults
/// that add up to more than the RAM are normal, groups rarely peak
/// together: only once a group has its own memory limit, or when one
/// group alone may take more than 90% of the RAM.
pub fn memory_problem(
	groups: &[(&str, Limits)],
	ram_mb: u32,
	swap_mb: u32,
) -> Option<String> {
	if let Some((name, l)) = groups
		.iter()
		.find(|(_, l)| l.memory_max() as u64 * 10 > ram_mb as u64 * 9)
	{
		return Some(format!(
			"The memory limit of {name} ({}) is more than 90% of the \
			server's {}",
			size(l.memory_max()),
			size(ram_mb)
		));
	}
	if !groups.iter().any(|(_, l)| l.memory_max_mb.is_some()) {
		return None;
	}
	overcommit(groups.iter().map(|(_, l)| *l), ram_mb, swap_mb)
}

/// A file in the drop-in folder of one of genco's units: systemd reads
/// it after genco's unit, its settings win. genco writes no drop-ins,
/// every one there is someone else's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DropIn {
	pub path: PathBuf,
	pub group: String,
	/// `genco-<group>.slice` or one of its services
	pub unit: String,
}

impl DropIn {
	pub fn is_slice(&self) -> bool {
		self.unit.ends_with(".slice")
	}

	pub fn text(&self) -> String {
		let path = self.path.display();
		if self.is_slice() {
			format!(
				"{path} overrides genco's limits of {}; move it into the \
				group's limits",
				self.group
			)
		} else {
			format!(
				"{path} overrides genco's unit {} of {}; move it into \
				genco's settings",
				self.unit, self.group
			)
		}
	}
}

/// The drop-ins among `paths` (files under /etc/systemd/system) that
/// change a unit of one of the `groups`.
pub fn foreign(paths: &[PathBuf], groups: &[&str]) -> Vec<DropIn> {
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
			// group names have no "-" and no "."
			let rest = unit.strip_prefix("genco-")?;
			let group = rest.split(['-', '.']).next()?;
			groups.contains(&group).then(|| DropIn {
				path: path.clone(),
				group: group.into(),
				unit: unit.into(),
			})
		})
		.collect();
	found.sort_by(|a, b| a.path.cmp(&b.path));
	found
}

/// Every file in the drop-in folders of genco's units.
pub fn dropin_files() -> Vec<PathBuf> {
	let mut files = vec![];
	let Ok(entries) = std::fs::read_dir(SYSTEMD) else {
		return files;
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
				let v = value.strip_suffix('%').ok_or_else(bad)?;
				limits.cpu_percent = Some(v.parse().map_err(|_| bad())?)
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

/// The drop-in files of a group's slice, in the order systemd reads them.
pub fn slice_dropins(group: &str) -> Vec<PathBuf> {
	let dir =
		Path::new(SYSTEMD).join(format!("{}.d", crate::render::slice(group)));
	let files: Vec<PathBuf> = std::fs::read_dir(&dir)
		.map(|e| e.flatten().map(|e| e.path()).collect())
		.unwrap_or_default();
	foreign(&files, &[group])
		.into_iter()
		.map(|d| d.path)
		.collect()
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
		let ok = |l: Limits| l.check(4000, 2).is_ok();
		assert!(ok(Limits::default()));
		let mem = |mb| Limits {
			memory_max_mb: Some(mb),
			..Limits::default()
		};
		assert!(ok(mem(128)));
		assert!(!ok(mem(100)));
		assert!(ok(mem(4000)));
		assert!(!ok(mem(4001)));
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
	fn limits_above_the_ram_warn() {
		let big = Limits {
			memory_max_mb: Some(2560),
			..Limits::default()
		};
		// 2.5 + 1.5 + 1.5 + 1 GB
		let all = [
			big,
			Limits::default(),
			Limits::default(),
			Limits {
				memory_max_mb: Some(1024),
				..Limits::default()
			},
		];
		assert_eq!(
			overcommit(all, 3993, 0).unwrap(),
			"Group memory limits add up to 6.5 GB, the server has 3.9 GB \
			and no swap"
		);
		assert!(
			overcommit(all, 3993, 2048)
				.unwrap()
				.ends_with("3.9 GB and 2 GB swap")
		);
		assert_eq!(overcommit(all, 8192, 0), None);
		assert_eq!(overcommit([], 1024, 0), None);
	}

	#[test]
	fn the_minute_check_ignores_defaults_that_add_up() {
		let d = Limits::default();
		let ten: Vec<(&str, Limits)> = (0..10).map(|_| ("g", d)).collect();
		// saving warns, the minute check doesn't
		assert!(overcommit(ten.iter().map(|(_, l)| *l), 3993, 0).is_some());
		assert_eq!(memory_problem(&ten, 3993, 0), None);

		// one group with its own limit makes the sum count
		let mut some = ten.clone();
		some[0].1.memory_max_mb = Some(2048);
		assert!(
			memory_problem(&some, 3993, 0)
				.unwrap()
				.starts_with("Group memory limits add up to 15.5 GB")
		);
		// its own limit within the RAM, the sum too
		assert_eq!(memory_problem(&some[..1], 3993, 0), None);

		// a single group above 90% of the RAM, also by default
		let big = Limits {
			memory_max_mb: Some(3700),
			..d
		};
		assert_eq!(
			memory_problem(&[("shop", big)], 3993, 0).unwrap(),
			"The memory limit of shop (3.6 GB) is more than 90% of the \
			server's 3.9 GB"
		);
		assert!(memory_problem(&[("tiny", d)], 1600, 0).is_some());
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
		]
		.into_iter()
		.map(PathBuf::from)
		.collect();
		let found = foreign(&paths, &["puresport", "shop"]);
		let units: Vec<(&str, &str)> = found
			.iter()
			.map(|d| (d.group.as_str(), d.unit.as_str()))
			.collect();
		assert_eq!(
			units,
			[
				("puresport", "genco-puresport.slice"),
				("shop", "genco-shop-php8.4.service"),
				("shop", "genco-shop-worker-q.service"),
			]
		);
		assert_eq!(
			found[0].text(),
			"/etc/systemd/system/genco-puresport.slice.d/memory.conf \
			overrides genco's limits of puresport; move it into the group's \
			limits"
		);
		assert!(found[1].text().contains("unit genco-shop-php8.4.service"));
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
