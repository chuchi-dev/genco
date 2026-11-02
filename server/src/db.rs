use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

/// Every entry is applied once, tracked with `PRAGMA user_version`.
/// Only ever append to this list.
const MIGRATIONS: &[&str] = &[
	r#"
	CREATE TABLE users (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		email TEXT NOT NULL UNIQUE,
		name TEXT NOT NULL,
		password_hash TEXT NOT NULL,
		created_at INTEGER NOT NULL
	);

	CREATE TABLE sessions (
		token TEXT PRIMARY KEY,
		id TEXT NOT NULL UNIQUE,
		user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
		user_agent TEXT,
		ip TEXT,
		created_at INTEGER NOT NULL,
		last_seen INTEGER NOT NULL
	);

	CREATE TABLE clients (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		name TEXT NOT NULL UNIQUE,
		contact TEXT NOT NULL DEFAULT '',
		notes TEXT NOT NULL DEFAULT '',
		created_at INTEGER NOT NULL
	);

	CREATE TABLE domain_groups (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		name TEXT NOT NULL UNIQUE,
		client_id INTEGER NOT NULL REFERENCES clients(id),
		notes TEXT NOT NULL DEFAULT '',
		created_at INTEGER NOT NULL
	);

	CREATE TABLE domains (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		group_id INTEGER NOT NULL REFERENCES domain_groups(id),
		name TEXT NOT NULL UNIQUE,
		kind TEXT NOT NULL,
		php_version TEXT,
		root TEXT NOT NULL DEFAULT 'public',
		created_at INTEGER NOT NULL
	);

	CREATE TABLE databases (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		group_id INTEGER NOT NULL REFERENCES domain_groups(id),
		name TEXT NOT NULL UNIQUE,
		created_at INTEGER NOT NULL
	);

	CREATE TABLE conversations (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		title TEXT NOT NULL,
		status TEXT NOT NULL,
		seen INTEGER NOT NULL DEFAULT 0,
		created_at INTEGER NOT NULL,
		updated_at INTEGER NOT NULL
	);

	CREATE TABLE messages (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		conversation_id INTEGER NOT NULL
			REFERENCES conversations(id) ON DELETE CASCADE,
		data TEXT NOT NULL,
		created_at INTEGER NOT NULL
	);

	CREATE INDEX messages_conversation_id ON messages(conversation_id);
"#,
	// node and rust apps: the command that starts them in their directory
	// (`root`); extra nginx lines for a domain's server block
	r#"
	ALTER TABLE domains ADD COLUMN command TEXT;
	ALTER TABLE domains ADD COLUMN nginx_extra TEXT NOT NULL DEFAULT '';
"#,
	// deploys from git: the group's repository, api tokens for CI and
	// later other clients, and every deploy with its log in a file
	r#"
	ALTER TABLE domain_groups ADD COLUMN repo TEXT;
	ALTER TABLE domain_groups ADD COLUMN branch TEXT NOT NULL DEFAULT 'main';

	CREATE TABLE api_tokens (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		name TEXT NOT NULL,
		token_hash TEXT NOT NULL UNIQUE,
		scopes TEXT NOT NULL,
		created_at INTEGER NOT NULL,
		last_used INTEGER
	);

	CREATE TABLE deploys (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		group_id INTEGER NOT NULL
			REFERENCES domain_groups(id) ON DELETE CASCADE,
		ref TEXT NOT NULL,
		commit_id TEXT,
		release TEXT,
		status TEXT NOT NULL,
		started_by TEXT NOT NULL,
		created_at INTEGER NOT NULL,
		finished_at INTEGER
	);

	CREATE INDEX deploys_group_id ON deploys(group_id);
"#,
	// backup runs, their log is a file like the deploys'
	r#"
	CREATE TABLE backups (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		status TEXT NOT NULL,
		started_by TEXT NOT NULL,
		created_at INTEGER NOT NULL,
		finished_at INTEGER
	);
"#,
	// settings of the panel (mail), 2FA of the admins, and deletions that
	// wait for an admin's confirmation by mail
	r#"
	CREATE TABLE settings (
		key TEXT PRIMARY KEY,
		value TEXT NOT NULL
	);

	ALTER TABLE users ADD COLUMN totp_secret TEXT;

	CREATE TABLE pending_actions (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		kind TEXT NOT NULL,
		target_id INTEGER NOT NULL,
		description TEXT NOT NULL,
		token_hash TEXT NOT NULL UNIQUE,
		requested_by TEXT NOT NULL,
		status TEXT NOT NULL,
		created_at INTEGER NOT NULL,
		done_at INTEGER
	);
"#,
	// a token belongs to the admin who created it and goes with them
	r#"
	ALTER TABLE api_tokens ADD COLUMN user_id INTEGER
		REFERENCES users(id) ON DELETE CASCADE;
"#,
	// sessions keep only the hash of their token from now on (everybody
	// logs in again); the last time step of a TOTP code, it can't be used
	// twice
	r#"
	DELETE FROM sessions;
	ALTER TABLE users ADD COLUMN totp_step INTEGER NOT NULL DEFAULT 0;
"#,
	// routines and alerts are conversations too, they end with a report
	// (ok or attention); the problems the minute check found, cleared ones
	// stay a while so a flapping problem doesn't alert every minute
	r#"
	ALTER TABLE conversations ADD COLUMN kind TEXT NOT NULL DEFAULT 'chat';
	ALTER TABLE conversations ADD COLUMN report TEXT;

	CREATE TABLE problems (
		key TEXT PRIMARY KEY,
		detail TEXT NOT NULL,
		since INTEGER NOT NULL,
		alerted_at INTEGER,
		cleared_at INTEGER
	);
"#,
	// who did what: admins, API tokens, the terminal, the agent
	r#"
	CREATE TABLE activity (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		at INTEGER NOT NULL,
		actor TEXT NOT NULL,
		action TEXT NOT NULL,
		target TEXT NOT NULL DEFAULT '',
		detail TEXT NOT NULL DEFAULT '',
		ok INTEGER
	);

	CREATE INDEX activity_at ON activity(at);

	-- an invited admin sets the password with the link of the mail
	ALTER TABLE users ADD COLUMN invite_hash TEXT;
	ALTER TABLE users ADD COLUMN invite_expires INTEGER;
"#,
	// a backup of a single group, NULL backs up everything
	r#"
	ALTER TABLE backups ADD COLUMN group_id INTEGER
		REFERENCES domain_groups(id) ON DELETE CASCADE;
"#,
	// files nginx serves itself in front of a Node or Rust app
	r#"
	ALTER TABLE domains ADD COLUMN public_dir TEXT;
"#,
	// mail hosting: mail domains of a group, their mailboxes (bcrypt for
	// Dovecot) and forwards (`*` is the catch-all, targets space separated)
	r#"
	CREATE TABLE mail_domains (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		group_id INTEGER NOT NULL REFERENCES domain_groups(id),
		name TEXT NOT NULL UNIQUE,
		created_at INTEGER NOT NULL
	);

	CREATE TABLE mailboxes (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		mail_domain_id INTEGER NOT NULL
			REFERENCES mail_domains(id) ON DELETE CASCADE,
		local_part TEXT NOT NULL,
		password_hash TEXT NOT NULL,
		quota_mb INTEGER NOT NULL,
		created_at INTEGER NOT NULL,
		UNIQUE (mail_domain_id, local_part)
	);

	CREATE TABLE mail_forwards (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		mail_domain_id INTEGER NOT NULL
			REFERENCES mail_domains(id) ON DELETE CASCADE,
		local_part TEXT NOT NULL,
		targets TEXT NOT NULL,
		created_at INTEGER NOT NULL,
		UNIQUE (mail_domain_id, local_part)
	);
"#,
	// passwords for the admins, the value encrypted with secrets.key
	r#"
	CREATE TABLE secrets (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		name TEXT NOT NULL UNIQUE,
		group_id INTEGER REFERENCES domain_groups(id) ON DELETE SET NULL,
		username TEXT NOT NULL DEFAULT '',
		url TEXT NOT NULL DEFAULT '',
		note TEXT NOT NULL DEFAULT '',
		value BLOB NOT NULL,
		created_by TEXT NOT NULL,
		created_at INTEGER NOT NULL,
		updated_at INTEGER NOT NULL
	);
"#,
	// every llm request of the agent with its tokens and price
	r#"
	CREATE TABLE llm_usage (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		at INTEGER NOT NULL,
		conversation_id INTEGER NOT NULL,
		model TEXT NOT NULL,
		input_tokens INTEGER NOT NULL,
		output_tokens INTEGER NOT NULL,
		cache_read_tokens INTEGER NOT NULL,
		cache_write_tokens INTEGER NOT NULL,
		cost_usd REAL
	);

	CREATE INDEX llm_usage_at ON llm_usage(at);
"#,
	// dumps imported into a group's database
	r#"
	CREATE TABLE db_imports (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		database_id INTEGER NOT NULL
			REFERENCES databases(id) ON DELETE CASCADE,
		status TEXT NOT NULL,
		started_by TEXT NOT NULL,
		created_at INTEGER NOT NULL,
		finished_at INTEGER
	);
"#,
	// PHP settings of a group (upload size, memory, time, input vars) as
	// JSON, defaults for what is missing
	r#"
	ALTER TABLE domain_groups ADD COLUMN php TEXT NOT NULL DEFAULT '{}';
"#,
	// build runners (`genco runner`) and the jobs they build for deploys
	r#"
	CREATE TABLE runners (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		name TEXT NOT NULL UNIQUE,
		token_hash TEXT NOT NULL UNIQUE,
		created_at INTEGER NOT NULL,
		last_seen INTEGER,
		info TEXT NOT NULL DEFAULT '{}'
	);

	CREATE TABLE build_jobs (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		deploy_id INTEGER NOT NULL,
		cache_key TEXT NOT NULL,
		image TEXT NOT NULL,
		run TEXT NOT NULL,
		artifacts TEXT NOT NULL,
		minutes INTEGER NOT NULL,
		-- packing, queued, running, done, failed, gone (built here)
		status TEXT NOT NULL,
		runner_id INTEGER,
		message TEXT,
		created_at INTEGER NOT NULL,
		started_at INTEGER,
		heartbeat_at INTEGER,
		finished_at INTEGER
	);
"#,
	// environment variables of an app domain, JSON (appenv.rs)
	r#"
	ALTER TABLE domains ADD COLUMN env TEXT NOT NULL DEFAULT '[]';
"#,
	// a database is MariaDB's or PostgreSQL's
	r#"
	ALTER TABLE databases ADD COLUMN engine TEXT NOT NULL DEFAULT 'mariadb';
"#,
	// the SMTP login of a group with mail domains: its sites send DKIM
	// signed mail with it (mailbox.rs)
	r#"
	ALTER TABLE domain_groups ADD COLUMN smtp_password TEXT;
	ALTER TABLE domain_groups ADD COLUMN smtp_hash TEXT;
"#,
	// a group's long running processes (a queue, a server on its own
	// port) and its commands on a timer, from the panel or genco.toml
	// (procs.rs); domains that only forward to another server
	r#"
	CREATE TABLE workers (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		group_id INTEGER NOT NULL
			REFERENCES domain_groups(id) ON DELETE CASCADE,
		name TEXT NOT NULL,
		dir TEXT NOT NULL,
		command TEXT NOT NULL,
		port INTEGER,
		source TEXT NOT NULL,
		created_at INTEGER NOT NULL,
		UNIQUE (group_id, name)
	);

	CREATE TABLE schedules (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		group_id INTEGER NOT NULL
			REFERENCES domain_groups(id) ON DELETE CASCADE,
		name TEXT NOT NULL,
		dir TEXT NOT NULL,
		command TEXT NOT NULL,
		every TEXT NOT NULL,
		timeout INTEGER NOT NULL,
		source TEXT NOT NULL,
		created_at INTEGER NOT NULL,
		UNIQUE (group_id, name)
	);

	ALTER TABLE domains ADD COLUMN upstream TEXT;
"#,
	r#"
	CREATE TABLE peers (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		name TEXT NOT NULL UNIQUE,
		key TEXT NOT NULL,
		key_hash TEXT NOT NULL UNIQUE,
		webhook_url TEXT NOT NULL DEFAULT '',
		webhook_all INTEGER NOT NULL DEFAULT 0,
		created_at INTEGER NOT NULL,
		last_used INTEGER
	);

	CREATE TABLE proposals (
		id INTEGER PRIMARY KEY AUTOINCREMENT,
		peer_id INTEGER NOT NULL REFERENCES peers(id) ON DELETE CASCADE,
		text TEXT NOT NULL,
		approver_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
		status TEXT NOT NULL,
		decided_by TEXT,
		conversation_id INTEGER
			REFERENCES conversations(id) ON DELETE SET NULL,
		answer TEXT,
		created_at INTEGER NOT NULL,
		decided_at INTEGER,
		answered_at INTEGER
	);
"#,
];

/// A single shared connection. The queries are tiny, so we accept briefly
/// blocking the async runtime instead of adding a pool.
#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
	pub fn open(path: &Path) -> Result<Self, String> {
		let mut conn = Connection::open(path).map_err(|e| {
			format!("failed to open database {}: {e}", path.display())
		})?;

		// `genco chat` may write while the server runs
		conn.execute_batch(
			"PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; \
			PRAGMA busy_timeout = 5000;",
		)
		.map_err(|e| e.to_string())?;

		migrate(&mut conn).map_err(|e| format!("migration failed: {e}"))?;

		Ok(Self(Arc::new(Mutex::new(conn))))
	}

	#[cfg(test)]
	pub fn memory() -> Self {
		let mut conn = Connection::open_in_memory().unwrap();
		conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
		migrate(&mut conn).unwrap();
		Self(Arc::new(Mutex::new(conn)))
	}

	pub fn conn(&self) -> MutexGuard<'_, Connection> {
		self.0.lock().unwrap()
	}
}

/// Reads the version inside the write transaction, `genco` commands and
/// the server may start at the same moment.
fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
	loop {
		let trans = conn.transaction_with_behavior(
			rusqlite::TransactionBehavior::Immediate,
		)?;
		let version: usize =
			trans.query_row("PRAGMA user_version", [], |row| row.get(0))?;
		let Some(sql) = MIGRATIONS.get(version) else {
			return Ok(());
		};
		trans.execute_batch(sql)?;
		trans.pragma_update(None, "user_version", version + 1)?;
		trans.commit()?;
	}
}

pub fn get_setting(
	conn: &Connection,
	key: &str,
) -> rusqlite::Result<Option<String>> {
	use rusqlite::OptionalExtension as _;
	conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
		r.get(0)
	})
	.optional()
}

/// A setting stored as JSON; `None` without one or when it doesn't parse
/// (an older shape), the caller takes its default then.
pub fn get_json<T: serde::de::DeserializeOwned>(
	conn: &Connection,
	key: &str,
) -> Option<T> {
	get_setting(conn, key)
		.ok()
		.flatten()
		.and_then(|raw| serde_json::from_str(&raw).ok())
}

pub fn set_setting(
	conn: &Connection,
	key: &str,
	value: &str,
) -> rusqlite::Result<()> {
	conn.execute(
		"INSERT INTO settings (key, value) VALUES (?1, ?2)
		ON CONFLICT (key) DO UPDATE SET value = excluded.value",
		[key, value],
	)?;
	Ok(())
}

/// unix time in milliseconds
pub fn now() -> i64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_millis() as i64
}
