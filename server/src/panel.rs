//! `genco panel rename|forget-old`: the panel gets another name, the old
//! one redirects to it (its API keeps answering for the runners) until
//! it is forgotten.

use std::path::Path;

use rusqlite::Connection;

use crate::config::Config;
use crate::db::{Db, get_json, set_setting};
use crate::hosting::{self, Group};
use crate::mailbox;

/// The panel's old names, JSON
const OLD_NAMES: &str = "panel:old-names";

#[derive(Debug, clap::Subcommand)]
pub enum PanelCmd {
	/// Give the panel a new name: config, postfix, nginx and certificate,
	/// then genco restarts. The old name redirects to the new one, its
	/// /api/ keeps working for runners, until `genco panel forget-old`.
	Rename {
		domain: String,
		/// also when the name doesn't point to this server yet
		#[clap(long)]
		force: bool,
		/// with mail domains: IMAP, SMTP and the webmail stay on
		/// mail.<old name> instead of moving to mail.<new name>
		#[clap(long)]
		keep_mail_host: bool,
	},
	/// Stop answering on an old name of the panel and remove its
	/// certificate
	ForgetOld { domain: String },
}

/// The old names that still redirect to the panel.
pub fn old_names(conn: &Connection) -> Vec<String> {
	get_json(conn, OLD_NAMES).unwrap_or_default()
}

fn set_old_names(conn: &Connection, names: &[String]) -> Result<(), String> {
	let json = serde_json::to_string(names).unwrap();
	set_setting(conn, OLD_NAMES, &json).map_err(|e| e.to_string())
}

fn step(text: &str) {
	println!("==> {text}");
}

pub async fn main(
	config_path: &Path,
	cfg: Config,
	cmd: PanelCmd,
) -> Result<(), String> {
	if !crate::util::is_root().await {
		return Err("genco panel needs root".into());
	}
	let db = Db::open(&cfg.db_path())?;
	match cmd {
		PanelCmd::Rename {
			domain,
			force,
			keep_mail_host,
		} => rename(config_path, cfg, &db, &domain, force, keep_mail_host).await,
		PanelCmd::ForgetOld { domain } => forget_old(&cfg, &db, &domain).await,
	}
}

async fn rename(
	config_path: &Path,
	cfg: Config,
	db: &Db,
	new: &str,
	force: bool,
	keep_mail_host: bool,
) -> Result<(), String> {
	let new = new.trim().trim_end_matches('.').to_lowercase();
	let old = cfg
		.domain
		.clone()
		.ok_or("the panel has no name yet: genco setup --domain <name>")?;
	if new == old {
		return Err(format!("the panel is called {old} already"));
	}
	hosting::check_domain_name(&new).map_err(|e| e.to_string())?;
	let groups = hosting::groups(&db.conn()).map_err(|e| e.to_string())?;
	if let Some(what) = taken(&cfg, &groups, &new) {
		return Err(format!("{new} is {what}"));
	}

	step(&format!("checking that {new} points to this server"));
	points_here(&new, force).await?;

	// mail.<panel> moves along unless it is set in the config
	let mail = mailbox::enabled(&groups) && cfg.mail_host.is_none();
	let old_mail = format!("mail.{old}");
	let new_mail = format!("mail.{new}");
	if mail && !keep_mail_host {
		step(&format!("checking that {new_mail} points to this server"));
		points_here(&new_mail, force).await.map_err(|e| {
			format!("{e}; or keep the mail host with --keep-mail-host")
		})?;
		println!(
			"warning: the mail host moves from {old_mail} to {new_mail}:\n  \
			- mail programs have to use {new_mail} for IMAP and SMTP (the \
			old name has no certificate for it anymore)\n  \
			- the webmail moves to https://{new_mail}\n  \
			- {new_mail} gets its own certificate\n  \
			- the MX and SRV records of every mail domain have to point to \
			{new_mail}, see below\n  \
			(--keep-mail-host keeps {old_mail})"
		);
	}

	step(&format!(
		"setting domain = {new} in {}",
		config_path.display()
	));
	let ctn = std::fs::read_to_string(config_path)
		.map_err(|e| format!("{}: {e}", config_path.display()))?;
	let mut ctn = crate::config::set_key(&ctn, "domain", &new);
	if mail && keep_mail_host {
		println!("    mail-host = {old_mail}: mail stays where it is");
		ctn = crate::config::set_key(&ctn, "mail-host", &old_mail);
	}
	let cfg: Config = toml::from_str(&ctn).map_err(|e| e.to_string())?;
	std::fs::write(config_path, &ctn)
		.map_err(|e| format!("{}: {e}", config_path.display()))?;

	step(&format!("/etc/mailname: {new}"));
	std::fs::write("/etc/mailname", format!("{new}\n"))
		.map_err(|e| format!("/etc/mailname: {e}"))?;

	// renamed back: the name is the panel's again
	let mut olds = old_names(&db.conn());
	olds.retain(|o| *o != new && *o != old);
	olds.push(old.clone());
	set_old_names(&db.conn(), &olds)?;

	step(&format!(
		"apply: the site and certificate of {new}, {old} redirects to it"
	));
	let report = crate::apply::apply(&cfg, db).await;

	step(&format!("DNS records {new} needs"));
	match mailbox::panel_dns(&cfg, &groups).await {
		Ok(records) => println!("{}", mailbox::records_text(&records)),
		Err(e) => println!("{e}"),
	}
	if mail && !keep_mail_host {
		for d in groups.iter().flat_map(|g| &g.mail_domains) {
			step(&format!("DNS records of the mail domain {}", d.name));
			match mailbox::dns_text(&cfg, db, &d.name).await {
				Ok(text) => println!("{text}"),
				Err(e) => println!("{e}"),
			}
		}
	}
	println!(
		"once `genco panel forget-old {old}` ran, these can go: the A/AAAA \
		records of {old}, its TXT records for genco's mail (SPF, \
		{sel}._domainkey.{old}, _dmarc.{old}) if nothing else sends as it{}",
		if mail && !keep_mail_host {
			format!(", and the A/AAAA records of {old_mail}")
		} else {
			String::new()
		},
		sel = crate::mail::PANEL_SELECTOR,
	);

	runners_text(db, &old, &new);

	if !report.ok {
		return Err(format!(
			"apply failed, see the errors above; fix them and run genco \
			apply, then systemctl restart genco (the panel still runs as \
			{old})"
		));
	}

	step("restarting genco in a few seconds, it answers as the new name");
	// its own unit: the agent's command would end with genco
	crate::util::run(&[
		"systemd-run",
		"--on-active=3",
		"--collect",
		"--quiet",
		"systemctl",
		"try-restart",
		"genco",
	])
	.await?;
	println!(
		"done: https://{new}; https://{old} redirects to it until `genco \
		panel forget-old {old}`"
	);
	Ok(())
}

/// What else `name` is on this server, it can't be the panel then.
fn taken(cfg: &Config, groups: &[Group], name: &str) -> Option<String> {
	for g in groups {
		for d in &g.domains {
			if d.name == name || (d.www && format!("www.{}", d.name) == name) {
				return Some(format!("a domain of the group {}", g.name));
			}
		}
		if g.mail_domains.iter().any(|d| d.name == name) {
			return Some(format!("a mail domain of the group {}", g.name));
		}
	}
	if mailbox::host(cfg).as_deref() == Some(name) {
		return Some("the mail host".into());
	}
	None
}

/// Whether `name`'s A/AAAA records point to this server; an error unless
/// `force`.
async fn points_here(name: &str, force: bool) -> Result<(), String> {
	let own = crate::dns::own_ips().await;
	let ips = crate::dns::resolve(name).await;
	if ips.iter().any(|ip| own.contains(ip)) {
		println!("    {name}: {}", ips.join(", "));
		return Ok(());
	}
	let msg = format!(
		"{name} doesn't point to this server ({}); set its A record to {}",
		if ips.is_empty() {
			"no DNS record".to_string()
		} else {
			ips.join(", ")
		},
		crate::dns::own_ipv4()
			.await
			.first()
			.cloned()
			.unwrap_or_default()
	);
	if !force {
		return Err(format!("{msg}, or rename with --force"));
	}
	println!("    {msg}; --force: it gets its certificate later with apply");
	Ok(())
}

/// The runners have the panel's URL in their config, genco can't change
/// it there.
fn runners_text(db: &Db, old: &str, new: &str) {
	let runners = crate::runners::list(&db.conn()).unwrap_or_default();
	if runners.is_empty() {
		return;
	}
	step("runners: they keep working through the old name's /api/");
	println!(
		"until the old name is forgotten; on each runner box run:\n  \
		sed -i 's|https://{old}|https://{new}|' /etc/genco/runner.toml \
		&& systemctl restart genco-runner"
	);
	for r in &runners {
		let host = r.info["host"].as_str().unwrap_or("host unknown");
		println!("  {} ({host})", r.name);
	}
	println!(
		"also CI jobs and git webhooks that call https://{old}/api/ \
		should get the new name"
	);
}

async fn forget_old(cfg: &Config, db: &Db, name: &str) -> Result<(), String> {
	let mut olds = old_names(&db.conn());
	if !olds.iter().any(|o| o == name) {
		return Err(if olds.is_empty() {
			"the panel has no old names".into()
		} else {
			format!("{name} is no old name, they are: {}", olds.join(", "))
		});
	}
	olds.retain(|o| o != name);
	set_old_names(&db.conn(), &olds)?;

	step(&format!("apply: removing the site of {name}"));
	let report = crate::apply::apply(cfg, db).await;

	let groups = hosting::groups(&db.conn()).map_err(|e| e.to_string())?;
	let used = taken(cfg, &groups, name).is_some()
		|| cfg.domain.as_deref() == Some(name);
	if crate::host::has_cert(name) && !used {
		step(&format!("removing the certificate of {name}"));
		crate::util::run(&["certbot", "delete", "-n", "--cert-name", name])
			.await?;
	}
	if !report.ok {
		return Err("apply failed, see the errors above".into());
	}
	println!(
		"{name} doesn't answer anymore; its A/AAAA records and its TXT \
		records for genco's mail can go"
	);
	Ok(())
}
