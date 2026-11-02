//! Files an admin attaches to a message for the agent. They are uploaded
//! first (`PUT /conversations/uploads?name=`) into
//! `<data dir>/uploads/<id>/<name>`, which only root reads, and the message
//! names them. The agent gets their paths and reads them with its tools;
//! images it also sees.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::users::Authed;

/// a bigger file is no message attachment, it goes to a group's files
const MAX_FILE: u64 = 100 * 1024 * 1024;
/// images up to this size go to the model too
const MAX_IMAGE: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
	/// `<hex>/<name>`, its place under the uploads
	pub id: String,
	pub name: String,
	pub size: u64,
	/// where the agent reads it
	pub path: String,
}

impl Attachment {
	/// The image's type if the model should see it.
	pub fn image_type(&self) -> Option<&'static str> {
		let mime = mime(&self.name);
		(mime.starts_with("image/") && self.size <= MAX_IMAGE).then_some(mime)
	}
}

fn dir(cfg: &Config) -> PathBuf {
	cfg.data_dir.join("uploads")
}

/// A file name as the browser sends it: one plain name, no folders.
fn check_name(name: &str) -> Result<()> {
	let ok = !name.trim().is_empty()
		&& name.len() <= 200
		&& name != "."
		&& name != ".."
		&& !name.contains(['/', '\\'])
		&& !name.chars().any(char::is_control);
	if !ok {
		return Err(Error::Invalid(format!("{name:?} is no file name")));
	}
	Ok(())
}

/// The file of an id, only inside the uploads.
fn path_of(cfg: &Config, id: &str) -> Result<PathBuf> {
	let (hex, name) = id.split_once('/').ok_or(Error::NotFound)?;
	if hex.len() != 16 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
		return Err(Error::NotFound);
	}
	check_name(name).map_err(|_| Error::NotFound)?;
	Ok(dir(cfg).join(hex).join(name))
}

/// The attachments of a message, from the ids the browser sends.
pub fn resolve(cfg: &Config, ids: &[String]) -> Result<Vec<Attachment>> {
	if ids.len() > 20 {
		return Err(Error::Invalid("at most 20 files per message".into()));
	}
	ids.iter()
		.map(|id| {
			let path = path_of(cfg, id)?;
			let meta = std::fs::symlink_metadata(&path).map_err(|_| {
				Error::Invalid(format!("{id} isn't uploaded (anymore)"))
			})?;
			Ok(Attachment {
				id: id.clone(),
				name: id.split_once('/').unwrap().1.into(),
				size: meta.len(),
				path: path.display().to_string(),
			})
		})
		.collect()
}

pub fn mime(name: &str) -> &'static str {
	let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
	match ext.as_str() {
		"png" => "image/png",
		"jpg" | "jpeg" => "image/jpeg",
		"gif" => "image/gif",
		"webp" => "image/webp",
		"pdf" => "application/pdf",
		"txt" | "log" | "md" | "csv" => "text/plain; charset=utf-8",
		_ => "application/octet-stream",
	}
}

/// The bytes of an image for the model, none if it's gone or too big.
pub fn image_bytes(a: &Attachment) -> Option<(String, Vec<u8>)> {
	let mime = a.image_type()?;
	let data = std::fs::read(FsPath::new(&a.path)).ok()?;
	(data.len() as u64 <= MAX_IMAGE).then(|| (mime.to_string(), data))
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
	name: String,
}

async fn upload(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	Query(q): Query<UploadQuery>,
	body: Body,
) -> Result<Json<Attachment>> {
	check_name(&q.name)?;
	let id = format!("{}/{}", crate::util::random_hex(8), q.name);
	let path = path_of(&cfg, &id)?;
	let parent = path.parent().unwrap();
	std::fs::create_dir_all(parent)
		.and_then(|()| crate::util::set_mode(&dir(&cfg), 0o700))
		.map_err(|e| Error::Internal(format!("uploads: {e}")))?;
	if let Err(e) = crate::util::body_to_file(body, &path, MAX_FILE).await {
		let _ = std::fs::remove_dir_all(parent);
		return Err(Error::Invalid(format!(
			"upload: {e}; files for the agent are up to 100 MB, put bigger \
			ones into a group's files"
		)));
	}
	let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
	Ok(Json(Attachment {
		id,
		name: q.name,
		size,
		path: path.display().to_string(),
	}))
}

/// An attachment for the panel (thumbnails, downloads).
async fn download(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	Path((hex, name)): Path<(String, String)>,
) -> Result<Response> {
	let path = path_of(&cfg, &format!("{hex}/{name}"))?;
	let file = tokio::fs::File::open(&path)
		.await
		.map_err(|_| Error::NotFound)?;
	let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
	Ok((
		[
			(header::CONTENT_TYPE, mime(&name)),
			(header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
		],
		body,
	)
		.into_response())
}

/// Under /conversations.
pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/uploads", put(upload))
		.route("/uploads/{hex}/{name}", get(download))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn ids_stay_inside_the_uploads() {
		let cfg: Config = toml::from_str("data-dir = \"/d\"").unwrap();
		assert_eq!(
			path_of(&cfg, "0123456789abcdef/shot.png").unwrap(),
			PathBuf::from("/d/uploads/0123456789abcdef/shot.png")
		);
		assert!(path_of(&cfg, "../../etc/passwd").is_err());
		assert!(path_of(&cfg, "0123456789abcdef/../x").is_err());
		assert!(path_of(&cfg, "0123456789abcdef/..").is_err());
		assert!(path_of(&cfg, "short/x").is_err());
		assert!(check_name("a\nb").is_err());
	}

	#[test]
	fn only_small_images_go_to_the_model() {
		let a = |name: &str, size| Attachment {
			id: String::new(),
			name: name.into(),
			size,
			path: String::new(),
		};
		assert_eq!(a("x.PNG", 10).image_type(), Some("image/png"));
		assert_eq!(a("x.png", MAX_IMAGE + 1).image_type(), None);
		assert_eq!(a("x.pdf", 10).image_type(), None);
	}
}
