//! What a release builds and keeps: `genco.toml` in the repository, or
//! the steps `detect` finds without one. It comes from the repository,
//! so everything in it is checked before it runs.

use super::*;

/// `genco.toml` in the repository, or what `detect` finds without one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DeployConfig {
	/// files and directories that stay from release to release, kept in
	/// `shared/`
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub shared: Vec<String>,
	/// before the switch, a failing step stops the deploy
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub build: Vec<Step>,
	/// after the switch, before the apps restart
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub after_switch: Vec<Step>,
	/// commands that run all the time, restarted with every deploy
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub worker: Vec<WorkerCfg>,
	/// commands that run at times
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub schedule: Vec<ScheduleCfg>,
	/// what detection couldn't decide, for the deploy log
	#[serde(skip)]
	pub hints: Vec<String>,
}

/// `[[worker]]` in genco.toml.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerCfg {
	pub name: String,
	/// relative to the release
	#[serde(default = "default_dir")]
	pub dir: String,
	pub run: String,
	/// a port of its own, open to the internet
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub port: Option<u16>,
}

/// `[[schedule]]` in genco.toml.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleCfg {
	pub name: String,
	#[serde(default = "default_dir")]
	pub dir: String,
	pub run: String,
	/// systemd's OnCalendar: hourly, *:0/5, Mon *-*-* 03:00
	pub every: String,
	/// minutes, 60 by default
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub timeout: Option<u32>,
}

/// A release's folder as seen from the group's directory: they run in
/// whatever `current` is.
fn in_current(dir: &str) -> String {
	if dir == "." {
		"current".into()
	} else {
		format!("current/{dir}")
	}
}

impl DeployConfig {
	/// The workers and schedules for `procs::sync_repo`, checked.
	pub(super) fn procs(&self) -> Res<(Vec<Worker>, Vec<Schedule>)> {
		let workers: Vec<Worker> = self
			.worker
			.iter()
			.map(|w| Worker {
				id: 0,
				group_id: 0,
				name: w.name.clone(),
				dir: in_current(&w.dir),
				command: w.run.clone(),
				port: w.port,
				address: String::new(),
				source: String::new(),
				state: None,
			})
			.collect();
		let schedules: Vec<Schedule> = self
			.schedule
			.iter()
			.map(|s| Schedule {
				id: 0,
				group_id: 0,
				name: s.name.clone(),
				dir: in_current(&s.dir),
				command: s.run.clone(),
				every: s.every.clone(),
				timeout: s.timeout.unwrap_or(60),
				source: String::new(),
			})
			.collect();
		for w in &workers {
			check_path_or_dot(&w.dir)?;
			w.check().map_err(|e| format!("genco.toml: {e}"))?;
		}
		for s in &schedules {
			s.check().map_err(|e| format!("genco.toml: {e}"))?;
		}
		Ok((workers, schedules))
	}
}

fn check_path_or_dot(dir: &str) -> Res<()> {
	match dir.strip_prefix("current/") {
		Some(rest) => check_path(rest),
		None => Ok(()),
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
	/// relative to the release
	#[serde(default = "default_dir")]
	pub dir: String,
	pub run: String,
	/// `runner`: on a build runner in a container, see runners.rs; here
	/// if none is online
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub on: Option<String>,
	/// the runner's image, `{codename}` is this server's Debian release;
	/// by default rust, node or debian for what `dir` holds
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub image: Option<String>,
	/// files and folders the runner sends back into the release, relative
	/// to `dir`
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub artifacts: Vec<String>,
	/// minutes, 60 by default
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub timeout: Option<u32>,
}

impl Step {
	pub(super) fn on_runner(&self) -> bool {
		self.on.as_deref() == Some("runner")
	}

	pub(super) fn minutes(&self) -> u32 {
		self.timeout.unwrap_or(60)
	}

	/// What genco.toml may say.
	pub(super) fn check(&self) -> Res<()> {
		if !matches!(self.on.as_deref(), None | Some("server" | "runner")) {
			return Err("on is server or runner".into());
		}
		if self.dir != "." {
			check_path(&self.dir)?;
		}
		for a in &self.artifacts {
			check_path(a)?;
		}
		if !self.artifacts.is_empty() && !self.on_runner() {
			return Err("artifacts only come from a runner".into());
		}
		if let Some(i) = &self.image {
			crate::runners::check_image(i)?;
		}
		if !(1..=600).contains(&self.minutes()) {
			return Err("a timeout is 1 to 600 minutes".into());
		}
		Ok(())
	}
}

pub(super) fn default_dir() -> String {
	".".into()
}

pub(super) fn step(dir: &str, run: &str) -> Step {
	Step {
		dir: dir.into(),
		run: run.into(),
		on: None,
		image: None,
		artifacts: vec![],
		timeout: None,
	}
}

/// Without a genco.toml: composer, Craft, npm and Cargo projects in the
/// release or one directory below (craft/ and svelte/ of a crelte
/// project). `shared` is the group's shared folder, which tells which
/// optional shared files are there; Cargo builds on a runner.
pub(super) fn detect(release: &Path, shared: &Path) -> DeployConfig {
	let shared_exists = |p: &str| shared.join(p).exists();
	let mut dirs = vec![".".to_string()];
	if let Ok(entries) = std::fs::read_dir(release) {
		let mut subs: Vec<String> = entries
			.flatten()
			.filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
			.map(|e| e.file_name().to_string_lossy().to_string())
			.filter(|n| !n.starts_with('.'))
			.collect();
		subs.sort();
		dirs.extend(subs);
	}

	let mut cfg = DeployConfig::default();
	// schedules whose need the environment decides at runtime
	let mut unsure = vec![];
	for dir in dirs {
		let path = release.join(&dir);
		let rel = |p: &str| {
			if dir == "." {
				p.to_string()
			} else {
				format!("{dir}/{p}")
			}
		};

		if is_file(&path.join("composer.json")) {
			cfg.build.push(step(
				&dir,
				"composer install --no-dev --no-interaction --prefer-dist \
				--optimize-autoloader",
			));
			if is_file(&path.join("craft")) {
				for p in [".env", "storage", "web/assets", "web/cpresources"] {
					cfg.shared.push(rel(p));
				}
				cfg.after_switch
					.push(step(&dir, "php craft up --interactive=0"));
				// the queue runs beside the site, not in its requests
				cfg.worker.push(WorkerCfg {
					name: "craft-queue".into(),
					dir: dir.clone(),
					run: "php craft queue/listen --verbose".into(),
					port: None,
				});
				// scheduled and expired entries change their status, only
				// with staticStatuses
				let statuses = ScheduleCfg {
					name: "craft-statuses".into(),
					dir: dir.clone(),
					run: "php craft update-statuses".into(),
					every: "*:0/5".into(),
					timeout: Some(10),
				};
				if craft_version(&path.join("composer.lock")) >= Some((5, 7)) {
					match static_statuses(
						&read_small(&path.join("config/general.php")),
						&read_small(&shared.join(rel(".env"))),
					) {
						Statuses::On => cfg.schedule.push(statuses),
						Statuses::Off => {}
						Statuses::Unsure => unsure.push(statuses),
					}
				}
			}
		}

		if is_file(&path.join("package.json")) {
			let install = if is_file(&path.join("package-lock.json")) {
				"npm ci"
			} else {
				"npm install"
			};
			let pkg = read_small(&path.join("package.json"));
			let build = serde_json::from_str::<serde_json::Value>(&pkg)
				.ok()
				.and_then(|v| v.get("scripts")?.get("build").cloned())
				.is_some();
			let run = if build {
				format!("{install} && npm run build")
			} else {
				install.to_string()
			};
			cfg.build.push(step(&dir, &run));
			if shared_exists(&rel(".env")) {
				cfg.shared.push(rel(".env"));
			}
		}

		if let Some(bin) = cargo_binary(&path.join("Cargo.toml")) {
			cfg.build.push(cargo_step(&dir, &bin));
			if shared_exists(&rel(".env")) && !cfg.shared.contains(&rel(".env"))
			{
				cfg.shared.push(rel(".env"));
			}
		}
	}

	// the admins decide with a genco.toml, which turns detection off:
	// it lists everything detected plus the schedule
	if !unsure.is_empty() {
		let with = DeployConfig {
			schedule: cfg.schedule.iter().chain(&unsure).cloned().collect(),
			..cfg.clone()
		};
		let toml = toml::to_string(&with).unwrap_or_default();
		for s in &unsure {
			cfg.hints.push(format!(
				"{}/config/general.php takes staticStatuses from the \
				environment, so genco doesn't know whether `{}` is needed \
				and left the schedule {} out. If staticStatuses is on, \
				commit this genco.toml (everything detected plus the \
				schedule):\n\n{toml}",
				s.dir, s.run, s.name
			));
		}
	}
	cfg
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Statuses {
	On,
	Off,
	/// general.php takes it from the environment
	Unsure,
}

/// Whether Craft's `staticStatuses` is on: general.php sets it (anything
/// but false) or the .env has CRAFT_STATIC_STATUSES=true. Without it
/// `craft update-statuses` does nothing. Comments don't count.
pub(super) fn static_statuses(general: &str, env: &str) -> Statuses {
	let mut found = Statuses::Off;
	for line in general.lines() {
		let code = line.trim_start();
		if code.starts_with("//")
			|| code.starts_with('*')
			|| code.starts_with('#')
		{
			continue;
		}
		let Some(i) = code.find("staticStatuses") else {
			continue;
		};
		// the value: up to the end of the call or of the array item
		let value = code[i + "staticStatuses".len()..]
			.chars()
			.take_while(|c| *c != ';' && *c != ',')
			.collect::<String>()
			.to_lowercase();
		if value.contains("false") {
			continue;
		}
		if value.contains("env") {
			found = Statuses::Unsure;
		} else {
			return Statuses::On;
		}
	}
	let in_env = env.lines().any(|l| {
		let Some((key, value)) = l.split_once('=') else {
			return false;
		};
		key.trim() == "CRAFT_STATIC_STATUSES"
			&& matches!(
				value
					.trim()
					.trim_matches(['"', '\''])
					.to_lowercase()
					.as_str(),
				"true" | "1"
			)
	});
	if in_env { Statuses::On } else { found }
}

/// Craft's major and minor version from composer.lock.
pub(super) fn craft_version(lock: &Path) -> Option<(u32, u32)> {
	let small = std::fs::symlink_metadata(lock)
		.is_ok_and(|m| m.file_type().is_file() && m.len() <= 8 * 1024 * 1024);
	if !small {
		return None;
	}
	let lock: serde_json::Value =
		serde_json::from_str(&std::fs::read_to_string(lock).ok()?).ok()?;
	let version = lock
		.get("packages")?
		.as_array()?
		.iter()
		.find(|p| {
			p.get("name").and_then(|n| n.as_str()) == Some("craftcms/cms")
		})?
		.get("version")?
		.as_str()?;
	let mut parts = version.trim_start_matches('v').split('.');
	Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// The binary of a Cargo package (not a workspace), if its name is plain.
pub(super) fn cargo_binary(path: &Path) -> Option<String> {
	let manifest: toml::Value = toml::from_str(&read_small(path)).ok()?;
	let name = manifest
		.get("bin")
		.and_then(|b| b.as_array()?.first()?.get("name")?.as_str())
		.or_else(|| manifest.get("package")?.get("name")?.as_str())?;
	let plain = !name.is_empty()
		&& name
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || "-_".contains(c));
	plain.then(|| name.to_string())
}

/// `cargo build` on a runner, the binary comes back into the step's
/// folder (here, if no runner is online).
pub(super) fn cargo_step(dir: &str, bin: &str) -> Step {
	let mut s = step(
		dir,
		&format!("cargo build --release --locked && cp target/release/{bin} ."),
	);
	s.on = Some("runner".into());
	s.artifacts = vec![bin.into()];
	s
}

/// A regular file, not a link: `detect` runs as root on what a repository
/// brings, a link to /dev/zero or a fifo must not hang or fill genco.
pub(super) fn is_file(path: &Path) -> bool {
	std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// A regular file up to 1 MB, else empty.
/// Read as root: no link as the file itself, nothing that blocks (a FIFO
/// the group put in shared/), at most 1 MiB, checked on the open file.
pub(super) fn read_small(path: &Path) -> String {
	use std::io::Read as _;
	use std::os::unix::fs::OpenOptionsExt as _;
	let Ok(file) = std::fs::OpenOptions::new()
		.read(true)
		.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
		.open(path)
	else {
		return String::new();
	};
	let small = file
		.metadata()
		.is_ok_and(|m| m.file_type().is_file() && m.len() <= 1024 * 1024);
	if !small {
		return String::new();
	}
	let mut s = String::new();
	let _ = file.take(1024 * 1024).read_to_string(&mut s);
	s
}

/// genco.toml is read as the group's user: a link in the repository to a
/// file only root may read stays unreadable. The detection reads as root,
/// but only decides on steps, nothing of it is shown.
pub(super) async fn config(
	shell: &GroupShell,
	release: &Path,
	base: &Path,
) -> Res<DeployConfig> {
	let toml = shell
		// a regular file and only the start: a link to /dev/zero or a huge
		// file would fill genco's memory, which reads the output
		.output(
			release,
			"[ -f genco.toml ] || exit 3; head -c 65536 genco.toml",
		)
		.await;
	match toml {
		Ok(ctn) => toml::from_str(&ctn).map_err(|e| format!("genco.toml: {e}")),
		Err(_) => Ok(detect(release, &base.join("shared"))),
	}
}

/// Shared paths stay inside the release.
pub(super) fn check_path(p: &str) -> Res<()> {
	if p.is_empty()
		|| p.starts_with('/')
		|| p.split('/').any(|c| c == ".." || c == ".")
	{
		return Err(format!("{p} is no path inside the release"));
	}
	Ok(())
}

/// `.env` or `config.json`, not `storage`
pub(super) fn looks_like_file(p: &str) -> bool {
	let name = p.rsplit('/').next().unwrap_or(p);
	name.starts_with('.') || name.contains('.')
}

#[cfg(test)]
mod tests {
	use super::*;

	fn tmp(name: &str) -> PathBuf {
		let dir =
			std::env::temp_dir().join(format!("genco-test-{name}-{}", now()));
		std::fs::create_dir_all(&dir).unwrap();
		dir
	}

	#[test]
	fn detects_a_crelte_project() {
		let dir = tmp("crelte");
		std::fs::create_dir_all(dir.join("craft")).unwrap();
		std::fs::write(dir.join("craft/composer.json"), "{}").unwrap();
		std::fs::write(
			dir.join("craft/composer.lock"),
			r#"{"packages":[{"name":"x/y","version":"1.0"},
			{"name":"craftcms/cms","version":"5.8.3"}]}"#,
		)
		.unwrap();
		std::fs::write(dir.join("craft/craft"), "").unwrap();
		std::fs::create_dir_all(dir.join("svelte")).unwrap();
		std::fs::write(
			dir.join("svelte/package.json"),
			r#"{"scripts":{"build":"vite build"}}"#,
		)
		.unwrap();
		std::fs::write(dir.join("svelte/package-lock.json"), "{}").unwrap();

		let shared = tmp("crelte-shared");
		std::fs::create_dir_all(shared.join("svelte")).unwrap();
		std::fs::write(shared.join("svelte/.env"), "").unwrap();
		std::fs::create_dir_all(dir.join("craft/config")).unwrap();
		std::fs::write(
			dir.join("craft/config/general.php"),
			"return GeneralConfig::create()->staticStatuses();",
		)
		.unwrap();

		let cfg = detect(&dir, &shared);
		assert_eq!(
			cfg.shared,
			[
				"craft/.env",
				"craft/storage",
				"craft/web/assets",
				"craft/web/cpresources",
				"svelte/.env"
			]
		);
		assert_eq!(cfg.build[0].dir, "craft");
		assert!(cfg.build[0].run.starts_with("composer install --no-dev"));
		assert_eq!(cfg.build[1], step("svelte", "npm ci && npm run build"));
		assert_eq!(
			cfg.after_switch,
			[step("craft", "php craft up --interactive=0")]
		);
		let (workers, schedules) = cfg.procs().unwrap();
		assert_eq!(workers[0].dir, "current/craft");
		assert_eq!(workers[0].command, "php craft queue/listen --verbose");
		assert_eq!(schedules[0].command, "php craft update-statuses");

		// from the environment: no schedule, a genco.toml in the log
		std::fs::write(
			dir.join("craft/config/general.php"),
			r#"->staticStatuses(App::env("CACHING") === true);"#,
		)
		.unwrap();
		let cfg = detect(&dir, &shared);
		assert!(cfg.schedule.is_empty());
		let (_, toml) = cfg.hints[0].split_once("\n\n").unwrap();
		let with: DeployConfig = toml::from_str(toml).unwrap();
		assert_eq!(with.schedule[0].name, "craft-statuses");
		assert_eq!(with.build, cfg.build);
		assert_eq!(with.shared, cfg.shared);

		// without staticStatuses update-statuses does nothing
		std::fs::write(dir.join("craft/config/general.php"), "").unwrap();
		assert!(detect(&dir, &shared).schedule.is_empty());

		// update-statuses came with Craft 5.7
		std::fs::write(
			dir.join("craft/composer.lock"),
			r#"{"packages":[{"name":"craftcms/cms","version":"5.6.0"}]}"#,
		)
		.unwrap();
		assert!(detect(&dir, &shared).schedule.is_empty());
		std::fs::remove_dir_all(dir).unwrap();
		std::fs::remove_dir_all(shared).unwrap();
	}

	#[test]
	fn static_statuses_from_config_or_env() {
		use Statuses::*;
		let s = static_statuses;
		assert_eq!(s("'staticStatuses' => true,", ""), On);
		assert_eq!(s("->staticStatuses()", ""), On);
		assert_eq!(s("'staticStatuses' => false,", ""), Off);
		assert_eq!(s("->staticStatuses(false)", ""), Off);
		assert_eq!(s("", ""), Off);
		assert_eq!(s("// enable staticStatuses if CACHING", ""), Off);
		let env = r#"->staticStatuses(App::env("CACHING") === true);"#;
		assert_eq!(s(env, ""), Unsure);
		assert_eq!(s(env, "CRAFT_STATIC_STATUSES=true"), On);
		assert_eq!(s("", "CRAFT_STATIC_STATUSES=true\n"), On);
		assert_eq!(s("", "CRAFT_STATIC_STATUSES=\"true\""), On);
		assert_eq!(s("", "CRAFT_STATIC_STATUSES=false"), Off);
		assert_eq!(s("", "#CRAFT_STATIC_STATUSES"), Off);
	}

	#[test]
	fn genco_toml() {
		let cfg: DeployConfig = toml::from_str(
			r#"
			shared = [".env"]
			[[build]]
			run = "cargo build"
			[[after-switch]]
			dir = "app"
			run = "./migrate"
			[[worker]]
			name = "packages"
			run = "./packages-server"
			port = 5426
			[[schedule]]
			name = "cleanup"
			dir = "app"
			run = "./cleanup"
			every = "daily"
			"#,
		)
		.unwrap();
		assert_eq!(cfg.build, [step(".", "cargo build")]);
		assert_eq!(cfg.after_switch, [step("app", "./migrate")]);
		let (workers, schedules) = cfg.procs().unwrap();
		assert_eq!(workers[0].dir, "current");
		assert_eq!(workers[0].port, Some(5426));
		assert_eq!(schedules[0].dir, "current/app");
		assert_eq!(schedules[0].timeout, 60);
		let bad: DeployConfig = toml::from_str(
			"[[worker]]\nname = \"q\"\ndir = \"../x\"\nrun = \"x\"",
		)
		.unwrap();
		assert!(bad.procs().is_err());
		assert!(toml::from_str::<DeployConfig>("unknown = 1").is_err());
	}

	#[test]
	fn shared_paths_stay_inside() {
		assert!(check_path("craft/.env").is_ok());
		assert!(check_path("../x").is_err());
		assert!(check_path("/etc").is_err());
		assert!(looks_like_file("craft/.env"));
		assert!(looks_like_file("config.json"));
		assert!(!looks_like_file("craft/storage"));
	}
}
