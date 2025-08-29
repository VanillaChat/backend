import { eq } from "drizzle-orm";
import { db } from "../../../db";
import { guildMembers } from "../../../db/schema/guild";
import { accountDeleteSchedule } from "../../../db/schema/user";
import { connectedUsers, type WS } from "../../../websocket";
import { UserFlags } from "../../bitfield/UserFlags";
import { deleteAccountQueue } from "../../bullmq/deleteAccount";
import log from "../../log";
import { verifyToken } from "../../token";
import { clearHeartbeat } from "./heartbeat";

export const identifyHandler = async (ws: WS) => {
	log("Gateway", `IDENTIFY opcode received from ${ws.id}.`);
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

	if (!token || !account) {
		log(
			"Gateway",
			`Cannot authenticate ${ws.id} due to invalid token. Closing connection...`,
		);
		ws.send({
			op: 9,
			d: false,
		});
		ws.terminate();
		clearHeartbeat(ws);
		return;
	}
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

	const presences = new Set<{ id: string; status: string }>();

	for (const member of members) {
		ws.subscribe(member.guildId);
		for (const guildMember of member.guild.members) {
			if (connectedUsers.has(guildMember.userId))
				presences.add({
					id: guildMember.userId,
					status: guildMember.user.status,
				});
		}
	}

	const obj: Record<string, any> = {
		op: 0,
		t: "READY",
		d: {
			account: {
				id: account.id,
				emailVerified: account.emailVerified,
				locale: account.locale,
				email: account.email,
			},
			settings: {
				theme: account.settings?.theme ?? "light",
				compactMode: account.settings?.compactMode ?? false,
				compactShowAvatars: account.settings?.compactShowAvatars ?? true,
			},
			user: account.user,
			guilds: members.map((member) => member.guild),
		},
	};

	if ((account.user.flags & UserFlags.ADMIN) === UserFlags.ADMIN) {
		ws.subscribe("admins");
		const inviteCodes = await db.query.inviteCodes.findMany({
			with: {
				createdBy: true,
				usedBy: {
					columns: { userId: true },
					with: {
						user: true,
					},
				},
			},
		});
		obj.d.appSettings = {
			inviteCodes,
		};
	}

	if (account.user.status !== "UNAVAILABLE") {
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
		presences.add({ id: account.id, status: account.user.status });
	}

	if (!account.user.bot) {
		obj.d.presences = Array.from(presences.values());
		const scheduledJob = await db.query.accountDeleteSchedule.findFirst({
			where: (accountDeleteSchedule, { eq }) =>
				eq(accountDeleteSchedule.id, account.id),
		});
		if (scheduledJob) {
			const job = await deleteAccountQueue.getJob(scheduledJob?.jobId);
			if (job) {
				obj.d.settings.pendingDeletion = true;
				obj.d.settings.deleteAt = scheduledJob.deleteAt;
			} else {
				obj.d.settings.pendingDeletion = false;
				await db
					.delete(accountDeleteSchedule)
					.where(eq(accountDeleteSchedule.id, account.id));
			}
		}
	}

	ws.send(obj);
	const user = connectedUsers.get(account.id);
	if (user) {
		connectedUsers.set(account.id, [...user, ws]);
	} else {
		connectedUsers.set(account.id, [ws]);
	}
};
