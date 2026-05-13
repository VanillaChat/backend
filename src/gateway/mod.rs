pub mod events;
pub mod handler;

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::IntoResponse,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::state::SharedState;

pub type WsSender = mpsc::UnboundedSender<String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload {
    pub op: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub d: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub s: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<String>,
}

impl Payload {
    pub fn new(op: i32) -> Self {
        Self {
            op,
            d: None,
            s: None,
            t: None,
        }
    }

    pub fn with_data(op: i32, d: serde_json::Value) -> Self {
        Self {
            op,
            d: Some(d),
            s: None,
            t: None,
        }
    }

    pub fn dispatch(event: &str, d: serde_json::Value) -> Self {
        Self {
            op: 0,
            d: Some(d),
            s: None,
            t: Some(event.to_string()),
        }
    }
}

pub fn router() -> Router<SharedState> {
    Router::new().route("/gateway", get(ws_handler))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    ws.on_upgrade(move |socket| handle_socket(socket, state, cookie_header))
}

fn extract_token_from_cookie(cookie_header: Option<&str>) -> Option<String> {
    let cookie_str = cookie_header?;
    let is_production = std::env::var("NODE_ENV").unwrap_or_default() == "production";
    let cookie_name = if is_production {
        "__Host-Token"
    } else {
        "token"
    };

    for part in cookie_str.split(';') {
        let part = part.trim();
        if let Some((name, value)) = part.split_once('=') {
            if name.trim() == cookie_name {
                let decoded = urlencoding::decode(value.trim()).ok()?;
                return Some(decoded.to_string());
            }
        }
    }
    None
}

async fn handle_socket(socket: WebSocket, state: SharedState, cookie_header: Option<String>) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    state.gateway_metrics.socket_connected();

    tracing::info!("WebSocket connected, cookie_header: {:?}", cookie_header);

    let hello = Payload::with_data(
        10,
        serde_json::json!({
            "heartbeat_interval": 30000
        }),
    );

    if sender
        .send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .await
        .is_err()
    {
        state.gateway_metrics.socket_disconnected();
        return;
    }

    let state_clone = state.clone();

    let mut user_id: Option<String> = None;
    let mut subscribed_rooms: Vec<String> = Vec::new();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let mut broadcast_rx = state.gateway_tx.subscribe();
    let tx_for_broadcast = tx.clone();
    let rooms_for_broadcast = Arc::new(tokio::sync::RwLock::new(Vec::<String>::new()));
    let rooms_clone = rooms_for_broadcast.clone();

    let broadcast_task = tokio::spawn(async move {
        while let Ok(broadcast) = broadcast_rx.recv().await {
            let rooms = rooms_clone.read().await;
            if rooms.contains(&broadcast.room) {
                let _ = tx_for_broadcast.send(broadcast.message);
            }
        }
    });

    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                let payload: Result<Payload, _> = serde_json::from_str(&text);

                match payload {
                    Ok(payload) => {
                        state
                            .gateway_metrics
                            .record_inbound(payload.op, user_id.as_deref());
                        match payload.op {
                            1 => {
                                let ack = Payload::new(11);
                                let _ = tx.send(serde_json::to_string(&ack).unwrap());
                            }
                            2 => {
                                let token = extract_token_from_cookie(cookie_header.as_deref());

                                match events::identify::handle(
                                    &state_clone,
                                    &tx,
                                    token,
                                    &state_clone.config.token_secret,
                                )
                                .await
                                {
                                    Ok(result) => {
                                        user_id = Some(result.user_id.clone());
                                        subscribed_rooms = result.rooms.clone();
                                        state_clone
                                            .gateway_metrics
                                            .subscribe_rooms(&subscribed_rooms);

                                        {
                                            let mut rooms = rooms_for_broadcast.write().await;
                                            *rooms = result.rooms;
                                        }

                                        let mut existing = state_clone
                                            .connected_users
                                            .entry(result.user_id.clone())
                                            .or_insert_with(Vec::new);
                                        existing.push(tx.clone());
                                    }
                                    Err(e) => {
                                        let invalid =
                                            Payload::with_data(9, serde_json::json!(false));
                                        let _ = tx.send(serde_json::to_string(&invalid).unwrap());
                                        tracing::error!("Identify failed: {:?}", e);
                                        break;
                                    }
                                }
                            }
                            3 => {
                                tracing::info!(
                                    "Received PRESENCE_UPDATE: user_id={:?}, data={:?}",
                                    user_id,
                                    payload.d
                                );
                                if let Some(ref uid) = user_id {
                                    state_clone
                                        .gateway_metrics
                                        .record_dispatch("PRESENCE_UPDATE", Some(uid));
                                    events::presence::handle(
                                        &state_clone,
                                        uid,
                                        &subscribed_rooms,
                                        payload.d,
                                    )
                                    .await;
                                } else {
                                    tracing::warn!(
                                        "PRESENCE_UPDATE received but user not identified yet"
                                    );
                                }
                            }
                            _ => {
                                tracing::warn!("Unknown opcode: {}", payload.op);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to parse payload: {:?}", e);
                    }
                }
            }
            Message::Close(_) => {
                break;
            }
            _ => {}
        }
    }

    if let Some(uid) = user_id {
        let still_connected = if let Some(mut entry) = state.connected_users.get_mut(&uid) {
            entry.retain(|s| !s.same_channel(&tx));
            !entry.is_empty()
        } else {
            false
        };
        if let Some(participant) = state.voice.leave_user(&uid) {
            let voice_event = Payload::dispatch(
                "VOICE_STATE_UPDATE",
                serde_json::json!({
                    "guildId": participant.guild_id,
                    "channelId": participant.channel_id,
                    "userId": participant.user_id,
                    "voiceState": null
                }),
            );

            if let Ok(serialized) = serde_json::to_string(&voice_event) {
                state.broadcast_to_room(&participant.guild_id, serialized);
            }
        }

        let members = if !still_connected {
            state
                .db
                .guild_members
                .find_many(|q| q.where_user_id(uid.clone()))
                .await
        } else {
            Ok(Vec::new())
        };

        if let Ok(memberships) = members {
            let presence = Payload::dispatch(
                "PRESENCE_UPDATE",
                serde_json::json!({
                    "userId": uid,
                    "status": "UNAVAILABLE"
                }),
            );
            let serialized = serde_json::to_string(&presence).unwrap();

            let mut recipients: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for membership in memberships {
                let guild_members = state
                    .db
                    .guild_members
                    .find_many(|q| q.where_guild_id(membership.guild_id.clone()))
                    .await;
                if let Ok(gms) = guild_members {
                    for gm in gms {
                        if gm.user_id == uid {
                            continue;
                        }
                        recipients.insert(gm.user_id);
                    }
                }
            }

            for rid in recipients {
                state.send_to_user(&rid, serialized.clone());
            }
        }
    }

    state.gateway_metrics.unsubscribe_rooms(&subscribed_rooms);
    state.gateway_metrics.socket_disconnected();

    broadcast_task.abort();
    send_task.abort();
}
