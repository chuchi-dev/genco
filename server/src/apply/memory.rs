//! Memory of the whole server: all groups share `genco.slice`, capped at
//! the RAM without a reserve for the system (limits.rs), so groups whose
//! own limits add up to more than the RAM run out among themselves. The
//! shared services get a low OOM score: when the kernel has to kill
//! outside the groups after all, it picks something else than the
//! databases, nginx, mail or genco.

use super::*;

pub(super) const GROUPS_SLICE: &str = "genco.slice";

/// The services the OOM killer spares, as units or their prefix (`@`).
const PROTECTED: &[&str] = &[
	"mariadb.service",
	"mysql.service",
	"postgresql@",
	"nginx.service",
	"postfix.service",
	"postfix@",
	"dovecot.service",
	"rspamd.service",
	"redis-server.service",
	"genco.service",
];
/// What they get, the kernel's scale goes from -1000 (never) to 1000.
const OOM_SCORE: i32 = -600;
const SYSTEM_SLICE: &str = "/sys/fs/cgroup/system.slice";

/// genco.slice and a drop-in with the OOM score for each protected service
/// that runs here.
pub(super) fn memory_files() -> Vec<File> {
	let (ram, _) = crate::limits::server_memory();
	let mut files = vec![File {
		path: render::unit_path(GROUPS_SLICE),
		content: crate::limits::groups_slice_unit(ram),
		private_to: None,
	}];
	for (unit, _) in protected_units() {
		files.push(File {
			path: Path::new("/etc/systemd/system")
				.join(format!("{unit}.d"))
				.join("genco-memory.conf"),
			content: format!(
				"{}\n[Service]\nOOMScoreAdjust={OOM_SCORE}\n",
				render::HEADER
			),
			private_to: None,
		});
	}
	files
}

/// The protected services running now and their cgroup, also in a
/// slice of system.slice (`system-postgresql.slice/postgresql@15-main`).
fn protected_units() -> Vec<(String, PathBuf)> {
	let mut units = vec![];
	let mut dirs = vec![PathBuf::from(SYSTEM_SLICE)];
	while let Some(dir) = dirs.pop() {
		let Ok(entries) = std::fs::read_dir(&dir) else {
			continue;
		};
		for e in entries.flatten() {
			let name = e.file_name().to_string_lossy().to_string();
			if protected(&name) {
				units.push((name, e.path()));
			} else if name.ends_with(".slice") && dir == Path::new(SYSTEM_SLICE)
			{
				dirs.push(e.path());
			}
		}
	}
	units.sort();
	units
}

fn protected(unit: &str) -> bool {
	PROTECTED.iter().any(|p| match p.strip_suffix('@') {
		Some(prefix) => {
			unit.starts_with(&format!("{prefix}@"))
				&& unit.ends_with(".service")
		}
		None => unit == *p,
	})
}

/// The cap on the running slice, and the OOM score of the protected
/// services' processes right away (the drop-in counts from their next
/// start).
pub(super) async fn memory(r: &mut Report, changed: &HashSet<PathBuf>) {
	if changed.contains(&render::unit_path(GROUPS_SLICE)) {
		let (ram, _) = crate::limits::server_memory();
		let cap = format!("MemoryMax={}M", crate::limits::groups_cap(ram));
		if r.run(&[
			"systemctl",
			"set-property",
			"--runtime",
			GROUPS_SLICE,
			&cap,
		])
		.await
		{
			r.info(format!("all groups together: {cap}"));
		}
	}
	for (unit, cgroup) in protected_units() {
		let procs = cgroup.join("cgroup.procs");
		let pids = std::fs::read_to_string(procs).unwrap_or_default();
		for pid in pids.lines() {
			let path = format!("/proc/{pid}/oom_score_adj");
			let now: Option<i32> = std::fs::read_to_string(&path)
				.ok()
				.and_then(|s| s.trim().parse().ok());
			if now.is_some_and(|n| n > OOM_SCORE)
				&& let Err(e) = std::fs::write(&path, OOM_SCORE.to_string())
			{
				r.info(format!("warning: OOM score of {unit} ({pid}): {e}"));
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_shared_services_are_protected() {
		assert!(protected("mariadb.service"));
		assert!(protected("postgresql@15-main.service"));
		assert!(protected("postfix@-.service"));
		assert!(protected("postfix.service"));
		assert!(protected("genco.service"));
		assert!(!protected("genco-shop-php8.4.service"));
		assert!(!protected("postgresql.service.d"));
		assert!(!protected("cron.service"));
	}
}
