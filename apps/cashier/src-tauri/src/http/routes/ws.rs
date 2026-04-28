use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// First-frame handshake timeout. Browsers can't set Authorization on
/// `new WebSocket(url)` so the upgrade is unconditional and auth happens
/// in-band as the first message.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientHello {
    Hello { token: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg {
    HelloOk,
    Error {
        code: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/ws", get(ws_handler))
}

async fn ws_handler(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let claims = match handshake(&mut socket, &state).await {
        Ok(c) => c,
        Err(reason) => {
            tracing::debug!(reason = %reason, "ws handshake failed");
            let _ = send_error(&mut socket, "unauthorized", Some(reason)).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    if send_json(&mut socket, &ServerMsg::HelloOk).await.is_err() {
        return;
    }
    tracing::info!(staff_id = claims.staff_id, "ws subscriber attached");

    let mut rx = state.broadcast_tx.subscribe();
    while let Ok(notice) = rx.recv().await {
        let json = match serde_json::to_string(&notice) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(?e, "ws serialize");
                continue;
            }
        };
        if socket.send(Message::Text(json)).await.is_err() {
            break;
        }
    }
}

async fn handshake(socket: &mut WebSocket, state: &Arc<AppState>) -> Result<TokenClaims, String> {
    let recv = tokio::time::timeout(HANDSHAKE_TIMEOUT, socket.recv())
        .await
        .map_err(|_| "handshake timeout".to_string())?;
    let msg = recv
        .ok_or_else(|| "client closed".to_string())?
        .map_err(|e| format!("recv error: {e}"))?;

    let text = match msg {
        Message::Text(s) => s,
        _ => return Err("expected text hello".into()),
    };

    let hello: ClientHello =
        serde_json::from_str(&text).map_err(|e| format!("bad hello json: {e}"))?;
    let ClientHello::Hello { token } = hello;

    // verify is sync + may hit sqlite (denylist). Run under spawn_blocking
    // to keep the runtime healthy even if the DB lock is contended.
    let auth = state.auth.clone();
    let claims = tokio::task::spawn_blocking(move || auth.verify(&token))
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|_| "invalid token".to_string())?;
    Ok(claims)
}

async fn send_json<T: Serialize>(socket: &mut WebSocket, v: &T) -> Result<(), axum::Error> {
    let json = serde_json::to_string(v).unwrap_or_else(|_| "{}".into());
    socket.send(Message::Text(json)).await
}

async fn send_error(
    socket: &mut WebSocket,
    code: &str,
    message: Option<String>,
) -> Result<(), axum::Error> {
    send_json(
        socket,
        &ServerMsg::Error {
            code: code.into(),
            message,
        },
    )
    .await
}
