//! Environment variables of a Node or Rust app: names and values per
//! domain, written to a file only root reads, which the app's unit loads
//! (`EnvironmentFile`, systemd reads it before it drops to the group's
//! user). Secret values never go back to the browser or the terminal.
//! Saving in the panel applies at once: the file is written and the app
//! restarts with the new variables.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::apply;
use crate::config::Config;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::events::{Event, Events};
use crate::users::Authed;

/// genco sets these itself.
const RESERVED: &[&str] = &["HOST", "PORT"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
	pub name: String,
	#[serde(default)]
	pub value: String,
	/// shown as set, never with its value
	#[serde(default)]
	pub secret: bool,
}

pub fn parse(raw: &str) -> Vec<EnvVar> {
	serde_json::from_str(raw).unwrap_or_default()
}

fn load(conn: &Connection, domain_id: i64) -> Result<Vec<EnvVar>> {
	let raw: String = conn.query_row(
		"SELECT env FROM domains WHERE id = ?1",
		[domain_id],
		|r| r.get(0),
	)?;
	Ok(parse(&raw))
}

/// For the browser and the terminal: secrets without their value.
fn masked(vars: &[EnvVar]) -> Vec<EnvVar> {
	vars.iter()
		.map(|v| EnvVar {
			value: if v.secret {
				String::new()
			} else {
				v.value.clone()
			},
			..v.clone()
		})
		.collect()
}

fn check(vars: &[EnvVar]) -> Result<()> {
	for (i, v) in vars.iter().enumerate() {
		let ok = v.name.len() <= 100
			&& v.name
				.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
			&& v.name
				.chars()
				.all(|c| c.is_ascii_alphanumeric() || c == '_');
		if !ok {
			return Err(Error::Invalid(format!(
				"{:?} is no variable name (letters, digits and _)",
				v.name
			)));
		}
		if RESERVED.contains(&v.name.as_str()) {
			return Err(Error::Invalid(format!(
				"genco sets {} itself (the app listens on HOST:PORT)",
				v.name
			)));
		}
		if vars[..i].iter().any(|o| o.name == v.name) {
			return Err(Error::Invalid(format!("{} is there twice", v.name)));
		}
		if v.value.contains(['\n', '\r', '\0']) || v.value.len() > 64 * 1024 {
			return Err(Error::Invalid(format!(
				"the value of {} has a line break or is too long",
				v.name
			)));
		}
	}
	Ok(())
}

/// Replaces the variables; a secret sent without a value keeps the one it
/// had.
fn set(conn: &Connection, domain_id: i64, mut vars: Vec<EnvVar>) -> Result<()> {
	let old = load(conn, domain_id)?;
	for v in &mut vars {
		v.name = v.name.trim().to_string();
		if v.secret
			&& v.value.is_empty()
			&& let Some(o) = old.iter().find(|o| o.name == v.name)
		{
			v.value = o.value.clone();
		}
	}
	check(&vars)?;
	conn.execute(
		"UPDATE domains SET env = ?1 WHERE id = ?2",
		params![serde_json::to_string(&vars).unwrap(), domain_id],
	)?;
	Ok(())
}

/// Sets one secret variable, e.g. an SMTP URL with its password that
/// genco writes so it never passes the terminal.
pub fn set_secret(
	conn: &Connection,
	domain_id: i64,
	name: &str,
	value: &str,
) -> Result<()> {
	let mut vars = load(conn, domain_id)?;
	vars.retain(|v| v.name != name);
	vars.push(EnvVar {
		name: name.into(),
		value: value.into(),
		secret: true,
	});
	set(conn, domain_id, vars)
}

/// The file systemd loads: `NAME="value"`, quotes and backslashes escaped,
/// nothing expanded. With the group's SMTP login (`user`, `password`)
/// the app gets `GENCO_SMTP_*` to send DKIM signed mail.
pub fn file(vars: &[EnvVar], smtp: Option<(&str, &str, &str)>) -> String {
	let mut out = String::from("# written by genco: change it in the panel\n");
	let mut all = vec![];
	if let Some((host, user, password)) = smtp {
		for (name, value) in [
			("GENCO_SMTP_HOST", host),
			("GENCO_SMTP_PORT", "587"),
			("GENCO_SMTP_USER", user),
			("GENCO_SMTP_PASSWORD", password),
		] {
			all.push(EnvVar {
				name: name.into(),
				value: value.into(),
				secret: true,
			});
		}
	}
	all.extend(vars.iter().cloned());
	for v in &all {
		let value = v.value.replace('\\', "\\\\").replace('"', "\\\"");
		out.push_str(&format!("{}=\"{value}\"\n", v.name));
	}
	out
}

async fn get_env(
	_authed: Authed,
	State(db): State<Db>,
	Path(id): Path<i64>,
) -> Result<Json<Vec<EnvVar>>> {
	Ok(Json(masked(&load(&db.conn(), id)?)))
}

/// A row of the panel: `from` is the name it had when it loaded, a
/// renamed secret keeps its value.
#[derive(Debug, Deserialize)]
struct EnvRow {
	name: String,
	#[serde(default)]
	value: String,
	#[serde(default)]
	secret: bool,
	#[serde(default)]
	from: Option<String>,
}

/// Stores the variables and applies: the app restarts with them.
async fn set_env(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	State(events): State<Events>,
	Path(id): Path<i64>,
	Json(rows): Json<Vec<EnvRow>>,
) -> Result<Json<apply::Report>> {
	// the connection mustn't be held over the apply
	store(&db.conn(), id, rows)?;
	let report = apply::apply(&cfg, &db).await;
	events.send(Event::Tree);
	Ok(Json(report))
}

/// The panel's rows as the domain's variables; an empty value keeps a
/// secret's.
fn store(conn: &Connection, id: i64, rows: Vec<EnvRow>) -> Result<()> {
	let old = load(conn, id)?;
	let mut vars = vec![];
	for r in rows {
		let was = r
			.from
			.as_deref()
			.and_then(|f| old.iter().find(|o| o.name == f));
		let value = match was {
			// a secret's value never left the server: an empty one keeps it
			Some(o) if r.value.is_empty() && o.secret && r.secret => {
				o.value.clone()
			}
			Some(o) if r.value.is_empty() && o.secret => {
				return Err(Error::Invalid(format!(
					"{} isn't secret anymore: type its value again",
					r.name.trim()
				)));
			}
			_ => r.value,
		};
		vars.push(EnvVar {
			name: r.name.trim().to_string(),
			value,
			secret: r.secret,
		});
	}
	check(&vars)?;
	conn.execute(
		"UPDATE domains SET env = ?1 WHERE id = ?2",
		params![serde_json::to_string(&vars).unwrap(), id],
	)?;
	Ok(())
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/domains/{id}/env", get(get_env).post(set_env))
}

/// `genco domain env <domain> [--set NAME=VALUE] [--secret NAME=VALUE]
/// [--unset NAME]`: changes, then prints them.
pub fn cli(
	conn: &Connection,
	domain_id: i64,
	sets: Vec<String>,
	secrets: Vec<String>,
	unsets: Vec<String>,
) -> Result<String> {
	let mut vars = load(conn, domain_id)?;
	let changed = !(sets.is_empty() && secrets.is_empty() && unsets.is_empty());
	vars.retain(|v| !unsets.contains(&v.name));
	for (pair, secret) in sets
		.iter()
		.map(|p| (p, false))
		.chain(secrets.iter().map(|p| (p, true)))
	{
		let (name, value) = pair
			.split_once('=')
			.ok_or_else(|| Error::Invalid(format!("{pair}: NAME=value")))?;
		let var = EnvVar {
			name: name.into(),
			value: value.into(),
			secret,
		};
		match vars.iter_mut().find(|v| v.name == name) {
			Some(v) => *v = var,
			None => vars.push(var),
		}
	}
	if changed {
		set(conn, domain_id, vars.clone())?;
	}
	let mut out: Vec<String> = masked(&vars)
		.iter()
		.map(|v| {
			if v.secret {
				format!("{}=(secret)", v.name)
			} else {
				format!("{}={}", v.name, v.value)
			}
		})
		.collect();
	if changed {
		out.push("changed, run genco apply (the app restarts)".into());
	}
	Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn var(name: &str, value: &str, secret: bool) -> EnvVar {
		EnvVar {
			name: name.into(),
			value: value.into(),
			secret,
		}
	}

	#[test]
	fn names_and_values_are_checked() {
		assert!(check(&[var("DATABASE_URL", "x", false)]).is_ok());
		assert!(check(&[var("1X", "x", false)]).is_err());
		assert!(check(&[var("A-B", "x", false)]).is_err());
		assert!(check(&[var("PORT", "1", false)]).is_err());
		assert!(check(&[var("A", "x\ny", false)]).is_err());
		assert!(check(&[var("A", "x", false), var("A", "y", false)]).is_err());
	}

	#[test]
	fn the_file_quotes_values() {
		let f = file(&[var("A", r#"a "b" \c $D"#, false)], None);
		assert!(f.contains(r#"A="a \"b\" \\c $D""#));
	}

	#[test]
	fn secrets_keep_their_value_and_stay_hidden() {
		let db = Db::memory();
		let conn = db.conn();
		conn.execute(
			"INSERT INTO clients (name, created_at) VALUES ('c', 0)",
			[],
		)
		.unwrap();
		conn.execute(
			"INSERT INTO domain_groups (name, client_id, created_at)
			VALUES ('g', 1, 0)",
			[],
		)
		.unwrap();
		conn.execute(
			"INSERT INTO domains (group_id, name, kind, root, created_at)
			VALUES (1, 'a.ch', 'rust', 'current', 0)",
			[],
		)
		.unwrap();
		set(&conn, 1, vec![var("KEY", "s3cret", true)]).unwrap();
		// the browser sends it back without its value
		set(
			&conn,
			1,
			vec![var("KEY", "", true), var("MODE", "x", false)],
		)
		.unwrap();
		let vars = load(&conn, 1).unwrap();
		assert_eq!(vars[0].value, "s3cret");
		assert_eq!(masked(&vars)[0].value, "");
		assert_eq!(masked(&vars)[1].value, "x");
	}
}
