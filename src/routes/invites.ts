import { eq } from "drizzle-orm";
import { Elysia } from "elysia";
import { db } from "../db";
import { guildMembers, invites } from "../db/schema/guild";
import isAuthenticated from "../middleware/isAuthenticated";
import { connectedUsers } from "../websocket";

export default new Elysia({ prefix: "/invites" })
	.get("/:code", async (ctx) => {
		const invite = await db.query.invites.findFirst({
			where: (invites, { eq }) => eq(invites.code, ctx.params.code),
			with: {
				guild: true,
				creator: true,
				channel: true,
			},
		});
		if (!invite) return ctx.status("Not Found");
		return {
			type: 0,
			code: invite.code,
			inviter: invite.creator ?? null,
			guild: {
				id: invite.guildId,
				name: invite.guild.name,
				brief: invite.guild.brief,
				icon: invite.guild.icon,
			},
			guildId: invite.guildId,
			channel: {
				id: invite.channelId,
				type: 0,
				name: invite.channel.name,
			},
		};
	})
	.guard(
		{
			beforeHandle(ctx) {
				if (
					!isAuthenticated(
						ctx.cookie[
							Bun.env.NODE_ENV === "production" ? "__Host-Token" : "token"
						],
					)
				)
					return ctx.status("Unauthorized");
			},
		},
		(_app) =>
			_app.group("/:code", (app) =>
				app
					.resolve(async (ctx) => {
						const user = await db.query.accounts.findFirst({
							where: (accounts, { eq }) =>
								eq(
									accounts.token,
									ctx.cookie[
										Bun.env.NODE_ENV === "production" ? "__Host-Token" : "token"
									]?.value as string,
								),
							with: {
								user: true,
							},
						});
						return { user };
					})
					.post("/", async (ctx) => {
						try {
							const invite = await db.query.invites.findFirst({
								where: (invites, { eq }) => eq(invites.code, ctx.params.code),
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
							if (!invite)
								return ctx.status("Not Found", {
									code: "servers.notFound",
									path: "invite",
								});
							const member = await db.query.guildMembers.findFirst({
								where: (members, { eq, and }) =>
									and(
										eq(members.userId, ctx.user?.id as string),
										eq(members.guildId, invite.guildId),
									),
							});
							if (member)
								return ctx.status("Conflict", {
									path: "invite",
									code: "ALREADY_A_MEMBER",
									message: "errors.serverCreation.alreadyAMember",
								});
							if (invite.uses++ >= invite.maxUses && invite.maxUses > 0) {
								await db
									.delete(invites)
									.where(eq(invites.code, ctx.params.code));
							}
							await db.insert(guildMembers).values({
								guildId: invite.guildId,
								userId: ctx.user?.id as string,
							});
							const sockets = connectedUsers.get(ctx.user?.id as string);
							ctx.server?.publish(
								invite.guildId,
								JSON.stringify({
									op: 0,
									t: "GUILD_MEMBER_ADD",
									d: {
										userId: ctx.user?.id as string,
										guildId: invite.guildId,
										user: ctx.user?.user,
										nickname: null,
									},
								}),
							);
							if (sockets) {
								sockets.forEach((ws) => {
									ws.subscribe(invite.guildId);
								});

								for (const member of invite.guild.members) {
									if (connectedUsers.has(member.userId)) {
										sockets.forEach((ws) => {
											ws.send({
												op: 0,
												t: "PRESENCE_UPDATE",
												d: {
													userId: member.userId,
													status: member.user.status,
												},
											});
										});
									}
								}

								if (ctx.user?.user.status !== "UNAVAILABLE") {
									ctx.server?.publish(
										invite.guildId,
										JSON.stringify({
											op: 0,
											t: "PRESENCE_UPDATE",
											d: {
												userId: ctx.user?.user.id,
												status: ctx.user?.user.status,
											},
										}),
									);
								}
							}
							return {
								...invite,
								guild: {
									...invite.guild,
									members: [
										...invite.guild.members,
										{
											userId: ctx.user?.id,
											guildId: invite.guildId,
											user: ctx.user?.user,
											nickname: null,
										},
									],
								},
							};
						} catch (e) {
							console.error(e);
							return ctx.status("Internal Server Error");
						}
					}),
			),
	);
