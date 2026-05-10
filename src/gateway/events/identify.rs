use std::collections::{HashMap, HashSet};

use serde_json::json;
use tokio::sync::mpsc;

use byteorm_client::UserStatus;

use crate::auth::token::verify_token;
use crate::gateway::{Payload, WsSender};
use crate::models::{
    Account, AccountSettings, Channel, Guild, GuildMember, Presence, User, UserFlags,
};
use crate::state::SharedState;

pub struct IdentifyResult {
    pub user_id: String,
    pub rooms: Vec<String>,
}

pub async fn handle(
    state: &SharedState,
    tx: &WsSender,
    token: Option<String>,
    token_secret: &str,
) -> Result<IdentifyResult, Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!("IDENTIFY: token={:?}", token);

    let token = token.ok_or("No token provided")?;

    tracing::info!("IDENTIFY: verifying token...");
    let token_data = verify_token(&token, token_secret).ok_or("Invalid token")?;

    tracing::info!("IDENTIFY: token verified, user_id={}", token_data.user_id);

    let account = state
        .db
        .accounts
        .find_first(|q| q.where_token(token))
        .await?
        .ok_or("Account not found")?;

    let user = state
        .db
        .users
        .find_first(|q| q.where_id(account.user_id.clone()))
        .await?
        .ok_or("User not found")?;

    let account_settings = state
        .db
        .account_settings
        .find_first(|q| q.where_account_id(account.id.clone()))
        .await?
        .ok_or("Account settings not found")?;

    let members = state
        .db
        .guild_members
        .find_many(|q| q.where_user_id(account.id.clone()))
        .await?;

    let mut guilds = Vec::new();
    let mut rooms = Vec::new();
    let mut presences = Vec::new();
    let mut seen_presences: HashSet<String> = HashSet::new();
    let mut users: HashMap<String, User> = HashMap::new();

    for member in &members {
        rooms.push(member.guild_id.clone());

        let guild = state
            .db
            .guilds
            .find_first(|q| q.where_id(member.guild_id.clone()))
            .await?;

        if let Some(guild) = guild {
            let channels = state
                .db
                .channels
                .find_many(|q| q.where_guild_id(guild.id.clone()))
                .await?;

            let guild_members = state
                .db
                .guild_members
                .find_many(|q| q.where_guild_id(guild.id.clone()))
                .await?;

            let mut guild_members_out = Vec::new();
            for gm in guild_members {
                let member_user = state
                    .db
                    .users
                    .find_first(|q| q.where_id(gm.user_id.clone()))
                    .await?;

                if let Some(u) = member_user {
                    if state.connected_users.contains_key(&u.id)
                        && seen_presences.insert(u.id.clone())
                    {
                        presences.push(Presence {
                            id: u.id.clone(),
                            status: u.status.to_string(),
                        });
                    }

                    users
                        .entry(u.id.clone())
                        .or_insert_with(|| User::from(u.clone()));

                    guild_members_out.push(GuildMember {
                        id: gm.id,
                        guild_id: gm.guild_id,
                        user_id: gm.user_id,
                        nickname: gm.nickname,
                        joined_at: gm.joined_at,
                    });
                }
            }

            guilds.push(Guild {
                id: guild.id,
                name: guild.name,
                brief: guild.brief,
                icon: guild.icon,
                owner_id: guild.owner_id,
                created_at: Some(guild.created_at),
                channels: Some(channels.into_iter().map(|c| c.into()).collect()),
                members: Some(guild_members_out),
            });
        }
    }

    if (user.flags & UserFlags::ADMIN) == UserFlags::ADMIN {
        rooms.push("admins".to_string());
    }

    if seen_presences.insert(account.id.clone()) {
        presences.push(Presence {
            id: account.id.clone(),
            status: user.status.to_string(),
        });
    }

    let self_user = User::from(user.clone());
    users
        .entry(account.id.clone())
        .or_insert_with(|| self_user.clone());

    let ready = Payload::dispatch(
        "READY",
        json!({
            "account": {
                "id": account.id,
                "emailVerified": account.email_verified,
                "locale": account.locale,
                "email": account.email
            },
            "settings": {
                "theme": account_settings.theme,
                "compactMode": account_settings.compact_mode,
                "compactShowAvatars": account_settings.compact_show_avatars
            },
            "appSettings": {
                "inviteCodes": []
            },
            "user": self_user,
            "guilds": guilds,
            "presences": presences,
            "users": users
        }),
    );

    let _ = tx.send(serde_json::to_string(&ready)?);

    if user.status != UserStatus::UNAVAILABLE {
        let presence = Payload::dispatch(
            "PRESENCE_UPDATE",
            json!({
                "userId": user.id,
                "status": user.status.to_string()
            }),
        );
        let serialized = serde_json::to_string(&presence)?;

        let mut recipients: HashSet<String> = HashSet::new();
        for member in &members {
            let guild_members = state
                .db
                .guild_members
                .find_many(|q| q.where_guild_id(member.guild_id.clone()))
                .await?;
            for gm in guild_members {
                if gm.user_id == account.id {
                    continue;
                }
                recipients.insert(gm.user_id);
            }
        }

        for rid in recipients {
            state.send_to_user(&rid, serialized.clone());
        }
    }

    Ok(IdentifyResult {
        user_id: account.id.clone(),
        rooms,
    })
}
