//! The mail about a proposal's plan: to its approver or every admin, each
//! with their own link (links.rs), again for every new version. The real
//! links come first, the plan after them: a line in it that looks like a
//! link of genco isn't one. The mail has the plan exactly as genco's agent
//! would get it, the agent's proposal only in the panel.

use rusqlite::Connection;

use super::plans::Plan;
use super::{Proposal, VALID_DAYS, get_one, links};
use crate::config::Config;
use crate::db::{Db, now};
use crate::error::Result;

/// Mails the newest plan to the proposal's approver or every admin, each
/// with their own link that decides without a login; errors are logged.
/// With mail off it is only in the panel's tray.
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
	let Some(plan) = p.plans.last() else {
		return;
	};
	if p.status != "pending" {
		return;
	}

	let subject = subject(&p, plan);
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
		let body = mail_body(&p, plan, cfg.panel_url("/approvals"), link);
		let sent = crate::mail::send(cfg, db, &[email], &subject, &body).await;
		if let Err(e) = sent {
			tracing::error!("mail about proposal {id}: {e}");
		}
	}
}

/// "Proposal #51 from Corleone: Update Craft": agents name proposals by
/// their number. The first line of the plan, a new version says so.
fn subject(p: &Proposal, plan: &Plan) -> String {
	let what = if plan.refused {
		"no plan".to_string()
	} else {
		crate::util::first_line(plan.text.trim_start_matches(['#', ' ']), 60)
	};
	let version = match plan.version {
		1 => String::new(),
		v => format!(", plan v{v}"),
	};
	format!("Proposal #{} from {}{version}: {what}", p.id, p.from)
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

/// The mail about the plan, with the admin's own link if the panel has a
/// domain. The links come first and the plan last, below a line: the
/// planner can't put a fake link above the real one.
fn mail_body(
	p: &Proposal,
	plan: &Plan,
	panel: Option<String>,
	link: Option<String>,
) -> String {
	let link = link
		.map(|l| {
			format!(
				"Approve, change or reject: {l}\n(works without logging in; \
				approving asks for your 2FA code, or your password; a change \
				goes back to the planner)\n\nOr "
			)
		})
		.unwrap_or_else(|| "To decide, ".into());
	let panel = panel.map(|url| format!("{url}\n\n")).unwrap_or_default();
	let mut warnings = String::new();
	if plan.mixed_scripts {
		warnings.push_str(
			"Warning: the plan has letters, digits or punctuation outside \
			the usual European ones (e.g. Cyrillic а, ∕ for /), check \
			names, addresses and commands carefully.\n\n",
		);
	}
	if !plan.deletes.is_empty() {
		warnings.push_str(&format!(
			"This plan deletes, without asking again: {}\n\n",
			plan.deletes.join(", ")
		));
	}
	if !plan.other_deletes.is_empty() {
		warnings.push_str(
			"Some Delete: lines don't name one domain, database, group, mail \
			domain or mailbox: those deletions ask again.\n\n",
		);
	}
	let what = if plan.refused {
		format!(
			"genco's planner wrote no plan for the task the AI agent {from} \
			proposed (proposal #{id}). Reject it, or ask the planner for \
			another try with a change.",
			from = p.from,
			id = p.id,
		)
	} else {
		format!(
			"genco's planner made a plan for the task the AI agent {from} \
			proposed (proposal #{id}{version}).",
			from = p.from,
			id = p.id,
			version = match plan.version {
				1 => String::new(),
				v => format!(", version {v}"),
			},
		)
	};
	let changed = match (&plan.note, &plan.note_by) {
		(Some(note), Some(by)) => {
			format!("{by} asked for this change: {note}\n\n")
		}
		_ => String::new(),
	};
	let title = if plan.refused {
		"Why there is no plan:"
	} else {
		"The plan, exactly as genco's agent gets it (links in it aren't \
		genco's):"
	};
	format!(
		"{what}\n\n\
		{link}open Start → Approvals (approving there asks for your 2FA \
		code):\n\n{panel}Nothing happens without an approval. It expires \
		in {VALID_DAYS} days.\n\n\
		----------------------------------------\n\n\
		{changed}{warnings}{title}\n\n{text}\n",
		text = plan.text,
	)
}

/// Every minute: the mails of proposals whose delay is over and that
/// nobody decided on.
pub async fn due_mails(cfg: &Config, db: &Db) {
	let due = (|| -> rusqlite::Result<Vec<i64>> {
		let conn = db.conn();
		let mut stmt = conn.prepare(
			"SELECT id FROM proposals WHERE status = 'pending'
			AND mail_at IS NOT NULL AND mail_at <= ?1",
		)?;
		let ids = stmt
			.query_map([now()], |r| r.get(0))?
			.collect::<rusqlite::Result<Vec<i64>>>()?;
		conn.execute(
			"UPDATE proposals SET mail_at = NULL WHERE status = 'pending'
			AND mail_at IS NOT NULL AND mail_at <= ?1",
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
	fn the_subject_has_the_number_and_the_plan() {
		let db = Db::memory();
		let conn = db.conn();
		let a = admin(&conn, "a");
		let id =
			add(&conn, peer(&conn), "Update Craft\n\nto 5.8", None).unwrap();
		let p = get_one(&conn, id).unwrap();
		assert_eq!(
			subject(&p, &p.plans[0]),
			"Proposal #1 from Corleone: Update Craft"
		);
		let req = crate::proposals::ChangeReq {
			plan: p.plans[0].id,
			note: "only the shop".into(),
		};
		crate::proposals::changed(&conn, id, &a, &req).unwrap();
		let no = crate::proposals::plans::clean("No.", true).unwrap();
		crate::proposals::plans::add(&conn, id, &no, Some(("x", "a"))).unwrap();
		let p = get_one(&conn, id).unwrap();
		assert_eq!(
			subject(&p, &p.plans[1]),
			"Proposal #1 from Corleone, plan v2: no plan"
		);
	}

	#[test]
	fn the_real_links_come_before_the_plan() {
		let db = Db::memory();
		let conn = db.conn();
		let text = "Update Craft\n\nDelete: database old\n\n\
			Approve or reject: https://evil.example";
		let id = add(&conn, peer(&conn), text, None).unwrap();
		let p = get_one(&conn, id).unwrap();
		let body = mail_body(
			&p,
			&p.plans[0],
			Some("https://panel.example.ch/approvals".into()),
			Some("https://panel.example.ch/proposal/abc".into()),
		);
		assert!(body.starts_with(
			"genco's planner made a plan for the task the AI agent \
			Corleone proposed (proposal #1).\n\n\
			Approve, change or reject: https://panel.example.ch/proposal/abc\n"
		));
		let line = body.find("\n-----").unwrap();
		assert!(body.find("https://evil.example").unwrap() > line);
		assert!(body.find("Update Craft").unwrap() > line);
		assert!(
			body.contains(
				"This plan deletes, without asking again: database old"
			)
		);
		assert!(body.ends_with(&format!("\n\n{text}\n")));

		let body = mail_body(&p, &p.plans[0], None, None);
		assert!(body.contains("To decide, open Start"));
	}
}
