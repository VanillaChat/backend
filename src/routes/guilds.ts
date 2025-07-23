import {Elysia} from "elysia";
import isAuthenticated from "../middleware/isAuthenticated";
import z from "zod/v4";
import {db} from "../db";
import {accounts} from "../db/schema/user";
import {count, eq} from "drizzle-orm";
import {channels, guildMembers, guilds} from "../db/schema/guild";
import {Snowflake} from "@theinternetfolks/snowflake";
import log from "../lib/log";

const ServerCreateSchema = z.object({
    name: z.string("modals.serverCreate.nameRequired").min(2, "modals.serverCreate.nameMinChars").max(64, "modals.serverCreate.nameMaxChars"),
    brief: z.string("modals.serverCreate.briefRequired").min(2, "modals.serverCreate.briefMinChars").max(36, "modals.serverCreate.briefMinChars"),
});

export default new Elysia({ prefix: '/guilds' })
    .guard({
        beforeHandle(ctx) {
            if (!isAuthenticated(ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token'])) return ctx.status('Unauthorized');
        }
    }, (app) =>
        app
            .resolve(async (ctx) => {
                const user = await db.query.accounts.findFirst({
                    where: eq(accounts.token, ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token']!.value!)
                });
                return { user }
            })
            .post('/', async (ctx) => {
                log('Guilds', `Creating guild ${(ctx.body as any).name} for user ${ctx.user!.id}`);
                const validatedBody = ServerCreateSchema.safeParse(ctx.body);
                if (!validatedBody.success) {
                    log('Guilds', `Creating guild ${(ctx.body as any).name} failed for user ${ctx.user!.id}: Validation failed. Stack trace below.`);
                    console.log(validatedBody.error.issues);
                    return ctx.status('Bad Request', {
                        code: 'VALIDATION_FAILED',
                        errors: validatedBody.error.issues.map((issue) => ({
                            code: issue.message,
                            path: issue.path[0]
                        }))
                    });
                }
                const [result] = await db.select({ count: count() }).from(guildMembers).where(eq(guildMembers.userId, ctx.user!.id));
                if (result.count >= Bun.env.USER_GUILD_LIMIT) {
                    log('Guilds', `Creating guild ${validatedBody.data.name} failed for user ${ctx.user!.id}: The user has too many guilds.`);
                    return ctx.status('Forbidden', {
                        code: 'GUILD_LIMIT_EXCEEDED',
                        path: 'global',
                        message: 'app.modals.serverCreate.serverLimitExceeded'
                    });
                }
                const id = Snowflake.generate();
                try {
                    const guild = await db.insert(guilds).values([
                        {
                            name: validatedBody.data.name,
                            brief: validatedBody.data.brief,
                            icon: null,
                            id,
                            ownerId: ctx.user!.id
                        }
                    ]).onConflictDoNothing().returning();
                    await db.insert(guildMembers).values([
                        {
                            nickname: null,
                            guildId: id,
                            userId: ctx.user!.id
                        }
                    ]);
                    const defaultChannelId = Snowflake.generate();
                    const channel = await db.insert(channels).values([
                        {
                            id: defaultChannelId,
                            guildId: id
                        }
                    ]).onConflictDoNothing().returning();
                    log('Guilds', `Creating guild ${validatedBody.data.name} for user ${ctx.user!.id} succeeded. Assigned ID: ${id}`);
                    return { guild: guild[0], channels: channel };
                } catch (e) {
                    console.error(e);
                    return ctx.status('Internal Server Error');
                }
            })
    );