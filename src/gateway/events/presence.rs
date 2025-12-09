use serde_json::json;

use crate::gateway::Payload;
use crate::state::SharedState;

pub async fn handle(
    state: &SharedState,
    user_id: &str,
    rooms: &[String],
    data: Option<serde_json::Value>,
) {
    tracing::info!("PRESENCE HANDLE: user_id={}, data={:?}", user_id, data);
    
    let new_status = data
        .and_then(|d| d.get("status").cloned())
        .and_then(|s| s.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "ONLINE".to_string());
    
    tracing::info!("PRESENCE: new_status={}", new_status);
    
    let user = match state.db.users
        .find_first(|q| q.where_id(user_id.to_string()))
        .await
    {
        Ok(Some(u)) => u,
        _ => return,
    };
    
    if user.status == new_status {
        return;
    }
    
    let result = state.db.users.update(|u| u
            .where_id(user_id.to_string())
            .set_status(new_status.clone())
        ).await.map(|_| 1);
        
    if let Err(e) = result {
        tracing::error!("Failed to update presence in DB: {:?}", e);
    } else {
        tracing::info!("Presence updated in DB for user {}", user_id);
    }
    
    for room in rooms {
        if room == "admins" {
            continue;
        }
        
        let presence = Payload::dispatch("PRESENCE_UPDATE", json!({
            "userId": user_id,
            "status": new_status
        }));
        
        state.broadcast_to_room(room, serde_json::to_string(&presence).unwrap());
    }
}
