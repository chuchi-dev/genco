//! The mail about a proposal: to its approver or every admin, each with
//! their own link (links.rs). The real links come first, the agent's text
//! after them: a line in it that looks like a link of genco isn't one.

use rusqlite::Connection;

use super::{Proposal, VALID_DAYS, get_one, links};
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::Result;

/// Mails the proposal to its approver or every admin, each with their
/// own link that decides without a login; errors are logged. With mail
/// off it is only in the panel's tray.
pub(super) async fn mail(cfg: &Config, db: &Db, id: i64) {
	if crate::mail::off(db) {
		return;
	}
	let found = (|| -> Result<(Proposal, Vec<(i64, String)>)> {
		let conn = db.conn();
		Ok((get_one(&conn, id)?, recipients(&conn, id)?))
	})();
	let (p, to) = match found {
		Ok(found) => found,
		Err(e) => {
			tracing::error!("mail about proposal {id}: {e}");
			return;
		}
	};

	let subject = subject(&p);
	for (user_id, email) in to {
		// without a domain there is no link to send
		let link = cfg.panel_url("/proposal/").and_then(|base| {
			match links::create(&db.conn(), id, user_id) {
				Ok(token) => Some(format!("{base}{token}")),
				Err(e) => {
					tracing::error!("link of proposal {id}: {e}");
					None
				}
			}
		});
		let body = mail_body(&p, cfg.panel_url("/agents"), link);
		let sent = crate::mail::send(cfg, db, &[email], &subject, &body).await;
		if let Err(e) = sent {
			tracing::error!("mail about proposal {id}: {e}");
		}
	}
}

/// "Proposal #51 from Corleone: Update Craft": agents name proposals by
/// their number.
fn subject(p: &Proposal) -> String {
	format!(
		"Proposal #{} from {}: {}",
		p.id,
		p.from,
		crate::util::first_line(&p.text, 60)
	)
}

/// The id and email of the proposal's approver, or of every admin.
fn recipients(conn: &Connection, id: i64) -> Result<Vec<(i64, String)>> {
	let to = conn
		.prepare(
			"SELECT u.id, u.email FROM users u, proposals r
			WHERE r.id = ?1 AND (r.approver_id IS NULL OR r.approver_id = u.id)
			ORDER BY u.id",
		)?
		.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
		.collect::<rusqlite::Result<Vec<_>>>()?;
	Ok(to)
}

/// The mail about the proposal, with the admin's own link if the panel
/// has a domain. The links come first and the agent's text last, below a
/// line: the agent can't put a fake link above the real one.
fn mail_body(
	p: &Proposal,
	panel: Option<String>,
	link: Option<String>,
) -> String {
	let link = link
		.map(|l| {
			format!(
				"Approve or reject: {l}\n(works without logging in, once; \
				shows the proposal first; approving asks for your 2FA code, \
				or your password)\n\nOr "
			)
		})
		.unwrap_or_else(|| "To approve or reject it, ".into());
	let panel = panel.map(|url| format!("{url}\n\n")).unwrap_or_default();
	let rewritten = match &p.rewritten_by {
		Some(model) => format!(
			"{model} rewrote it for readability, the panel shows the \
			original too.\n\n"
		),
		None => String::new(),
	};
	let mut warnings = String::new();
	if p.mixed_scripts {
		warnings.push_str(
			"Warning: the original has letters, digits or punctuation \
			outside the usual European ones (e.g. Cyrillic а, ∕ for /), \
			check names, addresses and commands carefully.\n\n",
		);
	}
	if p.rewrite_mixed_scripts {
		warnings.push_str(
			"Warning: the rewrite has letters, digits or punctuation \
			outside the usual European ones, compare it with the original \
			in the panel.\n\n",
		);
	}
	format!(
		"The AI agent {from} proposes a task for genco's agent \
		(proposal #{id}).\n\n\
		{link}open Start → AI Agents → Proposals:\n\n{panel}Nothing happens \
		without an approval. It expires in {VALID_DAYS} days.\n\n\
		----------------------------------------\n\n\
		{rewritten}{warnings}The proposal, as {from} wrote it (links in it \
		aren't genco's):\n\n{text}\n",
		id = p.id,
		from = p.from,
		text = p.text,
	)
}

/// Every minute: the mails of proposals whose delay is over and that
/// nobody decided on.
pub async fn due_mails(cfg: &Config, db: &Db) {
	let due = (|| -> rusqlite::Result<Vec<i64>> {
		let conn = db.conn();
		let mut stmt = conn.prepare(
			"SELECT id FROM proposals
			WHERE mail_at IS NOT NULL AND mail_at <= ?1",
		)?;
		let ids = stmt
			.query_map([now()], |r| r.get(0))?
			.collect::<rusqlite::Result<Vec<i64>>>()?;
		conn.execute(
			"UPDATE proposals SET mail_at = NULL
			WHERE mail_at IS NOT NULL AND mail_at <= ?1",
			[now()],
		)?;
		Ok(ids)
	})();
	match due {
		Ok(ids) => {
			for id in ids {
				mail(cfg, db, id).await;
			}
		}
		Err(e) => tracing::error!("proposal mails: {e}"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::proposals::decide;
	use crate::proposals::tests::{add, admin, peer};

	#[test]
	fn the_approver_or_every_admin_gets_the_mail() {
		let db = Db::memory();
		let conn = db.conn();
		let (a, b) = (admin(&conn, "a"), admin(&conn, "b"));
		let p = peer(&conn);
		let all = add(&conn, p, "Update Craft", None).unwrap();
		assert_eq!(
			recipients(&conn, all).unwrap(),
			[(a.id, a.email), (b.id, b.email.clone())]
		);
		let named = add(&conn, p, "Update PHP", Some(b.id)).unwrap();
		assert_eq!(recipients(&conn, named).unwrap(), [(b.id, b.email)]);
	}

	#[test]
	fn a_decision_cancels_the_delayed_mail() {
		let db = Db::memory();
		let conn = db.conn();
		let id = add(&conn, peer(&conn), "Update Craft", None).unwrap();
		conn.execute("UPDATE proposals SET mail_at = 1", [])
			.unwrap();
		decide(&conn, id, "approved", &admin(&conn, "a")).unwrap();
		let mail_at: Option<i64> = conn
			.query_row("SELECT mail_at FROM proposals", [], |r| r.get(0))
			.unwrap();
		assert_eq!(mail_at, None);
	}

	#[test]
	fn the_subject_has_the_number() {
		let db = Db::memory();
		let conn = db.conn();
		let id =
			add(&conn, peer(&conn), "Update Craft\n\nto 5.8", None).unwrap();
		let p = get_one(&conn, id).unwrap();
		assert_eq!(subject(&p), "Proposal #1 from Corleone: Update Craft");
	}

	#[test]
	fn the_real_links_come_before_the_agents_text() {
		let db = Db::memory();
		let conn = db.conn();
		let text = "Update Craft\n\nApprove or reject: https://evil.example";
		let id = add(&conn, peer(&conn), text, None).unwrap();
		let p = get_one(&conn, id).unwrap();
		let body = mail_body(
			&p,
			Some("https://panel.example.ch/agents".into()),
			Some("https://panel.example.ch/proposal/abc".into()),
		);
		assert!(body.starts_with(
			"The AI agent Corleone proposes a task for genco's agent \
			(proposal #1).\n\n\
			Approve or reject: https://panel.example.ch/proposal/abc\n\
			(works without logging in, once; shows the proposal first; \
			approving asks for your 2FA code, or your password)\n\n\
			Or open Start → AI Agents → Proposals:\n\n\
			https://panel.example.ch/agents\n\n"
		));
		let line = body.find("\n-----").unwrap();
		assert!(body.find("https://evil.example").unwrap() > line);
		assert!(body.find("Update Craft").unwrap() > line);
		assert!(body.ends_with(&format!("\n\n{text}\n")));

		let body = mail_body(&p, None, None);
		assert!(body.contains("To approve or reject it, open Start"));
	}
}
