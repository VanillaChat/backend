import {Elysia} from "elysia";
import isAuthenticated from "../middleware/isAuthenticated";
import {db} from "../db";
import {UserFlags} from "../lib/bitfield/UserFlags";
import {Snowflake} from "@theinternetfolks/snowflake";
import * as randomstring from "randomstring";
import {inviteCodes} from "../db/schema/user";
import {eq} from "drizzle-orm";

export default new Elysia({ prefix: '/admin' })
    .guard({
        beforeHandle(ctx) {
            if (!isAuthenticated(ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token'])) return ctx.status('Unauthorized');
        }
    }, (app) =>
        app
            .resolve(async (ctx) => {
                const account = await db.query.accounts.findFirst({
                    where: (accounts, {eq}) => eq(accounts.token, ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token']!.value!),
                    with: {
                        user: true
                    }
                });
                return {account};
            })
            .guard({
                async beforeHandle(ctx) {
                    if ((ctx.account!.user.flags & UserFlags.ADMIN) !== UserFlags.ADMIN) return ctx.status('Forbidden');
                }
            })
            .post('/invite-codes', async (ctx) => {
                const id = Snowflake.generate();
                const code = randomstring.generate({ charset: 'alphanumeric', length: 8 });
                try {
                    const row = await db.insert(inviteCodes).values({
                        id,
                        code,
                        createdBy: ctx.account!.id
                    }).returning();
                    return row[0];
                } catch (e) {
                    console.error(e);
                    return ctx.status('Internal Server Error');
                }
            })
            .delete('/invite-codes/:id', async (ctx) => {
                const code = await db.query.inviteCodes.findFirst({
                    where: (codes, { eq }) => eq(codes.id, ctx.params.id),
                });

                if (!code) {
                    return ctx.status('Not Found');
                } else if (code.used) {
                    return ctx.status('Forbidden');
                }

                try {
                    await db.delete(inviteCodes).where(eq(inviteCodes.id, ctx.params.id));
                    ctx.server!.publish('admins', JSON.stringify({
                        op: 0,
                        t: "INVITE_CODE_DELETE",
                        d: {
                            executor: ctx.account!.id,
                            id: ctx.params.id
                        }
                    }));
                    return ctx.status('No Content');
                } catch (err) {
                    console.error(err);
                    return ctx.status('Internal Server Error', {
                        error: err!.toString()
                    });
                }
            })
    )