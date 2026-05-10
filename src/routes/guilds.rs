use axum::{
    extract::State
    ,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use axum::extract::Path;
use axum_extra::extract::CookieJar;
use futures_util::TryFutureExt;
use serde::Deserialize;
use serde_json::json;

use crate::auth::token::verify_token;
use crate::error::{AppError, FieldError};
use crate::models::{Channel, Guild, User};
use crate::state::SharedState;

#[derive(Debug, Deserialize)]
pub struct CreateGuildRequest {
    name: String,
    brief: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateGuildChannelRequest {
    name: String,
    rate_limit_per_user: Option<i32>,
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

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/", post(create_guild))
        .route("/{id}/channels", post(create_guild_channel))
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

async fn create_guild(
    State(state): State<SharedState>,
    jar: CookieJar,
    Json(body): Json<CreateGuildRequest>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    
    let mut errors = Vec::new();
    
    if body.name.len() < 2 {
        errors.push(FieldError {
            code: "modals.serverCreate.nameMinChars".to_string(),
            path: "name".to_string(),
        });
    }
    if body.name.len() > 64 {
        errors.push(FieldError {
            code: "modals.serverCreate.nameMaxChars".to_string(),
            path: "name".to_string(),
        });
    }

    if let Some(brief) = &body.brief {
        if brief.len() < 2 {
            errors.push(FieldError {
                code: "modals.serverCreate.briefMinChars".to_string(),
                path: "brief".to_string(),
            });
        }
        if brief.len() > 36 {
            errors.push(FieldError {
                code: "modals.serverCreate.briefMaxChars".to_string(),
                path: "brief".to_string(),
            });
        }
    }

    if !errors.is_empty() {
        return Err(AppError::ValidationFailed(errors));
    }
    
    let member_count = state.db.guild_members
        .count(|q| q.where_user_id(account.id.clone()))
        .await?;
    
    if member_count >= state.config.user_guild_limit as i64 {
        return Err(AppError::Forbidden("app.modals.serverCreate.serverLimitExceeded".to_string()));
    }
    
    let guild_id = generate_snowflake();
    let channel_id = generate_snowflake();
    
    state.db.guilds
        .create(|c| c
            .set_id(guild_id.clone())
            .set_name(body.name)
            .set_brief(body.brief)
            .set_owner_id(account.id.clone())
        )
        .await?;
    
    state.db.guild_members
        .create(|c| c
            .set_guild_id(guild_id.clone())
            .set_user_id(account.id.clone())
        )
        .await?;
    
    state.db.channels
        .create(|c| c
            .set_id(channel_id.clone())
            .set_guild_id(guild_id.clone())
            .set_name("General".to_string())
            .set_rate_limit_per_user(0)
        )
        .await?;
    
    let guild = state.db.guilds
        .find_first(|q| q.where_id(guild_id.clone()))
        .await?
        .ok_or(AppError::InternalServerError("Failed to create guild".to_string()))?;
    
    let channel = state.db.channels
        .find_first(|q| q.where_id(channel_id))
        .await?
        .ok_or(AppError::InternalServerError("Failed to create channel".to_string()))?;

    let owner = state.db.users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?
        .ok_or(AppError::InternalServerError("Owner user not found".to_string()))?;

    #[derive(serde::Serialize)]
    struct CreateGuildResponse {
        guild: Guild,
        channels: Vec<Channel>,
        users: Vec<User>,
    }

    Ok(Json(CreateGuildResponse {
        guild: guild.into(),
        channels: vec![channel.into()],
        users: vec![User::from(owner)],
    }))
}

async fn create_guild_channel(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(guild_id): Path<String>,
    Json(body): Json<CreateGuildChannelRequest>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;

    let guild = state.db.guilds
        .find_first(|q| q.where_id(guild_id.clone()))
        .await?
        .ok_or(AppError::NotFound("servers.notFound".to_string()))?;

    if guild.owner_id != account.id {
        return Err(AppError::Forbidden("servers.notOwner".to_string()));
    }

    let mut errors = Vec::new();

    let name = body.name.trim().to_string();
    if name.len() < 1 {
        errors.push(FieldError {
            code: "modals.channelCreate.nameMinChars".to_string(),
            path: "name".to_string(),
        });
    }
    if name.len() > 100 {
        errors.push(FieldError {
            code: "modals.channelCreate.nameMaxChars".to_string(),
            path: "name".to_string(),
        });
    }

    let rate_limit = body.rate_limit_per_user.unwrap_or(0);
    if !(0..=21600).contains(&rate_limit) {
        errors.push(FieldError {
            code: "modals.channelCreate.rateLimitOutOfRange".to_string(),
            path: "rate_limit_per_user".to_string(),
        });
    }

    if !errors.is_empty() {
        return Err(AppError::ValidationFailed(errors));
    }

    let channel_id = generate_snowflake();

    let channel = state.db.channels
        .create(|c| c
            .set_id(channel_id.clone())
            .set_guild_id(guild_id.clone())
            .set_name(name)
            .set_rate_limit_per_user(rate_limit)
        )
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let response: Channel = channel.into();

    let broadcast = json!({
        "op": 0,
        "t": "CHANNEL_CREATE",
        "d": response,
    });
    state.broadcast_to_room(&guild_id, broadcast.to_string());

    Ok(Json(response))
}
