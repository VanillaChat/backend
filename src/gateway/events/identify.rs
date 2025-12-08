use serde_json::json;
use tokio::sync::mpsc;

use crate::auth::token::verify_token;
use crate::gateway::{Payload, WsSender};
use crate::models::{Account, AccountSettings, Channel, Guild, GuildMember, Presence, User, UserFlags};
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
    let token = token.ok_or("No token provided")?;
    
    let token_data = verify_token(&token, token_secret)
        .ok_or("Invalid token")?;
    
    let account = state.db.accounts
        .find_first(|q| q.where_id(token_data.user_id.clone()))
        .await?
        .ok_or("Account not found")?;
    
    let user = state.db.users
        .find_first(|q| q.where_id(account.user_id.clone()))
        .await?
        .ok_or("User not found")?;
    
    let members = state.db.guild_members
        .find_many(|q| q.where_user_id(account.id.clone()))
        .await?;
    
    let mut guilds = Vec::new();
    let mut rooms = Vec::new();
    let mut presences = Vec::new();
    
    for member in &members {
        rooms.push(member.guild_id.clone());
        
        let guild = state.db.guilds
            .find_first(|q| q.where_id(member.guild_id.clone()))
            .await?;
        
        if let Some(guild) = guild {
            let channels = state.db.channels
                .find_many(|q| q.where_guild_id(guild.id.clone()))
                .await?;
            
            let guild_members = state.db.guild_members
                .find_many(|q| q.where_guild_id(guild.id.clone()))
                .await?;
            
            let mut members_with_users = Vec::new();
            for gm in guild_members {
                let member_user = state.db.users
                    .find_first(|q| q.where_id(gm.user_id.clone()))
                    .await?;
                
                if let Some(u) = member_user {
                    if state.connected_users.contains_key(&u.id) {
                        presences.push(Presence {
                            id: u.id.clone(),
                            status: u.status.clone(),
                        });
                    }
                    
                    members_with_users.push(GuildMember {
                        id: gm.id,
                        guild_id: gm.guild_id,
                        user_id: gm.user_id,
                        nickname: gm.nickname,
                        joined_at: gm.joined_at,
                        user: Some(u.into()),
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
                members: Some(members_with_users),
            });
        }
    }
    
    if (user.flags & UserFlags::ADMIN) == UserFlags::ADMIN {
        rooms.push("admins".to_string());
    }
    
    presences.push(Presence {
        id: account.id.clone(),
        status: user.status.clone(),
    });
    
    let ready = Payload::dispatch("READY", json!({
        "account": {
            "id": account.id,
            "emailVerified": account.email_verified,
            "locale": account.locale,
            "email": account.email
        },
        "settings": {
            "theme": "LIGHT",
            "compactMode": false,
            "compactShowAvatars": true
        },
        "appSettings": null,
        "user": User::from(user.clone()),
        "guilds": guilds,
        "presences": presences
    }));
    
    let _ = tx.send(serde_json::to_string(&ready)?);
    
    if user.status != "UNAVAILABLE" {
        for member in &members {
            let presence = Payload::dispatch("PRESENCE_UPDATE", json!({
                "userId": user.id,
                "status": user.status
            }));
            state.broadcast_to_room(&member.guild_id, serde_json::to_string(&presence)?);
        }
    }
    
    Ok(IdentifyResult {
        user_id: account.id.clone(),
        rooms,
    })
}
