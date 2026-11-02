use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error, Clone, Serialize)]
#[serde(tag = "type", content = "detail", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Error {
	#[error("Email or password incorrect")]
	LoginIncorrect,
	#[error("The code of the authenticator app is missing")]
	TotpRequired,
	#[error("Missing session token in request")]
	MissingSessionToken,
	#[error("Invalid session token in request")]
	InvalidSessionToken,
	/// a bearer token genco doesn't know (API, runner)
	#[error("Unknown or deleted token")]
	InvalidToken,
	#[error("Not allowed")]
	Forbidden,
	#[error("Resource not found")]
	NotFound,
	/// a build job the runner may stop, its deploy gave up on it
	#[error("Gone")]
	Gone,
	#[error("{0}")]
	Invalid(String),
	#[error("Internal server error: {0}")]
	Internal(String),
}

impl Error {
	pub fn status_code(&self) -> StatusCode {
		match self {
			Self::LoginIncorrect
			| Self::TotpRequired
			| Self::MissingSessionToken => StatusCode::UNAUTHORIZED,
			Self::InvalidSessionToken
			| Self::InvalidToken
			| Self::Forbidden => StatusCode::FORBIDDEN,
			Self::NotFound => StatusCode::NOT_FOUND,
			Self::Gone => StatusCode::GONE,
			Self::Invalid(_) => StatusCode::BAD_REQUEST,
			Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
		}
	}
}

#[derive(Debug, Clone, Serialize)]
struct ErrorResponse {
	message: String,
	error: Error,
}

impl IntoResponse for Error {
	fn into_response(self) -> Response {
		let status = self.status_code();
		let body = ErrorResponse {
			message: self.to_string(),
			error: self,
		};
		(status, Json(body)).into_response()
	}
}

impl From<rusqlite::Error> for Error {
	fn from(e: rusqlite::Error) -> Self {
		match e {
			rusqlite::Error::QueryReturnedNoRows => Self::NotFound,
			rusqlite::Error::SqliteFailure(f, Some(msg))
				if f.code == rusqlite::ErrorCode::ConstraintViolation =>
			{
				Self::Invalid(constraint_message(&msg))
			}
			e => Self::Internal(e.to_string()),
		}
	}
}

/// "UNIQUE constraint failed: domains.name" -> "domains.name already exists"
fn constraint_message(msg: &str) -> String {
	if let Some(col) = msg.strip_prefix("UNIQUE constraint failed: ") {
		return format!("{col} already exists");
	}
	if msg.starts_with("FOREIGN KEY") {
		return "still in use or refers to something that doesn't exist".into();
	}
	msg.to_string()
}
