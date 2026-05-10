use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::json;

use byteorm_client::MessageType;

use crate::auth::token::verify_token;
use crate::error::AppError;
use crate::middleware::rate_limit;
use crate::models::{Invite, Message, User};
use crate::state::SharedState;

#[derive(Debug, Deserialize)]
pub struct MessageCreateRequest {
    content: String,
    nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MessageUpdateRequest {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MessagesQuery {
    before: Option<String>,
    after: Option<String>,
    limit: Option<i64>,
    around: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InviteCreateRequest {
    max_uses: Option<i32>,
    max_age: Option<i32>,
}

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
    (0..6).map(|_| chars[rng.gen_range(0..chars.len())]).collect()
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/{id}/messages", get(get_messages))
        .route("/{id}/messages", post(create_message))
        .route("/{id}/messages/{message_id}", patch(update_message))
        .route("/{id}/messages/{message_id}", delete(delete_message))
        .route("/{id}/typing", post(typing_start))
        .route("/{id}/invites", post(create_invite))
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

async fn get_channel_and_member(
    state: &SharedState,
    channel_id: &str,
    user_id: &str,
) -> Result<(byteorm_client::Channels, byteorm_client::GuildMembers), AppError> {
    let channel = state.db.channels
        .find_first(|q| q.where_id(channel_id.to_string()))
        .await?
        .ok_or(AppError::NotFound("messages.errors.channelNotFound".to_string()))?;

    let member = state.db.guild_members
        .find_first(|q| q
            .where_user_id(user_id.to_string())
            .where_guild_id(channel.guild_id.clone()))
        .await?
        .ok_or(AppError::Unauthorized)?;

    Ok((channel, member))
}

async fn get_messages(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(channel_id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    let (_channel, _member) = get_channel_and_member(&state, &channel_id, &account.id).await?;
    
    let limit = query.limit.unwrap_or(50).min(100).max(1) as usize;
    
    let mut builder = state.db.messages.query()
        .where_channel_id(channel_id)
        .include_users()
        .order_by_created_at_desc()
        .limit(limit);

    if let Some(before) = &query.before {
        builder = builder.where_id_lt(before.clone());
    } else if let Some(after) = &query.after {
        builder = builder.where_id_gt(after.clone());
    }
    
    let messages_json = builder.find_many_json().await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;
    
    let mut messages: Vec<Message> = messages_json.into_iter().map(|value| {
        let msg_model: byteorm_client::Messages = serde_json::from_value(value.clone())
             .expect("Failed to deserialize message");
        
        let user_json = value.get("users").expect("Missing users join");
        let user_model: byteorm_client::Users = serde_json::from_value(user_json.clone())
             .expect("Failed to deserialize user");
             
        let mut msg: Message = msg_model.into();
        let user: User = user_model.into();

        msg.author = Some(user);

        msg
    }).collect();
    
    messages.reverse();
    
    Ok(Json(messages))
}

async fn create_message(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(channel_id): Path<String>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    let body: MessageCreateRequest = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("Invalid JSON".to_string()))?;
    let account = get_current_user(&state, &jar).await?;
    let (channel, member) = get_channel_and_member(&state, &channel_id, &account.id).await?;

    let user = state.db.users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?
        .ok_or(AppError::Unauthorized)?;

    if body.content.is_empty() || body.content.len() > 2000 {
        return Err(AppError::BadRequest("messages.errors.validationFailed".to_string()));
    }

    if channel.rate_limit_per_user > 0 {
        let owner_id = state.guild_owner(&member.guild_id).await?;
        let is_owner = owner_id.as_deref() == Some(account.id.as_str());

        if !is_owner {
            let mut redis = state.redis.clone();
            let key = format!("channel:{}:msg", channel_id);
            let result = rate_limit(
                &mut redis,
                &account.id,
                1,
                (channel.rate_limit_per_user as i64) * 1000,
                &key,
            ).await?;
            if result.limited {
                return Err(AppError::TooManyRequests {
                    message: "messages.errors.rateLimited".to_string(),
                    retry_after: result.retry_after,
                });
            }
        }
    }

    let message_id = generate_snowflake();
    let nonce = body.nonce.clone().unwrap_or_else(|| "0".to_string());
    
    let new_msg = state.db.messages.create(|c| c
        .set_id(message_id.clone())
        .set_author_id(account.id.clone())
        .set_channel_id(channel_id.clone())
        .set_guild_id(member.guild_id.clone())
        .set_content(Some(body.content.clone()))
        .set_message_type(MessageType::DEFAULT)
        .set_nonce(nonce.clone())
    ).await.map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let mut response: Message = new_msg.into();
    response.author = Some(User::from(user));
    
    let broadcast_message = json!({
        "op": 0,
        "t": "MESSAGE_CREATE",
        "d": response
    });
    
    state.broadcast_to_room(&member.guild_id, broadcast_message.to_string());
    
    Ok(Json(response))
}

async fn update_message(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path((channel_id, message_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    let body: MessageUpdateRequest = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("Invalid JSON".to_string()))?;
    let account = get_current_user(&state, &jar).await?;
    let (_channel, _member) = get_channel_and_member(&state, &channel_id, &account.id).await?;
    
    let message = state.db.messages
        .find_first(|q| q.where_id(message_id.clone()))
        .await?
        .ok_or(AppError::NotFound("messages.errors.messageNotFound".to_string()))?;
    
    if message.author_id != account.id {
        return Err(AppError::Forbidden("messages.errors.notYourMessage".to_string()));
    }
    
    if let Some(content) = &body.content {
        if message.content.as_ref() == Some(content) {
            let response: Message = message.into();
            return Ok(Json(response));
        }
        
        state.db.messages
            .update(|u| u
                .set_content(Some(content.clone()))
                .set_updated_at(Some(chrono::Utc::now()))
                .where_id(message_id.clone())
            )
            .await?;
    }
    
    let updated = state.db.messages
        .find_first(|q| q.where_id(message_id))
        .await?
        .ok_or(AppError::InternalServerError("Failed to update message".to_string()))?;

    let author = state.db.users
        .find_first(|q| q.where_id(updated.author_id.clone()))
        .await?;

    let mut response: Message = updated.clone().into();
    response.author = author.map(User::from);

    let broadcast_message = json!({
        "op": 0,
        "t": "MESSAGE_UPDATE",
        "d": response
    });

    state.broadcast_to_room(&updated.guild_id, broadcast_message.to_string());

    Ok(Json(response))
}

async fn delete_message(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path((channel_id, message_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    let (_channel, _member) = get_channel_and_member(&state, &channel_id, &account.id).await?;
    
    let message = state.db.messages
        .find_first(|q| q.where_id(message_id.clone()))
        .await?
        .ok_or(AppError::NotFound("messages.errors.messageNotFound".to_string()))?;
    
    if message.author_id != account.id {
        return Err(AppError::Forbidden("messages.errors.forbidden".to_string()));
    }
    
    state.db.messages
        .delete(|d| d.where_id(message_id.clone()))
        .await?;
    
    let broadcast_message = json!({
        "op": 0,
        "t": "MESSAGE_DELETE",
        "d": {
            "id": message.id,
            "channelId": message.channel_id,
            "guildId": message.guild_id
        }
    });
    
    state.broadcast_to_room(&message.guild_id, broadcast_message.to_string());
    
    Ok(StatusCode::NO_CONTENT)
}

async fn typing_start(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(channel_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    let (_channel, member) = get_channel_and_member(&state, &channel_id, &account.id).await?;
    
    let user = state.db.users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?
        .ok_or(AppError::Unauthorized)?;
    
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    
    let broadcast_message = json!({
        "op": 0,
        "t": "TYPING_START",
        "d": {
            "channelId": channel_id,
            "userId": account.id,
            "user": {
                "id": user.id,
                "username": user.username,
                "tag": user.tag,
                "avatar": user.avatar,
                "member": {
                    "nickname": member.nickname
                }
            },
            "timestamp": now,
            "expiresAt": now + 10000
        }
    });
    
    state.broadcast_to_room(&member.guild_id, broadcast_message.to_string());
    
    Ok(Json(json!({"success": true})))
}

async fn create_invite(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(channel_id): Path<String>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, AppError> {
    let body: InviteCreateRequest = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("Invalid JSON".to_string()))?;
    let account = get_current_user(&state, &jar).await?;
    let (_channel, member) = get_channel_and_member(&state, &channel_id, &account.id).await?;
    
    let code = generate_code();
    let invite_id = generate_snowflake();
    let max_uses = body.max_uses.unwrap_or(0);
    
    state.db.guild_invites
        .create(|c| c
            .set_id(invite_id.clone())
            .set_guild_id(member.guild_id.clone())
            .set_code(code.clone())
            .set_uses(0)
            .set_max_uses(max_uses)
            .set_creator_id(account.id.clone())
            .set_channel_id(channel_id.clone())
            .set_vanity(false)
        )
        .await?;
    
    let invite = state.db.guild_invites
        .find_first(|q| q.where_id(invite_id))
        .await?
        .ok_or(AppError::InternalServerError("Failed to create invite".to_string()))?;
    
    Ok(Json(Invite {
        id: invite.id,
        guild_id: invite.guild_id,
        code: invite.code,
        uses: invite.uses,
        max_uses: invite.max_uses,
        creator_id: Some(invite.creator_id),
        channel_id: invite.channel_id,
        vanity: invite.vanity,
    }))
}
