use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::sse::{self, KeepAlive, Sse};
use axum::routing::get;
use futures::{Stream, StreamExt as _};
use serde::Serialize;
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::BroadcastStream;

use crate::AppState;
use crate::backup::Backup;
use crate::conversations::{Conversation, Message};
use crate::db::now;
use crate::deploy::Deploy;
use crate::users::Authed;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
	/// first event of every stream, the browser reloads when `boot`
	/// changes (the webui is part of the binary)
	Hello { boot: i64 },
	/// clients, groups, domains or databases changed
	Tree,
	/// a conversation was created or its status changed
	Conversation { conversation: Conversation },
	/// a new message was stored
	Message { message: Message },
	/// a deploy was queued, started or finished
	Deploy { deploy: Deploy },
	/// a backup run started or finished
	Backup { backup: Backup },
	/// the agent opens a page in the browsers showing its conversation
	#[serde(rename_all = "camelCase")]
	ShowPage { conversation_id: i64, path: String },
	/// streamed assistant text which is not stored yet
	#[serde(rename_all = "camelCase")]
	Delta { conversation_id: i64, text: String },
	/// a proposal of an AI agent or a pending action came in or was
	/// decided (Approvals)
	Approvals,
}

/// Broadcasts every event to every connected browser.
#[derive(Clone)]
pub struct Events {
	tx: broadcast::Sender<Event>,
	/// identifies this run of genco
	boot: i64,
	/// true once genco shuts down, the streams end so the graceful
	/// shutdown doesn't wait for them
	closed: watch::Sender<bool>,
}

impl Events {
	pub fn new() -> Self {
		Self {
			tx: broadcast::channel(1024).0,
			boot: now(),
			closed: watch::channel(false).0,
		}
	}

	/// Ends all streams, the browsers reconnect when genco is back.
	pub fn close(&self) {
		self.closed.send_replace(true);
	}

	/// Resolves when `close` was called.
	pub fn closed(&self) -> impl Future<Output = ()> + use<> {
		let mut rx = self.closed.subscribe();
		async move {
			let _ = rx.wait_for(|closed| *closed).await;
		}
	}

	pub fn subscribe(&self) -> broadcast::Receiver<Event> {
		self.tx.subscribe()
	}

	pub fn send(&self, event: Event) {
		// fails only if nobody is listening
		let _ = self.tx.send(event);
	}
}

async fn stream(
	authed: Authed,
	State(events): State<Events>,
	State(db): State<crate::db::Db>,
) -> Sse<impl Stream<Item = Result<sse::Event, Infallible>>> {
	// a session that ended (log out, new password, expired) gets no more
	// events, the stream closes within a minute
	let token = authed.token;
	let closed = events.closed();
	let expired = async move {
		loop {
			tokio::time::sleep(Duration::from_secs(60)).await;
			let alive: bool = db
				.conn()
				.query_row(
					"SELECT EXISTS (SELECT 1 FROM sessions WHERE token = ?1)",
					[&token],
					|r| r.get(0),
				)
				.unwrap_or(false);
			if !alive {
				return;
			}
		}
	};
	let ended = async move {
		tokio::select! {
			_ = expired => {}
			_ = closed => {}
		}
	};

	// lagged receivers just skip events, the browser reloads its data when
	// it reconnects
	let hello = Event::Hello { boot: events.boot };
	let stream = BroadcastStream::new(events.tx.subscribe())
		.filter_map(|ev| async move { ev.ok() });
	let stream = futures::stream::once(async move { hello })
		.chain(stream)
		.map(|ev| Ok(sse::Event::default().json_data(ev).unwrap()))
		.take_until(Box::pin(ended));

	Sse::new(stream)
		.keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

pub fn routes() -> Router<AppState> {
	Router::new().route("/", get(stream))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn close_ends_waiting_and_later_streams() {
		let events = Events::new();
		let waiting = tokio::spawn(events.closed());
		events.close();
		let soon = Duration::from_secs(1);
		tokio::time::timeout(soon, waiting).await.unwrap().unwrap();
		tokio::time::timeout(soon, events.closed()).await.unwrap();
	}
}
