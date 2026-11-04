//! `genco mail`: list, DNS, senders, the test mail, send-only
//! addresses, changes (applied right away) and deletions (confirmed by
//! mail).

use super::*;

#[derive(Debug, clap::Parser)]
pub enum MailCmd {
	/// List the mail domains with their mailboxes and forwards
	List,
	/// Add a mail domain to a group
	AddDomain {
		name: String,
		#[clap(long)]
		group: String,
		/// the inbox stays elsewhere, apps here only send from it
		#[clap(long)]
		send_only: bool,
	},
	/// Make a mail domain send-only (its inbox is elsewhere, only without
	/// mailboxes and forwards) or receiving again
	Set {
		name: String,
		#[clap(
			long,
			conflicts_with = "receive",
			required_unless_present = "receive"
		)]
		send_only: bool,
		#[clap(long)]
		receive: bool,
	},
	/// Ask the admins by mail to confirm deleting a mail domain and its mail
	DeleteDomain { name: String },
	/// Add a mailbox, the password is read from stdin
	Add {
		address: String,
		/// in MB
		#[clap(long)]
		quota: Option<i64>,
	},
	/// Set the password of a mailbox, read from stdin
	Password { address: String },
	/// Set the quota of a mailbox in MB
	Quota { address: String, mb: i64 },
	/// Ask the admins by mail to confirm deleting a mailbox and its mail
	Delete { address: String },
	/// Forward an address (`*@domain` for every other address) to others
	Forward {
		address: String,
		#[clap(required = true)]
		to: Vec<String>,
	},
	/// Remove a forward
	Unforward { address: String },
	/// The DNS records a mail domain needs and what its DNS has now
	Dns { domain: String },
	/// Which sender domains this server may send mail as (SPF), for an
	/// app's MAIL_FROM
	Senders,
	/// Mail per address in the last 30 days: received or forwarded per
	/// mailbox and forward, sent per send-only address and group login,
	/// unknown addresses that were rejected
	Stats { domain: Option<String> },
	/// Send genco's test mail (Settings → Mail) and wait for postfix:
	/// fails when it is deferred or bounced, with postfix's reason
	Test { to: String },
	/// Send-only addresses: an app's own SMTP login that sends only as
	/// that address, DKIM signed, no mailbox
	#[clap(subcommand)]
	SendOnly(SendOnlyCmd),
}

#[derive(Debug, clap::Parser)]
pub enum SendOnlyCmd {
	/// Add a send-only address for an app and print its SMTP settings
	Add {
		address: String,
		/// what the address is for
		#[clap(long, default_value = "")]
		note: String,
		#[clap(flatten)]
		out: SettingsOut,
	},
	/// List the send-only addresses
	List,
	/// Print the SMTP settings of a send-only address
	Show {
		address: String,
		#[clap(flatten)]
		out: SettingsOut,
	},
	/// Give a send-only address a new password, the old one stops working
	Password {
		address: String,
		#[clap(flatten)]
		out: SettingsOut,
	},
	/// Remove a send-only address, its login stops working
	Remove { address: String },
}

/// Where the password goes: into an app's environment, never through
/// the terminal unless asked.
#[derive(Debug, clap::Args)]
pub struct SettingsOut {
	/// write the SMTP URL with the password into the environment of an
	/// app as a secret: the group's only node or rust app, or
	/// <group>/<domain>
	#[clap(long, value_name = "GROUP[/DOMAIN]")]
	env: Option<String>,
	/// the variable of --env; ZIPP_SMTP_URL gets zipp's URL
	/// (?tls=required), another name the plain smtp:// URL
	#[clap(long, default_value = "ZIPP_SMTP_URL")]
	name: String,
	/// print the password and the URLs with it
	#[clap(long)]
	show_password: bool,
}

pub(super) fn find_domain(groups: &[Group], name: &str) -> Result<MailDomain> {
	let name = name.trim().to_lowercase();
	domains(groups)
		.find(|d| d.name == name)
		.cloned()
		.ok_or_else(|| Error::Invalid(format!("no mail domain {name}")))
}

pub(super) fn split(address: &str) -> Result<(&str, &str)> {
	address
		.rsplit_once('@')
		.ok_or_else(|| Error::Invalid(format!("{address} is no address")))
}

/// Runs a mail command; changes apply right away, the rest only reads or
/// asks the admins to confirm a deletion.
pub async fn cli(
	cfg: &Config,
	db: &Db,
	cmd: MailCmd,
	password: impl FnOnce() -> std::result::Result<String, String>,
) -> Result<String> {
	let groups = hosting::groups(&db.conn())?;
	let deletion = |kind, id| async move {
		crate::actions::request(cfg, db, kind, id, crate::activity::cli_actor())
			.await
			.map(|r| r.message)
	};
	match cmd {
		MailCmd::Senders => Ok(crate::senders::cli(cfg, db).await),
		MailCmd::List => Ok(list_text(&groups)),
		MailCmd::Test { to } => {
			let by =
				format!("`genco mail test` ({})", crate::activity::cli_actor());
			crate::mail::test(cfg, db, &to, &by).await
		}
		MailCmd::Dns { domain } => dns_text(cfg, db, &domain).await,
		MailCmd::Stats { domain } => {
			let stats = crate::stats::StatsDb::open(&crate::stats::path(cfg))
				.map_err(Error::Internal)?;
			counts::stats_text(&stats, &groups, domain.as_deref()).await
		}
		MailCmd::DeleteDomain { name } => {
			let d = find_domain(&groups, &name)?;
			deletion(crate::actions::Kind::MailDomain, d.id).await
		}
		MailCmd::Delete { address } => {
			let b = find_mailbox(&groups, &address)?;
			deletion(crate::actions::Kind::Mailbox, b.id).await
		}
		MailCmd::SendOnly(cmd) => {
			let key = crate::secrets::key_file();
			let (msg, changed) =
				send_only_cli(cfg, db, key, &groups, cmd).await?;
			if changed {
				apply(cfg, db, &msg).await?;
			}
			Ok(msg)
		}
		cmd => {
			let msg = change(db, &groups, cmd, password).await?;
			apply(cfg, db, &msg).await?;
			Ok(msg)
		}
	}
}

/// Applies mail after `msg`'s change.
async fn apply(cfg: &Config, db: &Db, msg: &str) -> Result<()> {
	let report = crate::apply::apply_mail(cfg, db).await;
	if !report.ok {
		return Err(Error::Internal(format!(
			"{msg}, but applying failed: {}",
			report.log.join("; ")
		)));
	}
	Ok(())
}

pub(super) fn find_mailbox(groups: &[Group], address: &str) -> Result<Mailbox> {
	domains(groups)
		.flat_map(|d| &d.mailboxes)
		.find(|b| b.address == address)
		.cloned()
		.ok_or_else(|| Error::Invalid(format!("no mailbox {address}")))
}

/// `genco mail list`
pub(super) fn list_text(groups: &[Group]) -> String {
	let mut out = String::new();
	for d in domains(groups) {
		let group = groups.iter().find(|g| g.id == d.group_id);
		out.push_str(&format!(
			"{} (group {}{})\n",
			d.name,
			group.map_or("?", |g| g.name.as_str()),
			if d.send_only { ", send-only" } else { "" }
		));
		for b in &d.mailboxes {
			out.push_str(&format!("  {}  {} MB\n", b.address, b.quota_mb));
		}
		for f in &d.forwards {
			out.push_str(&format!(
				"  {} -> {}\n",
				f.address,
				f.targets.join(", ")
			));
		}
		for s in &d.send_only_addresses {
			out.push_str(&format!("  {}  send-only {}\n", s.address, s.note));
		}
	}
	out.trim_end().to_string()
}

/// `genco mail dns <domain>`
pub async fn dns_text(cfg: &Config, db: &Db, domain: &str) -> Result<String> {
	Ok(records_text(&domain_dns(cfg, db, domain).await?))
}

/// DNS records for the terminal: what to enter, what is there now.
pub fn records_text(records: &[DnsRecord]) -> String {
	let mut out = String::new();
	for r in records {
		out.push_str(&format!(
			"{:8} {} {} {}\n  found: {}\n  {}\n",
			r.status,
			r.name,
			r.kind,
			r.value,
			if r.found.is_empty() {
				"nothing".to_string()
			} else {
				r.found.join(" | ")
			},
			r.note
		));
	}
	out.trim_end().to_string()
}

fn find_send_only(groups: &[Group], address: &str) -> Result<SendOnly> {
	let address = address.trim().to_lowercase();
	domains(groups)
		.flat_map(|d| &d.send_only_addresses)
		.find(|s| s.address == address)
		.cloned()
		.ok_or_else(|| {
			Error::Invalid(format!("no send-only address {address}"))
		})
}

/// The app settings for the terminal, the password and the URLs with it
/// only with `show_password`.
fn settings_text(s: &send_only::AppSettings, show_password: bool) -> String {
	let password = if show_password {
		s.password.as_str()
	} else {
		"(hidden, --show-password to print)"
	};
	let mut out = format!(
		"Host:     {}\nPort:     {} (STARTTLS)\nUser:     {}\nPassword: {}\n\
		From:     {}\n\n",
		s.host, s.port, s.user, password, s.from,
	);
	if show_password {
		out.push_str(&format!(
			"ZIPP_SMTP_URL={}\nSMTP_URL={}\n",
			s.zipp_smtp_url, s.smtp_url
		));
	} else {
		out.push_str(
			"ZIPP_SMTP_URL and SMTP_URL hold the password: --env \
			<group>[/<domain>] writes one into the app's environment\n",
		);
	}
	out.push_str(&format!(
		"Craft: transport SMTP, host {}, port {}, encryption tls, user {}, \
		the password\n",
		s.host, s.port, s.user,
	));
	if !s.ready {
		out.push_str(
			"\nThe mail host has no certificate yet, sending works once \
			genco apply got it.\n",
		);
	}
	out
}

/// The node or rust app of `<group>[/<domain>]`.
fn app_domain(conn: &Connection, target: &str) -> Result<hosting::Domain> {
	let (group, domain) = match target.split_once('/') {
		Some((g, d)) => (g, Some(d)),
		None => (target, None),
	};
	let g = hosting::group_by_name(conn, group)?
		.ok_or_else(|| Error::Invalid(format!("no group {group}")))?;
	let apps: Vec<&hosting::Domain> = g
		.domains
		.iter()
		.filter(|d| matches!(d.kind, hosting::Kind::Node | hosting::Kind::Rust))
		.collect();
	let names = || {
		let names: Vec<&str> = apps.iter().map(|d| d.name.as_str()).collect();
		names.join(", ")
	};
	match domain {
		Some(name) => apps.iter().find(|d| d.name == name).ok_or_else(|| {
			Error::Invalid(format!(
				"{name} is no node or rust app of {group} (apps: {})",
				names()
			))
		}),
		None if apps.len() == 1 => Ok(&apps[0]),
		None if apps.is_empty() => Err(Error::Invalid(format!(
			"{group} has no node or rust app; other apps get the \
			settings in the panel"
		))),
		None => Err(Error::Invalid(format!(
			"{group} has several apps, name one: --env {group}/<domain> \
			({})",
			names()
		))),
	}
	.map(|d| (*d).clone())
}

/// Prints the settings of `id` and writes the URL into an app's
/// environment if asked.
fn settings_out(
	cfg: &Config,
	db: &Db,
	key_file: &std::path::Path,
	id: i64,
	out: SettingsOut,
) -> Result<String> {
	let conn = db.conn();
	let s = send_only::app_settings(cfg, &conn, key_file, id)?;
	let mut text = settings_text(&s, out.show_password);
	if let Some(target) = &out.env {
		let d = app_domain(&conn, target)?;
		let url = if out.name == "ZIPP_SMTP_URL" {
			&s.zipp_smtp_url
		} else {
			&s.smtp_url
		};
		crate::appenv::set_secret(&conn, d.id, &out.name, url)?;
		text.push_str(&format!(
			"\nwrote {} (secret) into the environment of {}, run genco \
			apply (the app restarts)\n",
			out.name, d.name
		));
	}
	Ok(text)
}

/// `genco mail send-only`: the message, and whether the caller applies.
async fn send_only_cli(
	cfg: &Config,
	db: &Db,
	key_file: &std::path::Path,
	groups: &[Group],
	cmd: SendOnlyCmd,
) -> Result<(String, bool)> {
	Ok(match cmd {
		SendOnlyCmd::List => {
			let mut out = String::new();
			for d in domains(groups) {
				for s in &d.send_only_addresses {
					out.push_str(&format!("{}  {}\n", s.address, s.note));
				}
			}
			(out.trim_end().to_string(), false)
		}
		SendOnlyCmd::Show { address, out } => {
			let s = find_send_only(groups, &address)?;
			(settings_out(cfg, db, key_file, s.id, out)?, false)
		}
		SendOnlyCmd::Add { address, note, out } => {
			// a wrong app fails before the change
			if let Some(target) = &out.env {
				app_domain(&db.conn(), target)?;
			}
			let address = address.trim().to_lowercase();
			let (local, domain) = split(&address)?;
			let d = find_domain(groups, domain)?;
			let id =
				send_only::add(cfg, db, key_file, d.id, local, &note).await?;
			let settings = settings_out(cfg, db, key_file, id, out)?;
			(
				format!(
					"added the send-only address {address}; its settings, \
					shown again with genco mail send-only show:\n\n\
					{settings}"
				),
				true,
			)
		}
		SendOnlyCmd::Password { address, out } => {
			let s = find_send_only(groups, &address)?;
			if let Some(target) = &out.env {
				app_domain(&db.conn(), target)?;
			}
			send_only::new_password(db, key_file, s.id).await?;
			let settings = settings_out(cfg, db, key_file, s.id, out)?;
			(
				format!(
					"new password for {}, the old one stops working:\n\n\
					{settings}",
					s.address,
				),
				true,
			)
		}
		SendOnlyCmd::Remove { address } => {
			let s = find_send_only(groups, &address)?;
			send_only::remove(&db.conn(), s.id)?;
			(format!("removed the send-only address {}", s.address), true)
		}
	})
}

/// The mail commands that change something; the caller applies them.
pub(super) async fn change(
	db: &Db,
	groups: &[Group],
	cmd: MailCmd,
	password: impl FnOnce() -> std::result::Result<String, String>,
) -> Result<String> {
	let mailbox = |address: &str| find_mailbox(groups, address);
	Ok(match cmd {
		MailCmd::AddDomain {
			name,
			group,
			send_only,
		} => {
			let g = hosting::group_by_name(&db.conn(), &group)?
				.ok_or_else(|| Error::Invalid(format!("no group {group}")))?;
			add_domain(&db.conn(), g.id, &name, send_only)?;
			if send_only {
				format!(
					"added the send-only mail domain {name}; add an address \
					with genco mail send-only add noreply@{name}"
				)
			} else {
				format!("added the mail domain {name}")
			}
		}
		MailCmd::Set {
			name, send_only, ..
		} => {
			let d = find_domain(groups, &name)?;
			set_mode(&db.conn(), d.id, send_only)?;
			if send_only {
				format!("{} is send-only, its inbox is elsewhere", d.name)
			} else {
				format!("{} receives mail here", d.name)
			}
		}
		MailCmd::Add { address, quota } => {
			let (local, domain) = split(&address)?;
			let d = find_domain(groups, domain)?;
			let password = password().map_err(Error::Invalid)?;
			add_mailbox(db, d.id, local, password, quota).await?;
			format!("added the mailbox {address}")
		}
		MailCmd::Password { address } => {
			let b = mailbox(&address)?;
			let password = password().map_err(Error::Invalid)?;
			set_mailbox(db, b.id, Some(password), None).await?;
			"password changed".into()
		}
		MailCmd::Quota { address, mb } => {
			let b = mailbox(&address)?;
			set_mailbox(db, b.id, None, Some(mb)).await?;
			format!("{address} may use {mb} MB")
		}
		MailCmd::Forward { address, to } => {
			let (local, domain) = split(&address)?;
			let d = find_domain(groups, domain)?;
			set_forward(&db.conn(), d.id, local, &to)?;
			format!("{address} goes to {}", to.join(", "))
		}
		MailCmd::Unforward { address } => {
			let address = address.trim().to_lowercase();
			let (local, domain) = split(&address)?;
			let d = find_domain(groups, domain)?;
			let n = db.conn().execute(
				"DELETE FROM mail_forwards WHERE mail_domain_id = ?1
				AND local_part = ?2",
				params![d.id, local],
			)?;
			if n == 0 {
				return Err(Error::Invalid(format!("no forward {address}")));
			}
			format!("removed the forward {address}")
		}
		_ => unreachable!("cli handles the rest"),
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use clap::Parser as _;

	fn parse(args: &[&str]) -> std::result::Result<MailCmd, clap::Error> {
		MailCmd::try_parse_from(
			std::iter::once("mail").chain(args.iter().copied()),
		)
	}

	#[test]
	fn the_mode_is_one_or_the_other() {
		assert!(matches!(
			parse(&["set", "x.ch", "--send-only"]).unwrap(),
			MailCmd::Set {
				send_only: true,
				..
			}
		));
		assert!(matches!(
			parse(&["set", "x.ch", "--receive"]).unwrap(),
			MailCmd::Set {
				send_only: false,
				..
			}
		));
		assert!(parse(&["set", "x.ch"]).is_err());
		assert!(parse(&["set", "x.ch", "--send-only", "--receive"]).is_err());
		assert!(matches!(
			parse(&["send-only", "add", "noreply@x.ch", "--note", "shop"])
				.unwrap(),
			MailCmd::SendOnly(SendOnlyCmd::Add { .. })
		));
	}

	#[tokio::test]
	async fn send_only_domains_from_the_terminal() {
		let db = Db::memory();
		db.conn()
			.execute_batch(
				"INSERT INTO clients (name, created_at) VALUES ('c', 0);
				INSERT INTO domain_groups (name, client_id, created_at)
					VALUES ('shop', 1, 0);",
			)
			.unwrap();
		let run = async |args: &[&str]| {
			let groups = hosting::groups(&db.conn()).unwrap();
			change(&db, &groups, parse(args).unwrap(), || Err("no".into()))
				.await
		};
		let msg = run(&[
			"add-domain",
			"textrem.ch",
			"--group",
			"shop",
			"--send-only",
		])
		.await
		.unwrap();
		assert!(msg.contains("send-only"), "{msg}");
		let refused = run(&["forward", "info@textrem.ch", "a@b.ch"])
			.await
			.unwrap_err();
		assert!(refused.to_string().contains("send-only"), "{refused}");

		run(&["set", "textrem.ch", "--receive"]).await.unwrap();
		run(&["forward", "info@textrem.ch", "a@b.ch"])
			.await
			.unwrap();
		let refused = run(&["set", "textrem.ch", "--send-only"])
			.await
			.unwrap_err();
		assert!(refused.to_string().contains("forwards"), "{refused}");
		let list = list_text(&hosting::groups(&db.conn()).unwrap());
		assert_eq!(
			list,
			"textrem.ch (group shop)\n  info@textrem.ch -> a@b.ch"
		);
	}

	#[tokio::test]
	async fn send_only_passwords_go_into_the_env_not_the_terminal() {
		let db = Db::memory();
		let key = std::env::temp_dir()
			.join(format!("genco-key-{}", rand::random::<u32>()));
		let cfg: Config =
			toml::from_str("domain = \"panel.example.com\"").unwrap();
		super::super::tests::group_with_domains(&db.conn());
		db.conn()
			.execute_batch(
				"INSERT INTO domains (group_id, name, kind, root, created_at)
					VALUES (1, 'textrem.ch', 'rust', 'current', 0);
				INSERT INTO domains (group_id, name, kind, root, created_at)
					VALUES (1, 'shop.ch', 'php', 'public', 0);",
			)
			.unwrap();
		let run = async |args: &[&str]| {
			let groups = hosting::groups(&db.conn()).unwrap();
			let MailCmd::SendOnly(cmd) = parse(args).unwrap() else {
				panic!("{args:?}");
			};
			send_only_cli(&cfg, &db, &key, &groups, cmd).await
		};
		let env = || {
			let raw: String = db
				.conn()
				.query_row("SELECT env FROM domains WHERE id = 1", [], |r| {
					r.get(0)
				})
				.unwrap();
			crate::appenv::parse(&raw)
		};

		let (msg, changed) = run(&["send-only", "add", "noreply@textrem.ch"])
			.await
			.unwrap();
		assert!(changed);
		assert!(msg.contains("(hidden, --show-password to print)"), "{msg}");
		assert!(!msg.contains("smtp://"), "{msg}");

		let (msg, _) =
			run(&["send-only", "show", "noreply@textrem.ch", "--env", "shop"])
				.await
				.unwrap();
		assert!(msg.contains("wrote ZIPP_SMTP_URL (secret)"), "{msg}");
		assert!(msg.contains("textrem.ch, run genco apply"), "{msg}");
		let vars = env();
		assert_eq!(vars.len(), 1);
		assert_eq!(vars[0].name, "ZIPP_SMTP_URL");
		assert!(vars[0].secret);
		let password = vars[0]
			.value
			.strip_prefix("smtp://noreply%40textrem.ch:")
			.and_then(|v| {
				v.strip_suffix("@mail.panel.example.com:587?tls=required")
			})
			.unwrap()
			.to_string();
		assert!(!msg.contains(&password));

		// a new password replaces it in place, another name the plain URL
		run(&[
			"send-only",
			"password",
			"noreply@textrem.ch",
			"--env",
			"shop/textrem.ch",
		])
		.await
		.unwrap();
		run(&[
			"send-only",
			"show",
			"noreply@textrem.ch",
			"--env",
			"shop",
			"--name",
			"SMTP_URL",
		])
		.await
		.unwrap();
		let vars = env();
		assert_eq!(vars.len(), 2);
		assert!(!vars[0].value.contains(&password));
		assert!(vars[1].value.ends_with("@mail.panel.example.com:587"));

		// only a node or rust app
		let refused = run(&[
			"send-only",
			"show",
			"noreply@textrem.ch",
			"--env",
			"shop/shop.ch",
		])
		.await
		.unwrap_err();
		assert!(refused.to_string().contains("no node or rust app"));
		let refused =
			run(&["send-only", "add", "orders@textrem.ch", "--env", "nogroup"])
				.await
				.unwrap_err();
		assert!(refused.to_string().contains("no group"), "{refused}");
		let groups = hosting::groups(&db.conn()).unwrap();
		assert!(find_send_only(&groups, "orders@textrem.ch").is_err());

		let (msg, _) = run(&[
			"send-only",
			"show",
			"noreply@textrem.ch",
			"--show-password",
		])
		.await
		.unwrap();
		assert!(msg.contains("ZIPP_SMTP_URL=smtp://"), "{msg}");
		let _ = std::fs::remove_file(key);
	}
}
