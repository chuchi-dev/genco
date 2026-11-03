//! The mail API of the panel.

use super::*;

pub(super) fn domain_by_id(
	conn: &Connection,
	id: i64,
) -> Result<(i64, String)> {
	Ok(conn.query_row(
		"SELECT group_id, name FROM mail_domains WHERE id = ?1",
		[id],
		|r| Ok((r.get(0)?, r.get(1)?)),
	)?)
}

/// Brings mail in line after a change, in the background: a new mailbox
/// works a moment later.
pub(super) fn apply_soon(cfg: Arc<Config>, db: Db, events: Events) {
	events.send(Event::Tree);
	tokio::spawn(async move {
		let report = crate::apply::apply_mail(&cfg, &db).await;
		if !report.ok {
			tracing::error!("mail apply: {}", report.log.join("; "));
		}
		events.send(Event::Tree);
	});
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DomainReq {
	pub(super) group_id: i64,
	pub(super) name: String,
	#[serde(default)]
	pub(super) send_only: bool,
}

pub(super) async fn create_domain(
	_authed: Authed,
	State(state): State<AppState>,
	Json(req): Json<DomainReq>,
) -> Result<Json<i64>> {
	let id =
		add_domain(&state.db.conn(), req.group_id, &req.name, req.send_only)?;
	apply_soon(state.cfg.clone(), state.db.clone(), state.events.clone());
	Ok(Json(id))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModeReq {
	pub(super) send_only: bool,
}

pub(super) async fn update_mode(
	_authed: Authed,
	State(state): State<AppState>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<ModeReq>,
) -> Result<Json<()>> {
	set_mode(&state.db.conn(), id, req.send_only)?;
	apply_soon(state.cfg.clone(), state.db.clone(), state.events.clone());
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SenderReq {
	pub(super) local_part: String,
	#[serde(default)]
	pub(super) note: String,
}

pub(super) async fn create_sender(
	_authed: Authed,
	State(state): State<AppState>,
	UrlPath(domain_id): UrlPath<i64>,
	Json(req): Json<SenderReq>,
) -> Result<Json<i64>> {
	domain_by_id(&state.db.conn(), domain_id)?;
	let key = std::path::Path::new(crate::secrets::KEY_FILE);
	let id =
		send_only::add(&state.db, key, domain_id, &req.local_part, &req.note)
			.await?;
	apply_soon(state.cfg.clone(), state.db.clone(), state.events.clone());
	Ok(Json(id))
}

/// The app settings with the password; the activity log records who
/// asked.
pub(super) async fn sender_settings(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<send_only::AppSettings>> {
	let key = std::path::Path::new(crate::secrets::KEY_FILE);
	Ok(Json(send_only::app_settings(&cfg, &db.conn(), key, id)?))
}

pub(super) async fn sender_password(
	_authed: Authed,
	State(state): State<AppState>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<()>> {
	let key = std::path::Path::new(crate::secrets::KEY_FILE);
	send_only::new_password(&state.db, key, id).await?;
	apply_soon(state.cfg.clone(), state.db.clone(), state.events.clone());
	Ok(Json(()))
}

/// A send-only address holds no mail, it goes without a confirmation.
pub(super) async fn delete_sender(
	_authed: Authed,
	State(state): State<AppState>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<()>> {
	send_only::remove(&state.db.conn(), id)?;
	apply_soon(state.cfg.clone(), state.db.clone(), state.events.clone());
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MailboxReq {
	pub(super) local_part: String,
	pub(super) password: String,
	pub(super) quota_mb: Option<i64>,
}

pub(super) async fn create_mailbox(
	_authed: Authed,
	State(state): State<AppState>,
	UrlPath(domain_id): UrlPath<i64>,
	Json(req): Json<MailboxReq>,
) -> Result<Json<i64>> {
	domain_by_id(&state.db.conn(), domain_id)?;
	let id = add_mailbox(
		&state.db,
		domain_id,
		&req.local_part,
		req.password,
		req.quota_mb,
	)
	.await?;
	apply_soon(state.cfg.clone(), state.db.clone(), state.events.clone());
	Ok(Json(id))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateMailboxReq {
	pub(super) password: Option<String>,
	pub(super) quota_mb: Option<i64>,
}

pub(super) async fn update_mailbox(
	_authed: Authed,
	State(state): State<AppState>,
	UrlPath(id): UrlPath<i64>,
	Json(req): Json<UpdateMailboxReq>,
) -> Result<Json<()>> {
	let password = req.password.filter(|p| !p.is_empty());
	set_mailbox(&state.db, id, password, req.quota_mb).await?;
	apply_soon(state.cfg.clone(), state.db.clone(), state.events.clone());
	Ok(Json(()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ForwardReq {
	pub(super) local_part: String,
	pub(super) targets: Vec<String>,
}

pub(super) async fn create_forward(
	_authed: Authed,
	State(state): State<AppState>,
	UrlPath(domain_id): UrlPath<i64>,
	Json(req): Json<ForwardReq>,
) -> Result<Json<i64>> {
	let conn = state.db.conn();
	domain_by_id(&conn, domain_id)?;
	let id = set_forward(&conn, domain_id, &req.local_part, &req.targets)?;
	drop(conn);
	apply_soon(state.cfg.clone(), state.db.clone(), state.events.clone());
	Ok(Json(id))
}

/// Forwards hold no mail, they go without a confirmation.
pub(super) async fn delete_forward(
	_authed: Authed,
	State(state): State<AppState>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<()>> {
	let n = state
		.db
		.conn()
		.execute("DELETE FROM mail_forwards WHERE id = ?1", [id])?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	apply_soon(state.cfg.clone(), state.db.clone(), state.events.clone());
	Ok(Json(()))
}

pub(super) async fn dns_route(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<Vec<DnsRecord>>> {
	let (_, name) = domain_by_id(&db.conn(), id)?;
	Ok(Json(domain_dns(&cfg, &db, &name).await?))
}

pub fn routes() -> Router<AppState> {
	Router::new()
		.route("/mail-domains", post(create_domain))
		.route("/mail-domains/{id}/mailboxes", post(create_mailbox))
		.route("/mail-domains/{id}/forwards", post(create_forward))
		.route("/mail-domains/{id}/dns", get(dns_route))
		.route("/mail-domains/{id}/mode", post(update_mode))
		.route("/mail-domains/{id}/senders", post(create_sender))
		.route(
			"/mail-senders/{id}",
			get(sender_settings).delete(delete_sender),
		)
		.route("/mail-senders/{id}/password", post(sender_password))
		.route("/mailboxes/{id}", post(update_mailbox))
		.route("/forwards/{id}", delete(delete_forward))
		.route("/groups/{id}/smtp", get(group_smtp))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SmtpLogin {
	pub(super) host: String,
	pub(super) port: u16,
	pub(super) user: String,
	pub(super) password: String,
	/// the group's mail domains it may send as
	pub(super) domains: Vec<String>,
}

/// The group's SMTP login for its sites, None until apply made it.
pub(super) async fn group_smtp(
	_authed: Authed,
	State(cfg): State<std::sync::Arc<Config>>,
	State(db): State<Db>,
	UrlPath(id): UrlPath<i64>,
) -> Result<Json<Option<SmtpLogin>>> {
	let g = hosting::find_group(&db.conn(), id)?;
	// submission needs TLS for the mail host's name
	let host = host(&cfg).filter(|h| crate::host::has_cert(h));
	let (Some((password, _)), Some(host)) =
		(g.smtp.clone().filter(|_| !g.mail_domains.is_empty()), host)
	else {
		return Ok(Json(None));
	};
	Ok(Json(Some(SmtpLogin {
		host,
		port: 587,
		user: smtp_user(&g.name),
		password,
		domains: g.mail_domains.iter().map(|d| d.name.clone()).collect(),
	})))
}
