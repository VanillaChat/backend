use serde_json::json;

use crate::gateway::Payload;
use crate::state::SharedState;

pub async fn handle(
    state: &SharedState,
    user_id: &str,
    rooms: &[String],
    data: Option<serde_json::Value>,
) {
    let new_status = data
        .and_then(|d| d.get("status").cloned())
        .and_then(|s| s.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "ONLINE".to_string());
    
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
    
    let client = match state.db.get_client().await {
        Ok(c) => c,
        Err(_) => return,
    };
    
    let _ = client
        .execute(
            "UPDATE users SET status = $1 WHERE id = $2",
            &[&new_status, &user_id],
        )
        .await;
    
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
