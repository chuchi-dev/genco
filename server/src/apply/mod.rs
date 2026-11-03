//! Brings the server in line with the database: users, directories,
//! config files, units, firewall, databases and certificates. Safe to run
//! again, it only restarts what changed.

mod group;
mod mail;
mod system;

use std::collections::HashSet;
use std::os::unix::fs::chown;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio::process::Command;

use crate::config::Config;
use crate::db::Db;
use crate::host;
use crate::hosting::{Engine, Group, Kind};
use crate::mailbox;
use crate::mailfilter;
use crate::render::{self, File};
use crate::util::set_mode;
use crate::webmail;

use group::*;
use mail::*;
use system::*;

/// One apply at a time, the panel, the agent and the CLI may all start one;
/// a deploy that switches an app's slot holds it too.
pub const LOCK: &str = "/var/lib/genco/apply.lock";

/// Units of genco itself, `genco setup` writes them, they belong to no
/// group. Their names are reserved, see `hosting::RESERVED`.
const SYSTEM_UNITS: &[&str] = &["genco-backup.service", "genco-backup.timer"];

#[derive(Debug, Serialize)]
pub struct Report {
	pub ok: bool,
	pub log: Vec<String>,
}

impl Report {
	/// ok until an error
	fn new() -> Self {
		Self {
			ok: true,
			log: vec![],
		}
	}

	fn info(&mut self, line: impl Into<String>) {
		let line = line.into();
		tracing::info!("apply: {line}");
		self.log.push(line);
	}

	fn error(&mut self, line: impl Into<String>) {
		let line = format!("error: {}", line.into());
		tracing::error!("apply: {line}");
		self.log.push(line);
		self.ok = false;
	}

	/// Runs a command, logs it and its output if it fails.
	async fn run(&mut self, args: &[&str]) -> bool {
		self.run_input(args, None).await
	}

	async fn run_input(&mut self, args: &[&str], stdin: Option<&str>) -> bool {
		let mut cmd = Command::new(args[0]);
		cmd.args(&args[1..]);
		let out = match stdin {
			Some(input) => output_with_stdin(cmd, input).await,
			None => cmd.output().await,
		};
		match out {
			Ok(out) if out.status.success() => true,
			Ok(out) => {
				let err = String::from_utf8_lossy(&out.stderr);
				let err = err.trim();
				self.error(format!("{} failed: {err}", args.join(" ")));
				false
			}
			Err(e) => {
				self.error(format!("{} failed: {e}", args[0]));
				false
			}
		}
	}
}

async fn output_with_stdin(
	mut cmd: Command,
	input: &str,
) -> std::io::Result<std::process::Output> {
	use tokio::io::AsyncWriteExt as _;

	let mut child = cmd
		.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::piped())
		.spawn()?;
	child
		.stdin
		.take()
		.unwrap()
		.write_all(input.as_bytes())
		.await?;
	child.wait_with_output().await
}

/// Takes the lock, then reads the groups: an apply that waited for
/// another one works with what is in the database now.
async fn locked(
	r: &mut Report,
	cfg: &Config,
	db: &Db,
) -> Option<(std::fs::File, Vec<Group>)> {
	let lock = match crate::job::lock(LOCK).await {
		Ok(lock) => lock,
		Err(e) => {
			r.error(e);
			return None;
		}
	};
	if let Err(e) = crate::mailbox::ensure_smtp_logins(db).await {
		r.error(format!("SMTP logins: {e}"));
	}
	let host = mailbox::host(cfg).filter(|h| host::has_cert(h));
	match crate::hosting::groups(&db.conn()) {
		Ok(mut groups) => {
			for g in &mut groups {
				g.mail_host = host.clone();
			}
			Some((lock, groups))
		}
		Err(e) => {
			r.error(e.to_string());
			None
		}
	}
}

pub async fn apply(cfg: &Config, db: &Db) -> Report {
	let mut r = Report::new();
	let Some((_lock, mut groups)) = locked(&mut r, cfg, db).await else {
		return r;
	};
	let blocked = blocked(&mut r, db);

	for g in &groups {
		ensure_user(&mut r, g).await;
		ensure_dirs(&mut r, g).await;
		if g.repo.is_some() {
			deploy_key(&mut r, g).await;
		}
	}
	sync(&mut r, cfg, &groups, &blocked).await;
	databases(&mut r, &groups).await;

	// with a new certificate the sites get https, the apps the mail host
	if certificates(&mut r, cfg, &groups).await {
		let host = mailbox::host(cfg).filter(|h| host::has_cert(h));
		for g in &mut groups {
			g.mail_host = host.clone();
		}
		sync(&mut r, cfg, &groups, &blocked).await;
	}

	r.info(if r.ok { "done" } else { "done with errors" });
	r
}

/// Writes the files, removes the ones of deleted domains and groups,
/// (re)starts the units and reloads nginx and the firewall.
async fn sync(
	r: &mut Report,
	cfg: &Config,
	groups: &[Group],
	blocked: &[String],
) {
	let certs = host::certs(groups);
	let mut files: Vec<File> = groups
		.iter()
		.flat_map(|g| render::group_files(g, &certs))
		.collect();
	if let Some(domain) = &cfg.domain {
		files.push(render::panel_file(
			domain,
			&cfg.listen,
			host::has_cert(domain),
		));
	}
	files.push(firewall_file(cfg, groups, blocked));
	prepare_mail(r, groups).await;
	let mail_files = mail_files(r, cfg, groups);
	files.extend(mail_files.iter().cloned());

	let changed = write_files(r, &files);
	// nginx's master reads it as root; a group's processes shouldn't see
	// the other groups' domains and paths
	if let Err(e) = set_mode(Path::new("/etc/nginx/genco"), 0o750) {
		r.error(format!("/etc/nginx/genco: {e}"));
	}

	let paths: HashSet<&Path> =
		files.iter().map(|f| f.path.as_path()).collect();
	let stale_units = remove_stale(r, &paths).await;

	if !changed.is_empty() || !stale_units.is_empty() {
		r.run(&["systemctl", "daemon-reload"]).await;
	}

	for g in groups {
		start_units(r, g, &changed).await;
	}

	if changed.contains(Path::new(render::FIREWALL)) {
		r.run(&["nft", "-f", render::FIREWALL]).await;
	}
	if !mail_files.is_empty() {
		mail_services(r, cfg, groups, &changed).await;
	}

	// also without a change: a reload that failed before is tried again,
	// it is graceful
	if r.run(&["nginx", "-t", "-q"]).await {
		r.run(&["systemctl", "reload", "nginx"]).await;
	} else {
		r.error("nginx keeps the old config until the error is fixed");
	}
}

/// Writes the files that differ, returns their paths.
fn write_files(r: &mut Report, files: &[File]) -> HashSet<PathBuf> {
	let mut changed = HashSet::new();
	for f in files {
		let old = std::fs::read_to_string(&f.path).ok();
		if old.as_deref() == Some(f.content.as_str()) {
			restrict(r, f);
			continue;
		}
		if let Some(dir) = f.path.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		match write_file(f) {
			Ok(()) => {
				r.info(format!("wrote {}", f.path.display()));
				changed.insert(f.path.clone());
				restrict(r, f);
			}
			Err(e) => r.error(format!("writing {}: {e}", f.path.display())),
		}
	}
	changed
}

/// A private file is root's alone until `restrict` hands it to its group,
/// never readable by everyone in between.
fn write_file(f: &File) -> std::io::Result<()> {
	use std::io::Write as _;
	use std::os::unix::fs::OpenOptionsExt as _;

	if f.private_to.is_none() {
		return std::fs::write(&f.path, &f.content);
	}
	let _ = set_mode(&f.path, 0o600);
	let mut file = std::fs::OpenOptions::new()
		.write(true)
		.create(true)
		.truncate(true)
		.mode(0o600)
		.open(&f.path)?;
	file.write_all(f.content.as_bytes())
}

/// The blocklist's addresses that block now.
fn blocked(r: &mut Report, db: &Db) -> Vec<String> {
	crate::firewall::active(&db.conn(), crate::db::now()).unwrap_or_else(|e| {
		r.error(format!("blocklist: {e}"));
		vec![]
	})
}

fn firewall_file(cfg: &Config, groups: &[Group], blocked: &[String]) -> File {
	File {
		path: render::FIREWALL.into(),
		content: render::firewall(groups, cfg.panel_port(), blocked),
		private_to: None,
	}
}

/// The firewall only, after the blocklist changed.
pub async fn apply_firewall(cfg: &Config, db: &Db) -> Report {
	let mut r = Report::new();
	let Some((_lock, groups)) = locked(&mut r, cfg, db).await else {
		return r;
	};
	let blocked = blocked(&mut r, db);
	let file = firewall_file(cfg, &groups, &blocked);
	if !write_files(&mut r, &[file]).is_empty() {
		// atomic: rules nft refuses leave the old ones loaded
		r.run(&["nft", "-f", render::FIREWALL]).await;
	}
	r
}

/// Mail only, after a mailbox or forward changed: the maps, Dovecot,
/// Postfix and the mail host's certificate.
pub async fn apply_mail(cfg: &Config, db: &Db) -> Report {
	let mut r = Report::new();
	let Some((_lock, groups)) = locked(&mut r, cfg, db).await else {
		return r;
	};
	let groups = &groups;
	prepare_mail(&mut r, groups).await;
	for _ in 0..2 {
		let files = mail_files(&mut r, cfg, groups);
		let changed = write_files(&mut r, &files);
		if !files.is_empty() {
			mail_services(&mut r, cfg, groups, &changed).await;
		}
		// with the certificate IMAP and sending start; only the mail
		// host's, another domain's trouble is no mail error
		let host = mailbox::host(cfg).filter(|_| mailbox::enabled(groups));
		if !certificates_for(&mut r, cfg, host.as_deref().into_iter().collect())
			.await
		{
			break;
		}
	}
	r
}

async fn is_active(unit: &str) -> bool {
	Command::new("systemctl")
		.args(["is-active", "-q", unit])
		.status()
		.await
		.is_ok_and(|s| s.success())
}

/// root and the group may read a private file, nobody else.
fn restrict(r: &mut Report, f: &File) {
	let Some(user) = &f.private_to else {
		return;
	};
	let Some((_, gid)) = user_ids(user) else {
		return;
	};
	if let Err(e) = chown(&f.path, Some(0), Some(gid))
		.and_then(|()| set_mode(&f.path, 0o640))
	{
		r.error(format!("restricting {}: {e}", f.path.display()));
	}
}

/// Files and units genco wrote earlier for domains or groups that are gone.
/// Returns the removed units.
async fn remove_stale(r: &mut Report, keep: &HashSet<&Path>) -> Vec<String> {
	let mut stale = vec![];
	for (dir, prefix, suffix) in [
		("/etc/nginx/genco", "", ".conf"),
		("/etc/genco/php", "", ".conf"),
		("/etc/genco/apps", "", ".sh"),
		("/etc/genco/apps", "", ".env"),
		("/etc/genco/workers", "", ".sh"),
		("/etc/genco/schedules", "", ".sh"),
		("/etc/genco/smtp", "g-", ""),
		("/etc/logrotate.d", "genco-", ""),
		("/etc/systemd/system", "genco-", ".service"),
		("/etc/systemd/system", "genco-", ".slice"),
		("/etc/systemd/system", "genco-", ".timer"),
	] {
		let Ok(entries) = std::fs::read_dir(dir) else {
			continue;
		};
		for entry in entries.flatten() {
			let name = entry.file_name().to_string_lossy().to_string();
			if name.starts_with(prefix)
				&& name.ends_with(suffix)
				&& !keep.contains(entry.path().as_path())
				&& !SYSTEM_UNITS.contains(&name.as_str())
			{
				stale.push(entry.path());
			}
		}
	}

	let mut units = vec![];
	for path in stale {
		if path.starts_with("/etc/systemd/system") {
			let unit = path.file_name().unwrap().to_string_lossy().to_string();
			r.run(&["systemctl", "disable", "--now", &unit]).await;
			units.push(unit);
		}
		match std::fs::remove_file(&path) {
			Ok(()) => r.info(format!("removed {}", path.display())),
			Err(e) => r.error(format!("removing {}: {e}", path.display())),
		}
	}
	units
}
