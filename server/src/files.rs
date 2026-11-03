//! The file manager of a group. Every operation runs as the group's user
//! (`runuser`), so the kernel keeps it inside what the group may touch: a
//! symlink a hacked site plants can't make genco read or write as root.
//! Deleted files go to `.trash/` in the group's directory.

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;
use tokio_util::io::ReaderStream;

use crate::AppState;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::hosting::{self, Group};
use crate::render;
use crate::users::Authed;

/// Larger files download instead of opening in the editor.
const MAX_EDIT: u64 = 1024 * 1024;
const MAX_UPLOAD: usize = 2 * 1024 * 1024 * 1024;
pub const TRASH: &str = ".trash";

/// Top-level entries the file manager only reads: deploys and tools
/// write them, a change there is lost or breaks them.
const READ_ONLY: &[(&str, &str)] = &[
	("current", "deployed from git, change it in the repository"),
	("releases", "deployed from git, change it in the repository"),
	("repo.git", "the deploy's copy of the repository"),
	(".ssh", "the deploy key"),
	(".cache", "a cache"),
	(".npm", "a cache"),
	(".config", "settings of tools"),
	(".local", "files of tools"),
];

/// Why a path can't be changed here, `None` if it can.
fn read_only(rel: &str) -> Option<&'static str> {
	let first = rel.trim_start_matches('/').split('/').next()?;
	READ_ONLY
		.iter()
		.find(|(name, _)| *name == first)
		.map(|(_, why)| *why)
}

fn check_writable(rel: &str) -> Result<()> {
	match read_only(rel) {
		Some(why) => Err(Error::Invalid(format!("read only: {why}"))),
		None => Ok(()),
	}
}

/// A path inside the group's directory, from what the browser sends.
fn resolve(group: &Group, rel: &str) -> Result<PathBuf> {
	let rel = Path::new(rel.trim_start_matches('/'));
	let mut path = render::group_dir(&group.name);
	for c in rel.components() {
		match c {
			Component::Normal(part) => path.push(part),
			Component::CurDir => {}
			_ => return Err(Error::Invalid("invalid path".into())),
		}
	}
	Ok(path)
}

/// A command as the group's user, with its umask.
fn as_user(group: &Group, script: &str, args: &[&Path]) -> Command {
	let mut c = Command::new("runuser");
	c.args(["-u", &render::group_user(&group.name), "--", "sh", "-c"])
		.arg(format!("umask 027; {script}"))
		.arg("sh")
		.args(args)
		.stdin(Stdio::null());
	c
}

async fn output(mut cmd: Command) -> Result<Vec<u8>> {
	let out = cmd
		.output()
		.await
		.map_err(|e| Error::Internal(e.to_string()))?;
	if !out.status.success() {
		let err = String::from_utf8_lossy(&out.stderr);
		// "sh: 1: cannot open x: Permission denied" -> the reason
		let err = err.trim().rsplit(": ").next().unwrap_or("failed");
		return Err(Error::Invalid(err.to_string()));
	}
	Ok(out.stdout)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
	pub name: String,
	/// dir, file or link
	pub kind: String,
	/// a link to a directory opens like one
	pub dir: bool,
	pub size: u64,
	/// unix time in milliseconds
	pub modified: i64,
	/// where a link points
	pub target: Option<String>,
	/// a link whose target is missing (or a loop)
	pub broken: bool,
	/// where a link leads inside the group's directory, opening the link
	/// goes there; `None` for a link out of the group
	pub real: Option<String>,
	/// the file manager doesn't change where the link leads
	pub real_read_only: bool,
	/// the file manager doesn't change it, see `READ_ONLY`
	pub read_only: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
	/// why the directory can't be changed here
	pub read_only: Option<&'static str>,
	pub entries: Vec<Entry>,
}

/// One `find -printf '%y\t%Y\t%s\t%T@\t%l\t%f\0'` line.
fn parse_entry(line: &str) -> Option<Entry> {
	let mut cols = line.splitn(6, '\t');
	let (y, target_y, size, time, link, name) = (
		cols.next()?,
		cols.next()?,
		cols.next()?,
		cols.next()?,
		cols.next()?,
		cols.next()?,
	);
	let kind = match y {
		"d" => "dir",
		"l" => "link",
		_ => "file",
	};
	Some(Entry {
		name: name.to_string(),
		kind: kind.into(),
		dir: target_y == "d",
		size: size.parse().unwrap_or(0),
		modified: (time.parse::<f64>().unwrap_or(0.0) * 1000.0) as i64,
		target: (!link.is_empty()).then(|| link.to_string()),
		broken: y == "l" && matches!(target_y, "N" | "L"),
		real: None,
		real_read_only: false,
		read_only: false,
	})
}

/// The path of `real` (absolute, resolved) relative to the group's
/// directory, if it is inside.
fn real_path(group: &Group, real: &str) -> Option<String> {
	if real.is_empty() {
		return None;
	}
	let rel = Path::new(real)
		.strip_prefix(render::group_dir(&group.name))
		.ok()?;
	Some(rel.to_str()?.to_string())
}

/// Resolves the links of a listing as the group's user, so a link only
/// leads where the group could go itself.
async fn resolve_links(g: &Group, dir: &Path, entries: &mut [Entry]) {
	let links: Vec<PathBuf> = entries
		.iter()
		.filter(|e| e.kind == "link" && !e.broken)
		.map(|e| dir.join(&e.name))
		.collect();
	if links.is_empty() {
		return;
	}
	let args: Vec<&Path> = links.iter().map(PathBuf::as_path).collect();
	let Ok(out) = output(as_user(
		g,
		r#"for f; do printf '%s\0' "$(realpath -e -- "$f" 2>/dev/null)"; done"#,
		&args,
	))
	.await
	else {
		return;
	};

	let out = String::from_utf8_lossy(&out);
	let mut reals = out.split('\0');
	for e in entries.iter_mut().filter(|e| e.kind == "link" && !e.broken) {
		e.real = reals.next().and_then(|r| real_path(g, r));
		e.real_read_only =
			e.real.as_deref().is_some_and(|r| read_only(r).is_some());
	}
}

#[derive(Debug, Deserialize)]
struct PathQuery {
	#[serde(default)]
	path: String,
}

fn group(db: &Db, id: i64) -> Result<Group> {
	hosting::find_group(&db.conn(), id)
}

async fn list(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Query(q): Query<PathQuery>,
) -> Result<Json<Listing>> {
	let g = group(&db, id)?;
	let dir = resolve(&g, &q.path)?;
	let out = output(as_user(
		&g,
		r#"find -H "$1" -mindepth 1 -maxdepth 1 -printf '%y\t%Y\t%s\t%T@\t%l\t%f\0'"#,
		&[&dir],
	))
	.await?;

	let mut entries: Vec<Entry> = String::from_utf8_lossy(&out)
		.split('\0')
		.filter(|l| !l.is_empty())
		.filter_map(parse_entry)
		.collect();
	entries.sort_by(|a, b| b.dir.cmp(&a.dir).then(a.name.cmp(&b.name)));
	for e in &mut entries {
		e.read_only = read_only(&format!("{}/{}", q.path, e.name)).is_some();
	}
	resolve_links(&g, &dir, &mut entries).await;
	Ok(Json(Listing {
		read_only: read_only(&q.path),
		entries,
	}))
}

/// A text file for the editor.
async fn read(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Query(q): Query<PathQuery>,
) -> Result<String> {
	let g = group(&db, id)?;
	let path = resolve(&g, &q.path)?;
	let out = output(as_user(
		&g,
		// devices and fifos report size 0, only regular files are read
		&format!(r#"[ -f "$1" ] || {{ echo "not a regular file" >&2; exit 1; }}; size=$(stat -L -c %s -- "$1") || exit 1; [ "$size" -le {MAX_EDIT} ] || {{ echo "too large to edit, download it" >&2; exit 1; }}; head -c {MAX_EDIT} -- "$1""#),
		&[&path],
	))
	.await?;
	String::from_utf8(out)
		.map_err(|_| Error::Invalid("not a text file, download it".into()))
}

/// Writes the body to the file, for the editor and uploads.
async fn write(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Query(q): Query<PathQuery>,
	body: Body,
) -> Result<Json<()>> {
	check_writable(&q.path)?;
	let g = group(&db, id)?;
	let path = resolve(&g, &q.path)?;
	// an uploaded folder brings its subfolders; the file is replaced only
	// when it arrived complete, an aborted upload leaves the old one
	let script = r#"mkdir -p -- "$(dirname -- "$1")" && cat > "$1.genco-upload" && mv -f -- "$1.genco-upload" "$1""#;
	let mut child = as_user(&g, script, &[&path])
		.stdin(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.map_err(|e| Error::Internal(e.to_string()))?;

	let mut stdin = child.stdin.take().unwrap();
	let mut stream = body.into_data_stream();
	while let Some(chunk) = stream.next().await {
		let chunk: Bytes = match chunk {
			Ok(c) => c,
			Err(e) => {
				// the shell must not move the part over the file
				let _ = child.kill().await;
				let tmp =
					PathBuf::from(format!("{}.genco-upload", path.display()));
				let _ = output(as_user(&g, r#"rm -f -- "$1""#, &[&tmp])).await;
				return Err(Error::Invalid(format!("upload: {e}")));
			}
		};
		if stdin.write_all(&chunk).await.is_err() {
			break;
		}
	}
	drop(stdin);

	let out = child
		.wait_with_output()
		.await
		.map_err(|e| Error::Internal(e.to_string()))?;
	if !out.status.success() {
		let err = String::from_utf8_lossy(&out.stderr);
		return Err(Error::Invalid(
			err.trim().rsplit(": ").next().unwrap_or("failed").into(),
		));
	}
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
struct PathReq {
	path: String,
}

async fn mkdir(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<PathReq>,
) -> Result<Json<()>> {
	check_writable(&req.path)?;
	let g = group(&db, id)?;
	let path = resolve(&g, &req.path)?;
	output(as_user(&g, r#"mkdir -- "$1""#, &[&path])).await?;
	Ok(Json(()))
}

/// An empty file, never over an existing one.
async fn create(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<PathReq>,
) -> Result<Json<()>> {
	check_writable(&req.path)?;
	let g = group(&db, id)?;
	let path = resolve(&g, &req.path)?;
	output(as_user(&g, r#"set -C; : > "$1""#, &[&path])).await?;
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
struct RenameReq {
	from: String,
	to: String,
}

async fn rename(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<RenameReq>,
) -> Result<Json<()>> {
	check_writable(&req.from)?;
	check_writable(&req.to)?;
	let g = group(&db, id)?;
	let from = resolve(&g, &req.from)?;
	let to = resolve(&g, &req.to)?;
	output(as_user(&g, r#"mv -n -T -- "$1" "$2""#, &[&from, &to])).await?;
	Ok(Json(()))
}

/// A copy next to it or elsewhere in the group, never over something.
async fn copy(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<RenameReq>,
) -> Result<Json<()>> {
	check_writable(&req.to)?;
	let g = group(&db, id)?;
	let from = resolve(&g, &req.from)?;
	let to = resolve(&g, &req.to)?;
	output(as_user(
		&g,
		r#"[ ! -e "$2" ] || { echo "$2: exists already" >&2; exit 1; }; cp -a -T -- "$1" "$2""#,
		&[&from, &to],
	))
	.await?;
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
struct ExtractReq {
	/// the archive
	path: String,
	/// the folder it goes into, created if missing; files with the same
	/// names are replaced
	to: String,
}

/// The command that extracts an archive by its name.
fn extract_script(name: &str) -> Option<&'static str> {
	let lower = name.to_lowercase();
	if lower.ends_with(".zip") {
		Some(r#"unzip -q -o -- "$1" -d "$2""#)
	} else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
		Some(r#"tar -xzf "$1" -C "$2" --no-same-owner --no-same-permissions"#)
	} else if lower.ends_with(".tar") {
		Some(r#"tar -xf "$1" -C "$2" --no-same-owner --no-same-permissions"#)
	} else {
		None
	}
}

/// Unpacks a zip or tar archive, for uploading a big folder at once.
async fn extract(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<ExtractReq>,
) -> Result<Json<()>> {
	check_writable(&req.to)?;
	let script = extract_script(&req.path).ok_or(Error::Invalid(
		"only .zip, .tar, .tar.gz and .tgz can be extracted".into(),
	))?;
	let g = group(&db, id)?;
	let path = resolve(&g, &req.path)?;
	let to = resolve(&g, &req.to)?;
	// as the group's user: what an archive holds (../, links) can't get
	// further than the group could itself
	// archives may carry modes like 0600, nginx reads as group member
	output(as_user(
		&g,
		&format!(
			r#"mkdir -p -- "$2" && {script} && chmod -R u+rwX,g+rX,o-rwx -- "$2""#
		),
		&[&path, &to],
	))
	.await?;
	Ok(Json(()))
}

/// Moves the file or directory to `.trash/<time>-<name>`.
async fn delete(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<PathReq>,
) -> Result<Json<()>> {
	check_writable(&req.path)?;
	let g = group(&db, id)?;
	to_trash(&g, &req.path).await?;
	Ok(Json(()))
}

/// Moves `rel` of the group's directory to its trash, as the group's
/// user; the nightly backup empties it after 30 days.
pub async fn to_trash(g: &Group, rel: &str) -> Result<()> {
	let path = resolve(g, rel)?;
	let trash = render::group_dir(&g.name).join(TRASH);
	if path == render::group_dir(&g.name) || path.starts_with(&trash) {
		return Err(Error::Invalid("that can't be deleted here".into()));
	}
	output(as_user(
		g,
		r#"mkdir -p "$2" && mv -T -- "$1" "$2/$(date +%Y%m%d-%H%M%S-%N)-$(basename "$1")""#,
		&[&path, &trash],
	))
	.await?;
	Ok(())
}

/// A file as it is, a directory as zip. The browser opens it as link, so
/// the session token comes in the query.
async fn download(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	Query(q): Query<PathQuery>,
) -> Result<Response> {
	let g = group(&db, id)?;
	let path = resolve(&g, &q.path)?;
	let name = path
		.file_name()
		.map(|n| n.to_string_lossy().to_string())
		.unwrap_or_else(|| g.name.clone());

	if output(as_user(&g, r#"test -e "$1""#, &[&path]))
		.await
		.is_err()
	{
		return Err(Error::NotFound);
	}
	let is_dir = output(as_user(&g, r#"test -d "$1""#, &[&path]))
		.await
		.is_ok();
	let (script, file_name, mime) = if is_dir {
		(
			r#"cd "$1" && zip -q -r - ."#,
			format!("{name}.zip"),
			"application/zip",
		)
	} else {
		(r#"cat "$1""#, name, "application/octet-stream")
	};

	let mut child = as_user(&g, script, &[&path])
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.kill_on_drop(true)
		.spawn()
		.map_err(|e| Error::Internal(e.to_string()))?;
	let stdout = child.stdout.take().unwrap();
	// the child is reaped when the stream ends
	tokio::spawn(async move {
		let _ = child.wait().await;
	});

	let body = Body::from_stream(ReaderStream::new(stdout));
	let disposition = disposition(&file_name);
	Ok((
		[
			(header::CONTENT_TYPE, mime.to_string()),
			(header::CONTENT_DISPOSITION, disposition),
		],
		body,
	)
		.into_response())
}

/// An ASCII name for old browsers and the real one encoded: a file of a
/// site may be called anything, control characters would break the header.
fn disposition(name: &str) -> String {
	let ascii: String = name
		.chars()
		.map(|c| {
			if c.is_ascii_alphanumeric() || "._- ".contains(c) {
				c
			} else {
				'_'
			}
		})
		.collect();
	let encoded: String = name
		.bytes()
		.map(|b| {
			if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
				(b as char).to_string()
			} else {
				format!("%{b:02X}")
			}
		})
		.collect();
	format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/groups/{id}/files", get(list))
		.route("/groups/{id}/files/read", get(read))
		.route(
			"/groups/{id}/files/write",
			put(write).layer(DefaultBodyLimit::max(MAX_UPLOAD)),
		)
		.route("/groups/{id}/files/mkdir", post(mkdir))
		.route("/groups/{id}/files/create", post(create))
		.route("/groups/{id}/files/rename", post(rename))
		.route("/groups/{id}/files/copy", post(copy))
		.route("/groups/{id}/files/extract", post(extract))
		.route("/groups/{id}/files/delete", post(delete))
		.route("/groups/{id}/files/download", get(download))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn group() -> Group {
		Group {
			id: 1,
			name: "shop".into(),
			client_id: 1,
			user: "g-shop".into(),
			dir: "/srv/genco/shop".into(),
			branch: "main".into(),
			..Group::default()
		}
	}

	#[test]
	fn paths_stay_in_the_group() {
		let g = group();
		assert_eq!(
			resolve(&g, "shared/craft/.env").unwrap(),
			PathBuf::from("/srv/genco/shop/shared/craft/.env")
		);
		assert_eq!(resolve(&g, "").unwrap(), PathBuf::from("/srv/genco/shop"));
		assert_eq!(
			resolve(&g, "/a/./b").unwrap(),
			PathBuf::from("/srv/genco/shop/a/b")
		);
		assert!(resolve(&g, "../other").is_err());
		assert!(resolve(&g, "a/../../etc").is_err());
	}

	#[test]
	fn deploys_and_tools_are_read_only() {
		assert!(read_only("current/craft/web/index.php").is_some());
		assert!(read_only("/releases").is_some());
		assert!(read_only(".ssh/deploy_key").is_some());
		assert!(read_only("shared/craft/.env").is_none());
		assert!(read_only("").is_none());
		assert!(read_only("currently").is_none());
	}

	#[test]
	fn download_names_are_safe() {
		assert_eq!(
			disposition("ä \"x\"\n.txt"),
			"attachment; filename=\"_ _x__.txt\"; \
			filename*=UTF-8''%C3%A4%20%22x%22%0A.txt"
		);
	}

	#[test]
	fn extracts_zip_and_tar() {
		assert!(extract_script("assets.zip").unwrap().starts_with("unzip"));
		assert!(extract_script("Assets.ZIP").is_some());
		assert!(extract_script("a.tar.gz").unwrap().contains("-xzf"));
		assert!(extract_script("a.tgz").is_some());
		assert!(extract_script("a.tar").unwrap().contains("-xf"));
		assert!(extract_script("a.rar").is_none());
	}

	#[test]
	fn parses_find_lines() {
		let e =
			parse_entry("l\td\t24\t1793518657.5\treleases/1\tcurrent").unwrap();
		assert_eq!(e.kind, "link");
		assert!(e.dir);
		assert_eq!(e.target.as_deref(), Some("releases/1"));
		assert_eq!(e.modified, 1793518657500);

		let e = parse_entry("f\tf\t10\t1.0\t\tname\twith tab").unwrap();
		assert_eq!(e.name, "name\twith tab");
		assert!(!e.dir);
	}

	#[test]
	fn links_show_their_target() {
		let env = "/srv/genco/shop/shared/craft/.env";
		let e = parse_entry(&format!("l\tf\t33\t1.0\t{env}\t.env")).unwrap();
		assert_eq!(e.kind, "link");
		assert!(!e.dir);
		assert!(!e.broken);
		assert_eq!(e.target.as_deref(), Some(env));

		let e = parse_entry("l\td\t9\t1.0\t../x\tassets").unwrap();
		assert!(e.dir);
		assert!(!e.broken);

		for y in ["N", "L"] {
			let e = parse_entry(&format!("l\t{y}\t9\t1.0\tgone\tx")).unwrap();
			assert!(e.broken);
			assert!(!e.dir);
		}
	}

	#[test]
	fn links_lead_into_the_group_only() {
		let g = group();
		assert_eq!(
			real_path(&g, "/srv/genco/shop/shared/craft/web/assets").as_deref(),
			Some("shared/craft/web/assets")
		);
		assert_eq!(
			real_path(&g, "/srv/genco/shop/shared/craft/.env").as_deref(),
			Some("shared/craft/.env")
		);
		assert_eq!(
			real_path(&g, "/srv/genco/shop/releases/1").as_deref(),
			Some("releases/1")
		);
		assert_eq!(real_path(&g, "/srv/genco/shop").as_deref(), Some(""));
		assert!(real_path(&g, "/srv/genco/shopping/x").is_none());
		assert!(real_path(&g, "/srv/genco/other/shared/.env").is_none());
		assert!(real_path(&g, "/etc/passwd").is_none());
		assert!(real_path(&g, "").is_none());
	}
}
