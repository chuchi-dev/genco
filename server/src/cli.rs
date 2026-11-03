//! `genco client|group|domain|database ...`: what the panel does, for the
//! agent and the terminal. `genco apply` puts the changes on the server.

use clap::Parser;
use rusqlite::Connection;

use crate::actions;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::hosting::{self, Kind};
use crate::limits;
use crate::procs;

#[derive(Debug, Parser)]
pub enum ClientCmd {
	/// Add a client
	Add {
		name: String,
		#[clap(long, default_value = "")]
		contact: String,
	},
}

#[derive(Debug, Parser)]
pub enum GroupCmd {
	/// Add a group, its name becomes the linux user g-<name>
	Add {
		name: String,
		/// name or id of the client
		#[clap(long)]
		client: String,
	},
	/// Ask the admins by mail to confirm deleting an empty group
	Delete { name: String },
	/// Change a group: its repository and branch, its PHP settings, its
	/// limits
	Set {
		name: String,
		/// git URL the group deploys from
		#[clap(long)]
		repo: Option<String>,
		#[clap(long)]
		branch: Option<String>,
		/// the largest upload in MB
		#[clap(long)]
		php_upload: Option<u32>,
		/// PHP's memory limit in MB
		#[clap(long)]
		php_memory: Option<u32>,
		/// seconds a request may run
		#[clap(long)]
		php_time: Option<u32>,
		#[clap(long)]
		php_input_vars: Option<u32>,
		/// memory of the whole group (MemoryMax): 2.5G, 512M, 512 (MB) or
		/// default (1.5G)
		#[clap(long)]
		memory: Option<String>,
		/// where the kernel starts slowing it down (MemoryHigh), two
		/// thirds of --memory unless set; or default
		#[clap(long)]
		memory_high: Option<String>,
		/// CPU, 100% is one core: 200% or default (150%)
		#[clap(long)]
		cpu: Option<String>,
		/// processes and threads: 1024 or default (512)
		#[clap(long)]
		tasks: Option<String>,
	},
	/// Take the limits of drop-ins in genco-<group>.slice.d into the
	/// group's settings, move the files to the trash and apply
	AdoptLimits { name: String },
	/// Print the public key the group deploys with, for the repository
	DeployKey { name: String },
}

#[derive(Debug, Parser)]
pub enum DomainCmd {
	/// Add a domain to a group
	Add {
		name: String,
		/// name of the group
		#[clap(long)]
		group: String,
		#[clap(flatten)]
		opts: DomainOpts,
	},
	/// Change a domain, unset options stay
	Set {
		name: String,
		#[clap(flatten)]
		opts: DomainOpts,
	},
	/// Ask the admins by mail to confirm deleting a domain
	Delete { name: String },
	/// Environment variables of a node or rust app: prints them, secrets
	/// without their value
	Env {
		name: String,
		/// NAME=value, more than once
		#[clap(long = "set")]
		sets: Vec<String>,
		/// NAME=value of a secret, never printed again
		#[clap(long = "secret")]
		secrets: Vec<String>,
		/// NAME, more than once
		#[clap(long = "unset")]
		unsets: Vec<String>,
	},
}

#[derive(Debug, clap::Args)]
pub struct DomainOpts {
	/// static, php, node or rust
	#[clap(long)]
	kind: Option<String>,
	/// for php: 8.2, 8.3, 8.4 or 8.5
	#[clap(long)]
	php: Option<String>,
	/// relative to the group's directory: what nginx serves, or where the
	/// app runs
	#[clap(long)]
	root: Option<String>,
	/// starts a node or rust app
	#[clap(long)]
	command: Option<String>,
	/// node or rust: files nginx serves itself, relative to the group's
	/// directory, e.g. current/svelte/dist/public; "" removes it
	#[clap(long)]
	public: Option<String>,
	/// lines added to the nginx server block
	#[clap(long)]
	nginx_extra: Option<String>,
	/// proxy: where it forwards to, e.g. http://127.0.0.1:8081
	#[clap(long)]
	upstream: Option<String>,
	/// www.<domain> redirects to the domain: true or false
	#[clap(long)]
	www: Option<bool>,
	/// node or rust: a restart starts the new process beside the old one
	/// and switches over once it listens: true or false
	#[clap(long)]
	zero_downtime: Option<bool>,
}

#[derive(Debug, Parser)]
pub enum WorkerCmd {
	/// List the workers of every group
	List,
	/// Add a command that runs all the time as the group's user, restarted
	/// when it stops and after every deploy
	Add {
		name: String,
		#[clap(long)]
		group: String,
		/// relative to the group's directory, e.g. current/craft
		#[clap(long, default_value = ".")]
		dir: String,
		#[clap(long)]
		command: String,
		/// a port of its own, open to the internet; it gets it as PORT
		#[clap(long)]
		port: Option<u16>,
		/// where clients reach it, a note: packages.example.com:5426
		#[clap(long, default_value = "")]
		address: String,
	},
	/// Change a worker, unset options stay; of genco.toml's only the
	/// address
	Set {
		name: String,
		#[clap(long)]
		group: String,
		#[clap(long)]
		dir: Option<String>,
		#[clap(long)]
		command: Option<String>,
		#[clap(long)]
		port: Option<u16>,
		/// close its public port
		#[clap(long)]
		no_port: bool,
		/// where clients reach it, "" removes it
		#[clap(long)]
		address: Option<String>,
	},
	/// Remove a worker (not one of genco.toml)
	Delete {
		name: String,
		#[clap(long)]
		group: String,
	},
}

#[derive(Debug, Parser)]
pub enum ScheduleCmd {
	/// List the schedules of every group
	List,
	/// Add a command that runs at times, as the group's user
	Add {
		name: String,
		#[clap(long)]
		group: String,
		/// relative to the group's directory, e.g. current/craft
		#[clap(long, default_value = ".")]
		dir: String,
		#[clap(long)]
		command: String,
		/// systemd's OnCalendar: hourly, daily, *:0/5, Mon *-*-* 03:00
		#[clap(long)]
		every: String,
		/// minutes it may run
		#[clap(long, default_value_t = 60)]
		timeout: u32,
	},
	/// Change a schedule, unset options stay
	Set {
		name: String,
		#[clap(long)]
		group: String,
		#[clap(long)]
		dir: Option<String>,
		#[clap(long)]
		command: Option<String>,
		#[clap(long)]
		every: Option<String>,
		#[clap(long)]
		timeout: Option<u32>,
	},
	/// Remove a schedule (not one of genco.toml)
	Delete {
		name: String,
		#[clap(long)]
		group: String,
	},
	/// Run a schedule now, its output is in `journalctl -u
	/// genco-<group>-cron-<name>`
	Run {
		name: String,
		#[clap(long)]
		group: String,
	},
}

#[derive(Debug, Parser)]
pub enum DatabaseCmd {
	/// Add a database to a group, MariaDB unless --postgres
	Add {
		name: String,
		#[clap(long)]
		group: String,
		/// a PostgreSQL database (installed with the first one)
		#[clap(long)]
		postgres: bool,
	},
	/// Ask the admins by mail to confirm deleting a database
	Delete { name: String },
	/// Import a dump (.sql, .sql.gz or a zip with one), replacing what the
	/// database has; the current content goes to the trash first
	Import {
		name: String,
		file: std::path::PathBuf,
	},
}

fn kind(s: &str) -> Result<Kind> {
	serde_json::from_value(serde_json::Value::String(s.into()))
		.map_err(|_| Error::Invalid(format!("unknown kind {s}")))
}

fn group_id(conn: &Connection, name: &str) -> Result<i64> {
	hosting::group_by_name(conn, name)?
		.map(|g| g.id)
		.ok_or_else(|| Error::Invalid(format!("no group {name}")))
}

pub fn client(conn: &Connection, cmd: ClientCmd) -> Result<String> {
	let ClientCmd::Add { name, contact } = cmd;
	let req = hosting::ClientReq {
		name,
		contact,
		notes: String::new(),
	};
	let id = hosting::insert_client(conn, &req)?;
	Ok(format!("added client {id}"))
}

pub fn is_deletion(cmd: &crate::Cmd) -> bool {
	use crate::Cmd;
	matches!(
		cmd,
		Cmd::Group(GroupCmd::Delete { .. })
			| Cmd::Domain(DomainCmd::Delete { .. })
			| Cmd::Database(DatabaseCmd::Delete { .. })
	)
}

/// What `genco ... delete <name>` asks to delete, see `is_deletion`.
pub fn deletion(
	conn: &Connection,
	cmd: &crate::Cmd,
) -> Result<(actions::Kind, i64)> {
	use crate::Cmd;

	let groups = hosting::groups(conn)?;
	let found = match cmd {
		Cmd::Group(GroupCmd::Delete { name }) => groups
			.iter()
			.find(|g| &g.name == name)
			.map(|g| (actions::Kind::Group, g.id)),
		Cmd::Domain(DomainCmd::Delete { name }) => groups
			.iter()
			.flat_map(|g| &g.domains)
			.find(|d| &d.name == name)
			.map(|d| (actions::Kind::Domain, d.id)),
		Cmd::Database(DatabaseCmd::Delete { name }) => groups
			.iter()
			.flat_map(|g| &g.databases)
			.find(|d| &d.name == name)
			.map(|d| (actions::Kind::Database, d.id)),
		_ => None,
	};
	found.ok_or_else(|| Error::Invalid("not found".into()))
}

pub fn group(conn: &Connection, cmd: GroupCmd) -> Result<String> {
	if let GroupCmd::Set {
		name,
		repo,
		branch,
		php_upload,
		php_memory,
		php_time,
		php_input_vars,
		memory,
		memory_high,
		cpu,
		tasks,
	} = cmd
	{
		let g = hosting::group_by_name(conn, &name)?
			.ok_or_else(|| Error::Invalid(format!("no group {name}")))?;
		// all or nothing: wrong limits change nothing else either
		let mut l = g.limits;
		if let Some(v) = memory {
			l.memory_max_mb = limits::parse_size(&v)?;
		}
		if let Some(v) = memory_high {
			l.memory_high_mb = limits::parse_size(&v)?;
		}
		if let Some(v) = cpu {
			l.cpu_percent = limits::parse_percent(&v)?;
		}
		if let Some(v) = tasks {
			l.tasks_max = limits::parse_count(&v)?;
		}
		limits::check_here(&l)?;

		hosting::set_repo(conn, g.id, repo.as_deref(), branch.as_deref())?;
		let php = hosting::PhpSettings {
			upload_mb: php_upload.unwrap_or(g.php.upload_mb),
			memory_mb: php_memory.unwrap_or(g.php.memory_mb),
			max_execution_s: php_time.unwrap_or(g.php.max_execution_s),
			max_input_vars: php_input_vars.unwrap_or(g.php.max_input_vars),
		};
		if php != g.php {
			hosting::set_php(conn, g.id, &php)?;
		}
		let mut msg = "changed, run genco apply".to_string();
		if l != g.limits {
			let warning = hosting::set_limits(conn, g.id, &l)?;
			msg.push_str(&format!("\nlimits: {}", l.describe()));
			if let Some(w) = warning {
				msg.push_str(&format!("\nwarning: {w}"));
			}
		}
		return Ok(msg);
	}
	if let GroupCmd::DeployKey { .. } | GroupCmd::AdoptLimits { .. } = cmd {
		unreachable!("main runs them");
	}
	let GroupCmd::Add { name, client } = cmd else {
		unreachable!("deletions go through actions");
	};
	let client_id = hosting::tree(conn)?
		.into_iter()
		.find(|c| c.name == client || c.id.to_string() == client)
		.map(|c| c.id)
		.ok_or_else(|| Error::Invalid(format!("no client {client}")))?;
	let req = hosting::GroupReq {
		name,
		client_id,
		notes: String::new(),
	};
	let id = hosting::insert_group(conn, &req)?;
	Ok(format!("added group {id}, run genco apply"))
}

/// Takes the limits of the group's slice drop-ins into its settings, moves
/// the files to the trash and reloads systemd; `genco apply` after it
/// writes the slice and sets them on the running one.
pub async fn adopt_limits(db: &crate::db::Db, name: &str) -> Result<String> {
	let g = group_of(&db.conn(), name)?;
	let files = limits::slice_dropins(&g.name);
	if files.is_empty() {
		return Err(Error::Invalid(format!(
			"no drop-ins in /etc/systemd/system/genco-{name}.slice.d"
		)));
	}
	let mut l = g.limits;
	for f in &files {
		let content = std::fs::read_to_string(f)
			.map_err(|e| Error::Internal(format!("{}: {e}", f.display())))?;
		l = limits::adopt(l, &content)
			.map_err(|e| Error::Invalid(format!("{}: {e}", f.display())))?;
	}
	let warning = hosting::set_limits(&db.conn(), g.id, &l)?;

	actions::trash_dir().map_err(Error::Internal)?;
	let stamp = crate::util::stamp().await;
	let mut msg = String::new();
	for f in &files {
		let dir = f.parent().and_then(|d| d.file_name()).unwrap_or_default();
		let file = f.file_name().unwrap_or_default();
		let to = format!(
			"{}/{}-{}-{stamp}",
			actions::TRASH,
			dir.to_string_lossy(),
			file.to_string_lossy()
		);
		std::fs::copy(f, &to)
			.and_then(|_| std::fs::remove_file(f))
			.map_err(|e| Error::Internal(format!("{}: {e}", f.display())))?;
		msg.push_str(&format!("moved {} to {to}\n", f.display()));
	}
	// only when nothing else is in it
	if let Some(dir) = files[0].parent() {
		let _ = std::fs::remove_dir(dir);
	}
	crate::util::sh("systemctl daemon-reload")
		.await
		.map_err(Error::Internal)?;

	msg.push_str(&format!("limits of {name}: {}", l.describe()));
	if let Some(w) = warning {
		msg.push_str(&format!("\nwarning: {w}"));
	}
	Ok(msg)
}

pub fn domain(
	cfg: &Config,
	conn: &Connection,
	cmd: DomainCmd,
) -> Result<String> {
	match cmd {
		DomainCmd::Add { name, group, opts } => {
			let kind = kind(opts.kind.as_deref().unwrap_or("static"))?;
			let req = hosting::DomainReq {
				group_id: group_id(conn, &group)?,
				name,
				settings: hosting::DomainSettings {
					kind,
					php_version: opts.php,
					root: opts.root.unwrap_or_else(|| "public".into()),
					command: opts.command,
					public_dir: opts.public,
					nginx_extra: opts.nginx_extra.unwrap_or_default(),
					upstream: opts.upstream,
					www: opts.www.unwrap_or(false),
					zero_downtime: opts.zero_downtime.unwrap_or(false),
				},
			};
			let id = hosting::insert_domain(cfg, conn, &req)?;
			Ok(format!("added domain {id}, run genco apply"))
		}
		DomainCmd::Delete { .. } => {
			unreachable!("deletions go through actions")
		}
		DomainCmd::Env {
			name,
			sets,
			secrets,
			unsets,
		} => {
			let (_, d) = hosting::find_domain(conn, |d| d.name == name)
				.map_err(|_| Error::Invalid(format!("no domain {name}")))?;
			crate::appenv::cli(conn, d.id, sets, secrets, unsets)
		}
		DomainCmd::Set { name, opts } => {
			let (_, d) = hosting::find_domain(conn, |d| d.name == name)
				.map_err(|_| Error::Invalid(format!("no domain {name}")))?;
			let kind = match opts.kind {
				Some(k) => kind(&k)?,
				None => d.kind,
			};
			// switching to or from php drops or needs the version
			let php_version = match (kind, opts.php) {
				(Kind::Php, Some(v)) => Some(v),
				(Kind::Php, None) => d.php_version,
				_ => None,
			};
			let command = match kind {
				Kind::Node | Kind::Rust => opts.command.or(d.command),
				_ => None,
			};
			let req = hosting::DomainSettings {
				kind,
				php_version,
				root: opts.root.unwrap_or(d.root),
				command,
				public_dir: opts.public.or(d.public_dir),
				nginx_extra: opts.nginx_extra.unwrap_or(d.nginx_extra),
				upstream: match kind {
					Kind::Proxy => opts.upstream.or(d.upstream),
					_ => None,
				},
				www: opts.www.unwrap_or(d.www),
				zero_downtime: opts.zero_downtime.unwrap_or(
					d.zero_downtime && matches!(kind, Kind::Node | Kind::Rust),
				),
			};
			hosting::set_domain(cfg, conn, d.id, &req)?;
			Ok("changed, run genco apply".into())
		}
	}
}

pub fn engine(postgres: bool) -> hosting::Engine {
	if postgres {
		hosting::Engine::Postgres
	} else {
		hosting::Engine::Mariadb
	}
}

pub fn database(conn: &Connection, cmd: DatabaseCmd) -> Result<String> {
	let DatabaseCmd::Add {
		name,
		group,
		postgres,
	} = cmd
	else {
		unreachable!("deletions go through actions");
	};
	let req = hosting::DatabaseReq {
		group_id: group_id(conn, &group)?,
		name,
		engine: engine(postgres),
	};
	let id = hosting::insert_database(conn, &req)?;
	Ok(format!("added database {id}, run genco apply"))
}

fn group_of(conn: &Connection, name: &str) -> Result<hosting::Group> {
	hosting::group_by_name(conn, name)?
		.ok_or_else(|| Error::Invalid(format!("no group {name}")))
}

fn source_note(source: &str) -> &'static str {
	if source == procs::REPO {
		"  (repository)"
	} else {
		""
	}
}

pub fn worker(conn: &Connection, cmd: WorkerCmd) -> Result<String> {
	match cmd {
		WorkerCmd::List => {
			let lines: Vec<String> = hosting::groups(conn)?
				.iter()
				.flat_map(|g| {
					g.workers.iter().map(move |w| {
						let port = w
							.port
							.map(|p| format!(" :{p}"))
							.unwrap_or_default();
						let address = if w.address.is_empty() {
							String::new()
						} else {
							format!(" ({})", w.address)
						};
						format!(
							"{}/{}{port}{address}  in {}: {}{}",
							g.name,
							w.name,
							w.dir,
							w.command,
							source_note(&w.source)
						)
					})
				})
				.collect();
			Ok(lines.join("\n"))
		}
		WorkerCmd::Add {
			name,
			group,
			dir,
			command,
			port,
			address,
		} => {
			let w = procs::Worker {
				id: 0,
				group_id: group_id(conn, &group)?,
				name,
				dir,
				command,
				port,
				address,
				source: String::new(),
				state: None,
			};
			procs::save_worker(conn, &w)?;
			Ok("added, run genco apply".into())
		}
		WorkerCmd::Set {
			name,
			group,
			dir,
			command,
			port,
			no_port,
			address,
		} => {
			let g = group_of(conn, &group)?;
			let w =
				g.workers.into_iter().find(|w| w.name == name).ok_or_else(
					|| Error::Invalid(format!("no worker {name}")),
				)?;

			// the address alone also of genco.toml's, it needs no apply
			let only_address = dir.is_none()
				&& command.is_none()
				&& port.is_none()
				&& !no_port;
			if let (true, Some(address)) = (only_address, &address) {
				procs::set_address(conn, w.id, address)?;
				return Ok("changed".into());
			}

			let w = procs::Worker {
				dir: dir.unwrap_or(w.dir),
				command: command.unwrap_or(w.command),
				port: if no_port { None } else { port.or(w.port) },
				address: address.unwrap_or(w.address),
				..w
			};
			procs::save_worker(conn, &w)?;
			Ok("changed, run genco apply".into())
		}
		WorkerCmd::Delete { name, group } => {
			let g = group_of(conn, &group)?;
			let w =
				g.workers.iter().find(|w| w.name == name).ok_or_else(|| {
					Error::Invalid(format!("no worker {name}"))
				})?;
			procs::delete(conn, "workers", w.id)?;
			Ok("removed, run genco apply".into())
		}
	}
}

/// `every` is checked with systemd before, see main.
pub fn schedule(conn: &Connection, cmd: ScheduleCmd) -> Result<String> {
	match cmd {
		ScheduleCmd::List => {
			let lines: Vec<String> = hosting::groups(conn)?
				.iter()
				.flat_map(|g| {
					g.schedules.iter().map(move |s| {
						format!(
							"{}/{}  {}  in {}: {}{}",
							g.name,
							s.name,
							s.every,
							s.dir,
							s.command,
							source_note(&s.source)
						)
					})
				})
				.collect();
			Ok(lines.join("\n"))
		}
		ScheduleCmd::Add {
			name,
			group,
			dir,
			command,
			every,
			timeout,
		} => {
			let s = procs::Schedule {
				id: 0,
				group_id: group_id(conn, &group)?,
				name,
				dir,
				command,
				every,
				timeout,
				source: String::new(),
			};
			procs::save_schedule(conn, &s)?;
			Ok("added, run genco apply".into())
		}
		ScheduleCmd::Set {
			name,
			group,
			dir,
			command,
			every,
			timeout,
		} => {
			let g = group_of(conn, &group)?;
			let s = g
				.schedules
				.into_iter()
				.find(|s| s.name == name)
				.ok_or_else(|| Error::Invalid(format!("no schedule {name}")))?;
			let s = procs::Schedule {
				dir: dir.unwrap_or(s.dir),
				command: command.unwrap_or(s.command),
				every: every.unwrap_or(s.every),
				timeout: timeout.unwrap_or(s.timeout),
				..s
			};
			procs::save_schedule(conn, &s)?;
			Ok("changed, run genco apply".into())
		}
		ScheduleCmd::Delete { name, group } => {
			let g = group_of(conn, &group)?;
			let s =
				g.schedules.iter().find(|s| s.name == name).ok_or_else(
					|| Error::Invalid(format!("no schedule {name}")),
				)?;
			procs::delete(conn, "schedules", s.id)?;
			Ok("removed, run genco apply".into())
		}
		ScheduleCmd::Run { .. } => unreachable!("main starts it"),
	}
}
