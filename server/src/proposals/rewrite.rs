//! A small model rewrites a proposal into a short formatted version. A
//! rewrite that adds a name, address, number or command, or drops every
//! negation of the original, is refused: the admin would approve
//! something the agent didn't ask for. Only then the model is told what
//! was refused and asked once more; when that is refused too, or the
//! first answer is empty or too long (or the model fails), the original
//! stays and the panel shows why.

use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use soe_llms::{Input, Model, Output, Request, Role};
use tokio::time::Instant;

use super::{MAX_TEXT, clean_text};
use crate::config::Config;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::llm::Llm;

/// How long the agent's request waits for one answer of the model, and
/// for both together; then the original is used.
const REWRITE_TIMEOUT: Duration = Duration::from_secs(30);
const REWRITE_BUDGET: Duration = Duration::from_secs(45);
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

	/// Asks for the rewrite and, when the check refuses it, once more
	/// with what was refused. `ask` fails with the note. Else the
	/// original stays with the reason as the note.
	async fn rewrite_with<F, Fut>(self, model: &'static str, mut ask: F) -> Self
	where
		F: FnMut(Request) -> Fut,
		Fut: Future<Output = std::result::Result<String, String>>,
	{
		let req = rewrite_request(model, &self.original);
		let answer = match ask(req.clone()).await {
			Ok(answer) => answer,
			Err(note) => return self.kept(note),
		};
		let why = match checked_rewrite(&self.original, &answer) {
			Ok(text) => return self.taken(model, text, None),
			Err(Refused::Invents(why)) => why,
			Err(refused) => return self.kept(refused.to_string()),
		};

		tracing::info!("proposal rewrite refused ({why}), asking again");
		let correction = correction(&self.original, &why);
		let answer = match ask(retry_request(req, answer, correction)).await {
			Ok(answer) => answer,
			Err(note) => {
				return self
					.kept(format!("dropped: {why}; second try: {note}"));
			}
		};
		match checked_rewrite(&self.original, &answer) {
			Ok(text) => self.taken(model, text, Some("second try")),
			Err(Refused::Invents(why)) => {
				self.kept(format!("dropped twice: {why}"))
			}
			Err(refused) => {
				self.kept(format!("dropped: {why}; second try: {refused}"))
			}
		}
	}

	fn taken(
		mut self,
		model: &'static str,
		text: String,
		note: Option<&str>,
	) -> Self {
		self.text = text;
		self.rewritten_by = Some(model);
		self.note = note.map(String::from);
		self
	}

	fn kept(mut self, note: String) -> Self {
		tracing::warn!("proposal not rewritten: {note}");
		self.note = Some(note);
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

/// The first request with the refused answer and the correction after it.
fn retry_request(
	mut req: Request,
	answer: String,
	correction: String,
) -> Request {
	req.input.extend([
		Input::Text {
			role: Role::Assistant,
			content: answer,
		},
		Input::Text {
			role: Role::User,
			content: correction,
		},
	]);
	req
}

/// Tells the model what was refused and how to do it right.
fn correction(original: &str, why: &Invented) -> String {
	let what = match why {
		Invented::Word(word) => {
			let source = similar(original, word)
				.map(|s| format!(" (it writes `{s}`)"))
				.unwrap_or_default();
			format!(
				"it contains `{word}`, which is not written like that in \
				the proposal{source}"
			)
		}
		Invented::Code(code) => format!(
			"it has `{code}` in code, which is not in the proposal \
			character for character"
		),
		Invented::Negations(words) => {
			let words: Vec<_> =
				words.iter().map(|w| format!("`{w}`")).collect();
			format!(
				"it drops every negation and warning of the proposal ({})",
				words.join(", ")
			)
		}
	};
	format!(
		"Your rewrite was refused: {what}. Write every name, address, path, \
		version and number exactly as the proposal writes it, do not \
		expand, split, complete or shorten them. Keep all negations and \
		warnings. Answer with the corrected rewrite only."
	)
}

/// The word of the original that `word` most likely comes from: the one
/// with the longest common start or end, at least 3 characters.
fn similar(original: &str, word: &str) -> Option<String> {
	let common = |a: &str, b: &str| {
		let start = a.chars().zip(b.chars()).take_while(|(a, b)| a == b);
		let end = a.chars().rev().zip(b.chars().rev());
		start.count().max(end.take_while(|(a, b)| a == b).count())
	};
	words(original)
		.into_iter()
		.filter(|w| special(w))
		.map(|w| (common(&w, word), w))
		.filter(|(n, _)| *n >= 3)
		.max_by_key(|(n, _)| *n)
		.map(|(_, w)| short(&w))
}

/// Why a rewrite is refused.
#[derive(Debug, PartialEq)]
enum Refused {
	Empty,
	TooLong,
	Invents(Invented),
}

impl fmt::Display for Refused {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self {
			Self::Empty => f.write_str("empty answer"),
			Self::TooLong => f.write_str("dropped: too long"),
			Self::Invents(why) => write!(f, "dropped: {why}"),
		}
	}
}

/// The rewrite, cleaned like the original. Refused when it is empty, too
/// long or invents something.
fn checked_rewrite(
	original: &str,
	answer: &str,
) -> std::result::Result<String, Refused> {
	let (text, _) = clean_text(answer);
	let text = text.trim();
	if text.is_empty() {
		return Err(Refused::Empty);
	}
	if text.len() > MAX_TEXT {
		return Err(Refused::TooLong);
	}
	if let Some(why) = invented(original, text) {
		return Err(Refused::Invents(why));
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

/// The words that say not to do something or to take care.
fn negations(text: &str) -> Vec<String> {
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
		.map(String::from)
		.collect()
}

/// A word with a digit, dot, slash, @ or colon: a name, address, path,
/// version or number.
fn special(word: &str) -> bool {
	word.chars().any(char::is_numeric) || word.contains(['.', '/', '@', ':'])
}

/// What a rewrite adds or drops.
#[derive(Debug, PartialEq)]
enum Invented {
	Word(String),
	Code(String),
	/// the negations of the original, each once
	Negations(Vec<String>),
}

impl fmt::Display for Invented {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self {
			Self::Word(word) => write!(f, "adds {word}"),
			Self::Code(code) => write!(f, "adds `{code}`"),
			Self::Negations(_) => f.write_str("drops a negation"),
		}
	}
}

/// What the rewrite adds or drops: a special word the original doesn't
/// have as a whole word, `code` that isn't in the original as it is, or
/// every negation and warning of the original.
fn invented(original: &str, rewrite: &str) -> Option<Invented> {
	let known: HashSet<String> = words(original).into_iter().collect();
	let added = words(rewrite)
		.into_iter()
		.find(|w| special(w) && !known.contains(w));
	if let Some(word) = added {
		return Some(Invented::Word(short(&word)));
	}

	let code = rewrite
		.split('`')
		.skip(1)
		.step_by(2)
		.map(str::trim)
		.find(|c| !c.is_empty() && !original.contains(c));
	if let Some(code) = code {
		return Some(Invented::Code(short(code)));
	}

	let mut kept = negations(original);
	if !kept.is_empty() && negations(rewrite).is_empty() {
		let mut seen = HashSet::new();
		kept.retain(|w| seen.insert(w.clone()));
		return Some(Invented::Negations(kept));
	}
	None
}

fn short(text: &str) -> String {
	text.chars().take(60).collect()
}

/// Asks the model for the rewrite, twice when the first is refused;
/// without a key, on an error or a timeout the original stays.
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
	let deadline = Instant::now() + REWRITE_BUDGET;
	draft
		.rewrite_with(model, |req| ask(llm, db, model, req, deadline))
		.await
}

/// One answer of the model, within `REWRITE_TIMEOUT` and the deadline.
/// Costs are recorded without a conversation; a timeout drops the
/// request before its usage is known.
async fn ask(
	llm: &Llm,
	db: &Db,
	model: &str,
	req: Request,
	deadline: Instant,
) -> std::result::Result<String, String> {
	let ask = async {
		let mut stream = llm.request(&req).await.map_err(|e| e.to_string())?;
		while let Some(ev) = stream.next().await {
			ev.map_err(|e| e.to_string())?;
		}
		stream
			.into_response()
			.ok_or_else(|| "the response is incomplete".to_string())
	};
	let until = deadline.min(Instant::now() + REWRITE_TIMEOUT);
	let resp = match tokio::time::timeout_at(until, ask).await {
		Ok(Ok(resp)) => resp,
		Ok(Err(e)) => {
			tracing::warn!("rewriting a proposal: {e}");
			return Err("model error".into());
		}
		Err(_) => return Err("timeout".into()),
	};

	if let Err(e) = crate::costs::record(&db.conn(), 0, model, &resp.usage) {
		tracing::error!("recording the llm usage: {e}");
	}
	Ok(resp
		.output
		.into_iter()
		.filter_map(|o| match o {
			Output::Text { content } => Some(content),
			Output::ToolCall { .. } => None,
		})
		.collect())
}

/// Rewrites with a model that gives `answers` in turn; the draft and the
/// requests the model got.
#[cfg(test)]
pub(super) async fn mocked(
	draft: Draft,
	answers: &[std::result::Result<&str, &str>],
) -> (Draft, Vec<Request>) {
	let reqs = std::cell::RefCell::new(vec![]);
	let draft = draft
		.rewrite_with("m", |req| {
			let mut reqs = reqs.borrow_mut();
			let answer =
				answers[reqs.len()].map(String::from).map_err(String::from);
			reqs.push(req);
			std::future::ready(answer)
		})
		.await;
	(draft, reqs.into_inner())
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

	#[tokio::test]
	async fn a_failed_or_inventing_rewrite_keeps_the_original() {
		let original = "Please update Craft on example.ch to 5.8.1, \
			mail admin@example.ch when done.";
		let draft = || Draft::new(original).unwrap();

		let (failed, _) = mocked(draft(), &[Err("timeout")]).await;
		assert_eq!(
			(failed.text.as_str(), failed.rewritten_by),
			(original, None)
		);
		assert_eq!(failed.note.as_deref(), Some("timeout"));
		let (empty, _) = mocked(draft(), &[Ok(" \u{200B}\n")]).await;
		assert_eq!(empty.rewritten_by, None);
		assert_eq!(empty.note.as_deref(), Some("empty answer"));
		for changed in [
			"Update Craft on `example.com` to `5.8.1`",
			"Update Craft on `example.ch` to `5.9.0`",
			"Mail `root@example.ch`",
			"See [the site](https://example.ch)",
			"Update Craft on example.ch to 5.8",
		] {
			let (d, _) = mocked(draft(), &[Ok(changed), Ok(changed)]).await;
			assert_eq!(d.text, original, "{changed}");
		}

		let good = "Update Craft on example.ch\n\n\
			- update Craft on `example.ch` to `5.8.1`.\n\
			- mail **admin@example.ch** (when done!)";
		let answer = format!("{good}\u{FEFF}\n");
		let (d, reqs) = mocked(draft(), &[Ok(&answer)]).await;
		assert_eq!(d.text, good);
		assert_eq!(d.original, original);
		assert_eq!(d.rewritten_by, Some("m"));
		assert_eq!(d.note, None);
		assert_eq!(reqs.len(), 1);
	}

	#[tokio::test]
	async fn a_refused_rewrite_is_asked_for_once_more() {
		let original = "Point the domain to dns1/dns2.registrar-servers.com.";
		let draft = || Draft::new(original).unwrap();
		let split = "Point the domain to `dns1.registrar-servers.com` and \
			`dns2.registrar-servers.com`";
		let good = "Point the domain to `dns1/dns2.registrar-servers.com`";

		let (d, reqs) = mocked(draft(), &[Ok(split), Ok(good)]).await;
		assert_eq!(d.text, good);
		assert_eq!(d.rewritten_by, Some("m"));
		assert_eq!(d.note.as_deref(), Some("second try"));
		assert_eq!(reqs[0].instructions, reqs[1].instructions);
		let [
			Input::Text {
				role: Role::User,
				content: proposal,
			},
			Input::Text {
				role: Role::Assistant,
				content: refused,
			},
			Input::Text {
				role: Role::User,
				content: correction,
			},
		] = &reqs[1].input[..]
		else {
			panic!("the proposal, the refused answer and the correction");
		};
		assert_eq!((proposal.as_str(), refused.as_str()), (original, split));
		assert!(correction.starts_with(
			"Your rewrite was refused: it contains \
			`dns1.registrar-servers.com`, which is not written like that in \
			the proposal (it writes `dns1/dns2.registrar-servers.com`)."
		));
		assert!(correction.contains("Keep all negations"));

		let (d, reqs) = mocked(draft(), &[Ok(split), Ok(split)]).await;
		assert_eq!((d.text.as_str(), d.rewritten_by), (original, None));
		assert_eq!(
			d.note.as_deref(),
			Some("dropped twice: adds dns1.registrar-servers.com")
		);
		assert_eq!(reqs.len(), 2);

		let (d, _) = mocked(draft(), &[Ok(split), Err("timeout")]).await;
		assert_eq!(d.text, original);
		assert_eq!(
			d.note.as_deref(),
			Some(
				"dropped: adds dns1.registrar-servers.com; second try: timeout"
			)
		);
	}

	#[test]
	fn the_correction_names_what_was_refused() {
		let original = "Restart php-fpm. Don't touch the database, never \
			restart nginx. Do not reload.";
		let why = invented(original, "Restart php-fpm and the database.");
		assert_eq!(
			why,
			Some(Invented::Negations(vec![
				"don't".into(),
				"never".into(),
				"not".into()
			]))
		);
		assert!(correction(original, &why.unwrap()).starts_with(
			"Your rewrite was refused: it drops every negation and warning \
			of the proposal (`don't`, `never`, `not`)."
		));

		let why = invented("Clear it with rm", "Run `rm -r`").unwrap();
		assert!(
			correction("Clear it with rm", &why)
				.contains("it has `rm -r` in code, which is not")
		);
		let why = invented("Update Craft", "Update Craft to 5.9").unwrap();
		assert!(correction("Update Craft", &why).starts_with(
			"Your rewrite was refused: it contains `5.9`, which is not \
			written like that in the proposal. Write"
		));
	}

	#[test]
	fn only_whole_words_of_the_original_count() {
		let ok = |original: &str, rewrite: &str| {
			let found = invented(original, rewrite);
			assert_eq!(found, None, "{rewrite}");
		};
		let refused = |original: &str, rewrite: &str, why: &str| {
			let found = invented(original, rewrite).map(|i| i.to_string());
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
			Some(Invented::Negations(vec!["don't".into()]))
		);
		assert_eq!(
			invented(original, "Restart php-fpm on shop, keep the database."),
			None
		);
		assert_eq!(
			invented("Bitte nicht neu starten", "Neu starten")
				.map(|i| i.to_string()),
			Some("drops a negation".into())
		);
		assert_eq!(
			negations("No, never; only without it. Risky! Keine").len(),
			6
		);
		assert!(negations("Update Craft now").is_empty());
		assert_eq!(invented("Update Craft", "Update Craft, no risk"), None);
	}
}
