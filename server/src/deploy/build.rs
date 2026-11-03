//! From a new release to the switch: unpack the upload or export the
//! commit, link the shared paths, run the build steps (here or on a
//! runner), check the binaries, switch `current`, restart, prune.

use super::*;
use crate::render::Restart;

pub(super) async fn deploy(ctx: &Ctx, id: i64, log: &mut Log) -> Res<()> {
	let (d, group) = {
		let conn = ctx.db.conn();
		let d = get(&conn, id).map_err(|e| e.to_string())?;
		let group = hosting::find_group(&conn, d.group_id)
			.map_err(|e| e.to_string())?;
		(d, group)
	};
	let base = render::group_dir(&group.name);
	let shell = GroupShell::new(&group);
	let stamp = util::stamp().await;
	let release = base.join("releases").join(&stamp);

	let new = Release {
		ctx,
		shell: &shell,
		group: &group,
		base: &base,
		stamp: &stamp,
		id,
	};
	let cfg = match new.prepare(log, &d.git_ref).await {
		Ok(cfg) => cfg,
		Err(e) => {
			// the live site stays as it is, the half release goes (prune
			// would count it); as the group's user, root must not follow
			// links the group made
			let rm = format!("rm -rf -- releases/{stamp}");
			let _ = shell.run(log, &base, &rm).await;
			return Err(e);
		}
	};

	switch(log, &base, &stamp, &shell.user).await?;
	let mut after = Ok(());
	for s in &cfg.after_switch {
		log.line(&format!("$ {} (in {})", s.run, s.dir)).await;
		after = shell.run(log, &release.join(&s.dir), &s.run).await;
		if after.is_err() {
			break;
		}
	}
	// the release is live already, the apps run it even if a step after
	// the switch failed
	let group = match sync_procs(ctx, log, &group, &cfg).await {
		Ok(g) => g,
		Err(e) => {
			after = after.and(Err(e));
			group
		}
	};
	after = after
		.map_err(|e| format!("{e}, the new release is live, check the site"));
	// a failed restart says itself what runs
	after = after.and(restart(ctx, log, &group).await);
	prune(&shell, log, &base).await;
	after
}

/// A new release of a deploy, from its upload or the repository.
pub(super) struct Release<'a> {
	pub(super) ctx: &'a Ctx,
	pub(super) shell: &'a GroupShell,
	pub(super) group: &'a Group,
	pub(super) base: &'a Path,
	pub(super) stamp: &'a str,
	pub(super) id: i64,
}

impl Release<'_> {
	/// Unpacks or exports the release and builds it, up to what is ready
	/// to switch to.
	pub(super) async fn prepare(
		&self,
		log: &mut Log,
		git_ref: &str,
	) -> Res<DeployConfig> {
		let (shell, base, stamp) = (self.shell, self.base, self.stamp);
		let upload = upload_path(self.id);
		let uploaded = upload.exists();
		if uploaded {
			log.line(&format!("deploying {git_ref}, built elsewhere"))
				.await;
			unpack(shell, log, base, stamp, &upload).await?;
		} else {
			let repo = (self.group.repo.clone())
				.ok_or("the group has no repository")?;
			let commit = fetch(shell, log, base, &repo, git_ref).await?;
			self.ctx.set_commit(self.id, &commit);
			log.line(&format!(
				"deploying {git_ref} ({})",
				commit.get(..12).unwrap_or(&commit)
			))
			.await;
			let export = format!(
				"set -o pipefail; mkdir -p releases/{stamp} \
				&& git --git-dir=repo.git archive {commit} \
				| tar -x -C releases/{stamp}"
			);
			shell.run(log, base, &export).await?;
		}
		self.ctx.set_release(self.id, stamp);

		let release = base.join("releases").join(stamp);
		Build {
			ctx: self.ctx,
			shell,
			group: self.group,
			base,
			release: &release,
			id: self.id,
			uploaded,
		}
		.run(log)
		.await
	}
}

/// Updates the group's mirror of the repository (clones it the first
/// time) and returns the commit of `git_ref`.
pub(super) async fn fetch(
	shell: &GroupShell,
	log: &mut Log,
	base: &Path,
	repo: &str,
	git_ref: &str,
) -> Res<String> {
	let mirror = base.join("repo.git");
	// a token in the URL stays out of the log, a CI job shows it
	if let Some(user) = url_credentials(repo) {
		log.hide(user);
	}
	if mirror.exists() {
		log.line(&format!("fetching {repo}")).await;
		shell.run(log, &mirror, "git remote update --prune").await?;
	} else {
		log.line(&format!("cloning {repo}")).await;
		let clone =
			format!("git clone --mirror -q -- {} repo.git", sh_quote(repo));
		shell.run(log, base, &clone).await?;
	}

	let rev =
		format!("git rev-parse --verify {}^{{commit}}", sh_quote(git_ref));
	shell
		.output(&mirror, &rev)
		.await
		.map_err(|_| format!("{git_ref} is no branch, tag or commit"))
}

/// Unpacks an uploaded release (tar, gzip or not) as the group's user.
pub(super) async fn unpack(
	shell: &GroupShell,
	log: &mut Log,
	base: &Path,
	stamp: &str,
	upload: &Path,
) -> Res<()> {
	let mut magic = [0u8; 2];
	let gz = std::fs::File::open(upload)
		.and_then(|mut f| std::io::Read::read_exact(&mut f, &mut magic))
		.is_ok()
		&& magic == [0x1f, 0x8b];
	let file = std::fs::File::open(upload).map_err(|e| e.to_string())?;
	let z = if gz { "z" } else { "" };
	let mut cmd = shell.command(
		base,
		&format!(
			"exec 2>&1; mkdir -p releases/{stamp} \
			&& tar -x{z}f - --no-same-owner -C releases/{stamp}"
		),
		60,
	);
	cmd.stdin(Stdio::from(file));
	log.run(cmd)
		.await
		.map_err(|e| format!("unpacking the upload {e}"))
}

/// The steps from the release to what is ready to switch to.
pub(super) struct Build<'a> {
	pub(super) ctx: &'a Ctx,
	pub(super) shell: &'a GroupShell,
	pub(super) group: &'a Group,
	pub(super) base: &'a Path,
	pub(super) release: &'a Path,
	/// the deploy's id, for the runner's job
	pub(super) id: i64,
	/// built elsewhere: no build steps
	pub(super) uploaded: bool,
}

impl Build<'_> {
	/// Links the shared paths, runs the build steps and checks the
	/// binaries.
	pub(super) async fn run(&self, log: &mut Log) -> Res<DeployConfig> {
		let cfg = config(self.shell, self.release, self.base).await?;
		if !self.release.join("genco.toml").exists() {
			log.line("no genco.toml, detected the steps").await;
		}
		for hint in &cfg.hints {
			log.line(&format!("note: {hint}")).await;
		}
		for s in cfg.build.iter().chain(&cfg.after_switch) {
			s.check().map_err(|e| format!("genco.toml: {e}"))?;
		}
		if cfg.after_switch.iter().any(Step::on_runner) {
			return Err("genco.toml: steps after the switch run here".into());
		}
		cfg.procs()?;
		for s in &cfg.schedule {
			procs::check_every(&s.every)
				.await
				.map_err(|e| format!("genco.toml: {e}"))?;
		}

		for p in &cfg.shared {
			check_path(p)?;
			link_shared(self.shell, log, self.base, self.release, p).await?;
		}

		if self.uploaded && !cfg.build.is_empty() {
			log.line("built elsewhere, the build steps don't run").await;
		}
		let steps = if self.uploaded {
			&[][..]
		} else {
			&cfg.build[..]
		};
		for s in steps {
			log.line(&format!("$ {} (in {})", s.run, s.dir)).await;
			if s.on_runner() {
				self.on_runner(log, s).await?;
			} else {
				let dir = self.release.join(&s.dir);
				self.shell.run_for(log, &dir, &s.run, s.minutes()).await?;
			}
		}

		check_binaries(self.shell, log, self.release).await?;
		Ok(cfg)
	}

	/// A step on a runner: the step's folder goes there (packed as the
	/// group's user), the artifacts come back (unpacked as it). Without a
	/// runner it runs here, with caches outside the release.
	pub(super) async fn on_runner(&self, log: &mut Log, s: &Step) -> Res<()> {
		let dir = self.release.join(&s.dir);
		let req = crate::runners::Request {
			db: &self.ctx.db,
			deploy_id: self.id,
			group: &self.group.name,
			dir: &dir,
			rel_dir: &s.dir,
			image: s.image.as_deref(),
			run: &s.run,
			artifacts: &s.artifacts,
			minutes: s.minutes(),
		};
		let pack = self.shell.command(
			&dir,
			"tar --exclude=./target --exclude=./node_modules -cf - .",
			60,
		);
		let unpack = |file: std::fs::File| {
			let mut c = self.shell.command(
				&dir,
				"exec 2>&1; tar -xf - --no-same-owner",
				60,
			);
			c.stdin(Stdio::from(file));
			c
		};
		match crate::runners::build(req, log, pack, unpack).await? {
			crate::runners::Outcome::Built => {
				log.line(&format!("got {}", s.artifacts.join(", "))).await;
				Ok(())
			}
			crate::runners::Outcome::Here(why) => {
				log.line(&format!("{why}, building here")).await;
				let run = format!(
					"export CARGO_HOME=$HOME/.cache/cargo \
					npm_config_cache=$HOME/.cache/npm; {}",
					s.run
				);
				self.shell.run_for(log, &dir, &run, s.minutes()).await
			}
		}
	}
}

/// Every ELF binary of the release has to find its libraries here: one
/// built against a newer glibc than this server's would fail at its start.
/// As the group's user, `ldd` runs its loader.
pub(super) async fn check_binaries(
	shell: &GroupShell,
	log: &mut Log,
	release: &Path,
) -> Res<()> {
	// the ELF machine of this server: a binary for another one is "not a
	// dynamic executable" for ldd and would pass
	let machine = match std::env::consts::ARCH {
		"x86_64" => 62,
		"aarch64" => 183,
		_ => 0,
	};
	let script = format!(
		r#"find . -xdev -type f \( -perm -u+x -o -name '*.so' -o -name '*.so.*' \) \
		-size +1k -not -path '*/node_modules/*' -not -path './.git/*' 2>/dev/null \
	| head -n 2000 | while IFS= read -r f; do
		[ "$(head -c 4 "$f" | od -An -c | tr -d ' ')" = '177ELF' ] || continue
		m=$(od -An -tu2 -j18 -N2 "$f" | tr -d ' ')
		if [ {machine} != 0 ] && [ "$m" != {machine} ]; then
			printf '%s:\n\tbuilt for another processor (ELF machine %s)\n' "$f" "$m"
			continue
		fi
		bad=$(ldd "$f" 2>&1 | grep 'not found')
		[ -z "$bad" ] || printf '%s:\n%s\n' "$f" "$bad"
	done"#
	);
	let out = shell.output(release, &script).await?;
	if out.is_empty() {
		return Ok(());
	}
	for line in out.lines() {
		log.line(line).await;
	}
	Err("binaries of the release don't run on this server (missing \
		libraries or a newer glibc), build them for it (genco runners shows \
		what for)"
		.into())
}

/// Links `p` in the release to `shared/p`. The first deploy brings it if
/// shared/ has none yet, then it stays; a missing file like .env stops the
/// deploy, its folder in shared/ is there to create it in. As the group's
/// user, like everything in its directory.
pub(super) async fn link_shared(
	shell: &GroupShell,
	log: &mut Log,
	base: &Path,
	release: &Path,
	p: &str,
) -> Res<()> {
	let script = format!(
		r#"s={shared}; t={target}
		if [ ! -e "$s" ]; then
			if [ -e "$t" ]; then mkdir -p "$(dirname "$s")" && mv "$t" "$s"
			elif [ {file} = 1 ]; then mkdir -p "$(dirname "$s")"; echo "$s is missing, create it and deploy again"; exit 3
			else mkdir -p "$s"
			fi
		fi
		rm -rf "$t" && mkdir -p "$(dirname "$t")" && ln -s "$s" "$t""#,
		shared = sh_quote(&base.join("shared").join(p).display().to_string()),
		target = sh_quote(&release.join(p).display().to_string()),
		file = u8::from(looks_like_file(p)),
	);
	shell
		.run(log, base, &script)
		.await
		.map_err(|_| format!("sharing {p} failed, see above"))?;
	log.line(&format!("shared {p}")).await;
	Ok(())
}

/// `current` points to the new release in one step (rename).
pub(super) async fn switch(
	log: &mut Log,
	base: &Path,
	stamp: &str,
	user: &str,
) -> Res<()> {
	let current = base.join("current");
	if current.exists() && !current.is_symlink() {
		return Err(format!(
			"{} is a directory, move it away so it can link to the releases",
			current.display()
		));
	}
	let tmp = base.join("current.new");
	let _ = std::fs::remove_file(&tmp);
	std::os::unix::fs::symlink(format!("releases/{stamp}"), &tmp)
		.and_then(|()| std::fs::rename(&tmp, &current))
		.map_err(|e| format!("switching current: {e}"))?;
	// the link belongs to the group like everything in its directory
	let _ = Command::new("chown")
		.args(["-h", &format!("{user}:{user}")])
		.arg(&current)
		.status()
		.await;
	log.line(&format!("current -> releases/{stamp}")).await;
	Ok(())
}

/// The workers and schedules of genco.toml (or detected) replace the
/// earlier ones; with a change apply writes their units. Returns the group
/// as it is now.
async fn sync_procs(
	ctx: &Ctx,
	log: &mut Log,
	group: &Group,
	cfg: &DeployConfig,
) -> Res<Group> {
	let (workers, schedules) = cfg.procs()?;
	let changed =
		procs::sync_repo(&ctx.db.conn(), group.id, &workers, &schedules)
			.map_err(|e| format!("genco.toml: {e}"))?;
	if !changed {
		return Ok(group.clone());
	}
	log.line("the workers or schedules changed, applying").await;
	let report = crate::apply::apply(&ctx.cfg, &ctx.db).await;
	// its errors may be about other groups, the CI job of this one shows
	// the log: only that there were some
	let errors = report.log.iter().filter(|l| l.starts_with("error")).count();
	if errors > 0 {
		log.line(&format!(
			"the apply had {errors} error(s), an admin sees them in the panel \
			(Server → Apply Changes)"
		))
		.await;
	}
	ctx.events.send(crate::events::Event::Tree);
	hosting::find_group(&ctx.db.conn(), group.id).map_err(|e| e.to_string())
}

/// PHP drops its cached code, the apps start with the new release. Fails
/// when an app that restarts without downtime didn't come up, its old
/// process stays live.
pub(super) async fn restart(
	ctx: &Ctx,
	log: &mut Log,
	group: &Group,
) -> Res<()> {
	for v in render::php_versions(group) {
		let unit = render::php_unit(&group.name, v);
		systemctl(log, "reload", &unit).await;
	}

	let mut res = Ok(());
	for d in group.domains.iter().filter(|d| render::has_app(d)) {
		// apply writes nginx and the units from the database, it waits
		// until the live slot is settled; read after it: a deploy before
		// this one may have switched the slot or the option changed
		let _lock = job::lock(crate::apply::LOCK).await?;
		let Ok((_, d)) = hosting::find_domain(&ctx.db.conn(), |x| x.id == d.id)
		else {
			log.line(&format!("{} is gone, not restarted", d.name))
				.await;
			continue;
		};
		match render::restart(&group.name, &d) {
			Restart::InPlace { unit } => {
				systemctl(log, "restart", &unit).await;
			}
			Restart::Switch { start, stop, .. }
				if !render::unit_path(&start).exists() =>
			{
				log.line(&format!(
					"{start} isn't there yet (Apply Changes writes it), \
					restarting in place"
				))
				.await;
				systemctl(log, "restart", &stop).await;
			}
			Restart::Switch {
				start,
				port,
				to,
				stop,
			} => {
				let switch = slot::Switch {
					domain: &d,
					start: &start,
					port,
					to,
					stop: &stop,
				};
				res = res.and(switch.run(ctx, log).await);
			}
		}
	}

	for w in &group.workers {
		let unit = render::worker_unit(&group.name, &w.name);
		systemctl(log, "restart", &unit).await;
	}
	res
}

pub(super) async fn systemctl(log: &mut Log, action: &str, unit: &str) {
	let ok = Command::new("systemctl")
		.args([action, unit])
		.status()
		.await
		.is_ok_and(|s| s.success());
	log.line(&format!(
		"{action} {unit}{}",
		if ok { "" } else { " failed" }
	))
	.await;
}

/// Keeps the newest releases and the current one. As the group's user:
/// root deleting in a directory the group controls would follow its links.
pub(super) async fn prune(shell: &GroupShell, log: &mut Log, base: &Path) {
	let script = format!(
		r#"cur=$(readlink current); cd releases || exit 0
		ls -1 | sort -r | tail -n +{} | while read -r n; do
			[ "releases/$n" = "$cur" ] || {{ rm -rf -- "$n" && echo "removed the old release $n"; }}
		done"#,
		KEEP_RELEASES + 1
	);
	let _ = shell.run(log, base, &script).await;
}
/// The user (and password) of `scheme://user:pass@host/…`, e.g. a
/// GitLab token; `git@host:path` has no secret.
fn url_credentials(url: &str) -> Option<&str> {
	let (_, rest) = url.split_once("://")?;
	let host_part = rest.split('/').next()?;
	host_part.rsplit_once('@').map(|(user, _)| user)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn credentials_in_repo_urls() {
		let url = "https://oauth2:glpat-x@git.example/a/b.git";
		assert_eq!(url_credentials(url), Some("oauth2:glpat-x"));
		assert_eq!(url_credentials("git@git.example:a/b.git"), None);
		assert_eq!(url_credentials("https://git.example/a@b.git"), None);
	}
}
