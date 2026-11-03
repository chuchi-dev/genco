//! Restarts without downtime: the app's other slot starts with the new
//! release beside the live one, nginx switches to it once it listens,
//! then the old one stops (see render::Slot).

use std::time::Duration;

use crate::hosting::Domain;
use crate::render::Slot;

use super::*;

/// How long a new process may take until it listens.
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// nginx's old workers send what they started to the old process, they
/// had a moment to finish before it stops.
const DRAIN: Duration = Duration::from_secs(5);

pub(super) struct Switch<'a> {
	pub(super) domain: &'a Domain,
	pub(super) start: &'a str,
	pub(super) port: u16,
	pub(super) to: Slot,
	pub(super) stop: &'a str,
}

impl Switch<'_> {
	/// Fails when the new process doesn't come up, the old one stays live.
	pub(super) async fn run(&self, ctx: &Ctx, log: &mut Log) -> Res<()> {
		let name = &self.domain.name;
		// apply writes nginx and the units from the database, it waits
		// until the live slot is settled
		let _lock = job::lock(crate::apply::LOCK).await?;

		// another process there would pass for the new one
		systemctl(log, "stop", self.start).await;
		if listening(self.port).await {
			return Err(format!(
				"{name}: port {} is taken by another process, the app \
				keeps running as it was",
				self.port
			));
		}
		systemctl(log, "start", self.start).await;
		if !self.started().await {
			self.abort(log).await;
			return Err(format!(
				"{name}: the new process didn't listen on port {} within \
				{} s (journalctl -u {}), the old one stays live",
				self.port,
				START_TIMEOUT.as_secs(),
				self.start
			));
		}

		set_slot(ctx, self.domain.id, self.to)?;
		if let Err(e) = point_nginx(ctx, self.domain).await {
			let _ = set_slot(ctx, self.domain.id, self.to.other());
			self.abort(log).await;
			return Err(format!("{name}: {e}, the old process stays live"));
		}
		log.line(&format!("nginx sends {name} to port {}", self.port))
			.await;

		// the live slot starts at boot
		systemctl(log, "enable", self.start).await;
		systemctl(log, "disable", self.stop).await;
		tokio::time::sleep(DRAIN).await;
		systemctl(log, "stop", self.stop).await;
		Ok(())
	}

	async fn started(&self) -> bool {
		let until = tokio::time::Instant::now() + START_TIMEOUT;
		while tokio::time::Instant::now() < until {
			if listening(self.port).await {
				return true;
			}
			tokio::time::sleep(Duration::from_millis(250)).await;
		}
		false
	}

	/// Stops the new process; not a failed unit the minute check reports.
	async fn abort(&self, log: &mut Log) {
		systemctl(log, "stop", self.start).await;
		let _ = Command::new("systemctl")
			.args(["reset-failed", self.start])
			.status()
			.await;
	}
}

async fn listening(port: u16) -> bool {
	let connect = tokio::net::TcpStream::connect(("127.0.0.1", port));
	matches!(
		tokio::time::timeout(Duration::from_secs(2), connect).await,
		Ok(Ok(_))
	)
}

fn set_slot(ctx: &Ctx, domain: i64, slot: Slot) -> Res<()> {
	ctx.db
		.conn()
		.execute(
			"UPDATE domains SET slot_b = ?1 WHERE id = ?2",
			params![slot == Slot::B, domain],
		)
		.map(|_| ())
		.map_err(|e| e.to_string())
}

/// Writes the domain's nginx config as the database has it now and
/// reloads nginx; the old file comes back when nginx rejects the new one.
async fn point_nginx(ctx: &Ctx, d: &Domain) -> Res<()> {
	let group = hosting::find_group(&ctx.db.conn(), d.group_id)
		.map_err(|e| e.to_string())?;
	let certs = crate::host::certs(std::slice::from_ref(&group));
	let path = render::nginx_config(&d.name);
	let file = render::group_files(&group, &certs)
		.into_iter()
		.find(|f| f.path == path)
		.ok_or("the domain has no nginx config")?;

	let old = std::fs::read_to_string(&path).ok();
	std::fs::write(&path, &file.content)
		.map_err(|e| format!("writing {}: {e}", path.display()))?;
	let reloaded = match util::run(&["nginx", "-t", "-q"]).await {
		Ok(_) => util::run(&["systemctl", "reload", "nginx"]).await,
		Err(e) => Err(e),
	};
	if let Err(e) = reloaded {
		if let Some(old) = old {
			let _ = std::fs::write(&path, old);
		}
		return Err(format!("nginx: {e}"));
	}
	Ok(())
}
