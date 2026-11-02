//! Clients, groups, domains and databases. This is only the data for now,
//! `host` reads what runs for them on the server.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use std::sync::Arc;

use crate::AppState;
use crate::apply;
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::host;
use crate::render;
use crate::users::Authed;
use crate::util;

/// genco's own units are called genco-<name>, like a group's
pub const RESERVED: &[&str] = &["backup"];

pub const PHP_VERSIONS: &[&str] = &["8.2", "8.3", "8.4", "8.5"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
	Static,
	Php,
	Node,
	Rust,
	/// forwards to another web server (`upstream`), e.g. one that runs
	/// until the site moves into genco
	Proxy,
}

impl Kind {
	fn as_str(self) -> &'static str {
		match self {
			Self::Static => "static",
			Self::Php => "php",
			Self::Node => "node",
			Self::Rust => "rust",
			Self::Proxy => "proxy",
		}
	}

	fn parse(s: &str) -> Self {
		match s {
			"php" => Self::Php,
			"node" => Self::Node,
			"rust" => Self::Rust,
			"proxy" => Self::Proxy,
			_ => Self::Static,
		}
	}
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Client {
	pub id: i64,
	pub name: String,
	pub contact: String,
	pub notes: String,
	pub groups: Vec<Group>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
	pub id: i64,
	pub name: String,
	pub client_id: i64,
	pub notes: String,
	/// the linux user of the group
	pub user: String,
	pub dir: String,
	/// git repository the group deploys from, see `deploy`
	pub repo: Option<String>,
	pub branch: String,
	pub domains: Vec<Domain>,
	pub databases: Vec<Database>,
	pub mail_domains: Vec<crate::mailbox::MailDomain>,
	pub workers: Vec<crate::procs::Worker>,
	pub schedules: Vec<crate::procs::Schedule>,
	pub php: PhpSettings,
	/// password and Dovecot hash of its SMTP login `g-<group>`, made by
	/// apply once it has mail domains; never to the browser with the tree
	#[serde(skip)]
	pub smtp: Option<(String, String)>,
	/// the mail host, once it has a certificate: submission needs TLS
	/// for that name, apps connect to it (set by apply)
	#[serde(skip)]
	pub mail_host: Option<String>,
}

/// What a group's PHP may use, for all its PHP domains.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PhpSettings {
	/// the largest upload, nginx takes a bit more for the form around it
	pub upload_mb: u32,
	pub memory_mb: u32,
	pub max_execution_s: u32,
	/// form fields per request, Craft's big entries need more than PHP's
	/// 1000
	pub max_input_vars: u32,
}

impl Default for PhpSettings {
	fn default() -> Self {
		Self {
			upload_mb: 64,
			memory_mb: 256,
			max_execution_s: 60,
			max_input_vars: 5000,
		}
	}
}

impl PhpSettings {
	/// Within what a group's slice (1.5 GB) and nginx can take.
	pub fn check(&self) -> Result<()> {
		let ok = (1..=2048).contains(&self.upload_mb)
			&& (64..=1024).contains(&self.memory_mb)
			&& (10..=3600).contains(&self.max_execution_s)
			&& (100..=100_000).contains(&self.max_input_vars);
		if !ok {
			return Err(Error::Invalid(
				"PHP: upload 1 to 2048 MB, memory 64 to 1024 MB, time 10 to \
				3600 s, input vars 100 to 100000"
					.into(),
			));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Domain {
	pub id: i64,
	pub group_id: i64,
	pub name: String,
	pub kind: Kind,
	pub php_version: Option<String>,
	/// relative to the group's directory: what nginx serves for static
	/// and PHP, where the app runs for Node and Rust
	pub root: String,
	/// starts a Node or Rust app, run with `sh -c` in its directory
	pub command: Option<String>,
	/// of a Node or Rust app, relative to the group's directory: files
	/// nginx serves itself, the rest goes to the app
	pub public_dir: Option<String>,
	/// lines added to the domain's nginx server block
	pub nginx_extra: String,
	/// where a proxy domain forwards to, `http://127.0.0.1:8080`
	pub upstream: Option<String>,
	/// www.<name> redirects here
	pub www: bool,
	/// of a Node or Rust app, never to the browser (secrets), see appenv
	#[serde(skip)]
	pub env: Vec<crate::appenv::EnvVar>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Database {
	pub id: i64,
	pub group_id: i64,
	pub name: String,
	pub engine: Engine,
}

impl Database {
	/// The shell command that prints a plain SQL dump of it.
	pub fn dump_command(&self) -> String {
		match self.engine {
			Engine::Mariadb => {
				format!("{} {}", crate::util::MARIADB_DUMP, self.name)
			}
			Engine::Postgres => crate::pg::dump_command(&self.name),
		}
	}
}

/// Where a database lives; PostgreSQL is installed with its first one.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
	#[default]
	Mariadb,
	Postgres,
}

impl Engine {
	pub fn parse(s: &str) -> Self {
		if s == "postgres" {
			Self::Postgres
		} else {
			Self::Mariadb
		}
	}

	pub fn as_str(self) -> &'static str {
		match self {
			Self::Mariadb => "mariadb",
			Self::Postgres => "postgres",
		}
	}
}

/// All clients with their groups, domains and databases.
pub fn tree(conn: &Connection) -> rusqlite::Result<Vec<Client>> {
	let mut stmt = conn.prepare(
		"SELECT id, group_id, name, kind, php_version, root, command,
		nginx_extra, public_dir, env, upstream, www FROM domains
		ORDER BY name",
	)?;
	let domains: Vec<Domain> = stmt
		.query_map([], |row| {
			Ok(Domain {
				id: row.get(0)?,
				group_id: row.get(1)?,
				name: row.get(2)?,
				kind: Kind::parse(&row.get::<_, String>(3)?),
				php_version: row.get(4)?,
				root: row.get(5)?,
				command: row.get(6)?,
				nginx_extra: row.get(7)?,
				public_dir: row.get(8)?,
				env: crate::appenv::parse(&row.get::<_, String>(9)?),
				upstream: row.get(10)?,
				www: row.get(11)?,
			})
		})?
		.collect::<rusqlite::Result<_>>()?;

	let mut stmt = conn.prepare(
		"SELECT id, group_id, name, engine FROM databases ORDER BY name",
	)?;
	let databases: Vec<Database> = stmt
		.query_map([], |row| {
			Ok(Database {
				id: row.get(0)?,
				group_id: row.get(1)?,
				name: row.get(2)?,
				engine: Engine::parse(&row.get::<_, String>(3)?),
			})
		})?
		.collect::<rusqlite::Result<_>>()?;

	let mail_domains = crate::mailbox::load(conn)?;
	let (workers, schedules) = crate::procs::load(conn)?;

	let mut stmt = conn.prepare(
		"SELECT id, name, client_id, notes, repo, branch, php, smtp_password,
		smtp_hash FROM domain_groups ORDER BY name",
	)?;
	let groups: Vec<Group> = stmt
		.query_map([], |row| {
			let id = row.get(0)?;
			let name: String = row.get(1)?;
			Ok(Group {
				id,
				user: render::group_user(&name),
				dir: render::group_dir(&name).display().to_string(),
				name,
				client_id: row.get(2)?,
				notes: row.get(3)?,
				repo: row.get(4)?,
				branch: row.get(5)?,
				php: serde_json::from_str(&row.get::<_, String>(6)?)
					.unwrap_or_default(),
				smtp: match (row.get(7)?, row.get(8)?) {
					(Some(p), Some(h)) => Some((p, h)),
					_ => None,
				},
				mail_host: None,
				domains: util::matching(&domains, |d| d.group_id == id),
				databases: util::matching(&databases, |d| d.group_id == id),
				mail_domains: util::matching(&mail_domains, |d| {
					d.group_id == id
				}),
				workers: util::matching(&workers, |w| w.group_id == id),
				schedules: util::matching(&schedules, |s| s.group_id == id),
			})
		})?
		.collect::<rusqlite::Result<_>>()?;

	let mut stmt = conn.prepare(
		"SELECT id, name, contact, notes FROM clients ORDER BY name",
	)?;
	stmt.query_map([], |row| {
		let id = row.get(0)?;
		Ok(Client {
			id,
			name: row.get(1)?,
			contact: row.get(2)?,
			notes: row.get(3)?,
			groups: util::matching(&groups, |g| g.client_id == id),
		})
	})?
	.collect()
}

pub fn groups(conn: &Connection) -> rusqlite::Result<Vec<Group>> {
	Ok(tree(conn)?.into_iter().flat_map(|c| c.groups).collect())
}

pub fn find_group(conn: &Connection, id: i64) -> Result<Group> {
	groups(conn)?
		.into_iter()
		.find(|g| g.id == id)
		.ok_or(Error::NotFound)
}

/// A domain with its group, by the domain's id or name.
pub fn find_domain(
	conn: &Connection,
	is: impl Fn(&Domain) -> bool,
) -> Result<(Group, Domain)> {
	groups(conn)?
		.into_iter()
		.find_map(|g| {
			let d = g.domains.iter().find(|d| is(d))?.clone();
			Some((g, d))
		})
		.ok_or(Error::NotFound)
}

/// A database with its group, by the database's id or name.
pub fn find_database(
	conn: &Connection,
	is: impl Fn(&Database) -> bool,
) -> Result<(Group, Database)> {
	groups(conn)?
		.into_iter()
		.find_map(|g| {
			let d = g.databases.iter().find(|d| is(d))?.clone();
			Some((g, d))
		})
		.ok_or(Error::NotFound)
}

pub fn set_php(conn: &Connection, id: i64, php: &PhpSettings) -> Result<()> {
	php.check()?;
	conn.execute(
		"UPDATE domain_groups SET php = ?1 WHERE id = ?2",
		params![serde_json::to_string(php).unwrap(), id],
	)?;
	Ok(())
}

/// The repository a group deploys from and its branch, `None` keeps.
pub fn set_repo(
	conn: &Connection,
	id: i64,
	repo: Option<&str>,
	branch: Option<&str>,
) -> Result<()> {
	if let Some(repo) = repo.map(str::trim).filter(|r| !r.is_empty()) {
		check_git(repo, "repository")?;
		conn.execute(
			"UPDATE domain_groups SET repo = ?1 WHERE id = ?2",
			params![repo, id],
		)?;
	}
	if let Some(branch) = branch {
		check_git(branch.trim(), "branch")?;
		conn.execute(
			"UPDATE domain_groups SET branch = ?1 WHERE id = ?2",
			params![branch.trim(), id],
		)?;
	}
	Ok(())
}

/// `None` if there is no group with that name.
pub fn group_by_name(
	conn: &Connection,
	name: &str,
) -> rusqlite::Result<Option<Group>> {
	Ok(groups(conn)?.into_iter().find(|g| g.name == name))
}

/// Group names become linux users (`g-<name>`) and directories.
fn check_group_name(name: &str) -> Result<()> {
	let ok = !name.is_empty()
		&& name.len() <= 24
		&& name.starts_with(|c: char| c.is_ascii_lowercase())
		&& name
			.chars()
			.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
	// no "-": units are genco-<group>-<domain>, with "-" in the name group
	// "a" with domain "b-x.ch" and group "a-b" with "x.ch" would share one,
	// and systemd would nest the slice genco-a-b in genco-a
	if !ok {
		return Err(Error::Invalid(
			"group names have up to 24 characters a-z and 0-9, starting \
			with a letter"
				.into(),
		));
	}
	if RESERVED.contains(&name) {
		return Err(Error::Invalid(format!("{name} is reserved")));
	}
	Ok(())
}

pub fn check_domain_name(name: &str) -> Result<()> {
	let label_ok = |l: &str| {
		!l.is_empty()
			&& l.len() <= 63
			&& !l.starts_with('-')
			&& !l.ends_with('-')
			&& l.chars().all(|c| {
				c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
			})
	};
	if name.len() > 253 || !name.contains('.') || !name.split('.').all(label_ok)
	{
		return Err(Error::Invalid(format!("{name} is not a valid domain")));
	}
	Ok(())
}

fn check_database_name(name: &str) -> Result<()> {
	let ok = !name.is_empty()
		&& name.len() <= 64
		&& name
			.chars()
			.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
	if !ok {
		return Err(Error::Invalid(
			"database names have up to 64 characters a-z, 0-9 and _".into(),
		));
	}
	// a grant on them would give a group all databases
	if ["mysql", "sys", "performance_schema", "information_schema"]
		.contains(&name)
	{
		return Err(Error::Invalid(format!("{name} is MariaDB's own")));
	}
	if ["postgres", "template0", "template1"].contains(&name) {
		return Err(Error::Invalid(format!("{name} is PostgreSQL's own")));
	}
	Ok(())
}

/// Repositories and branches go to git: no option (-x), no whitespace.
fn check_git(value: &str, what: &str) -> Result<()> {
	if value.is_empty()
		|| value.starts_with('-')
		|| value.chars().any(|c| c.is_whitespace() || c.is_control())
	{
		return Err(Error::Invalid(format!("invalid {what}")));
	}
	Ok(())
}

/// Only apps have a public folder, empty is none; it stays inside the
/// group's directory like the root.
fn public_dir(kind: Kind, dir: &Option<String>) -> Result<Option<String>> {
	let dir = dir.as_deref().map(str::trim).filter(|d| !d.is_empty());
	match dir {
		Some(d) if matches!(kind, Kind::Node | Kind::Rust) => {
			check_root(d)?;
			Ok(Some(d.to_string()))
		}
		_ => Ok(None),
	}
}

/// The root stays inside the group's directory and is a plain path, it
/// goes into nginx and unit files.
pub fn check_root(root: &str) -> Result<()> {
	let plain = root
		.chars()
		.all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c));
	if !plain || root.starts_with('/') || root.split('/').any(|p| p == "..") {
		return Err(Error::Invalid(
			"the root is a path inside the group's directory".into(),
		));
	}
	Ok(())
}

fn check_command(kind: Kind, command: &Option<String>) -> Result<()> {
	let has = command.as_deref().is_some_and(|c| !c.trim().is_empty());
	match kind {
		Kind::Node | Kind::Rust if !has => Err(Error::Invalid(
			"Node and Rust apps need a command that starts them".into(),
		)),
		Kind::Static | Kind::Php if has => Err(Error::Invalid(
			"only Node and Rust apps have a command".into(),
		)),
		_ => Ok(()),
	}
}

/// Where a proxy domain forwards to: http(s), a host and a port, maybe a
/// path; it goes into nginx's config.
fn check_upstream(kind: Kind, upstream: &Option<String>) -> Result<()> {
	let upstream = upstream.as_deref().map(str::trim).filter(|u| !u.is_empty());
	match (kind, upstream) {
		(Kind::Proxy, Some(u)) => {
			let rest = u
				.strip_prefix("http://")
				.or_else(|| u.strip_prefix("https://"))
				.unwrap_or("");
			let plain = !rest.is_empty()
				&& !rest.starts_with(['/', ':'])
				&& rest.chars().all(|c| {
					c.is_ascii_alphanumeric() || "-._:/[]".contains(c)
				});
			if !plain {
				return Err(Error::Invalid(format!(
					"{u} is no address like http://127.0.0.1:8080"
				)));
			}
			Ok(())
		}
		(Kind::Proxy, None) => Err(Error::Invalid(
			"a proxy domain needs the address it forwards to".into(),
		)),
		(_, Some(_)) => {
			Err(Error::Invalid("only proxy domains forward".into()))
		}
		(_, None) => Ok(()),
	}
}

fn check_php(kind: Kind, php: &Option<String>) -> Result<()> {
	match (kind, php) {
		(Kind::Php, Some(v)) if PHP_VERSIONS.contains(&v.as_str()) => Ok(()),
		(Kind::Php, _) => Err(Error::Invalid(format!(
			"PHP domains need one of the versions {}",
			PHP_VERSIONS.join(", ")
		))),
		(_, None) => Ok(()),
		(_, Some(_)) => {
			Err(Error::Invalid("only PHP domains have a PHP version".into()))
		}
	}
}

async fn get_tree(
	_authed: Authed,
	State(db): State<Db>,
) -> Result<Json<Vec<Client>>> {
	Ok(Json(tree(&db.conn())?))
}

#[derive(Debug, Deserialize)]
pub struct ClientReq {
	pub name: String,
	#[serde(default)]
	pub contact: String,
	#[serde(default)]
	pub notes: String,
}

pub fn insert_client(conn: &Connection, req: &ClientReq) -> Result<i64> {
	if req.name.trim().is_empty() {
		return Err(Error::Invalid("the client needs a name".into()));
	}
	conn.execute(
		"INSERT INTO clients (name, contact, notes, created_at)
		VALUES (?1, ?2, ?3, ?4)",
		params![req.name.trim(), req.contact, req.notes, now()],
	)?;
	Ok(conn.last_insert_rowid())
}

async fn create_client(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Json(req): Json<ClientReq>,
) -> Result<Json<i64>> {
	let id = insert_client(&db.conn(), &req)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

async fn update_client(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(req): Json<ClientReq>,
) -> Result<Json<()>> {
	if req.name.trim().is_empty() {
		return Err(Error::Invalid("the client needs a name".into()));
	}
	let n = db.conn().execute(
		"UPDATE clients SET name = ?1, contact = ?2, notes = ?3 WHERE id = ?4",
		params![req.name.trim(), req.contact, req.notes, id],
	)?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	events.send(Event::Tree);
	Ok(Json(()))
}

/// Only clients without groups, deleting a group is a separate step.
async fn delete_client(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	db.conn()
		.execute("DELETE FROM clients WHERE id = ?1", [id])?;
	events.send(Event::Tree);
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupReq {
	pub name: String,
	pub client_id: i64,
	#[serde(default)]
	pub notes: String,
}

pub fn insert_group(conn: &Connection, req: &GroupReq) -> Result<i64> {
	check_group_name(&req.name)?;
	conn.execute(
		"INSERT INTO domain_groups (name, client_id, notes, created_at)
		VALUES (?1, ?2, ?3, ?4)",
		params![req.name, req.client_id, req.notes, now()],
	)?;
	Ok(conn.last_insert_rowid())
}

async fn create_group(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Json(req): Json<GroupReq>,
) -> Result<Json<i64>> {
	let id = insert_group(&db.conn(), &req)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateGroupReq {
	client_id: i64,
	#[serde(default)]
	notes: String,
	repo: Option<String>,
	branch: String,
	/// unchanged when missing
	php: Option<PhpSettings>,
}

/// The name stays, it is the linux user and the directory.
async fn update_group(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(req): Json<UpdateGroupReq>,
) -> Result<Json<()>> {
	let repo = req.repo.as_deref().map(str::trim).filter(|r| !r.is_empty());
	if let Some(repo) = repo {
		check_git(repo, "repository")?;
	}
	check_git(req.branch.trim(), "branch")?;
	// all or nothing: wrong PHP settings change nothing else either
	if let Some(php) = &req.php {
		php.check()?;
	}
	let n = db.conn().execute(
		"UPDATE domain_groups SET client_id = ?1, notes = ?2, repo = ?3,
		branch = ?4 WHERE id = ?5",
		params![req.client_id, req.notes, repo, req.branch.trim(), id],
	)?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	if let Some(php) = req.php {
		set_php(&db.conn(), id, &php)?;
	}
	events.send(Event::Tree);
	Ok(Json(()))
}

/// How a domain is served, the same when it is added and changed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainSettings {
	pub kind: Kind,
	pub php_version: Option<String>,
	pub root: String,
	pub command: Option<String>,
	#[serde(default)]
	pub public_dir: Option<String>,
	#[serde(default)]
	pub nginx_extra: String,
	#[serde(default)]
	pub upstream: Option<String>,
	/// www.<name> redirects to the domain
	#[serde(default)]
	pub www: bool,
}

impl DomainSettings {
	/// The public dir as stored, if the settings are valid.
	fn check(&self) -> Result<Option<String>> {
		check_php(self.kind, &self.php_version)?;
		check_command(self.kind, &self.command)?;
		check_upstream(self.kind, &self.upstream)?;
		check_root(&self.root)?;
		public_dir(self.kind, &self.public_dir)
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainReq {
	pub group_id: i64,
	pub name: String,
	#[serde(flatten)]
	pub settings: DomainSettings,
}

/// Trimmed, only for proxies.
fn upstream(s: &DomainSettings) -> Option<String> {
	s.upstream
		.as_deref()
		.map(str::trim)
		.filter(|u| !u.is_empty() && s.kind == Kind::Proxy)
		.map(String::from)
}

/// A domain and the www redirect of another can't have the same name.
fn check_www(conn: &Connection, name: &str, www: bool) -> Result<()> {
	let taken = |n: &str, www_only: bool| -> Result<bool> {
		let sql = if www_only {
			"SELECT count(*) FROM domains WHERE name = ?1 AND www = 1"
		} else {
			"SELECT count(*) FROM domains WHERE name = ?1"
		};
		Ok(conn.query_row(sql, [n], |r| r.get::<_, i64>(0))? > 0)
	};
	if www && taken(&format!("www.{name}"), false)? {
		return Err(Error::Invalid(format!(
			"www.{name} is a domain of its own, delete it first or leave \
			www off"
		)));
	}
	if let Some(base) = name.strip_prefix("www.")
		&& taken(base, true)?
	{
		return Err(Error::Invalid(format!(
			"{base} redirects {name} already (its www option)"
		)));
	}
	Ok(())
}

pub fn insert_domain(conn: &Connection, req: &DomainReq) -> Result<i64> {
	let name = req.name.trim().to_lowercase();
	check_domain_name(&name)?;
	let s = &req.settings;
	let public_dir = s.check()?;
	check_www(conn, &name, s.www)?;

	conn.execute(
		"INSERT INTO domains (group_id, name, kind, php_version, root,
		command, nginx_extra, created_at, public_dir, upstream, www)
		VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
		params![
			req.group_id,
			name,
			s.kind.as_str(),
			s.php_version,
			s.root,
			s.command,
			s.nginx_extra,
			now(),
			public_dir,
			upstream(s),
			s.www
		],
	)?;
	Ok(conn.last_insert_rowid())
}

async fn create_domain(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Json(req): Json<DomainReq>,
) -> Result<Json<i64>> {
	let id = insert_domain(&db.conn(), &req)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

pub fn set_domain(
	conn: &Connection,
	id: i64,
	req: &DomainSettings,
) -> Result<()> {
	let public_dir = req.check()?;
	let name: String =
		conn.query_row("SELECT name FROM domains WHERE id = ?1", [id], |r| {
			r.get(0)
		})?;
	check_www(conn, &name, req.www)?;

	let n = conn.execute(
		"UPDATE domains SET kind = ?1, php_version = ?2, root = ?3,
		command = ?4, nginx_extra = ?5, public_dir = ?6, upstream = ?7,
		www = ?8 WHERE id = ?9",
		params![
			req.kind.as_str(),
			req.php_version,
			req.root,
			req.command,
			req.nginx_extra,
			public_dir,
			upstream(req),
			req.www,
			id
		],
	)?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	Ok(())
}

async fn update_domain(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(req): Json<DomainSettings>,
) -> Result<Json<()>> {
	set_domain(&db.conn(), id, &req)?;
	events.send(Event::Tree);
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseReq {
	pub group_id: i64,
	pub name: String,
	#[serde(default)]
	pub engine: Engine,
}

pub fn insert_database(conn: &Connection, req: &DatabaseReq) -> Result<i64> {
	check_database_name(&req.name)?;
	// PostgreSQL cuts longer names silently: two would share one database
	if req.engine == Engine::Postgres && req.name.len() > 63 {
		return Err(Error::Invalid(
			"PostgreSQL database names have up to 63 characters".into(),
		));
	}
	conn.execute(
		"INSERT INTO databases (group_id, name, engine, created_at)
		VALUES (?1, ?2, ?3, ?4)",
		params![req.group_id, req.name, req.engine.as_str(), now()],
	)?;
	Ok(conn.last_insert_rowid())
}

async fn create_database(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Json(req): Json<DatabaseReq>,
) -> Result<Json<i64>> {
	let id = insert_database(&db.conn(), &req)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

async fn group_services(
	_authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<Vec<host::Unit>>> {
	let group = find_group(&db.conn(), id)?;
	Ok(Json(host::units(&group.name).await?))
}

async fn group_config(
	_authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<Vec<host::ConfigFile>>> {
	let group = find_group(&db.conn(), id)?;
	Ok(Json(host::config_files(&group).await))
}

async fn group_databases(
	_authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<Vec<host::DatabaseInfo>>> {
	let group = find_group(&db.conn(), id)?;
	Ok(Json(host::databases(&group).await?))
}

async fn apply_all(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
) -> Result<Json<apply::Report>> {
	let report = apply::apply(&cfg, &db).await;
	events.send(Event::Tree);
	Ok(Json(report))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/apply", post(apply_all))
		.route("/tree", get(get_tree))
		.route("/clients", post(create_client))
		.route("/clients/{id}", post(update_client).delete(delete_client))
		.route("/groups", post(create_group))
		.route("/groups/{id}", post(update_group))
		.route("/groups/{id}/services", get(group_services))
		.route("/groups/{id}/config", get(group_config))
		.route("/groups/{id}/databases", get(group_databases))
		.route("/domains", post(create_domain))
		.route("/domains/{id}", post(update_domain))
		.route("/databases", post(create_database))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn group_names() {
		assert!(check_group_name("shop").is_ok());
		assert!(check_group_name("shop2").is_ok());
		assert!(check_group_name("my-shop").is_err());
		assert!(check_group_name("2fast").is_err());
		assert!(check_group_name("Shop").is_err());
		assert!(check_group_name("a/b").is_err());
		assert!(check_group_name(&"a".repeat(25)).is_err());
		assert!(check_group_name("backup").is_err());
	}

	#[test]
	fn domain_names() {
		assert!(check_domain_name("shop.ch").is_ok());
		assert!(check_domain_name("admin.shop.example.com").is_ok());
		assert!(check_domain_name("localhost").is_err());
		assert!(check_domain_name("-a.ch").is_err());
		assert!(check_domain_name("a..ch").is_err());
		assert!(check_domain_name("a b.ch").is_err());
	}

	#[test]
	fn www_and_a_domain_of_that_name_exclude_each_other() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
			VALUES ('g', 1, 0);",
		)
		.unwrap();
		conn.execute(
			"INSERT INTO domains (group_id, name, kind, created_at, www)
			VALUES (1, 'shop.ch', 'static', 0, 1),
			(1, 'www.blog.ch', 'static', 0, 0)",
			[],
		)
		.unwrap();
		assert!(check_www(&conn, "www.shop.ch", false).is_err());
		assert!(check_www(&conn, "blog.ch", true).is_err());
		assert!(check_www(&conn, "blog.ch", false).is_ok());
		assert!(check_www(&conn, "other.ch", true).is_ok());
	}

	#[test]
	fn roots_stay_inside() {
		assert!(check_root("public").is_ok());
		assert!(check_root("craft/web").is_ok());
		assert!(check_root("/etc").is_err());
		assert!(check_root("../other").is_err());
		assert!(check_root("web\nExecStartPre=+/bin/sh").is_err());
		assert!(check_root("a b").is_err());
	}

	#[test]
	fn upstreams_only_for_proxies() {
		let u = |s: &str| Some(s.to_string());
		assert!(
			check_upstream(Kind::Proxy, &u("http://127.0.0.1:8081")).is_ok()
		);
		assert!(
			check_upstream(Kind::Proxy, &u("https://old.example.ch/")).is_ok()
		);
		assert!(check_upstream(Kind::Proxy, &None).is_err());
		assert!(check_upstream(Kind::Proxy, &u("ftp://x")).is_err());
		assert!(
			check_upstream(Kind::Proxy, &u("http://a; return 200")).is_err()
		);
		assert!(check_upstream(Kind::Proxy, &u("http://a}\nserver{")).is_err());
		assert!(check_upstream(Kind::Php, &u("http://a")).is_err());
		assert!(check_upstream(Kind::Php, &None).is_ok());
	}

	#[test]
	fn php_version_only_for_php() {
		assert!(check_php(Kind::Php, &Some("8.4".into())).is_ok());
		assert!(check_php(Kind::Php, &None).is_err());
		assert!(check_php(Kind::Php, &Some("7.4".into())).is_err());
		assert!(check_php(Kind::Node, &Some("8.4".into())).is_err());
		assert!(check_php(Kind::Static, &None).is_ok());
	}

	#[test]
	fn tree_nests_groups_and_domains() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('acme', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
				VALUES ('shop', 1, 0);
			INSERT INTO domains (group_id, name, kind, created_at)
				VALUES (1, 'shop.ch', 'node', 0);
			INSERT INTO databases (group_id, name, created_at)
				VALUES (1, 'shop', 0);",
		)
		.unwrap();

		let tree = tree(&conn).unwrap();
		let group = &tree[0].groups[0];
		assert_eq!(group.user, "g-shop");
		assert_eq!(group.domains[0].kind, Kind::Node);
		assert_eq!(group.databases[0].name, "shop");
	}
}
