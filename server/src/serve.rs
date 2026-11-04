//! `genco serve`: the panel and the agent; what a restart left half done
//! is ended first.

use crate::agent::Runner;
use crate::config::Config;
use crate::db::Db;
use crate::events::Events;
use crate::*;
use axum::Router;
use axum::http::{HeaderName, HeaderValue};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

/// Deploys, backups and deletions that ran when genco stopped don't run
/// anymore, they would show as running forever.
pub(crate) fn end_interrupted(db: &Db) -> Result<(), String> {
	let now = db::now();
	let conn = db.conn();
	// a deploy or backup of `genco` in a terminal or the timer holds its
	// lock, it is still running
	let mut sql = String::new();
	// per group: a deploy in a terminal holds its group's lock
	let open: Vec<(i64, String)> = conn
		.prepare(
			"SELECT d.id, g.name FROM deploys d
			JOIN domain_groups g ON g.id = d.group_id
			WHERE d.status IN ('queued', 'running')",
		)
		.and_then(|mut stmt| {
			stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
				.collect()
		})
		.map_err(|e| e.to_string())?;
	for (id, group) in open {
		// a waiting one belonged to the process that is gone (a terminal's
		// own deploy sets its status again when it gets the lock)
		let status: String = conn
			.query_row("SELECT status FROM deploys WHERE id = ?1", [id], |r| {
				r.get(0)
			})
			.map_err(|e| e.to_string())?;
		if status == "queued" || job::is_free(&deploy::lock_path(&group)) {
			sql += &format!(
				"UPDATE deploys SET status = 'failed', finished_at = {now}
				WHERE id = {id};
				UPDATE build_jobs SET status = 'failed', finished_at = {now},
				message = 'genco restarted'
				WHERE deploy_id = {id} AND status IN ('packing', 'queued', 'running');"
			);
		}
	}
	if job::is_free(backup::LOCK) {
		sql += &format!(
			"UPDATE backups SET status = 'failed', finished_at = {now}
			WHERE status = 'running';"
		);
	}
	sql += &format!(
		"UPDATE pending_actions SET status = 'failed', done_at = {now}
		WHERE status = 'running';"
	);
	conn.execute_batch(&sql).map_err(|e| e.to_string())?;
	drop(conn);
	sweep(db);
	Ok(())
}

/// Files of runs that are over: build jobs' folders and uploads whose
/// deploy isn't running.
pub(crate) fn sweep(db: &Db) {
	let conn = db.conn();
	let open = |sql: &str, id: i64| -> bool {
		conn.query_row(sql, [id], |r| r.get::<_, bool>(0))
			.unwrap_or(false)
	};
	for entry in std::fs::read_dir("/var/lib/genco/builds")
		.into_iter()
		.flatten()
		.flatten()
	{
		let job = entry.file_name().to_string_lossy().parse::<i64>().ok();
		let live = job.is_some_and(|j| {
			open(
				"SELECT EXISTS (SELECT 1 FROM build_jobs WHERE id = ?1
				AND status IN ('packing', 'queued', 'running', 'done'))",
				j,
			)
		});
		if !live {
			let _ = std::fs::remove_dir_all(entry.path());
		}
	}
	for entry in std::fs::read_dir("/var/lib/genco/deploys")
		.into_iter()
		.flatten()
		.flatten()
	{
		let name = entry.file_name().to_string_lossy().to_string();
		let Some(id) = name
			.strip_suffix(".upload")
			.and_then(|n| n.parse::<i64>().ok())
		else {
			continue;
		};
		let live = open(
			"SELECT EXISTS (SELECT 1 FROM deploys WHERE id = ?1
			AND status IN ('queued', 'running'))",
			id,
		);
		if !live {
			let _ = std::fs::remove_file(entry.path());
		}
	}
}

/// A header on every response, see `serve`.
pub(crate) fn header(
	name: &'static str,
	value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
	SetResponseHeaderLayer::overriding(
		HeaderName::from_static(name),
		HeaderValue::from_static(value),
	)
}

/// The content security policy: the panel loads nothing from other
/// origins and tells nobody where it came from; Monaco needs its
/// same-origin worker and inline styles, the 2FA qr code is a data: image,
/// the chat's attachments are blob: ones. Only the sites in
/// `frame-ancestors` may frame it.
fn csp(frame_ancestors: &[String]) -> Result<HeaderValue, String> {
	for site in frame_ancestors {
		let host = site.strip_prefix("https://").unwrap_or("");
		let ok = !host.is_empty()
			&& host.chars().all(|c| {
				c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')
			});
		if !ok {
			return Err(format!(
				"frame-ancestors: {site} is not an https://host address"
			));
		}
	}
	let ancestors = match frame_ancestors {
		[] => "'none'".to_string(),
		sites => sites.join(" "),
	};
	HeaderValue::from_str(&format!(
		"default-src 'self'; script-src 'self'; worker-src 'self'; \
		style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
		font-src 'self' data:; connect-src 'self'; \
		frame-ancestors {ancestors}; base-uri 'none'; form-action 'self'; \
		object-src 'none'"
	))
	.map_err(|e| e.to_string())
}

pub(crate) async fn serve(cfg: Config) -> Result<(), String> {
	std::fs::create_dir_all(&cfg.data_dir).map_err(|e| e.to_string())?;
	// only root reads genco's database, logs and dumps
	util::set_mode(&cfg.data_dir, 0o700).map_err(|e| e.to_string())?;
	let cfg = Arc::new(cfg);
	let db = Db::open(&cfg.db_path())?;
	let events = Events::new();
	if let Err(e) = peers::adopt_old_webhook(&db) {
		tracing::error!("the old notification webhook: {e}");
	}
	if let Err(e) = proposals::clean_pending(&db) {
		tracing::error!("cleaning the waiting proposals: {e}");
	}
	let runner = Runner::new(cfg.clone(), db.clone(), events.clone());
	runner.resume_interrupted();
	end_interrupted(&db)?;
	if runner.has_llm() {
		runner.onboard();
	}
	checks::spawn(cfg.clone(), db.clone(), runner.clone(), events.clone());
	let stats = stats::StatsDb::open(&stats::path(&cfg))?;
	stats::spawn(db.clone(), stats.clone());

	let state = AppState {
		cfg: cfg.clone(),
		db,
		events: events.clone(),
		runner: runner.clone(),
		stats,
	};

	let api = Router::new()
		.nest("/users", users::routes())
		.nest("/conversations", conversations::routes())
		.nest("/events", events::routes())
		.nest("/tokens", tokens::routes())
		.merge(hosting::routes())
		.merge(deploy::routes())
		.merge(backup::routes())
		.merge(actions::routes())
		.merge(mail::routes())
		.merge(mailbox::routes())
		.merge(files::routes())
		.merge(dumps::routes())
		.merge(routines::routes())
		.merge(activity::routes())
		.merge(stats::routes())
		.merge(status::routes())
		.merge(secrets::routes())
		.merge(firewall::routes())
		.merge(costs::routes())
		.merge(imports::routes())
		.merge(notify::routes())
		.merge(llm::routes())
		.merge(peers::routes())
		.merge(proposals::routes())
		.merge(update::routes())
		.merge(appenv::routes())
		.merge(procs::routes())
		.merge(runners::routes())
		.layer(axum::middleware::from_fn_with_state(
			state.clone(),
			activity::record,
		));
	let mut app = Router::new()
		.nest("/api", api)
		.fallback(webui::serve)
		.layer(SetResponseHeaderLayer::overriding(
			HeaderName::from_static("content-security-policy"),
			csp(&cfg.frame_ancestors)?,
		));
	// for older browsers, which don't know frame-ancestors
	if cfg.frame_ancestors.is_empty() {
		app = app.layer(header("x-frame-options", "DENY"));
	}
	let app = app
		.layer(header("x-content-type-options", "nosniff"))
		.layer(header("referrer-policy", "no-referrer"))
		.layer(TraceLayer::new_for_http())
		.with_state(state);

	let listener = TcpListener::bind(&cfg.listen)
		.await
		.map_err(|e| format!("failed to listen on {}: {e}", cfg.listen))?;
	tracing::info!("listening on http://{}", cfg.listen);

	// open requests get a few seconds to finish, the event streams end
	// right away
	let server = axum::serve(listener, app).with_graceful_shutdown({
		let events = events.clone();
		async move {
			let mut term =
				signal::unix::signal(signal::unix::SignalKind::terminate())
					.unwrap();
			tokio::select! {
				_ = signal::ctrl_c() => {}
				_ = term.recv() => {}
			}
			runner.shutdown();
			events.close();
		}
	});
	tokio::select! {
		res = server => res.map_err(|e| e.to_string()),
		_ = async {
			events.closed().await;
			tokio::time::sleep(Duration::from_secs(5)).await;
		} => {
			tracing::warn!("requests still open after 5 s, exiting anyway");
			Ok(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn framed_only_by_the_configured_sites() {
		let none = csp(&[]).unwrap();
		assert!(none.to_str().unwrap().contains("frame-ancestors 'none';"));
		let one = csp(&["https://noah.example".into()]).unwrap();
		assert!(
			one.to_str()
				.unwrap()
				.contains("frame-ancestors https://noah.example;")
		);
		assert!(csp(&["http://noah.example".into()]).is_err());
		assert!(csp(&["https://a.example; script-src *".into()]).is_err());
	}
}
