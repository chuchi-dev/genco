//! What genco asks MariaDB and PostgreSQL outside of apply, which sets up
//! the databases (apply/system.rs, pg.rs). Database names are checked to
//! be [a-z0-9_], group names [a-z0-9-].

use std::path::Path;

use crate::db::Db;
use crate::error::{Error, Result};
use crate::hosting::{Engine, check_database_name};
use crate::render;
use crate::util;

/// A database name in a MariaDB grant: there "_" matches any character,
/// shop_1 would also cover shopx1 of another group.
pub fn mariadb_grant(name: &str) -> String {
	name.replace('_', "\\_")
}

/// The grant pattern as it is stored in mysql.db, in an SQL string.
pub fn mariadb_grant_literal(name: &str) -> String {
	mariadb_grant(name).replace('\\', "\\\\")
}

/// Refuses a database that exists in its engine already but isn't
/// genco's: apply would hand another app's data to the group (and take
/// it from the app). One of genco's own the insert refuses itself.
pub async fn check_database_free(
	db: &Db,
	name: &str,
	engine: Engine,
) -> Result<()> {
	check_database_name(name)?;
	let ours: bool = db.conn().query_row(
		"SELECT EXISTS(SELECT 1 FROM databases WHERE name = ?1)",
		[name],
		|r| r.get(0),
	)?;
	if ours {
		return Ok(());
	}

	let out = match engine {
		Engine::Postgres if !crate::pg::installed() => return Ok(()),
		Engine::Postgres => {
			let sql =
				format!("SELECT 1 FROM pg_database WHERE datname = '{name}'");
			let mut args = crate::pg::PSQL_ADMIN.to_vec();
			args.extend(["-c", &sql]);
			util::run(&args).await.map_err(Error::Internal)?
		}
		// apply installs and starts it
		Engine::Mariadb if !Path::new("/usr/bin/mariadb").exists() => {
			return Ok(());
		}
		Engine::Mariadb => {
			let sql = format!(
				"SELECT 1 FROM information_schema.schemata \
				WHERE schema_name = '{name}'"
			);
			match util::run(&["mariadb", "-N", "-B", "-e", &sql]).await {
				Ok(out) => out,
				Err(e) if mariadb_down(&e) => return Ok(()),
				Err(e) => return Err(Error::Internal(e)),
			}
		}
	};
	foreign_database(name, &out)
}

/// The client couldn't reach the server: it isn't running (yet).
fn mariadb_down(err: &str) -> bool {
	err.contains("ERROR 2002")
}

/// `out` is what the engine says about the name: "1" if it exists.
fn foreign_database(name: &str, out: &str) -> Result<()> {
	if out.trim().is_empty() {
		return Ok(());
	}
	Err(Error::Invalid(format!(
		"a database {name} exists already on this server and isn't \
		genco's: pick another name (or move its data into a new genco \
		database)"
	)))
}

/// Takes the group's privileges on a dropped MariaDB database: MariaDB
/// keeps the grant, and apply would count another database of that name
/// as the group's. Returns the command it ran, None without a grant.
pub async fn mariadb_revoke(
	name: &str,
	group: &str,
) -> std::result::Result<Option<String>, String> {
	let user = render::group_user(group);
	let exists = format!(
		"SELECT 1 FROM mysql.db WHERE Db = '{}' AND User = '{user}' \
		AND Host = 'localhost'",
		mariadb_grant_literal(name)
	);
	if util::run(&["mariadb", "-N", "-B", "-e", &exists])
		.await?
		.is_empty()
	{
		return Ok(None);
	}

	let revoke = format!(
		"REVOKE ALL PRIVILEGES ON `{}`.* FROM `{user}`@localhost",
		mariadb_grant(name)
	);
	util::run(&["mariadb", "-e", &revoke]).await?;
	Ok(Some(revoke))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn foreign_databases() {
		assert!(foreign_database("shop", "").is_ok());
		let err = foreign_database("shop", "1\n").unwrap_err().to_string();
		assert!(err.contains("a database shop exists already"));
	}

	#[test]
	fn a_stopped_mariadb_has_no_databases() {
		assert!(mariadb_down(
			"mariadb -N -B -e SELECT 1 failed: ERROR 2002 (HY000): Can't \
			connect to local server through socket \
			'/run/mysqld/mysqld.sock' (2)"
		));
		assert!(!mariadb_down(
			"mariadb failed: ERROR 1045 (28000): Access denied for user"
		));
	}

	#[test]
	fn grants_match_only_their_name() {
		assert_eq!(mariadb_grant("shop_db"), "shop\\_db");
		assert_eq!(mariadb_grant_literal("shop_db"), "shop\\\\_db");
	}
}
