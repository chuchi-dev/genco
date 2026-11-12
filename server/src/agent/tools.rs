use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use soe_llms::Tool;
use tokio::fs;
use tokio::process::Command;

const MAX_OUTPUT: usize = 30_000;

static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(|| {
	vec![
		tool::<ShellParams>(
			"shell",
			"Run a bash command on the machine as root and return its exit \
			code, stdout and stderr. The command blocks until it exits, so \
			detach long running processes (systemd-run, or nohup with \
			redirected output).",
		),
		tool::<ReadParams>(
			"read_file",
			"Read a text file. Lines are prefixed with their line number. \
			The offset starts at 1.",
		),
		tool::<WriteParams>(
			"write_file",
			"Create or overwrite a file with the given content. Parent \
			directories are created automatically.",
		),
		tool::<EditParams>(
			"edit_file",
			"Replace an exact piece of text in a file. old_text must match \
			exactly once unless replace_all is set.",
		),
		tool::<ShowPageParams>(
			"show_page",
			"Open a page of the panel in the browser of the admin who has \
			this conversation open, e.g. to show what you set up or where \
			to click.",
		),
	]
});

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShowPageParams {
	/// a path of the panel: /, /clients/<id>, /groups/<id>,
	/// /groups/<id>/<tab> (domains, files, services, config, databases,
	/// deploy, backups, usage), /groups/<id>/files/<folder>,
	/// /domains/<id>, /databases/<id>, /mail/<id>, /passwords/<id> (opens
	/// the password in Start → Passwords)
	pub path: String,
}

/// The path if it is one of the panel's own pages, nothing else.
pub fn page_path(input: Value) -> Result<String, String> {
	let p: ShowPageParams = parse(input)?;
	// browsers read `/\host` as `//host`; deletions are confirmed by an
	// admin from the mail, never shown by the agent
	let ok = p.path.starts_with('/')
		&& !p.path.starts_with("//")
		&& !p.path.contains('\\')
		&& !p.path.starts_with("/confirm")
		&& p.path.len() < 300
		&& p.path.chars().all(|c| c.is_ascii_graphic());
	if !ok {
		return Err(
			"the path must be a page of the panel like /groups/3".into()
		);
	}
	Ok(p.path)
}

fn tool<T: JsonSchema>(name: &str, description: &str) -> Tool {
	Tool {
		name: name.into(),
		description: description.into(),
		parameters: Some(schema_for!(T).to_value()),
	}
}

pub fn definitions() -> Vec<Tool> {
	TOOLS.clone()
}

/// Runs a tool call of the conversation, long outputs get truncated.
pub async fn call(
	name: &str,
	input: Value,
	workspace: &Path,
	conversation: i64,
) -> Result<String, String> {
	let out = match name {
		"shell" => shell(parse(input)?, workspace, conversation).await,
		"read_file" => read_file(parse(input)?, workspace).await,
		"write_file" => write_file(parse(input)?, workspace).await,
		"edit_file" => edit_file(parse(input)?, workspace).await,
		_ => Err(format!("unknown tool {name}")),
	};

	out.map(|s| truncate(&s)).map_err(|s| truncate(&s))
}

fn parse<T: DeserializeOwned>(input: Value) -> Result<T, String> {
	serde_json::from_value(input).map_err(|e| format!("invalid input: {e}"))
}

fn resolve(path: &str, workspace: &Path) -> PathBuf {
	workspace.join(path)
}

/// A command's exit code, stdout and stderr as a tool returns them.
pub(crate) fn output_text(out: &std::process::Output) -> String {
	let mut s = match out.status.code() {
		Some(code) => format!("exit code: {code}\n"),
		None => "killed by signal\n".to_string(),
	};
	if !out.stdout.is_empty() {
		s.push_str(&format!(
			"stdout:\n{}\n",
			String::from_utf8_lossy(&out.stdout)
		));
	}
	if !out.stderr.is_empty() {
		s.push_str(&format!(
			"stderr:\n{}\n",
			String::from_utf8_lossy(&out.stderr)
		));
	}
	s
}

/// Keeps the start and the end of long outputs.
pub(crate) fn truncate(s: &str) -> String {
	if s.len() <= MAX_OUTPUT {
		return s.to_string();
	}

	let mut head = MAX_OUTPUT / 3;
	while !s.is_char_boundary(head) {
		head -= 1;
	}
	let mut tail = s.len() - (MAX_OUTPUT - head);
	while !s.is_char_boundary(tail) {
		tail += 1;
	}

	format!(
		"{}\n\n[... {} bytes truncated ...]\n\n{}",
		&s[..head],
		tail - head,
		&s[tail..]
	)
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ShellParams {
	command: String,
	/// working directory, defaults to the workspace
	cwd: Option<String>,
	/// defaults to 300, max 3600
	timeout_secs: Option<u64>,
}

/// Kills the whole process group if the command times out or the task gets
/// stopped while the command runs.
struct KillGroup(Option<u32>);

impl Drop for KillGroup {
	fn drop(&mut self) {
		if let Some(pid) = self.0 {
			let _ = std::process::Command::new("kill")
				.args(["-KILL", "--", &format!("-{pid}")])
				.stderr(Stdio::null())
				.status();
		}
	}
}

async fn shell(
	p: ShellParams,
	workspace: &Path,
	conversation: i64,
) -> Result<String, String> {
	let cwd = p
		.cwd
		.map(|c| resolve(&c, workspace))
		.unwrap_or_else(|| workspace.to_path_buf());
	let secs = p.timeout_secs.unwrap_or(300).clamp(1, 3600);

	let child = Command::new("bash")
		.arg("-c")
		.arg(&p.command)
		// commands like `genco domain delete` say who asked
		.env("GENCO_AGENT", "1")
		// a rejected deletion is answered in this conversation (actions.rs)
		.env("GENCO_CONVERSATION", conversation.to_string())
		.current_dir(&cwd)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.process_group(0)
		.kill_on_drop(true)
		.spawn()
		.map_err(|e| format!("failed to spawn bash: {e}"))?;
	let mut guard = KillGroup(child.id());

	let out = tokio::time::timeout(
		Duration::from_secs(secs),
		child.wait_with_output(),
	)
	.await
	.map_err(|_| format!("command timed out after {secs}s"))?
	.map_err(|e| format!("failed to run command: {e}"))?;

	// background processes started by the command may keep running
	guard.0 = None;

	let mut s = output_text(&out);

	// gets truncated, the agent can grep the file instead of running the
	// command again
	if s.len() > MAX_OUTPUT {
		let path = std::env::temp_dir()
			.join(format!("genco-output-{}.txt", crate::db::now()));
		if fs::write(&path, &s).await.is_ok() {
			s = format!("full output saved to {}\n{s}", path.display());
		}
	}

	Ok(s)
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadParams {
	path: String,
	/// first line to read, starts at 1
	offset: Option<usize>,
	/// number of lines, defaults to 2000
	limit: Option<usize>,
}

async fn read_file(p: ReadParams, workspace: &Path) -> Result<String, String> {
	let path = resolve(&p.path, workspace);
	let ctn = fs::read_to_string(&path)
		.await
		.map_err(|e| format!("failed to read {}: {e}", path.display()))?;

	let offset = p.offset.unwrap_or(1).max(1);
	let limit = p.limit.unwrap_or(2000);
	let total = ctn.lines().count();

	let mut out = String::new();
	for (i, line) in ctn.lines().enumerate().skip(offset - 1).take(limit) {
		out.push_str(&format!("{:>6}\t{line}\n", i + 1));
	}

	if offset - 1 + limit < total {
		out.push_str(&format!("\n[file has {total} lines]\n"));
	}

	Ok(out)
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WriteParams {
	path: String,
	content: String,
}

async fn write_file(
	p: WriteParams,
	workspace: &Path,
) -> Result<String, String> {
	let path = resolve(&p.path, workspace);

	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent).await.map_err(|e| {
			format!("failed to create {}: {e}", parent.display())
		})?;
	}

	fs::write(&path, &p.content)
		.await
		.map_err(|e| format!("failed to write {}: {e}", path.display()))?;

	Ok(format!(
		"wrote {} bytes to {}",
		p.content.len(),
		path.display()
	))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EditParams {
	path: String,
	old_text: String,
	new_text: String,
	#[serde(default)]
	replace_all: bool,
}

async fn edit_file(p: EditParams, workspace: &Path) -> Result<String, String> {
	let path = resolve(&p.path, workspace);
	let ctn = fs::read_to_string(&path)
		.await
		.map_err(|e| format!("failed to read {}: {e}", path.display()))?;

	let count = ctn.matches(&p.old_text).count();
	if count == 0 {
		return Err("old_text not found".into());
	}
	if count > 1 && !p.replace_all {
		return Err(format!(
			"old_text found {count} times, add more context or set replace_all"
		));
	}

	let ctn = ctn.replace(&p.old_text, &p.new_text);
	fs::write(&path, ctn)
		.await
		.map_err(|e| format!("failed to write {}: {e}", path.display()))?;

	Ok(format!("replaced {count} occurrence(s)"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn truncate_keeps_head_and_tail() {
		let s = format!("start{}end", "x".repeat(MAX_OUTPUT * 2));
		let t = truncate(&s);

		assert!(t.len() < MAX_OUTPUT + 100);
		assert!(t.starts_with("start"));
		assert!(t.ends_with("end"));
	}

	#[tokio::test]
	async fn shell_reports_exit_code_and_output() {
		let out = shell(
			ShellParams {
				command: "echo hi $GENCO_CONVERSATION; echo err >&2; exit 3"
					.into(),
				cwd: None,
				timeout_secs: None,
			},
			Path::new("/tmp"),
			7,
		)
		.await
		.unwrap();

		assert!(out.contains("exit code: 3"));
		assert!(out.contains("stdout:\nhi 7"));
		assert!(out.contains("stderr:\nerr"));
	}

	#[tokio::test]
	async fn shell_times_out() {
		let err = shell(
			ShellParams {
				command: "sleep 5".into(),
				cwd: None,
				timeout_secs: Some(1),
			},
			Path::new("/tmp"),
			1,
		)
		.await
		.unwrap_err();

		assert!(err.contains("timed out"));
	}
}
