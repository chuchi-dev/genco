//! Anthropic models go directly to Anthropic when the config has an
//! `anthropic-key`, everything else (and a refused request) to OpenRouter.
//! The agent's model is chosen in Settings → Agent, `model` in the config
//! is the default.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use soe_llms::{Llms, LlmsConfig, LlmsError, Model, Request, ResponseStream};

use crate::AppState;
use crate::config::Config;
use crate::db::{Db, get_setting, set_setting};
use crate::error::Error;
use crate::users::Authed;

/// The models Settings → Agent offers, as OpenRouter ids and names.
const MODELS: &[(&str, &str)] = &[
	("anthropic/claude-opus-5.5", "Claude Opus 5.5"),
	("anthropic/claude-sonnet-5.5", "Claude Sonnet 5.5"),
	("google/gemini-4-argon", "Gemini 4 Argon"),
	("openai/gpt-6.1-sol", "GPT-6.1 Sol"),
];

/// Whether a key in the config reaches the model.
pub(crate) fn reachable(cfg: &Config, id: &str) -> bool {
	cfg.openrouter_key.is_some()
		|| (cfg.anthropic_key.is_some() && id.starts_with("anthropic/"))
}

/// The agent's model: the one chosen in the panel, else the config's.
pub fn model(cfg: &Config, db: &Db) -> String {
	get_setting(&db.conn(), "model")
		.ok()
		.flatten()
		.unwrap_or_else(|| cfg.model.clone())
}

#[derive(Clone)]
pub struct Llm {
	llms: Llms,
	direct: bool,
	openrouter: bool,
}

impl Llm {
	pub fn new(cfg: &Config) -> Self {
		let llms = Llms::new(
			LlmsConfig::new()
				.openrouter(cfg.openrouter_key.clone())
				.anthropic(cfg.anthropic_key.clone()),
		);

		Self {
			llms,
			direct: cfg.anthropic_key.is_some(),
			openrouter: cfg.openrouter_key.is_some(),
		}
	}

	/// `req.model` is an OpenRouter model.
	pub async fn request(
		&self,
		req: &Request,
	) -> Result<ResponseStream, LlmsError> {
		if let Some(model) = self.direct_model(&req.model) {
			let direct = Request {
				model,
				..req.clone()
			};
			match self.llms.request(&direct).await {
				Ok(stream) => return Ok(stream),
				Err(e) if self.openrouter => {
					tracing::warn!("anthropic failed, using openrouter: {e}")
				}
				Err(e) => return Err(e),
			}
		}

		self.llms.request(req).await
	}

	fn direct_model(&self, model: &Model) -> Option<Model> {
		let Model::OpenRouter(Some(id)) = model else {
			return None;
		};
		if !self.direct {
			return None;
		}

		match id.as_str() {
			"anthropic/claude-opus-5.5" => Some(Model::ClaudeOpus5_5),
			"anthropic/claude-sonnet-5.5" => Some(Model::ClaudeSonnet5_5),
			"anthropic/claude-haiku-5.5" => Some(Model::ClaudeHaiku5_5),
			_ => None,
		}
	}
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelChoice {
	id: &'static str,
	name: &'static str,
	/// a key in the config reaches it
	available: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelSettings {
	model: String,
	models: Vec<ModelChoice>,
}

async fn get_model(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
) -> Json<ModelSettings> {
	Json(ModelSettings {
		model: model(&cfg, &db),
		models: MODELS
			.iter()
			.map(|(id, name)| ModelChoice {
				id,
				name,
				available: reachable(&cfg, id),
			})
			.collect(),
	})
}

#[derive(Debug, Deserialize)]
struct SetModel {
	model: String,
}

async fn set_model(
	_authed: Authed,
	State(cfg): State<Arc<Config>>,
	State(db): State<Db>,
	Json(req): Json<SetModel>,
) -> crate::error::Result<Json<()>> {
	if !MODELS.iter().any(|(id, _)| *id == req.model) {
		return Err(Error::Invalid(format!("unknown model {}", req.model)));
	}
	if !reachable(&cfg, &req.model) {
		return Err(Error::Invalid(
			"this model needs an openrouter-key in /etc/genco/config.toml"
				.into(),
		));
	}
	set_setting(&db.conn(), "model", &req.model)?;
	Ok(Json(()))
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/settings/model", get(get_model).post(set_model))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn cfg(anthropic: bool, openrouter: bool) -> Config {
		let mut toml = String::new();
		if anthropic {
			toml.push_str("anthropic-key = \"a\"\n");
		}
		if openrouter {
			toml.push_str("openrouter-key = \"o\"\n");
		}
		toml::from_str(&toml).unwrap()
	}

	#[test]
	fn other_models_need_openrouter() {
		let direct = cfg(true, false);
		assert!(reachable(&direct, "anthropic/claude-sonnet-5.5"));
		assert!(!reachable(&direct, "openai/gpt-6.1-sol"));
		assert!(reachable(&cfg(false, true), "openai/gpt-6.1-sol"));
	}

	#[test]
	fn the_panel_overrides_the_config() {
		let cfg = cfg(true, false);
		let db = Db::memory();
		assert_eq!(model(&cfg, &db), "anthropic/claude-opus-5.5");
		set_setting(&db.conn(), "model", "anthropic/claude-sonnet-5.5")
			.unwrap();
		assert_eq!(model(&cfg, &db), "anthropic/claude-sonnet-5.5");
	}
}
