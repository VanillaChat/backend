import * as crypto from "node:crypto";

export const generateToken = (userId: string, version: number) => {
    const randomBytes = crypto.randomBytes(16).toString('hex');
    const payload = `${userId}:${version}:${randomBytes}`;
    const signature = crypto.createHmac('sha256', Bun.env.TOKEN_SECRET!)
        .update(payload)
        .digest('hex');
    return Buffer.from(`${payload}:${signature}`).toString('base64');
};

export const verifyToken = (token: string) => {
    try {
        const decoded = Buffer.from(token, 'base64').toString();
        const [userId, _version, randomBytes, signature] = decoded.split(':');
        const version = parseInt(_version, 10);
        const payload = `${userId}:${version}:${randomBytes}`;
        const expectedSignature = crypto.createHmac('sha256', Bun.env.TOKEN_SECRET!)
            .update(payload)
            .digest('hex');
        if (signature === expectedSignature) {
            return { userId, version };
        }
        return null;
    } catch (error) {
        return null;
    }
};