//! `genco setup`: looks at the server first (inventory), then hardens it
//! and installs what genco needs. Safe to run again.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::util::sh;

/// pgsql too: a PostgreSQL database may come later, and PostgreSQL itself
/// may be there already without genco installing it
const PHP_EXTENSIONS: &[&str] = &[
	"fpm", "cli", "mysql", "pgsql", "gd", "intl", "mbstring", "xml", "zip",
	"curl", "bcmath", "imagick",
];
const NODE_MAJOR: &str = "24";
pub const INVENTORY: &str = "/var/lib/genco/inventory.txt";

fn step(text: &str) {
	println!("==> {text}");
}

/// Writes the file if its content differs, returns true if it did.
fn write(path: &str, content: &str) -> Result<bool, String> {
	if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
		return Ok(false);
	}
	if let Some(dir) = Path::new(path).parent() {
		std::fs::create_dir_all(dir).map_err(|e| format!("{path}: {e}"))?;
	}
	std::fs::write(path, content).map_err(|e| format!("{path}: {e}"))?;
	Ok(true)
}

pub async fn main(
	config_path: &Path,
	domain: Option<String>,
	email: Option<String>,
) -> Result<(), String> {
	if !crate::util::is_root().await {
		return Err("genco setup needs root".into());
	}

	step("updating the package lists");
	sh("apt-get -o DPkg::Lock::Timeout=300 update -qq").await?;

	step("inventory");
	let inventory = inventory().await;
	println!("{inventory}");
	write(INVENTORY, &inventory)?;
	check_ports().await?;

	step("ssh: keys only");
	harden_ssh().await?;
	step("firewall: ssh, http, https and mail");
	firewall().await?;
	step("automatic updates: Debian security only");
	security_updates().await?;

	step("installing nginx, MariaDB, certbot, composer");
	apt(&[
		"nginx",
		"mariadb-server",
		"certbot",
		"composer",
		"git",
		"curl",
		"gnupg",
		"zip",
		"unzip",
		"ca-certificates",
		// consistent copies of the sites' SQLite files for the backup
		"sqlite3",
		// dig, for the DNS checks
		"bind9-dnsutils",
	])
	.await?;
	step("installing PHP 8.2 to 8.5");
	php().await?;
	step(&format!("installing Node {NODE_MAJOR}"));
	node().await?;

	step("configuring MariaDB and nginx");
	mariadb().await?;
	nginx().await?;

	step("postfix for the mails of genco and the sites");
	postfix(domain_for_mail(config_path, &domain)).await?;

	step("Dovecot for mailboxes");
	dovecot().await?;
	step("spam filter and DKIM (Rspamd, Redis), SRS for forwards");
	mail_filter().await?;

	step("webmail: SnappyMail, checked against its release key");
	let version = crate::webmail::install().await?;
	println!("    SnappyMail {version}");

	step("daily backups with restic");
	backups().await?;

	step("the genco service");
	config(config_path, domain, email)?;
	service().await?;
	Ok(())
}

async fn inventory() -> String {
	let lines = [
		(
			"System",
			"grep PRETTY_NAME /etc/os-release | cut -d'\"' -f2",
		),
		("Kernel", "uname -r"),
		("CPU cores", "nproc"),
		(
			"Memory",
			"free -h | awk '/Mem:/ {print $2 \" total, \" $7 \" available\"}'",
		),
		(
			"Disk /",
			"df -h / | awk 'NR==2 {print $2 \" total, \" $4 \" free\"}'",
		),
		(
			"Login users",
			"awk -F: '$7 !~ /(nologin|false|sync)$/ {print $1}' /etc/passwd | xargs",
		),
		(
			"Users with ssh keys",
			"for d in /root /home/*; do [ -s $d/.ssh/authorized_keys ] && \
			basename $d; done | xargs",
		),
		(
			"sshd",
			"sshd -T 2>/dev/null | grep -E '^(port|permitrootlogin|passwordauthentication) ' | xargs",
		),
		(
			"Listening",
			"ss -Htlnp | awk '{split($4,a,\":\"); p=a[length(a)]; \
			match($6,/\"[^\"]+\"/); print p \"/\" substr($6,RSTART+1,RLENGTH-2)}' \
			| sort -un | xargs",
		),
		(
			"Firewall",
			"nft list ruleset 2>/dev/null | grep -c . | \
			awk '{print ($1 > 0 ? $1 \" lines of nftables rules\" : \"none\")}'",
		),
		(
			"Pending updates",
			"apt-get -s upgrade 2>/dev/null | grep -c '^Inst' || true",
		),
		(
			"Security updates",
			"apt-get -s upgrade 2>/dev/null | grep '^Inst' | grep -ci security || true",
		),
		(
			"Services",
			"systemctl list-units --type=service --state=running --no-legend \
			| awk '{print $1}' | sed 's/.service$//' | xargs",
		),
	];

	let mut out = String::new();
	for (label, cmd) in lines {
		let value = sh(cmd).await.unwrap_or_else(|e| format!("({e})"));
		out.push_str(&format!("{label}: {value}\n"));
	}
	// many cloud hosts block it, then only a relay gets mail out
	let port25 = if crate::mail::port25_blocked().await {
		"seems blocked (common at cloud hosts like Infomaniak): use an SMTP \
		relay in Settings → Mail"
	} else {
		"open"
	};
	out.push_str(&format!("Outgoing port 25: {port25}\n"));
	out
}

/// Something other than nginx on 80 or 443 would take the sites' place.
async fn check_ports() -> Result<(), String> {
	let others = sh(
		"ss -Htlnp '( sport = :80 or sport = :443 )' | grep -v nginx || true",
	)
	.await?;
	if !others.is_empty() {
		return Err(format!(
			"something else listens on port 80 or 443, stop it first:\n{others}"
		));
	}
	Ok(())
}

async fn harden_ssh() -> Result<(), String> {
	let keys = sh(
		"for d in /root /home/*; do [ -s $d/.ssh/authorized_keys ] && echo $d; done; true",
	)
	.await?;
	if keys.is_empty() {
		println!(
			"    skipped: nobody has an ssh key yet, password logins would \
			lock everyone out"
		);
		return Ok(());
	}

	// genco's file is read first and wins: a server that allows no root
	// login at all keeps that
	let root = if root_login_off() {
		"no"
	} else {
		"prohibit-password"
	};
	let changed = write(
		"/etc/ssh/sshd_config.d/10-genco.conf",
		&format!(
			"# written by genco: keys only\nPasswordAuthentication no\n\
			KbdInteractiveAuthentication no\nPermitRootLogin {root}\n"
		),
	)?;
	if changed {
		sh("sshd -t && systemctl reload ssh").await?;
	}
	Ok(())
}

/// Whether sshd's own config (not genco's file) says `PermitRootLogin no`.
fn root_login_off() -> bool {
	let mut files = vec![PathBuf::from("/etc/ssh/sshd_config")];
	if let Ok(dir) = std::fs::read_dir("/etc/ssh/sshd_config.d") {
		files.extend(
			dir.flatten()
				.map(|e| e.path())
				.filter(|p| !p.ends_with("10-genco.conf")),
		);
	}
	files.iter().any(|f| {
		std::fs::read_to_string(f)
			.unwrap_or_default()
			.lines()
			.any(|l| {
				let words: Vec<String> =
					l.split_whitespace().map(str::to_lowercase).collect();
				words == ["permitrootlogin", "no"]
			})
	})
}

async fn firewall() -> Result<(), String> {
	// cloud images come without it
	apt(&["nftables"]).await?;
	let port = sh("sshd -T | awk '/^port / {print $2; exit}'").await?;
	let port = if port.is_empty() { "22".into() } else { port };

	let current =
		std::fs::read_to_string("/etc/nftables.conf").unwrap_or_default();
	if !current.contains("written by genco") && current.contains("dport") {
		std::fs::write("/etc/nftables.conf.before-genco", &current)
			.map_err(|e| e.to_string())?;
		println!("    the old rules are in /etc/nftables.conf.before-genco");
	}

	// the apps' port rules, `apply` fills it
	if !Path::new(crate::render::FIREWALL).exists() {
		write(crate::render::FIREWALL, "")?;
	}

	let conf = format!(
		"#!/usr/sbin/nft -f
# written by genco: ssh, http, https and mail (smtp, submission, imaps)
# from everywhere; nothing listens on the mail ports without mail domains
table inet filter
delete table inet filter

table inet filter {{
	# the workers' public ports, `apply` fills it
	set genco_public {{
		type inet_service;
	}}

	chain input {{
		type filter hook input priority filter; policy drop;
		iif lo accept
		ct state established,related accept
		ct state invalid drop
		meta l4proto {{ icmp, ipv6-icmp }} accept
		udp dport {{ 68, 546 }} accept
		tcp dport {{ {port}, 80, 443, 25, 465, 587, 993 }} accept
		tcp dport @genco_public accept
		# Docker (a server moving into genco still runs containers): its
		# containers reach the host's services, e.g. PostgreSQL
		iifname \"docker0\" accept
		iifname \"br-*\" accept
	}}
	chain forward {{
		type filter hook forward priority filter; policy drop;
		# Docker's own rules decide what goes to and from its containers
		iifname \"docker0\" accept
		oifname \"docker0\" accept
		iifname \"br-*\" accept
		oifname \"br-*\" accept
	}}
}}

include \"{}\"
",
		crate::render::FIREWALL
	);
	write("/etc/nftables.conf", &conf)?;
	sh("nft -c -f /etc/nftables.conf && nft -f /etc/nftables.conf \
		&& systemctl enable -q nftables")
	.await?;
	Ok(())
}

async fn security_updates() -> Result<(), String> {
	apt(&["unattended-upgrades", "needrestart"]).await?;
	write(
		"/etc/apt/apt.conf.d/51genco-security-only",
		"// written by genco: only Debian security updates install by themselves
#clear Unattended-Upgrade::Origins-Pattern;
Unattended-Upgrade::Origins-Pattern {
	\"origin=Debian,codename=${distro_codename},label=Debian-Security\";
	\"origin=Debian,codename=${distro_codename}-security,label=Debian-Security\";
};
",
	)?;
	write(
		"/etc/apt/apt.conf.d/20auto-upgrades",
		"APT::Periodic::Update-Package-Lists \"1\";\n\
		APT::Periodic::Unattended-Upgrade \"1\";\n",
	)?;
	Ok(())
}

async fn apt(packages: &[&str]) -> Result<(), String> {
	sh(&format!(
		"DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=300 install -y -qq {} >/dev/null",
		packages.join(" ")
	))
	.await
	.map(|_| ())
}

/// All versions from packages.sury.org, each group runs its own php-fpm,
/// so the packaged services stay off.
async fn php() -> Result<(), String> {
	if !Path::new("/etc/apt/sources.list.d/php.list").exists() {
		sh("curl -fsSLo /tmp/debsuryorg-archive-keyring.deb \
			https://packages.sury.org/debsuryorg-archive-keyring.deb \
			&& dpkg -i /tmp/debsuryorg-archive-keyring.deb >/dev/null \
			&& echo \"deb [signed-by=/usr/share/keyrings/debsuryorg-archive-keyring.gpg] \
			https://packages.sury.org/php/ $(. /etc/os-release && echo $VERSION_CODENAME) main\" \
			>/etc/apt/sources.list.d/php.list && apt-get -o DPkg::Lock::Timeout=300 update -qq")
			.await?;
	}

	for v in crate::hosting::PHP_VERSIONS {
		let mut packages: Vec<String> = PHP_EXTENSIONS
			.iter()
			.map(|e| format!("php{v}-{e}"))
			.collect();
		// built in since 8.5
		if sh(&format!("apt-cache show php{v}-opcache >/dev/null 2>&1"))
			.await
			.is_ok()
		{
			packages.push(format!("php{v}-opcache"));
		}
		let packages: Vec<&str> = packages.iter().map(String::as_str).collect();
		let missing =
			sh(&format!("dpkg -s {} >/dev/null 2>&1", packages.join(" ")))
				.await
				.is_err();
		apt(&packages).await?;
		sh(&format!("systemctl disable -q --now php{v}-fpm")).await?;
		// a new module only loads in a restarted php-fpm
		if missing {
			sh(&format!("systemctl try-restart 'genco-*-php{v}.service'"))
				.await?;
		}
		// deploys put it first in PATH, composer and craft use the
		// group's version
		let bin = crate::render::php_bin_dir(v);
		sh(&format!(
			"mkdir -p {bin} && ln -sfn /usr/bin/php{v} {bin}/php"
		))
		.await?;
	}
	Ok(())
}

async fn node() -> Result<(), String> {
	let list = "/etc/apt/sources.list.d/nodesource.list";
	let line = format!(
		"deb [signed-by=/etc/apt/keyrings/nodesource.gpg] \
		https://deb.nodesource.com/node_{NODE_MAJOR}.x nodistro main\n"
	);
	if std::fs::read_to_string(list).ok().as_deref() != Some(line.as_str()) {
		sh("mkdir -p /etc/apt/keyrings && curl -fsSL \
			https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
			| gpg --dearmor --yes -o /etc/apt/keyrings/nodesource.gpg")
		.await?;
		write(list, &line)?;
		sh("apt-get -o DPkg::Lock::Timeout=300 update -qq").await?;
	}
	apt(&["nodejs"]).await
}

/// Only the local socket, the groups log in with their unix user.
async fn mariadb() -> Result<(), String> {
	let changed = write(
		"/etc/mysql/mariadb.conf.d/90-genco.cnf",
		"# written by genco: only the local socket\n[mysqld]\nskip-networking\n",
	)?;
	if changed {
		sh("systemctl restart mariadb").await?;
	}
	Ok(())
}

/// nginx's own settings for genco and the hosts it doesn't know.
const NGINX_CONF: &str = "# written by genco
# the panel's log leaves out the query, the event stream and downloads send
# a ticket for the session in it, and the tokens of the links in mails
log_format genco_panel '$remote_addr [$time_local] \"$request_method $genco_uri\" '
	'$status $body_bytes_sent \"$http_user_agent\"';
map $uri $genco_uri {
	default $uri;
	\"~^(?<path>/(api/)?(proposal|proposal-links|confirm|actions|invite|users/invite|email|users/email)/)[^/]+(?<rest>.*)$\" \"${path}...${rest}\";
}
# password guessing: 10 login attempts per minute and client
limit_req_zone $binary_remote_addr zone=genco_login:1m rate=10r/m;
limit_req_status 429;

map $http_upgrade $connection_upgrade {
	default upgrade;
	'' close;
}

# unknown hosts get nothing
server {
	listen 80 default_server;
	listen [::]:80 default_server;
	location ^~ /.well-known/acme-challenge/ {
		root /var/lib/genco-acme;
	}
	location / {
		return 444;
	}
}

include /etc/nginx/genco/*.conf;
";

/// The domains' nginx logs, rotated like Debian's /var/log/nginx/*.log.
const NGINX_LOGROTATE: &str = "# written by genco
/var/log/nginx/genco/*.log {
	daily
	missingok
	rotate 14
	compress
	delaycompress
	notifempty
	create 0640 www-data adm
	sharedscripts
	postrotate
		invoke-rc.d nginx rotate >/dev/null 2>&1
	endscript
}
";

async fn nginx() -> Result<(), String> {
	for dir in [
		"/etc/nginx/genco",
		"/var/log/nginx/genco",
		"/var/lib/genco-acme",
		"/srv/genco",
		"/etc/genco/php",
		"/etc/genco/apps",
	] {
		std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
	}
	// the groups' php-fpm and apps read their config, the secrets are in
	// config.toml (0600)
	sh("chmod 755 /etc/genco /etc/genco/php /etc/genco/apps").await?;

	write("/etc/nginx/conf.d/genco.conf", NGINX_CONF)?;

	// Debian's rule only rotates /var/log/nginx/*.log
	write("/etc/logrotate.d/genco", NGINX_LOGROTATE)?;

	// the debian default site and the panel site of earlier installs
	sh(
		"rm -f /etc/nginx/sites-enabled/default /etc/nginx/sites-enabled/genco-panel \
		/etc/nginx/sites-available/genco-panel",
	)
	.await?;
	crate::util::run(&["nginx", "-t", "-q"]).await?;
	crate::util::reload_nginx().await?;
	Ok(())
}

/// The panel's domain from the arguments or the config.
fn domain_for_mail(config_path: &Path, domain: &Option<String>) -> String {
	domain
		.clone()
		.or_else(|| Config::load(config_path).ok().and_then(|c| c.domain))
		.unwrap_or_else(|| "localhost".into())
}

/// Only on localhost: it delivers directly or through the relay set in
/// Settings → Mail, which this doesn't touch; the relay's login needs
/// the SASL modules. The sites' PHP sends with msmtp to it, postfix's
/// own sendmail needs setgid, which their sandbox doesn't allow.
async fn postfix(domain: String) -> Result<(), String> {
	sh(&format!(
		"echo 'postfix postfix/main_mailer_type select Internet Site' \
		| debconf-set-selections && echo 'postfix postfix/mailname string {domain}' \
		| debconf-set-selections"
	))
	.await?;
	apt(&["postfix", "msmtp", crate::mail::SASL_PACKAGE]).await?;
	sh(&format!(
		"postconf -e 'inet_interfaces = loopback-only' 'myhostname = {domain}' \
		'mydestination = localhost' 'smtp_tls_security_level = may' \
		&& systemctl restart postfix"
	))
	.await?;
	Ok(())
}

/// Installed without starting: Debian's own config would let the server's
/// users log in. `apply` writes genco's config and starts it once there
/// are mail domains.
async fn dovecot() -> Result<(), String> {
	if Path::new("/usr/sbin/dovecot").exists() {
		return Ok(());
	}
	apt_stopped(&["dovecot-imapd", "dovecot-lmtpd", "bind9-dnsutils"]).await?;
	sh("systemctl disable -q --now dovecot").await?;
	Ok(())
}

/// Rspamd, Redis and postsrsd installed without starting, they take
/// about 600 MB: `apply` runs them only while there are mail domains.
async fn mail_filter() -> Result<(), String> {
	let packages = ["dovecot-sieve", "rspamd", "redis-server", "postsrsd"];
	if crate::mailfilter::installed() {
		return apt(&packages).await;
	}
	apt_stopped(&packages).await?;
	sh("systemctl disable -q --now rspamd redis-server postsrsd").await?;
	Ok(())
}

/// Installs packages without starting their services.
async fn apt_stopped(packages: &[&str]) -> Result<(), String> {
	write("/usr/sbin/policy-rc.d", "#!/bin/sh\nexit 101\n")?;
	sh("chmod 755 /usr/sbin/policy-rc.d").await?;
	let installed = apt(packages).await;
	let _ = std::fs::remove_file("/usr/sbin/policy-rc.d");
	installed
}

/// restic, the repository's password and a timer at night. Without the
/// password the backups can't be read: keep a copy somewhere else.
async fn backups() -> Result<(), String> {
	apt(&["restic"]).await?;
	// the repository and the trash, only root may read them
	sh("mkdir -p /var/backups/genco && chmod 700 /var/backups/genco").await?;
	let password = crate::backup::PASSWORD_FILE;
	if !Path::new(password).exists() {
		sh(&format!(
			"umask 077 && openssl rand -base64 32 > {password}"
		))
		.await?;
		println!("    keep a copy of {password}, the backups need it");
	}

	write(
		"/etc/systemd/system/genco-backup.service",
		"# written by genco
[Unit]
Description=genco backup of the groups and the server's config

[Service]
Type=oneshot
ExecStart=/usr/local/bin/genco backup
",
	)?;
	write(
		"/etc/systemd/system/genco-backup.timer",
		"# written by genco
[Unit]
Description=genco backup every night

[Timer]
OnCalendar=*-*-* 03:00
RandomizedDelaySec=30m
Persistent=true

[Install]
WantedBy=timers.target
",
	)?;
	sh(
		"systemctl daemon-reload && systemctl enable -q --now genco-backup.timer",
	)
	.await?;
	Ok(())
}

/// Sets the panel's domain and the email in the config if they are new.
fn config(
	path: &Path,
	domain: Option<String>,
	email: Option<String>,
) -> Result<(), String> {
	let mut ctn = std::fs::read_to_string(path).unwrap_or_default();
	let cfg: Config = toml::from_str(&ctn).map_err(|e| e.to_string())?;

	for (key, new, old) in
		[("domain", domain, cfg.domain), ("email", email, cfg.email)]
	{
		if let Some(v) = new
			&& old.as_deref() != Some(v.as_str())
		{
			ctn = crate::config::set_key(&ctn, key, &v);
		}
	}

	std::fs::create_dir_all(path.parent().unwrap())
		.map_err(|e| e.to_string())?;
	std::fs::write(path, ctn).map_err(|e| e.to_string())?;
	Ok(())
}

async fn service() -> Result<(), String> {
	write(
		"/etc/systemd/system/genco.service",
		"# written by genco
[Unit]
Description=genco hosting panel and maintenance agent
After=network-online.target mariadb.service
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/genco serve
Restart=always
RestartSec=2
# services the agent started with systemd-run keep running when genco
# restarts
KillMode=mixed
TimeoutStopSec=20

[Install]
WantedBy=multi-user.target
",
	)?;
	sh("systemctl daemon-reload && systemctl enable -q genco").await?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn nginx_logs_of_the_domains_are_rotated() {
		let conf = NGINX_LOGROTATE;
		assert!(
			conf.starts_with(
				"# written by genco\n/var/log/nginx/genco/*.log {"
			)
		);
		assert!(conf.contains("rotate 14"));
		assert!(conf.contains("create 0640 www-data adm"));
		assert!(conf.contains("invoke-rc.d nginx rotate"));
	}

	#[test]
	fn the_panel_log_masks_the_tokens_of_links() {
		let conf = NGINX_CONF;
		assert!(conf.contains("\"$request_method $genco_uri\""));
		assert!(!conf.contains("$request_method $uri"));
		let map = conf.split("map $uri $genco_uri {").nth(1).unwrap();
		let map = map.split("\n}").next().unwrap();
		// every page and API route with a token of a mail in its path
		for path in [
			"proposal",
			"proposal-links",
			"confirm",
			"actions",
			"invite",
			"users/invite",
			"email",
			"users/email",
		] {
			assert!(
				map.contains(&format!("|{path}|"))
					|| map.contains(&format!("({path}|"))
					|| map.contains(&format!("|{path})"))
			);
		}
		assert!(map.contains("\"${path}...${rest}\";"));
	}
}
