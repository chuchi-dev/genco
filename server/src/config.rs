use std::path::{Path, PathBuf};

use serde::Deserialize;

/// `/etc/genco/config.toml`, written by `genco setup`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
	#[serde(default = "default_listen")]
	pub listen: String,
	#[serde(default = "default_data_dir")]
	pub data_dir: PathBuf,
	/// the panel's domain
	pub domain: Option<String>,
	/// for Let's Encrypt
	pub email: Option<String>,
	/// the agent's model as OpenRouter id, Anthropic models go directly to
	/// Anthropic when `anthropic-key` is set
	#[serde(default = "default_model")]
	pub model: String,
	pub anthropic_key: Option<String>,
	/// host name of IMAP, SMTP and webmail, `mail.<domain>` by default
	pub mail_host: Option<String>,
	pub openrouter_key: Option<String>,
	/// sites that may show the panel in a frame (`https://host`), none by
	/// default
	#[serde(default)]
	pub frame_ancestors: Vec<String>,
}

fn default_listen() -> String {
	"127.0.0.1:4100".into()
}

fn default_data_dir() -> PathBuf {
	"/var/lib/genco".into()
}

fn default_model() -> String {
	"anthropic/claude-opus-5.5".into()
}

impl Config {
	/// A link into the panel, `None` without a domain.
	pub fn panel_url(&self, path: &str) -> Option<String> {
		self.domain.as_ref().map(|d| format!("https://{d}{path}"))
	}

	pub fn load(path: &Path) -> Result<Self, String> {
		let ctn = std::fs::read_to_string(path)
			.map_err(|e| format!("failed to read {}: {e}", path.display()))?;
		toml::from_str(&ctn)
			.map_err(|e| format!("invalid config {}: {e}", path.display()))
	}

	/// The port the panel listens on, 4100 by default.
	pub fn panel_port(&self) -> u16 {
		self.listen
			.rsplit(':')
			.next()
			.and_then(|p| p.parse().ok())
			.unwrap_or(4100)
	}

	pub fn db_path(&self) -> PathBuf {
		self.data_dir.join("genco.db")
	}

	/// what the agent writes down about this server, part of its
	/// instructions
	pub fn notes_path(&self) -> PathBuf {
		self.data_dir.join("notes.md")
	}
}
