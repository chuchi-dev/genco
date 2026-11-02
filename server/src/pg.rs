//! PostgreSQL databases next to the MariaDB ones. Debian's postgresql is
//! installed by `genco apply` when the first one is created. Each group
//! has a role named like its unix user, which logs in over the socket
//! (`peer`, no password) and owns the group's databases; other roles may
//! not connect to them. Root works as `postgres`, but never runs SQL
//! inside a group's database: the group owns it and could have changed
//! its search_path to hand the superuser its own functions (the
//! CVE-2018-1058 pattern); pg_dump sets a safe path itself.

use std::path::Path;

use tokio::process::Command;

use crate::hosting::{Engine, Group};
use crate::render;
use crate::util::sh;

pub fn installed() -> bool {
	Path::new("/usr/bin/psql").exists()
		&& Path::new("/usr/bin/pg_dump").exists()
		&& Path::new("/usr/lib/postgresql").exists()
}

pub fn used(groups: &[Group]) -> bool {
	groups
		.iter()
		.flat_map(|g| &g.databases)
		.any(|d| d.engine == Engine::Postgres)
}

/// `"g-shop"`: role names with a dash need quotes.
fn role(group: &str) -> String {
	format!("\"{}\"", render::group_user(group))
}

/// A database's name in SQL; names are checked to be [a-z0-9_], quoted
/// a leading digit is fine.
pub fn ident(name: &str) -> String {
	format!("\"{name}\"")
}

/// The SQL that creates the roles and databases genco knows, safe to run
/// again. Names are checked to be [a-z0-9_] and [a-z0-9-].
pub fn setup_sql(groups: &[Group]) -> String {
	// the groups' roles list neither the maintenance databases nor, from
	// there, the other groups'
	let mut sql = String::from(
		"REVOKE CONNECT ON DATABASE postgres FROM PUBLIC;\n\
		REVOKE CONNECT ON DATABASE template1 FROM PUBLIC;\n",
	);
	for g in groups {
		let dbs: Vec<_> = g
			.databases
			.iter()
			.filter(|d| d.engine == Engine::Postgres)
			.collect();
		if dbs.is_empty() {
			continue;
		}
		let user = render::group_user(&g.name);
		let role = role(&g.name);
		sql.push_str(&format!(
			"SELECT 'CREATE ROLE {role} LOGIN' WHERE NOT EXISTS \
			(SELECT FROM pg_roles WHERE rolname = '{user}')\\gexec\n"
		));
		for d in dbs {
			let name = &d.name;
			let id = ident(name);
			sql.push_str(&format!(
				"SELECT 'CREATE DATABASE {id} OWNER {role} ENCODING ''UTF8'' \
				TEMPLATE template0' WHERE NOT EXISTS \
				(SELECT FROM pg_database WHERE datname = '{name}')\\gexec\n\
				REVOKE ALL ON DATABASE {id} FROM PUBLIC;\n\
				ALTER DATABASE {id} OWNER TO {role};\n"
			));
		}
	}
	// nothing to do without a group database
	if !used(groups) {
		return String::new();
	}
	sql
}

/// psql as `postgres` in its own database, SQL on stdin, stops at the
/// first error.
pub const PSQL_ADMIN: [&str; 8] = [
	"runuser",
	"-u",
	"postgres",
	"--",
	"psql",
	"-qAtX",
	"-v",
	"ON_ERROR_STOP=1",
];

/// The plain SQL dump of a database, without owners and grants: it goes
/// back into any database of the group. A shell command, the name is
/// checked to be [a-z0-9_].
pub fn dump_command(name: &str) -> String {
	format!(
		"runuser -u postgres -- pg_dump --no-owner --no-acl --dbname={name}"
	)
}

/// psql as `postgres` in its own database (never a group's), for
/// creating and dropping.
pub fn admin_sql(sql: &str) -> Vec<String> {
	[
		"runuser",
		"-u",
		"postgres",
		"--",
		"psql",
		"-qX",
		"-v",
		"ON_ERROR_STOP=1",
		"-c",
		sql,
	]
	.iter()
	.map(|s| s.to_string())
	.collect()
}

/// Size in bytes and number of tables of a group's database, asked as
/// the group's own role (see the module's doc), without its psqlrc.
pub async fn info(group: &str, name: &str) -> Option<(i64, i64)> {
	let out = Command::new("runuser")
		.args(["-u", &render::group_user(group), "--", "psql", "-qAtX"])
		.arg(format!("--dbname={name}"))
		.args([
			"-F",
			" ",
			"-c",
			"SET search_path = pg_catalog, pg_temp; \
			SELECT pg_catalog.pg_database_size(pg_catalog.current_database()), \
			(SELECT pg_catalog.count(*) FROM information_schema.tables \
			WHERE table_schema NOT IN ('pg_catalog', 'information_schema'))",
		])
		.output()
		.await
		.ok()?;
	let text = String::from_utf8_lossy(&out.stdout);
	let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
	let (size, tables) = line.trim().split_once(' ')?;
	Some((size.parse().ok()?, tables.parse().ok()?))
}

/// Sizes of all databases, for the usage stats.
pub async fn sizes() -> Vec<(String, i64)> {
	if !installed() {
		return vec![];
	}
	sh(
		"runuser -u postgres -- psql -qAt -F ' ' -c \"SELECT datname, \
		pg_database_size(datname) FROM pg_database WHERE NOT datistemplate\"",
	)
	.await
	.unwrap_or_default()
	.lines()
	.filter_map(|l| {
		let (name, size) = l.split_once(' ')?;
		Some((name.to_string(), size.parse().ok()?))
	})
	.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::hosting::Database;

	#[test]
	fn creates_only_what_is_missing() {
		let g = Group {
			name: "shop".into(),
			databases: vec![
				Database {
					id: 1,
					group_id: 1,
					name: "shop_pg".into(),
					engine: Engine::Postgres,
				},
				Database {
					id: 2,
					group_id: 1,
					name: "shop_my".into(),
					engine: Engine::Mariadb,
				},
			],
			..Group::default()
		};
		let sql = setup_sql(&[g]);
		assert!(sql.contains("CREATE ROLE \"g-shop\" LOGIN"));
		assert!(sql.contains("WHERE rolname = 'g-shop'"));
		assert!(sql.contains("CREATE DATABASE \"shop_pg\" OWNER \"g-shop\""));
		assert!(sql.contains("REVOKE ALL ON DATABASE \"shop_pg\" FROM PUBLIC"));
		assert!(
			sql.contains("REVOKE CONNECT ON DATABASE postgres FROM PUBLIC")
		);
		assert!(!sql.contains("shop_my"));
		assert!(setup_sql(&[]).is_empty());
	}
}
