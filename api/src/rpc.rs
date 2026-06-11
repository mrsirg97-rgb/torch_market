// RPC proxy (prompt-005): the browser's Solana RPC path, folded into the api
// tier so the Helius key lives in Secret Manager and the traffic rides Cloud
// Armor. Stateless passthrough — no DB involvement; the SELECT-only posture
// is untouched.
//
//   POST /rpc     → forward JSON-RPC body to RPC_UPSTREAM, key already inline
//   GET  /rpc-ws  → WebSocket bridge to the upstream's WS endpoint
//                   (account subscriptions); frames pumped both directions,
//                   close propagates. Client reconnect handles drops.

use axum::{
    body::Bytes,
    extract::{
        ws::{Message as AxMsg, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as TgMsg;
use tracing::{debug, warn};

use crate::state::AppState;

const MAX_BODY: usize = 64 * 1024; // JSON-RPC requests are small; cap abuse.

pub async fn rpc_http(State(state): State<AppState>, body: Bytes) -> Response {
    let Some(upstream) = state.rpc_upstream.clone() else {
        return (StatusCode::NOT_IMPLEMENTED, "rpc proxy not configured").into_response();
    };
    if body.len() > MAX_BODY {
        return (StatusCode::PAYLOAD_TOO_LARGE, "body too large").into_response();
    }
    match state
        .http
        .post(&upstream)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            match resp.bytes().await {
                Ok(b) => (status, [("content-type", "application/json")], b).into_response(),
                Err(e) => {
                    warn!(error = %e, "rpc upstream body read failed");
                    (StatusCode::BAD_GATEWAY, "upstream error").into_response()
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "rpc upstream request failed");
            (StatusCode::BAD_GATEWAY, "upstream error").into_response()
        }
    }
}

pub async fn rpc_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let Some(upstream) = state.rpc_upstream.clone() else {
        return (StatusCode::NOT_IMPLEMENTED, "rpc proxy not configured").into_response();
    };
    let ws_upstream = upstream.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    ws.on_upgrade(move |client| bridge(client, ws_upstream))
}

async fn bridge(client: WebSocket, upstream_url: String) {
    let (upstream, _) = match tokio_tungstenite::connect_async(&upstream_url).await {
        Ok(ok) => ok,
        Err(e) => {
            warn!(error = %e, "rpc-ws upstream connect failed");
            return;
        }
    };
    debug!("rpc-ws bridge up");
    let (mut ctx, mut crx) = client.split();
    let (mut utx, mut urx) = upstream.split();

    let c2u = async {
        while let Some(Ok(msg)) = crx.next().await {
            let out = match msg {
                AxMsg::Text(t) => TgMsg::Text(t.to_string()),
                AxMsg::Binary(b) => TgMsg::Binary(b.into()),
                AxMsg::Ping(p) => TgMsg::Ping(p.into()),
                AxMsg::Pong(p) => TgMsg::Pong(p.into()),
                AxMsg::Close(_) => break,
            };
            if utx.send(out).await.is_err() {
                break;
            }
        }
        let _ = utx.close().await;
    };
    let u2c = async {
        while let Some(Ok(msg)) = urx.next().await {
            let out = match msg {
                TgMsg::Text(t) => AxMsg::Text(t.to_string().into()),
                TgMsg::Binary(b) => AxMsg::Binary(b.to_vec().into()),
                TgMsg::Ping(p) => AxMsg::Ping(p.to_vec().into()),
                TgMsg::Pong(p) => AxMsg::Pong(p.to_vec().into()),
                TgMsg::Close(_) => break,
                TgMsg::Frame(_) => continue,
            };
            if ctx.send(out).await.is_err() {
                break;
            }
        }
        let _ = ctx.close().await;
    };
    tokio::select! { _ = c2u => {}, _ = u2c => {} }
    debug!("rpc-ws bridge closed");
}
