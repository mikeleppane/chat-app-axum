use axum::extract::ws::Message;
use axum::response::IntoResponse;
use axum::routing::{get, get_service};
use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    http::StatusCode,
    Extension, Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use sync_wrapper::SyncWrapper;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{mpsc, RwLock};
use tower_http::services::ServeDir;

async fn hello_world() -> &'static str {
    "Hello, world!"
}

static NEXT_USERID: AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
type Users = Arc<RwLock<HashMap<usize, UnboundedSender<Message>>>>;

#[shuttle_service::main]
async fn axum(
    #[shuttle_static_folder::StaticFolder] static_folder: PathBuf,
) -> shuttle_service::ShuttleAxum {
    let router = router(static_folder);
    let sync_wrapper = SyncWrapper::new(router);

    Ok(sync_wrapper)
}

fn router(static_folder: PathBuf) -> Router {
    let directory = ServeDir::new(static_folder);
    let directory = get_service(directory).handle_error(handle_error);
    let users = Users::default();
    let router = Router::new()
        .route("/ws", get(handle_websocket_upgrade))
        .layer(Extension(users))
        .fallback_service(directory);
    router
}

async fn handle_websocket_upgrade(
    ws: WebSocketUpgrade,
    Extension(users): Extension<Users>,
) -> impl IntoResponse {
    ws.on_upgrade(|ws| handle_socket(ws, users))
}

#[derive(Serialize, Deserialize)]
struct ChatMessage {
    name: String,
    message: String,
    uid: Option<usize>,
}

async fn handle_socket(ws: WebSocket, users: Users) {
    let user_id = NEXT_USERID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (tx, mut rx): (UnboundedSender<Message>, UnboundedReceiver<Message>) =
        mpsc::unbounded_channel();
    users.write().await.insert(user_id, tx);
    let (mut sender, mut receiver) = ws.split();

    tokio::task::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {}
        }
    });

    while let Some(Ok(result)) = receiver.next().await {
        if let Ok(result) = enrich_result(user_id, result) {
            broadcast_message(result, &users).await;
        };
    }

    disconnect(user_id, &users);
}

fn enrich_result(user_id: usize, message: Message) -> Result<Message, serde_json::Error> {
    match message {
        Message::Text(message) => {
            let mut chat_message: ChatMessage = serde_json::from_str(&message)?;
            chat_message.uid = Some(user_id);
            let message = serde_json::to_string(&chat_message)?;
            Ok(Message::Text(message))
        }
        _ => Ok(message),
    }
}

async fn disconnect(user_id: usize, users: &Users) {
    users.write().await.remove(&user_id);
}

async fn broadcast_message(msg: Message, users: &Users) {
    if let Message::Text(msg) = msg {
        for (_, tx) in users.read().await.iter() {
            let _ = tx.send(Message::Text(msg.clone()));
        }
    }
}

async fn handle_error(_err: std::io::Error) -> impl IntoResponse {
    (StatusCode::NOT_FOUND, ":-(")
}
