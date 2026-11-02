//! The databases (MariaDB, PostgreSQL) and the certificates.

use super::*;

pub(super) const ACME_DIR: &str = "/var/lib/genco-acme";

/// A MariaDB user per group that logs in with its unix user over the
/// socket, no password; the same in PostgreSQL, see pg.rs.
pub(super) async fn databases(r: &mut Report, groups: &[Group]) {
	postgres(r, groups).await;
	let mut sql = String::new();
	let mariadb =
		|g: &&Group| g.databases.iter().any(|d| d.engine == Engine::Mariadb);
	for g in groups.iter().filter(mariadb) {
		let user = render::group_user(&g.name);
		sql.push_str(&format!(
			"CREATE USER IF NOT EXISTS `{user}`@localhost \
			IDENTIFIED VIA unix_socket;\n"
		));
		for db in g.databases.iter().filter(|d| d.engine == Engine::Mariadb) {
			let name = &db.name;
			sql.push_str(&format!(
				"CREATE DATABASE IF NOT EXISTS `{name}` \
				CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;\n\
				GRANT ALL PRIVILEGES ON `{grant}`.* TO `{user}`@localhost;\n",
				// in a grant "_" matches any character: shop_1 would also
				// cover shopx1 of another group
				grant = name.replace('_', "\\_"),
			));
		}
	}
	if sql.is_empty() {
		return;
	}

	r.run_input(&["mariadb"], Some(&sql)).await;
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
	let mut domains: Vec<&str> = groups
		.iter()
		.flat_map(|g| &g.domains)
		.map(|d| d.name.as_str())
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

		let ips = resolve(domain).await;
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
		}
	}
	added
}

pub(super) async fn resolve(domain: &str) -> Vec<String> {
	match tokio::net::lookup_host((domain, 80)).await {
		Ok(addrs) => addrs.map(|a| a.ip().to_string()).collect(),
		Err(_) => vec![],
	}
}
