use axum::{
    Json, Router,
    extract::{Path, State},
    response::IntoResponse,
    routing::{get, post},
};
use std::collections::{HashMap, HashSet};

use axum_extra::extract::CookieJar;
use serde_json::json;

use byteorm_client::UserStatus;

use crate::auth::token::verify_token;
use crate::error::AppError;
use crate::models::{Guild, GuildMember, InviteChannel, InviteGuild, InviteResponse, User};
use crate::state::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/{code}", get(get_invite))
        .route("/{code}", post(use_invite))
}

async fn get_current_user(
    state: &SharedState,
    jar: &CookieJar,
) -> Result<byteorm_client::Accounts, AppError> {
    let is_production = std::env::var("NODE_ENV").unwrap_or_default() == "production";
    let cookie_name = if is_production {
        "__Host-Token"
    } else {
        "token"
    };

    let token = jar
        .get(cookie_name)
        .map(|c| c.value().to_string())
        .ok_or(AppError::Unauthorized)?;

    let _token_data =
        verify_token(&token, &state.config.token_secret).ok_or(AppError::Unauthorized)?;

    state
        .db
        .accounts
        .find_first(|q| q.where_token(token))
        .await?
        .ok_or(AppError::Unauthorized)
}

async fn get_invite(
    State(state): State<SharedState>,
    Path(code): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let client = state
        .db
        .get_client()
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let sql = "SELECT i.*, g.id as guild_id, g.name as guild_name, g.brief as guild_brief, g.icon as guild_icon,
               c.id as channel_id, c.name as channel_name,
               u.id as creator_id, u.username as creator_username, u.tag as creator_tag, u.avatar as creator_avatar, u.bot as creator_bot, u.status as creator_status, u.flags as creator_flags
               FROM guildinvites i
               JOIN guilds g ON i.guild_id = g.id
               JOIN channels c ON i.channel_id = c.id
               LEFT JOIN users u ON i.creator_id = u.id
               WHERE i.code = $1";

    let rows = client
        .query(sql, &[&code])
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let row = rows
        .first()
        .ok_or(AppError::NotFound("Invite not found".to_string()))?;

    let creator_id: Option<String> = row.get("creator_id");
    let inviter = match (
        creator_id,
        row.try_get::<_, Option<String>>("creator_username")
            .ok()
            .flatten(),
    ) {
        (Some(id), Some(username)) => Some(User {
            id,
            username,
            tag: row
                .try_get::<_, Option<String>>("creator_tag")
                .ok()
                .flatten()
                .unwrap_or_default(),
            created_at: chrono::Utc::now(),
            bot: row
                .try_get::<_, Option<bool>>("creator_bot")
                .ok()
                .flatten()
                .unwrap_or(false),
            flags: row
                .try_get::<_, Option<i32>>("creator_flags")
                .ok()
                .flatten()
                .unwrap_or(0),
            bio: None,
            avatar: row
                .try_get::<_, Option<String>>("creator_avatar")
                .ok()
                .flatten(),
            banner: None,
        }),
        _ => None,
    };

    let response = InviteResponse {
        invite_type: 0,
        code: row.get("code"),
        inviter,
        guild: InviteGuild {
            id: row.get("guild_id"),
            name: row.get("guild_name"),
            brief: row.get("guild_brief"),
            icon: row.get("guild_icon"),
        },
        guild_id: row.get("guild_id"),
        channel: InviteChannel {
            id: row.get("channel_id"),
            channel_type: 0,
            name: row.get("channel_name"),
        },
    };

    Ok(Json(response))
}

async fn use_invite(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(code): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;

    let client = state
        .db
        .get_client()
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let sql = "SELECT guild_id FROM guildinvites WHERE code = $1";

    let rows = client
        .query(sql, &[&code])
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let row = rows
        .first()
        .ok_or(AppError::NotFound("servers.notFound".to_string()))?;

    let guild_id: String = row.get("guild_id");

    let existing_member = state
        .db
        .guild_members
        .find_first(|q| {
            q.where_user_id(account.id.clone())
                .where_guild_id(guild_id.clone())
        })
        .await?;

    if existing_member.is_some() {
        return Err(AppError::Conflict(crate::error::ApiError {
            code: "ALREADY_A_MEMBER".to_string(),
            path: Some("invite".to_string()),
            message: Some("errors.serverCreation.alreadyAMember".to_string()),
        }));
    }

    state
        .db
        .guild_members
        .create(|c| {
            c.set_guild_id(guild_id.clone())
                .set_user_id(account.id.clone())
        })
        .await?;

    let user = state
        .db
        .users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?
        .ok_or(AppError::Unauthorized)?;

    let broadcast = json!({
        "op": 0,
        "t": "GUILD_MEMBER_ADD",
        "d": {
            "userId": account.id,
            "guildId": guild_id,
            "user": User::from(user.clone()),
            "nickname": null
        }
    });
    state.broadcast_to_room(&guild_id, broadcast.to_string());

    if user.status != UserStatus::UNAVAILABLE {
        let presence_broadcast = json!({
            "op": 0,
            "t": "PRESENCE_UPDATE",
            "d": {
                "userId": user.id,
                "status": user.status.to_string()
            }
        });
        let serialized = presence_broadcast.to_string();

        let guild_member_rows = state
            .db
            .guild_members
            .find_many(|q| q.where_guild_id(guild_id.clone()))
            .await?;
        let mut recipients: HashSet<String> = HashSet::new();
        for gm in &guild_member_rows {
            if gm.user_id == account.id {
                continue;
            }
            recipients.insert(gm.user_id.clone());
        }
        for rid in recipients {
            state.send_to_user(&rid, serialized.clone());
        }
    }

    let guild = state
        .db
        .guilds
        .find_first(|q| q.where_id(guild_id.clone()))
        .await?
        .ok_or(AppError::InternalServerError("Guild not found".to_string()))?;

    let channels = state
        .db
        .channels
        .find_many(|q| q.where_guild_id(guild_id.clone()))
        .await?;

    let members = state
        .db
        .guild_members
        .find_many(|q| q.where_guild_id(guild_id.clone()))
        .await?;

    let mut guild_response: Guild = guild.into();
    guild_response.channels = Some(channels.into_iter().map(|c| c.into()).collect());

    let mut users_map: HashMap<String, User> = HashMap::new();
    let mut members_out: Vec<GuildMember> = Vec::new();
    for member in members {
        let member_user = state
            .db
            .users
            .find_first(|q| q.where_id(member.user_id.clone()))
            .await?;
        if let Some(u) = member_user {
            users_map
                .entry(u.id.clone())
                .or_insert_with(|| User::from(u));
        }
        members_out.push(member.into());
    }
    guild_response.members = Some(members_out);

    let users_array: Vec<&User> = users_map.values().collect();

    Ok(Json(json!({
        "code": code,
        "guild": guild_response,
        "users": users_array
    })))
}
