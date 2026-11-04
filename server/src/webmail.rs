//! Webmail: SnappyMail on the mail host, as its own user with its own
//! PHP. `genco setup` installs or updates it from snappymail.eu, checked
//! against the release key in this repository; `apply` writes its PHP
//! pool, unit, nginx site and the one domain setting that sends every
//! login to this server's IMAP and SMTP.

use std::path::{Path, PathBuf};

use crate::render::{File, HEADER, server_blocks};
use crate::util::sh;

/// SnappyMail's code, only root writes it.
const APP: &str = "/usr/local/share/genco-webmail";
/// Its settings, caches and the users' settings.
pub const DATA: &str = "/var/lib/genco-webmail";
pub const USER: &str = "webmail";
pub const UNIT: &str = "genco-webmail.service";
const PHP: &str = "8.4";
const SOCKET: &str = "/run/genco-webmail/php-fpm.sock";
const POOL: &str = "/etc/genco/webmail/php-fpm.conf";
const RELEASE: &str = "https://snappymail.eu/repository/latest.tar.gz";
const KEY: &str = include_str!("webmail/snappymail-releases.asc");

pub fn installed() -> bool {
	Path::new(APP).join("index.php").exists()
}

/// Downloads the latest release, checks its signature and puts it in
/// place if it is new. Returns the installed version.
pub async fn install() -> Result<String, String> {
	let tmp = sh("mktemp -d /tmp/genco-webmail.XXXXXX").await?;
	let tmp = tmp.as_str();
	sh(&format!("mkdir -p {tmp}/x")).await?;
	std::fs::write(format!("{tmp}/key.asc"), KEY).map_err(|e| e.to_string())?;
	sh(&format!(
		"cd {tmp} && curl -fsSL -o release.tar.gz {RELEASE} \
		&& curl -fsSL -o release.tar.gz.asc {RELEASE}.asc \
		&& gpg --batch --quiet --no-default-keyring --keyring {tmp}/keys.gpg \
			--import key.asc \
		&& gpgv --keyring {tmp}/keys.gpg release.tar.gz.asc release.tar.gz \
		&& tar xzf release.tar.gz -C x"
	))
	.await
	.map_err(|e| format!("SnappyMail download or signature: {e}"))?;

	let version = std::fs::read_to_string(format!("{tmp}/x/data/VERSION"))
		.map_err(|e| e.to_string())?
		.trim()
		.to_string();
	let current = std::fs::read_to_string(Path::new(APP).join("VERSION"))
		.unwrap_or_default();
	if current.trim() != version {
		// the data folder lives apart, the release's own one goes
		std::fs::write(
			format!("{tmp}/x/include.php"),
			format!(
				"<?php\n// written by genco\ndefine('APP_DATA_FOLDER_PATH', '{DATA}/');\n"
			),
		)
		.map_err(|e| e.to_string())?;
		std::fs::write(format!("{tmp}/x/VERSION"), &version)
			.map_err(|e| e.to_string())?;
		sh(&format!(
			"rm -rf {tmp}/x/data {tmp}/x/_include.php \
			&& chown -R root:root {tmp}/x && chmod -R go-w {tmp}/x \
			&& rm -rf {APP}.old && {{ [ ! -e {APP} ] || mv {APP} {APP}.old; }} \
			&& mv {tmp}/x {APP} && rm -rf {APP}.old"
		))
		.await?;
	}
	sh(&format!("rm -rf {tmp}")).await?;
	Ok(version)
}

/// The domain setting for every login (`default` is SnappyMail's `*`):
/// IMAP and SMTP of this server, sending only after a login.
fn domain_json() -> String {
	let server = |port: u16, extra: &str| {
		format!(
			"{{
		\"host\": \"127.0.0.1\",
		\"port\": {port},
		\"type\": 1,
		\"timeout\": 60,
		\"shortLogin\": false,
		\"lowerLogin\": true,
		\"sasl\": [\"PLAIN\", \"LOGIN\"],
		\"ssl\": {{
			\"verify_peer\": false,
			\"verify_peer_name\": false,
			\"allow_self_signed\": false,
			\"SNI_enabled\": true,
			\"disable_compression\": true,
			\"security_level\": 1
		}}{extra}
	}}"
		)
	};
	format!(
		"{{
	\"IMAP\": {},
	\"SMTP\": {},
	\"Sieve\": {{
		\"host\": \"127.0.0.1\",
		\"port\": 4190,
		\"type\": 0,
		\"enabled\": false
	}},
	\"whiteList\": \"\"
}}
",
		server(993, ",\n\t\t\"use_expunge_all_on_delete\": false"),
		server(
			465,
			",\n\t\t\"useAuth\": true,\n\t\t\"setSender\": false,\n\t\t\"usePhpMail\": false"
		),
	)
}

fn pool() -> String {
	let header = HEADER.replacen('#', ";", 1);
	format!(
		"{header}
[global]
error_log = syslog

[webmail]
listen = {SOCKET}
listen.mode = 0660
pm = ondemand
pm.max_children = 10
pm.process_idle_timeout = 60s
catch_workers_output = yes
php_admin_value[open_basedir] = {APP}/:{DATA}/:/tmp/
php_admin_value[upload_tmp_dir] = /tmp
php_admin_value[memory_limit] = 256M
php_admin_value[upload_max_filesize] = 25M
php_admin_value[post_max_size] = 30M
php_admin_flag[log_errors] = on
php_admin_flag[expose_php] = off
"
	)
}

fn service() -> String {
	format!(
		"{HEADER}
[Unit]
Description=Webmail (SnappyMail) of genco
After=network.target dovecot.service

[Service]
Type=notify
ExecStart=/usr/sbin/php-fpm{PHP} --nodaemonize --fpm-config {POOL}
ExecReload=/bin/kill -USR2 $MAINPID
RuntimeDirectory=genco-webmail
RuntimeDirectoryMode=0750
Restart=on-failure
{deny}
User={USER}
Group={USER}
UMask=0027
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths={DATA}
PrivateTmp=yes
PrivateDevices=yes
ProtectProc=invisible
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
SystemCallArchitectures=native

[Install]
WantedBy=multi-user.target
",
		deny = crate::render::BIND_DENY,
	)
}

fn site(host: &str, cert: bool) -> String {
	// without a certificate nobody logs in over http, only ACME answers
	if !cert {
		return server_blocks(host, false, "");
	}
	let body = format!(
		"\taccess_log /var/log/nginx/genco/{host}.access.log;
	error_log /var/log/nginx/genco/{host}.error.log;
	add_header Strict-Transport-Security \"max-age=31536000\" always;
	client_max_body_size 30m;
	root {APP};
	index index.php;

	# SnappyMail's admin panel stays off (also in its config), genco sets
	# it up; it answers to ?admin and ?/admin
	if ($args ~* \"(^|[&/])admin\") {{
		return 403;
	}}

	location / {{
		try_files $uri $uri/ /index.php?$query_string;
	}}

	location ~ \\.php$ {{
		try_files $fastcgi_script_name =404;
		include fastcgi_params;
		fastcgi_param SCRIPT_FILENAME $realpath_root$fastcgi_script_name;
		fastcgi_pass unix:{SOCKET};
		fastcgi_read_timeout 120;
	}}

	location ~ /\\.(?!well-known) {{
		deny all;
	}}
"
	);
	server_blocks(host, true, &body)
}

/// Its files once it is installed and there is mail.
pub fn files(host: &str, cert: bool) -> Vec<File> {
	if !installed() {
		return vec![];
	}
	let domains = PathBuf::from(DATA).join("_data_/_default_/domains");
	vec![
		File {
			path: POOL.into(),
			content: pool(),
			private_to: None,
		},
		File {
			path: PathBuf::from("/etc/systemd/system").join(UNIT),
			content: service(),
			private_to: None,
		},
		File {
			path: crate::render::nginx_config(host),
			content: site(host, cert),
			private_to: None,
		},
		File {
			path: domains.join("default.json"),
			content: domain_json(),
			private_to: Some(USER.into()),
		},
		// SnappyMail copies its sample domains (gmail, …) without it
		File {
			path: domains.join("disabled"),
			content: String::new(),
			private_to: Some(USER.into()),
		},
	]
}

const CONFIG: &str = "_data_/_default_/configs/application.ini";

/// Turns SnappyMail's admin panel off in its config, which SnappyMail
/// writes itself on its first request. True if it changed.
pub fn admin_panel_off() -> std::io::Result<bool> {
	let path = Path::new(DATA).join(CONFIG);
	let Ok(ini) = std::fs::read_to_string(&path) else {
		// not there yet: SnappyMail fills in the rest
		std::fs::create_dir_all(path.parent().unwrap())?;
		std::fs::write(&path, "[security]\nallow_admin_panel = Off\n")?;
		let _ = std::process::Command::new("chown")
			.args([&format!("{USER}:{USER}"), &path.display().to_string()])
			.status();
		return Ok(true);
	};
	let mut changed = false;
	let lines: Vec<String> = ini
		.lines()
		.map(|l| {
			if l.trim_start().starts_with("allow_admin_panel")
				&& !l.contains("Off")
			{
				changed = true;
				"allow_admin_panel = Off".to_string()
			} else {
				l.to_string()
			}
		})
		.collect();
	if changed {
		std::fs::write(&path, lines.join("\n") + "\n")?;
	}
	Ok(changed)
}

/// SnappyMail's sample domains (gmail, hotmail) would let anyone use it
/// for other servers: only genco's files stay.
pub fn remove_other_domains() -> Vec<PathBuf> {
	let dir = PathBuf::from(DATA).join("_data_/_default_/domains");
	let mut removed = vec![];
	for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
		let path = entry.path();
		let ours = path
			.file_name()
			.is_some_and(|n| n == "default.json" || n == "disabled");
		if !ours && std::fs::remove_file(&path).is_ok() {
			removed.push(path);
		}
	}
	removed
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn domain_setting_is_json_for_this_server() {
		let v: serde_json::Value =
			serde_json::from_str(&domain_json()).unwrap();
		assert_eq!(v["IMAP"]["port"], 993);
		assert_eq!(v["SMTP"]["port"], 465);
		assert_eq!(v["SMTP"]["useAuth"], true);
		assert_eq!(v["Sieve"]["enabled"], false);
	}

	#[test]
	fn the_unit_binds_no_tcp_port_but_port_0() {
		let u = service();
		assert!(u.contains("SocketBindDeny=tcp:1-65535\n"));
		assert!(!u.contains("SocketBindAllow"));
	}

	#[test]
	fn no_webmail_over_http() {
		let s = site("mail.x.ch", false);
		assert!(!s.contains("fastcgi_pass"));
		assert!(site("mail.x.ch", true).contains("fastcgi_pass"));
	}
}
