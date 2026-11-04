//! Send-only addresses: an app's own SMTP login, the whole address, that
//! sends only as that address, DKIM signed. No mailbox and no IMAP: the
//! login is in Dovecot's passdb for the groups' SMTP logins, which has no
//! userdb. On a send-only mail domain (its inbox is elsewhere) or on a
//! receiving one (noreply@ without a mailbox).

use std::path::Path;

use super::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOnly {
	pub id: i64,
	pub mail_domain_id: i64,
	pub address: String,
	pub note: String,
	pub created_at: i64,
	/// for Dovecot's passwd file, never sent to the browser
	#[serde(skip)]
	pub password_hash: String,
}

pub(super) fn load(conn: &Connection) -> rusqlite::Result<Vec<SendOnly>> {
	let mut stmt = conn.prepare(
		"SELECT s.id, s.mail_domain_id, s.local_part || '@' || d.name,
		s.note, s.created_at, s.password_hash FROM mail_senders s
		JOIN mail_domains d ON d.id = s.mail_domain_id ORDER BY s.local_part",
	)?;
	stmt.query_map([], |r| {
		Ok(SendOnly {
			id: r.get(0)?,
			mail_domain_id: r.get(1)?,
			address: r.get(2)?,
			note: r.get(3)?,
			created_at: r.get(4)?,
			password_hash: r.get(5)?,
		})
	})?
	.collect()
}

/// Addresses no app gets a login for: genco's own sender and the
/// panel domain's postmaster@ and abuse@, mail signed as the panel, and
/// postmaster@ of every domain.
fn check_reserved(cfg: &Config, local: &str, domain: &str) -> Result<()> {
	let panel = cfg.domain.as_deref() == Some(domain);
	let reserved =
		local == "postmaster" || (panel && matches!(local, "genco" | "abuse"));
	if reserved {
		return Err(Error::Invalid(format!(
			"{local}@{domain} is reserved, it sends as the server itself"
		)));
	}
	Ok(())
}

/// A new send-only address with a random password. Not with mail
/// hosting off,
/// not for an address that is a mailbox or forward or reserved.
pub(super) async fn add(
	cfg: &Config,
	db: &Db,
	key_file: &Path,
	domain_id: i64,
	local: &str,
	note: &str,
) -> Result<i64> {
	let local = local.trim().to_lowercase();
	check_local_part(&local)?;
	let password = util::random_hex(16);
	let hash = hash(password.clone()).await?;

	let conn = db.conn();
	super::check_hosting(&conn, "no new send-only addresses")?;
	let domain: String = conn.query_row(
		"SELECT name FROM mail_domains WHERE id = ?1",
		[domain_id],
		|r| r.get(0),
	)?;
	check_reserved(cfg, &local, &domain)?;
	match address_in_use(&conn, domain_id, &local)? {
		Some("mailbox") => {
			return Err(Error::Invalid(format!(
				"{local}@ is a mailbox, it sends with its own login"
			)));
		}
		Some(kind) => {
			return Err(Error::Invalid(format!(
				"{local}@ is a {kind} already"
			)));
		}
		None => {}
	}

	conn.execute(
		"INSERT INTO mail_senders (mail_domain_id, local_part, password,
		password_hash, note, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
		params![
			domain_id,
			local,
			crate::secrets::seal_with(key_file, &password)?,
			hash,
			note.trim(),
			now()
		],
	)?;
	Ok(conn.last_insert_rowid())
}

/// A new random password, the old one stops working after apply.
pub(super) async fn new_password(
	db: &Db,
	key_file: &Path,
	id: i64,
) -> Result<()> {
	let password = util::random_hex(16);
	let hash = hash(password.clone()).await?;
	let sealed = crate::secrets::seal_with(key_file, &password)?;
	let n = db.conn().execute(
		"UPDATE mail_senders SET password = ?1, password_hash = ?2
		WHERE id = ?3",
		params![sealed, hash, id],
	)?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	Ok(())
}

/// Withdraws the address, its login is gone after apply.
pub(super) fn remove(conn: &Connection, id: i64) -> Result<()> {
	let n = conn.execute("DELETE FROM mail_senders WHERE id = ?1", [id])?;
	if n == 0 {
		return Err(Error::NotFound);
	}
	Ok(())
}

/// What an app needs to send as the address.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AppSettings {
	pub(super) host: String,
	/// STARTTLS
	pub(super) port: u16,
	pub(super) user: String,
	pub(super) password: String,
	pub(super) from: String,
	pub(super) zipp_smtp_url: String,
	pub(super) smtp_url: String,
	/// the mail host has its certificate, submission works
	pub(super) ready: bool,
}

/// Percent-encodes all but the unreserved characters of RFC 3986, for
/// the user and password in a URL.
fn url_encode(s: &str) -> String {
	let mut out = String::new();
	for b in s.bytes() {
		if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
			out.push(b as char);
		} else {
			out.push_str(&format!("%{b:02X}"));
		}
	}
	out
}

/// `smtp://user:password@host:587`, STARTTLS on 587.
fn smtp_url(user: &str, password: &str, host: &str) -> String {
	format!(
		"smtp://{}:{}@{host}:587",
		url_encode(user),
		url_encode(password)
	)
}

pub(super) fn app_settings(
	cfg: &Config,
	conn: &Connection,
	key_file: &Path,
	id: i64,
) -> Result<AppSettings> {
	let (address, blob): (String, Vec<u8>) = conn.query_row(
		"SELECT s.local_part || '@' || d.name, s.password FROM mail_senders s
		JOIN mail_domains d ON d.id = s.mail_domain_id WHERE s.id = ?1",
		[id],
		|r| Ok((r.get(0)?, r.get(1)?)),
	)?;
	let host =
		host(cfg).ok_or(Error::Invalid("the panel has no domain".into()))?;
	let password = crate::secrets::open_with(key_file, &blob)?;
	let url = smtp_url(&address, &password, &host);
	Ok(AppSettings {
		ready: crate::host::has_cert(&host),
		zipp_smtp_url: format!("{url}?tls=required"),
		smtp_url: url,
		host,
		port: 587,
		user: address.clone(),
		password,
		from: address,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	fn key_file() -> std::path::PathBuf {
		std::env::temp_dir()
			.join(format!("genco-key-{}", rand::random::<u32>()))
	}

	#[test]
	fn smtp_urls_are_encoded() {
		assert_eq!(url_encode("noreply@textrem.ch"), "noreply%40textrem.ch");
		assert_eq!(url_encode("a b/c:d?e#f%"), "a%20b%2Fc%3Ad%3Fe%23f%25");
		assert_eq!(url_encode("x-y_z.~9"), "x-y_z.~9");
		assert_eq!(
			smtp_url("noreply@textrem.ch", "p@ss:w", "mail.x.ch"),
			"smtp://noreply%40textrem.ch:p%40ss%3Aw@mail.x.ch:587"
		);
	}

	#[tokio::test]
	async fn addresses_are_unique_and_never_a_mailbox_or_forward() {
		let cfg: Config =
			toml::from_str("domain = \"panel.example.com\"").unwrap();
		let db = Db::memory();
		let key = key_file();
		super::super::tests::group_with_domains(&db.conn());
		set_forward(&db.conn(), 1, "info", &["a@b.ch".into()]).unwrap();
		add_mailbox(&db, 1, "anna", "long enough pw".into(), None)
			.await
			.unwrap();

		let refused = add(&cfg, &db, &key, 1, "info", "").await.unwrap_err();
		assert!(refused.to_string().contains("forward"), "{refused}");
		let refused = add(&cfg, &db, &key, 1, "anna", "").await.unwrap_err();
		assert!(refused.to_string().contains("mailbox"), "{refused}");

		// on a receiving domain and a send-only one
		add(&cfg, &db, &key, 1, "noreply", "").await.unwrap();
		let id = add(&cfg, &db, &key, 2, "NoReply ", "the shop")
			.await
			.unwrap();
		let refused = add(&cfg, &db, &key, 2, "noreply", "").await.unwrap_err();
		assert!(refused.to_string().contains("already"), "{refused}");

		// and the other way round
		let refused = set_forward(&db.conn(), 1, "noreply", &["a@b.ch".into()])
			.unwrap_err();
		assert!(refused.to_string().contains("send-only"), "{refused}");
		let refused =
			add_mailbox(&db, 1, "noreply", "long enough pw".into(), None)
				.await
				.unwrap_err();
		assert!(refused.to_string().contains("send-only"), "{refused}");

		let s = app_settings(&cfg, &db.conn(), &key, id).unwrap();
		assert_eq!(s.user, "noreply@textrem.ch");
		assert_eq!(s.host, "mail.panel.example.com");
		assert_eq!(s.password.len(), 32);
		assert_eq!(
			s.zipp_smtp_url,
			format!(
				"smtp://noreply%40textrem.ch:{}@mail.panel.example.com:587\
				?tls=required",
				s.password
			)
		);

		// a new password, the old one is gone
		new_password(&db, &key, id).await.unwrap();
		let again = app_settings(&cfg, &db.conn(), &key, id).unwrap();
		assert_ne!(again.password, s.password);
		remove(&db.conn(), id).unwrap();
		assert!(app_settings(&cfg, &db.conn(), &key, id).is_err());
		let _ = std::fs::remove_file(key);
	}

	#[tokio::test]
	async fn no_send_only_address_without_hosting() {
		let cfg: Config = toml::from_str("").unwrap();
		let db = Db::memory();
		super::super::tests::group_with_domains(&db.conn());
		crate::db::set_setting(&db.conn(), "mail", r#"{"mode":"off"}"#)
			.unwrap();
		let refused = add(&cfg, &db, &key_file(), 2, "noreply", "")
			.await
			.unwrap_err();
		assert!(
			refused.to_string().contains("mail hosting is off"),
			"{refused}"
		);
	}

	#[tokio::test]
	async fn the_servers_own_addresses_are_reserved() {
		let cfg: Config = toml::from_str("domain = \"shop.ch\"").unwrap();
		let db = Db::memory();
		let key = key_file();
		super::super::tests::group_with_domains(&db.conn());
		// shop.ch is the panel's domain here
		for local in ["genco", "postmaster", "abuse"] {
			let refused = add(&cfg, &db, &key, 1, local, "").await.unwrap_err();
			assert!(refused.to_string().contains("reserved"), "{refused}");
		}
		let refused =
			add(&cfg, &db, &key, 2, "postmaster", "").await.unwrap_err();
		assert!(refused.to_string().contains("reserved"), "{refused}");
		add(&cfg, &db, &key, 2, "genco", "").await.unwrap();
		add(&cfg, &db, &key, 2, "abuse", "").await.unwrap();
		let _ = std::fs::remove_file(key);
	}
}
