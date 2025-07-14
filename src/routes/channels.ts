import {Elysia} from "elysia";
import isAuthenticated from "../middleware/isAuthenticated";
import {db} from "../db";
import {messages} from "../db/schema/message";
import z from "zod/v4";
import {Snowflake} from "@theinternetfolks/snowflake";
import {and, eq, gt, InferSelectModel, lt} from "drizzle-orm";
import {invites} from "../db/schema/guild";
import * as randomstring from "randomstring";

const messageCreateBody = z.object({
    content: z.string().min(1).max(2000),
    nonce: z.string().optional()
});

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
        orderBy: (messages, { asc }) => [asc(messages.id)]
    }));
}

export default new Elysia({prefix: '/channels'})
    .guard({
            beforeHandle(ctx) {
                if (!isAuthenticated(ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token'])) return ctx.status(401);
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
                            const guildMember = await db.query.guildMembers.findFirst({
                                where: (guildMembers, {eq}) => eq(guildMembers.userId, user!.id),
                                with: {
                                    guild: true
                                }
                            });
                            if (guildMember) {
                                const { guild, ...member } = guildMember;
                                return { user, member, guild };
                            }
                            return {user, member: null};
                        })
                        .guard({
                            async beforeHandle(ctx) {
                                if (!ctx.member) return ctx.status(401, {
                                    code: 'messages.errors.unauthorized'
                                });
                                const channel = await db.query.channels.findFirst({
                                    where: (channels, {eq}) => eq(channels.id, ctx.params.id)
                                });
                                if (!channel) return ctx.status(404, {
                                    code: 'messages.errors.channelNotFound'
                                });
                            }
                        })
                        .post('/messages', async (ctx) => {
                            const data = messageCreateBody.safeParse(JSON.parse(ctx.body as any));
                            if (!data.success) {
                                console.log(data.error);
                                return ctx.status(400, {
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
                                        ...ctx.user!.user,
                                        ...ctx.member
                                    }
                                };
                            } catch (e) {
                                console.error(e);
                                return ctx.status(500, {
                                    e
                                });
                            }
                        })
                        .get('/messages', async (ctx) => {
                            const query = messageRetrieveQuery.safeParse(ctx.query);
                            if (!query.success) {
                                console.error(query.error);
                                return ctx.status(400, {
                                    code: 'errors.validationFailed'
                                });
                            }
                            return await getMessages(ctx.params.id, query.data);
                        })
                        .post('/invites', async (ctx) => {
                            const query = inviteCreateBody.safeParse(ctx.body);
                            if (!query.success) {
                                console.error(query.error);
                                return ctx.status(400, {
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
                                return ctx.status(500);
                            }
                        })
                )
    )