//! `genco chat`: the agent in the terminal. It runs in this process, so it
//! also works when the server is down.

use std::io::Write as _;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::broadcast;

use crate::agent::Runner;
use crate::config::Config;
use crate::conversations::{self as convs, MessageData};
use crate::db::Db;
use crate::events::{Event, Events};

const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

pub async fn main(cfg: Config, resume: Option<i64>) -> Result<(), String> {
	let db = Db::open(&cfg.db_path())?;
	let events = Events::new();
	let runner = Runner::new(Arc::new(cfg), db.clone(), events.clone());

	let mut rx = events.subscribe();
	tokio::spawn(async move {
		loop {
			match rx.recv().await {
				Ok(ev) => print_event(ev),
				// a long answer can outrun the printing, it goes on
				Err(broadcast::error::RecvError::Lagged(_)) => continue,
				Err(broadcast::error::RecvError::Closed) => break,
			}
		}
	});

	let mut id = resume;
	if let Some(id) = id {
		let conv = convs::get(&db.conn(), id).map_err(|e| e.to_string())?;
		println!("{DIM}continuing \"{}\"{RESET}", conv.title);
	}
	println!("{DIM}genco agent, it has root access. Ctrl-D to quit.{RESET}");

	let mut lines = BufReader::new(tokio::io::stdin()).lines();
	loop {
		print!("\n> ");
		std::io::stdout().flush().ok();
		let Some(line) = lines.next_line().await.map_err(|e| e.to_string())?
		else {
			println!();
			return Ok(());
		};
		if line.trim().is_empty() {
			continue;
		}

		let conv_id = match id {
			Some(id) => id,
			None => {
				let new = convs::create(&db.conn(), &line)
					.map_err(|e| e.to_string())?;
				id = Some(new);
				new
			}
		};
		convs::insert_message(
			&db.conn(),
			conv_id,
			MessageData::User {
				text: line,
				user_name: None,
			},
		)
		.map_err(|e| e.to_string())?;

		println!();
		if !runner.run_now(conv_id).await {
			println!(
				"{RED}the panel runs this conversation right now, the message \
				waits there{RESET}"
			);
		}
	}
}

fn print_event(ev: Event) {
	match ev {
		Event::Delta { text, .. } => print!("{text}"),
		Event::Message { message } => match message.data {
			MessageData::Assistant { .. } => println!(),
			MessageData::ToolCall { name, input, .. } => {
				let detail = input
					.get("command")
					.or_else(|| input.get("path"))
					.and_then(|v| v.as_str())
					.unwrap_or("");
				println!("{DIM}[{name}] {}{RESET}", first_lines(detail, 3));
			}
			MessageData::ToolOutput { output, .. } => {
				println!("{DIM}{}{RESET}", first_lines(&output, 4));
			}
			MessageData::Error { text } => println!("{RED}{text}{RESET}"),
			_ => {}
		},
		_ => {}
	}
	std::io::stdout().flush().ok();
}

fn first_lines(s: &str, n: usize) -> String {
	let lines: Vec<&str> = s.lines().collect();
	let mut out = lines[..lines.len().min(n)].join("\n");
	if lines.len() > n {
		out.push_str(&format!("\n... {} more lines", lines.len() - n));
	}
	out
}
