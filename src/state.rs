use byteorm_client::Client;
use dashmap::DashMap;
use redis::aio::ConnectionManager;
use std::sync::Arc;
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
}

#[derive(Debug, Clone)]
pub struct GatewayBroadcast {
    pub room: String,
    pub message: String,
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let db = Client::new(&config.database_url).await?;
        
        let redis_client = redis::Client::open(config.redis_url.as_str())?;
        let redis = redis_client.get_connection_manager().await?;
        
        let (gateway_tx, _) = broadcast::channel(1024);
        
        Ok(Self {
            db,
            redis,
            config: Arc::new(config),
            connected_users: Arc::new(DashMap::new()),
            gateway_tx,
        })
    }
    
    pub fn broadcast_to_room(&self, room: &str, message: String) {
        let _ = self.gateway_tx.send(GatewayBroadcast {
            room: room.to_string(),
            message,
        });
    }
}
