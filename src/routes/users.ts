import {Elysia} from "elysia";
import isAuthenticated from "../middleware/isAuthenticated";
import {db} from "../db";
import {users} from "../db/schema/user";
import {eq} from "drizzle-orm";
import sharp from "sharp";
import {mkdir, readdir, rmdir} from "fs/promises";
import {join} from "path";
import crypto from "node:crypto";
import rateLimit from "../middleware/rateLimit";

const AVATARS_DIR = join(import.meta.dir, '..', '..', '..', 'cdn', 'avatars');
await mkdir(AVATARS_DIR, { recursive: true }).catch(console.error);

export default new Elysia({prefix: '/users'})
    .guard({
            beforeHandle(ctx) {
                if (!isAuthenticated(ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token'])) return ctx.status('Unauthorized');
            }
        }, (_app) =>
            _app
                .group('/@me', (app) =>
                    app
                        .resolve(async (ctx) => {
                            const user = await db.query.accounts.findFirst({
                                where: (accounts, {eq}) => eq(accounts.token, ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token']!.value!),
                                with: {
                                    user: true
                                }
                            });

                            const servers = await db.query.guildMembers.findMany({
                                where: (members, {eq}) => eq(members.userId, user!.user.id)
                            });

                            return {user, guildIds: servers.map(g => g.guildId)};
                        })
                        .patch('/', async ({ user, guildIds, body, status, server }) => {
                            try {
                                const { username, tag, avatar: base64Avatar, password } = body as {
                                    username?: string;
                                    tag?: string;
                                    avatar?: string;
                                    password?: string;
                                };

                                if (!user?.user.bot && !base64Avatar) {
                                    if (!password) return status('Bad Request', {
                                        error: 'Password is required'
                                    });

                                    const isValidPassword = await Bun.password.verify(password, user!.password, 'bcrypt');
                                    if (!isValidPassword) return status('Unauthorized', {
                                        error: 'Invalid password'
                                    });
                                }

                                if (!username && !tag && !base64Avatar) {
                                    return status('Bad Request', {
                                        error: 'At least one field (username, tag, or avatar) is required'
                                    })
                                }

                                const updates: any = {};

                                if (username) updates.username = username;
                                if (tag) updates.tag = tag;

                                if (base64Avatar) {
                                    try {
                                        const base64Data = base64Avatar.split(';base64,').pop();
                                        if (!base64Data) {
                                            throw new Error('Invalid base64 data');
                                        }

                                        const buffer = Buffer.from(base64Data, 'base64');
                                        const maxSize = parseInt(Bun.env.MAX_AVATAR_SIZE || '10000000'); // Default 5MB
                                        
                                        if (buffer.length > maxSize) {
                                            return status('Payload Too Large', {
                                                error: `Avatar size exceeds maximum allowed size of ${maxSize / 1024 / 1024}MB`
                                            });
                                        }

                                        const processedBuffer = await sharp(buffer)
                                            .resize(256, 256, { 
                                                fit: 'cover', 
                                                withoutEnlargement: true 
                                            })
                                            .webp({ quality: 80 })
                                            .toBuffer();

                                        const hash = crypto.createHash('sha256')
                                            .update(processedBuffer)
                                            .digest('hex')
                                            .substring(0, 16);

                                        const userDir = join(AVATARS_DIR, user!.user.id);
                                        await mkdir(userDir, { recursive: true });

                                        if (user!.user.avatar) {
                                            const oldAvatarPath = join(process.cwd(), 'public', user!.user.avatar);
                                            try {
                                                await Bun.file(oldAvatarPath).unlink();

                                                try {
                                                    const files = await readdir(userDir);
                                                    if (files.length === 0) {
                                                        await rmdir(userDir);
                                                    }
                                                } catch (error) {
                                                    console.warn('Failed to check/remove user avatar directory:', error);
                                                }
                                            } catch (error) {
                                                console.warn('Failed to remove old avatar:', error);
                                            }
                                        }

                                        await Bun.write(join(userDir, `${hash}.webp`), processedBuffer);
                                        updates.avatar = hash;
                                    } catch (error) {
                                        console.error('Error processing avatar:', error);
                                        return status('Bad Request', {
                                            error: 'Failed to process avatar image'
                                        });
                                    }
                                }

                                if (Object.keys(updates).length > 0) {
                                    await db.update(users)
                                        .set(updates)
                                        .where(eq(users.id, user!.user.id));

                                    const [updatedUser] = await db.select()
                                        .from(users)
                                        .where(eq(users.id, user!.user.id))
                                        .limit(1);

                                    guildIds.forEach(guildId => {
                                        server!.publish(guildId, JSON.stringify({
                                            op: 0,
                                            t: 'USER_UPDATE',
                                            d: {
                                                guildId,
                                                user: updatedUser
                                            }
                                        }));
                                    });

                                    return updatedUser;
                                }

                                return user!.user;
                            } catch (error) {
                                console.error('Error updating user:', error);
                                return status('Internal Server Error', {
                                    error: 'Internal server error'
                                });
                            }
                        }, {
                            async beforeHandle(ctx) {
                                const { limited, retryAfter } = await rateLimit(
                                    ctx.server!.requestIP(ctx.request)!.address,
                                    2,
                                    3_600_000,
                                    `profile-change:${ctx.user!.id}`
                                );
                                if (limited) return ctx.status('Too Many Requests', {
                                    message: 'You are being rate limited.',
                                    retryAfter
                                });
                            }
                        })

                )
    )