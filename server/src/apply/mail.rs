//! Mail after its files were written: Dovecot, the spam filter, Postfix
//! with its submission ports, the webmail; the users and keys it needs.

use super::*;

/// Mail's files once Dovecot is installed (`genco setup`), else none.
pub(super) fn mail_files(
	r: &mut Report,
	cfg: &Config,
	groups: &[Group],
) -> Vec<File> {
	if !Path::new("/usr/sbin/dovecot").exists() {
		if mailbox::enabled(groups) {
			r.error("mail domains need Dovecot: run genco setup");
		}
		return vec![];
	}
	let cert = mailbox::host(cfg).is_some_and(|h| host::has_cert(&h));
	mailbox::files(cfg, groups, cert, mailbox::docker_installed())
}

/// Postfix and Dovecot after their files were written. Dovecot and the
/// spam filter run only while there are mail domains: Rspamd starts
/// before Postfix uses it and stops after Postfix no longer does.
pub(super) async fn mail_services(
	r: &mut Report,
	cfg: &Config,
	groups: &[Group],
	changed: &HashSet<PathBuf>,
) {
	let on = mailbox::enabled(groups);
	let cert = mailbox::host(cfg).is_some_and(|h| host::has_cert(&h));

	if on {
		let conf = changed.contains(Path::new(mailbox::DOVECOT_CONF));
		let test = ["doveconf", "-n", "-c", mailbox::DOVECOT_CONF];
		ensure_running(r, "dovecot", conf, Some(&test)).await;
	} else if is_active("dovecot").await {
		r.run(&["systemctl", "disable", "--now", "-q", "dovecot"])
			.await;
	}
	if on && mailfilter::installed() {
		filter_services(r, changed).await;
	}
	postfix(r, cfg, groups, on && cert, changed).await;
	if !on && mailfilter::installed() {
		stop_filter(r).await;
	}
	if on && webmail::installed() {
		webmail_service(r, changed).await;
	}
}

/// Enables and restarts `unit` when its files changed or it doesn't run;
/// with `test`, only once that check of its config passes: a broken
/// config would stop the running one.
pub(super) async fn ensure_running(
	r: &mut Report,
	unit: &str,
	changed: bool,
	test: Option<&[&str]>,
) {
	if !changed && is_active(unit).await {
		return;
	}
	if let Some(test) = test
		&& !r.run(test).await
	{
		r.error(format!(
			"{unit} keeps the old config until the error is fixed"
		));
		return;
	}
	r.run(&["systemctl", "enable", "-q", unit]).await;
	r.run(&["systemctl", "restart", unit]).await;
}

/// main.cf's values and the submission ports; a reload for most changes,
/// a restart for the interfaces. Without Docker its order after Docker
/// goes, docker0 too.
pub(super) async fn postfix(
	r: &mut Report,
	cfg: &Config,
	groups: &[Group],
	submit: bool,
	changed: &HashSet<PathBuf>,
) {
	let cert = mailbox::host(cfg).is_some_and(|h| host::has_cert(&h));
	let mut restart = false;
	let mut reload = changed.iter().any(|p| p.starts_with("/etc/genco/mail"));
	if !mailbox::docker_installed()
		&& remove_unwritten(r, Path::new(mailbox::POSTFIX_AFTER_DOCKER))
	{
		r.run(&["systemctl", "daemon-reload"]).await;
	}

	let bridges = mailbox::docker_bridges().await;
	let settings = mailbox::postfix_settings(cfg, groups, cert, &bridges);
	let names: Vec<&str> = settings.iter().map(|(n, _)| *n).collect();
	let current = postconf(&names).await;
	for ((name, value), now) in settings.iter().zip(&current) {
		if value != now {
			let line = format!("{name} = {value}");
			if r.run(&["postconf", "-e", &line]).await {
				r.info(format!("postfix: {line}"));
				restart |= *name == "inet_interfaces";
				reload = true;
			}
		}
	}
	if submission(r, submit).await {
		reload = true;
	}
	if restart {
		r.run(&["systemctl", "restart", "postfix"]).await;
	} else if reload {
		r.run(&["postfix", "reload"]).await;
	}
}

/// Removes a unit file or drop-in genco no longer writes, only one with
/// its header; true when it did and systemd needs a daemon-reload.
fn remove_unwritten(r: &mut Report, path: &Path) -> bool {
	let Ok(content) = std::fs::read_to_string(path) else {
		return false;
	};
	if !content.starts_with(render::HEADER) {
		return false;
	}
	match std::fs::remove_file(path) {
		Ok(()) => {
			r.info(format!("removed {}", path.display()));
			true
		}
		Err(e) => {
			r.error(format!("removing {}: {e}", path.display()));
			false
		}
	}
}

/// SnappyMail's PHP and its site, its settings kept in line.
pub(super) async fn webmail_service(
	r: &mut Report,
	changed: &HashSet<PathBuf>,
) {
	let unit = Path::new("/etc/systemd/system").join(webmail::UNIT);
	if changed.contains(&unit) {
		r.run(&["systemctl", "daemon-reload"]).await;
	}
	let ours = changed
		.iter()
		.any(|p| p == &unit || p.starts_with("/etc/genco/webmail"));
	ensure_running(r, webmail::UNIT, ours, None).await;
	for path in webmail::remove_other_domains() {
		r.info(format!("removed {}", path.display()));
	}
	match webmail::admin_panel_off() {
		Ok(true) => r.info("webmail: admin panel off"),
		Ok(false) => {}
		Err(e) => r.error(format!("webmail config: {e}")),
	}
}

/// The relay's login needs the SASL modules.
pub(super) async fn relay_sasl(r: &mut Report, db: &Db) {
	if !crate::mail::settings(db).is_ok_and(|s| s.mode == "relay") {
		return;
	}
	match crate::mail::ensure_sasl().await {
		Ok(true) => {
			r.info("installed the SASL modules for the relay's login");
			r.run(&["postfix", "reload"]).await;
		}
		Ok(false) => {}
		Err(e) => r.error(format!("SASL modules for the relay: {e}")),
	}
}

/// The users and directories mail needs, before its files are written.
pub(super) async fn prepare_mail(r: &mut Report, groups: &[Group]) {
	// genco's own mails are signed with or without mail domains
	match crate::mail::ensure_panel_key().await {
		Ok(true) => r.info("made the DKIM key of genco's own mail"),
		Ok(false) => {}
		Err(e) => r.error(format!("DKIM key of genco's mail: {e}")),
	}
	if !mailbox::enabled(groups) {
		return;
	}
	ensure_vmail(r).await;
	match mailfilter::ensure_keys(groups).await {
		Ok(made) => {
			for d in made {
				r.info(format!("made a DKIM key for {d}"));
			}
		}
		Err(e) => r.error(format!("DKIM keys: {e}")),
	}
	if !webmail::installed() {
		return;
	}
	if user_ids(webmail::USER).is_none() {
		super::group::add_user(r, webmail::USER, webmail::DATA).await;
		// nginx talks to its PHP socket
		r.run(&["usermod", "-aG", webmail::USER, "www-data"]).await;
	}
	let Some((uid, gid)) = user_ids(webmail::USER) else {
		return;
	};
	let domains = Path::new(webmail::DATA).join("_data_/_default_/domains");
	for dir in domains
		.ancestors()
		.take_while(|d| d.starts_with(webmail::DATA))
	{
		let made = std::fs::create_dir_all(dir)
			.and_then(|()| chown(dir, Some(uid), Some(gid)))
			.and_then(|()| set_mode(dir, 0o700));
		if let Err(e) = made {
			r.error(format!("{}: {e}", dir.display()));
		}
	}
}

/// Rspamd with Redis, postsrsd and the Junk rule after their files changed
/// or when they don't run.
pub(super) async fn filter_services(
	r: &mut Report,
	changed: &HashSet<PathBuf>,
) {
	if !is_active("redis-server").await {
		r.run(&["systemctl", "enable", "--now", "-q", "redis-server"])
			.await;
	}
	let rspamd = changed.iter().any(|p| p.starts_with("/etc/rspamd"));
	ensure_running(r, "rspamd", rspamd, Some(&["rspamadm", "configtest"]))
		.await;
	let srs = changed.contains(Path::new(mailfilter::SRS_DEFAULTS));
	ensure_running(r, "postsrsd", srs, None).await;
	// compiled once, Dovecot can't write next to it when it delivers
	if changed.contains(Path::new(mailfilter::SPAM_SIEVE))
		&& Path::new("/usr/bin/sievec").exists()
	{
		r.run(&["sievec", mailfilter::SPAM_SIEVE]).await;
	}
}

/// Rspamd, postsrsd and Redis off without mail domains, Postfix no
/// longer uses them.
async fn stop_filter(r: &mut Report) {
	for unit in ["rspamd", "postsrsd", "redis-server"] {
		let enabled = Command::new("systemctl")
			.args(["is-enabled", "-q", unit])
			.status()
			.await
			.is_ok_and(|s| s.success());
		if enabled || is_active(unit).await {
			r.run(&["systemctl", "disable", "--now", "-q", unit]).await;
		}
	}
}

/// The values of main.cf settings, in order.
pub(super) async fn postconf(names: &[&str]) -> Vec<String> {
	let out = Command::new("postconf")
		.arg("-h")
		.args(names)
		.output()
		.await;
	let out = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string());
	out.unwrap_or_default().lines().map(String::from).collect()
}

/// Ports 587 and 465 in master.cf, removed when off. True if changed.
pub(super) async fn submission(r: &mut Report, on: bool) -> bool {
	let mut changed = false;
	for (service, extra) in mailbox::SUBMISSION {
		let key = format!("{service}/inet");
		let out = Command::new("postconf").args(["-M", &key]).output().await;
		let exists = out.is_ok_and(|o| !o.stdout.is_empty());

		if !on {
			if exists {
				changed |= r.run(&["postconf", "-MX", &key]).await;
			}
			continue;
		}

		let line = format!("{key}={service} inet n - y - - smtpd");
		let mut ok = exists || r.run(&["postconf", "-M", &line]).await;
		let options = mailbox::SUBMISSION_OPTIONS.iter().chain(extra.iter());
		let wanted: Vec<String> = [format!("syslog_name=postfix/{service}")]
			.into_iter()
			.chain(options.map(|(k, v)| format!("{k}={v}")))
			.collect();
		for w in &wanted {
			let (k, v) = w.split_once('=').unwrap();
			let out = Command::new("postconf")
				.args(["-P", &format!("{key}/{k}")])
				.output()
				.await;
			let now = out
				.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
				.unwrap_or_default();
			if now != format!("{key}/{k} = {v}") {
				ok &= r.run(&["postconf", "-P", &format!("{key}/{w}")]).await;
				changed = true;
			}
		}
		if ok && !exists {
			r.info(format!("postfix: {service} on"));
			changed = true;
		}
	}
	changed
}

/// The system user that owns the mail, and its directory only it reads.
pub(super) async fn ensure_vmail(r: &mut Report) {
	if user_ids("vmail").is_none() {
		super::group::add_user(r, "vmail", mailbox::VMAIL).await;
	}
	let Some((uid, gid)) = user_ids("vmail") else {
		return;
	};
	let dir = Path::new(mailbox::VMAIL);
	let made = std::fs::create_dir_all(dir)
		.and_then(|()| chown(dir, Some(uid), Some(gid)))
		.and_then(|()| set_mode(dir, 0o700));
	if let Err(e) = made {
		r.error(format!("{}: {e}", dir.display()));
	}
}
