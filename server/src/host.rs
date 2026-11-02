//! What runs on the server for a group, read from the system.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::{Error, Result};
use crate::hosting::{Engine, Group};
use crate::render;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
	pub unit: String,
	pub load: String,
	pub active: String,
	pub sub: String,
	pub description: String,
}

pub async fn units(group: &str) -> Result<Vec<Unit>> {
	let out = Command::new("systemctl")
		.args(["list-units", "--all", "--output=json", "--no-pager"])
		// not `genco-{group}*`: group shop would list shop2's units
		.args([format!("genco-{group}-*"), format!("genco-{group}.slice")])
		.output()
		.await
		.map_err(|e| Error::Internal(format!("systemctl failed: {e}")))?;

	serde_json::from_slice(&out.stdout).map_err(|e| {
		Error::Internal(format!("unexpected systemctl output: {e}"))
	})
}

pub fn has_cert(domain: &str) -> bool {
	Path::new("/etc/letsencrypt/live")
		.join(domain)
		.join("fullchain.pem")
		.exists()
}

/// The domains of the groups that have a certificate.
pub fn certs(groups: &[Group]) -> HashSet<String> {
	groups
		.iter()
		.flat_map(|g| &g.domains)
		.filter(|d| has_cert(&d.name))
		.map(|d| d.name.clone())
		.collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigFile {
	pub path: String,
	/// what genco writes
	pub expected: String,
	/// what is on the disk, `None` if the file doesn't exist
	pub actual: Option<String>,
}

/// The config files of a group, as genco would write them and as they are.
pub async fn config_files(group: &Group) -> Vec<ConfigFile> {
	let certs = certs(std::slice::from_ref(group));
	let mut files = vec![];
	// root's alone (an app's environment with its secrets) stays out
	let shown = render::group_files(group, &certs)
		.into_iter()
		.filter(|f| f.private_to.as_deref() != Some("root"));
	for f in shown {
		files.push(ConfigFile {
			actual: tokio::fs::read_to_string(&f.path).await.ok(),
			path: f.path.display().to_string(),
			expected: f.content,
		});
	}
	files
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
	pub name: String,
	pub exists: bool,
	pub size_bytes: i64,
	pub tables: i64,
}

/// Size and table count of the group's databases in MariaDB and
/// PostgreSQL.
pub async fn databases(group: &Group) -> Result<Vec<DatabaseInfo>> {
	let mut infos: Vec<DatabaseInfo> = group
		.databases
		.iter()
		.map(|d| DatabaseInfo {
			name: d.name.clone(),
			exists: false,
			size_bytes: 0,
			tables: 0,
		})
		.collect();
	for d in group
		.databases
		.iter()
		.filter(|d| d.engine == Engine::Postgres)
	{
		if let Some((size, tables)) =
			crate::pg::info(&group.name, &d.name).await
			&& let Some(info) = infos.iter_mut().find(|i| i.name == d.name)
		{
			info.exists = true;
			info.size_bytes = size;
			info.tables = tables;
		}
	}
	let names: Vec<&str> = group
		.databases
		.iter()
		.filter(|d| d.engine == Engine::Mariadb)
		.map(|d| d.name.as_str())
		.collect();
	if names.is_empty() {
		return Ok(infos);
	}

	// the names are checked to be [a-z0-9_]
	let list = names
		.iter()
		.map(|n| format!("'{n}'"))
		.collect::<Vec<_>>()
		.join(",");
	let sql = format!(
		"SELECT s.schema_name, \
		COALESCE(SUM(t.data_length + t.index_length), 0), \
		COUNT(t.table_name) FROM information_schema.schemata s \
		LEFT JOIN information_schema.tables t \
		ON t.table_schema = s.schema_name \
		WHERE s.schema_name IN ({list}) GROUP BY s.schema_name"
	);
	// a failure is no "doesn't exist"
	let out = Command::new("mariadb")
		.args(["--batch", "--skip-column-names", "-e", &sql])
		.output()
		.await
		.map_err(|e| Error::Internal(format!("mariadb: {e}")))?;
	if !out.status.success() {
		return Err(Error::Internal(format!(
			"mariadb: {}",
			String::from_utf8_lossy(&out.stderr).trim()
		)));
	}

	for line in String::from_utf8_lossy(&out.stdout).lines() {
		let cols: Vec<&str> = line.split('\t').collect();
		if let [name, size, tables] = cols[..]
			&& let Some(info) = infos.iter_mut().find(|i| i.name == name)
		{
			info.exists = true;
			info.size_bytes = size.parse().unwrap_or(0);
			info.tables = tables.parse().unwrap_or(0);
		}
	}
	Ok(infos)
}
