//! The files mail runs on: Postfix's maps and settings, Dovecot's whole
//! config, the groups' SMTP logins.

use super::*;

pub(super) fn map_file(name: &str, lines: Vec<String>) -> File {
	let mut content =
		"# written by genco, changes here are overwritten\n".to_string();
	for l in lines {
		content.push_str(&l);
		content.push('\n');
	}
	File {
		path: PathBuf::from(MAPS).join(name),
		content,
		private_to: None,
	}
}

/// Postfix's maps, Dovecot's users and config, the mail host's nginx site.
/// `cert`: the mail host has a certificate.
/// `smtpd_sender_login_maps`: who may send as an address. A mailbox or
/// send-only address itself, and the SMTP login of its group for every
/// address of the group's mail domains. Postfix looks up the whole
/// address before @domain, so its line names the group's login too.
pub(super) fn sender_logins(groups: &[Group]) -> Vec<String> {
	let mut lines = vec![];
	for g in groups {
		let login = g.smtp.as_ref().map(|_| smtp_user(&g.name));
		for d in &g.mail_domains {
			let own = d.mailboxes.iter().map(|b| &b.address);
			let send_only = d.send_only_addresses.iter().map(|s| &s.address);
			for address in own.chain(send_only) {
				match &login {
					Some(l) => lines.push(format!("{address} {address},{l}")),
					None => lines.push(format!("{address} {address}")),
				}
			}
			if let Some(l) = &login {
				lines.push(format!("@{} {l}", d.name));
			}
		}
	}
	lines
}

/// `docker`: Docker is installed (`docker_installed`).
pub fn files(
	cfg: &Config,
	groups: &[Group],
	cert: bool,
	docker: bool,
) -> Vec<File> {
	let domains: Vec<&MailDomain> = domains(groups).collect();
	let boxes = || domains.iter().flat_map(|d| &d.mailboxes);

	let mut forwards: BTreeMap<String, Vec<String>> = BTreeMap::new();
	// a mailbox maps to itself: Postfix looks up forwards first, a
	// catch-all (@domain) would take its mail otherwise
	for b in boxes() {
		forwards.insert(b.address.clone(), vec![b.address.clone()]);
	}
	for d in &domains {
		for f in &d.forwards {
			// the catch-all of postfix is written @domain
			let from = f.address.strip_prefix('*').unwrap_or(&f.address);
			forwards.insert(from.to_string(), f.targets.clone());
		}
	}

	// a send-only domain's mail goes out to its own MX, also from here
	let receiving = domains.iter().filter(|d| !d.send_only);
	let mut files = vec![
		map_file(
			"domains",
			receiving.map(|d| format!("{} OK", d.name)).collect(),
		),
		map_file(
			"mailboxes",
			boxes().map(|b| format!("{0} {0}", b.address)).collect(),
		),
		// a mailbox and a send-only address send as themselves only, a
		// group's SMTP login as any address of the group's mail domains
		map_file("senders", sender_logins(groups)),
		// Postfix checks no owner for the null sender: on the submission
		// ports a login could send bounces as anyone
		map_file(
			"null-sender",
			vec!["<> REJECT a login sends with its own sender address".into()],
		),
		map_file(
			"forwards",
			forwards
				.iter()
				.map(|(from, to)| format!("{from} {}", to.join(",")))
				.collect(),
		),
	];

	let mut passwd =
		"# written by genco, changes here are overwritten\n".to_string();
	for b in boxes() {
		passwd.push_str(&format!(
			"{}:{}::::::userdb_quota_storage_size={}M\n",
			b.address, b.password_hash, b.quota_mb
		));
	}
	files.push(File {
		path: PathBuf::from(MAPS).join("passwd"),
		content: passwd,
		private_to: Some("dovecot".into()),
	});

	// the groups' SMTP logins and the send-only addresses: a passdb
	// without a userdb, they can send but never open a mailbox
	let mut smtp =
		"# written by genco, changes here are overwritten\n".to_string();
	// a group in quarantine sends nothing (quarantine.rs)
	let quarantined = |group_id: i64| {
		groups
			.iter()
			.any(|g| g.id == group_id && g.quarantine.is_some())
	};
	for d in domains.iter().filter(|d| !quarantined(d.group_id)) {
		for s in &d.send_only_addresses {
			smtp.push_str(&format!("{}:{}\n", s.address, s.password_hash));
		}
	}
	for g in groups.iter().filter(|g| !g.mail_domains.is_empty()) {
		let Some((password, hash)) = &g.smtp else {
			continue;
		};
		if g.quarantine.is_none() {
			smtp.push_str(&format!("{}:{hash}\n", smtp_user(&g.name)));
		}
		files.push(File {
			path: smtp_password_path(&g.name),
			content: format!("{password}\n"),
			private_to: Some(crate::render::group_user(&g.name)),
		});
	}
	files.push(File {
		path: PathBuf::from(MAPS).join("smtp-passwd"),
		content: smtp,
		private_to: Some("dovecot".into()),
	});

	let host = host(cfg);
	let imap = cert && !domains.is_empty();
	files.push(File {
		path: DOVECOT_CONF.into(),
		content: dovecot_conf(host.as_deref().filter(|_| imap)),
		private_to: None,
	});
	if let Some(host) = host.as_deref().filter(|_| !domains.is_empty()) {
		files.extend(crate::webmail::files(host, cert));
	}
	files.extend(crate::mailfilter::files(cfg, groups));
	// Postfix may listen on docker0, which Docker makes at boot; without
	// Docker apply removes it again (`postfix`)
	if docker {
		files.push(File {
			path: POSTFIX_AFTER_DOCKER.into(),
			content: "# written by genco, changes here are overwritten
[Unit]
Wants=docker.service
After=docker.service
"
			.into(),
			private_to: None,
		});
	}
	files
}

pub const POSTFIX_AFTER_DOCKER: &str =
	"/etc/systemd/system/postfix.service.d/genco-docker.conf";

/// Docker is installed: its unit and dockerd exist, and the unit isn't
/// masked (`systemctl mask` links it to /dev/null, it can't start). A
/// purge without a reboot leaves docker0 and the br-* bridges behind.
fn docker_installed_with(
	exists: impl Fn(&str) -> bool,
	masked: impl Fn(&str) -> bool,
) -> bool {
	let unit = [
		"/etc/systemd/system/docker.service",
		"/run/systemd/system/docker.service",
		"/lib/systemd/system/docker.service",
		"/usr/lib/systemd/system/docker.service",
	];
	let dockerd = ["/usr/bin/dockerd", "/usr/sbin/dockerd"];
	!unit.into_iter().any(&masked)
		&& unit.into_iter().any(&exists)
		&& dockerd.into_iter().any(&exists)
}

pub fn docker_installed() -> bool {
	docker_installed_with(
		|p| std::path::Path::new(p).exists(),
		|p| {
			let to = std::fs::read_link(p);
			to.is_ok_and(|to| to.as_os_str() == "/dev/null")
		},
	)
}

/// A Docker bridge on this server: docker0 (host.docker.internal) or a
/// compose network's br-*.
pub struct Bridge {
	pub name: String,
	pub addr: std::net::Ipv4Addr,
	/// its network, e.g. 172.17.0.0/16
	pub net: crate::firewall::Net,
}

/// The Docker bridges of `ip -4 -o addr show`. Only private networks,
/// they go into mynetworks and may relay.
fn parse_bridges(out: &str) -> Vec<Bridge> {
	let mut bridges = vec![];
	for line in out.lines() {
		let mut words = line.split_whitespace().skip(1);
		let (Some(name), Some("inet"), Some(cidr)) =
			(words.next(), words.next(), words.next())
		else {
			continue;
		};
		if name != "docker0" && !name.starts_with("br-") {
			continue;
		}
		let Some((addr, prefix)) = cidr.split_once('/') else {
			continue;
		};
		let (Ok(addr), Ok(prefix), Ok(net)) = (
			addr.parse::<std::net::Ipv4Addr>(),
			prefix.parse::<u8>(),
			crate::firewall::Net::parse(cidr),
		) else {
			continue;
		};
		// within 10.0.0.0/8, 172.16.0.0/12 or 192.168.0.0/16
		let min = match addr.octets()[0] {
			10 => 8,
			172 => 12,
			_ => 16,
		};
		if addr.is_private() && prefix >= min {
			bridges.push(Bridge {
				name: name.into(),
				addr,
				net,
			});
		}
	}
	bridges
}

/// The Docker bridges that exist now, none without Docker. Podman's
/// (podman0, cni-podman0) are never Docker's.
pub async fn docker_bridges() -> Vec<Bridge> {
	if !docker_installed() {
		return vec![];
	}
	let out = tokio::process::Command::new("ip")
		.args(["-4", "-o", "addr", "show"])
		.output()
		.await;
	let out = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string());
	parse_bridges(&out.unwrap_or_default())
}

/// Dovecot's whole config: only genco's mailboxes log in, never the
/// server's users. Without a certificate it only delivers (LMTP).
pub(super) fn dovecot_conf(host: Option<&str>) -> String {
	let ssl = match host {
		Some(h) => format!(
			"protocols = imap lmtp
ssl = required
ssl_server_cert_file = /etc/letsencrypt/live/{h}/fullchain.pem
ssl_server_key_file = /etc/letsencrypt/live/{h}/privkey.pem
ssl_min_protocol = TLSv1.2

service imap-login {{
	# IMAP only with TLS from the start, port 993
	inet_listener imap {{
		port = 0
	}}
}}
"
		),
		None => "protocols = lmtp\nssl = no\n".into(),
	};
	format!(
		"# written by genco, changes here are overwritten
dovecot_config_version = 2.4.0
dovecot_storage_version = 2.4.0
!include_try /usr/share/dovecot/protocols.d/*.protocol

{ssl}
mail_driver = maildir
mail_home = {VMAIL}/%{{user | domain}}/%{{user | username}}
mail_path = ~/Maildir
mail_inbox_path =

auth_mechanisms = plain login
auth_username_format = %{{user | lower}}

passdb passwd-file {{
	passwd_file_path = {MAPS}/passwd
}}

# the groups' SMTP logins and the send-only addresses, no userdb: no
# IMAP for them
passdb smtp {{
	driver = passwd-file
	passwd_file_path = {MAPS}/smtp-passwd
}}

userdb passwd-file {{
	passwd_file_path = {MAPS}/passwd
	fields {{
		uid = vmail
		gid = vmail
		home = {VMAIL}/%{{user | domain}}/%{{user | username}}
	}}
}}

mail_plugins {{
	quota = yes
}}

quota \"User quota\" {{
}}

protocol imap {{
	mail_plugins {{
		imap_quota = yes
	}}
}}

namespace inbox {{
	inbox = yes
	mailbox Drafts {{
		special_use = \\Drafts
		auto = subscribe
	}}
	mailbox Junk {{
		special_use = \\Junk
		auto = subscribe
	}}
	mailbox Trash {{
		special_use = \\Trash
		auto = subscribe
	}}
	mailbox Sent {{
		special_use = \\Sent
		auto = subscribe
	}}
}}

service lmtp {{
	unix_listener /var/spool/postfix/private/dovecot-lmtp {{
		mode = 0600
		user = postfix
		group = postfix
	}}
}}

service auth {{
	unix_listener /var/spool/postfix/private/auth {{
		mode = 0660
		user = postfix
		group = postfix
	}}
}}
{sieve}",
		sieve = crate::mailfilter::dovecot_sieve()
	)
}

/// Postfix's main.cf values for mail. Without mail domains it listens
/// only on localhost and docker0, where containers send through
/// host.docker.internal; the Docker bridges' networks may relay.
pub fn postfix_settings(
	cfg: &Config,
	groups: &[Group],
	cert: bool,
	bridges: &[Bridge],
) -> Vec<(&'static str, String)> {
	let on = enabled(groups);
	let host = host(cfg).unwrap_or_else(|| "localhost".into());
	let panel = cfg.domain.clone().unwrap_or_else(|| "localhost".into());
	let docker0 = bridges.iter().find(|b| b.name == "docker0");
	let interfaces = match docker0 {
		_ if on => "all".to_string(),
		// loopback-only can't be combined, localhost is 127.0.0.1 and ::1
		Some(b) => format!("localhost, {}", b.addr),
		None => "loopback-only".to_string(),
	};
	let mut networks =
		"127.0.0.0/8 [::ffff:127.0.0.0]/104 [::1]/128".to_string();
	for b in bridges {
		networks.push(' ');
		networks.push_str(&b.net.to_string());
	}
	let mut s = vec![
		("inet_interfaces", interfaces),
		("mynetworks", networks),
		("myhostname", if on { host.clone() } else { panel }),
		(
			"virtual_mailbox_domains",
			format!("texthash:{MAPS}/domains"),
		),
		("virtual_mailbox_maps", format!("texthash:{MAPS}/mailboxes")),
		("virtual_alias_maps", format!("texthash:{MAPS}/forwards")),
		(
			"virtual_transport",
			"lmtp:unix:private/dovecot-lmtp".to_string(),
		),
		(
			"smtpd_sender_login_maps",
			format!("texthash:{MAPS}/senders"),
		),
		("disable_vrfy_command", "yes".into()),
		// logins only on the submission ports (they turn it on for
		// themselves); an adopted server's old main.cf may ask Dovecot,
		// which may be off, and break port 25
		("smtpd_sasl_auth_enable", "no".into()),
		("smtpd_helo_required", "yes".into()),
		("message_size_limit", "52428800".into()),
	];
	s.extend(crate::mailfilter::postfix_settings(
		on && crate::mailfilter::installed(),
	));
	if cert && on {
		s.push((
			"smtpd_tls_cert_file",
			format!("/etc/letsencrypt/live/{host}/fullchain.pem"),
		));
		s.push((
			"smtpd_tls_key_file",
			format!("/etc/letsencrypt/live/{host}/privkey.pem"),
		));
		s.push(("smtpd_tls_security_level", "may".into()));
	}
	s
}

/// Sending from a mail program: 587 with STARTTLS and 465 with TLS, only
/// after a login, only as the mailbox's own address.
pub const SUBMISSION: &[(&str, &[(&str, &str)])] = &[
	("submission", &[]),
	("submissions", &[("smtpd_tls_wrappermode", "yes")]),
];

pub const SUBMISSION_OPTIONS: &[(&str, &str)] = &[
	("smtpd_tls_security_level", "encrypt"),
	("smtpd_sasl_auth_enable", "yes"),
	("smtpd_sasl_type", "dovecot"),
	("smtpd_sasl_path", "private/auth"),
	(
		"smtpd_client_restrictions",
		"permit_sasl_authenticated,reject",
	),
	// `null-sender` of MAPS refuses <>; commas only, master.cf's -o
	(
		"smtpd_sender_restrictions",
		"reject_sender_login_mismatch,check_sender_access,\
		texthash:/etc/genco/mail/null-sender",
	),
	(
		"smtpd_recipient_restrictions",
		"permit_sasl_authenticated,reject",
	),
];

#[cfg(test)]
mod tests {
	use super::*;

	const IP: &str = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever
2: eth0    inet 203.0.113.7/24 brd 203.0.113.255 scope global eth0\\       valid_lft forever
3: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0\\       valid_lft forever
4: br-879bce4e5a38    inet 192.168.96.1/20 brd 192.168.111.255 scope global br-879bce4e5a38\\       valid_lft forever
";

	fn setting(s: &[(&str, String)], name: &str) -> String {
		s.iter().find(|(k, _)| *k == name).unwrap().1.clone()
	}

	fn groups(mail: bool) -> Vec<Group> {
		let domain = MailDomain {
			id: 1,
			group_id: 1,
			name: "example.com".into(),
			send_only: false,
			mailboxes: vec![],
			forwards: vec![],
			send_only_addresses: vec![],
		};
		let mail_domains = if mail { vec![domain] } else { vec![] };
		vec![Group {
			mail_domains,
			..Default::default()
		}]
	}

	/// shop.ch receives with a mailbox and a noreply@, textrem.ch is
	/// send-only with a noreply@.
	fn send_only_groups() -> Vec<Group> {
		let sender = |id, domain: i64, address: &str| SendOnly {
			id,
			mail_domain_id: domain,
			address: address.into(),
			note: String::new(),
			created_at: 0,
			password_hash: format!("{{BLF-CRYPT}}hash-{id}"),
		};
		let shop = MailDomain {
			id: 1,
			group_id: 1,
			name: "shop.ch".into(),
			send_only: false,
			mailboxes: vec![Mailbox {
				id: 1,
				mail_domain_id: 1,
				address: "anna@shop.ch".into(),
				quota_mb: 100,
				password_hash: "{BLF-CRYPT}anna".into(),
			}],
			forwards: vec![],
			send_only_addresses: vec![sender(1, 1, "noreply@shop.ch")],
		};
		let textrem = MailDomain {
			id: 2,
			group_id: 1,
			name: "textrem.ch".into(),
			send_only: true,
			mailboxes: vec![],
			forwards: vec![],
			send_only_addresses: vec![sender(2, 2, "noreply@textrem.ch")],
		};
		vec![Group {
			name: "shop".into(),
			smtp: Some(("pw".into(), "{BLF-CRYPT}group".into())),
			mail_domains: vec![shop, textrem],
			..Default::default()
		}]
	}

	fn content<'a>(files: &'a [File], path: &str) -> &'a str {
		let f = files.iter().find(|f| f.path.as_os_str() == path);
		&f.unwrap_or_else(|| panic!("no {path}")).content
	}

	#[test]
	fn send_only_domains_are_no_mailbox_domains() {
		let cfg: Config =
			toml::from_str("domain = \"panel.example.com\"").unwrap();
		let files = files(&cfg, &send_only_groups(), true, false);
		let domains = content(&files, "/etc/genco/mail/domains");
		assert!(domains.contains("shop.ch OK"));
		assert!(!domains.contains("textrem.ch"), "{domains}");
		for map in ["mailboxes", "forwards", "passwd"] {
			let c = content(&files, &format!("/etc/genco/mail/{map}"));
			assert!(!c.contains("noreply@"), "{map}: {c}");
		}
	}

	#[test]
	fn send_only_addresses_send_as_themselves() {
		let lines = sender_logins(&send_only_groups());
		assert_eq!(
			lines,
			[
				"anna@shop.ch anna@shop.ch,g-shop",
				"noreply@shop.ch noreply@shop.ch,g-shop",
				"@shop.ch g-shop",
				"noreply@textrem.ch noreply@textrem.ch,g-shop",
				"@textrem.ch g-shop",
			]
		);
	}

	#[test]
	fn logins_never_send_with_the_null_sender() {
		let cfg: Config =
			toml::from_str("domain = \"panel.example.com\"").unwrap();
		let files = files(&cfg, &groups(false), false, false);
		let map = content(&files, "/etc/genco/mail/null-sender");
		assert!(map.contains("\n<> REJECT "), "{map}");
		let (_, restrictions) = SUBMISSION_OPTIONS
			.iter()
			.find(|(k, _)| *k == "smtpd_sender_restrictions")
			.unwrap();
		assert_eq!(
			*restrictions,
			format!(
				"reject_sender_login_mismatch,check_sender_access,\
				texthash:{MAPS}/null-sender"
			)
		);
		assert!(!restrictions.contains(' '));
	}

	#[test]
	fn send_only_logins_have_no_mailbox() {
		let cfg: Config =
			toml::from_str("domain = \"panel.example.com\"").unwrap();
		let files = files(&cfg, &send_only_groups(), true, false);
		let smtp = content(&files, "/etc/genco/mail/smtp-passwd");
		assert!(smtp.contains("noreply@shop.ch:{BLF-CRYPT}hash-1\n"));
		assert!(smtp.contains("noreply@textrem.ch:{BLF-CRYPT}hash-2\n"));
		assert!(smtp.contains("g-shop:{BLF-CRYPT}group\n"));
		assert!(!smtp.contains("anna@"));

		// IMAP needs a userdb entry, and the only userdb is the mailboxes'
		let conf = content(&files, DOVECOT_CONF);
		assert_eq!(conf.matches("userdb").count(), 2, "{conf}");
		let userdb = conf.split("userdb passwd-file {").nth(1).unwrap();
		let userdb = userdb.split('}').next().unwrap();
		assert!(userdb.contains("/etc/genco/mail/passwd\n"), "{userdb}");
		assert!(
			conf.contains("passwd_file_path = /etc/genco/mail/smtp-passwd")
		);
		assert!(!conf.contains("prefetch"));
	}

	#[test]
	fn docker_bridges_are_parsed() {
		let b = parse_bridges(IP);
		let names: Vec<&str> = b.iter().map(|b| b.name.as_str()).collect();
		assert_eq!(names, ["docker0", "br-879bce4e5a38"]);
		assert_eq!(b[0].net.to_string(), "172.17.0.0/16");
		assert_eq!(b[1].net.to_string(), "192.168.96.0/20");
		assert!(parse_bridges("").is_empty());
	}

	#[test]
	fn docker_is_installed_with_its_unit_and_dockerd() {
		let with = |paths: &[&str]| {
			docker_installed_with(|p| paths.contains(&p), |_| false)
		};
		assert!(with(&[
			"/lib/systemd/system/docker.service",
			"/usr/bin/dockerd"
		]));
		assert!(with(&[
			"/etc/systemd/system/docker.service",
			"/usr/sbin/dockerd"
		]));
		// purged: the unit's gone, maybe a leftover binary or override
		assert!(!with(&["/usr/bin/dockerd"]));
		assert!(!with(&["/etc/systemd/system/docker.service"]));
		assert!(!with(&[]));

		// masked in /etc, the package's unit is still there
		let mask = "/etc/systemd/system/docker.service";
		let paths = [
			mask,
			"/lib/systemd/system/docker.service",
			"/usr/bin/dockerd",
		];
		assert!(!docker_installed_with(
			|p| paths.contains(&p),
			|p| p == mask
		));
	}

	#[test]
	fn podman_bridges_are_not_docker() {
		let ip = "5: podman0    inet 10.88.0.1/16 scope global podman0
6: cni-podman0    inet 10.89.0.1/24 scope global cni-podman0
";
		assert!(parse_bridges(ip).is_empty());
	}

	#[test]
	fn postfix_waits_for_docker_only_where_installed() {
		let cfg: Config =
			toml::from_str("domain = \"panel.example.com\"").unwrap();
		let has = |docker| {
			files(&cfg, &groups(false), false, docker)
				.iter()
				.any(|f| f.path.as_os_str() == POSTFIX_AFTER_DOCKER)
		};
		assert!(has(true));
		assert!(!has(false));
	}

	#[test]
	fn only_private_bridge_networks_may_relay() {
		let nets = |cidr: &str| {
			let line = format!("5: br-1    inet {cidr} scope global br-1");
			let b = parse_bridges(&line);
			b.iter().map(|b| b.net.to_string()).collect::<Vec<_>>()
		};
		assert_eq!(nets("172.18.0.1/16"), ["172.18.0.0/16"]);
		assert_eq!(nets("10.0.0.1/8"), ["10.0.0.0/8"]);
		assert!(nets("172.18.0.1/0").is_empty());
		assert!(nets("172.18.0.1/8").is_empty());
		assert!(nets("10.0.0.1/7").is_empty());
		assert!(nets("203.0.113.1/24").is_empty());
		assert!(nets("127.0.0.1/8").is_empty());
		assert!(nets("172.18.0.1/33").is_empty());
	}

	#[test]
	fn postfix_listens_on_docker0_without_mail_domains() {
		let cfg: Config =
			toml::from_str("domain = \"panel.example.com\"").unwrap();
		let bridges = parse_bridges(IP);

		let s = postfix_settings(&cfg, &groups(false), false, &[]);
		assert_eq!(setting(&s, "inet_interfaces"), "loopback-only");
		assert_eq!(
			setting(&s, "mynetworks"),
			"127.0.0.0/8 [::ffff:127.0.0.0]/104 [::1]/128"
		);

		let s = postfix_settings(&cfg, &groups(false), false, &bridges);
		assert_eq!(setting(&s, "inet_interfaces"), "localhost, 172.17.0.1");
		let nets = setting(&s, "mynetworks");
		assert!(nets.ends_with(" 172.17.0.0/16 192.168.96.0/20"), "{nets}");

		// a compose network alone: nothing listens on it
		let s = postfix_settings(&cfg, &groups(false), false, &bridges[1..]);
		assert_eq!(setting(&s, "inet_interfaces"), "loopback-only");
	}

	#[test]
	fn postfix_listens_everywhere_with_mail_domains() {
		let cfg: Config =
			toml::from_str("domain = \"panel.example.com\"").unwrap();
		let bridges = parse_bridges(IP);
		for b in [&bridges[..], &[]] {
			let s = postfix_settings(&cfg, &groups(true), true, b);
			assert_eq!(setting(&s, "inet_interfaces"), "all");
			// never the public network
			assert!(!setting(&s, "mynetworks").contains("203.0.113"));
		}
	}

	#[test]
	fn dovecot_without_certificate_only_delivers() {
		let c = dovecot_conf(None);
		assert!(c.contains("protocols = lmtp\n"));
		assert!(!c.contains("imap lmtp"));
		let c = dovecot_conf(Some("mail.x.ch"));
		assert!(c.contains("protocols = imap lmtp"));
		assert!(c.contains("/etc/letsencrypt/live/mail.x.ch/fullchain.pem"));
		// only genco's passwd file, never the system's users
		assert!(!c.contains("pam"));
	}
}
