//! The built webui is part of the binary, every unknown path gets
//! `index.html` so the router in the browser can handle it.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::Embed)]
#[folder = "../webui/dist"]
struct Dist;

pub async fn serve(uri: Uri) -> Response {
	let path = uri.path().trim_start_matches('/');

	if let Some(file) = Dist::get(path) {
		let mime = mime_guess::from_path(path).first_or_octet_stream();
		// vite puts a hash into the names of assets
		let cache = if path.starts_with("assets/") {
			"public, max-age=31536000, immutable"
		} else {
			"no-cache"
		};
		return (
			[
				(header::CONTENT_TYPE, mime.as_ref()),
				(header::CACHE_CONTROL, cache),
			],
			file.data,
		)
			.into_response();
	}

	if path.starts_with("api/") || path.starts_with("assets/") {
		return StatusCode::NOT_FOUND.into_response();
	}

	match Dist::get("index.html") {
		Some(file) => (
			[
				(header::CONTENT_TYPE, "text/html; charset=utf-8"),
				(header::CACHE_CONTROL, "no-cache"),
			],
			file.data,
		)
			.into_response(),
		None => (StatusCode::NOT_FOUND, "webui not built").into_response(),
	}
}
