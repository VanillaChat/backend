use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, patch, post},
    Json, Router,
};
use axum_extra::extract::CookieJar;
use serde::{Deserialize};
use serde_json::json;
use std::{io::Cursor, path::PathBuf};

use byteorm_client::Theme;

use crate::auth::token::verify_token;
use crate::error::AppError;
use crate::models::{User};
use crate::state::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/@me", patch(update_user))
        .route("/@me/avatar", post(upload_avatar))
        .route("/@me/banner", post(upload_banner))
        .route("/@me/user-settings", patch(update_settings))
        .route("/@me/guilds/{guild_id}", delete(leave_guild))
        .route("/@me/request-deletion", post(request_deletion))
        .route("/@me/cancel-deletion", post(cancel_deletion))
}

#[derive(Clone, Copy)]
enum UserImageKind {
    Avatar,
    Banner,
}

impl UserImageKind {
    fn field_name(self) -> &'static str {
        match self {
            Self::Avatar => "avatar",
            Self::Banner => "banner",
        }
    }

    fn directory(self) -> &'static str {
        match self {
            Self::Avatar => "avatars",
            Self::Banner => "banners",
        }
    }

    fn max_size(self, state: &SharedState) -> usize {
        match self {
            Self::Avatar => state.config.max_avatar_size,
            Self::Banner => state.config.max_banner_size,
        }
    }
}

fn generate_snowflake() -> String {
    use rand::Rng;
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let epoch = 1420070400000u64;
    let timestamp = now - epoch;
    let random: u64 = rand::thread_rng().gen_range(0..4096);

    ((timestamp << 22) | random).to_string()
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
    avatar: Option<Option<String>>,
    banner: Option<Option<String>>,
    password: Option<String>,
}

async fn update_user(
    State(state): State<SharedState>,
    jar: CookieJar,
    Json(body): Json<UpdateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;
    
    let current_user = state.db.users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?
        .ok_or(AppError::Unauthorized)?;
    
    if body.username.is_none() && body.tag.is_none() && body.bio.is_none() 
        && body.avatar.is_none() && body.banner.is_none() {
        return Err(AppError::BadRequest("At least one field is required".to_string()));
    }
    
    let old_avatar = current_user.avatar.clone();
    let old_banner = current_user.banner.clone();

    let mut update_builder = state.db.users.update(|u| {
        let mut u = u.where_id(account.id.clone());
        if let Some(username) = &body.username {
            u = u.set_username(username.clone());
        }
        if let Some(tag) = &body.tag {
            u = u.set_tag(tag.clone());
        }
        if let Some(bio) = &body.bio {
            u = u.set_bio(Some(bio.clone()));
        }
        if let Some(avatar) = &body.avatar {
            u = u.set_avatar(avatar.clone());
        }
        if let Some(banner) = &body.banner {
            u = u.set_banner(banner.clone());
        }
        u
    });
    update_builder.await?;

    if matches!(body.avatar, Some(None)) {
        remove_user_image(UserImageKind::Avatar, &account.id, old_avatar).await;
    }

    if matches!(body.banner, Some(None)) {
        remove_user_image(UserImageKind::Banner, &account.id, old_banner).await;
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

async fn upload_avatar(
    State(state): State<SharedState>,
    jar: CookieJar,
    multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    upload_user_image(state, jar, multipart, UserImageKind::Avatar).await
}

async fn upload_banner(
    State(state): State<SharedState>,
    jar: CookieJar,
    multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    upload_user_image(state, jar, multipart, UserImageKind::Banner).await
}

async fn upload_user_image(
    state: SharedState,
    jar: CookieJar,
    mut multipart: Multipart,
    kind: UserImageKind,
) -> Result<impl IntoResponse, AppError> {
    let account = get_current_user(&state, &jar).await?;

    let current_user = state
        .db
        .users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?
        .ok_or(AppError::Unauthorized)?;

    let mut image_bytes = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let field_name = field.name().unwrap_or_default();
        if field_name != "image" && field_name != "file" {
            continue;
        }

        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        image_bytes = Some(bytes.to_vec());
        break;
    }

    let image_bytes = image_bytes.ok_or(AppError::BadRequest(format!(
        "Missing {} file",
        kind.field_name()
    )))?;

    if image_bytes.len() > kind.max_size(&state) {
        return Err(AppError::BadRequest(format!(
            "{} is too large",
            kind.field_name()
        )));
    }

    let converted = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let image = image::load_from_memory(&image_bytes).map_err(|e| e.to_string())?;
        let mut output = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut output), image::ImageFormat::WebP)
            .map_err(|e| e.to_string())?;
        Ok(output)
    })
    .await
    .map_err(|e| AppError::InternalServerError(e.to_string()))?
    .map_err(AppError::BadRequest)?;

    let image_id = generate_snowflake();
    let image_dir = PathBuf::from("./cdn")
        .join(kind.directory())
        .join(&account.id);
    tokio::fs::create_dir_all(&image_dir)
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let image_path = image_dir.join(format!("{image_id}.webp"));
    tokio::fs::write(&image_path, converted)
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let old_image = match kind {
        UserImageKind::Avatar => current_user.avatar,
        UserImageKind::Banner => current_user.banner,
    };

    let mut update = state.db.users.update(|u| {
        let mut u = u.where_id(account.id.clone());
        match kind {
            UserImageKind::Avatar => {
                u = u.set_avatar(Some(image_id.clone()));
            }
            UserImageKind::Banner => {
                u = u.set_banner(Some(image_id.clone()));
            }
        }
        u
    });
    update.await?;

    remove_user_image(kind, &account.id, old_image).await;

    let updated_user = state
        .db
        .users
        .find_first(|q| q.where_id(account.id.clone()))
        .await?
        .ok_or(AppError::InternalServerError("Failed to update user".to_string()))?;

    Ok(Json(User::from(updated_user)))
}

async fn remove_user_image(kind: UserImageKind, user_id: &str, image_id: Option<String>) {
    let Some(image_id) = image_id else {
        return;
    };

    if image_id.starts_with("data:")
        || image_id.starts_with("http://")
        || image_id.starts_with("https://")
        || image_id.starts_with("blob:")
    {
        return;
    }

    let image_path = PathBuf::from("./cdn")
        .join(kind.directory())
        .join(user_id)
        .join(format!("{image_id}.webp"));
    let _ = tokio::fs::remove_file(image_path).await;
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

    let theme = body
        .theme
        .as_deref()
        .map(str::parse::<Theme>)
        .transpose()
        .map_err(AppError::BadRequest)?;

    let mut update_needed = false;
    let update_future = state.db.account_settings.update(|u| {
        let mut u = u.where_account_id(account.id.clone());
        
        if let Some(theme) = theme {
             u = u.set_theme(theme);
             update_needed = true;
        }

        if let Some(compact_mode) = body.compact_mode {
            u = u.set_compact_mode(compact_mode);
            update_needed = true;
        }

        if let Some(compact_show_avatars) = body.compact_show_avatars {
            u = u.set_compact_show_avatars(compact_show_avatars);
            update_needed = true;
        }
        u
    });

    if update_needed {
        update_future.await.map_err(|e| AppError::InternalServerError(e.to_string()))?;
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
