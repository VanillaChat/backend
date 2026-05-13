use crate::auth::token::verify_token;
use crate::error::AppError;
use crate::state::SharedState;
use crate::voice::{VoiceParticipant, VoiceQuality, now_ms};
use axum::{
    Json, Router,
    extract::{Path, State},
    response::IntoResponse,
    routing::{get, post},
};
use axum_extra::extract::CookieJar;
use byteorm_client::ChannelType;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/channels/{id}/voice", get(get_voice_state))
        .route("/channels/{id}/voice/join", post(join_voice))
        .route("/channels/{id}/voice/leave", post(leave_voice))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinVoiceRequest {
    pub quality: Option<VoiceQuality>,
    pub self_mute: Option<bool>,
    pub self_deaf: Option<bool>,
    pub client_session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveVoiceRequest {
    pub client_session_id: Option<String>,
}

fn require_client_session_id(value: Option<String>) -> Result<String, AppError> {
    let value = value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .ok_or_else(|| AppError::BadRequest("voice.errors.clientSessionRequired".to_string()))?;

    Ok(value)
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

async fn get_channel_and_member(
    state: &SharedState,
    channel_id: &str,
    user_id: &str,
) -> Result<(byteorm_client::Channels, byteorm_client::GuildMembers), AppError> {
    let channel = state
        .db
        .channels
        .find_first(|q| q.where_id(channel_id.to_string()))
        .await?
        .ok_or(AppError::NotFound("channels.notFound".to_string()))?;

    let member = state
        .db
        .guild_members
        .find_first(|q| {
            q.where_user_id(user_id.to_string())
                .where_guild_id(channel.guild_id.clone())
        })
        .await?
        .ok_or(AppError::Unauthorized)?;

    Ok((channel, member))
}

fn ensure_voice_channel(channel: &byteorm_client::Channels) -> Result<(), AppError> {
    if channel.channel_type != ChannelType::VOICE {
        return Err(AppError::BadRequest(
            "voice.errors.channelIsNotVoice".to_string(),
        ));
    }

    Ok(())
}

async fn get_voice_state(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(channel_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    let (channel, _member) = get_channel_and_member(&state, &channel_id, &account.id).await?;
    ensure_voice_channel(&channel)?;

    Ok(Json(state.voice.channel_state(
        channel.guild_id.clone(),
        channel.id.clone(),
    )))
}

async fn join_voice(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(channel_id): Path<String>,
    Json(body): Json<JoinVoiceRequest>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    let (channel, _member) = get_channel_and_member(&state, &channel_id, &account.id).await?;
    ensure_voice_channel(&channel)?;

    let quality = body.quality.unwrap_or_default();

    let client_session_id = require_client_session_id(body.client_session_id)?;

    let participant = VoiceParticipant {
        user_id: account.id.clone(),
        guild_id: channel.guild_id.clone(),
        channel_id: channel.id.clone(),
        client_session_id,
        quality,
        audio_format: quality.audio_format(),
        self_mute: body.self_mute.unwrap_or(false),
        self_deaf: body.self_deaf.unwrap_or(false),
        joined_at: now_ms(),
    };

    let previous = state.voice.join(participant.clone());

    if let Some(previous) = previous {
        if previous.channel_id != participant.channel_id {
            let leave_event = json!({
                "op": 0,
                "t": "VOICE_STATE_UPDATE",
                "d": {
                    "guildId": previous.guild_id,
                    "channelId": previous.channel_id,
                    "userId": previous.user_id,
                    "voiceState": null
                }
            });

            state.broadcast_to_room(&previous.guild_id, leave_event.to_string());
        }
    }

    let join_event = json!({
        "op": 0,
        "t": "VOICE_STATE_UPDATE",
        "d": {
            "guildId": participant.guild_id,
            "channelId": participant.channel_id,
            "userId": participant.user_id,
            "voiceState": participant
        }
    });

    state.broadcast_to_room(&channel.guild_id, join_event.to_string());

    Ok(Json(state.voice.channel_state(
        channel.guild_id.clone(),
        channel.id.clone(),
    )))
}

async fn leave_voice(
    State(state): State<SharedState>,
    jar: CookieJar,
    Path(channel_id): Path<String>,
    Json(body): Json<LeaveVoiceRequest>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    let (channel, _member) = get_channel_and_member(&state, &channel_id, &account.id).await?;
    ensure_voice_channel(&channel)?;

    let client_session_id = require_client_session_id(body.client_session_id)?;

    if let Some(participant) =
        state
            .voice
            .leave_channel_session(&account.id, &channel.id, &client_session_id)
    {
        let leave_event = json!({
            "op": 0,
            "t": "VOICE_STATE_UPDATE",
            "d": {
                "guildId": participant.guild_id,
                "channelId": participant.channel_id,
                "userId": participant.user_id,
                "voiceState": null
            }
        });

        state.broadcast_to_room(&participant.guild_id, leave_event.to_string());
    }

    Ok(Json(state.voice.channel_state(
        channel.guild_id.clone(),
        channel.id.clone(),
    )))
}
