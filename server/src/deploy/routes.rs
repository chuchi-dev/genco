//! The deploy API: the panel's deploys and rollbacks, CI deploys and
//! uploads with a deploy token, the log and the deploy key.

use super::*;

pub(super) fn ctx(state: &AppState) -> Ctx {
	Ctx {
		cfg: state.cfg.clone(),
		db: state.db.clone(),
		events: state.events.clone(),
		print: false,
	}
}

pub(super) fn start(state: &AppState, id: i64) {
	let ctx = ctx(state);
	tokio::spawn(async move { run(&ctx, id).await });
}

#[derive(Debug, Deserialize)]
pub(super) struct DeployReq {
	#[serde(rename = "ref")]
	pub(super) git_ref: Option<String>,
}

pub(super) async fn deploy_group(
	authed: Authed,
	State(state): State<AppState>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<DeployReq>,
) -> Result<Json<i64>> {
	let queued = {
		let conn = state.db.conn();
		let group = hosting::find_group(&conn, id)?;
		queue(&conn, &group, req.git_ref.as_deref(), &authed.user.name)?
	};
	if queued.new {
		start(&state, queued.id);
	}
	Ok(Json(queued.id))
}

#[derive(Debug, Deserialize)]
pub(super) struct CiDeployReq {
	/// name of the group
	pub(super) group: String,
	#[serde(rename = "ref")]
	pub(super) git_ref: Option<String>,
}

/// For CI jobs, with a token that may deploy the group.
pub(super) async fn ci_deploy(
	token: ApiToken,
	State(state): State<AppState>,
	Json(req): Json<CiDeployReq>,
) -> Result<Json<i64>> {
	let queued = {
		let conn = state.db.conn();
		let group = hosting::group_by_name(&conn, &req.group)?
			.ok_or(Error::Forbidden)?;
		token.require(&tokens::deploy_scope(group.id))?;
		queue(
			&conn,
			&group,
			req.git_ref.as_deref(),
			&format!("token {}", token.name),
		)?
	};
	if queued.new {
		start(&state, queued.id);
	}
	Ok(Json(queued.id))
}

#[derive(Debug, Deserialize)]
pub(super) struct UploadQuery {
	pub(super) group: String,
	/// names the deploy, a tag or branch
	#[serde(rename = "ref")]
	pub(super) git_ref: Option<String>,
	pub(super) commit: Option<String>,
}

/// A release a CI built, as a tarball (gzip or not) of the release's
/// files, with a token that may deploy the group. Build steps don't run,
/// the rest does (shared paths, binary check, after the switch).
pub(super) async fn upload_deploy(
	token: ApiToken,
	State(state): State<AppState>,
	axum::extract::Query(q): axum::extract::Query<UploadQuery>,
	body: axum::body::Body,
) -> Result<Json<i64>> {
	let id = {
		let conn = state.db.conn();
		let group =
			hosting::group_by_name(&conn, &q.group)?.ok_or(Error::Forbidden)?;
		token.require(&tokens::deploy_scope(group.id))?;
		queue_upload(
			&conn,
			&group,
			q.git_ref.as_deref(),
			q.commit.as_deref(),
			&format!("token {}", token.name),
		)?
	};
	let path = upload_path(id);
	// a client that stops sending, or goes away, mustn't keep the group's
	// one waiting deploy: the guard fails it if this ends early
	let guard = UploadGuard {
		ctx: ctx(&state),
		id,
		armed: true,
	};
	let written = util::body_to_file(body, &path, 2 << 30).await;
	if let Err(e) = written {
		return Err(Error::Invalid(format!("upload: {e}")));
	}
	let mut guard = guard;
	guard.armed = false;
	start(&state, id);
	Ok(Json(id))
}

/// Fails an upload's deploy and removes its file unless disarmed.
pub(super) struct UploadGuard {
	pub(super) ctx: Ctx,
	pub(super) id: i64,
	pub(super) armed: bool,
}

impl Drop for UploadGuard {
	fn drop(&mut self) {
		if self.armed {
			let _ = std::fs::remove_file(upload_path(self.id));
			self.ctx.status(self.id, "failed");
		}
	}
}

pub(super) async fn list_deploys(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<Vec<Deploy>>> {
	let mut deploys = list(&db.conn(), id)?;
	mark_live(&db, id, &mut deploys)?;
	Ok(Json(deploys))
}

/// The newest deploy of the release `current` points to is live; a
/// rollback shares the release with the deploy it goes back to.
fn mark_live(db: &Db, group_id: i64, deploys: &mut [Deploy]) -> Result<()> {
	let group = hosting::find_group(&db.conn(), group_id)?;
	let link =
		std::fs::read_link(render::group_dir(&group.name).join("current"));
	let release = link
		.ok()
		.and_then(|l| Some(l.file_name()?.to_string_lossy().to_string()));
	if let Some(d) = deploys
		.iter_mut()
		.find(|d| d.release.is_some() && d.release == release)
	{
		d.live = true;
	}
	Ok(())
}

/// The deploy if the caller may see it: an admin, or a token that may
/// deploy its group (a CI job following the deploy it started). An
/// unknown one and another group's answer the same, a token learns
/// nothing of other groups.
fn readable(caller: &Caller, db: &Db, id: i64) -> Result<Deploy> {
	let d = get(&db.conn(), id).map_err(|_| Error::NotFound)?;
	if let Caller::Token(token) = caller {
		token
			.require(&tokens::deploy_scope(d.group_id))
			.map_err(|_| Error::NotFound)?;
	}
	Ok(d)
}

/// One deploy: its status and the commit it deployed (a CI job compares
/// it with its own, a queued deploy takes the newest request's ref).
pub(super) async fn get_deploy(
	caller: Caller,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<Deploy>> {
	let mut d = readable(&caller, &db, id)?;
	let mut newest = list(&db.conn(), d.group_id)?;
	mark_live(&db, d.group_id, &mut newest)?;
	d.live = newest.iter().any(|n| n.id == d.id && n.live);
	Ok(Json(d))
}

#[derive(Debug, Deserialize)]
pub(super) struct LogQuery {
	/// bytes already read: only what came after, the header
	/// `x-log-offset` says where to go on
	#[serde(default)]
	offset: Option<u64>,
}

pub(super) async fn deploy_log(
	caller: Caller,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
	axum::extract::Query(q): axum::extract::Query<LogQuery>,
) -> Result<([(&'static str, String); 1], String)> {
	readable(&caller, &db, id)?;
	let (text, next) = job::read_from(LOG_DIR, id, q.offset.unwrap_or(0)).await;
	Ok(([("x-log-offset", next.to_string())], text))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RollbackReq {
	pub(super) deploy_id: i64,
}

pub(super) async fn rollback_group(
	authed: Authed,
	State(state): State<AppState>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<RollbackReq>,
) -> Result<Json<i64>> {
	let (group, to) = {
		let conn = state.db.conn();
		let group = hosting::find_group(&conn, id)?;
		let to = get(&conn, req.deploy_id)?;
		if to.group_id != group.id {
			return Err(Error::NotFound);
		}
		(group, to)
	};
	let id = queue_rollback(&state.db.conn(), &group, &to, &authed.user.name)?;
	let ctx = ctx(&state);
	tokio::spawn(async move { run_rollback(&ctx, id, &group).await });
	Ok(Json(id))
}

/// Takes a deploy's build out of the runners' queue, the deploy fails.
pub(super) async fn cancel_deploy(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<()>> {
	crate::runners::cancel(&db.conn(), id)?;
	Ok(Json(()))
}

pub(super) async fn group_deploy_key(
	_authed: Authed,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<Option<String>>> {
	let group = hosting::find_group(&db.conn(), id)?;
	Ok(Json(deploy_key(&group).await))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/deploy", post(ci_deploy))
		.route(
			"/deploy/artifact",
			routing::put(upload_deploy)
				.layer(axum::extract::DefaultBodyLimit::max(2 << 30)),
		)
		.route("/groups/{id}/deploy", post(deploy_group))
		.route("/groups/{id}/deploys", routing::get(list_deploys))
		.route("/groups/{id}/rollback", post(rollback_group))
		.route("/groups/{id}/deploy-key", routing::get(group_deploy_key))
		.route("/deploys/{id}", routing::get(get_deploy))
		.route("/deploys/{id}/log", routing::get(deploy_log))
		.route("/deploys/{id}/cancel", post(cancel_deploy))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_token_sees_only_its_group_s_deploys() {
		let db = Db::memory();
		db.conn()
			.execute_batch(
				"INSERT INTO clients (name, created_at) VALUES ('c', 0);
				INSERT INTO domain_groups (name, client_id, created_at)
				VALUES ('a', 1, 0), ('b', 1, 0);
				INSERT INTO deploys (group_id, ref, status, started_by,
				created_at) VALUES (1, 'main', 'done', 'x', 0),
				(2, 'main', 'done', 'x', 0);",
			)
			.unwrap();
		let token =
			|scope: &str| Caller::Token(ApiToken::for_test(vec![scope.into()]));
		let a = token("deploy:1");
		assert_eq!(readable(&a, &db, 1).unwrap().group_id, 1);
		let other = readable(&a, &db, 2).unwrap_err();
		let unknown = readable(&a, &db, 99).unwrap_err();
		assert_eq!(other.to_string(), unknown.to_string());
		assert!(readable(&token("databases"), &db, 1).is_err());
	}
}
