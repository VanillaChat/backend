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
                                            channels: true
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
                            const ws = connectedUsers.get(ctx.user!.id);
                            if (ws) {
                                console.log(ws);
                                ws.subscribe(invite.guildId);
                            }
                            return invite;
                        } catch (e) {
                            console.error(e);
                            return ctx.status(500);
                        }
                    })
            )
    )