import { eq } from "drizzle-orm";
import { db } from "../../../db";
import { guildMembers } from "../../../db/schema/guild";
import { users } from "../../../db/schema/user";
import type { Payload, WS } from "../../../websocket";
import log from "../../log";
import { verifyToken } from "../../token";

export const updatePresenceHandler = async (ws: WS, data: Payload) => {
	log("Gateway", `UPDATE_PRESENCE opcode received from ${ws.id}.`);
	const cookies = new Bun.CookieMap(
		ws.data.request.headers.get("cookie") as string,
	);
	const cookie = cookies.get(
		Bun.env.NODE_ENV === "production" ? "__Host-Token" : "token",
	);
	const token = cookie ? verifyToken(cookie) : null;
	const account = token
		? await db.query.accounts.findFirst({
				where: (accounts, { eq }) => eq(accounts.id, token.userId),
				with: {
					user: true,
					settings: true,
				},
			})
		: null;
	if (!token || !account) return;
	const members = await db.query.guildMembers.findMany({
		where: eq(guildMembers.userId, account.id),
		with: {
			guild: {
				with: {
					channels: true,
					members: {
						with: {
							user: true,
						},
					},
				},
			},
		},
	});

	if (account.user.status === data.d.status) return;

	await db
		.update(users)
		.set({
			status: data.d.status,
		})
		.where(eq(users.id, account.user.id));

	for (const member of members) {
		ws.publish(
			member.guildId,
			JSON.stringify({
				op: 0,
				t: "PRESENCE_UPDATE",
				d: {
					userId: account.user.id,
					status: account.user.status,
				},
			}),
		);
	}
};
