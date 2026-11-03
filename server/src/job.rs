//! What deploys and backups share: a log file per run that the panel
//! shows, one run of a kind at a time on the server (a file lock, the CLI
//! runs them too) and the output of commands in the log.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{
	AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader,
};
use tokio::process::Command;

pub struct Log {
	file: tokio::fs::File,
	/// the CLI prints the log too
	print: bool,
	/// false once an error was logged
	pub ok: bool,
}

/// The longest line and the most output a step logs.
const MAX_LINE: u64 = 8 * 1024;
const MAX_LOG: usize = 10 * 1024 * 1024;

pub fn path(dir: &str, id: i64) -> PathBuf {
	Path::new(dir).join(format!("{id}.log"))
}

/// The log of a run, empty if there is none.
pub async fn read(dir: &str, id: i64) -> String {
	tokio::fs::read_to_string(path(dir, id))
		.await
		.unwrap_or_default()
}

/// Waits until no other run holds `path`, the lock ends with the file.
pub async fn lock(path: impl Into<String>) -> Result<std::fs::File, String> {
	let path = path.into();
	let name = path.clone();
	tokio::task::spawn_blocking(move || {
		let file = std::fs::File::create(&path)?;
		file.lock()?;
		Ok::<_, std::io::Error>(file)
	})
	.await
	.unwrap()
	.map_err(|e| format!("lock {name}: {e}"))
}

/// The lock of `path` if nobody holds it, in a directory only root may
/// enter; `None` while another run holds it.
pub fn try_lock(path: &str) -> std::io::Result<Option<std::fs::File>> {
	if let Some(dir) = Path::new(path).parent() {
		crate::util::private_dir(dir)?;
	}
	let file = std::fs::File::create(path)?;
	match file.try_lock() {
		Ok(()) => Ok(Some(file)),
		Err(std::fs::TryLockError::WouldBlock) => Ok(None),
		Err(std::fs::TryLockError::Error(e)) => Err(e),
	}
}

/// Whether nobody holds the lock right now, e.g. a `genco backup` of the
/// timer in its own process.
pub fn is_free(path: &str) -> bool {
	let Ok(file) = std::fs::File::create(path) else {
		return true;
	};
	file.try_lock().is_ok()
}

impl Log {
	pub async fn create(
		dir: &str,
		id: i64,
		print: bool,
	) -> Result<Self, String> {
		let _ = tokio::fs::create_dir_all(dir).await;
		let file = tokio::fs::File::create(path(dir, id))
			.await
			.map_err(|e| format!("log of {id}: {e}"))?;
		Ok(Self {
			file,
			print,
			ok: true,
		})
	}

	pub async fn line(&mut self, line: &str) {
		if self.print {
			println!("{line}");
		}
		let _ = self.file.write_all(format!("{line}\n").as_bytes()).await;
		let _ = self.file.flush().await;
	}

	pub async fn error(&mut self, line: &str) {
		self.ok = false;
		self.line(&format!("error: {line}")).await;
	}

	/// Runs `cmd`, every line of its stdout goes to the log: the caller
	/// sends stderr there too (`2>&1`). Bytes that are no UTF-8 are
	/// replaced, a build may print anything and must not stop the reading.
	pub async fn run(&mut self, mut cmd: Command) -> Result<(), String> {
		let mut child = cmd
			.stdout(Stdio::piped())
			.stderr(Stdio::null())
			.spawn()
			.map_err(|e| e.to_string())?;
		let mut out = BufReader::new(child.stdout.take().unwrap());
		let mut buf = Vec::new();
		// a step that prints without end (cat /dev/zero, yes) must not
		// fill genco's memory or disk: long lines are cut, the rest of a
		// huge output is read but not logged
		let mut logged = 0;
		loop {
			buf.clear();
			let read =
				(&mut out).take(MAX_LINE).read_until(b'\n', &mut buf).await;
			match read {
				Ok(0) | Err(_) => break,
				Ok(n) if logged > MAX_LOG => {
					logged += n;
				}
				Ok(n) => {
					logged += n;
					let line = String::from_utf8_lossy(&buf);
					self.line(line.trim_end_matches(['\n', '\r'])).await;
					if logged > MAX_LOG {
						self.line("… more output, not logged").await;
					}
				}
			}
		}

		let status = child.wait().await.map_err(|e| e.to_string())?;
		if !status.success() {
			return Err(format!("failed ({status})"));
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn output_that_is_no_utf8_goes_on() {
		let dir = std::env::temp_dir()
			.join(format!("genco-job-{}", std::process::id()));
		let dir = dir.to_str().unwrap();
		let mut log = Log::create(dir, 1, false).await.unwrap();

		let mut cmd = Command::new("sh");
		cmd.args(["-c", r"printf 'a\n\377\376 b\nc\n'; exit 3"]);
		let res = log.run(cmd).await;

		assert!(res.unwrap_err().contains("failed"));
		assert_eq!(read(dir, 1).await, "a\n\u{fffd}\u{fffd} b\nc\n");
		std::fs::remove_dir_all(dir).unwrap();
	}
}
