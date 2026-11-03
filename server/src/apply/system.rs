//! The databases (MariaDB, PostgreSQL) and the certificates.

use super::*;

pub(super) const ACME_DIR: &str = "/var/lib/genco-acme";

/// A MariaDB user per group that logs in with its unix user over the
/// socket, no password; the same in PostgreSQL, see pg.rs.
pub(super) async fn databases(r: &mut Report, groups: &[Group]) {
	postgres(r, groups).await;
	let sql = mariadb_sql(groups);
	if sql.is_empty() {
		return;
	}

	r.run_input(&["mariadb"], Some(&sql)).await;
}

/// MariaDB has no owners: genco grants a database to its group only when
/// it creates it or the group has it already. One of the same name that
/// is there without the group's grant (another app's, made before genco
/// knew the name) stays as it is, and the script fails at the end naming
/// it, after everything else is set up.
fn mariadb_sql(groups: &[Group]) -> String {
	let mut sql = String::new();
	let mariadb =
		|g: &&Group| g.databases.iter().any(|d| d.engine == Engine::Mariadb);
	for g in groups.iter().filter(mariadb) {
		let user = render::group_user(&g.name);
		sql.push_str(&format!(
			"CREATE USER IF NOT EXISTS `{user}`@localhost \
			IDENTIFIED VIA unix_socket;;\n"
		));
		for db in g.databases.iter().filter(|d| d.engine == Engine::Mariadb) {
			let name = &db.name;
			// in a grant "_" matches any character: shop_1 would also
			// cover shopx1 of another group
			let grant = name.replace('_', "\\_");
			sql.push_str(&format!(
				"BEGIN NOT ATOMIC\n\
				IF NOT EXISTS (SELECT 1 FROM information_schema.schemata \
				WHERE schema_name = '{name}') OR EXISTS (SELECT 1 FROM \
				mysql.db WHERE Db = '{literal}' AND User = '{user}' \
				AND Host = 'localhost') THEN\n\
				CREATE DATABASE IF NOT EXISTS `{name}` \
				CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;\n\
				GRANT ALL PRIVILEGES ON `{grant}`.* TO `{user}`@localhost;\n\
				ELSE SET @foreign = CONCAT_WS(', ', @foreign, '{name}');\n\
				END IF;\n\
				END;;\n",
				literal = grant.replace('\\', "\\\\"),
			));
		}
	}
	if sql.is_empty() {
		return sql;
	}
	format!(
		"DELIMITER ;;\n{sql}\
		BEGIN NOT ATOMIC\n\
		DECLARE msg VARCHAR(512) DEFAULT CONCAT('genco leaves these \
		databases alone, they are not its own: ', @foreign);\n\
		IF @foreign IS NOT NULL THEN \
		SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = msg;\n\
		END IF;\n\
		END;;\n"
	)
}

/// PostgreSQL, installed with its first database.
pub(super) async fn postgres(r: &mut Report, groups: &[Group]) {
	if !crate::pg::used(groups) {
		return;
	}
	if !crate::pg::installed() {
		r.info("installing PostgreSQL for the first database");
		let ok = r
			.run(&[
				"sh",
				"-c",
				"DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
				postgresql postgresql-client >/dev/null",
			])
			.await;
		if !ok {
			return;
		}
	}
	let sql = crate::pg::setup_sql(groups);
	r.run_input(&crate::pg::PSQL_ADMIN, Some(&sql)).await;
}

/// Gets certificates for the domains that point to this server and have
/// none yet. Returns true if one was added.
pub(super) async fn certificates(
	r: &mut Report,
	cfg: &Config,
	groups: &[Group],
) -> bool {
	// a www redirect gets its own certificate: without DNS for it the
	// domain's own still works
	let www: Vec<String> = groups
		.iter()
		.flat_map(|g| &g.domains)
		.filter(|d| d.www)
		.map(|d| format!("www.{}", d.name))
		.collect();
	let mut domains: Vec<&str> = groups
		.iter()
		.flat_map(|g| &g.domains)
		.map(|d| d.name.as_str())
		.chain(www.iter().map(String::as_str))
		.collect();
	domains.extend(cfg.domain.as_deref());
	let mail_host = mailbox::host(cfg).filter(|_| mailbox::enabled(groups));
	domains.extend(mail_host.as_deref());
	certificates_for(r, cfg, domains).await
}

/// Gets certificates for those of `domains` that point to this server and
/// have none yet. Returns true if one was added.
pub(super) async fn certificates_for(
	r: &mut Report,
	cfg: &Config,
	domains: Vec<&str>,
) -> bool {
	let own = crate::dns::own_ips()
		.await
		.into_iter()
		.collect::<HashSet<_>>();
	let mut added = false;
	for domain in domains {
		if host::has_cert(domain) {
			continue;
		}

		let ips = crate::dns::resolve(domain).await;
		if !ips.iter().any(|ip| own.contains(ip)) {
			r.info(format!(
				"no certificate for {domain} yet: it doesn't point to this \
				server ({})",
				if ips.is_empty() {
					"no DNS record".to_string()
				} else {
					ips.join(", ")
				}
			));
			continue;
		}

		let mut args = vec![
			"certbot",
			"certonly",
			"--webroot",
			"-w",
			ACME_DIR,
			"-d",
			domain,
			"-n",
			"--agree-tos",
			"--keep-until-expiring",
			"--deploy-hook",
			"systemctl reload nginx",
		];
		match &cfg.email {
			Some(email) => args.extend(["-m", email]),
			None => args.push("--register-unsafely-without-email"),
		}
		if r.run(&args).await {
			r.info(format!("got a certificate for {domain}"));
			added = true;
		} else {
			r.info(cert_hint(domain));
		}
	}
	added
}

/// certbot only says "Some challenges have failed"; its log has Let's
/// Encrypt's reason. The usual one is a firewall in front of the server.
fn cert_hint(domain: &str) -> String {
	let log = std::fs::read_to_string("/var/log/letsencrypt/letsencrypt.log")
		.unwrap_or_default();
	let detail = log
		.lines()
		.rev()
		.find_map(|l| l.split_once("Detail: ").map(|(_, d)| d.trim()))
		.map(|d| format!("Let's Encrypt says: {d}. "))
		.unwrap_or_default();
	format!(
		"{detail}Let's Encrypt has to reach http://{domain}/.well-known/\
		acme-challenge/ on port 80 from the internet: a cloud firewall or \
		security group in front of the server (Hetzner, Infomaniak, AWS) \
		blocks it until it allows ports 80 and 443; then run genco apply"
	)
}

/// Reloads nginx and checks that its master took the config: a reload
/// only signals the master, which keeps the old config when it can't load
/// the new one, e.g. an older running binary than the one `nginx -t`
/// tested. Then a restart brings the new one.
pub(super) async fn reload_nginx(r: &mut Report) {
	let before = nginx_workers();
	if !r.run(&["systemctl", "reload", "nginx"]).await {
		return;
	}
	// the master starts new workers for a new config
	let Some(before) = before else {
		return;
	};
	for _ in 0..50 {
		if nginx_workers().is_some_and(|after| reloaded(&before, &after)) {
			return;
		}
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	}
	r.info("nginx kept its old config on reload, restarting it");
	r.run(&["systemctl", "restart", "nginx"]).await;
}

/// The processes of nginx's master, None without a running master.
fn nginx_workers() -> Option<Vec<u32>> {
	let pid = std::fs::read_to_string("/run/nginx.pid").ok()?;
	let pid = pid.trim();
	let children =
		std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
			.ok()?;
	Some(
		children
			.split_whitespace()
			.filter_map(|p| p.parse().ok())
			.collect(),
	)
}

/// A new process means the master loaded the config again.
fn reloaded(before: &[u32], after: &[u32]) -> bool {
	after.iter().any(|p| !before.contains(p))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::hosting::Database;

	#[test]
	fn grants_only_its_own_databases() {
		let g = Group {
			name: "shop".into(),
			databases: vec![Database {
				id: 1,
				group_id: 1,
				name: "shop_db".into(),
				engine: Engine::Mariadb,
			}],
			..Group::default()
		};
		let sql = mariadb_sql(&[g]);
		assert!(sql.starts_with("DELIMITER ;;\n"));
		assert!(sql.contains(
			"IF NOT EXISTS (SELECT 1 FROM information_schema.schemata \
			WHERE schema_name = 'shop_db') OR EXISTS (SELECT 1 FROM \
			mysql.db WHERE Db = 'shop\\\\_db' AND User = 'g-shop'"
		));
		assert!(sql.contains("GRANT ALL PRIVILEGES ON `shop\\_db`.*"));
		assert!(sql.contains("SIGNAL SQLSTATE"));
		assert!(mariadb_sql(&[]).is_empty());
	}

	#[test]
	fn a_reload_shows_in_new_nginx_processes() {
		assert!(reloaded(&[10, 11], &[10, 11, 12]));
		// old workers still serving their connections
		assert!(reloaded(&[10, 11], &[11, 13]));
		assert!(!reloaded(&[10, 11], &[10, 11]));
		assert!(!reloaded(&[10, 11], &[10]));
	}
}
