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

/// A mailbox or forward can't have the address of a send-only login.
pub(super) fn taken_by_sender(
	conn: &Connection,
	domain_id: i64,
	local: &str,
) -> Result<()> {
	let taken: bool = conn.query_row(
		"SELECT EXISTS (SELECT 1 FROM mail_senders
		WHERE mail_domain_id = ?1 AND local_part = ?2)",
		params![domain_id, local],
		|r| r.get(0),
	)?;
	if taken {
		return Err(Error::Invalid(format!(
			"{local}@ is a send-only address already"
		)));
	}
	Ok(())
}

/// A new send-only address with a random password. Not with mail off,
/// not for an address that is a mailbox or forward.
pub(super) async fn add(
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
	if crate::mail::is_off(&conn) {
		return Err(crate::mail::off_error(
			"a send-only address can't send; turn mail on first",
		));
	}
	let taken: (bool, bool) = conn.query_row(
		"SELECT
		EXISTS (SELECT 1 FROM mailboxes
			WHERE mail_domain_id = ?1 AND local_part = ?2),
		EXISTS (SELECT 1 FROM mail_forwards
			WHERE mail_domain_id = ?1 AND local_part = ?2)",
		params![domain_id, local],
		|r| Ok((r.get(0)?, r.get(1)?)),
	)?;
	match taken {
		(true, _) => {
			return Err(Error::Invalid(format!(
				"{local}@ is a mailbox, it sends with its own login"
			)));
		}
		(_, true) => {
			return Err(Error::Invalid(format!("{local}@ is a forward")));
		}
		_ => {}
	}
	taken_by_sender(&conn, domain_id, &local)?;

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

/// The app settings for the terminal.
fn settings_text(s: &AppSettings) -> String {
	let mut out = format!(
		"Host:     {}\nPort:     {} (STARTTLS)\nUser:     {}\nPassword: {}\n\
		From:     {}\n\nZIPP_SMTP_URL={}\nSMTP_URL={}\n\
		Craft: transport SMTP, host {}, port {}, encryption tls, user {}, \
		the password above\n",
		s.host,
		s.port,
		s.user,
		s.password,
		s.from,
		s.zipp_smtp_url,
		s.smtp_url,
		s.host,
		s.port,
		s.user,
	);
	if !s.ready {
		out.push_str(
			"\nThe mail host has no certificate yet, sending works once \
			genco apply got it.\n",
		);
	}
	out
}

#[derive(Debug, clap::Parser)]
pub enum SenderCmd {
	/// Add a send-only address for an app and print its SMTP settings
	Add {
		address: String,
		/// what the address is for
		#[clap(long, default_value = "")]
		note: String,
	},
	/// List the send-only addresses
	List,
	/// Print the SMTP settings of a send-only address, with its password
	Show { address: String },
	/// Give a send-only address a new password, the old one stops working
	Password { address: String },
	/// Remove a send-only address, its login stops working
	Remove { address: String },
}

fn find(groups: &[Group], address: &str) -> Result<SendOnly> {
	let address = address.trim().to_lowercase();
	domains(groups)
		.flat_map(|d| &d.senders)
		.find(|s| s.address == address)
		.cloned()
		.ok_or_else(|| {
			Error::Invalid(format!("no send-only address {address}"))
		})
}

/// `genco mail sender`: the message, and whether the caller applies.
pub(super) async fn cli(
	cfg: &Config,
	db: &Db,
	groups: &[Group],
	cmd: SenderCmd,
) -> Result<(String, bool)> {
	let key_file = Path::new(crate::secrets::KEY_FILE);
	let settings = |id| {
		app_settings(cfg, &db.conn(), key_file, id).map(|s| settings_text(&s))
	};
	Ok(match cmd {
		SenderCmd::List => {
			let mut out = String::new();
			for d in domains(groups) {
				for s in &d.senders {
					out.push_str(&format!("{}  {}\n", s.address, s.note));
				}
			}
			(out.trim_end().to_string(), false)
		}
		SenderCmd::Show { address } => {
			(settings(find(groups, &address)?.id)?, false)
		}
		SenderCmd::Add { address, note } => {
			let address = address.trim().to_lowercase();
			let (local, domain) = cli::split(&address)?;
			let d = cli::find_domain(groups, domain)?;
			let id = add(db, key_file, d.id, local, &note).await?;
			(
				format!(
					"added the send-only address {address}; its settings, \
					shown again with genco mail sender show:\n\n{}",
					settings(id)?
				),
				true,
			)
		}
		SenderCmd::Password { address } => {
			let s = find(groups, &address)?;
			new_password(db, key_file, s.id).await?;
			(
				format!(
					"new password for {}, the old one stops working:\n\n{}",
					s.address,
					settings(s.id)?
				),
				true,
			)
		}
		SenderCmd::Remove { address } => {
			let s = find(groups, &address)?;
			remove(&db.conn(), s.id)?;
			(format!("removed the send-only address {}", s.address), true)
		}
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
		let db = Db::memory();
		let key = key_file();
		super::super::tests::group_with_domains(&db.conn());
		set_forward(&db.conn(), 1, "info", &["a@b.ch".into()]).unwrap();
		add_mailbox(&db, 1, "anna", "long enough pw".into(), None)
			.await
			.unwrap();

		let refused = add(&db, &key, 1, "info", "").await.unwrap_err();
		assert!(refused.to_string().contains("forward"), "{refused}");
		let refused = add(&db, &key, 1, "anna", "").await.unwrap_err();
		assert!(refused.to_string().contains("mailbox"), "{refused}");

		// on a receiving domain and a send-only one
		add(&db, &key, 1, "noreply", "").await.unwrap();
		let id = add(&db, &key, 2, "NoReply ", "the shop").await.unwrap();
		let refused = add(&db, &key, 2, "noreply", "").await.unwrap_err();
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

		let cfg: Config =
			toml::from_str("domain = \"panel.example.com\"").unwrap();
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
	async fn no_send_only_address_with_mail_off() {
		let db = Db::memory();
		super::super::tests::group_with_domains(&db.conn());
		crate::db::set_setting(&db.conn(), "mail", r#"{"mode":"off"}"#)
			.unwrap();
		let refused =
			add(&db, &key_file(), 2, "noreply", "").await.unwrap_err();
		assert!(refused.to_string().contains("mail is off"), "{refused}");
	}
}
