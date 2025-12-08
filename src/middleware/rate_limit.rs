use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use crate::error::AppError;

pub async fn rate_limit(
    redis: &mut ConnectionManager,
    ip: &str,
    limit: i64,
    window_ms: i64,
    key: &str,
) -> Result<RateLimitResult, AppError> {
    let redis_key = format!("rate-limit:{}:{}", ip, key);
    
    let count: i64 = redis.incr(&redis_key, 1).await?;
    
    if count == 1 {
        let _: () = redis.expire(&redis_key, (window_ms / 1000) as i64).await?;
    }
    
    let ttl: i64 = redis.ttl(&redis_key).await?;
    
    Ok(RateLimitResult {
        limited: count > limit,
        retry_after: ttl,
    })
}

pub struct RateLimitResult {
    pub limited: bool,
    pub retry_after: i64,
}
