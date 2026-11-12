mod tools;

pub(crate) use tools::{output_text, truncate};

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use soe_llms::{
	ContentPart, Input, Model, Output, Request, ResponseEvent, Role,
};
use tokio::task::{AbortHandle, JoinHandle};

use crate::config::Config;
use crate::conversations::{
	self as convs, Kind, Message, MessageData, Report, Status,
};
use crate::db::Db;
use crate::events::{Event, Events};
use crate::llm::Llm;

/// Max llm requests per run, protects against endless loops.
const MAX_STEPS: usize = 200;

const INSTRUCTIONS: &str = include_str!("instructions.md");
const ONBOARDING: &str = include_str!("onboarding.md");

/// The agent's shell starts here.
const WORKSPACE: &str = "/root";

/// Runs the agent loop of conversations in the background.
#[derive(Clone)]
pub struct Runner(Arc<Inner>);

struct Inner {
	cfg: Arc<Config>,
	db: Db,
	events: Events,
	llm: Llm,
	/// conversations which currently have a running loop
	running: Mutex<HashMap<i64, AbortHandle>>,
	/// set when genco is about to exit
	shutting_down: AtomicBool,
	/// reports and answers on their way to the mail and the webhooks,
	/// `genco chat` waits for them before it exits
	sending: Mutex<Vec<JoinHandle<()>>>,
}

/// Held by the process that runs a conversation's loop.
const LOCKS: &str = "/var/lib/genco/conversations";

fn lock_path(id: i64) -> String {
	format!("{LOCKS}/{id}.lock")
}

fn conversation_lock(id: i64) -> Option<std::fs::File> {
	crate::job::try_lock(&lock_path(id)).ok().flatten()
}

fn lock_free(id: i64) -> bool {
	crate::job::is_free(&lock_path(id))
}

impl Runner {
	pub fn new(cfg: Arc<Config>, db: Db, events: Events) -> Self {
		let llm = Llm::new(&cfg);

		Self(Arc::new(Inner {
			cfg,
			db,
			events,
			llm,
			running: Mutex::new(HashMap::new()),
			shutting_down: AtomicBool::new(false),
			sending: Mutex::new(vec![]),
		}))
	}

	/// Sends in the background, `sent` waits for it.
	fn send_later(&self, f: impl Future<Output = ()> + Send + 'static) {
		let mut sending = self.0.sending.lock().unwrap();
		sending.retain(|h| !h.is_finished());
		sending.push(tokio::spawn(f));
	}

	/// Waits for the reports and answers on their way, at most `limit`:
	/// a process that exits would cut them off.
	pub async fn sent(&self, limit: std::time::Duration) {
		let handles: Vec<_> =
			self.0.sending.lock().unwrap().drain(..).collect();
		let all = futures::future::join_all(handles);
		if tokio::time::timeout(limit, all).await.is_err() {
			tracing::warn!("a report is still on its way, not waiting longer");
		}
	}

	/// The llm client, shared with the proposals' planner.
	pub fn llm(&self) -> &Llm {
		&self.0.llm
	}

	/// Starts the loop for a conversation if it is not already running. A
	/// running loop picks up new messages by itself.
	pub fn start(&self, id: i64) {
		let mut running = self.0.running.lock().unwrap();
		if running.contains_key(&id) {
			return;
		}

		let me = self.clone();
		let handle = tokio::spawn(async move { me.run(id).await });
		running.insert(id, handle.abort_handle());

		self.set_status(id, Status::Running);
	}

	/// Runs the loop until the agent is done, for `genco chat`. False if
	/// another process (the panel) runs the conversation right now.
	pub async fn run_now(&self, id: i64) -> bool {
		// taken here and held by the loop: between a check and the loop
		// the panel could take it
		let Some(lock) = conversation_lock(id) else {
			return false;
		};
		self.set_status(id, Status::Running);
		self.clone().run_locked(id, lock).await;
		true
	}

	pub fn stop(&self, id: i64) {
		let Some(handle) = self.0.running.lock().unwrap().remove(&id) else {
			return;
		};
		handle.abort();

		self.close_open_tool_calls(id, "Stopped by the admin.");
		self.push(
			id,
			MessageData::Error {
				text: "Stopped by the admin.".into(),
			},
		);
		self.set_status(id, Status::Idle);
	}

	/// Called when genco is about to exit. Runs failing from now on are
	/// most likely killed by the shutdown, they stay `running` and get
	/// resumed after the restart.
	pub fn shutdown(&self) {
		self.0.shutting_down.store(true, Ordering::SeqCst);
	}

	/// The first start of genco: the agent welcomes the admins with what
	/// it finds and a checklist.
	pub fn onboard(&self) {
		let res = (|| {
			let conn = self.0.db.conn();
			if !convs::list(&conn)?.is_empty() {
				return Ok(None);
			}
			let id = convs::create(&conn, "Welcome to genco")?;
			let text =
				ONBOARDING.replace("{{inventory}}", crate::setup::INVENTORY);
			convs::insert_message(&conn, id, MessageData::System { text })?;
			Ok::<_, rusqlite::Error>(Some(id))
		})();
		match res {
			Ok(Some(id)) => self.start(id),
			Ok(None) => {}
			Err(e) => tracing::error!("onboarding: {e}"),
		}
	}

	/// Continues conversations which were running when genco stopped.
	pub fn resume_interrupted(&self) {
		let running = convs::with_status(&self.0.db.conn(), Status::Running)
			.unwrap_or_default();

		for id in running {
			// `genco chat` in a terminal runs it, it goes on there
			if !lock_free(id) {
				continue;
			}
			tracing::info!("resuming conversation {id}");

			self.close_open_tool_calls(
				id,
				"Interrupted because genco restarted.",
			);
			self.push(
				id,
				MessageData::System {
					text: "genco was restarted. Continue where you left off \
						and verify your changes if needed."
						.into(),
				},
			);
			self.start(id);
		}
	}

	async fn run(self, id: i64) {
		// one loop per conversation over all processes: the panel and a
		// `genco chat` would both answer
		let Some(lock) = conversation_lock(id) else {
			tracing::info!("conversation {id} runs in another process");
			self.0.running.lock().unwrap().remove(&id);
			return;
		};
		self.run_locked(id, lock).await;
	}

	/// The loop, `_lock` is the conversation's lock file.
	async fn run_locked(self, id: i64, _lock: std::fs::File) {
		let mut steps = 0;
		let mut last_seen = 0;

		loop {
			// stays running, gets resumed after the restart. Without this
			// the llm would react to tool commands killed by the shutdown.
			if self.0.shutting_down.load(Ordering::SeqCst) {
				tracing::info!("conversation {id} interrupted by shutdown");
				return;
			}
			steps += 1;

			let status = match self.step(id, &mut last_seen).await {
				Ok(true) if steps < MAX_STEPS => continue,
				Ok(true) => {
					self.push(
						id,
						MessageData::Error {
							text: format!(
								"Stopped after {MAX_STEPS} steps. Send a \
								message to continue."
							),
						},
					);
					Status::Idle
				}
				Ok(false) => Status::Idle,
				Err(e) if self.0.shutting_down.load(Ordering::SeqCst) => {
					tracing::info!("conversation {id} interrupted: {e}");
					return;
				}
				Err(e) => {
					tracing::error!("conversation {id} failed: {e}");
					self.close_open_tool_calls(id, "Aborted.");
					self.push(id, MessageData::Error { text: e });
					Status::Failed
				}
			};

			let failed = status == Status::Failed;
			if self.finish(id, status, last_seen, failed && steps >= MAX_STEPS)
			{
				self.report(id, failed);
				return;
			}

			// a new message arrived, like a fresh start
			if !failed {
				steps = 0;
			}
		}
	}

	/// One llm request plus the execution of its tool calls.
	/// Returns true if tool calls were made and the loop should continue.
	async fn step(&self, id: i64, last_seen: &mut i64) -> Result<bool, String> {
		let messages = convs::messages(&self.0.db.conn(), id)
			.map_err(|e| e.to_string())?;
		*last_seen = messages.last().map(|m| m.id).unwrap_or(0);
		convs::set_seen(&self.0.db.conn(), id, *last_seen)
			.map_err(|e| e.to_string())?;

		let output = self.ask_llm(id, &messages).await?;
		let calls = self.store_output(id, output);

		let has_calls = !calls.is_empty();
		for (call_id, name, input) in calls {
			self.run_tool(id, call_id, &name, input).await;
		}
		// the agent may have changed groups or domains with the CLI, which
		// writes the database in its own process
		if has_calls {
			self.0.events.send(Event::Tree);
		}

		Ok(has_calls)
	}

	/// Sends the conversation to the llm, streams its text to the browser
	/// and records the cost.
	async fn ask_llm(
		&self,
		id: i64,
		messages: &[Message],
	) -> Result<Vec<Output>, String> {
		let model = crate::llm::model(&self.0.cfg, &self.0.db);
		let req = Request {
			input: to_input(messages),
			instructions: self.instructions(),
			model: Model::OpenRouter(Some(model.clone())),
			user_id: "genco".into(),
			tools: tools::definitions(),
			reasoning_effort: None,
		};

		let mut stream = self
			.0
			.llm
			.request(&req)
			.await
			.map_err(|e| format!("llm request failed: {e}"))?;

		while let Some(ev) = stream.next().await {
			let ResponseEvent::TextDelta { content } =
				ev.map_err(|e| format!("llm response failed: {e}"))?;
			self.0.events.send(Event::Delta {
				conversation_id: id,
				text: content,
			});
		}
		let resp = stream.into_response().ok_or("llm response incomplete")?;
		let used =
			crate::costs::record(&self.0.db.conn(), id, &model, &resp.usage);
		if let Err(e) = used {
			tracing::error!("recording the llm usage: {e}");
		}

		Ok(resp.output)
	}

	/// Stores the llm's text and tool calls, returns the calls to run as
	/// (call id, name, input).
	fn store_output(
		&self,
		id: i64,
		output: Vec<Output>,
	) -> Vec<(String, String, serde_json::Value)> {
		let mut calls = vec![];
		for output in output {
			match output {
				Output::Text { content } => {
					if !content.trim().is_empty() {
						self.push(id, MessageData::Assistant { text: content });
					}
				}
				Output::ToolCall {
					id: call_id,
					name,
					input,
					context,
				} => {
					self.push(
						id,
						MessageData::ToolCall {
							call_id: call_id.clone(),
							name: name.clone(),
							input: input.clone(),
							context,
						},
					);
					calls.push((call_id, name, input));
				}
			}
		}
		calls
	}

	/// Runs one tool call and stores its output.
	async fn run_tool(
		&self,
		id: i64,
		call_id: String,
		name: &str,
		input: serde_json::Value,
	) {
		let what: String = match &input {
			serde_json::Value::Object(m) => m
				.values()
				.filter_map(|v| v.as_str())
				.collect::<Vec<_>>()
				.join(" "),
			_ => String::new(),
		};
		crate::activity::log(
			&self.0.db.conn(),
			"agent",
			&format!("Tool {name}"),
			&format!("conversation {id}"),
			&what.chars().take(500).collect::<String>(),
			None,
		);

		// the only tool that acts in the browser, not on the machine
		let res = if name == "show_page" {
			tools::page_path(input).map(|path| {
				self.0.events.send(Event::ShowPage {
					conversation_id: id,
					path,
				});
				"shown".to_string()
			})
		} else {
			tools::call(name, input, Path::new(WORKSPACE), id).await
		};
		let is_error = res.is_err();
		let output = res.unwrap_or_else(|e| e);

		self.push(
			id,
			MessageData::ToolOutput {
				call_id,
				output,
				is_error,
			},
		);
	}

	/// Marks the run as finished unless a message arrived the llm hasn't
	/// seen yet. Returns false if the loop should continue.
	fn finish(
		&self,
		id: i64,
		status: Status,
		last_seen: i64,
		force: bool,
	) -> bool {
		// holding the lock makes sure `start` either sees us running or
		// the message gets picked up here
		let mut running = self.0.running.lock().unwrap();

		let unseen = convs::has_input_after(&self.0.db.conn(), id, last_seen)
			.unwrap_or(false);
		if !force && unseen {
			return false;
		}

		running.remove(&id);
		self.set_status(id, status);
		true
	}

	/// Whether a key for the llm is configured.
	pub fn has_llm(&self) -> bool {
		self.0.cfg.anthropic_key.is_some()
			|| self.0.cfg.openrouter_key.is_some()
	}

	/// Starts a routine or alert. Without a key for the llm it ends right
	/// away and the admins get the mail.
	pub fn start_report(&self, id: i64) {
		if self.has_llm() {
			self.start(id);
			return;
		}
		self.push(
			id,
			MessageData::Error {
				text: "The agent has no API key, see /etc/genco/config.toml."
					.into(),
			},
		);
		self.report(id, true);
	}

	/// A routine or alert ended: its report is the last status line of the
	/// agent, a failed run needs attention. The admins get a notification
	/// when it turns to attention, see `notify`.
	pub fn report(&self, id: i64, failed: bool) {
		let conn = self.0.db.conn();
		let (Ok(before), Ok(messages)) =
			(convs::get(&conn, id), convs::messages(&conn, id))
		else {
			return;
		};
		if before.kind == Kind::Chat {
			// a conversation from an AI agent's proposal answers it
			let cfg = self.0.cfg.clone();
			let db = self.0.db.clone();
			self.send_later(async move {
				crate::proposals::answer(&cfg, &db, id, failed).await
			});
			return;
		}

		let said = messages.iter().rev().find_map(|m| match &m.data {
			MessageData::Assistant { text } => Report::find(text),
			_ => None,
		});
		let report = match said {
			Some(r) if !failed => r,
			_ => Report::Attention,
		};
		match convs::set_report(&conn, id, report) {
			Ok(conversation) => {
				self.0.events.send(Event::Conversation { conversation })
			}
			Err(e) => tracing::error!("failed to set the report: {e}"),
		}
		let older = if before.kind == Kind::Routine {
			older_attention(&conn, id, &before.title).unwrap_or_default()
		} else {
			vec![]
		};
		drop(conn);

		// what this run reports is the routine's state now
		for old in older {
			self.resolve(
				old,
				&format!(
					"a newer run of this routine (conversation {id}) reports {}.",
					if report == Report::Attention {
						"attention"
					} else {
						"OK"
					}
				),
			);
		}

		// a mail once it turns to attention; a webhook may want the OK
		// reports too
		let attention = report == Report::Attention;
		if !attention || before.report != Some(report) {
			let me = self.clone();
			self.send_later(
				async move { me.notify(id, attention, messages).await },
			);
		}
	}

	async fn notify(&self, id: i64, attention: bool, messages: Vec<Message>) {
		let Ok(c) = convs::get(&self.0.db.conn(), id) else {
			return;
		};
		// the agent's last words, or the problem and why the agent failed
		let body = messages
			.iter()
			.rev()
			.find_map(|m| match &m.data {
				MessageData::Assistant { text } => Some(text.clone()),
				_ => None,
			})
			.unwrap_or_else(|| {
				let first = messages.iter().find_map(|m| match &m.data {
					MessageData::System { text } => Some(text.as_str()),
					_ => None,
				});
				let error = messages.iter().rev().find_map(|m| match &m.data {
					MessageData::Error { text } => Some(text.as_str()),
					_ => None,
				});
				format!(
					"The agent couldn't finish: {}\n\n{}",
					error.unwrap_or("no answer"),
					first.unwrap_or("")
				)
			});
		let url = self.0.cfg.panel_url(&format!("/?chat={id}"));
		let notice = crate::notify::Notice {
			kind: "report",
			attention,
			title: c.title,
			text: body,
			url,
			proposal: None,
		};
		crate::notify::send(&self.0.cfg, &self.0.db, notice).await;
	}

	fn close_open_tool_calls(&self, id: i64, output: &str) {
		let messages =
			convs::messages(&self.0.db.conn(), id).unwrap_or_default();

		for call_id in open_tool_calls(&messages) {
			self.push(
				id,
				MessageData::ToolOutput {
					call_id,
					output: output.into(),
					is_error: true,
				},
			);
		}
	}

	/// The instructions followed by the agent's notes about this server.
	fn instructions(&self) -> String {
		let notes = std::fs::read_to_string(self.0.cfg.notes_path())
			.unwrap_or_default();
		let notes = match notes.trim() {
			"" => "(empty)",
			n => n,
		};
		let ctn = INSTRUCTIONS
			.replace(
				"{{notes_path}}",
				&self.0.cfg.notes_path().display().to_string(),
			)
			.replace("{{inventory}}", crate::setup::INVENTORY)
			.replace("{{mail_mode}}", &mail_mode(&self.0.db))
			.replace("{{mail_hosting}}", mail_hosting(&self.0.db));
		format!("{ctn}\n\n# Your notes\n\n{notes}")
	}

	/// Clears the attention of a routine or alert that is taken care of,
	/// see `convs::resolve_by_genco`.
	pub fn resolve(&self, id: i64, why: &str) {
		match convs::resolve_by_genco(&self.0.db.conn(), id, why) {
			Ok((conversation, message)) => {
				self.0.events.send(Event::Message { message });
				self.0.events.send(Event::Conversation { conversation });
			}
			Err(e) => tracing::error!("resolving conversation {id}: {e}"),
		}
	}

	fn push(&self, id: i64, data: MessageData) {
		match convs::insert_message(&self.0.db.conn(), id, data) {
			Ok(message) => self.0.events.send(Event::Message { message }),
			Err(e) => tracing::error!("failed to store message: {e}"),
		}
	}

	fn set_status(&self, id: i64, status: Status) {
		match convs::set_status(&self.0.db.conn(), id, status) {
			Ok(conversation) => {
				self.0.events.send(Event::Conversation { conversation })
			}
			Err(e) => tracing::error!("failed to set status: {e}"),
		}
	}
}

/// The earlier runs of the routine `title` that still need attention.
fn older_attention(
	conn: &rusqlite::Connection,
	id: i64,
	title: &str,
) -> rusqlite::Result<Vec<i64>> {
	conn.prepare(
		"SELECT id FROM conversations WHERE kind = 'routine'
		AND report = 'attention' AND title = ?1 AND id < ?2",
	)?
	.query_map(rusqlite::params![title, id], |r| r.get(0))?
	.collect()
}

/// Settings → Mail in a few words, off changes what the agent can do.
fn mail_mode(db: &Db) -> String {
	match crate::mail::settings(db).map(|s| s.mode).as_deref() {
		Ok("off") => "off, genco sends no mail: no invites, no new mail \
			domains, reports only reach the panel and webhooks"
			.into(),
		Ok("relay") => "through an SMTP relay".into(),
		_ => "direct delivery".into(),
	}
}

/// Settings → Mail → Mail hosting, separate from how mail is sent.
fn mail_hosting(db: &Db) -> &'static str {
	if crate::mail::hosting_off(&db.conn()) {
		"off in Start → Settings → Mail on this server: no new mail \
		domains, mailboxes, forwards or send-only addresses, existing ones \
		stay; genco's own mails still go out unless mail is off"
	} else {
		"on"
	}
}

fn open_tool_calls(messages: &[Message]) -> Vec<String> {
	let done: HashSet<&str> = messages
		.iter()
		.filter_map(|m| match &m.data {
			MessageData::ToolOutput { call_id, .. } => Some(call_id.as_str()),
			_ => None,
		})
		.collect();

	messages
		.iter()
		.filter_map(|m| match &m.data {
			MessageData::ToolCall { call_id, .. }
				if !done.contains(call_id.as_str()) =>
			{
				Some(call_id.clone())
			}
			_ => None,
		})
		.collect()
}

/// Converts stored messages to llm input.
///
/// Providers require tool outputs to directly follow their tool calls, but
/// admins can send messages while tools run. Those messages are held back
/// until all outputs of the pending calls are in. Calls without any output
/// are dropped.
/// An admin's message; attached files are listed with their paths, the
/// model sees images too.
fn user_input(
	text: &str,
	user_name: Option<&str>,
	files: &[crate::attachments::Attachment],
) -> Input {
	let mut content = format!("[{}] {text}", user_name.unwrap_or("terminal"));
	if files.is_empty() {
		return Input::Text {
			role: Role::User,
			content,
		};
	}
	content.push_str(
		"\n\nAttached files (on this server, read them with your tools):",
	);
	for f in files {
		content.push_str(&format!("\n- {} ({} bytes)", f.path, f.size));
	}
	let mut parts = vec![ContentPart::Text { text: content }];
	for f in files {
		if let Some((media_type, data)) = crate::attachments::image_bytes(f) {
			parts.push(ContentPart::Image { media_type, data });
		}
	}
	Input::Message {
		role: Role::User,
		parts,
	}
}

fn to_input(messages: &[Message]) -> Vec<Input> {
	let open: HashSet<String> = open_tool_calls(messages).into_iter().collect();

	let mut input = vec![];
	let mut pending: HashSet<&str> = HashSet::new();
	let mut held_back = vec![];

	for message in messages {
		let item = match &message.data {
			MessageData::User {
				text,
				user_name,
				files,
			} => Some(user_input(text, user_name.as_deref(), files)),
			MessageData::System { text } => Some(Input::Text {
				role: Role::User,
				content: format!("[system] {text}"),
			}),
			MessageData::Assistant { text } => Some(Input::Text {
				role: Role::Assistant,
				content: text.clone(),
			}),
			MessageData::ToolCall {
				call_id,
				name,
				input: call_input,
				context,
			} => {
				if open.contains(call_id) {
					continue;
				}

				pending.insert(call_id);
				input.push(Input::ToolCall {
					id: call_id.clone(),
					name: name.clone(),
					input: call_input.clone(),
					context: context.clone(),
				});
				continue;
			}
			MessageData::ToolOutput {
				call_id, output, ..
			} => {
				if open.contains(call_id) {
					continue;
				}

				pending.remove(call_id.as_str());
				input.push(Input::ToolCallOutput {
					id: call_id.clone(),
					output: output.clone(),
				});

				if pending.is_empty() {
					input.append(&mut held_back);
				}
				continue;
			}
			MessageData::Error { .. } => None,
		};

		if let Some(item) = item {
			if pending.is_empty() {
				input.push(item);
			} else {
				held_back.push(item);
			}
		}
	}

	input.append(&mut held_back);
	input
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn a_newer_run_supersedes_the_routine_s_old_attention() {
		let db = Db::memory();
		let conn = db.conn();
		let run = |title: &str, report| {
			let id = convs::create_kind(&conn, title, Kind::Routine).unwrap();
			convs::set_report(&conn, id, report).unwrap();
			id
		};
		let old = run("Daily health check", Report::Attention);
		run("Daily health check", Report::Ok);
		run("Weekly update round", Report::Attention);
		let now = run("Daily health check", Report::Attention);
		assert_eq!(
			older_attention(&conn, now, "Daily health check").unwrap(),
			[old]
		);

		let (c, _) = convs::resolve_by_genco(&conn, old, "newer run").unwrap();
		assert_eq!(c.report, Some(Report::Ok));
		assert!(
			older_attention(&conn, now, "Daily health check")
				.unwrap()
				.is_empty()
		);
	}

	fn msg(id: i64, data: MessageData) -> Message {
		Message {
			id,
			conversation_id: 1,
			created_at: 0,
			data,
		}
	}

	fn user(text: &str) -> MessageData {
		MessageData::User {
			text: text.into(),
			user_name: Some("a".into()),
			files: vec![],
		}
	}

	fn call(id: &str) -> MessageData {
		MessageData::ToolCall {
			call_id: id.into(),
			name: "shell".into(),
			input: json!({}),
			context: None,
		}
	}

	fn output(id: &str) -> MessageData {
		MessageData::ToolOutput {
			call_id: id.into(),
			output: "ok".into(),
			is_error: false,
		}
	}

	fn kinds(input: &[Input]) -> Vec<String> {
		input
			.iter()
			.map(|i| match i {
				Input::Text { content, .. } => content.clone(),
				Input::ToolCall { id, .. } => format!("call {id}"),
				Input::ToolCallOutput { id, .. } => format!("out {id}"),
				Input::Message { .. } => "message".into(),
			})
			.collect()
	}

	#[tokio::test]
	async fn the_exit_waits_for_reports_on_their_way() {
		let cfg = Arc::new(toml::from_str::<Config>("").unwrap());
		let runner = Runner::new(cfg, Db::memory(), Events::new());
		let done = Arc::new(AtomicBool::new(false));
		let flag = done.clone();
		runner.send_later(async move {
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
			flag.store(true, Ordering::SeqCst);
		});
		runner.sent(std::time::Duration::from_secs(5)).await;
		assert!(done.load(Ordering::SeqCst));
		assert!(runner.0.sending.lock().unwrap().is_empty());
	}

	#[test]
	fn message_during_tool_call_is_held_back() {
		let messages = vec![
			msg(1, user("a")),
			msg(2, call("x")),
			msg(3, call("y")),
			msg(4, user("b")),
			msg(5, output("x")),
			msg(6, output("y")),
		];

		assert_eq!(
			kinds(&to_input(&messages)),
			["[a] a", "call x", "call y", "out x", "out y", "[a] b"]
		);
	}

	#[test]
	fn calls_without_output_are_dropped() {
		let messages =
			vec![msg(1, user("a")), msg(2, call("x")), msg(3, user("b"))];

		assert_eq!(kinds(&to_input(&messages)), ["[a] a", "[a] b"]);
	}
}
