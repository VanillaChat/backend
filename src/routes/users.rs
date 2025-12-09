use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, patch, post},
    Json, Router,
};
use axum_extra::extract::CookieJar;
use serde::{Deserialize};
use serde_json::json;

use crate::auth::token::verify_token;
use crate::error::AppError;
use crate::models::{User};
use crate::state::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/@me", patch(update_user))
        .route("/@me/user-settings", patch(update_settings))
        .route("/@me/guilds/{guild_id}", delete(leave_guild))
        .route("/@me/request-deletion", post(request_deletion))
        .route("/@me/cancel-deletion", post(cancel_deletion))
}

async fn get_current_user(
    state: &SharedState,
    jar: &CookieJar,
) -> Result<byteorm_client::Accounts, AppError> {
    let is_production = std::env::var("NODE_ENV").unwrap_or_default() == "production";
    let cookie_name = if is_production { "__Host-Token" } else { "token" };
    
    let token = jar.get(cookie_name)
        .map(|c| c.value().to_string())
        .ok_or(AppError::Unauthorized)?;
    
    let _token_data = verify_token(&token, &state.config.token_secret)
        .ok_or(AppError::Unauthorized)?;
    
    state.db.accounts
        .find_first(|q| q.where_token(token))
        .await?
        .ok_or(AppError::Unauthorized)
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    username: Option<String>,
    tag: Option<String>,
    bio: Option<String>,
    avatar: Option<String>,
    banner: Option<String>,
    password: Option<String>,
}

async fn update_user(
    State(state): State<SharedState>,
    jar: CookieJar,
    Json(body): Json<UpdateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    
    state.db.users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?
        .ok_or(AppError::Unauthorized)?;
    
    if body.username.is_none() && body.tag.is_none() && body.bio.is_none() 
        && body.avatar.is_none() && body.banner.is_none() {
        return Err(AppError::BadRequest("At least one field is required".to_string()));
    }
    
    let client = state.db.get_client().await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;
    
    let mut updates = Vec::new();
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
    let mut param_idx = 1;
    
    if let Some(username) = &body.username {
        updates.push(format!("username = ${}", param_idx));
        params.push(Box::new(username.clone()));
        param_idx += 1;
    }
    
    if let Some(tag) = &body.tag {
        updates.push(format!("tag = ${}", param_idx));
        params.push(Box::new(tag.clone()));
        param_idx += 1;
    }
    
    if let Some(bio) = &body.bio {
        updates.push(format!("bio = ${}", param_idx));
        params.push(Box::new(bio.clone()));
        param_idx += 1;
    }
    
    if !updates.is_empty() {
        params.push(Box::new(account.id.clone()));
        let sql = format!(
            "UPDATE users SET {} WHERE id = ${}",
            updates.join(", "),
            param_idx
        );
        
        let params_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = 
            params.iter().map(|p| p.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
        
        client.execute(&sql, &params_refs).await
            .map_err(|e| AppError::InternalServerError(e.to_string()))?;
    }
    
    let updated_user = state.db.users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?
        .ok_or(AppError::InternalServerError("Failed to update user".to_string()))?;
    
    let members = state.db.guild_members
        .find_many(|q| q.where_user_id(account.id.clone()))
        .await?;
    
    let user_response: User = updated_user.into();
    
    for member in members {
        let broadcast = json!({
            "op": 0,
            "t": "USER_UPDATE",
            "d": {
                "guildId": member.guild_id,
                "user": user_response
            }
        });
        state.broadcast_to_room(&member.guild_id, broadcast.to_string());
    }
    
    Ok(Json(user_response))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsRequest {
    theme: Option<String>,
    compact_mode: Option<bool>,
    compact_show_avatars: Option<bool>,
}

async fn update_settings(
    State(state): State<SharedState>,
    jar: CookieJar,
    Json(body): Json<UpdateSettingsRequest>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;

    if body.theme.is_none() && body.compact_mode.is_none() && body.compact_show_avatars.is_none() {
        return Err(AppError::BadRequest("At least one field is required".to_string()));
    }

    let client = state.db.get_client().await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let mut updates = Vec::new();
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
    let mut param_idx = 1;

    if let Some(theme) = &body.theme {
        println!("{}", theme.clone());
        updates.push(format!("theme = ${}::text::theme", param_idx));
        params.push(Box::new(theme.clone()));
        param_idx += 1;
    }

    if let Some(compact_mode) = &body.compact_mode {
        updates.push(format!("compact_mode = ${}", param_idx));
        params.push(Box::new(compact_mode.clone()));
        param_idx += 1;
    }

    if let Some(compact_show_avatars) = &body.compact_show_avatars {
        updates.push(format!("compact_show_avatars = ${}", param_idx));
        params.push(Box::new(compact_show_avatars.clone()));
        param_idx += 1;
    }

    if !updates.is_empty() {
        params.push(Box::new(account.id.clone()));
        let sql = format!(
            "UPDATE accountsettings SET {} WHERE account_id = ${}",
            updates.join(", "),
            param_idx
        );

        let params_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
            params.iter().map(|p| p.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();

        client.execute(&sql, &params_refs).await
            .map_err(|e| {
                println!("Failed to update settings: {:?}", e);
                return AppError::InternalServerError(e.to_string());
            })?;
    }
    
    Ok(StatusCode::NO_CONTENT)
}

async fn leave_guild(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(guild_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    
    state.db.guild_members
        .find_first(|q| q
            .where_user_id(account.id.clone())
            .where_guild_id(guild_id.clone()))
        .await?
        .ok_or(AppError::NotFound("You are not a member of this guild.".to_string()))?;
    
    let guild = state.db.guilds
        .find_first(|q| q.where_id(guild_id.clone()))
        .await?
        .ok_or(AppError::NotFound("Guild not found".to_string()))?;
    
    if guild.owner_id == account.id {
        return Err(AppError::Forbidden(
            "You can't leave the server as its owner. Please transfer ownership or delete the server first.".to_string()
        ));
    }
    
    let user = state.db.users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?;
    
    state.db.guild_members
        .delete(|d| d
            .where_user_id(account.id.clone())
            .where_guild_id(guild_id.clone()))
        .await?;
    
    // if let Some(sockets) = state.connected_users.get(&account.id) {
    //     for socket in sockets.iter() {
    //     }
    // }
    
    if let Some(user) = user {
        let broadcast = json!({
            "op": 0,
            "t": "GUILD_MEMBER_REMOVE",
            "d": {
                "guildId": guild_id,
                "user": User::from(user)
            }
        });
        state.broadcast_to_room(&guild_id, broadcast.to_string());
    }
    
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDeletionRequest {
    password: String,
    delete_messages: bool,
}

async fn request_deletion(
    State(state): State<SharedState>,
    jar: CookieJar,
    Json(body): Json<RequestDeletionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    
    let parsed_hash = PasswordHash::new(&account.password)
        .map_err(|_| AppError::InternalServerError("Invalid password hash".to_string()))?;
    
    if Argon2::default().verify_password(body.password.as_bytes(), &parsed_hash).is_err() {
        return Err(AppError::Unauthorized);
    }
    
    let delete_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64 + 7 * 24 * 60 * 60 * 1000;
    
    Ok(Json(json!({
        "deleteAt": delete_at
    })))
}

async fn cancel_deletion(
    State(state): State<SharedState>,
    jar: CookieJar,
) -> Result<impl IntoResponse, AppError> {
    let _account = get_current_user(&state, &jar).await?;
    
    Ok(StatusCode::NO_CONTENT)
}
