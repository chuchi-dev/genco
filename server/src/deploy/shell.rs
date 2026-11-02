//! Commands as the group's user, in a sandbox like its services.

use super::*;

pub(super) fn sh_quote(s: &str) -> String {
	format!("'{}'", s.replace('\'', r"'\''"))
}

/// Runs commands as the group's user in a sandbox like its services, with
/// its deploy key for git and the PHP version of its domains.
pub(super) struct GroupShell {
	pub(super) user: String,
	pub(super) slice: String,
	pub(super) dir: PathBuf,
	pub(super) php: Option<String>,
}

impl GroupShell {
	pub(super) fn new(group: &Group) -> Self {
		let php = group
			.domains
			.iter()
			.filter(|d| d.kind == Kind::Php)
			.find_map(|d| d.php_version.clone());
		Self {
			user: render::group_user(&group.name),
			slice: render::slice(&group.name),
			dir: render::group_dir(&group.name),
			php,
		}
	}

	/// `cmd` in a sandbox, stopped after `minutes`.
	pub(super) fn command(
		&self,
		cwd: &Path,
		cmd: &str,
		minutes: u32,
	) -> Command {
		let dir = self.dir.display();
		let mut path = String::new();
		if let Some(v) = &self.php {
			path.push_str(&format!("{}:", php_bin_dir(v)));
		}
		path.push_str("/usr/local/bin:/usr/bin:/bin");

		let mut c = Command::new("systemd-run");
		c.args(["--quiet", "--wait", "--pipe", "--collect"])
			.arg(format!("--uid={}", self.user))
			.arg(format!("--gid={}", self.user))
			.arg(format!("--slice={}", self.slice))
			.arg(format!("--working-directory={}", cwd.display()))
			.arg(format!("--setenv=HOME={dir}"))
			.arg(format!("--setenv=PATH={path}"))
			.arg(format!(
				"--setenv=GIT_SSH_COMMAND=ssh -i {dir}/.ssh/deploy_key \
				-o UserKnownHostsFile={dir}/.ssh/known_hosts \
				-o StrictHostKeyChecking=accept-new"
			))
			.args([
				"-p",
				"UMask=0027",
				"-p",
				"NoNewPrivileges=yes",
				"-p",
				"ProtectSystem=strict",
				"-p",
				"ProtectHome=yes",
				"-p",
				"PrivateTmp=yes",
				"-p",
				"ProtectProc=invisible",
				// like the group's services, see render::sandbox
				"-p",
				"SocketBindDeny=any",
			])
			// a step that never ends would hold every group's deploys
			.arg("-p")
			.arg(format!("RuntimeMaxSec={minutes}min"))
			.arg("-p")
			.arg(format!("ReadWritePaths={dir}"))
			.arg("-p")
			.arg(format!("InaccessiblePaths={}", render::HIDDEN.join(" ")))
			.args(["bash", "-c", cmd])
			.stdin(Stdio::null());
		c
	}

	/// Runs `cmd`, its output goes to the log.
	pub(super) async fn run(
		&self,
		log: &mut Log,
		cwd: &Path,
		cmd: &str,
	) -> Res<()> {
		self.run_for(log, cwd, cmd, 60).await
	}

	/// `run` with another time limit.
	pub(super) async fn run_for(
		&self,
		log: &mut Log,
		cwd: &Path,
		cmd: &str,
		minutes: u32,
	) -> Res<()> {
		let started = std::time::Instant::now();
		let res = log
			.run(self.command(cwd, &format!("exec 2>&1; {cmd}"), minutes))
			.await;
		res.map_err(|e| {
			let first = cmd.lines().next().unwrap_or("");
			if started.elapsed().as_secs() >= u64::from(minutes) * 60 {
				format!("{first}: stopped after {minutes} minutes")
			} else {
				format!("{first}: {e}")
			}
		})
	}

	/// stdout of `cmd`, trimmed
	pub(super) async fn output(&self, cwd: &Path, cmd: &str) -> Res<String> {
		let out = self
			.command(cwd, cmd, 60)
			.output()
			.await
			.map_err(|e| e.to_string())?;
		if !out.status.success() {
			return Err(String::from_utf8_lossy(&out.stderr).into());
		}
		Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
	}
}

/// `php` there is the given version, `genco setup` creates them.
pub fn php_bin_dir(version: &str) -> String {
	format!("/usr/local/lib/genco/php{version}")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn quotes_for_the_shell() {
		assert_eq!(sh_quote("it's"), r"'it'\''s'");
	}
}
