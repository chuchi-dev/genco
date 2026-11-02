//! The `genco` commands that run here in the terminal (deploy, backup,
//! import, users, runners).

use crate::config::Config;
use crate::db::Db;
use crate::events::Events;
use crate::*;
use std::io::Read as _;
use std::path::PathBuf;

/// Imports a dump in the terminal, printing its log.
pub(crate) async fn import_cmd(
	cfg: Config,
	name: String,
	file: PathBuf,
) -> Result<(), String> {
	let db = Db::open(&cfg.db_path())?;
	let id = {
		let conn = db.conn();
		let database_id: i64 = conn
			.query_row(
				"SELECT id FROM databases WHERE name = ?1",
				[&name],
				|r| r.get(0),
			)
			.map_err(|_| format!("no database {name}"))?;
		imports::create(&conn, database_id, activity::cli_actor())
			.map_err(|e| e.to_string())?
	};
	let (id, lock) = id;
	let upload = imports::upload_path(id);
	std::fs::create_dir_all(upload.parent().unwrap())
		.map_err(|e| e.to_string())?;
	std::fs::copy(&file, &upload)
		.map_err(|e| format!("{}: {e}", file.display()))?;
	if !imports::run(&db, id, lock, true).await {
		return Err(format!("import {id} failed"));
	}
	Ok(())
}

pub(crate) async fn deploy_cmd(
	cfg: Config,
	group: String,
	git_ref: Option<String>,
	artifact: Option<PathBuf>,
) -> Result<(), String> {
	let db = Db::open(&cfg.db_path())?;
	let queued = {
		let conn = db.conn();
		let group = hosting::group_by_name(&conn, &group)
			.map_err(|e| e.to_string())?
			.ok_or(format!("no group {group}"))?;
		match &artifact {
			Some(_) => deploy::queue_upload(
				&conn,
				&group,
				git_ref.as_deref(),
				None,
				"terminal",
			)
			.map(|id| deploy::Queued { id, new: true }),
			None => {
				deploy::queue(&conn, &group, git_ref.as_deref(), "terminal")
			}
		}
		.map_err(|e| e.to_string())?
	};
	// the waiting one runs in the panel's process, with the new ref
	let deploy::Queued { id, new } = queued;
	if !new {
		println!(
			"deploy {id} of the group waits already, it deploys this ref now"
		);
		return Ok(());
	}
	let ctx = deploy::Ctx {
		db: db.clone(),
		events: Events::new(),
		print: true,
	};
	if let Some(file) = artifact {
		// root's copy, the group's user unpacks it from genco's stdin
		if let Err(e) = std::fs::copy(&file, deploy::upload_path(id)) {
			// a waiting deploy without its file would take the next ones
			ctx.status(id, "failed");
			return Err(format!("{}: {e}", file.display()));
		}
	}
	deploy::run(&ctx, id).await;

	let status = deploy::get(&db.conn(), id)
		.map_err(|e| e.to_string())?
		.status;
	if status != "done" {
		return Err(format!("deploy {id} {status}"));
	}
	Ok(())
}

pub(crate) async fn runners_cmd(
	cfg: &Config,
	cmd: RunnersCmd,
) -> Result<(), String> {
	let db = Db::open(&cfg.db_path())?;
	match cmd {
		RunnersCmd::List => {
			let list = runners::list(&db.conn()).map_err(|e| e.to_string())?;
			for r in &list {
				let seen = match r.last_seen {
					Some(t) => util::format_time(t).await,
					None => "never".into(),
				};
				let state = if r.online { "online" } else { "offline" };
				let host = r.info["host"].as_str().unwrap_or("");
				println!("{}  {state}  last seen {seen}  {host}", r.name);
			}
			if list.is_empty() {
				println!(
					"no runner, build steps with on = \"runner\" run here"
				);
			}
			let t = runners::target().await;
			println!(
				"builds for: {} {} ({}), glibc {}, {}",
				t.os, t.version, t.codename, t.glibc, t.arch
			);
		}
		RunnersCmd::Add { name } => {
			let token = runners::create(&db.conn(), &name)
				.map_err(|e| e.to_string())?;
			let domain = cfg.domain.as_deref().unwrap_or("<this server>");
			println!("on the build box (after genco runner install):");
			println!("genco runner register https://{domain} {token}");
		}
		RunnersCmd::Remove { name } => {
			runners::remove(&db.conn(), &name).map_err(|e| e.to_string())?;
			println!("removed {name}");
		}
	}
	Ok(())
}

pub(crate) async fn backup_cmd(cfg: Config) -> Result<(), String> {
	let db = Db::open(&cfg.db_path())?;
	let id = backup::create(&db.conn(), "terminal", None)
		.map_err(|e| e.to_string())?;
	let ctx = backup::Ctx {
		db: db.clone(),
		events: Events::new(),
		print: true,
	};
	backup::run(&ctx, id).await;
	let status = backup::get(&db.conn(), id)
		.map_err(|e| e.to_string())?
		.status;
	if status != "done" {
		return Err(format!("backup {id} {status}"));
	}
	Ok(())
}

pub(crate) fn edit(
	cfg: Config,
	f: impl FnOnce(&rusqlite::Connection) -> error::Result<String>,
) -> Result<(), String> {
	let db = Db::open(&cfg.db_path())?;
	let msg = f(&db.conn()).map_err(|e| e.to_string())?;
	println!("{msg}");
	Ok(())
}

pub(crate) async fn apply_cmd(cfg: Config) -> Result<(), String> {
	std::fs::create_dir_all(&cfg.data_dir).map_err(|e| e.to_string())?;
	let db = Db::open(&cfg.db_path())?;

	let report = apply::apply(&cfg, &db).await;
	if !report.ok {
		return Err("apply failed, see the errors above".into());
	}
	Ok(())
}

pub(crate) fn user(cfg: Config, cmd: UserCmd) -> Result<(), String> {
	let db = Db::open(&cfg.db_path())?;
	let conn = db.conn();

	match cmd {
		UserCmd::Add { email, name } => {
			let user = users::create(&conn, &email, &name, &hash_from_stdin()?)
				.map_err(|e| e.to_string())?;
			println!("added {}", user.email);
		}
		UserCmd::Password { email } => {
			users::set_password(
				&conn,
				user_id(&conn, &email)?,
				&hash_from_stdin()?,
			)
			.map_err(|e| e.to_string())?;
			println!("password changed");
		}
		UserCmd::TotpOff { email } => {
			conn.execute(
				"UPDATE users SET totp_secret = NULL WHERE id = ?1",
				[user_id(&conn, &email)?],
			)
			.map_err(|e| e.to_string())?;
			println!(
				"two-factor login turned off, turn it on again in the panel"
			);
		}
		UserCmd::List => {
			for u in users::list(&conn).map_err(|e| e.to_string())? {
				println!("{}\t{}", u.email, u.name);
			}
		}
	}
	Ok(())
}

pub(crate) fn user_id(
	conn: &rusqlite::Connection,
	email: &str,
) -> Result<i64, String> {
	conn.query_row(
		"SELECT id FROM users WHERE email = ?1",
		[email.trim().to_lowercase()],
		|row| row.get(0),
	)
	.map_err(|_| format!("no user {email}"))
}

pub(crate) fn hash_from_stdin() -> Result<String, String> {
	users::hash_password(&read_password()?).map_err(|e| e.to_string())
}

pub(crate) fn read_password() -> Result<String, String> {
	let mut password = String::new();
	std::io::stdin()
		.read_to_string(&mut password)
		.map_err(|e| e.to_string())?;
	Ok(password.trim_end_matches(['\n', '\r']).to_string())
}
