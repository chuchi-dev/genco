//! A group on the system: its user, its folders, its deploy key and its
//! services started (or waiting for the first deploy).

use super::*;

/// Starts the group's units, restarts those whose config changed.
pub(super) async fn start_units(
	r: &mut Report,
	g: &Group,
	changed: &HashSet<PathBuf>,
) {
	for v in render::php_versions(g) {
		let unit = render::php_unit(&g.name, v);
		let conf = render::php_config(&g.name, v);
		let restart = changed.contains(&render::unit_path(&unit))
			|| changed.contains(&conf);
		// a broken pool would stop the running one and the sites with it
		// (as the group's user, php-fpm refuses root without a user)
		let conf = conf.display().to_string();
		let fpm = format!("php-fpm{v}");
		let user = render::group_user(&g.name);
		let test = [
			"runuser",
			"-u",
			&user,
			"--",
			&fpm,
			"-t",
			"--fpm-config",
			&conf,
		];
		if restart && !r.run(&test).await {
			r.error(format!("{unit} keeps running with its old config"));
			continue;
		}
		start_unit(r, &unit, restart).await;
	}

	for d in g.domains.iter().filter(|d| render::has_app(d)) {
		let unit = render::app_unit(&g.name, &d.name);
		let script = render::app_script(&d.name);
		let restart = changed.contains(&render::unit_path(&unit))
			|| changed.contains(&script)
			|| changed.contains(&render::app_env(&d.name));
		// before its first deploy it would only fail and restart
		if !render::app_ready(&g.name, d) {
			r.run(&["systemctl", "enable", "-q", &unit]).await;
			r.info(format!(
				"{unit} waits for the first deploy ({} isn't there)",
				d.root
			));
			continue;
		}
		start_unit(r, &unit, restart).await;
	}

	for w in &g.workers {
		let unit = render::worker_unit(&g.name, &w.name);
		let restart = changed.contains(&render::unit_path(&unit))
			|| changed.contains(&render::worker_script(&g.name, &w.name));
		if !render::worker_ready(&g.name, w) {
			r.run(&["systemctl", "enable", "-q", &unit]).await;
			r.info(format!(
				"{unit} waits for the first deploy ({} isn't there)",
				w.dir
			));
			continue;
		}
		start_unit(r, &unit, restart).await;
	}

	// the timer starts the service; a changed time needs a restart
	for s in &g.schedules {
		let timer = render::schedule_timer(&g.name, &s.name);
		let restart = changed.contains(&render::unit_path(&timer));
		start_unit(r, &timer, restart).await;
	}
}

pub(super) async fn start_unit(r: &mut Report, unit: &str, restart: bool) {
	r.run(&["systemctl", "enable", "-q", unit]).await;
	// one that failed before is started again even without a change
	let failed = Command::new("systemctl")
		.args(["is-failed", "--quiet", unit])
		.status()
		.await
		.is_ok_and(|s| s.success());
	let restart = restart || failed;
	let action = if restart { "restart" } else { "start" };
	if r.run(&["systemctl", action, unit]).await && restart {
		r.info(format!("restarted {unit}"));
	}
}

/// uid and gid of a user from /etc/passwd
pub(super) fn user_ids(user: &str) -> Option<(u32, u32)> {
	let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
	passwd.lines().find_map(|line| {
		let cols: Vec<&str> = line.split(':').collect();
		if cols.first() != Some(&user) || cols.len() < 4 {
			return None;
		}
		Some((cols[2].parse().ok()?, cols[3].parse().ok()?))
	})
}

/// The group's user; nginx is in its group to read the files.
pub(super) async fn ensure_user(r: &mut Report, g: &Group) {
	let user = render::group_user(&g.name);
	if user_ids(&user).is_none() {
		let home = render::group_dir(&g.name).display().to_string();
		if !r
			.run(&[
				"useradd",
				"--system",
				"--user-group",
				"--home-dir",
				&home,
				"--no-create-home",
				"--shell",
				"/usr/sbin/nologin",
				&user,
			])
			.await
		{
			return;
		}
		r.info(format!("added the user {user}"));
	}

	let groups = std::fs::read_to_string("/etc/group").unwrap_or_default();
	let member = groups.lines().any(|l| {
		l.starts_with(&format!("{user}:"))
			&& l.rsplit(':')
				.next()
				.unwrap_or("")
				.split(',')
				.any(|m| m == "www-data")
	});
	if !member {
		r.run(&["gpasswd", "-a", "www-data", &user]).await;
	}
}

/// The group's directory, logs, tmp and the domains' roots. New roots of
/// static and PHP domains get a placeholder page.
///
/// Only the group's directory is made as root: its parent belongs to root,
/// so nobody can swap it for a link. Everything inside is made as the
/// group's user, root following a link the group placed there could hand
/// it any file of the system.
pub(super) async fn ensure_dirs(r: &mut Report, g: &Group) {
	let user = render::group_user(&g.name);
	let Some((uid, gid)) = user_ids(&user) else {
		return;
	};
	let base = render::group_dir(&g.name);

	// /srv/genco belongs to root
	let top = base.parent().unwrap();
	if std::fs::symlink_metadata(&base).is_err() {
		let made = std::fs::create_dir_all(top)
			.and_then(|()| std::fs::create_dir(&base))
			.and_then(|()| chown(&base, Some(uid), Some(gid)))
			.and_then(|()| set_mode(&base, 0o750));
		match made {
			Ok(()) => r.info(format!("created {}", base.display())),
			Err(e) => {
				r.error(format!("creating {}: {e}", base.display()));
				return;
			}
		}
	}

	let mut dirs = vec![(base.join("logs"), None), (base.join("tmp"), None)];
	for d in &g.domains {
		// deploys create current, it links to a release; a proxy has
		// no files
		if d.root.split('/').next() == Some("current") || d.kind == Kind::Proxy
		{
			continue;
		}
		let page =
			matches!(d.kind, Kind::Static | Kind::Php).then_some(&d.name);
		dirs.push((base.join(&d.root), page));
	}

	for (dir, page) in dirs {
		if dir.exists() {
			continue;
		}
		let html = page
			.map(|domain| {
				format!(
					"<!doctype html><title>{domain}</title>\
					<p>{domain} is hosted by genco.</p>\n"
				)
			})
			.unwrap_or_default();
		let dir_arg = dir.display().to_string();
		let script = r#"umask 027; mkdir -p -- "$1" && { [ -z "$2" ] || printf '%s' "$2" > "$1/index.html"; }"#;
		let args = [
			"runuser", "-u", &user, "--", "sh", "-c", script, "sh", &dir_arg,
			&html,
		];
		if r.run(&args).await {
			r.info(format!("created {}", dir.display()));
		}
	}
}

/// The key the group fetches its repository with, read-only access in
/// GitLab or GitHub is enough.
pub(super) async fn deploy_key(r: &mut Report, g: &Group) {
	let user = render::group_user(&g.name);
	let key = render::deploy_key_path(&g.name);
	let ssh = key.parent().unwrap().to_path_buf();
	if key.exists() {
		return;
	}

	let ssh = ssh.display().to_string();
	let key = key.display().to_string();
	let comment = format!("genco {} {}", g.name, crate::dns::fqdn().await);
	let script = format!(
		"mkdir -p -m 700 {ssh} && ssh-keygen -q -t ed25519 -N '' \
		-C '{comment}' -f {key}"
	);
	if r.run(&["runuser", "-u", &user, "--", "sh", "-c", &script])
		.await
	{
		r.info(format!("created the deploy key of {}", g.name));
	}
}
