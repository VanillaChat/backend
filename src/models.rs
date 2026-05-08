use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    pub tag: String,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub created_at: DateTime<Utc>,
    pub bot: bool,
    pub status: String,
    pub flags: i32,
    pub bio: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub email: String,
    pub email_verified: bool,
    pub locale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSettings {
    pub theme: String,
    pub compact_mode: Option<bool>,
    pub compact_show_avatars: Option<bool>,
    pub pending_deletion: Option<bool>,
    pub delete_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Guild {
    pub id: String,
    pub name: String,
    pub brief: String,
    pub icon: Option<String>,
    pub owner_id: String,
    #[serde(with = "chrono::serde::ts_milliseconds_option")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<Channel>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<GuildMember>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuildMember {
    pub id: i32,
    pub guild_id: String,
    pub user_id: String,
    pub nickname: Option<String>,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub joined_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub guild_id: String,
    #[serde(with = "chrono::serde::ts_milliseconds_option")]
    pub created_at: Option<DateTime<Utc>>,
    pub rate_limit_per_user: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub author_id: String,
    pub channel_id: String,
    pub guild_id: String,
    pub content: Option<String>,
    #[serde(with = "chrono::serde::ts_milliseconds_option")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(with = "chrono::serde::ts_milliseconds_option")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(rename = "type")]
    pub message_type: String,
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<MessageAuthor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageAuthor {
    pub id: String,
    pub username: String,
    pub tag: String,
    pub avatar: Option<String>,
    pub bot: bool,
    pub status: String,
    pub flags: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<MemberInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub nickname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub id: String,
    pub guild_id: String,
    pub code: String,
    pub uses: i32,
    pub max_uses: i32,
    pub creator_id: Option<String>,
    pub channel_id: String,
    pub vanity: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteResponse {
    #[serde(rename = "type")]
    pub invite_type: i32,
    pub code: String,
    pub inviter: Option<User>,
    pub guild: InviteGuild,
    pub guild_id: String,
    pub channel: InviteChannel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteGuild {
    pub id: String,
    pub name: String,
    pub brief: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteChannel {
    pub id: String,
    #[serde(rename = "type")]
    pub channel_type: i32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub id: String,
    pub status: String,
}

pub struct UserFlags;

impl UserFlags {
    pub const ADMIN: i32 = 1 << 0;
}

impl From<byteorm_client::Users> for User {
    fn from(u: byteorm_client::Users) -> Self {
        Self {
            id: u.id,
            username: u.username,
            tag: u.tag,
            created_at: u.created_at,
            bot: u.bot,
            status: u.status.to_string(),
            flags: u.flags,
            bio: u.bio,
            avatar: u.avatar,
            banner: u.banner,
        }
    }
}

impl From<byteorm_client::Guilds> for Guild {
    fn from(g: byteorm_client::Guilds) -> Self {
        Self {
            id: g.id,
            name: g.name,
            brief: g.brief,
            icon: g.icon,
            owner_id: g.owner_id,
            created_at: Some(g.created_at),
            channels: None,
            members: None,
        }
    }
}

impl From<byteorm_client::Channels> for Channel {
    fn from(c: byteorm_client::Channels) -> Self {
        Self {
            id: c.id,
            name: c.name,
            guild_id: c.guild_id,
            created_at: Some(c.created_at),
            rate_limit_per_user: c.rate_limit_per_user,
        }
    }
}

impl From<byteorm_client::GuildMembers> for GuildMember {
    fn from(m: byteorm_client::GuildMembers) -> Self {
        Self {
            id: m.id,
            guild_id: m.guild_id,
            user_id: m.user_id,
            nickname: m.nickname,
            joined_at: m.joined_at,
            user: None,
        }
    }
}

impl From<byteorm_client::Messages> for Message {
    fn from(m: byteorm_client::Messages) -> Self {
        Self {
            id: m.id,
            author_id: m.author_id,
            channel_id: m.channel_id,
            guild_id: m.guild_id,
            content: m.content,
            created_at: Some(m.created_at),
            updated_at: m.updated_at,
            message_type: m.message_type.to_string(),
            nonce: Some(m.nonce),
            author: None,
        }
    }
}
