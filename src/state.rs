use anyhow::Context;
use byteorm_client::Client;
use dashmap::DashMap;
use redis::aio::ConnectionManager;
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;

use crate::config::Config;
use crate::gateway::WsSender;

pub type SharedState = Arc<AppState>;

#[derive(Clone)]
pub struct AppState {
    pub db: Client,
    pub redis: ConnectionManager,
    pub config: Arc<Config>,
    pub connected_users: Arc<DashMap<String, Vec<WsSender>>>,
    pub gateway_tx: broadcast::Sender<GatewayBroadcast>,
    pub gateway_metrics: Arc<GatewayMetrics>,
}

#[derive(Debug, Clone)]
pub struct GatewayBroadcast {
    pub room: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GatewayMetricEntry {
    pub key: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GatewayRecentEvent {
    pub at_ms: u128,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayMetricsSnapshot {
    pub connected_sockets: u64,
    pub identified_users: usize,
    pub subscribed_rooms: usize,
    pub total_inbound: u64,
    pub total_dispatches: u64,
    pub total_broadcasts: u64,
    pub inbound_by_op: Vec<GatewayMetricEntry>,
    pub dispatch_by_type: Vec<GatewayMetricEntry>,
    pub broadcasts_by_room: Vec<GatewayMetricEntry>,
    pub recent: Vec<GatewayRecentEvent>,
}

#[derive(Debug)]
pub struct GatewayMetrics {
    connected_sockets: AtomicU64,
    total_inbound: AtomicU64,
    total_dispatches: AtomicU64,
    total_broadcasts: AtomicU64,
    room_subscriptions: DashMap<String, u64>,
    inbound_by_op: DashMap<String, u64>,
    dispatch_by_type: DashMap<String, u64>,
    broadcasts_by_room: DashMap<String, u64>,
    recent: Mutex<VecDeque<GatewayRecentEvent>>,
}

impl GatewayMetrics {
    pub fn new() -> Self {
        Self {
            connected_sockets: AtomicU64::new(0),
            total_inbound: AtomicU64::new(0),
            total_dispatches: AtomicU64::new(0),
            total_broadcasts: AtomicU64::new(0),
            room_subscriptions: DashMap::new(),
            inbound_by_op: DashMap::new(),
            dispatch_by_type: DashMap::new(),
            broadcasts_by_room: DashMap::new(),
            recent: Mutex::new(VecDeque::with_capacity(256)),
        }
    }

    pub fn socket_connected(&self) {
        self.connected_sockets.fetch_add(1, Ordering::Relaxed);
        self.push_recent("socket", "connected");
    }

    pub fn socket_disconnected(&self) {
        self.connected_sockets
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
        self.push_recent("socket", "disconnected");
    }

    pub fn record_inbound(&self, op: i32, user_id: Option<&str>) {
        self.total_inbound.fetch_add(1, Ordering::Relaxed);
        let key = format!("op:{op}");
        Self::increment(&self.inbound_by_op, &key, 1);
        let detail = match user_id {
            Some(user_id) => format!("{key} from {user_id}"),
            None => key,
        };
        self.push_recent("inbound", detail);
    }

    pub fn record_dispatch(&self, event: &str, user_id: Option<&str>) {
        self.total_dispatches.fetch_add(1, Ordering::Relaxed);
        Self::increment(&self.dispatch_by_type, event, 1);
        let detail = match user_id {
            Some(user_id) => format!("{event} for {user_id}"),
            None => event.to_string(),
        };
        self.push_recent("dispatch", detail);
    }

    pub fn record_broadcast(&self, room: &str, message: &str) {
        self.total_broadcasts.fetch_add(1, Ordering::Relaxed);
        Self::increment(&self.broadcasts_by_room, room, 1);

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(message) {
            if let Some(event) = value.get("t").and_then(|value| value.as_str()) {
                self.record_dispatch(event, None);
            }
        }

        self.push_recent("broadcast", format!("{room}: {} bytes", message.len()));
    }

    pub fn subscribe_rooms(&self, rooms: &[String]) {
        for room in rooms {
            Self::increment(&self.room_subscriptions, room, 1);
        }
        self.push_recent("rooms", format!("subscribed {}", rooms.len()));
    }

    pub fn unsubscribe_rooms(&self, rooms: &[String]) {
        for room in rooms {
            if let Some(mut count) = self.room_subscriptions.get_mut(room) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    drop(count);
                    self.room_subscriptions.remove(room);
                }
            }
        }
        self.push_recent("rooms", format!("unsubscribed {}", rooms.len()));
    }

    pub fn snapshot(&self, identified_users: usize) -> GatewayMetricsSnapshot {
        GatewayMetricsSnapshot {
            connected_sockets: self.connected_sockets.load(Ordering::Relaxed),
            identified_users,
            subscribed_rooms: self.room_subscriptions.len(),
            total_inbound: self.total_inbound.load(Ordering::Relaxed),
            total_dispatches: self.total_dispatches.load(Ordering::Relaxed),
            total_broadcasts: self.total_broadcasts.load(Ordering::Relaxed),
            inbound_by_op: Self::top_entries(&self.inbound_by_op, 10),
            dispatch_by_type: Self::top_entries(&self.dispatch_by_type, 10),
            broadcasts_by_room: Self::top_entries(&self.broadcasts_by_room, 10),
            recent: self
                .recent
                .lock()
                .map(|events| events.iter().cloned().rev().take(50).collect())
                .unwrap_or_default(),
        }
    }

    fn increment(map: &DashMap<String, u64>, key: &str, amount: u64) {
        let mut entry = map.entry(key.to_string()).or_insert(0);
        *entry += amount;
    }

    fn top_entries(map: &DashMap<String, u64>, limit: usize) -> Vec<GatewayMetricEntry> {
        let mut entries = map
            .iter()
            .map(|entry| GatewayMetricEntry {
                key: entry.key().clone(),
                count: *entry.value(),
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
        entries.truncate(limit);
        entries
    }

    fn push_recent(&self, kind: impl Into<String>, detail: impl Into<String>) {
        if let Ok(mut events) = self.recent.lock() {
            if events.len() >= 256 {
                events.pop_front();
            }
            events.push_back(GatewayRecentEvent {
                at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_millis())
                    .unwrap_or_default(),
                kind: kind.into(),
                detail: detail.into(),
            });
        }
    }
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let db = Client::new(&config.database_url)
            .await
            .context("failed to connect to Postgres using DATABASE_URL")?;

        let redis_client =
            redis::Client::open(config.redis_url.as_str()).context("failed to parse REDIS_URL")?;
        let redis = redis_client
            .get_connection_manager()
            .await
            .with_context(|| {
                format!(
                    "failed to connect to Redis/Valkey at REDIS_URL='{}'",
                    config.redis_url
                )
            })?;

        let (gateway_tx, _) = broadcast::channel(1024);

        Ok(Self {
            db,
            redis,
            config: Arc::new(config),
            connected_users: Arc::new(DashMap::new()),
            gateway_tx,
            gateway_metrics: Arc::new(GatewayMetrics::new()),
        })
    }

    pub fn broadcast_to_room(&self, room: &str, message: String) {
        self.gateway_metrics.record_broadcast(room, &message);
        let _ = self.gateway_tx.send(GatewayBroadcast {
            room: room.to_string(),
            message,
        });
    }

    pub fn send_to_user(&self, user_id: &str, message: String) {
        if let Some(senders) = self.connected_users.get(user_id) {
            for tx in senders.iter() {
                let _ = tx.send(message.clone());
            }
        }
    }

    pub async fn guild_owner(&self, guild_id: &str) -> Result<Option<String>, crate::error::AppError> {
        use redis::AsyncCommands;
        let mut redis = self.redis.clone();
        let cache_key = format!("guild:{}:owner", guild_id);

        let cached: Option<String> = redis.get(&cache_key).await.ok();
        if let Some(owner) = cached {
            return Ok(Some(owner));
        }

        let guild = self
            .db
            .guilds
            .find_first(|q| q.where_id(guild_id.to_string()))
            .await?;

        if let Some(g) = guild {
            let _: Result<(), _> = redis.set_ex(&cache_key, &g.owner_id, 300).await;
            Ok(Some(g.owner_id))
        } else {
            Ok(None)
        }
    }

    pub async fn invalidate_guild_owner(&self, guild_id: &str) {
        use redis::AsyncCommands;
        let mut redis = self.redis.clone();
        let cache_key = format!("guild:{}:owner", guild_id);
        let _: Result<(), _> = redis.del(&cache_key).await;
    }
}
