import "./lib/bullmq/deleteAccount";
import { logger } from "@chneau/elysia-logger";
import { cors } from "@elysiajs/cors";
import { type Context, Elysia } from "elysia";
import Valkey from "iovalkey";
import * as randomstring from "randomstring";
import { db } from "./db";
import {
	accountSettings,
	accounts,
	users as usersTable,
} from "./db/schema/user";
import admin from "./routes/admin";
import auth from "./routes/auth";
import cdn from "./routes/cdn";
import channels from "./routes/channels";
import guilds from "./routes/guilds";
import invites from "./routes/invites";
import users from "./routes/users";
import websocket from "./websocket";

export const valkey = new Valkey(Bun.env.REDIS_URL);

export const app = new Elysia()
	.use(logger())
	.use(
		cors({
			origin: ["http://localhost:5173"],
		}),
	)
	.use(auth)
	.use(guilds)
	.use(channels)
	.use(invites)
	.use(admin)
	.use(cdn)
	.use(users)
	.use(websocket)
	.all("/health", () => "OK")
	.all("/", () => "OK")
	.all("*", (ctx: Context) => ctx.status("Not Found", "404 Not Found!"))
	.listen(3000);

const deletedAccount = await db.query.accounts.findFirst({
	where: (accounts, { eq }) => eq(accounts.id, "0"),
});

if (!deletedAccount) {
	await db.insert(usersTable).values({
		id: "0",
		username: "Deleted User",
		tag: "unavailable",
		createdAt: new Date(),
		bot: false,
		status: "UNAVAILABLE",
		flags: 0,
	});
	await db.insert(accountSettings).values({
		accountId: "0",
		theme: "LIGHT",
		compactMode: false,
		compactShowAvatars: true,
	});
	await db.insert(accounts).values({
		id: "0",
		email: `${randomstring.generate({ length: 16 })}@deleted.com`,
		password: randomstring.generate({ length: 24 }),
		token: "",
		settingsId: "0",
		userId: "0",
		locale: "en-US",
		emailVerified: false,
	});
}

console.log(`[Core] Server listening on port ${app.server?.port}`);
