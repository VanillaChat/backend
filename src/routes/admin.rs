use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, post},
    Json, Router,
};
use axum_extra::extract::CookieJar;
use serde_json::json;

use crate::auth::token::verify_token;
use crate::error::AppError;
use crate::models::UserFlags;
use crate::state::SharedState;

fn generate_snowflake() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    
    let epoch = 1420070400000u64;
    let timestamp = now - epoch;
    
    use rand::Rng;
    let random: u64 = rand::thread_rng().gen_range(0..4096);
    
    let id = (timestamp << 22) | random;
    id.to_string()
}

fn generate_code() -> String {
    use rand::Rng;
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        .chars()
        .collect();
    let mut rng = rand::thread_rng();
    (0..8).map(|_| chars[rng.gen_range(0..chars.len())]).collect()
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/invite-codes", post(create_invite_code))
        .route("/invite-codes/{id}", delete(delete_invite_code))
}

async fn get_admin_user(
    state: &SharedState,
    jar: &CookieJar,
) -> Result<(byteorm_client::Accounts, byteorm_client::Users), AppError> {
    let is_production = std::env::var("NODE_ENV").unwrap_or_default() == "production";
    let cookie_name = if is_production { "__Host-Token" } else { "token" };
    
    let token = jar.get(cookie_name)
        .map(|c| c.value().to_string())
        .ok_or(AppError::Unauthorized)?;
    
    let _token_data = verify_token(&token, &state.config.token_secret)
        .ok_or(AppError::Unauthorized)?;
    
    let account = state.db.accounts
        .find_first(|q| q.where_token(token))
        .await?
        .ok_or(AppError::Unauthorized)?;
    
    let user = state.db.users
        .find_first(|q| q.where_id(account.user_id.clone()))
        .await?
        .ok_or(AppError::Unauthorized)?;
    
    if (user.flags & UserFlags::ADMIN) != UserFlags::ADMIN {
        return Err(AppError::Forbidden("Admin access required".to_string()));
    }
    
    Ok((account, user))
}

async fn create_invite_code(
    State(state): State<SharedState>,
    jar: CookieJar,
) -> Result<impl IntoResponse, AppError> {
    let (account, _user) = get_admin_user(&state, &jar).await?;
    
    let id = generate_snowflake();
    let code = generate_code();
    
    let client = state.db.get_client().await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;
    
    let sql = "INSERT INTO invite_codes (id, code, created_by, used, created_at) 
               VALUES ($1, $2, $3, $4, NOW()) RETURNING *";
    
    let rows = client.query(sql, &[&id, &code, &account.id, &false]).await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;
    
    if let Some(row) = rows.first() {
        Ok(Json(json!({
            "id": row.get::<_, String>("id"),
            "code": row.get::<_, String>("code"),
            "createdBy": row.get::<_, String>("created_by"),
            "used": row.get::<_, bool>("used"),
        })))
    } else {
        Err(AppError::InternalServerError("Failed to create invite code".to_string()))
    }
}

async fn delete_invite_code(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let (account, _user) = get_admin_user(&state, &jar).await?;
    
    let client = state.db.get_client().await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;
    
    let sql = "SELECT * FROM invite_codes WHERE id = $1";
    let rows = client.query(sql, &[&id]).await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;
    
    let row = rows.first()
        .ok_or(AppError::NotFound("Invite code not found".to_string()))?;
    
    let used: bool = row.get("used");
    if used {
        return Err(AppError::Forbidden("Cannot delete used invite code".to_string()));
    }
    
    let delete_sql = "DELETE FROM invite_codes WHERE id = $1";
    client.execute(delete_sql, &[&id]).await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;
    
    let broadcast = json!({
        "op": 0,
        "t": "INVITE_CODE_DELETE",
        "d": {
            "executor": account.id,
            "id": id
        }
    });
    state.broadcast_to_room("admins", broadcast.to_string());
    
    Ok(StatusCode::NO_CONTENT)
}
