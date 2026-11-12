//! The state of the server in one read, without the llm: for a dashboard
//! of all servers or another agent, with a `read` token or as an admin.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::Connection;
use serde::Serialize;

use crate::AppState;
use crate::config::Config;
use crate::db::Db;
use crate::error::Result;
use crate::hosting;
use crate::tokens::Reader;
use crate::util::sh;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
	server: Server,
	groups: Vec<GroupStatus>,
	/// the newest backup run and when everything was last backed up
	last_backup: Option<Run>,
	last_full_backup: Option<i64>,
	/// the copy off the server, when there is one
	remote_backup: Option<crate::backup::remote::Summary>,
	/// what the minute check sees right now
	problems: Vec<Problem>,
	/// routines and alerts that wait for an admin
	attention: Vec<Attention>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Server {
	/// the panel's domain
	domain: Option<String>,
	hostname: String,
	genco: &'static str,
	uptime_secs: u64,
	/// over 1, 5 and 15 minutes
	load: Vec<f64>,
	cores: usize,
	memory_used: Option<i64>,
	memory_total: Option<i64>,
	disk_used: Option<i64>,
	disk_total: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupStatus {
	name: String,
	client: String,
	domains: Vec<String>,
	databases: Vec<String>,
	/// unit and its state (active, failed, …)
	services: Vec<(String, String)>,
	last_deploy: Option<Run>,
	/// since when the group is in quarantine (quarantine.rs)
	quarantined_at: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Run {
	id: i64,
	status: String,
	at: i64,
	/// a deploy's ref, a backup's group when it was only one
	what: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Problem {
	/// what it is about, `site:shop.ch`, `unit:genco-shop-php8.4.service`
	key: String,
	/// for admins, a token gets the key
	detail: String,
	since: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Attention {
	conversation_id: i64,
	/// routine or alert
	kind: String,
	/// for admins, a token gets the kind
	title: String,
	updated_at: i64,
}

async fn server(domain: Option<String>) -> Server {
	let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
	let total = crate::stats::meminfo(&info, "MemTotal:");
	let available = crate::stats::meminfo(&info, "MemAvailable:");
	let df = sh("df -B1 --output=used,size / | tail -1")
		.await
		.unwrap_or_default();
	let mut df = df.split_whitespace().map(|n| n.parse::<i64>().ok());

	Server {
		domain,
		hostname: crate::dns::fqdn().await,
		genco: env!("CARGO_PKG_VERSION"),
		uptime_secs: std::fs::read_to_string("/proc/uptime")
			.ok()
			.and_then(|u| u.split('.').next()?.parse().ok())
			.unwrap_or(0),
		load: std::fs::read_to_string("/proc/loadavg")
			.unwrap_or_default()
			.split_whitespace()
			.take(3)
			.filter_map(|n| n.parse().ok())
			.collect(),
		cores: std::thread::available_parallelism()
			.map(|n| n.get())
			.unwrap_or(1),
		memory_used: total.zip(available).map(|(t, a)| t - a),
		memory_total: total,
		disk_used: df.next().flatten(),
		disk_total: df.next().flatten(),
	}
}

fn last_deploy(
	conn: &Connection,
	group_id: i64,
) -> rusqlite::Result<Option<Run>> {
	use rusqlite::OptionalExtension as _;
	conn.query_row(
		"SELECT id, status, created_at, ref FROM deploys
		WHERE group_id = ?1 ORDER BY id DESC LIMIT 1",
		[group_id],
		|r| {
			Ok(Run {
				id: r.get(0)?,
				status: r.get(1)?,
				at: r.get(2)?,
				what: r.get(3)?,
			})
		},
	)
	.optional()
}

/// Everything from genco's database, in one lock.
struct Stored {
	groups: Vec<(hosting::Group, String, Option<Run>)>,
	last_backup: Option<Run>,
	last_full_backup: Option<i64>,
	remote_backup: Option<crate::backup::remote::Summary>,
	problems: Vec<Problem>,
	attention: Vec<Attention>,
}

fn stored(conn: &Connection) -> rusqlite::Result<Stored> {
	use rusqlite::OptionalExtension as _;

	let mut groups = vec![];
	for c in hosting::tree(conn)? {
		for g in c.groups {
			let deploy = last_deploy(conn, g.id)?;
			groups.push((g, c.name.clone(), deploy));
		}
	}

	let last_backup = conn
		.query_row(
			"SELECT b.id, b.status, b.created_at, g.name FROM backups b
			LEFT JOIN domain_groups g ON g.id = b.group_id
			ORDER BY b.id DESC LIMIT 1",
			[],
			|r| {
				Ok(Run {
					id: r.get(0)?,
					status: r.get(1)?,
					at: r.get(2)?,
					what: r.get(3)?,
				})
			},
		)
		.optional()?;
	let last_full_backup = conn.query_row(
		"SELECT max(created_at) FROM backups
		WHERE status = 'done' AND group_id IS NULL",
		[],
		|r| r.get(0),
	)?;

	let problems = conn
		.prepare(
			"SELECT key, detail, since FROM problems WHERE cleared_at IS NULL
			ORDER BY since",
		)?
		.query_map([], |r| {
			Ok(Problem {
				key: r.get(0)?,
				detail: r.get(1)?,
				since: r.get(2)?,
			})
		})?
		.collect::<rusqlite::Result<_>>()?;
	let attention = conn
		.prepare(
			"SELECT id, kind, title, updated_at FROM conversations
			WHERE report = 'attention' ORDER BY updated_at DESC",
		)?
		.query_map([], |r| {
			Ok(Attention {
				conversation_id: r.get(0)?,
				kind: r.get(1)?,
				title: r.get(2)?,
				updated_at: r.get(3)?,
			})
		})?
		.collect::<rusqlite::Result<_>>()?;

	Ok(Stored {
		groups,
		last_backup,
		last_full_backup,
		remote_backup: crate::backup::remote::summary(conn),
		problems,
		attention,
	})
}

async fn status(
	reader: Reader,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
) -> Result<Json<Status>> {
	let s = stored(&db.conn())?;

	let mut groups = vec![];
	for (g, client, last_deploy) in s.groups {
		let services = crate::host::units(&g.name)
			.await
			.unwrap_or_default()
			.into_iter()
			.map(|u| (u.unit, u.active))
			.collect();
		groups.push(GroupStatus {
			domains: g.domains.iter().map(|d| d.name.clone()).collect(),
			databases: g.databases.iter().map(|d| d.name.clone()).collect(),
			quarantined_at: g.quarantine.as_ref().map(|q| q.at),
			name: g.name,
			client,
			services,
			last_deploy,
		});
	}

	let (mut problems, mut attention) = (s.problems, s.attention);
	if reader.token {
		for p in &mut problems {
			p.detail = p.key.clone();
		}
		for a in &mut attention {
			a.title = a.kind.clone();
		}
	}
	Ok(Json(Status {
		server: server(cfg.domain.clone()).await,
		groups,
		last_backup: s.last_backup,
		last_full_backup: s.last_full_backup,
		remote_backup: s.remote_backup,
		problems,
		attention,
	}))
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/status", get(status))
}
