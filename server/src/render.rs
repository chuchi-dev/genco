//! The config files genco writes, rendered from the database. Pure, so
//! `apply` writes them and the config view compares them with the disk.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::hosting::{Domain, Group, Kind, PhpSettings};
use crate::procs::{Schedule, Worker};

pub const HEADER: &str = "# written by genco, changes here are overwritten";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
	pub path: PathBuf,
	pub content: String,
	/// only root and this group's user may read it, others can read
	/// every file without
	pub private_to: Option<String>,
}

pub fn group_user(group: &str) -> String {
	format!("g-{group}")
}

pub fn group_dir(group: &str) -> PathBuf {
	PathBuf::from("/srv/genco").join(group)
}

pub fn slice(group: &str) -> String {
	format!("genco-{group}.slice")
}

pub fn php_unit(group: &str, version: &str) -> String {
	format!("genco-{group}-php{version}.service")
}

pub fn app_unit(group: &str, domain: &str) -> String {
	format!("genco-{group}-{domain}.service")
}

/// An app that restarts without downtime has two units: A is the usual
/// one, B its twin on another port. A restart starts the one that
/// doesn't run, nginx switches to it once it listens, then the old one
/// stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
	A,
	B,
}

impl Slot {
	pub fn other(self) -> Self {
		match self {
			Self::A => Self::B,
			Self::B => Self::A,
		}
	}
}

/// The slot nginx proxies to; always A without zero downtime.
pub fn live_slot(domain: &Domain) -> Slot {
	if domain.zero_downtime && domain.slot_b {
		Slot::B
	} else {
		Slot::A
	}
}

/// The slots that have a unit.
pub fn slots(domain: &Domain) -> &'static [Slot] {
	if domain.zero_downtime {
		&[Slot::A, Slot::B]
	} else {
		&[Slot::A]
	}
}

/// `_` can't be in a domain name: no other domain's unit has this name.
pub fn slot_unit(group: &str, domain: &str, slot: Slot) -> String {
	match slot {
		Slot::A => app_unit(group, domain),
		Slot::B => format!("genco-{group}-{domain}_b.service"),
	}
}

/// The unit of the app that runs now.
pub fn live_unit(group: &str, domain: &Domain) -> String {
	slot_unit(group, &domain.name, live_slot(domain))
}

/// How a deploy restarts an app.
#[derive(Debug, PartialEq, Eq)]
pub enum Restart {
	InPlace {
		unit: String,
	},
	/// start `start`, wait for `port`, point nginx at `to`, stop `stop`
	Switch {
		start: String,
		port: u16,
		to: Slot,
		stop: String,
	},
}

pub fn restart(group: &str, domain: &Domain) -> Restart {
	let live = live_slot(domain);
	if !domain.zero_downtime {
		return Restart::InPlace {
			unit: slot_unit(group, &domain.name, live),
		};
	}
	let to = live.other();
	Restart::Switch {
		start: slot_unit(group, &domain.name, to),
		port: slot_port(domain, to),
		to,
		stop: slot_unit(group, &domain.name, live),
	}
}

pub fn worker_unit(group: &str, name: &str) -> String {
	format!("genco-{group}-worker-{name}.service")
}

/// The service a schedule's timer starts; the timer is `.timer`.
pub fn schedule_unit(group: &str, name: &str) -> String {
	format!("genco-{group}-cron-{name}.service")
}

pub fn schedule_timer(group: &str, name: &str) -> String {
	format!("genco-{group}-cron-{name}.timer")
}

pub fn worker_script(group: &str, name: &str) -> PathBuf {
	PathBuf::from("/etc/genco/workers").join(format!("{group}-{name}.sh"))
}

pub fn schedule_script(group: &str, name: &str) -> PathBuf {
	PathBuf::from("/etc/genco/schedules").join(format!("{group}-{name}.sh"))
}

/// A worker waits for its first release like an app.
pub fn worker_ready(group: &str, w: &Worker) -> bool {
	group_dir(group).join(&w.dir).exists()
}

/// The PHP of the group's first PHP domain is `php` for its commands
/// (deploy steps, workers, schedules); the folder has links that
/// `genco setup` creates.
pub fn php_cli(group: &Group) -> Option<&str> {
	group
		.domains
		.iter()
		.filter(|d| d.kind == Kind::Php)
		.find_map(|d| d.php_version.as_deref())
}

pub fn php_bin_dir(version: &str) -> String {
	format!("/usr/local/lib/genco/php{version}")
}

/// PATH of a group's commands, with its PHP first.
pub fn group_path(group: &Group) -> String {
	let mut path = String::new();
	if let Some(v) = php_cli(group) {
		path.push_str(&format!("{}:", php_bin_dir(v)));
	}
	path.push_str("/usr/local/bin:/usr/bin:/bin");
	path
}

pub fn nginx_config(domain: &str) -> PathBuf {
	PathBuf::from("/etc/nginx/genco").join(format!("{domain}.conf"))
}

pub fn php_config(group: &str, version: &str) -> PathBuf {
	PathBuf::from("/etc/genco/php").join(format!("{group}-{version}.conf"))
}

fn php_socket(group: &str, version: &str) -> String {
	format!("/run/genco/{group}/php{version}/fpm.sock")
}

pub fn app_script(domain: &str) -> PathBuf {
	PathBuf::from("/etc/genco/apps").join(format!("{domain}.sh"))
}

/// The app's environment variables, root's alone (systemd reads it).
pub fn app_env(domain: &str) -> PathBuf {
	PathBuf::from("/etc/genco/apps").join(format!("{domain}.env"))
}

/// An app waits for its first release: its directory isn't there yet.
pub fn app_ready(group: &str, domain: &Domain) -> bool {
	group_dir(group).join(&domain.root).exists()
}

/// The key a group fetches its repository with (`.pub` beside it).
pub fn deploy_key_path(group: &str) -> PathBuf {
	group_dir(group).join(".ssh/deploy_key")
}

pub fn unit_path(unit: &str) -> PathBuf {
	PathBuf::from("/etc/systemd/system").join(unit)
}

pub const FIREWALL: &str = "/etc/genco/nftables.conf";

/// Above it slot A's port would be another domain's slot B, insert_domain
/// refuses such ids.
pub const MAX_DOMAIN_ID: i64 = 9999;

/// Node and Rust apps listen on 127.0.0.1 on this port, see `firewall`;
/// slot B on its own (workers can't take either, see procs).
pub fn slot_port(domain: &Domain, slot: Slot) -> u16 {
	let base = match slot {
		Slot::A => 20000,
		Slot::B => 30000,
	};
	base + domain.id as u16
}

/// The public ports of the workers, open in the firewall.
pub fn public_ports(groups: &[Group]) -> Vec<u16> {
	let mut ports: Vec<u16> = groups
		.iter()
		.flat_map(|g| &g.workers)
		.filter_map(|w| w.port)
		.collect();
	ports.sort();
	ports.dedup();
	ports
}

pub fn has_app(domain: &Domain) -> bool {
	matches!(domain.kind, Kind::Node | Kind::Rust)
}

/// The PHP versions the group's domains use.
pub fn php_versions(group: &Group) -> Vec<&str> {
	let mut versions: Vec<&str> = group
		.domains
		.iter()
		.filter_map(|d| d.php_version.as_deref())
		.collect();
	versions.sort();
	versions.dedup();
	versions
}

/// Every file of a group. `certs` are the domains that have a
/// certificate, they get https.
/// The group's SMTP login for its apps (host, user, password), once it
/// has mail domains and the mail host its certificate.
fn smtp(group: &Group) -> Option<(&str, &str, &str)> {
	if group.mail_domains.is_empty() {
		return None;
	}
	let (password, _) = group.smtp.as_ref()?;
	let host = group.mail_host.as_deref()?;
	Some((host, group.user.as_str(), password.as_str()))
}

pub fn group_files(group: &Group, certs: &HashSet<String>) -> Vec<File> {
	let g = &group.name;
	let mut files = vec![File {
		path: unit_path(&slice(g)),
		content: slice_unit(g),
		private_to: None,
	}];

	for v in php_versions(group) {
		files.push(File {
			path: php_config(g, v),
			content: php_pool(g, v, &group.php),
			private_to: Some(group_user(g)),
		});
		files.push(File {
			path: unit_path(&php_unit(g, v)),
			content: php_service(g, v),
			private_to: None,
		});
	}

	for d in &group.domains {
		files.push(File {
			path: nginx_config(&d.name),
			content: nginx_site(group, d, certs.contains(&d.name)),
			private_to: None,
		});
		if d.www {
			let www = format!("www.{}", d.name);
			let cert = certs.contains(&www);
			files.push(File {
				path: nginx_config(&www),
				content: www_redirect(&www, &d.name, cert),
				private_to: None,
			});
		}
		if has_app(d) {
			files.push(File {
				path: app_script(&d.name),
				content: format!(
					"{HEADER}: change the command in the panel\n{}\n",
					d.command.as_deref().unwrap_or_default()
				),
				private_to: Some(group_user(g)),
			});
			files.push(File {
				path: app_env(&d.name),
				content: crate::appenv::file(&d.env, smtp(group)),
				private_to: Some("root".into()),
			});
			for &slot in slots(d) {
				files.push(File {
					path: unit_path(&slot_unit(g, &d.name, slot)),
					content: app_service(group, d, slot),
					private_to: None,
				});
			}
		}
	}

	for w in &group.workers {
		files.push(File {
			path: worker_script(g, &w.name),
			content: format!("{HEADER}\n{}\n", w.command),
			private_to: Some(group_user(g)),
		});
		files.push(File {
			path: unit_path(&worker_unit(g, &w.name)),
			content: worker_service(group, w),
			private_to: None,
		});
	}
	for s in &group.schedules {
		files.push(File {
			path: schedule_script(g, &s.name),
			content: format!("{HEADER}\n{}\n", s.command),
			private_to: Some(group_user(g)),
		});
		files.push(File {
			path: unit_path(&schedule_unit(g, &s.name)),
			content: schedule_service(group, s),
			private_to: None,
		});
		files.push(File {
			path: unit_path(&schedule_timer(g, &s.name)),
			content: schedule_timer_unit(g, s),
			private_to: None,
		});
	}

	files.push(File {
		path: PathBuf::from(format!("/etc/logrotate.d/genco-{g}")),
		content: logrotate(g),
		private_to: None,
	});

	files
}

/// The group's own logs (php-fpm, PHP errors). As the group's user: the
/// directory is theirs. php-fpm and PHP append, so a copy and truncate
/// needs no reopen.
fn logrotate(g: &str) -> String {
	format!(
		"{HEADER}
{dir}/logs/*.log {{
	su {user} {user}
	daily
	missingok
	rotate 14
	compress
	delaycompress
	notifempty
	copytruncate
}}
",
		dir = group_dir(g).display(),
		user = group_user(g),
	)
}

/// The panel itself, proxied to genco's listen address.
pub fn panel_file(domain: &str, listen: &str, cert: bool) -> File {
	let body = format!(
		"\taccess_log /var/log/nginx/genco/{domain}.access.log genco_panel;
	# its own error log with only the worst: nginx writes the request with
	# its query there, the event stream's has the session token
	error_log /var/log/nginx/genco/{domain}.error.log crit;
	add_header Strict-Transport-Security \"max-age=31536000\" always;
	# uploads of the file manager
	client_max_body_size 2g;

	location / {{
		proxy_pass http://{listen};
		proxy_set_header Host $host;
		proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
		proxy_set_header X-Forwarded-Proto $scheme;
		# the event stream (SSE) must not be buffered or time out
		proxy_buffering off;
		proxy_read_timeout 1h;
	}}

	location = /api/users/login {{
		limit_req zone=genco_login burst=5 nodelay;
		proxy_pass http://{listen};
		proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
	}}
"
	);
	File {
		path: nginx_config(domain),
		content: server_blocks(domain, cert, &body),
		private_to: None,
	}
}

/// A port 80 server that answers ACME challenges, with a certificate it
/// redirects to a 443 server with `body`. 308, not 301: a POST stays a
/// POST with its body (a server-side GraphQL call to an http:// URL).
pub fn server_blocks(domain: &str, cert: bool, body: &str) -> String {
	let http = format!(
		"server {{
	listen 80;
	listen [::]:80;
	server_name {domain};

	location /.well-known/acme-challenge/ {{
		root /var/lib/genco-acme;
	}}
"
	);

	if !cert {
		return format!("{HEADER}\n{http}\n{body}}}\n");
	}

	format!(
		"{HEADER}\n{http}
	location / {{
		return 308 https://$host$request_uri;
	}}
}}

server {{
	listen 443 ssl;
	listen [::]:443 ssl;
	http2 on;
	server_name {domain};
	ssl_certificate /etc/letsencrypt/live/{domain}/fullchain.pem;
	ssl_certificate_key /etc/letsencrypt/live/{domain}/privkey.pem;

{body}}}
"
	)
}

/// www.<domain> sends every request to the domain (308 like the https
/// redirect), over https once the www name has its certificate.
fn www_redirect(www: &str, domain: &str, cert: bool) -> String {
	let body = format!(
		"\tlocation / {{\n\t\treturn 308 https://{domain}$request_uri;\n\t}}\n"
	);
	server_blocks(www, cert, &body)
}

fn nginx_site(group: &Group, d: &Domain, cert: bool) -> String {
	let root = group_dir(&group.name).join(&d.root);
	let root = root.display();
	let name = &d.name;

	let mut body = format!(
		"\taccess_log /var/log/nginx/genco/{name}.access.log;
	error_log /var/log/nginx/genco/{name}.error.log;
	client_max_body_size {max_body}m;
",
		// uploads of the sites, a bit more than PHP's own limit
		max_body = (group.php.upload_mb + 8).max(64),
	);

	// nginx may read every group's files: a link only counts when it and
	// its target have the same owner, one group can't link to another's
	// (also in the public folder of an app)
	body.push_str("\tdisable_symlinks if_not_owner;\n");

	match d.kind {
		Kind::Static => body.push_str(&format!(
			"	root {root};
	index index.html;

	location / {{
		try_files $uri $uri/ =404;
	}}

	location ~ /\\.(?!well-known) {{
		deny all;
	}}
"
		)),
		Kind::Php => body.push_str(&format!(
			"	root {root};
	index index.php index.html;

	location / {{
		try_files $uri $uri/ /index.php?$query_string;
	}}

	location ~ \\.php$ {{
		try_files $fastcgi_script_name =404;
		include fastcgi_params;
		fastcgi_param SCRIPT_FILENAME $realpath_root$fastcgi_script_name;
		fastcgi_param DOCUMENT_ROOT $realpath_root;
		fastcgi_pass unix:{};
		fastcgi_read_timeout {timeout};
	}}

	location ~ /\\.(?!well-known) {{
		deny all;
	}}
",
			php_socket(&group.name, d.php_version.as_deref().unwrap_or("")),
			timeout = (group.php.max_execution_s + 30).max(300),
		)),
		Kind::Node | Kind::Rust => body.push_str(&format!(
			"{}
	location {} {{
		proxy_pass http://127.0.0.1:{};
		proxy_http_version 1.1;
		proxy_set_header Host $host;
		proxy_set_header X-Real-IP $remote_addr;
		proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
		proxy_set_header X-Forwarded-Proto $scheme;
		proxy_set_header Upgrade $http_upgrade;
		proxy_set_header Connection $connection_upgrade;
		proxy_read_timeout 300;
	}}
",
			public_files(group, d),
			if d.public_dir.is_some() { "@app" } else { "/" },
			slot_port(d, live_slot(d))
		)),
		Kind::Proxy => {
			let upstream = d.upstream.as_deref().unwrap_or("http://127.0.0.1");
			// https upstreams get the domain's name in the handshake
			let sni = if upstream.starts_with("https://") {
				"\t\tproxy_ssl_server_name on;\n\t\tproxy_ssl_name $host;\n"
			} else {
				""
			};
			body.push_str(&format!(
				"
	location / {{
		proxy_pass {upstream};
		proxy_http_version 1.1;
		proxy_set_header Host $host;
		proxy_set_header X-Real-IP $remote_addr;
		proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
		proxy_set_header X-Forwarded-Proto $scheme;
		proxy_set_header Upgrade $http_upgrade;
		proxy_set_header Connection $connection_upgrade;
		proxy_read_timeout 300;
{sni}	}}
"
			));
		}
	}

	let extra = d.nginx_extra.trim();
	if !extra.is_empty() {
		body.push_str("\n\t# extra config of the domain\n");
		for line in extra.lines() {
			body.push_str(&format!("\t{line}\n"));
		}
	}

	server_blocks(name, cert, &body)
}

/// nginx serves the files of the public folder itself, everything else
/// goes to the app.
fn public_files(group: &Group, d: &Domain) -> String {
	let Some(dir) = &d.public_dir else {
		return String::new();
	};
	format!(
		"
	root {}/{dir};

	location / {{
		try_files $uri @app;
	}}

	location ~ /\\.(?!well-known) {{
		deny all;
	}}
",
		group_dir(&group.name).display()
	)
}

fn slice_unit(g: &str) -> String {
	format!(
		"{HEADER}
[Unit]
Description=Limits of the group {g}

[Slice]
MemoryHigh=1G
MemoryMax=1536M
TasksMax=512
CPUQuota=150%
"
	)
}

/// Runs as the group's user and only sees its own files. `bind` is the
/// one port it may listen on: a free port of another app or of the panel
/// would otherwise be open to any group while its owner restarts.
/// genco's own state, the groups' processes never see it (`-`: fine if
/// it doesn't exist)
pub const HIDDEN: &[&str] = &[
	"-/var/lib/genco",
	"-/var/backups/genco",
	"-/etc/genco/config.toml",
	"-/etc/genco/secrets.key",
	"-/etc/genco/restic-password",
	"-/etc/genco/mail",
	"-/etc/genco/dkim",
];

fn sandbox(g: &str, bind: Option<u16>) -> String {
	let bind = match bind {
		Some(port) => format!("SocketBindAllow=tcp:{port}\n"),
		None => String::new(),
	};
	format!(
		"{bind}SocketBindDeny=any
User={user}
Group={user}
Slice={slice}
UMask=0027
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths={dir}
InaccessiblePaths={hidden}
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
",
		user = group_user(g),
		slice = slice(g),
		dir = group_dir(g).display(),
		hidden = HIDDEN.join(" "),
	)
}

fn php_pool(g: &str, v: &str, php: &PhpSettings) -> String {
	let dir = group_dir(g);
	let dir = dir.display();
	// ini files comment with ;
	let header = HEADER.replacen('#', ";", 1);
	format!(
		"{header}
[global]
error_log = {dir}/logs/php{v}-fpm.log

[{g}]
listen = {sock}
listen.mode = 0660
pm = ondemand
pm.max_children = 10
pm.process_idle_timeout = 60s
catch_workers_output = yes
php_admin_value[open_basedir] = {dir}/:/tmp/:/usr/share/php/
php_admin_value[session.save_path] = {dir}/tmp
php_admin_value[upload_tmp_dir] = {dir}/tmp
php_admin_value[error_log] = {dir}/logs/php-error.log
php_admin_flag[log_errors] = on
php_admin_value[memory_limit] = {memory}M
php_admin_value[upload_max_filesize] = {upload}M
php_admin_value[post_max_size] = {post}M
php_admin_value[max_execution_time] = {time}
php_admin_value[max_input_vars] = {vars}
php_admin_value[sendmail_path] = \"/usr/bin/msmtp --host=127.0.0.1 --port=25 --read-envelope-from -t\"
",
		sock = php_socket(g, v),
		memory = php.memory_mb,
		upload = php.upload_mb,
		post = php.upload_mb + 8,
		time = php.max_execution_s,
		vars = php.max_input_vars,
	)
}

fn php_service(g: &str, v: &str) -> String {
	format!(
		"{HEADER}
[Unit]
Description=PHP {v} of the group {g}
After=network.target

[Service]
Type=notify
ExecStart=/usr/sbin/php-fpm{v} --nodaemonize --fpm-config {conf}
ExecReload=/bin/kill -USR2 $MAINPID
RuntimeDirectory=genco/{g}/php{v}
RuntimeDirectoryMode=0750
Restart=on-failure
{sandbox}
[Install]
WantedBy=multi-user.target
",
		conf = php_config(g, v).display(),
		sandbox = sandbox(g, None),
	)
}

fn app_service(group: &Group, d: &Domain, slot: Slot) -> String {
	let g = &group.name;
	let port = slot_port(d, slot);
	let kind = if d.kind == Kind::Node { "Node" } else { "Rust" };
	let node = if d.kind == Kind::Node {
		"Environment=NODE_ENV=production\n"
	} else {
		""
	};
	format!(
		"{HEADER}
[Unit]
Description={kind} app {name} of the group {g}
After=network.target mariadb.service

[Service]
WorkingDirectory={dir}
EnvironmentFile={env}
Environment=HOST=127.0.0.1 PORT={port}
{node}ExecStart=/bin/sh {script}
Restart=always
RestartSec=3
{sandbox}
[Install]
WantedBy=multi-user.target
",
		name = d.name,
		dir = group_dir(g).join(&d.root).display(),
		script = app_script(&d.name).display(),
		env = app_env(&d.name).display(),
		sandbox = sandbox(g, Some(port)),
	)
}

/// Environment of a worker or a schedule: the group's PHP first in PATH,
/// its directory as HOME.
fn group_env(group: &Group) -> String {
	format!(
		"Environment=PATH={} HOME={}\n",
		group_path(group),
		group_dir(&group.name).display()
	)
}

fn worker_service(group: &Group, w: &Worker) -> String {
	let g = &group.name;
	let port = match w.port {
		Some(p) => format!("Environment=HOST=0.0.0.0 PORT={p}\n"),
		None => String::new(),
	};
	format!(
		"{HEADER}
[Unit]
Description=Worker {name} of the group {g}
After=network.target mariadb.service postgresql.service
# one that only crashes fails after a while, the minute check reports it
StartLimitIntervalSec=10min
StartLimitBurst=20

[Service]
WorkingDirectory={dir}
{env}{port}ExecStart=/bin/sh {script}
Restart=always
RestartSec=5
{sandbox}
[Install]
WantedBy=multi-user.target
",
		name = w.name,
		dir = group_dir(g).join(&w.dir).display(),
		env = group_env(group),
		script = worker_script(g, &w.name).display(),
		sandbox = sandbox(g, w.port),
	)
}

fn schedule_service(group: &Group, s: &Schedule) -> String {
	let g = &group.name;
	format!(
		"{HEADER}
[Unit]
Description=Schedule {name} of the group {g}
After=network.target mariadb.service postgresql.service

[Service]
Type=oneshot
WorkingDirectory={dir}
{env}ExecStart=/bin/sh {script}
TimeoutStartSec={timeout}min
{sandbox}",
		name = s.name,
		dir = group_dir(g).join(&s.dir).display(),
		env = group_env(group),
		script = schedule_script(g, &s.name).display(),
		timeout = s.timeout,
		sandbox = sandbox(g, None),
	)
}

fn schedule_timer_unit(g: &str, s: &Schedule) -> String {
	format!(
		"{HEADER}
[Unit]
Description=Timer of the schedule {name} of the group {g}

[Timer]
OnCalendar={every}
AccuracySec=1s

[Install]
WantedBy=timers.target
",
		name = s.name,
		every = s.every,
	)
}

/// Only nginx, root and the app's own group may connect to an app's
/// port, a port on localhost is open to every user otherwise.
/// Local services the groups' processes may not talk to: the panel (its
/// login limit is in nginx), Redis, Rspamd and postsrsd.
const PRIVATE_PORTS: &[u16] = &[6379, 10001, 10002, 11332, 11333, 11334];

/// What blocked addresses can't reach besides the workers' public ports:
/// the sites and the mail. Never ssh, an admin who blocked themselves gets
/// in to unblock.
const BLOCKED_PORTS: &[u16] = &[80, 443, 25, 465, 587, 993];

/// `blocked` are the addresses and ranges of the blocklist (firewall.rs).
pub fn firewall(
	groups: &[Group],
	panel_port: u16,
	blocked: &[String],
) -> String {
	// the input chain of `genco setup` accepts the ports in this set
	let ports = public_ports(groups);
	let mut public = "add set inet filter genco_public { type inet_service; }
flush set inet filter genco_public
"
	.to_string();
	if !ports.is_empty() {
		let list: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
		public.push_str(&format!(
			"add element inet filter genco_public {{ {} }}\n",
			list.join(", ")
		));
	}
	let mut rules = String::new();
	if !groups.is_empty() {
		let users: Vec<String> =
			groups.iter().map(|g| group_user(&g.name)).collect();
		let ports: Vec<String> = std::iter::once(panel_port)
			.chain(PRIVATE_PORTS.iter().copied())
			.map(|p| p.to_string())
			.collect();
		for (family, local) in [("ip", "127.0.0.0/8"), ("ip6", "::1")] {
			rules.push_str(&format!(
				"\t\tmeta skuid {{ {} }} {family} daddr {local} \
				tcp dport {{ {} }} reject with tcp reset\n",
				users.join(", "),
				ports.join(", "),
			));
		}
	}
	for g in groups {
		for d in g.domains.iter().filter(|d| has_app(d)) {
			for &slot in slots(d) {
				// nft resolves the user names when it loads the file
				rules.push_str(&format!(
					"\t\ttcp dport {} meta skuid != {{ 0, www-data, {} }} \
					reject with tcp reset\n",
					slot_port(d, slot),
					group_user(&g.name),
				));
			}
		}
	}

	let mut sets = String::new();
	for (name, kind, v6) in [
		("blocked4", "ipv4_addr", false),
		("blocked6", "ipv6_addr", true),
	] {
		let list: Vec<&str> = blocked
			.iter()
			.filter(|a| a.contains(':') == v6)
			.map(String::as_str)
			.collect();
		let elements = if list.is_empty() {
			String::new()
		} else {
			format!("\t\telements = {{ {} }}\n", list.join(", "))
		};
		sets.push_str(&format!(
			"\tset {name} {{\n\t\ttype {kind}; flags interval; auto-merge;\n\
			{elements}\t}}\n"
		));
	}
	// the ports, never all of them: ssh stays open
	let dports: Vec<String> = BLOCKED_PORTS
		.iter()
		.copied()
		.chain(ports.iter().copied())
		.map(|p| p.to_string())
		.collect();
	let dports = dports.join(", ");

	format!(
		"{HEADER}
{public}
table inet genco
delete table inet genco

table inet genco {{
{sets}
	# before the input chain of `genco setup` accepts them
	chain input {{
		type filter hook input priority filter - 1; policy accept;
		ip saddr @blocked4 tcp dport {{ {dports} }} drop
		ip6 saddr @blocked6 tcp dport {{ {dports} }} drop
	}}
	chain output {{
		type filter hook output priority filter; policy accept;
{rules}	}}
}}
"
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::Path;

	fn domain(id: i64, name: &str, kind: Kind) -> Domain {
		Domain {
			id,
			group_id: 1,
			name: name.into(),
			kind,
			php_version: (kind == Kind::Php).then(|| "8.4".into()),
			root: "public".into(),
			command: matches!(kind, Kind::Node | Kind::Rust)
				.then(|| "node build/index.js".into()),
			nginx_extra: String::new(),
			env: vec![],
			public_dir: None,
			upstream: (kind == Kind::Proxy)
				.then(|| "http://127.0.0.1:8081".into()),
			www: false,
			zero_downtime: false,
			slot_b: false,
		}
	}

	fn group(domains: Vec<Domain>) -> Group {
		Group {
			id: 1,
			name: "shop".into(),
			client_id: 1,
			user: "g-shop".into(),
			dir: "/srv/genco/shop".into(),
			branch: "main".into(),
			domains,
			..Group::default()
		}
	}

	fn find<'a>(files: &'a [File], path: &str) -> &'a str {
		&files
			.iter()
			.find(|f| f.path == Path::new(path))
			.unwrap_or_else(|| panic!("no file {path}"))
			.content
	}

	#[test]
	fn php_domain_gets_a_pool_and_a_unit() {
		let g = group(vec![domain(1, "admin.shop.ch", Kind::Php)]);
		let files = group_files(&g, &HashSet::new());

		let site = find(&files, "/etc/nginx/genco/admin.shop.ch.conf");
		assert!(site.contains("root /srv/genco/shop/public;"));
		assert!(site.contains("unix:/run/genco/shop/php8.4/fpm.sock"));
		assert!(!site.contains("443"));

		let unit =
			find(&files, "/etc/systemd/system/genco-shop-php8.4.service");
		assert!(unit.contains("User=g-shop"));
		assert!(unit.contains("ReadWritePaths=/srv/genco/shop"));
		assert!(unit.contains("/usr/sbin/php-fpm8.4"));
		assert!(unit.contains("SocketBindDeny=any"));
		assert!(!unit.contains("SocketBindAllow"));
		assert!(site.contains("disable_symlinks if_not_owner;"));
	}

	#[test]
	fn group_logs_are_rotated_as_the_group() {
		let g = group(vec![domain(1, "shop.ch", Kind::Php)]);
		let files = group_files(&g, &HashSet::new());

		let conf = find(&files, "/etc/logrotate.d/genco-shop");
		assert!(conf.contains("/srv/genco/shop/logs/*.log {"));
		assert!(conf.contains("su g-shop g-shop"));
		assert!(conf.contains("rotate 14"));
		assert!(conf.contains("copytruncate"));
	}

	#[test]
	fn certificate_adds_https_and_redirect() {
		let g = group(vec![domain(1, "shop.ch", Kind::Static)]);
		let certs = HashSet::from(["shop.ch".to_string()]);
		let site = &group_files(&g, &certs)[1].content;

		assert!(site.contains("return 308 https://"));
		assert!(site.contains("listen 443 ssl;"));
		assert!(site.contains("/etc/letsencrypt/live/shop.ch/fullchain.pem"));
		// the challenges still work over http
		assert!(site.contains("acme-challenge"));
	}

	#[test]
	fn www_redirects_to_the_domain_with_its_own_certificate() {
		let mut d = domain(1, "shop.ch", Kind::Static);
		d.www = true;
		let g = group(vec![d]);

		let files = group_files(&g, &HashSet::new());
		let www = find(&files, "/etc/nginx/genco/www.shop.ch.conf");
		assert!(www.contains("server_name www.shop.ch;"));
		assert!(www.contains("return 308 https://shop.ch$request_uri;"));
		assert!(www.contains("acme-challenge"));
		assert!(!www.contains("listen 443"));

		let certs = HashSet::from(["www.shop.ch".to_string()]);
		let files = group_files(&g, &certs);
		let www = find(&files, "/etc/nginx/genco/www.shop.ch.conf");
		assert!(www.contains("/etc/letsencrypt/live/www.shop.ch/"));
		// the domain itself has no certificate yet
		let site = find(&files, "/etc/nginx/genco/shop.ch.conf");
		assert!(!site.contains("listen 443"));
	}

	#[test]
	fn node_app_gets_a_port_and_a_firewall_rule() {
		let g = group(vec![domain(5, "shop.ch", Kind::Node)]);
		let files = group_files(&g, &HashSet::new());

		let site = find(&files, "/etc/nginx/genco/shop.ch.conf");
		assert!(site.contains("proxy_pass http://127.0.0.1:20005;"));
		let unit =
			find(&files, "/etc/systemd/system/genco-shop-shop.ch.service");
		assert!(unit.contains("PORT=20005"));
		assert!(unit.contains("WorkingDirectory=/srv/genco/shop/public"));
		assert!(unit.contains("Slice=genco-shop.slice"));
		assert!(unit.contains("SocketBindAllow=tcp:20005\nSocketBindDeny=any"));
		assert!(
			find(&files, "/etc/genco/apps/shop.ch.sh")
				.contains("node build/index.js")
		);

		let fw = firewall(&[g], 4100, &[]);
		assert!(
			fw.contains(
				"tcp dport 20005 meta skuid != { 0, www-data, g-shop }"
			)
		);
		// the panel and the mail services are no business of the groups
		assert!(fw.contains(
			"meta skuid { g-shop } ip daddr 127.0.0.0/8 tcp dport { 4100, 6379"
		));
	}

	#[test]
	fn zero_downtime_adds_slot_b_and_nginx_follows_the_live_slot() {
		let mut d = domain(5, "shop.ch", Kind::Node);
		d.zero_downtime = true;
		let g = group(vec![d.clone()]);
		let files = group_files(&g, &HashSet::new());

		let site = find(&files, "/etc/nginx/genco/shop.ch.conf");
		assert!(site.contains("proxy_pass http://127.0.0.1:20005;"));
		let a = find(&files, "/etc/systemd/system/genco-shop-shop.ch.service");
		assert!(a.contains("PORT=20005"));
		let b =
			find(&files, "/etc/systemd/system/genco-shop-shop.ch_b.service");
		assert!(b.contains("PORT=30005"));
		assert!(b.contains("SocketBindAllow=tcp:30005\nSocketBindDeny=any"));
		let fw = firewall(std::slice::from_ref(&g), 4100, &[]);
		assert!(fw.contains("tcp dport 20005 meta skuid"));
		assert!(fw.contains("tcp dport 30005 meta skuid"));

		d.slot_b = true;
		let files = group_files(&group(vec![d.clone()]), &HashSet::new());
		let site = find(&files, "/etc/nginx/genco/shop.ch.conf");
		assert!(site.contains("proxy_pass http://127.0.0.1:30005;"));

		// switched off, slot B is gone and nginx is back at A
		d.zero_downtime = false;
		let g = group(vec![d]);
		let files = group_files(&g, &HashSet::new());
		let site = find(&files, "/etc/nginx/genco/shop.ch.conf");
		assert!(site.contains("proxy_pass http://127.0.0.1:20005;"));
		assert!(!files.iter().any(|f| f.path.ends_with("shop.ch_b.service")));
		assert!(!firewall(&[g], 4100, &[]).contains("30005"));
	}

	#[test]
	fn restart_switches_slots_only_with_zero_downtime() {
		let mut d = domain(5, "shop.ch", Kind::Rust);
		assert_eq!(
			restart("shop", &d),
			Restart::InPlace {
				unit: "genco-shop-shop.ch.service".into()
			}
		);

		d.zero_downtime = true;
		assert_eq!(
			restart("shop", &d),
			Restart::Switch {
				start: "genco-shop-shop.ch_b.service".into(),
				port: 30005,
				to: Slot::B,
				stop: "genco-shop-shop.ch.service".into(),
			}
		);
		d.slot_b = true;
		assert_eq!(
			restart("shop", &d),
			Restart::Switch {
				start: "genco-shop-shop.ch.service".into(),
				port: 20005,
				to: Slot::A,
				stop: "genco-shop-shop.ch_b.service".into(),
			}
		);
	}

	#[test]
	fn php_settings_reach_the_pool_and_nginx() {
		let mut g = group(vec![domain(1, "admin.shop.ch", Kind::Php)]);
		g.php.upload_mb = 200;
		g.php.max_execution_s = 600;
		let files = group_files(&g, &HashSet::new());

		let pool = find(&files, "/etc/genco/php/shop-8.4.conf");
		assert!(pool.contains("upload_max_filesize] = 200M"));
		assert!(pool.contains("post_max_size] = 208M"));
		assert!(pool.contains("max_execution_time] = 600"));
		let site = find(&files, "/etc/nginx/genco/admin.shop.ch.conf");
		assert!(site.contains("client_max_body_size 208m;"));
		assert!(site.contains("fastcgi_read_timeout 630;"));
	}

	#[test]
	fn public_folder_is_served_by_nginx_before_the_app() {
		let mut d = domain(5, "shop.ch", Kind::Node);
		d.public_dir = Some("current/svelte/dist/public".into());
		let g = group(vec![d]);
		let files = group_files(&g, &HashSet::new());

		let site = find(&files, "/etc/nginx/genco/shop.ch.conf");
		assert!(
			site.contains("root /srv/genco/shop/current/svelte/dist/public;")
		);
		assert!(site.contains("try_files $uri @app;"));
		assert!(site.contains("location @app {"));
		assert!(site.contains("proxy_pass http://127.0.0.1:20005;"));
	}

	#[test]
	fn proxy_forwards_with_the_host() {
		let g = group(vec![domain(1, "old.shop.ch", Kind::Proxy)]);
		let files = group_files(&g, &HashSet::new());
		let site = find(&files, "/etc/nginx/genco/old.shop.ch.conf");
		assert!(site.contains("proxy_pass http://127.0.0.1:8081;"));
		assert!(site.contains("proxy_set_header Host $host;"));
		assert!(!site.contains("proxy_ssl_server_name"));
		assert!(!site.contains("root /srv"));
	}

	#[test]
	fn workers_and_schedules_get_units() {
		let mut g = group(vec![domain(1, "admin.shop.ch", Kind::Php)]);
		g.workers = vec![
			Worker {
				id: 1,
				group_id: 1,
				name: "queue".into(),
				dir: "current/craft".into(),
				command: "php craft queue/listen --verbose".into(),
				port: None,
				source: "repo".into(),
			},
			Worker {
				id: 2,
				group_id: 1,
				name: "packages".into(),
				dir: "current".into(),
				command: "./packages-server".into(),
				port: Some(5426),
				source: "panel".into(),
			},
		];
		g.schedules = vec![Schedule {
			id: 1,
			group_id: 1,
			name: "statuses".into(),
			dir: "current/craft".into(),
			command: "php craft update-statuses".into(),
			every: "*:0/5".into(),
			timeout: 10,
			source: "repo".into(),
		}];
		let files = group_files(&g, &HashSet::new());

		let q = find(
			&files,
			"/etc/systemd/system/genco-shop-worker-queue.service",
		);
		assert!(q.contains("WorkingDirectory=/srv/genco/shop/current/craft"));
		assert!(q.contains("PATH=/usr/local/lib/genco/php8.4:/usr/local/bin"));
		assert!(q.contains("User=g-shop"));
		assert!(q.contains("SocketBindDeny=any"));
		assert!(!q.contains("SocketBindAllow"));
		assert!(
			find(&files, "/etc/genco/workers/shop-queue.sh")
				.contains("php craft queue/listen --verbose")
		);

		let p = find(
			&files,
			"/etc/systemd/system/genco-shop-worker-packages.service",
		);
		assert!(p.contains("PORT=5426"));
		assert!(p.contains("SocketBindAllow=tcp:5426"));

		let s = find(
			&files,
			"/etc/systemd/system/genco-shop-cron-statuses.service",
		);
		assert!(s.contains("Type=oneshot"));
		assert!(s.contains("TimeoutStartSec=10min"));
		let t =
			find(&files, "/etc/systemd/system/genco-shop-cron-statuses.timer");
		assert!(t.contains("OnCalendar=*:0/5"));

		let fw = firewall(&[g], 4100, &[]);
		assert!(fw.contains("add element inet filter genco_public { 5426 }"));
		assert!(fw.contains("flush set inet filter genco_public"));
	}

	#[test]
	fn blocked_addresses_lose_the_web_mail_and_worker_ports_not_ssh() {
		let mut g = group(vec![]);
		g.workers = vec![Worker {
			id: 2,
			group_id: 1,
			name: "packages".into(),
			dir: "current".into(),
			command: "./packages-server".into(),
			port: Some(5426),
			source: "panel".into(),
		}];
		let blocked = [
			"198.51.100.0/24".to_string(),
			"203.0.113.7".into(),
			"2001:db8::/48".into(),
		];
		let fw = firewall(&[g], 4100, &blocked);

		assert!(fw.contains(
			"set blocked4 {\n\t\ttype ipv4_addr; flags interval; auto-merge;\n\
			\t\telements = { 198.51.100.0/24, 203.0.113.7 }\n"
		));
		assert!(fw.contains("elements = { 2001:db8::/48 }"));
		let drops: Vec<&str> =
			fw.lines().filter(|l| l.ends_with(" drop")).collect();
		assert_eq!(drops.len(), 2);
		for rule in drops {
			assert!(
				rule.contains("tcp dport { 80, 443, 25, 465, 587, 993, 5426 }"),
				"{rule}"
			);
			assert!(!rule.contains(" 22,") && !rule.contains(" 22 "));
		}

		// empty sets have no elements, nft refuses `elements = { }`
		let fw = firewall(&[], 4100, &[]);
		assert!(fw.contains("set blocked6 {"));
		assert!(!fw.contains("elements"));
	}

	#[test]
	fn extra_nginx_lines_are_indented_into_the_server() {
		let mut d = domain(1, "shop.ch", Kind::Static);
		d.nginx_extra = "add_header X-Test 1;\nadd_header X-Two 2;".into();
		let site = &group_files(&group(vec![d]), &HashSet::new())[1].content;
		assert!(
			site.contains("\tadd_header X-Test 1;\n\tadd_header X-Two 2;\n}")
		);
	}
}
