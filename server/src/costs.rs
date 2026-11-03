//! What the agent costs: every llm request with its tokens and price,
//! summed up per day for the server page.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use soe_llms::Usage;

use crate::AppState;
use crate::db::{Db, now};
use crate::error::Result;
use crate::users::Authed;

/// USD per million tokens: input, output, cache read, cache write (5
/// minutes). From Anthropic's and OpenRouter's price lists; models not
/// listed are counted without a price.
const PRICES: &[(&str, [f64; 4])] = &[
	("claude-opus-5.5", [4.0, 20.0, 0.20, 5.0]),
	("claude-sonnet-5.5", [2.0, 10.0, 0.20, 2.5]),
	("claude-haiku-5.5", [0.10, 0.50, 0.01, 0.125]),
	("gemini-4-argon", [2.0, 10.0, 0.10, 0.1875]),
	("gemini-3.8-flash", [0.825, 4.125, 0.0825, 0.0]),
	("gpt-6.1-sol", [2.0, 10.0, 0.10, 2.5]),
];

/// The price in USD, `None` for a model without a known price.
fn price(model: &str, u: &Usage) -> Option<f64> {
	let name = model.rsplit('/').next().unwrap_or(model);
	let (_, [input, output, read, write]) =
		PRICES.iter().find(|(m, _)| *m == name)?;
	// the input count includes the cached tokens
	let plain = u
		.input_tokens
		.saturating_sub(u.cached_input_tokens)
		.saturating_sub(u.cache_write_tokens);
	let tokens = |n: u32, per_million: f64| n as f64 * per_million / 1e6;
	Some(
		tokens(plain, *input)
			+ tokens(u.output_tokens, *output)
			+ tokens(u.cached_input_tokens, *read)
			+ tokens(u.cache_write_tokens, *write),
	)
}

pub fn record(
	conn: &Connection,
	conversation_id: i64,
	model: &str,
	u: &Usage,
) -> rusqlite::Result<()> {
	conn.execute(
		"INSERT INTO llm_usage (at, conversation_id, model, input_tokens,
		output_tokens, cache_read_tokens, cache_write_tokens, cost_usd)
		VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
		params![
			now(),
			conversation_id,
			model,
			u.input_tokens,
			u.output_tokens,
			u.cached_input_tokens,
			u.cache_write_tokens,
			price(model, u)
		],
	)?;
	Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
	/// local date, YYYY-MM-DD
	pub date: String,
	/// conversations of the admins, routines, alerts
	pub chat: f64,
	pub routine: f64,
	pub alert: f64,
	pub requests: i64,
	pub tokens: i64,
	/// requests of models without a price
	pub unpriced: i64,
}

/// The last `days` days with any request, oldest first.
fn daily(conn: &Connection, days: i64) -> rusqlite::Result<Vec<Day>> {
	let mut stmt = conn.prepare(
		"SELECT date(u.at / 1000, 'unixepoch', 'localtime') AS day,
		sum(CASE WHEN coalesce(c.kind, 'chat') = 'chat' THEN u.cost_usd END),
		sum(CASE WHEN c.kind = 'routine' THEN u.cost_usd END),
		sum(CASE WHEN c.kind = 'alert' THEN u.cost_usd END),
		count(*),
		sum(u.input_tokens + u.output_tokens),
		sum(u.cost_usd IS NULL)
		FROM llm_usage u LEFT JOIN conversations c ON c.id = u.conversation_id
		WHERE u.at >= ?1 GROUP BY day ORDER BY day",
	)?;
	let since = now() - days * 24 * 60 * 60 * 1000;
	stmt.query_map([since], |r| {
		Ok(Day {
			date: r.get(0)?,
			chat: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
			routine: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
			alert: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
			requests: r.get(4)?,
			tokens: r.get(5)?,
			unpriced: r.get(6)?,
		})
	})?
	.collect()
}

#[derive(Debug, Deserialize)]
struct CostQuery {
	days: Option<i64>,
}

#[derive(Debug, Serialize)]
struct CostsResp {
	/// the server's date, its days are in its time zone
	today: String,
	days: Vec<Day>,
}

async fn daily_route(
	_authed: Authed,
	State(db): State<Db>,
	Query(q): Query<CostQuery>,
) -> Result<Json<CostsResp>> {
	let conn = db.conn();
	let today: String =
		conn.query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))?;
	let days = daily(&conn, q.days.unwrap_or(30).clamp(1, 400))?;
	Ok(Json(CostsResp { today, days }))
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/costs", get(daily_route))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn usage(input: u32, output: u32, read: u32, write: u32) -> Usage {
		let mut u = Usage::default();
		u.input_tokens = input;
		u.output_tokens = output;
		u.cached_input_tokens = read;
		u.cache_write_tokens = write;
		u
	}

	#[test]
	fn cached_tokens_cost_less() {
		let model = "anthropic/claude-opus-5.5";
		let fresh = price(model, &usage(1_000_000, 0, 0, 0)).unwrap();
		assert!((fresh - 4.0).abs() < 1e-9);
		let cached = price(model, &usage(1_000_000, 0, 1_000_000, 0)).unwrap();
		assert!((cached - 0.20).abs() < 1e-9);
		let out = price("claude-opus-5.5", &usage(0, 1_000_000, 0, 0)).unwrap();
		assert!((out - 20.0).abs() < 1e-9);
		assert!(price("openai/gpt-x", &usage(1, 1, 0, 0)).is_none());
	}

	#[test]
	fn days_split_by_kind() {
		let db = Db::memory();
		let conn = db.conn();
		let id = crate::conversations::create(&conn, "hi").unwrap();
		record(&conn, id, "claude-opus-5.5", &usage(1_000_000, 0, 0, 0))
			.unwrap();
		record(&conn, 0, "other", &usage(10, 10, 0, 0)).unwrap();
		let days = daily(&conn, 1).unwrap();
		assert_eq!(days.len(), 1);
		assert!((days[0].chat - 4.0).abs() < 1e-9);
		assert_eq!(days[0].requests, 2);
		assert_eq!(days[0].unpriced, 1);
	}
}
