//! A group's workers and schedules. A worker runs all the time (Craft's
//! queue, a server on a public port of its own), a schedule runs a command
//! on a timer (`craft update-statuses`); both as the group's user in its
//! sandbox, as systemd units `render` writes. They come from the panel or
//! the CLI, or from `genco.toml` with every deploy (`source` "repo"): those
//! change only with the repository.

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::users::Authed;

/// The panel's or the CLI's, or genco.toml's (`repo`).
pub const PANEL: &str = "panel";
pub const REPO: &str = "repo";

/// Ports a worker may not take: genco's panel, the databases, Redis,
/// Rspamd, postsrsd and the apps' (20000 + domain id, 30000 + id for the
/// second slot of one that restarts without downtime).
const TAKEN: &[u16] = &[
	3306, 4100, 4190, 5432, 6379, 10001, 10002, 11332, 11333, 11334,
];
const APP_PORTS: std::ops::RangeInclusive<u16> = 20000..=39999;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worker {
	#[serde(default)]
	pub id: i64,
	#[serde(default)]
	pub group_id: i64,
	pub name: String,
	/// relative to the group's directory, like a domain's root
	pub dir: String,
	pub command: String,
	/// open to the internet, the worker gets it as PORT
	pub port: Option<u16>,
	/// a note where clients reach it, `packages.example.com:5426`; the
	/// panel's also for the repository's workers, kept over deploys
	#[serde(default)]
	pub address: String,
	#[serde(default)]
	pub source: String,
	/// its unit's ActiveState (active, failed, inactive, …), only in the
	/// tree the panel loads
	#[serde(default, skip_deserializing)]
	pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
	#[serde(default)]
	pub id: i64,
	#[serde(default)]
	pub group_id: i64,
	pub name: String,
	pub dir: String,
	pub command: String,
	/// when, as systemd's OnCalendar: `hourly`, `*:0/5`, `Mon *-*-* 03:00`
	pub every: String,
	/// minutes it may run
	pub timeout: u32,
	#[serde(default)]
	pub source: String,
}

pub fn load(
	conn: &Connection,
) -> rusqlite::Result<(Vec<Worker>, Vec<Schedule>)> {
	let mut stmt = conn.prepare(
		"SELECT id, group_id, name, dir, command, port, address, source
		FROM workers ORDER BY name",
	)?;
	let workers = stmt
		.query_map([], |r| {
			Ok(Worker {
				id: r.get(0)?,
				group_id: r.get(1)?,
				name: r.get(2)?,
				dir: r.get(3)?,
				command: r.get(4)?,
				port: r.get(5)?,
				address: r.get(6)?,
				source: r.get(7)?,
				state: None,
			})
		})?
		.collect::<rusqlite::Result<_>>()?;
	let mut stmt = conn.prepare(
		"SELECT id, group_id, name, dir, command, every, timeout, source
		FROM schedules ORDER BY name",
	)?;
	let schedules = stmt
		.query_map([], |r| {
			Ok(Schedule {
				id: r.get(0)?,
				group_id: r.get(1)?,
				name: r.get(2)?,
				dir: r.get(3)?,
				command: r.get(4)?,
				every: r.get(5)?,
				timeout: r.get(6)?,
				source: r.get(7)?,
			})
		})?
		.collect::<rusqlite::Result<_>>()?;
	Ok((workers, schedules))
}

/// Names become unit names: genco-<group>-worker-<name>.service.
fn check_name(name: &str) -> Result<()> {
	let ok = !name.is_empty()
		&& name.len() <= 32
		&& name.starts_with(|c: char| c.is_ascii_lowercase())
		&& name
			.chars()
			.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
	if !ok {
		return Err(Error::Invalid(format!(
			"{name:?}: names have up to 32 characters a-z, 0-9 and -, \
			starting with a letter"
		)));
	}
	Ok(())
}

/// One line, it goes into a script the unit runs with `sh`.
fn check_command(command: &str) -> Result<()> {
	if command.trim().is_empty()
		|| command.len() > 4096
		|| command.contains(['\n', '\r', '\0'])
	{
		return Err(Error::Invalid(
			"the command is one line of up to 4096 characters".into(),
		));
	}
	Ok(())
}

fn check_port(port: u16, ssh: &[u16]) -> Result<()> {
	if port < 1024
		|| TAKEN.contains(&port)
		|| APP_PORTS.contains(&port)
		|| ssh.contains(&port)
	{
		return Err(Error::Invalid(format!(
			"port {port} isn't free for a worker: 1024 to 65535, not 20000 \
			to 39999 (the apps') and not genco's own or ssh's"
		)));
	}
	Ok(())
}

/// sshd's `Port` lines, 22 without one. A worker's port is open to
/// everyone and closed to blocked addresses, ssh must stay open to them.
fn ssh_ports() -> Vec<u16> {
	let mut files = vec![std::path::PathBuf::from("/etc/ssh/sshd_config")];
	if let Ok(dir) = std::fs::read_dir("/etc/ssh/sshd_config.d") {
		files.extend(dir.flatten().map(|e| e.path()));
	}
	let ports: Vec<u16> = files
		.iter()
		.flat_map(|f| {
			let text = std::fs::read_to_string(f).unwrap_or_default();
			sshd_ports(&text)
		})
		.collect();
	if ports.is_empty() { vec![22] } else { ports }
}

fn sshd_ports(config: &str) -> Vec<u16> {
	config
		.lines()
		.filter_map(|l| {
			let mut words = l.split_whitespace();
			let key = words.next()?;
			key.eq_ignore_ascii_case("port")
				.then(|| words.next()?.parse().ok())?
		})
		.collect()
}

/// OnCalendar, checked for its characters here (it goes into the timer
/// file); `check_every` asks systemd whether it means a time.
fn check_every_chars(every: &str) -> Result<()> {
	let ok = !every.trim().is_empty()
		&& every.len() <= 100
		&& every
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || " *:/,.~-".contains(c));
	if !ok {
		return Err(Error::Invalid(format!(
			"{every:?} is no time like hourly, *:0/5 or Mon *-*-* 03:00"
		)));
	}
	Ok(())
}

/// Whether systemd understands `every`, before a timer gets it.
pub async fn check_every(every: &str) -> Result<()> {
	check_every_chars(every)?;
	let out = tokio::process::Command::new("systemd-analyze")
		.args(["calendar", every])
		.output()
		.await
		.map_err(|e| Error::Internal(format!("systemd-analyze: {e}")))?;
	if !out.status.success() {
		return Err(Error::Invalid(format!(
			"{every:?} is no time systemd knows (hourly, *:0/5, Mon *-*-* 03:00)"
		)));
	}
	Ok(())
}

/// A short line of text, it shows in the panel only.
fn check_address(address: &str) -> Result<()> {
	if address.len() > 200 || address.chars().any(|c| c.is_control()) {
		return Err(Error::Invalid(
			"the address is one line of up to 200 characters".into(),
		));
	}
	Ok(())
}

impl Worker {
	pub fn check(&self) -> Result<()> {
		check_name(&self.name)?;
		crate::hosting::check_root(&self.dir)?;
		check_command(&self.command)?;
		if let Some(p) = self.port {
			check_port(p, &ssh_ports())?;
		}
		check_address(&self.address)
	}
}

impl Schedule {
	pub fn check(&self) -> Result<()> {
		check_name(&self.name)?;
		crate::hosting::check_root(&self.dir)?;
		check_command(&self.command)?;
		check_every_chars(&self.every)?;
		if !(1..=1440).contains(&self.timeout) {
			return Err(Error::Invalid(
				"a timeout is 1 to 1440 minutes".into(),
			));
		}
		Ok(())
	}
}

/// A port only one worker of the whole server may have.
fn port_free(conn: &Connection, port: u16, except: i64) -> Result<()> {
	let n: i64 = conn.query_row(
		"SELECT count(*) FROM workers WHERE port = ?1 AND id != ?2",
		params![port, except],
		|r| r.get(0),
	)?;
	if n > 0 {
		return Err(Error::Invalid(format!("another worker has port {port}")));
	}
	Ok(())
}

/// Adds or (with an id) changes a worker of the panel or the CLI.
pub fn save_worker(conn: &Connection, w: &Worker) -> Result<i64> {
	w.check()?;
	if let Some(p) = w.port {
		port_free(conn, p, w.id)?;
	}
	if w.id == 0 {
		conn.execute(
			"INSERT INTO workers (group_id, name, dir, command, port, address,
			source, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
			params![
				w.group_id,
				w.name,
				w.dir,
				w.command,
				w.port,
				w.address.trim(),
				PANEL,
				now()
			],
		)
		.map_err(|e| taken(e, &w.name))?;
		return Ok(conn.last_insert_rowid());
	}
	panel_only(conn, "workers", w.id)?;
	conn.execute(
		"UPDATE workers SET name = ?1, dir = ?2, command = ?3, port = ?4,
		address = ?5 WHERE id = ?6",
		params![w.name, w.dir, w.command, w.port, w.address.trim(), w.id],
	)
	.map_err(|e| taken(e, &w.name))?;
	Ok(w.id)
}

/// Only the address, of any worker: where clients reach one of the
/// repository is known on the server, not in genco.toml.
pub fn set_address(conn: &Connection, id: i64, address: &str) -> Result<()> {
	check_address(address)?;
	let n = conn.execute(
		"UPDATE workers SET address = ?1 WHERE id = ?2",
		params![address.trim(), id],
	)?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	Ok(())
}

pub fn save_schedule(conn: &Connection, s: &Schedule) -> Result<i64> {
	s.check()?;
	if s.id == 0 {
		conn.execute(
			"INSERT INTO schedules (group_id, name, dir, command, every,
			timeout, source, created_at)
			VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
			params![
				s.group_id,
				s.name,
				s.dir,
				s.command,
				s.every,
				s.timeout,
				PANEL,
				now()
			],
		)
		.map_err(|e| taken(e, &s.name))?;
		return Ok(conn.last_insert_rowid());
	}
	panel_only(conn, "schedules", s.id)?;
	conn.execute(
		"UPDATE schedules SET name = ?1, dir = ?2, command = ?3, every = ?4,
		timeout = ?5 WHERE id = ?6",
		params![s.name, s.dir, s.command, s.every, s.timeout, s.id],
	)
	.map_err(|e| taken(e, &s.name))?;
	Ok(s.id)
}

fn taken(e: rusqlite::Error, name: &str) -> Error {
	if e.to_string().contains("UNIQUE") {
		Error::Invalid(format!("the group has a {name} already"))
	} else {
		e.into()
	}
}

/// genco.toml's change with the repository only.
fn panel_only(conn: &Connection, table: &str, id: i64) -> Result<()> {
	use rusqlite::OptionalExtension as _;
	let source: Option<String> = conn
		.query_row(
			&format!("SELECT source FROM {table} WHERE id = ?1"),
			[id],
			|r| r.get(0),
		)
		.optional()?;
	match source.as_deref() {
		None => Err(Error::NotFound),
		Some(REPO) => Err(Error::Invalid(
			"it comes with the deploys (genco.toml or detected), change it in the repository".into(),
		)),
		Some(_) => Ok(()),
	}
}

pub fn delete(conn: &Connection, table: &str, id: i64) -> Result<()> {
	panel_only(conn, table, id)?;
	conn.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [id])?;
	Ok(())
}

/// After a deploy: the group's workers and schedules from genco.toml
/// replace the earlier ones from there; the panel's stay, one of the
/// same name wins over the repository's. Returns whether anything changed.
pub fn sync_repo(
	conn: &Connection,
	group_id: i64,
	workers: &[Worker],
	schedules: &[Schedule],
) -> Result<bool> {
	for w in workers {
		w.check()?;
	}
	for s in schedules {
		s.check()?;
	}
	let (old_w, old_s) = load(conn)?;
	let mine = |source: &str, gid: i64| source == REPO && gid == group_id;
	let panel_w: Vec<&str> = old_w
		.iter()
		.filter(|w| w.group_id == group_id && w.source == PANEL)
		.map(|w| w.name.as_str())
		.collect();
	let panel_s: Vec<&str> = old_s
		.iter()
		.filter(|s| s.group_id == group_id && s.source == PANEL)
		.map(|s| s.name.as_str())
		.collect();
	// the panel's address of one stays
	let address = |name: &str| {
		old_w
			.iter()
			.find(|w| mine(&w.source, w.group_id) && w.name == name)
			.map(|w| w.address.clone())
			.unwrap_or_default()
	};
	let new_w: Vec<Worker> = workers
		.iter()
		.filter(|w| !panel_w.contains(&w.name.as_str()))
		.map(|w| Worker {
			id: 0,
			group_id,
			address: address(&w.name),
			source: REPO.into(),
			..w.clone()
		})
		.collect();
	let new_s: Vec<Schedule> = schedules
		.iter()
		.filter(|s| !panel_s.contains(&s.name.as_str()))
		.map(|s| Schedule {
			id: 0,
			group_id,
			source: REPO.into(),
			..s.clone()
		})
		.collect();
	let same_w = {
		let mut was: Vec<Worker> = old_w
			.into_iter()
			.filter(|w| mine(&w.source, w.group_id))
			.map(|w| Worker { id: 0, ..w })
			.collect();
		was.sort_by(|a, b| a.name.cmp(&b.name));
		let mut now = new_w.clone();
		now.sort_by(|a, b| a.name.cmp(&b.name));
		was == now
	};
	let same_s = {
		let mut was: Vec<Schedule> = old_s
			.into_iter()
			.filter(|s| mine(&s.source, s.group_id))
			.map(|s| Schedule { id: 0, ..s })
			.collect();
		was.sort_by(|a, b| a.name.cmp(&b.name));
		let mut now = new_s.clone();
		now.sort_by(|a, b| a.name.cmp(&b.name));
		was == now
	};
	if same_w && same_s {
		return Ok(false);
	}

	conn.execute_batch("SAVEPOINT sync_repo")?;
	let res =
		(|| -> Result<()> {
			conn.execute(
				"DELETE FROM workers WHERE group_id = ?1 AND source = ?2",
				params![group_id, REPO],
			)?;
			conn.execute(
				"DELETE FROM schedules WHERE group_id = ?1 AND source = ?2",
				params![group_id, REPO],
			)?;
			for w in &new_w {
				if let Some(p) = w.port {
					port_free(conn, p, 0)?;
				}
				conn.execute(
					"INSERT INTO workers (group_id, name, dir, command, port,
				address, source, created_at)
				VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
					params![
						group_id,
						w.name,
						w.dir,
						w.command,
						w.port,
						w.address,
						REPO,
						now()
					],
				)?;
			}
			for s in &new_s {
				conn.execute(
				"INSERT INTO schedules (group_id, name, dir, command, every,
				timeout, source, created_at)
				VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
				params![
					group_id, s.name, s.dir, s.command, s.every, s.timeout, REPO,
					now()
				],
			)?;
			}
			Ok(())
		})();
	match res {
		Ok(()) => {
			conn.execute_batch("RELEASE sync_repo")?;
			Ok(true)
		}
		Err(e) => {
			conn.execute_batch("ROLLBACK TO sync_repo; RELEASE sync_repo")?;
			Err(e)
		}
	}
}

async fn create_worker(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(group_id): Path<i64>,
	Json(w): Json<Worker>,
) -> Result<Json<i64>> {
	let w = Worker {
		id: 0,
		group_id,
		..w
	};
	let id = save_worker(&db.conn(), &w)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

async fn update_worker(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(w): Json<Worker>,
) -> Result<Json<i64>> {
	let w = Worker { id, ..w };
	save_worker(&db.conn(), &w)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

#[derive(Deserialize)]
struct AddressReq {
	address: String,
}

async fn update_address(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(req): Json<AddressReq>,
) -> Result<Json<()>> {
	set_address(&db.conn(), id, &req.address)?;
	events.send(Event::Tree);
	Ok(Json(()))
}

async fn delete_worker(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	delete(&db.conn(), "workers", id)?;
	events.send(Event::Tree);
	Ok(Json(()))
}

async fn create_schedule(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(group_id): Path<i64>,
	Json(s): Json<Schedule>,
) -> Result<Json<i64>> {
	check_every(&s.every).await?;
	let s = Schedule {
		id: 0,
		group_id,
		..s
	};
	let id = save_schedule(&db.conn(), &s)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

async fn update_schedule(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(s): Json<Schedule>,
) -> Result<Json<i64>> {
	check_every(&s.every).await?;
	let s = Schedule { id, ..s };
	save_schedule(&db.conn(), &s)?;
	events.send(Event::Tree);
	Ok(Json(id))
}

async fn delete_schedule(
	_authed: Authed,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	delete(&db.conn(), "schedules", id)?;
	events.send(Event::Tree);
	Ok(Json(()))
}

/// Starts a schedule now, without waiting for its time.
async fn run_schedule(
	_authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<()>> {
	let unit = schedule_unit_by_id(&db.conn(), id)?;
	start_now(&unit).await?;
	Ok(Json(()))
}

fn schedule_unit_by_id(conn: &Connection, id: i64) -> Result<String> {
	let (group, name): (String, String) = conn
		.query_row(
			"SELECT g.name, s.name FROM schedules s
			JOIN domain_groups g ON g.id = s.group_id WHERE s.id = ?1",
			[id],
			|r| Ok((r.get(0)?, r.get(1)?)),
		)
		.map_err(|_| Error::NotFound)?;
	Ok(crate::render::schedule_unit(&group, &name))
}

/// `systemctl start --no-block`: a long run doesn't hold the request.
pub async fn start_now(unit: &str) -> Result<()> {
	let ok = tokio::process::Command::new("systemctl")
		.args(["start", "--no-block", unit])
		.status()
		.await
		.is_ok_and(|s| s.success());
	if !ok {
		return Err(Error::Invalid(format!(
			"{unit} didn't start, run Server → Apply Changes first"
		)));
	}
	Ok(())
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/groups/{id}/workers", post(create_worker))
		.route("/workers/{id}", post(update_worker).delete(delete_worker))
		.route("/workers/{id}/address", post(update_address))
		.route("/groups/{id}/schedules", post(create_schedule))
		.route(
			"/schedules/{id}",
			post(update_schedule).delete(delete_schedule),
		)
		.route("/schedules/{id}/run", post(run_schedule))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn setup() -> Db {
		let db = Db::memory();
		db.conn()
			.execute_batch(
				"INSERT INTO clients (name, created_at) VALUES ('c', 0);
				INSERT INTO domain_groups (name, client_id, created_at)
					VALUES ('shop', 1, 0);",
			)
			.unwrap();
		db
	}

	fn worker(name: &str, port: Option<u16>) -> Worker {
		Worker {
			id: 0,
			group_id: 1,
			name: name.into(),
			dir: "current/craft".into(),
			command: "php craft queue/listen --verbose".into(),
			port,
			address: String::new(),
			source: String::new(),
			state: None,
		}
	}

	fn schedule(name: &str) -> Schedule {
		Schedule {
			id: 0,
			group_id: 1,
			name: name.into(),
			dir: "current/craft".into(),
			command: "php craft update-statuses".into(),
			every: "*:0/5".into(),
			timeout: 10,
			source: String::new(),
		}
	}

	#[test]
	fn names_ports_and_commands_are_checked() {
		assert!(check_name("queue").is_ok());
		assert!(check_name("queue-2").is_ok());
		assert!(check_name("Queue").is_err());
		assert!(check_name("a.b").is_err());
		assert!(check_name("2a").is_err());
		assert!(check_port(5426, &[22]).is_ok());
		assert!(check_port(80, &[22]).is_err());
		assert!(check_port(20005, &[22]).is_err());
		assert!(check_port(30005, &[22]).is_err());
		assert!(check_port(3306, &[22]).is_err());
		assert!(check_port(2222, &[22, 2222]).is_err());
		assert_eq!(
			sshd_ports("# Port 1\nport 2222\nPort  2223 \nPermitRootLogin no"),
			[2222, 2223]
		);
		assert!(check_command("a\nExecStartPre=x").is_err());
		assert!(check_every_chars("Mon *-*-* 03:00").is_ok());
		assert!(check_every_chars("hourly\n[Service]").is_err());
		let mut w = worker("q", None);
		w.dir = "../other".into();
		assert!(w.check().is_err());
	}

	#[test]
	fn a_port_belongs_to_one_worker() {
		let db = setup();
		let conn = db.conn();
		save_worker(&conn, &worker("packages", Some(5426))).unwrap();
		assert!(save_worker(&conn, &worker("other", Some(5426))).is_err());
		assert!(save_worker(&conn, &worker("packages", None)).is_err());
	}

	#[test]
	fn the_address_stays_over_deploys() {
		let db = setup();
		let conn = db.conn();
		sync_repo(&conn, 1, &[worker("packages", Some(5426))], &[]).unwrap();
		let (w, _) = load(&conn).unwrap();
		set_address(&conn, w[0].id, " packages.p2.goodserver.ch:5426 ")
			.unwrap();
		assert!(set_address(&conn, w[0].id, "a\nb").is_err());

		// the same genco.toml again: nothing changed
		let again = [worker("packages", Some(5426))];
		assert!(!sync_repo(&conn, 1, &again, &[]).unwrap());
		let other = [worker("packages", Some(5427))];
		assert!(sync_repo(&conn, 1, &other, &[]).unwrap());
		let (w, _) = load(&conn).unwrap();
		assert_eq!(w[0].address, "packages.p2.goodserver.ch:5426");
	}

	#[test]
	fn the_repository_replaces_only_its_own() {
		let db = setup();
		let conn = db.conn();
		save_worker(&conn, &worker("mine", None)).unwrap();
		let changed = sync_repo(
			&conn,
			1,
			&[worker("queue", None), worker("mine", Some(6000))],
			&[schedule("statuses")],
		)
		.unwrap();
		assert!(changed);
		let (w, s) = load(&conn).unwrap();
		let names: Vec<(&str, &str)> = w
			.iter()
			.map(|w| (w.name.as_str(), w.source.as_str()))
			.collect();
		// the panel's wins over the repository's of the same name
		assert_eq!(names, [("mine", PANEL), ("queue", REPO)]);
		assert_eq!(s[0].source, REPO);

		// the same again changes nothing, a removed one goes
		assert!(
			!sync_repo(
				&conn,
				1,
				&[worker("queue", None)],
				&[schedule("statuses")]
			)
			.unwrap()
		);
		assert!(sync_repo(&conn, 1, &[], &[]).unwrap());
		let (w, s) = load(&conn).unwrap();
		assert_eq!(w.len(), 1);
		assert!(s.is_empty());

		// the repository's can't be changed in the panel
		sync_repo(&conn, 1, &[worker("queue", None)], &[]).unwrap();
		let (w, _) = load(&conn).unwrap();
		let q = w.iter().find(|w| w.name == "queue").unwrap();
		assert!(save_worker(&conn, q).is_err());
		assert!(delete(&conn, "workers", q.id).is_err());
	}
}
