use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VoiceQuality {
    Auto,
    VoiceLossless,
    StudioLossless,
    MusicLossless,
}

impl Default for VoiceQuality {
    fn default() -> Self {
        Self::VoiceLossless
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceAudioFormat {
    pub sample_rate: u32,
    pub bit_depth: u8,
    pub channels: u8,
    pub estimated_bitrate_bps: u32,
}

impl VoiceQuality {
    pub fn audio_format(self) -> VoiceAudioFormat {
        match self {
            VoiceQuality::Auto => VoiceAudioFormat {
                sample_rate: 48_000,
                bit_depth: 16,
                channels: 1,
                estimated_bitrate_bps: 768_000,
            },
            VoiceQuality::VoiceLossless => VoiceAudioFormat {
                sample_rate: 48_000,
                bit_depth: 16,
                channels: 1,
                estimated_bitrate_bps: 768_000,
            },
            VoiceQuality::StudioLossless => VoiceAudioFormat {
                sample_rate: 48_000,
                bit_depth: 24,
                channels: 1,
                estimated_bitrate_bps: 1_152_000,
            },
            VoiceQuality::MusicLossless => VoiceAudioFormat {
                sample_rate: 48_000,
                bit_depth: 16,
                channels: 2,
                estimated_bitrate_bps: 1_536_000,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceParticipant {
    pub user_id: String,
    pub guild_id: String,
    pub channel_id: String,
    pub client_session_id: String,
    pub quality: VoiceQuality,
    pub audio_format: VoiceAudioFormat,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub joined_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceChannelState {
    pub guild_id: String,
    pub channel_id: String,
    pub participants: Vec<VoiceParticipant>,
}

#[derive(Debug, Default)]
pub struct VoiceState {
    participants_by_user: DashMap<String, VoiceParticipant>,
}

impl VoiceState {
    pub fn join(&self, participant: VoiceParticipant) -> Option<VoiceParticipant> {
        self.participants_by_user
            .insert(participant.user_id.clone(), participant)
    }

    pub fn leave_user(&self, user_id: &str) -> Option<VoiceParticipant> {
        self.participants_by_user
            .remove(user_id)
            .map(|(_, participant)| participant)
    }

    pub fn leave_session(
        &self,
        user_id: &str,
        client_session_id: &str,
    ) -> Option<VoiceParticipant> {
        let current = self.participants_by_user.get(user_id)?;

        if current.client_session_id != client_session_id {
            return None;
        }

        drop(current);
        self.leave_user(user_id)
    }

    pub fn leave_channel_session(
        &self,
        user_id: &str,
        channel_id: &str,
        client_session_id: &str,
    ) -> Option<VoiceParticipant> {
        let current = self.participants_by_user.get(user_id)?;

        if current.channel_id != channel_id || current.client_session_id != client_session_id {
            return None;
        }

        drop(current);
        self.leave_user(user_id)
    }

    pub fn leave_channel(&self, user_id: &str, channel_id: &str) -> Option<VoiceParticipant> {
        let current = self.participants_by_user.get(user_id)?;
        if current.channel_id != channel_id {
            return None;
        }
        drop(current);

        self.leave_user(user_id)
    }

    pub fn channel_state(&self, guild_id: String, channel_id: String) -> VoiceChannelState {
        let participants = self
            .participants_by_user
            .iter()
            .filter(|entry| entry.guild_id == guild_id && entry.channel_id == channel_id)
            .map(|entry| entry.value().clone())
            .collect();

        VoiceChannelState {
            guild_id,
            channel_id,
            participants,
        }
    }

    pub fn participants_for_guilds(&self, guild_ids: &[String]) -> Vec<VoiceParticipant> {
        let guilds: HashSet<&str> = guild_ids.iter().map(String::as_str).collect();

        self.participants_by_user
            .iter()
            .filter(|entry| guilds.contains(entry.guild_id.as_str()))
            .map(|entry| entry.value().clone())
            .collect()
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
