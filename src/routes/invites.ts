import {Elysia} from "elysia";
import isAuthenticated from "../middleware/isAuthenticated";
import {db} from "../db";
import {guildMembers, invites} from "../db/schema/guild";
import {eq} from "drizzle-orm";
import {connectedUsers} from "../websocket";

export default new Elysia({ prefix: '/invites' })
    .guard({
        beforeHandle(ctx) {
            if (!isAuthenticated(ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token'])) return ctx.status(401);
        }
    }, (_app) =>
        _app
            .group('/:code', (app) =>
                app
                    .resolve(async (ctx) => {
                        const user = await db.query.accounts.findFirst({
                            where: (accounts, {eq}) => eq(accounts.token, ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token']!.value!),
                            with: {
                                user: true
                            }
                        });
                        return {user};
                    })
                    .post('/', async (ctx) => {
                        try {
                            const invite = await db.query.invites.findFirst({
                                where: (invites, {eq}) => eq(invites.code, ctx.params.code),
                                with: {
                                    guild: {
                                        with: {
                                            channels: true,
                                            members: {
                                                with: {
                                                    user: true
                                                }
                                            }
                                        }
                                    }
                                }
                            });
                            if (!invite) return ctx.status(404);
                            if (invite.uses++ >= invite.maxUses && invite.maxUses > 0) {
                                await db.delete(invites).where(eq(invites.code, ctx.params.code))
                            }
                            await db.insert(guildMembers).values({
                                guildId: invite.guildId,
                                userId: ctx.user!.id
                            });
                            // await setTimeout(1000);
                            const ws = connectedUsers.get(ctx.user!.id);
                            ctx.server!.publish(invite.guildId, JSON.stringify({
                                op: 0,
                                t: "GUILD_MEMBER_ADD",
                                d: {
                                    userId: ctx.user!.id,
                                    guildId: invite.guildId,
                                    user: ctx.user!.user,
                                    nickname: null
                                }
                            }));
                            if (ws) {
                                ws.subscribe(invite.guildId);

                                for (const member of invite.guild.members) {
                                    if (connectedUsers.has(member.userId)) ws.send({
                                       op: 0,
                                       t: "PRESENCE_UPDATE",
                                       d: {
                                           userId: member.userId,
                                           status: member.user.status
                                       }
                                    });
                                }

                                if (ctx.user!.user.status !== "UNAVAILABLE") {
                                    ctx.server!.publish(invite.guildId, JSON.stringify({
                                        op: 0,
                                        t: "PRESENCE_UPDATE",
                                        d: {
                                            userId: ctx.user!.user.id,
                                            status: ctx.user!.user.status
                                        }
                                    }));
                                }
                            }
                            return {
                                ...invite,
                                guild: {
                                    ...invite.guild,
                                    members: [
                                        ...invite.guild.members,
                                        {
                                            userId: ctx.user!.id,
                                            guildId: invite.guildId,
                                            user: ctx.user!.user,
                                            nickname: null
                                        }
                                    ]
                                }};
                        } catch (e) {
                            console.error(e);
                            return ctx.status(500);
                        }
                    })
            )
    )