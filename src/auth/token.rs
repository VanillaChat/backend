use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn generate_token(user_id: &str, version: i32, secret: &str) -> String {
    let mut random_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut random_bytes);
    let random_hex = hex::encode(random_bytes);

    let payload = format!("{}:{}:{}", user_id, version, random_hex);

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(payload.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());

    let full = format!("{}:{}", payload, signature);
    BASE64.encode(full.as_bytes())
}

pub fn verify_token(token: &str, secret: &str) -> Option<TokenData> {
    let decoded = BASE64.decode(token).ok()?;
    let decoded_str = String::from_utf8(decoded).ok()?;

    let parts: Vec<&str> = decoded_str.split(':').collect();
    if parts.len() != 4 {
        return None;
    }

    let user_id = parts[0].to_string();
    let version: i32 = parts[1].parse().ok()?;
    let random_bytes = parts[2];
    let signature = parts[3];

    let payload = format!("{}:{}:{}", user_id, version, random_bytes);

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload.as_bytes());
    let expected_signature = hex::encode(mac.finalize().into_bytes());

    if signature == expected_signature {
        Some(TokenData { user_id, version })
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct TokenData {
    pub user_id: String,
    pub version: i32,
}
