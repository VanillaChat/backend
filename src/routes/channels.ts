import {Elysia} from "elysia";
import isAuthenticated from "../middleware/isAuthenticated";
import {db} from "../db";
import {messages} from "../db/schema/message";
import z from "zod/v4";
import {Snowflake} from "@theinternetfolks/snowflake";
import {and, eq, gt, InferSelectModel, lt} from "drizzle-orm";
import {invites} from "../db/schema/guild";
import * as randomstring from "randomstring";
import rateLimit from "../middleware/rateLimit";
import {ip} from "elysia-ip";

const messageCreateBody = z.object({
    content: z.string().min(1).max(2000),
    nonce: z.string().optional()
});

const messageUpdateBody = z.optional(messageCreateBody);

const messageRetrieveQuery = z.object({
    before: z.string().optional(),
    after: z.string().optional(),
    limit: z.coerce.number().min(1).max(100).default(50),
    around: z.string().optional()
});

const inviteCreateBody = z.object({
    max_uses: z.number().max(100).optional(),
    max_age: z.number().max(604800).default(86400).optional()
});

type GetMessagesOptions = {
    before?: string;
    after?: string;
    around?: string;
    limit: number;
}

async function getMessages(channel: string, options: GetMessagesOptions): Promise<InferSelectModel<typeof messages>[]> {
    if (options.around) {
        const before = await getMessages(channel, {
            before: options.around,
            limit: (options.limit ?? 50) / 2
        });
        const after = await getMessages(channel, {
            after: options.around,
            limit: (options.limit ?? 50) / 2
        });
        return [...before, ...after];
    }

    let q = eq(messages.channelId, channel);
    if (options.before) q = and(q, lt(messages.id, options.before))!;
    else if (options.after) q = and(q, gt(messages.id, options.after))!;


    return (await db.query.messages.findMany({
        limit: (options.limit ?? 50),
        where: q,
        with: {
            author: true
        },
        orderBy: (messages, { desc }) => [desc(messages.createdAt)]
    })).reverse();
}

export default new Elysia({prefix: '/channels'})
    .use(ip())
    .guard({
            beforeHandle(ctx) {
                if (!isAuthenticated(ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token'])) return ctx.status('Unauthorized');
            }
        }, (_app) =>
            _app
                .group('/:id', (app) =>
                    app
                        .resolve(async (ctx) => {
                            const user = await db.query.accounts.findFirst({
                                where: (accounts, {eq}) => eq(accounts.token, ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token']!.value!),
                                with: {
                                    user: true
                                }
                            });
                            
                            const channel = await db.query.channels.findFirst({
                                where: (channels, {eq}) => eq(channels.id, ctx.params.id),
                                with: {
                                    guild: true
                                }
                            });
                            
                            if (!channel) {
                                return {user, member: null, channel: null, guild: null};
                            }
                            
                            const guildMember = await db.query.guildMembers.findFirst({
                                where: (guildMembers, {eq, and}) => and(
                                    eq(guildMembers.userId, user!.id),
                                    eq(guildMembers.guildId, channel.guildId)
                                )
                            });

                            if (guildMember) {
                                return { user, member: guildMember, guild: channel.guild, channel };
                            }

                            return {user, member: null, channel, guild: null};
                        })
                        .guard({
                            async beforeHandle(ctx) {
                                if (!ctx.member) {
                                    console.log("no member");
                                    return ctx.status('Unauthorized', {
                                        code: 'messages.errors.unauthorized'
                                    });
                                }
                                if (!ctx.channel) {
                                    console.log("no channel");
                                    // @ts-expect-error
                                    return ctx.status('Not Found', {
                                        code: 'messages.errors.channelNotFound'
                                    });
                                }
                                const { limited, retryAfter } = await rateLimit(
                                    ctx.ip,
                                    ctx.channel!.rateLimitPerUser > 0 ? 1 : 50,
                                    ctx.channel!.rateLimitPerUser > 0 ? ctx.channel!.rateLimitPerUser : 1,
                                    `messages::${ctx.channel!.id}`
                                );
                                if (limited) return ctx.status('Too Many Requests', {
                                    message: 'You are being rate limited.',
                                    retryAfter
                                });
                            }
                        })
                        .post('/messages', async (ctx) => {
                            const data = messageCreateBody.safeParse(JSON.parse(ctx.body as any));
                            if (!data.success) {
                                console.log(data.error);
                                return ctx.status('Bad Request', {
                                    code: 'messages.errors.validationFailed'
                                });
                            }
                            try {
                                const id = Snowflake.generate();
                                const message = await db.insert(messages).values({
                                    content: data.data.content,
                                    authorId: ctx.user!.id,
                                    guildId: ctx.member!.guildId,
                                    channelId: ctx.params.id,
                                    type: 'DEFAULT',
                                    id,
                                    nonce: data.data.nonce ?? "0"
                                }).returning();
                                ctx.server?.publish(ctx.member!.guildId, JSON.stringify({
                                    op: 0,
                                    t: "MESSAGE_CREATE",
                                    d: {
                                        ...message[0],
                                        author: {
                                            ...ctx.user!.user,
                                            member: {
                                                nickname: ctx.member!.nickname
                                            }
                                        }
                                    }
                                }));
                                return {
                                    ...message[0],
                                    author: {
                                        ...ctx.member,
                                        ...ctx.user!.user
                                    }
                                };
                            } catch (e) {
                                console.error(e);
                                return ctx.status('Internal Server Error', {
                                    e
                                });
                            }
                        })
                        .get('/messages', async (ctx) => {
                            const query = messageRetrieveQuery.safeParse(ctx.query);
                            if (!query.success) {
                                console.error(query.error);
                                return ctx.status('Bad Request', {
                                    code: 'errors.validationFailed'
                                });
                            }
                            return await getMessages(ctx.params.id, query.data);
                        })
                        .patch('/messages/:messageId', async (ctx) => {
                            const data = messageUpdateBody.safeParse(JSON.parse(ctx.body as string));
                            if (!data.success) {
                                console.log(data.error);
                                return ctx.status('Bad Request', {
                                    code: 'messages.errors.validationFailed'
                                });
                            }
                            const message = await db.query.messages.findFirst({
                                where: (messages, {eq}) => eq(messages.id, ctx.params.messageId)
                            });
                            if (!message) return ctx.status('Not Found', {
                                code: 'messages.errors.messageNotFound'
                            });
                            if (message.authorId !== ctx.user!.userId) return ctx.status('Forbidden', {
                                code: 'messages.errors.notYourMessage'
                            });
                            if (message.content === data.data?.content) return message;
                            const updatedMessage = await db
                                .update(messages)
                                .set({
                                    content: data.data?.content,
                                    updatedAt: new Date()
                                })
                                .where(eq(messages.id, ctx.params.messageId))
                                .returning();
                            ctx.server?.publish(message.guildId, JSON.stringify({
                                op: 0,
                                t: 'MESSAGE_UPDATE',
                                d: updatedMessage[0]
                            }));
                            return updatedMessage[0];
                        })
                        .delete('/messages/:messageId', async (ctx) => {
                            const message = await db.query.messages.findFirst({
                                where: (messages, {eq}) => eq(messages.id, ctx.params.messageId)
                            });
                            if (!message) return ctx.status('Not Found', {
                                code: 'messages.errors.messageNotFound'
                            });
                            if (ctx.user!.userId !== message.authorId) return ctx.status('Forbidden', {
                                code: 'messages.errors.forbidden'
                            });
                            await db
                                .delete(messages)
                                .where(eq(messages.id, ctx.params.messageId));
                            ctx.server?.publish(message.guildId, JSON.stringify({
                                op: 0,
                                t: "MESSAGE_DELETE",
                                d: {
                                    id: message.id,
                                    channelId: message.channelId,
                                    guildId: message.guildId
                                }
                            }));
                            return ctx.status('No Content');
                        })
                        .post('/typing', async (ctx) => {
                            try {
                                ctx.server?.publish(ctx.member!.guildId, JSON.stringify({
                                    op: 0,
                                    t: "TYPING_START",
                                    d: {
                                        channelId: ctx.params.id,
                                        userId: ctx.user!.userId,
                                        user: {
                                            ...ctx.user!.user,
                                            member: {
                                                nickname: ctx.member!.nickname
                                            }
                                        },
                                        timestamp: Date.now(),
                                        expiresAt: Date.now() + 10000
                                    }
                                }));

                                return { success: true };
                            } catch (e) {
                                console.error(e);
                                return ctx.status('Internal Server Error');
                            }
                        })
                        .post('/invites', async (ctx) => {
                            const query = inviteCreateBody.safeParse(ctx.body);
                            if (!query.success) {
                                console.error(query.error);
                                return ctx.status('Bad Request', {
                                    code: 'errors.validationFailed'
                                });
                            }
                            try {
                                const code = randomstring.generate({
                                    charset: "alphanumeric",
                                    length: 6
                                });
                                const id = Snowflake.generate();
                                return (await db.insert(invites).values({
                                    code,
                                    id,
                                    guildId: ctx.guild!.id,
                                    channelId: ctx.params.id,
                                    creatorId: ctx.user!.id
                                }).returning())[0];
                            } catch (e) {
                                console.error(e);
                                return ctx.status('Internal Server Error');
                            }
                        })
                )
    )