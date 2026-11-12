//! The plans of a proposal, written by the planner (planner.rs). Every
//! version stays, the newest is the one an admin approves or sends back
//! with a change. A plan is cleaned like a proposal and capped at
//! `MAX_PLAN` characters before anyone sees it: what the admin reads is
//! exactly what genco's agent gets. Lines like `Delete: database shop_old`
//! name the deletions the approved plan confirms: they run without
//! another confirmation while genco's agent works on it, once each
//! (actions.rs); any other deletion still asks.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::{clean_text, look_alike};
use crate::actions::Kind;
use crate::db::now;
use crate::error::Result;

/// The longest plan, in characters.
pub const MAX_PLAN: usize = 4000;
/// Versions of one proposal, changes after that are refused.
pub const MAX_PLANS: i64 = 10;

/// The kinds a `Delete:` line names, as the planner writes them; "mail
/// domain" before "domain".
const DELETABLE: &[(&str, Kind)] = &[
	("mail domain", Kind::MailDomain),
	("mailbox", Kind::Mailbox),
	("domain", Kind::Domain),
	("database", Kind::Database),
	("group", Kind::Group),
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
	pub id: i64,
	/// 1 for the first plan of the proposal
	pub version: i64,
	/// the plan, cleaned; with `refused` why the planner wrote none
	pub text: String,
	pub refused: bool,
	/// invisible and control characters removed from the planner's text
	pub removed_chars: i64,
	/// look-alikes of ASCII letters may hide in it (`look_alike`)
	pub mixed_scripts: bool,
	/// the deletions it confirms, e.g. "database shop_old"
	pub deletes: Vec<String>,
	/// `Delete:` lines genco doesn't understand, those still ask
	pub other_deletes: Vec<String>,
	/// the admin's change that led to this version
	pub note: Option<String>,
	pub note_by: Option<String>,
	pub created_at: i64,
}

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Plan> {
	let text: String = row.get(2)?;
	let refused: bool = row.get(3)?;
	let (deletes, other_deletes) = if refused {
		(vec![], vec![])
	} else {
		let (found, other) = deletions(&text);
		(found.into_iter().map(|d| d.entry()).collect(), other)
	};
	Ok(Plan {
		id: row.get(0)?,
		version: row.get(1)?,
		mixed_scripts: look_alike(&text),
		text,
		refused,
		removed_chars: row.get(4)?,
		deletes,
		other_deletes,
		note: row.get(5)?,
		note_by: row.get(6)?,
		created_at: row.get(7)?,
	})
}

const SELECT: &str = "SELECT id, (SELECT count(*) FROM proposal_plans o
	WHERE o.proposal_id = p.proposal_id AND o.id <= p.id), text, refused,
	removed_chars, note, note_by, created_at FROM proposal_plans p";

/// The plans of a proposal, oldest first.
pub fn of(conn: &Connection, proposal_id: i64) -> rusqlite::Result<Vec<Plan>> {
	conn.prepare(&format!("{SELECT} WHERE proposal_id = ?1 ORDER BY id"))?
		.query_map([proposal_id], from_row)?
		.collect()
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<Plan>> {
	conn.query_row(&format!("{SELECT} WHERE id = ?1"), [id], from_row)
		.optional()
}

/// What the planner wrote, cleaned and checked: the plan, or why there is
/// none. `Err` with the reason when the plan is too long.
pub fn clean(text: &str, refused: bool) -> std::result::Result<New, String> {
	let (text, removed) = clean_text(text);
	let text = text.trim().to_string();
	if text.is_empty() {
		return Err("the plan is empty".into());
	}
	let chars = text.chars().count();
	if chars > MAX_PLAN {
		return Err(format!(
			"the plan has {chars} characters, at most {MAX_PLAN} are allowed"
		));
	}
	Ok(New {
		text,
		refused,
		removed,
	})
}

/// A plan before it is stored, see `clean`.
#[derive(Debug)]
pub struct New {
	pub text: String,
	pub refused: bool,
	pub removed: usize,
}

/// Stores the next version with the change that led to it, returns its
/// id.
pub fn add(
	conn: &Connection,
	proposal_id: i64,
	plan: &New,
	note: Option<(&str, &str)>,
) -> rusqlite::Result<i64> {
	conn.execute(
		"INSERT INTO proposal_plans (proposal_id, text, refused,
		removed_chars, note, note_by, created_at)
		VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
		params![
			proposal_id,
			plan.text,
			plan.refused,
			plan.removed,
			note.map(|n| n.0),
			note.map(|n| n.1),
			now()
		],
	)?;
	Ok(conn.last_insert_rowid())
}

/// A deletion a plan names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deletion {
	pub kind: Kind,
	/// lowercase: domain, database, group and mail domain names, a
	/// mailbox's address
	pub name: String,
}

impl Deletion {
	/// "database shop_old", how the panel shows it and the action stores
	/// it
	pub fn entry(&self) -> String {
		let word = DELETABLE
			.iter()
			.find(|(_, k)| *k == self.kind)
			.map(|(w, _)| *w)
			.unwrap_or("?");
		format!("{word} {}", self.name)
	}
}

/// The `Delete: <kind> <name>` lines of a plan, also as list items, bold
/// or with the name in backticks; and the `Delete:` lines that don't name
/// one thing genco can delete.
pub fn deletions(text: &str) -> (Vec<Deletion>, Vec<String>) {
	let mut found = vec![];
	let mut other = vec![];
	for line in text.lines() {
		let plain = line.replace("**", "").replace('`', "");
		let plain = plain.trim().trim_start_matches(['-', '*', '+']);
		let plain = plain
			.trim_start()
			.trim_start_matches(|c: char| c.is_ascii_digit());
		let plain = plain.trim_start_matches(['.', ')']).trim();
		let Some(rest) = plain
			.get(..7)
			.filter(|p| p.eq_ignore_ascii_case("delete:"))
			.map(|_| &plain[7..])
		else {
			continue;
		};
		match target(rest.trim()) {
			Some(d) if !found.contains(&d) => found.push(d),
			Some(_) => {}
			None => other.push(line.trim().to_string()),
		}
	}
	(found, other)
}

/// "database shop_old": one name after the kind, nothing else.
fn target(rest: &str) -> Option<Deletion> {
	let lower = rest.to_lowercase();
	let (kind, name) = DELETABLE.iter().find_map(|(word, kind)| {
		let name = lower.strip_prefix(word)?.strip_prefix(' ')?;
		Some((*kind, name.trim()))
	})?;
	let valid = !name.is_empty()
		&& name
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || "._-@+".contains(c));
	valid.then(|| Deletion {
		kind,
		name: name.into(),
	})
}

/// The approved plan's confirmation of a deletion genco's agent asks for
/// in `conversation`: while it works on the plan, once per line. Returns
/// the plan, its line and the admin who approved it.
pub fn confirmed(
	conn: &Connection,
	conversation: i64,
	deletion: &Deletion,
) -> Result<Option<Confirmed>> {
	let found: Option<(i64, i64, String)> = conn
		.query_row(
			"SELECT id, plan_id, decided_by FROM proposals
			WHERE conversation_id = ?1 AND status = 'approved'
			AND plan_id IS NOT NULL",
			[conversation],
			|r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
		)
		.optional()?;
	let Some((proposal, plan_id, by)) = found else {
		return Ok(None);
	};
	let Some(plan) = get(conn, plan_id)? else {
		return Ok(None);
	};
	let entry = deletion.entry();
	if !plan.deletes.contains(&entry) {
		return Ok(None);
	}
	let used: i64 = conn.query_row(
		"SELECT count(*) FROM pending_actions
		WHERE plan_id = ?1 AND plan_entry = ?2",
		params![plan_id, entry],
		|r| r.get(0),
	)?;
	Ok((used == 0).then_some(Confirmed {
		proposal,
		plan_id,
		entry,
		by,
	}))
}

#[derive(Debug)]
pub struct Confirmed {
	pub proposal: i64,
	pub plan_id: i64,
	/// "database shop_old"
	pub entry: String,
	/// the admin who approved the plan
	pub by: String,
}

#[cfg(test)]
mod tests {
	use super::*;

	fn entries(text: &str) -> (Vec<String>, Vec<String>) {
		let (found, other) = deletions(text);
		(found.iter().map(Deletion::entry).collect(), other)
	}

	#[test]
	fn delete_lines_name_what_the_plan_deletes() {
		let text = "1. Dump the database\n\
			Delete: database shop_old\n\
			- Delete: `mail domain` Old.Example.ch\n\
			* **Delete:** mailbox `info@old.example.ch`\n\
			3) delete: group shopold\n\
			Delete: database shop_old\n\
			Delete: everything old\n\
			Delete: domain a.ch and b.ch\n\
			We don't delete: domain keep.ch here";
		let (found, other) = entries(text);
		assert_eq!(
			found,
			[
				"database shop_old",
				"mail domain old.example.ch",
				"mailbox info@old.example.ch",
				"group shopold",
			]
		);
		assert_eq!(
			other,
			["Delete: everything old", "Delete: domain a.ch and b.ch"]
		);
	}

	#[test]
	fn a_plan_is_cleaned_and_capped() {
		let new = clean("Restart\u{200B} nginx\r\n", false).unwrap();
		assert_eq!((new.text.as_str(), new.removed), ("Restart nginx", 1));
		assert!(clean(&"x".repeat(MAX_PLAN + 1), false).is_err());
		assert!(clean(&"ä".repeat(MAX_PLAN), false).is_ok());
		assert!(clean(" \n", false).is_err());
	}
}
