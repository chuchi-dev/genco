//! `genco mail`: list, DNS, senders, changes (applied right away) and
//! deletions (confirmed by mail).

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
		MailCmd::Dns { domain } => dns_text(cfg, &domain).await,
		MailCmd::DeleteDomain { name } => {
			let d = find_domain(&groups, &name)?;
			deletion(crate::actions::Kind::MailDomain, d.id).await
		}
		MailCmd::Delete { address } => {
			let b = find_mailbox(&groups, &address)?;
			deletion(crate::actions::Kind::Mailbox, b.id).await
		}
		cmd => {
			let msg = change(db, &groups, cmd, password).await?;
			let report = crate::apply::apply_mail(cfg, db).await;
			if !report.ok {
				return Err(Error::Internal(format!(
					"{msg}, but applying failed: {}",
					report.log.join("; ")
				)));
			}
			Ok(msg)
		}
	}
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
			"{} (group {})\n",
			d.name,
			group.map_or("?", |g| g.name.as_str())
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
	}
	out.trim_end().to_string()
}

/// `genco mail dns <domain>`
pub async fn dns_text(cfg: &Config, domain: &str) -> Result<String> {
	Ok(records_text(&dns_check(cfg, domain).await?))
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

/// The mail commands that change something; the caller applies them.
pub(super) async fn change(
	db: &Db,
	groups: &[Group],
	cmd: MailCmd,
	password: impl FnOnce() -> std::result::Result<String, String>,
) -> Result<String> {
	let mailbox = |address: &str| find_mailbox(groups, address);
	Ok(match cmd {
		MailCmd::AddDomain { name, group } => {
			let g = hosting::group_by_name(&db.conn(), &group)?
				.ok_or_else(|| Error::Invalid(format!("no group {group}")))?;
			add_domain(&db.conn(), g.id, &name)?;
			format!("added the mail domain {name}")
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
