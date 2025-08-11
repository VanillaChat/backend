import {valkey} from "../index";

export default async function rateLimit(ip: string, limit: number = 100, windowMs: number, key: string) {
    const _key = `rate-limit:${ip}:${key}`;
    const count = await valkey.incr(_key);

    if (count === 1) await valkey.expire(_key, Math.trunc(windowMs / 1000));
    const ttl = await valkey.ttl(_key);

    return {
        limited: count > limit,
        retryAfter: ttl
    }
}