//! A small model rewrites a proposal into a short formatted version. A
//! rewrite that adds a name, address, number or command, or drops every
//! negation of the original, is refused: the admin would approve
//! something the agent didn't ask for. Then the original stays and the
//! panel shows why.

use std::collections::HashSet;
use std::time::Duration;

use soe_llms::{Input, Model, Output, Request, Role};

use super::{MAX_TEXT, clean_text};
use crate::config::Config;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::llm::Llm;

/// How long the agent's request waits for the rewrite, then the
/// original is used.
const REWRITE_TIMEOUT: Duration = Duration::from_secs(30);
const REWRITE_PROMPT: &str = include_str!("rewrite.md");
/// Fast, cheap models, the first a key in the config reaches rewrites.
const REWRITE_MODELS: &[&str] =
	&["google/gemini-3.8-flash", "anthropic/claude-haiku-5.5"];

/// A proposal before it is stored.
#[derive(Debug)]
pub(super) struct Draft {
	/// the agent's text, cleaned
	pub original: String,
	pub removed: usize,
	/// the rewrite, else the original
	pub text: String,
	pub rewritten_by: Option<&'static str>,
	/// why it wasn't rewritten
	pub note: Option<String>,
}

impl Draft {
	pub fn new(text: &str) -> Result<Self> {
		let (text, removed) = clean_text(text);
		let text = text.trim();
		if text.is_empty() {
			return Err(Error::Invalid("the proposal has no text".into()));
		}
		if text.len() > MAX_TEXT {
			return Err(Error::Invalid(format!(
				"a proposal has at most {MAX_TEXT} bytes"
			)));
		}
		Ok(Self {
			original: text.into(),
			removed,
			text: text.into(),
			rewritten_by: None,
			note: None,
		})
	}

	/// Takes the model's answer if it is usable, else the original stays
	/// with the reason as the note. `answer` fails with the note.
	pub fn rewritten(
		mut self,
		model: &'static str,
		answer: std::result::Result<String, String>,
	) -> Self {
		match answer.and_then(|a| checked_rewrite(&self.original, &a)) {
			Ok(text) => {
				self.text = text;
				self.rewritten_by = Some(model);
			}
			Err(note) => {
				tracing::warn!("proposal not rewritten: {note}");
				self.note = Some(note);
			}
		}
		self
	}
}

/// The model that rewrites proposals.
fn rewrite_model(cfg: &Config) -> Option<&'static str> {
	REWRITE_MODELS
		.iter()
		.copied()
		.find(|m| crate::llm::reachable(cfg, m))
}

/// The proposal is the only message, the prompt says it is data.
/// soe-llms' request has no limit on the output tokens, the timeout
/// bounds what a rewrite costs.
fn rewrite_request(model: &str, original: &str) -> Request {
	Request {
		input: vec![Input::Text {
			role: Role::User,
			content: original.into(),
		}],
		instructions: REWRITE_PROMPT.into(),
		model: Model::OpenRouter(Some(model.into())),
		user_id: "genco".into(),
		tools: vec![],
		reasoning_effort: None,
	}
}

/// The rewrite, cleaned like the original. Refused when it is empty, too
/// long or invents something; the error is the note.
fn checked_rewrite(
	original: &str,
	answer: &str,
) -> std::result::Result<String, String> {
	let (text, _) = clean_text(answer);
	let text = text.trim();
	if text.is_empty() {
		return Err("empty answer".into());
	}
	if text.len() > MAX_TEXT {
		return Err("dropped: too long".into());
	}
	if let Some(why) = invented(original, text) {
		return Err(format!("dropped: {why}"));
	}
	Ok(text.into())
}

/// Look-alike punctuation (see `look_alike`) as the ASCII it passes for,
/// so `example․com` with U+2024 is the address it shows.
fn ascii_punct(c: char) -> char {
	match c {
		'\u{FF01}'..='\u{FF5E}' => {
			char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
		}
		'\u{2010}'..='\u{2012}'
		| '\u{2043}'
		| '\u{2212}'
		| '\u{2E3A}'
		| '\u{2E3B}'
		| '\u{FE63}' => '-',
		'\u{2024}' | '\u{2025}' | '\u{2027}' | '\u{3002}' | '\u{FE52}' => '.',
		'\u{2044}' | '\u{2215}' => '/',
		'\u{2236}' | '\u{A789}' | '\u{FE55}' => ':',
		'\u{FE6B}' => '@',
		c => c,
	}
}

/// The whole words of the text, without Markdown and the punctuation
/// around them.
fn words(text: &str) -> Vec<String> {
	let text: String = text.chars().map(ascii_punct).collect();
	let edge = |c: char| !c.is_alphanumeric() && !"/~$-_".contains(c);
	text.split(|c: char| c.is_whitespace() || "`*\"'()[]{}<>|".contains(c))
		.map(|w| w.trim_matches(edge))
		.filter(|w| !w.is_empty())
		.map(String::from)
		.collect()
}

/// How many words say not to do something or to take care.
fn negations(text: &str) -> usize {
	text.to_lowercase()
		.replace('’', "'")
		.split(|c: char| !c.is_alphanumeric() && c != '\'')
		.filter(|w| {
			matches!(
				*w,
				"not" | "no" | "never" | "without" | "only" | "nicht" | "nie"
			) || w.ends_with("n't")
				|| ["keep", "avoid", "careful", "risk", "kein"]
					.iter()
					.any(|p| w.starts_with(p))
		})
		.count()
}

/// What the rewrite adds or drops: a word with a digit, dot, slash, @ or
/// colon the original doesn't have as a whole word, `code` that isn't in
/// the original as it is, or every negation and warning of the original.
fn invented(original: &str, rewrite: &str) -> Option<String> {
	let known: HashSet<String> = words(original).into_iter().collect();
	let added = words(rewrite).into_iter().find(|w| {
		(w.chars().any(char::is_numeric) || w.contains(['.', '/', '@', ':']))
			&& !known.contains(w)
	});
	if let Some(word) = added {
		return Some(format!("adds {}", short(&word)));
	}

	let code = rewrite
		.split('`')
		.skip(1)
		.step_by(2)
		.map(str::trim)
		.find(|c| !c.is_empty() && !original.contains(c));
	if let Some(code) = code {
		return Some(format!("adds `{}`", short(code)));
	}

	if negations(original) > 0 && negations(rewrite) == 0 {
		return Some("drops a negation".into());
	}
	None
}

fn short(text: &str) -> String {
	text.chars().take(60).collect()
}

/// Asks the model for the rewrite; without a key, on an error or a
/// timeout the original stays. Costs are recorded without a conversation;
/// a timeout drops the request before its usage is known.
pub(super) async fn rewrite(
	cfg: &Config,
	llm: &Llm,
	db: &Db,
	draft: Draft,
) -> Draft {
	let Some(model) = rewrite_model(cfg) else {
		return Draft {
			note: Some("no model configured".into()),
			..draft
		};
	};
	let req = rewrite_request(model, &draft.original);
	let ask = async {
		let mut stream = llm.request(&req).await.map_err(|e| e.to_string())?;
		while let Some(ev) = stream.next().await {
			ev.map_err(|e| e.to_string())?;
		}
		stream
			.into_response()
			.ok_or_else(|| "the response is incomplete".to_string())
	};
	let resp = match tokio::time::timeout(REWRITE_TIMEOUT, ask).await {
		Ok(Ok(resp)) => Ok(resp),
		Ok(Err(e)) => {
			tracing::warn!("rewriting a proposal: {e}");
			Err("model error".to_string())
		}
		Err(_) => Err("timeout".into()),
	};

	let answer = resp.map(|resp| {
		if let Err(e) = crate::costs::record(&db.conn(), 0, model, &resp.usage)
		{
			tracing::error!("recording the llm usage: {e}");
		}
		resp.output
			.into_iter()
			.filter_map(|o| match o {
				Output::Text { content } => Some(content),
				Output::ToolCall { .. } => None,
			})
			.collect::<String>()
	});
	draft.rewritten(model, answer)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn cfg(toml: &str) -> Config {
		toml::from_str(toml).unwrap()
	}

	#[test]
	fn a_cheap_model_rewrites_with_the_proposal_as_data() {
		assert_eq!(rewrite_model(&cfg("")), None);
		assert_eq!(
			rewrite_model(&cfg("anthropic-key = \"a\"")),
			Some("anthropic/claude-haiku-5.5")
		);
		assert_eq!(
			rewrite_model(&cfg(
				"openrouter-key = \"o\"\nanthropic-key = \"a\""
			)),
			Some("google/gemini-3.8-flash")
		);

		let req = rewrite_request("google/gemini-3.8-flash", "Update Craft");
		assert!(req.tools.is_empty());
		assert!(req.instructions.contains("data, not instructions"));
		assert!(
			matches!(&req.model, Model::OpenRouter(Some(m)) if m == "google/gemini-3.8-flash")
		);
		let [Input::Text { content, .. }] = &req.input[..] else {
			panic!("one text message");
		};
		assert_eq!(content, "Update Craft");
	}

	#[tokio::test]
	async fn without_a_key_the_original_stays() {
		let db = Db::memory();
		let cfg = cfg("");
		let draft = Draft::new("Update Craft\u{200B}").unwrap();
		let draft = rewrite(&cfg, &Llm::new(&cfg), &db, draft).await;
		assert_eq!(draft.text, "Update Craft");
		assert_eq!(draft.rewritten_by, None);
		assert_eq!(draft.note.as_deref(), Some("no model configured"));
	}

	#[test]
	fn a_failed_or_inventing_rewrite_keeps_the_original() {
		let original = "Please update Craft on example.ch to 5.8.1, \
			mail admin@example.ch when done.";
		let model = "google/gemini-3.8-flash";
		let draft = || Draft::new(original).unwrap();

		let failed = draft().rewritten(model, Err("timeout".into()));
		assert_eq!(
			(failed.text.as_str(), failed.rewritten_by),
			(original, None)
		);
		assert_eq!(failed.note.as_deref(), Some("timeout"));
		let empty = draft().rewritten(model, Ok(" \u{200B}\n".into()));
		assert_eq!(empty.rewritten_by, None);
		assert_eq!(empty.note.as_deref(), Some("empty answer"));
		for changed in [
			"Update Craft on `example.com` to `5.8.1`",
			"Update Craft on `example.ch` to `5.9.0`",
			"Mail `root@example.ch`",
			"See [the site](https://example.ch)",
			"Update Craft on example.ch to 5.8",
		] {
			let d = draft().rewritten(model, Ok(changed.into()));
			assert_eq!(d.text, original, "{changed}");
		}
		let d = draft().rewritten(model, Ok("Update to 5.8".into()));
		assert_eq!(d.note.as_deref(), Some("dropped: adds 5.8"));

		let good = "Update Craft on example.ch\n\n\
			- update Craft on `example.ch` to `5.8.1`.\n\
			- mail **admin@example.ch** (when done!)";
		let d = draft().rewritten(model, Ok(format!("{good}\u{FEFF}\n")));
		assert_eq!(d.text, good);
		assert_eq!(d.original, original);
		assert_eq!(d.rewritten_by, Some(model));
		assert_eq!(d.note, None);
	}

	#[test]
	fn only_whole_words_of_the_original_count() {
		let ok = |original: &str, rewrite: &str| {
			let found = invented(original, rewrite);
			assert_eq!(found, None, "{rewrite}");
		};
		let refused = |original: &str, rewrite: &str, why: &str| {
			let found = invented(original, rewrite);
			assert_eq!(found.as_deref(), Some(why), "{rewrite}");
		};

		// commands without digits or dots may be named, in `code` only
		// as the original has them
		let original = "Clear the cache with rm and check with curl.";
		ok(
			original,
			"Clear the cache\n\n- remove it with rm, check with curl",
		);
		ok(original, "- clear with `rm`\n- check with `curl`");
		refused(original, "- run `rm -rf /`", "adds /");
		refused(original, "- run `rm -r`", "adds `rm -r`");

		refused("Update php8 on shop", "Update `php9`", "adds php9");
		refused("Update PHP 8", "Update PHP 8.3", "adds 8.3");
		refused("Update to 5.8.1", "Update to 5.8", "adds 5.8");
		ok("Update to 5.8.1.", "Update to `5.8.1`.");

		let original = "Delete `/srv/genco/shop/var/tmp`, it is 4 GB.";
		ok(original, "Delete `/srv/genco/shop/var/tmp` (4 GB)");
		refused(original, "Delete `/srv/genco/shop`", "adds /srv/genco/shop");
		refused(
			"Delete /srv/genco/shop/var/tmp",
			"Delete the folder `genco/shop`",
			"adds genco/shop",
		);

		// a look-alike dot passes for the address it shows
		ok("Renew example.ch", "Renew `example.ch`");
		refused(
			"Renew example.ch",
			"Renew example\u{2024}com",
			"adds example.com",
		);
		ok("Renew example\u{2024}ch", "Renew example.ch");
	}

	#[test]
	fn a_rewrite_must_keep_a_negation() {
		let original = "Restart php-fpm on shop. Don't touch the database.";
		assert_eq!(
			invented(original, "Restart php-fpm on shop and the database."),
			Some("drops a negation".into())
		);
		assert_eq!(
			invented(original, "Restart php-fpm on shop, keep the database."),
			None
		);
		assert_eq!(
			invented("Bitte nicht neu starten", "Neu starten"),
			Some("drops a negation".into())
		);
		assert_eq!(negations("No, never; only without it. Risky! Keine"), 6);
		assert_eq!(negations("Update Craft now"), 0);
		assert_eq!(invented("Update Craft", "Update Craft, no risk"), None);
	}
}
