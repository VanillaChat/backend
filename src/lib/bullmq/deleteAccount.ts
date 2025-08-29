import { Queue, Worker } from "bullmq";
import { and, eq } from "drizzle-orm";
import { db } from "../../db";
import { guildMembers } from "../../db/schema/guild";
import { messages } from "../../db/schema/message";
import {
	accountDeleteSchedule,
	inviteCodes,
	users,
} from "../../db/schema/user";
import { app } from "../../index";
import { connectedUsers } from "../../websocket";

export const deleteAccountQueue = new Queue("deleteAccount", {
	connection: {
		path: Bun.env.REDIS_URL,
	},
});

new Worker(
	"deleteAccount",
	async (job) => {
		console.log(`Delete account job ${job.data.userId}`);

		try {
			await db
				.delete(accountDeleteSchedule)
				.where(eq(accountDeleteSchedule.id, job.data.userId));
			const guilds = await db.query.guildMembers.findMany({
				where: (guildMembers, { eq }) =>
					eq(guildMembers.userId, job.data.userId),
			});
			const msgs = await db.query.messages.findMany({
				where: (messages, { eq }) => eq(messages.authorId, job.data.userId),
			});

			if (job.data.deleteMessages) {
				await db.delete(messages).where(eq(messages.authorId, job.data.userId));
			} else {
				await db
					.update(messages)
					.set({
						authorId: "0",
					})
					.where(eq(messages.authorId, job.data.userId));
			}

			const code = await db.query.inviteCodes.findFirst({
				where: (inviteCodes, { eq }) => eq(inviteCodes.usedBy, job.data.userId),
				with: {
					usedBy: true,
				},
			});
			if (code) {
				await db
					.update(inviteCodes)
					.set({
						usedBy: "0",
					})
					.where(eq(inviteCodes.usedBy, job.data.userId));
				app.server?.publish(
					"admins",
					JSON.stringify({
						op: 0,
						t: "INVITE_CODE_USE",
						d: {
							executor: {
								userId: "0",
								user: {
									id: "0",
									username: "Deleted User",
								},
							},
							code: code.id,
						},
					}),
				);
			}

			await db.delete(users).where(eq(users.id, job.data.userId));

			for (const guild of guilds) {
				app.server?.publish(
					guild.guildId,
					JSON.stringify({
						op: 0,
						t: "GUILD_MEMBER_REMOVE",
						d: {
							guildId: guild.guildId,
							user: {
								id: job.data.userId,
							},
						},
					}),
				);
				await db
					.delete(guildMembers)
					.where(
						and(
							eq(guildMembers.guildId, guild.guildId),
							eq(guildMembers.userId, job.data.userId),
						),
					);

				if (job.data.deleteMessages) {
					const m = msgs.filter((m) => m.guildId === guild.guildId);
					const channels = Array.from(new Set(m.map((m) => m.channelId)));
					for (const channel of channels) {
						const channelMessages = m.filter((m) => m.channelId === channel);
						app.server?.publish(
							guild.guildId,
							JSON.stringify({
								op: 0,
								t: "MESSAGE_DELETE_BULK",
								d: {
									channelId: channel,
									guildId: guild.guildId,
									messages: channelMessages.map((m) => m.id),
								},
							}),
						);
					}
				}
			}

			const sockets = connectedUsers.get(job.data.userId) || [];
			for (const socket of sockets) {
				socket.send(
					JSON.stringify({
						op: 9,
						d: false,
					}),
				);
				socket.close();
			}
		} catch (e) {
			console.log(e);
		}
		console.log(
			`[DELETE ACCOUNT] Deleted account ${job.data.userId} from database.`,
		);
	},
	{
		connection: {
			path: Bun.env.REDIS_URL,
		},
	},
);
