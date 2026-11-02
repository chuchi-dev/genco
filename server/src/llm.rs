//! Anthropic models go directly to Anthropic when the config has an
//! `anthropic-key`, everything else (and a refused request) to OpenRouter.

use soe_llms::{Llms, LlmsConfig, LlmsError, Model, Request, ResponseStream};

use crate::config::Config;

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
