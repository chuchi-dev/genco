//! Small helpers more modules need.

use std::path::Path;

use tokio::process::Command;

/// The dump of one database with what MariaDB needs to restore it.
pub const MARIADB_DUMP: &str =
	"mariadb-dump --single-transaction --routines --triggers --events";

/// Runs a bash script, its trimmed stdout or its stderr as error.
pub async fn sh(script: &str) -> Result<String, String> {
	let out = Command::new("bash")
		.args(["-c", script])
		.output()
		.await
		.map_err(|e| format!("bash: {e}"))?;
	if !out.status.success() {
		let err = String::from_utf8_lossy(&out.stderr);
		return Err(format!("{script} failed: {}", err.trim()));
	}
	Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Runs `args` (no shell): its trimmed stdout, or `"<args> failed:
/// <stderr>"`.
pub async fn run(args: &[&str]) -> Result<String, String> {
	let out = Command::new(args[0])
		.args(&args[1..])
		.output()
		.await
		.map_err(|e| format!("{}: {e}", args[0]))?;
	if !out.status.success() {
		return Err(format!(
			"{} failed: {}",
			args.join(" "),
			String::from_utf8_lossy(&out.stderr).trim()
		));
	}
	Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// how long an upload may send nothing
pub const UPLOAD_STALL: std::time::Duration =
	std::time::Duration::from_secs(60);

/// Writes an upload to `path`; a client that sends nothing for a minute
/// or breaks off is an error, and what arrived goes.
/// More than `max` bytes is an error too.
pub async fn body_to_file(
	body: axum::body::Body,
	path: &Path,
	max: u64,
) -> std::io::Result<()> {
	use futures::StreamExt as _;
	use tokio::io::AsyncWriteExt as _;

	let written = async {
		let mut file = tokio::fs::File::create(path).await?;
		let mut stream = body.into_data_stream();
		let mut size = 0;
		loop {
			let next = tokio::time::timeout(UPLOAD_STALL, stream.next())
				.await
				.map_err(|_| {
					std::io::Error::other("nothing came for a minute")
				})?;
			let Some(chunk) = next else { break };
			let chunk = chunk.map_err(std::io::Error::other)?;
			size += chunk.len() as u64;
			if size > max {
				return Err(std::io::Error::other(format!(
					"bigger than {} MB",
					max >> 20
				)));
			}
			file.write_all(&chunk).await?;
		}
		file.flush().await
	}
	.await;
	if written.is_err() {
		let _ = std::fs::remove_file(path);
	}
	written
}

/// The local time like 20261101-075939, for releases, dumps and archives.
pub async fn stamp() -> String {
	sh("date +%Y%m%d-%H%M%S")
		.await
		.unwrap_or_else(|_| crate::db::now().to_string())
}

/// A time in milliseconds as local date and time, for the terminal.
pub async fn format_time(ms: i64) -> String {
	sh(&format!("date -d @{} '+%F %T'", ms / 1000))
		.await
		.unwrap_or_else(|_| ms.to_string())
}

/// The items `keep` picks, copied: the children of one parent in a tree.
pub fn matching<T: Clone>(items: &[T], keep: impl Fn(&T) -> bool) -> Vec<T> {
	items.iter().filter(|x| keep(x)).cloned().collect()
}

/// The first line of `text`, cut at `max` characters with "...", for
/// titles and subjects.
pub fn first_line(text: &str, max: usize) -> String {
	let line = text.lines().next().unwrap_or("");
	if line.chars().count() > max {
		line.chars().take(max - 3).collect::<String>() + "..."
	} else {
		line.into()
	}
}

/// Whether genco runs as root, some commands need it.
pub async fn is_root() -> bool {
	sh("id -u").await.is_ok_and(|u| u == "0")
}

/// `n` random bytes as hex, for tokens.
pub fn random_hex(n: usize) -> String {
	(0..n)
		.map(|_| format!("{:02x}", rand::random::<u8>()))
		.collect()
}

pub fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
	use std::os::unix::fs::PermissionsExt as _;
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// A directory only root may enter, with its parents.
pub fn private_dir(path: &Path) -> std::io::Result<()> {
	std::fs::create_dir_all(path)?;
	set_mode(path, 0o700)
}

/// Reloads nginx and checks that its master took the config: a reload
/// only signals the master, which keeps the old config when it can't load
/// the new one, e.g. an older running binary than the one `nginx -t`
/// tested. Then a restart brings the new one. True when it restarted.
pub async fn reload_nginx() -> Result<bool, String> {
	// without a master there is nothing to wait for
	let Some(before) = nginx_workers() else {
		run(&["systemctl", "reload-or-restart", "nginx"]).await?;
		return Ok(false);
	};
	run(&["systemctl", "reload", "nginx"]).await?;

	// the master starts new workers for a new config
	for _ in 0..50 {
		if nginx_workers().is_some_and(|after| reloaded(&before, &after)) {
			return Ok(false);
		}
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	}
	tracing::warn!("nginx kept its old config on reload, restarting it");
	if let Err(e) = run(&["systemctl", "restart", "nginx"]).await {
		// a failed restart leaves nginx stopped and every site down: one
		// more start, the error stays either way
		return Err(match run(&["systemctl", "start", "nginx"]).await {
			Ok(_) => format!("restarting nginx failed, started it again: {e}"),
			Err(again) => {
				format!(
					"nginx is down, restarting failed: {e}, starting: {again}"
				)
			}
		});
	}
	Ok(true)
}

/// The processes of nginx's master, None without a running master: a
/// pid file left from a crash may name no process or another one.
fn nginx_workers() -> Option<Vec<u32>> {
	let pid = std::fs::read_to_string("/run/nginx.pid").ok()?;
	let pid = pid.trim();
	let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
	if comm.trim() != "nginx" {
		return None;
	}
	let children =
		std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
			.ok()?;
	Some(
		children
			.split_whitespace()
			.filter_map(|p| p.parse().ok())
			.collect(),
	)
}

/// A new process means the master loaded the config again.
fn reloaded(before: &[u32], after: &[u32]) -> bool {
	after.iter().any(|p| !before.contains(p))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_reload_shows_in_new_nginx_processes() {
		assert!(reloaded(&[10, 11], &[10, 11, 12]));
		// old workers still serving their connections
		assert!(reloaded(&[10, 11], &[11, 13]));
		assert!(!reloaded(&[10, 11], &[10, 11]));
		assert!(!reloaded(&[10, 11], &[10]));
	}

	#[test]
	fn random_hex_has_twice_the_length() {
		let h = random_hex(16);
		assert_eq!(h.len(), 32);
		assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
		assert_ne!(h, random_hex(16));
	}
}
