//! Database downloads for development copies: with an admin's session or
//! a token with the `databases` scope. A Craft database (it has `info`
//! and `elements` tables) loses its personal data on the way, see
//! craft-dbstrip; others come as they are.

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRef, FromRequestParts, Path as UrlPath, State};
use axum::http::header;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio_util::io::ReaderStream;

use crate::AppState;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::hosting;
use crate::tokens;
use crate::util;

const DIR: &str = "/var/lib/genco/dumps";

/// An admin in the panel or a token that may download databases.
pub struct DbAccess {
	/// who downloads, for the log
	pub who: String,
}

impl<S> FromRequestParts<S> for DbAccess
where
	S: Send + Sync,
	Db: FromRef<S>,
{
	type Rejection = Error;

	async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
		Ok(Self {
			who: tokens::admin_or_token(parts, state, tokens::DATABASES)
				.await?,
		})
	}
}

/// Whether the tables are Craft's, with any prefix.
fn is_craft(tables: &[String]) -> bool {
	tables.iter().any(|t| {
		t.strip_suffix("elements")
			.is_some_and(|prefix| tables.contains(&format!("{prefix}info")))
	})
}

async fn sh(script: &str) -> Result<String> {
	util::sh(script).await.map_err(Error::Internal)
}

/// Dumps, strips if Craft and gzips into a file in DIR, returns it and
/// what the strip did.
async fn prepare(database: &hosting::Database) -> Result<(PathBuf, String)> {
	let name = database.name.as_str();
	util::private_dir(Path::new(DIR))
		.map_err(|e| Error::Internal(e.to_string()))?;
	let base = Path::new(DIR).join(format!("{name}-{}", rand::random::<u64>()));
	let raw = base.with_extension("sql");
	let out = base.with_extension("sql.gz");

	let res = async {
		// the names are checked to be [a-z0-9_]
		sh(&format!(
			"set -o pipefail; {} > {}",
			database.dump_command(),
			raw.display()
		))
		.await?;
		// craft-dbstrip reads MariaDB dumps
		if database.engine == hosting::Engine::Postgres {
			sh(&format!("gzip -c {} > {}", raw.display(), out.display()))
				.await?;
			return Ok("full, PostgreSQL".to_string());
		}
		let tables: Vec<String> = sh(&format!(
			"mariadb --batch --skip-column-names -e \"SELECT table_name FROM \
			information_schema.tables WHERE table_schema = '{name}'\""
		))
		.await?
		.lines()
		.map(str::to_string)
		.collect();

		if !is_craft(&tables) {
			sh(&format!("gzip -c {} > {}", raw.display(), out.display()))
				.await?;
			return Ok("full".to_string());
		}
		let (input, output) = (raw.clone(), out.clone());
		let report = tokio::task::spawn_blocking(move || {
			craft_dbstrip::strip_file(&input, &output)
		})
		.await
		.map_err(|e| Error::Internal(e.to_string()))?
		.map_err(Error::Internal)?;
		Ok(format!(
			"craft, {} rows of personal data removed",
			report.dropped()
		))
	}
	.await;

	let _ = std::fs::remove_file(&raw);
	match res {
		Ok(what) => Ok((out, what)),
		Err(e) => {
			let _ = std::fs::remove_file(&out);
			Err(e)
		}
	}
}

async fn download(
	access: DbAccess,
	State(db): State<Db>,
	UrlPath(name): UrlPath<String>,
) -> Result<Response> {
	let (_, database) = hosting::find_database(&db.conn(), |d| d.name == name)?;

	let (path, what) = prepare(&database).await?;
	let file = tokio::fs::File::open(&path)
		.await
		.map_err(|e| Error::Internal(e.to_string()))?;
	// gone from the disk, the open file streams until it is closed
	let _ = std::fs::remove_file(&path);
	tracing::info!("{} downloads the database {name} ({what})", access.who);

	let disposition = format!(
		"attachment; filename=\"{name}-{}.sql.gz\"",
		util::stamp().await
	);
	Ok((
		[
			(header::CONTENT_TYPE, "application/gzip".to_string()),
			(header::CONTENT_DISPOSITION, disposition),
			(header::HeaderName::from_static("x-genco-dump"), what),
		],
		Body::from_stream(ReaderStream::new(file)),
	)
		.into_response())
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/databases/{name}/dump", get(download))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn names(n: &[&str]) -> Vec<String> {
		n.iter().map(|s| s.to_string()).collect()
	}

	#[test]
	fn craft_with_any_prefix() {
		assert!(is_craft(&names(&["info", "elements", "users"])));
		assert!(is_craft(&names(&["craft_info", "craft_elements"])));
		assert!(!is_craft(&names(&["craft_info", "other_elements"])));
		assert!(!is_craft(&names(&["wp_posts", "wp_options"])));
	}
}
