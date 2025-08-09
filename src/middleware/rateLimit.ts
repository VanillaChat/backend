export default async function rateLimit(ip: string, limit: number = 100, windowMs: number, key: string) {
    const _key = `rate-limit:${ip}:${key}`;
    const count = await Bun.redis.incr(_key);

    if (count === 1) await Bun.redis.expire(_key, Math.trunc(windowMs / 1000));
    const ttl = await Bun.redis.ttl(_key);

    return {
        limited: count > limit,
        retryAfter: ttl
    }
}