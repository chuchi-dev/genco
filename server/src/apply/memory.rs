//! Memory of the whole server: all groups share `genco.slice`, capped at
//! the RAM without the reserve for the system (memory.rs), so groups whose
//! own limits add up to more than the RAM run out among themselves. The
//! shared services get a low OOM score: when the kernel has to kill
//! outside the groups after all, it picks something else than the
//! databases, nginx or mail. Not genco: what it starts (the agent's
//! commands, imports and deploys as the group's user) would inherit it.

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
];
/// What they get, the kernel's scale goes from -1000 (never) to 1000.
const OOM_SCORE: i32 = -600;
const SYSTEM_SLICE: &str = "/sys/fs/cgroup/system.slice";

/// genco.slice and a drop-in with the OOM score for each protected service
/// that runs here.
pub(super) fn memory_files(cap_mb: u32) -> Vec<File> {
	let mut files = vec![File {
		path: render::unit_path(GROUPS_SLICE),
		content: crate::limits::groups_slice_unit(cap_mb),
		private_to: None,
	}];
	// a unit that sets a lower score itself (PostgreSQL's -900) keeps it
	for (unit, _) in protected_units()
		.into_iter()
		.filter(|(u, _)| own_score(u).is_none_or(|s| s > OOM_SCORE))
	{
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
pub(super) async fn memory(
	r: &mut Report,
	changed: &HashSet<PathBuf>,
	cap_mb: u32,
) {
	if changed.contains(&render::unit_path(GROUPS_SLICE)) {
		let cap = format!("MemoryMax={cap_mb}M");
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
		let score = match own_score(&unit) {
			Some(own) if own <= OOM_SCORE => {
				drop_old_dropin(r, &unit).await;
				own
			}
			_ => OOM_SCORE,
		};
		let procs = cgroup.join("cgroup.procs");
		let pids = std::fs::read_to_string(procs).unwrap_or_default();
		for pid in pids.lines() {
			let path = format!("/proc/{pid}/oom_score_adj");
			let now: Option<i32> = std::fs::read_to_string(&path)
				.ok()
				.and_then(|s| s.trim().parse().ok());
			if now.is_some_and(|n| n > score)
				&& let Err(e) = std::fs::write(&path, score.to_string())
			{
				r.info(format!("warning: OOM score of {unit} ({pid}): {e}"));
			}
		}
	}
}

/// The OOM score a unit's own file sets (its template's for an
/// instance like `postgresql@17-main.service`), not genco's drop-in.
fn own_score(unit: &str) -> Option<i32> {
	let name = match unit.split_once('@') {
		Some((prefix, _)) => format!("{prefix}@.service"),
		None => unit.to_string(),
	};
	let file = [
		"/etc/systemd/system",
		"/usr/lib/systemd/system",
		"/lib/systemd/system",
	]
	.iter()
	.find_map(|dir| std::fs::read_to_string(Path::new(dir).join(&name)).ok())?;
	score_in(&file)
}

/// The last `OOMScoreAdjust=` of a unit file.
fn score_in(file: &str) -> Option<i32> {
	file.lines()
		.rev()
		.find_map(|l| l.trim().strip_prefix("OOMScoreAdjust="))?
		.trim()
		.parse()
		.ok()
}

/// genco's drop-in of a unit that protects itself better, from before
/// genco looked: it would raise the unit's own score at its next start.
async fn drop_old_dropin(r: &mut Report, unit: &str) {
	let path = Path::new("/etc/systemd/system")
		.join(format!("{unit}.d"))
		.join("genco-memory.conf");
	if path.exists() && std::fs::remove_file(&path).is_ok() {
		r.run(&["systemctl", "daemon-reload"]).await;
		r.info(format!("{unit} keeps its own OOM score"));
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
		assert!(!protected("genco.service"));
		assert!(!protected("genco-shop-php8.4.service"));
		assert!(!protected("postgresql.service.d"));
		assert!(!protected("cron.service"));
	}

	#[test]
	fn a_unit_s_own_score_is_read() {
		let unit = "[Service]\nOOMScoreAdjust=-100\nType=forking\n\
			OOMScoreAdjust=-900\n";
		assert_eq!(score_in(unit), Some(-900));
		assert_eq!(score_in("[Service]\nType=simple\n"), None);
	}
}
