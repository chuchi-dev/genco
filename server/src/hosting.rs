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
use crate::limits::{self, Limits};
use crate::render;
use crate::users::Authed;
use crate::util;

/// genco's own units are called genco-<name>, like a group's
pub const RESERVED: &[&str] = &["backup"];

pub const PHP_VERSIONS: &[&str] = &["8.2", "8.3", "8.4", "8.5"];

/// What a redirect domain answers: 301 and 302 may turn a POST into a GET,
/// 308 and 307 keep it; 302 and 307 are temporary.
pub const REDIRECT_STATUSES: &[u16] = &[301, 302, 307, 308];

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
	/// sends every request to another address (`target`), www.<name> too
	Redirect,
}

impl Kind {
	fn as_str(self) -> &'static str {
		match self {
			Self::Static => "static",
			Self::Php => "php",
			Self::Node => "node",
			Self::Rust => "rust",
			Self::Proxy => "proxy",
			Self::Redirect => "redirect",
		}
	}

	fn parse(s: &str) -> Self {
		match s {
			"php" => Self::Php,
			"node" => Self::Node,
			"rust" => Self::Rust,
			"proxy" => Self::Proxy,
			"redirect" => Self::Redirect,
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
	/// what its slice may use, the panel gets the defaults too
	#[serde(serialize_with = "limits::with_defaults")]
	pub limits: Limits,
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
	/// Within what a group's default slice (1.5 GB) and nginx can take.
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
	/// where a redirect domain sends its requests, `https://example.ch/`
	pub target: Option<String>,
	/// a redirect appends the request's path and query to the target
	pub keep_path: bool,
	/// of a redirect: 301, 302, 307 or 308
	pub redirect_status: u16,
	/// www.<name> redirects here (a redirect domain's straight to its
	/// target)
	pub www: bool,
	/// of a Node or Rust app: a restart starts the new process beside the
	/// old one and switches nginx over once it listens, see render::Slot
	pub zero_downtime: bool,
	/// slot B's unit and port are the live ones (only with zero_downtime)
	#[serde(skip)]
	pub slot_b: bool,
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
		nginx_extra, public_dir, env, upstream, www, zero_downtime, slot_b,
		target, keep_path, redirect_status
		FROM domains
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
				zero_downtime: row.get(12)?,
				slot_b: row.get(13)?,
				target: row.get(14)?,
				keep_path: row.get(15)?,
				redirect_status: row.get(16)?,
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
		smtp_hash, memory_max_mb, memory_high_mb, cpu_percent, tasks_max
		FROM domain_groups ORDER BY name",
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
				limits: Limits {
					memory_max_mb: row.get(9)?,
					memory_high_mb: row.get(10)?,
					cpu_percent: row.get(11)?,
					tasks_max: row.get(12)?,
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

/// Checks the limits against the server, warns when the groups' memory
/// limits together are more than it has.
pub fn set_limits(
	conn: &Connection,
	id: i64,
	limits: &Limits,
) -> Result<Option<String>> {
	limits::check_here(limits)?;
	conn.execute(
		"UPDATE domain_groups SET memory_max_mb = ?1, memory_high_mb = ?2,
		cpu_percent = ?3, tasks_max = ?4 WHERE id = ?5",
		params![
			limits.memory_max_mb,
			limits.memory_high_mb,
			limits.cpu_percent,
			limits.tasks_max,
			id
		],
	)?;
	Ok(memory_warning(conn)?)
}

/// When the groups' memory limits add up to more than the server has.
fn memory_warning(conn: &Connection) -> rusqlite::Result<Option<String>> {
	let (ram, swap) = limits::server_memory();
	let all = groups(conn)?.into_iter().map(|g| g.limits);
	Ok(limits::overcommit(all, ram, swap))
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

pub fn check_database_name(name: &str) -> Result<()> {
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

/// Where a redirect domain sends its requests, as stored: an absolute
/// http(s) URL, re-serialized by the url crate (IDN to punycode) and then
/// only of characters that are plain in nginx's config. Not the domain
/// itself (or its www name), that would loop.
fn check_target(s: &DomainSettings, name: &str) -> Result<Option<String>> {
	let target = s.target.as_deref().map(str::trim).filter(|t| !t.is_empty());
	let t = match (s.kind, target) {
		(Kind::Redirect, Some(t)) => t,
		(Kind::Redirect, None) => {
			return Err(Error::Invalid(
				"a redirect domain needs the address it sends to".into(),
			));
		}
		(_, Some(_)) => {
			return Err(Error::Invalid(
				"only redirect domains redirect".into(),
			));
		}
		(_, None) => return Ok(None),
	};
	if !REDIRECT_STATUSES.contains(&s.redirect_status) {
		return Err(Error::Invalid(
			"a redirect answers 301, 302, 307 or 308".into(),
		));
	}

	let invalid = || {
		Error::Invalid(format!(
			"{t} is no address like https://example.ch/ (http or https, \
			no spaces, quotes, ; {{ }} or $)"
		))
	};
	// the url crate drops tabs and newlines silently and encodes quotes
	// and braces, an address with them is a mistake
	let odd = |c: char| {
		c.is_whitespace() || c.is_control() || "\"'`{};$\\<>".contains(c)
	};
	if t.len() > 2000 || t.chars().any(odd) {
		return Err(invalid());
	}
	let url = reqwest::Url::parse(t).map_err(|_| invalid())?;
	let host = url.host_str().unwrap_or("");
	if !matches!(url.scheme(), "http" | "https")
		|| host.is_empty()
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.fragment().is_some()
	{
		return Err(invalid());
	}
	let out = url.as_str();
	let plain = out.chars().all(|c| {
		c.is_ascii_alphanumeric() || "-._~:/?&=%+,!*@[]()".contains(c)
	});
	if !plain {
		return Err(invalid());
	}
	if s.keep_path && url.query().is_some() {
		return Err(Error::Invalid(
			"with the path kept the target can't have a query, the \
			request's comes after it"
				.into(),
		));
	}

	// every path of the domain redirects, any address on it loops
	let www = format!("www.{name}");
	if url.port().is_none() && (host == name || (s.www && host == www)) {
		return Err(Error::Invalid(format!("{name} can't redirect to itself")));
	}
	Ok(Some(out.to_string()))
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
	let mut clients = tree(&db.conn())?;
	// without systemctl the tree still shows, only without states
	let units = host::worker_units().await.unwrap_or_default();
	set_worker_states(&mut clients, &units);
	Ok(Json(clients))
}

/// Each worker's ActiveState, from the units systemd knows.
fn set_worker_states(clients: &mut [Client], units: &[host::Unit]) {
	for g in clients.iter_mut().flat_map(|c| &mut c.groups) {
		for w in &mut g.workers {
			let unit = render::worker_unit(&g.name, &w.name);
			w.state = units
				.iter()
				.find(|u| u.unit == unit)
				.map(|u| u.active.clone());
		}
	}
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
	/// unchanged when missing
	limits: Option<Limits>,
}

#[derive(Debug, Serialize)]
struct UpdateGroupResp {
	/// the groups' memory limits add up to more than the server has
	warning: Option<String>,
}

/// The name stays, it is the linux user and the directory.
async fn update_group(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(req): Json<UpdateGroupReq>,
) -> Result<Json<UpdateGroupResp>> {
	let repo = req.repo.as_deref().map(str::trim).filter(|r| !r.is_empty());
	if let Some(repo) = repo {
		check_git(repo, "repository")?;
	}
	check_git(req.branch.trim(), "branch")?;
	// all or nothing: wrong PHP settings or limits change nothing else
	// either
	let conn = db.conn();
	let tx = conn.unchecked_transaction()?;
	let n = tx.execute(
		"UPDATE domain_groups SET client_id = ?1, notes = ?2, repo = ?3,
		branch = ?4 WHERE id = ?5",
		params![req.client_id, req.notes, repo, req.branch.trim(), id],
	)?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	if let Some(php) = req.php {
		set_php(&tx, id, &php)?;
	}
	let warning = match req.limits {
		Some(l) => set_limits(&tx, id, &l)?,
		None => None,
	};
	tx.commit()?;
	events.send(Event::Tree);
	Ok(Json(UpdateGroupResp { warning }))
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
	#[serde(default)]
	pub target: Option<String>,
	#[serde(default = "yes")]
	pub keep_path: bool,
	#[serde(default = "moved")]
	pub redirect_status: u16,
	/// www.<name> redirects to the domain
	#[serde(default)]
	pub www: bool,
	/// Node and Rust: restarts without downtime
	#[serde(default)]
	pub zero_downtime: bool,
}

impl DomainSettings {
	/// The public dir as stored, if the settings are valid.
	fn check(&self) -> Result<Option<String>> {
		check_php(self.kind, &self.php_version)?;
		check_command(self.kind, &self.command)?;
		check_upstream(self.kind, &self.upstream)?;
		check_root(&self.root)?;
		if self.zero_downtime && !matches!(self.kind, Kind::Node | Kind::Rust) {
			return Err(Error::Invalid(
				"only Node and Rust apps restart without downtime".into(),
			));
		}
		public_dir(self.kind, &self.public_dir)
	}
}

fn yes() -> bool {
	true
}

fn moved() -> u16 {
	301
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

/// The panel's names have nginx sites of their own, a domain (or its www
/// redirect) can't take one: the two sites would share a config file.
fn check_panel_names(
	cfg: &Config,
	conn: &Connection,
	name: &str,
	www: bool,
) -> Result<()> {
	let www_name = format!("www.{name}");
	let names = [Some(name), www.then_some(www_name.as_str())];
	for n in names.into_iter().flatten() {
		if cfg.domain.as_deref() == Some(n) {
			return Err(Error::Invalid(format!("{n} is the panel's name")));
		}
		if crate::mailbox::host(cfg).as_deref() == Some(n) {
			return Err(Error::Invalid(format!("{n} is the mail host")));
		}
		if crate::panel::old_names(conn).iter().any(|o| o == n) {
			return Err(Error::Invalid(format!(
				"{n} is an old name of the panel, genco panel forget-old \
				{n} first"
			)));
		}
	}
	Ok(())
}

pub fn insert_domain(
	cfg: &Config,
	conn: &Connection,
	req: &DomainReq,
) -> Result<i64> {
	let name = req.name.trim().to_lowercase();
	check_domain_name(&name)?;
	let s = &req.settings;
	let public_dir = s.check()?;
	let target = check_target(s, &name)?;
	check_www(conn, &name, s.www)?;
	check_panel_names(cfg, conn, &name, s.www)?;

	conn.execute(
		"INSERT INTO domains (group_id, name, kind, php_version, root,
		command, nginx_extra, created_at, public_dir, upstream, www,
		zero_downtime, target, keep_path, redirect_status)
		VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
		?15)",
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
			s.www,
			s.zero_downtime,
			target,
			s.keep_path,
			s.redirect_status
		],
	)?;
	let id = conn.last_insert_rowid();
	if id > render::MAX_DOMAIN_ID {
		conn.execute("DELETE FROM domains WHERE id = ?1", [id])?;
		return Err(Error::Invalid(format!(
			"no more domains: the ids above {} would share their app's \
			port with another",
			render::MAX_DOMAIN_ID
		)));
	}
	Ok(id)
}

async fn create_domain(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	Json(req): Json<DomainReq>,
) -> Result<Json<i64>> {
	let id = insert_domain(&cfg, &db.conn(), &req)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

pub fn set_domain(
	cfg: &Config,
	conn: &Connection,
	id: i64,
	req: &DomainSettings,
) -> Result<()> {
	let public_dir = req.check()?;
	let name: String =
		conn.query_row("SELECT name FROM domains WHERE id = ?1", [id], |r| {
			r.get(0)
		})?;
	let target = check_target(req, &name)?;
	check_www(conn, &name, req.www)?;
	check_panel_names(cfg, conn, &name, req.www)?;

	let n = conn.execute(
		"UPDATE domains SET kind = ?1, php_version = ?2, root = ?3,
		command = ?4, nginx_extra = ?5, public_dir = ?6, upstream = ?7,
		www = ?8, zero_downtime = ?9,
		-- switched off, slot A is the live one again (apply starts it)
		slot_b = slot_b AND ?9, target = ?11, keep_path = ?12,
		redirect_status = ?13 WHERE id = ?10",
		params![
			req.kind.as_str(),
			req.php_version,
			req.root,
			req.command,
			req.nginx_extra,
			public_dir,
			upstream(req),
			req.www,
			req.zero_downtime,
			id,
			target,
			req.keep_path,
			req.redirect_status
		],
	)?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	Ok(())
}

async fn update_domain(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(req): Json<DomainSettings>,
) -> Result<Json<()>> {
	set_domain(&cfg, &db.conn(), id, &req)?;
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
	crate::engines::check_database_free(&db, &req.name, req.engine).await?;
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

	fn cfg() -> Config {
		toml::from_str("domain = \"panel.example.ch\"").unwrap()
	}

	#[test]
	fn the_panels_names_are_no_domains() {
		let db = Db::memory();
		let conn = db.conn();
		crate::db::set_setting(
			&conn,
			"panel:old-names",
			r#"["old.example.ch", "www.shop.ch"]"#,
		)
		.unwrap();
		let check = |name, www| check_panel_names(&cfg(), &conn, name, www);
		assert!(check("panel.example.ch", false).is_err());
		assert!(check("mail.panel.example.ch", false).is_err());
		let err = check("old.example.ch", false).unwrap_err().to_string();
		assert!(err.contains("forget-old old.example.ch"), "{err}");
		assert!(check("example.ch", false).is_ok());
		assert!(check("shop.ch", false).is_ok());
		assert!(check("shop.ch", true).is_err());
	}

	#[test]
	fn domain_ids_stop_before_their_ports_collide() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
			VALUES ('g', 1, 0);
			INSERT INTO domains (id, group_id, name, kind, created_at)
			VALUES (9998, 1, 'a.ch', 'static', 0);",
		)
		.unwrap();
		let req = |name: &str| DomainReq {
			group_id: 1,
			name: name.into(),
			settings: DomainSettings {
				kind: Kind::Static,
				php_version: None,
				root: "public".into(),
				command: None,
				public_dir: None,
				nginx_extra: String::new(),
				upstream: None,
				target: None,
				keep_path: true,
				redirect_status: 301,
				www: false,
				zero_downtime: false,
			},
		};
		assert_eq!(insert_domain(&cfg(), &conn, &req("b.ch")).unwrap(), 9999);
		assert!(insert_domain(&cfg(), &conn, &req("c.ch")).is_err());
		let n: i64 = conn
			.query_row("SELECT count(*) FROM domains", [], |r| r.get(0))
			.unwrap();
		assert_eq!(n, 2);
	}

	#[test]
	fn zero_downtime_only_for_apps_and_off_goes_back_to_slot_a() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
			VALUES ('g', 1, 0);
			INSERT INTO domains (group_id, name, kind, created_at,
			command, zero_downtime, slot_b)
			VALUES (1, 'shop.ch', 'node', 0, 'node index.js', 1, 1);",
		)
		.unwrap();
		let mut s = DomainSettings {
			kind: Kind::Php,
			php_version: Some("8.4".into()),
			root: "public".into(),
			command: None,
			public_dir: None,
			nginx_extra: String::new(),
			upstream: None,
			target: None,
			keep_path: true,
			redirect_status: 301,
			www: false,
			zero_downtime: true,
		};
		assert!(set_domain(&cfg(), &conn, 1, &s).is_err());

		s.kind = Kind::Node;
		s.php_version = None;
		s.command = Some("node index.js".into());
		set_domain(&cfg(), &conn, 1, &s).unwrap();
		let slot = |conn: &Connection| -> bool {
			conn.query_row("SELECT slot_b FROM domains", [], |r| r.get(0))
				.unwrap()
		};
		assert!(slot(&conn));
		s.zero_downtime = false;
		set_domain(&cfg(), &conn, 1, &s).unwrap();
		assert!(!slot(&conn));
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

	fn redirect(target: &str) -> DomainSettings {
		DomainSettings {
			kind: Kind::Redirect,
			php_version: None,
			root: "public".into(),
			command: None,
			public_dir: None,
			nginx_extra: String::new(),
			upstream: None,
			target: Some(target.into()),
			keep_path: true,
			redirect_status: 301,
			www: false,
			zero_downtime: false,
		}
	}

	#[test]
	fn redirect_targets_are_plain_urls() {
		let ok = |t: &str| check_target(&redirect(t), "s-me.ch").unwrap();
		assert_eq!(
			ok("https://soerenmeier.ch").unwrap(),
			"https://soerenmeier.ch/"
		);
		assert_eq!(
			ok(" http://Example.CH:8080/a/b ").unwrap(),
			"http://example.ch:8080/a/b"
		);
		assert_eq!(
			ok("https://bücher.ch/").unwrap(),
			"https://xn--bcher-kva.ch/"
		);
		assert!(ok("https://[::1]/").is_some());

		let bad = |t: &str| check_target(&redirect(t), "s-me.ch").is_err();
		assert!(bad(""));
		assert!(bad("example.ch"));
		assert!(bad("/path"));
		assert!(bad("ftp://example.ch/"));
		assert!(bad("javascript:alert(1)"));
		assert!(bad("https://example.ch/; return 200"));
		assert!(bad("https://example.ch/;"));
		assert!(bad("https://example.ch/$host"));
		assert!(bad("https://example.ch/\nserver {"));
		assert!(bad("https://example.ch/a\tb"));
		assert!(bad("https://example.ch/a'b"));
		assert!(bad("https://example.ch/a\"b"));
		assert!(bad("https://example.ch/{x}"));
		assert!(bad("https://example.ch/a\\b"));
		assert!(bad("https://user:pw@example.ch/"));
		assert!(bad("https://example.ch/#top"));
		assert!(bad(&format!("https://example.ch/{}", "a".repeat(2000))));
		// the request's query would follow the target's
		assert!(bad("https://example.ch/?a=1"));
		let mut s = redirect("https://example.ch/?a=1");
		s.keep_path = false;
		assert!(check_target(&s, "s-me.ch").is_ok());

		// loops
		assert!(bad("https://s-me.ch/"));
		assert!(bad("http://S-ME.ch/other"));
		assert!(ok("https://www.s-me.ch/").is_some());
		let mut s = redirect("https://www.s-me.ch/");
		s.www = true;
		assert!(check_target(&s, "s-me.ch").is_err());
		assert!(ok("https://s-me.ch:8443/").is_some());

		let mut s = redirect("https://example.ch/");
		s.redirect_status = 200;
		assert!(check_target(&s, "s-me.ch").is_err());
		s.redirect_status = 308;
		assert!(check_target(&s, "s-me.ch").is_ok());
		s.kind = Kind::Static;
		assert!(check_target(&s, "s-me.ch").is_err());
		s.target = None;
		assert!(check_target(&s, "s-me.ch").unwrap().is_none());
		let mut s = redirect("");
		assert!(check_target(&s, "s-me.ch").is_err());
		s.target = None;
		assert!(check_target(&s, "s-me.ch").is_err());
	}

	#[test]
	fn an_app_becomes_a_redirect_and_then_static() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
			VALUES ('g', 1, 0);
			INSERT INTO domains (group_id, name, kind, created_at, command,
			zero_downtime, slot_b)
			VALUES (1, 's-me.ch', 'node', 0, 'node index.js', 1, 1);",
		)
		.unwrap();
		let mut s = redirect("https://soerenmeier.ch");
		s.redirect_status = 308;
		s.keep_path = false;
		s.www = true;
		set_domain(&cfg(), &conn, 1, &s).unwrap();
		let (_, d) = find_domain(&conn, |d| d.id == 1).unwrap();
		assert_eq!(d.kind, Kind::Redirect);
		assert_eq!(d.target.as_deref(), Some("https://soerenmeier.ch/"));
		assert_eq!(d.redirect_status, 308);
		assert!(!d.keep_path && d.www);
		// the app is gone: no command, slot A, no unit to render
		assert!(d.command.is_none() && !d.slot_b && !d.zero_downtime);
		assert!(!render::has_app(&d));

		s.kind = Kind::Static;
		assert!(set_domain(&cfg(), &conn, 1, &s).is_err());
		s.target = None;
		set_domain(&cfg(), &conn, 1, &s).unwrap();
		let (_, d) = find_domain(&conn, |d| d.id == 1).unwrap();
		assert_eq!(d.kind, Kind::Static);
		assert!(d.target.is_none());
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

	#[test]
	fn tree_has_workers_with_port_address_and_state() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('acme', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
				VALUES ('packages', 1, 0);
			INSERT INTO workers (group_id, name, dir, command, port, address,
				source, created_at)
				VALUES (1, 'server', 'current', './packages-server', 5426,
				'packages.p2.goodserver.ch:5426', 'panel', 0),
				(1, 'other', 'current', './other', NULL, '', 'repo', 0);",
		)
		.unwrap();

		let mut clients = tree(&conn).unwrap();
		let unit = |name: &str, active: &str| host::Unit {
			unit: name.into(),
			load: "loaded".into(),
			active: active.into(),
			sub: "running".into(),
			description: String::new(),
		};
		set_worker_states(
			&mut clients,
			&[unit("genco-packages-worker-server.service", "failed")],
		);

		let json = serde_json::to_value(&clients).unwrap();
		let workers = &json[0]["groups"][0]["workers"];
		assert_eq!(workers[0]["name"], "other");
		assert_eq!(workers[0]["state"], serde_json::Value::Null);
		assert_eq!(workers[1]["port"], 5426);
		assert_eq!(workers[1]["address"], "packages.p2.goodserver.ch:5426");
		assert_eq!(workers[1]["state"], "failed");
	}
}
