//! `genco client|group|domain|database ...`: what the panel does, for the
//! agent and the terminal. `genco apply` puts the changes on the server.

use clap::Parser;
use rusqlite::Connection;

use crate::actions;
use crate::error::{Error, Result};
use crate::hosting::{self, Kind};

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
	/// Change a group: its repository and branch, its PHP settings
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
	},
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
	} = cmd
	{
		let g = hosting::group_by_name(conn, &name)?
			.ok_or_else(|| Error::Invalid(format!("no group {name}")))?;
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
		return Ok("changed, run genco apply".into());
	}
	if let GroupCmd::DeployKey { .. } = cmd {
		unreachable!("main reads it as the group's user");
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

pub fn domain(conn: &Connection, cmd: DomainCmd) -> Result<String> {
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
				},
			};
			let id = hosting::insert_domain(conn, &req)?;
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
			};
			hosting::set_domain(conn, d.id, &req)?;
			Ok("changed, run genco apply".into())
		}
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
	let engine = if postgres {
		hosting::Engine::Postgres
	} else {
		hosting::Engine::Mariadb
	};
	let req = hosting::DatabaseReq {
		group_id: group_id(conn, &group)?,
		name,
		engine,
	};
	let id = hosting::insert_database(conn, &req)?;
	Ok(format!("added database {id}, run genco apply"))
}
