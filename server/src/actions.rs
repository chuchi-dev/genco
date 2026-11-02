//! Deleting a domain, a database or a group doesn't happen right away: it
//! becomes a pending action and every admin gets a mail with a link. It
//! runs when an admin opens the link while logged in and confirms. What
//! it deletes goes to the trash first (dump, archive), kept 30 days.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::AppState;
use crate::apply;
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::hosting;
use crate::mail;
use crate::render;
use crate::tokens;
use crate::users::Authed;
use crate::util;

pub const TRASH: &str = "/var/backups/genco/trash";
const TRASH_DAYS: u64 = 30;
const VALID_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
	Domain,
	Database,
	Group,
	MailDomain,
	Mailbox,
}

impl Kind {
	fn as_str(self) -> &'static str {
		match self {
			Self::Domain => "delete_domain",
			Self::Database => "delete_database",
			Self::Group => "delete_group",
			Self::MailDomain => "delete_mail_domain",
			Self::Mailbox => "delete_mailbox",
		}
	}

	fn parse(s: &str) -> Option<Self> {
		match s {
			"delete_domain" => Some(Self::Domain),
			"delete_database" => Some(Self::Database),
			"delete_group" => Some(Self::Group),
			"delete_mail_domain" => Some(Self::MailDomain),
			"delete_mailbox" => Some(Self::Mailbox),
			_ => None,
		}
	}
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
	pub description: String,
	pub requested_by: String,
	/// pending, done, failed, expired
	pub status: String,
	pub created_at: i64,
	#[serde(skip)]
	id: i64,
	#[serde(skip)]
	kind: String,
	#[serde(skip)]
	target_id: i64,
}

/// What the action does, also checks that it may happen.
fn describe(conn: &Connection, kind: Kind, id: i64) -> Result<String> {
	let groups = hosting::groups(conn)?;
	match kind {
		Kind::Domain => {
			let (g, d) = hosting::find_domain(conn, |d| d.id == id)?;
			Ok(format!(
				"Delete the domain {} of the group {}. Its nginx site, \
				service and certificate go away, the files in {} stay.",
				d.name,
				g.name,
				render::group_dir(&g.name).join(&d.root).display()
			))
		}
		Kind::Database => {
			let (g, d) = hosting::find_database(conn, |d| d.id == id)?;
			Ok(format!(
				"Delete the database {} of the group {}. A dump goes to \
				the trash ({TRASH}, {TRASH_DAYS} days) first.",
				d.name, g.name
			))
		}
		Kind::Group => {
			let g =
				groups.iter().find(|g| g.id == id).ok_or(Error::NotFound)?;
			if !g.domains.is_empty()
				|| !g.databases.is_empty()
				|| !g.mail_domains.is_empty()
			{
				return Err(Error::Invalid(format!(
					"delete the domains, databases and mail domains of {} \
					first",
					g.name
				)));
			}
			Ok(format!(
				"Delete the group {}: its user {}, its files in {} (an \
				archive goes to the trash, {TRASH_DAYS} days), deploys and \
				tokens.",
				g.name, g.user, g.dir
			))
		}
		Kind::MailDomain => {
			let d = groups
				.iter()
				.flat_map(|g| &g.mail_domains)
				.find(|d| d.id == id)
				.ok_or(Error::NotFound)?;
			Ok(format!(
				"Delete the mail domain {} with its {} mailboxes and {} \
				forwards. The mail goes to the trash as an archive ({TRASH}, \
				{TRASH_DAYS} days).",
				d.name,
				d.mailboxes.len(),
				d.forwards.len()
			))
		}
		Kind::Mailbox => {
			let b = groups
				.iter()
				.flat_map(|g| &g.mail_domains)
				.flat_map(|d| &d.mailboxes)
				.find(|b| b.id == id)
				.ok_or(Error::NotFound)?;
			Ok(format!(
				"Delete the mailbox {} and its mail. An archive goes to the \
				trash ({TRASH}, {TRASH_DAYS} days).",
				b.address
			))
		}
	}
}

/// Creates the action and mails the admins the link to confirm it.
/// Returns what the requester is told.
pub async fn request(
	cfg: &Config,
	db: &Db,
	kind: Kind,
	target_id: i64,
	requested_by: &str,
) -> Result<String> {
	let domain = cfg.domain.as_deref().ok_or(Error::Invalid(
		"the panel has no domain for the link".into(),
	))?;
	let token = util::random_hex(24);

	let (id, description) = {
		let conn = db.conn();
		let description = describe(&conn, kind, target_id)?;
		conn.execute(
			"INSERT INTO pending_actions (kind, target_id, description,
			token_hash, requested_by, status, created_at)
			VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
			params![
				kind.as_str(),
				target_id,
				description,
				tokens::hash(&token),
				requested_by,
				now()
			],
		)?;
		(conn.last_insert_rowid(), description)
	};

	let body = format!(
		"{requested_by} asked genco on {domain} for this:\n\n{description}\n\n\
		To confirm it, open this link while logged in to the panel:\n\n\
		https://{domain}/confirm/{token}\n\n\
		The link works for 24 hours. If you didn't expect this, ignore it:\n\
		nothing happens without a confirmation.\n"
	);
	let first = description.split('.').next().unwrap_or(&description);
	let sent = mail::send(
		cfg,
		&mail::admins(db)?,
		&format!("Confirm: {first}"),
		&body,
	)
	.await;
	if let Err(e) = sent {
		db.conn()
			.execute("DELETE FROM pending_actions WHERE id = ?1", [id])?;
		return Err(e);
	}

	Ok(format!(
		"Waiting for confirmation: every admin got a mail with a link to \
		confirm it ({description})"
	))
}

fn find(conn: &Connection, token: &str) -> Result<Action> {
	let action = conn
		.query_row(
			"SELECT id, kind, target_id, description, requested_by, status,
			created_at FROM pending_actions WHERE token_hash = ?1",
			[tokens::hash(token)],
			|row| {
				Ok(Action {
					id: row.get(0)?,
					kind: row.get(1)?,
					target_id: row.get(2)?,
					description: row.get(3)?,
					requested_by: row.get(4)?,
					status: row.get(5)?,
					created_at: row.get(6)?,
				})
			},
		)
		.optional()?
		.ok_or(Error::NotFound)?;

	let expired =
		action.status == "pending" && action.created_at + VALID_MS < now();
	Ok(Action {
		status: if expired {
			"expired".into()
		} else {
			action.status.clone()
		},
		..action
	})
}

#[derive(Debug, Serialize)]
pub struct Outcome {
	pub ok: bool,
	pub log: Vec<String>,
}

/// Runs a confirmed action.
async fn execute(
	cfg: &Config,
	db: &Db,
	action: &Action,
	confirmed_by: &str,
) -> Result<Outcome> {
	let kind = Kind::parse(&action.kind)
		.ok_or(Error::Internal(format!("unknown action {}", action.kind)))?;
	// the target may have changed since the request
	describe(&db.conn(), kind, action.target_id)?;
	// only one confirmation runs it, two at the same moment would both pass
	// the check in `confirm`
	let claimed = db.conn().execute(
		"UPDATE pending_actions SET status = 'running'
		WHERE id = ?1 AND status = 'pending'",
		[action.id],
	)?;
	if claimed != 1 {
		return Err(Error::Invalid("this action runs already".into()));
	}

	let mut log = vec![format!("confirmed by {confirmed_by}")];
	let res = match kind {
		Kind::Domain => {
			delete_domain(cfg, db, action.target_id, &mut log).await
		}
		Kind::Database => delete_database(db, action.target_id, &mut log).await,
		Kind::Group => delete_group(cfg, db, action.target_id, &mut log).await,
		Kind::MailDomain => {
			delete_mail(cfg, db, Kind::MailDomain, action.target_id, &mut log)
				.await
		}
		Kind::Mailbox => {
			delete_mail(cfg, db, Kind::Mailbox, action.target_id, &mut log)
				.await
		}
	};
	let ok = match res {
		Ok(()) => true,
		Err(e) => {
			log.push(format!("error: {e}"));
			false
		}
	};

	db.conn().execute(
		"UPDATE pending_actions SET status = ?1, done_at = ?2 WHERE id = ?3",
		params![if ok { "done" } else { "failed" }, now(), action.id],
	)?;
	Ok(Outcome { ok, log })
}

async fn run(
	log: &mut Vec<String>,
	args: &[&str],
) -> std::result::Result<(), String> {
	util::run(args).await?;
	log.push(format!("$ {}", args.join(" ")));
	Ok(())
}

/// Brings the server in line after a deletion. Its errors (a certificate
/// of another domain) are warnings: the deletion has to finish, its row
/// is gone already.
async fn reapply(
	cfg: &Config,
	db: &Db,
	log: &mut Vec<String>,
) -> std::result::Result<(), String> {
	let report = apply::apply(cfg, db).await;
	log.extend(report.log);
	if !report.ok {
		log.push("warning: apply had errors, see above; run it again once they are fixed".into());
	}
	Ok(())
}

async fn delete_domain(
	cfg: &Config,
	db: &Db,
	id: i64,
	log: &mut Vec<String>,
) -> std::result::Result<(), String> {
	let (group, domain) = hosting::find_domain(&db.conn(), |d| d.id == id)
		.map_err(|_| "the domain is gone")?;
	let name = domain.name.clone();
	db.conn()
		.execute("DELETE FROM domains WHERE id = ?1", [id])
		.map_err(|e| e.to_string())?;
	log.push(format!("removed the domain {name}"));
	reapply(cfg, db, log).await?;

	// the other domains' roots and public folders
	let others: Vec<&str> = group
		.domains
		.iter()
		.filter(|d| d.id != id)
		.flat_map(|d| {
			std::iter::once(d.root.as_str()).chain(d.public_dir.as_deref())
		})
		.collect();
	let root = normalize(&domain.root);
	let alone = own_folder(&root, &others);
	let exists = crate::render::group_dir(&group.name).join(&root).exists();
	if alone && exists {
		match crate::files::to_trash(&group, &root).await {
			Ok(()) => log.push(format!(
				"moved its folder {root} to the group's trash (.trash, kept 30 days)"
			)),
			Err(e) => log.push(format!("its folder {root} stays: {e}")),
		}
	}
	if crate::host::has_cert(&name) {
		let _ =
			run(log, &["certbot", "delete", "-n", "--cert-name", &name]).await;
	}
	Ok(())
}

/// Whether a deleted domain's root is its own: not a deploy's
/// (`current/...`) and no other domain's root in or around it.
fn own_folder(root: &str, others: &[&str]) -> bool {
	let root = normalize(root);
	let inside = |a: &str, b: &str| {
		b.is_empty() || a == b || a.starts_with(&format!("{b}/"))
	};
	!root.is_empty()
		&& root.split('/').next() != Some("current")
		&& !others.iter().any(|o| {
			let o = normalize(o);
			inside(&root, &o) || inside(&o, &root)
		})
}

/// `./public//` is `public`: roots compared as written would miss that.
fn normalize(path: &str) -> String {
	path.split('/')
		.filter(|c| !c.is_empty() && *c != ".")
		.collect::<Vec<_>>()
		.join("/")
}

async fn delete_database(
	db: &Db,
	id: i64,
	log: &mut Vec<String>,
) -> std::result::Result<(), String> {
	let (name, engine): (String, String) = db
		.conn()
		.query_row(
			"SELECT name, engine FROM databases WHERE id = ?1",
			[id],
			|r| Ok((r.get(0)?, r.get(1)?)),
		)
		.map_err(|e| e.to_string())?;
	let database = crate::hosting::Database {
		id,
		group_id: 0,
		name: name.clone(),
		engine: crate::hosting::Engine::parse(&engine),
	};

	trash_dir()?;
	let dump = format!("{TRASH}/db-{name}-{}.sql.gz", util::stamp().await);
	// the names are checked to be [a-z0-9_]
	run(
		log,
		&[
			"bash",
			"-c",
			&format!(
				"set -o pipefail; {} | gzip > {dump}",
				database.dump_command()
			),
		],
	)
	.await?;
	log.push(format!("dumped to {dump}"));

	let drop = match database.engine {
		crate::hosting::Engine::Mariadb => vec![
			"mariadb".to_string(),
			"-e".into(),
			format!("DROP DATABASE IF EXISTS `{name}`"),
		],
		crate::hosting::Engine::Postgres => crate::pg::admin_sql(&format!(
			"DROP DATABASE IF EXISTS {} WITH (FORCE)",
			crate::pg::ident(&name)
		)),
	};
	let drop: Vec<&str> = drop.iter().map(String::as_str).collect();
	run(log, &drop).await?;
	db.conn()
		.execute("DELETE FROM databases WHERE id = ?1", [id])
		.map_err(|e| e.to_string())?;
	log.push(format!("deleted the database {name}"));
	Ok(())
}

/// A mail domain or a mailbox: its mail goes to the trash as an archive.
async fn delete_mail(
	cfg: &Config,
	db: &Db,
	kind: Kind,
	id: i64,
	log: &mut Vec<String>,
) -> std::result::Result<(), String> {
	let (table, path, name): (&str, String, String) = {
		let conn = db.conn();
		if kind == Kind::MailDomain {
			let name: String = conn
				.query_row(
					"SELECT name FROM mail_domains WHERE id = ?1",
					[id],
					|r| r.get(0),
				)
				.map_err(|e| e.to_string())?;
			("mail_domains", name.clone(), name)
		} else {
			let (local, domain): (String, String) = conn
				.query_row(
					"SELECT b.local_part, d.name FROM mailboxes b
					JOIN mail_domains d ON d.id = b.mail_domain_id
					WHERE b.id = ?1",
					[id],
					|r| Ok((r.get(0)?, r.get(1)?)),
				)
				.map_err(|e| e.to_string())?;
			(
				"mailboxes",
				format!("{domain}/{local}"),
				format!("{local}@{domain}"),
			)
		}
	};

	trash_dir()?;
	let dir = std::path::Path::new(crate::mailbox::VMAIL).join(&path);
	if dir.exists() {
		let archive = format!(
			"{TRASH}/mail-{}-{}.tar.gz",
			name.replace('@', "-at-"),
			util::stamp().await
		);
		run(
			log,
			&["tar", "czf", &archive, "-C", crate::mailbox::VMAIL, &path],
		)
		.await?;
		log.push(format!("archived the mail to {archive}"));
	}

	db.conn()
		.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [id])
		.map_err(|e| e.to_string())?;
	log.push(format!("removed {name}"));
	// its key would sign for the domain if someone added it again
	if kind == Kind::MailDomain {
		let _ = std::fs::remove_file(crate::mailfilter::key_path(&name));
	}
	reapply(cfg, db, log).await?;
	if dir.exists() {
		std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
	}
	Ok(())
}

async fn delete_group(
	cfg: &Config,
	db: &Db,
	id: i64,
	log: &mut Vec<String>,
) -> std::result::Result<(), String> {
	let group =
		hosting::find_group(&db.conn(), id).map_err(|e| e.to_string())?;
	let dir = render::group_dir(&group.name);
	let user = render::group_user(&group.name);

	trash_dir()?;
	if dir.exists() {
		let archive = format!(
			"{TRASH}/group-{}-{}.tar.gz",
			group.name,
			util::stamp().await
		);
		run(
			log,
			&["tar", "czf", &archive, "-C", "/srv/genco", &group.name],
		)
		.await?;
		log.push(format!("archived the files to {archive}"));
	}

	{
		let conn = db.conn();
		// every token that may deploy it, also those with other scopes
		let scope =
			serde_json::to_string(&crate::tokens::deploy_scope(id)).unwrap();
		conn.execute(
			"DELETE FROM api_tokens WHERE instr(scopes, ?1) > 0",
			[scope],
		)
		.map_err(|e| e.to_string())?;
		conn.execute("DELETE FROM domain_groups WHERE id = ?1", [id])
			.map_err(|e| e.to_string())?;
	}
	log.push(format!("removed the group {}", group.name));

	// stops and removes its units
	reapply(cfg, db, log).await?;
	if dir.exists() {
		std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
	}
	let _ = run(
		log,
		&[
			"mariadb",
			"-e",
			&format!("DROP USER IF EXISTS `{user}`@localhost"),
		],
	)
	.await;
	// its databases are gone already; a role that still owns something
	// stays, and says so: a new group of the same name would get it
	if crate::pg::installed() {
		let drop =
			crate::pg::admin_sql(&format!("DROP ROLE IF EXISTS \"{user}\""));
		let drop: Vec<&str> = drop.iter().map(String::as_str).collect();
		if let Err(e) = run(log, &drop).await {
			log.push(format!("the PostgreSQL role {user} stays: {e}"));
		}
	}
	run(log, &["userdel", &user]).await?;
	log.push(format!("deleted the user {user}"));
	Ok(())
}

/// Only root may read it, it holds the data of every group.
fn trash_dir() -> std::result::Result<(), String> {
	util::private_dir(std::path::Path::new("/var/backups/genco"))
		.and_then(|()| std::fs::create_dir_all(TRASH))
		.map_err(|e| e.to_string())
}

/// Removes what has been in the trash for more than 30 days.
pub fn clean_trash() -> Vec<String> {
	let mut removed = vec![];
	let Ok(entries) = std::fs::read_dir(TRASH) else {
		return removed;
	};
	let max = std::time::Duration::from_secs(TRASH_DAYS * 24 * 60 * 60);
	for e in entries.flatten() {
		let old = e
			.metadata()
			.and_then(|m| m.modified())
			.ok()
			.and_then(|t| t.elapsed().ok())
			.is_some_and(|age| age > max);
		if old && std::fs::remove_file(e.path()).is_ok() {
			removed.push(e.path().display().to_string());
		}
	}
	removed
}

async fn request_delete(
	authed: &Authed,
	state: &AppState,
	kind: Kind,
	id: i64,
) -> Result<Json<String>> {
	let msg =
		request(&state.cfg, &state.db, kind, id, &authed.user.name).await?;
	Ok(Json(msg))
}

async fn delete_domain_route(
	authed: Authed,
	State(state): State<AppState>,
	Path(id): Path<i64>,
) -> Result<Json<String>> {
	request_delete(&authed, &state, Kind::Domain, id).await
}

async fn delete_database_route(
	authed: Authed,
	State(state): State<AppState>,
	Path(id): Path<i64>,
) -> Result<Json<String>> {
	request_delete(&authed, &state, Kind::Database, id).await
}

async fn delete_group_route(
	authed: Authed,
	State(state): State<AppState>,
	Path(id): Path<i64>,
) -> Result<Json<String>> {
	request_delete(&authed, &state, Kind::Group, id).await
}

async fn delete_mail_domain_route(
	authed: Authed,
	State(state): State<AppState>,
	Path(id): Path<i64>,
) -> Result<Json<String>> {
	request_delete(&authed, &state, Kind::MailDomain, id).await
}

async fn delete_mailbox_route(
	authed: Authed,
	State(state): State<AppState>,
	Path(id): Path<i64>,
) -> Result<Json<String>> {
	request_delete(&authed, &state, Kind::Mailbox, id).await
}

async fn get_action(
	_authed: Authed,
	State(db): State<Db>,
	Path(token): Path<String>,
) -> Result<Json<Action>> {
	Ok(Json(find(&db.conn(), &token)?))
}

async fn confirm(
	authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(token): Path<String>,
) -> Result<Json<Outcome>> {
	let action = find(&db.conn(), &token)?;
	if action.status != "pending" {
		return Err(Error::Invalid(format!(
			"this action is {} already",
			action.status
		)));
	}
	let outcome = execute(&cfg, &db, &action, &authed.user.name).await?;
	events.send(Event::Tree);
	Ok(Json(outcome))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/domains/{id}", delete(delete_domain_route))
		.route("/databases/{id}", delete(delete_database_route))
		.route("/groups/{id}", delete(delete_group_route))
		.route("/mail-domains/{id}", delete(delete_mail_domain_route))
		.route("/mailboxes/{id}", delete(delete_mailbox_route))
		.route("/actions/{token}", get(get_action))
		.route("/actions/{token}/confirm", post(confirm))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_deleted_domain_takes_only_its_own_folder() {
		assert!(own_folder("shop", &["blog"]));
		assert!(!own_folder("current/craft/web", &[]));
		assert!(!own_folder("www", &["www"]));
		assert!(!own_folder("www", &["www/shop"]));
		assert!(!own_folder("www/shop", &["www"]));
		assert!(own_folder("www2", &["www"]));
		assert!(!own_folder("", &[]));
		assert!(!own_folder("./public", &["public"]));
		assert!(!own_folder("public/", &["./public//x"]));
		assert!(!own_folder("./current/web", &[]));
		// another domain's root "." is the whole group directory
		assert!(!own_folder("shop", &["."]));
	}

	#[test]
	fn a_group_with_domains_cant_be_deleted() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute_batch(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0);
			INSERT INTO domain_groups (name, client_id, created_at)
				VALUES ('shop', 1, 0);
			INSERT INTO domains (group_id, name, kind, created_at)
				VALUES (1, 'shop.ch', 'static', 0);",
		)
		.unwrap();

		assert!(describe(&conn, Kind::Group, 1).is_err());
		assert!(
			describe(&conn, Kind::Domain, 1)
				.unwrap()
				.contains("shop.ch")
		);
		assert!(matches!(
			describe(&conn, Kind::Database, 7),
			Err(Error::NotFound)
		));
	}

	#[test]
	fn old_actions_expire() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute(
			"INSERT INTO pending_actions (kind, target_id, description,
			token_hash, requested_by, status, created_at)
			VALUES ('delete_domain', 1, 'x', ?1, 'a', 'pending', 0)",
			[tokens::hash("tok")],
		)
		.unwrap();
		assert_eq!(find(&conn, "tok").unwrap().status, "expired");
		assert!(matches!(find(&conn, "other"), Err(Error::NotFound)));
	}
}
