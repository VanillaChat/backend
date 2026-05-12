use crate::voice::VoiceState;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub database_url: String,
    pub redis_url: String,
    pub port: u16,
    pub token_secret: String,
    pub registration_closed: bool,
    pub verbose: bool,
    pub user_guild_limit: u32,
    pub max_avatar_size: usize,
    pub max_banner_size: usize,
    pub cors_enabled: bool,
    pub cors_allowed_origins: Vec<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Self {
            database_url: std::env::var("DATABASE_URL")?,
            redis_url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://127.0.0.1".to_string()),
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()?,
            token_secret: std::env::var("TOKEN_SECRET")?,
            registration_closed: std::env::var("REGISTRATION_CLOSED")
                .unwrap_or_else(|_| "0".to_string())
                == "1",
            verbose: std::env::var("VERBOSE").unwrap_or_else(|_| "0".to_string()) == "1",
            user_guild_limit: std::env::var("USER_GUILD_LIMIT")
                .unwrap_or_else(|_| "100".to_string())
                .parse()?,
            max_avatar_size: std::env::var("MAX_AVATAR_SIZE")
                .unwrap_or_else(|_| "10000000".to_string())
                .parse()?,
            max_banner_size: std::env::var("MAX_BANNER_SIZE")
                .unwrap_or_else(|_| "25000000".to_string())
                .parse()?,
            cors_enabled: std::env::var("CORS_ENABLED").unwrap_or_else(|_| "1".to_string()) == "1",
            cors_allowed_origins: std::env::var("CORS_ALLOWED_ORIGINS")
                .unwrap_or_else(|_| {
                    [
                        "http://localhost:5173",
                        "http://127.0.0.1:5173",
                        "https://vanilla.meetz.li",
                        "http://vanilla.meetz.li",
                        "https://svanilla.meetz.li",
                        "http://svanilla.meetz.li",
                    ]
                    .join(",")
                })
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        })
    }
}
