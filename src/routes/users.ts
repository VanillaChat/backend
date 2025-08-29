import crypto from "node:crypto";
import { mkdir, readdir, rmdir } from "node:fs/promises";
import { join } from "node:path";
import { and, eq } from "drizzle-orm";
import { Elysia } from "elysia";
import { ip } from "elysia-ip";
import sharp from "sharp";
import { db } from "../db";
import { guildMembers } from "../db/schema/guild";
import {
	accountDeleteSchedule,
	accountSettings,
	users,
} from "../db/schema/user";
import { deleteAccountQueue } from "../lib/bullmq/deleteAccount";
import isAuthenticated from "../middleware/isAuthenticated";
import rateLimit from "../middleware/rateLimit";
import { connectedUsers } from "../websocket";

const AVATARS_DIR = join(import.meta.dir, "..", "..", "..", "cdn", "avatars");
const BANNERS_DIR = join(import.meta.dir, "..", "..", "..", "cdn", "banners");
await mkdir(AVATARS_DIR, { recursive: true }).catch(console.error);
await mkdir(BANNERS_DIR, { recursive: true }).catch(console.error);

export default new Elysia({ prefix: "/users" }).use(ip()).guard(
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
		_app.group("/@me", (app) =>
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

					const servers = await db.query.guildMembers.findMany({
						where: (members, { eq }) =>
							eq(members.userId, user?.user.id as string),
					});

					return { user, guildIds: servers.map((g) => g.guildId) };
				})
				.patch(
					"/user-settings",
					async ({ user, body, status }) => {
						try {
							const { theme, compactMode, compactShowAvatars } = body as {
								theme?: "LIGHT" | "DARK" | "DIM";
								compactMode?: boolean;
								compactShowAvatars?: boolean;
							};
							if (
								[theme, compactMode, compactShowAvatars].every(
									(x) => typeof x === "undefined",
								)
							)
								return status("Bad Request", {
									error:
										"At least one field (theme, compactMode, compactShowAvatars) is required",
								});

							const updates: any = {};

							if (theme) updates.theme = theme;
							if (compactMode !== undefined) updates.compactMode = compactMode;
							if (compactShowAvatars !== undefined)
								updates.compactShowAvatars = compactShowAvatars;

							if (Object.keys(updates).length > 0) {
								await db
									.update(accountSettings)
									.set(updates)
									.where(
										eq(accountSettings.accountId, user?.user.id as string),
									);
								return status("No Content");
							}

							return status("Bad Request", {
								error:
									"At least one field (theme, compactMode, compactShowAvatars) is required",
							});
						} catch (error) {
							console.error("Error updating user settings:", error);
							return status("Internal Server Error", {
								error: "Internal server error",
							});
						}
					},
					{
						async beforeHandle(ctx) {
							const { limited, retryAfter } = await rateLimit(
								ctx.ip,
								25,
								120_000,
								`settings-change:${ctx.user?.id as string}`,
							);
							if (limited)
								return ctx.status("Too Many Requests", {
									message: "You are being rate limited.",
									retryAfter,
								});
						},
					},
				)
				.post("/request-deletion", async ({ user, status, body }) => {
					const { password, deleteMessages } = body as {
						password: string;
						deleteMessages: boolean;
					};

					if (
						!(await Bun.password.verify(
							password,
							user?.password as string,
							"bcrypt",
						))
					)
						return status("Unauthorized", {
							error: "Invalid password",
							path: "password",
						});

					const deleteSchedule = await db.query.accountDeleteSchedule.findFirst(
						{
							where: (deleteSchedule, { eq }) =>
								eq(deleteSchedule.id, user?.user.id as string),
						},
					);
					if (deleteSchedule)
						return status("Conflict", {
							error: "This account already has a pending deletion request.",
							path: "password",
						});

					// 7 * 24 * 60 *
					const deleteAt = Date.now() + 10 * 1000;
					const job = await deleteAccountQueue.add(
						"delete-account",
						{
							userId: user?.user.id as string,
							deleteMessages,
						},
						{
							delay: 10 * 1000,
						},
					);

					try {
						await db.insert(accountDeleteSchedule).values({
							id: user?.user.id as string,
							jobId: job.id as string,
							deleteMessages,
							deleteAt: deleteAt.toString(),
						});
					} catch (e) {
						console.error(e);
					}

					return status("OK", {
						deleteAt,
					});
				})
				.post("/cancel-deletion", async ({ user, status }) => {
					const deleteSchedule = await db.query.accountDeleteSchedule.findFirst(
						{
							where: (deleteSchedule, { eq }) =>
								eq(deleteSchedule.id, user?.user.id as string),
						},
					);
					if (!deleteSchedule)
						return status("Not Found", {
							error: "This account does not have a pending deletion request.",
						});

					const job = await deleteAccountQueue.getJob(deleteSchedule.jobId);
					if (!job) {
						await db
							.delete(accountDeleteSchedule)
							.where(eq(accountDeleteSchedule.id, user?.user.id as string));
						return status("Not Found", {
							error: "This account does not have a pending deletion request.",
						});
					}

					await job.remove();
					await db
						.delete(accountDeleteSchedule)
						.where(eq(accountDeleteSchedule.id, user?.user.id as string));

					return status("No Content");
				})
				.delete("/guilds/:guildId", async (ctx) => {
					const member = await db.query.guildMembers.findFirst({
						where: (guildMembers, { eq, and }) =>
							and(
								eq(guildMembers.userId, ctx.user?.id as string),
								eq(guildMembers.guildId, ctx.params.guildId),
							),
						with: {
							user: true,
						},
					});
					if (!member)
						return ctx.status("Not Found", {
							error: "You are not a member of this guild.",
						});

					try {
						await db
							.delete(guildMembers)
							.where(
								and(
									eq(guildMembers.userId, ctx.user?.id as string),
									eq(guildMembers.guildId, ctx.params.guildId),
								),
							);

						const sockets = connectedUsers.get(ctx.user?.id as string);
						if (sockets) {
							sockets.forEach((socket) => {
								socket.unsubscribe(member.guildId);
							});
						}

						ctx.server?.publish(
							member.guildId,
							JSON.stringify({
								op: 0,
								t: "GUILD_MEMBER_REMOVE",
								d: {
									guildId: member.guildId,
									user: member.user,
								},
							}),
						);

						return ctx.status("No Content");
					} catch (e) {
						console.error(e);
						return ctx.status("Internal Server Error", {
							error: "An unknown error occurred.",
						});
					}
				})
				.patch(
					"/",
					async ({ user, guildIds, body, status, server }) => {
						try {
							const {
								username,
								tag,
								bio,
								avatar: base64Avatar,
								banner: base64Banner,
								password,
							} = body as {
								username?: string;
								tag?: string;
								bio?: string;
								avatar?: string;
								banner?: string;
								password?: string;
							};

							if (
								!user?.user.bot &&
								[base64Avatar, base64Banner].every(
									(x) => typeof x === "undefined",
								)
							) {
								if (!password)
									return status("Bad Request", {
										error: "Password is required",
									});

								const isValidPassword = await Bun.password.verify(
									password,
									user?.password as string,
									"bcrypt",
								);
								if (!isValidPassword)
									return status("Unauthorized", {
										error: "Invalid password",
									});
							}

							if (
								!username &&
								!tag &&
								!bio &&
								[base64Avatar, base64Banner].every(
									(x) => typeof x === "undefined",
								)
							) {
								return status("Bad Request", {
									error:
										"At least one field (username, tag, avatar or banner) is required",
								});
							}

							const updates: any = {};

							if (username) updates.username = username;
							if (tag) updates.tag = tag;
							if (bio) updates.bio = bio;

							if (base64Avatar) {
								try {
									const base64Data = base64Avatar.split(";base64,").pop();
									if (!base64Data) {
										throw new Error("Invalid base64 data");
									}

									const buffer = Buffer.from(base64Data, "base64");
									const maxSize = parseInt(
										Bun.env.MAX_AVATAR_SIZE || "10000000",
										10,
									); // Default 10MB

									if (buffer.length > maxSize) {
										return status("Payload Too Large", {
											error: `Avatar size exceeds maximum allowed size of ${maxSize / 1024 / 1024}MB`,
										});
									}

									const processedBuffer = await sharp(buffer)
										.ensureAlpha()
										.resize(256, 256, {
											fit: "cover",
											withoutEnlargement: true,
										})
										.webp({ quality: 80 })
										.toBuffer();

									const hash = crypto
										.createHash("sha256")
										.update(processedBuffer)
										.digest("hex")
										.substring(0, 64);

									const userDir = join(AVATARS_DIR, user?.user.id as string);
									await mkdir(userDir, { recursive: true });

									if (user?.user.avatar) {
										const oldAvatarPath = join(
											AVATARS_DIR,
											user?.user.avatar as string,
										);
										try {
											const file = Bun.file(oldAvatarPath);
											if (await file.exists()) await file.unlink();

											try {
												const files = await readdir(userDir);
												if (files.length === 0) {
													await rmdir(userDir);
												}
											} catch (error) {
												console.warn(
													"Failed to check/remove user avatar directory:",
													error,
												);
											}
										} catch (error) {
											console.warn("Failed to remove old avatar:", error);
										}
									}

									await Bun.write(
										join(userDir, `${hash}.webp`),
										processedBuffer,
									);
									updates.avatar = hash;
								} catch (error) {
									console.error("Error processing avatar:", error);
									return status("Bad Request", {
										error: "Failed to process avatar image",
									});
								}
							} else if (base64Avatar === null) {
								const userDir = join(AVATARS_DIR, user?.user.id as string);
								try {
									const files = await readdir(userDir);
									if (files.length === 0) {
										await rmdir(userDir);
									}
								} catch (error) {
									console.warn(
										"Failed to check/remove user avatar directory:",
										error,
									);
								}
								updates.avatar = null;
							}

							if (base64Banner) {
								try {
									const base64Data = base64Banner.split(";base64,").pop();
									if (!base64Data) {
										throw new Error("Invalid base64 data");
									}

									const buffer = Buffer.from(base64Data, "base64");
									const maxSize = parseInt(
										Bun.env.MAX_BANNER_SIZE || "25000000",
										10,
									); // Default 25MB

									if (buffer.length > maxSize) {
										return status("Payload Too Large", {
											error: `Banner size exceeds maximum allowed size of ${maxSize / 1024 / 1024}MB`,
										});
									}

									const processedBuffer = await sharp(buffer)
										.ensureAlpha()
										.resize(342, 130, {
											fit: "cover",
											withoutEnlargement: true,
										})
										.webp({ quality: 80 })
										.toBuffer();

									const hash = crypto
										.createHash("sha256")
										.update(processedBuffer)
										.digest("hex")
										.substring(0, 64);

									const userDir = join(BANNERS_DIR, user?.user.id as string);
									await mkdir(userDir, { recursive: true });

									if (user?.user.banner) {
										const oldBannerPath = join(BANNERS_DIR, user?.user.banner);
										try {
											const file = Bun.file(oldBannerPath);
											if (await file.exists()) await file.unlink();

											try {
												const files = await readdir(userDir);
												if (files.length === 0) {
													await rmdir(userDir);
												}
											} catch (error) {
												console.warn(
													"Failed to check/remove user banner directory:",
													error,
												);
											}
										} catch (error) {
											console.warn("Failed to remove old banner:", error);
										}
									}

									await Bun.write(
										join(userDir, `${hash}.webp`),
										processedBuffer,
									);
									updates.banner = hash;
								} catch (error) {
									console.error("Error processing banner:", error);
									return status("Bad Request", {
										error: "Failed to process banner image",
									});
								}
							} else if (base64Banner === null) {
								const userDir = join(BANNERS_DIR, user?.user.id as string);
								try {
									const files = await readdir(userDir);
									if (files.length === 0) {
										await rmdir(userDir);
									}
								} catch (error) {
									console.warn(
										"Failed to check/remove user banner directory:",
										error,
									);
								}
								updates.banner = null;
							}

							if (Object.keys(updates).length > 0) {
								await db
									.update(users)
									.set(updates)
									.where(eq(users.id, user?.user.id as string));

								const [updatedUser] = await db
									.select()
									.from(users)
									.where(eq(users.id, user?.user.id as string))
									.limit(1);

								guildIds.forEach((guildId) => {
									server?.publish(
										guildId,
										JSON.stringify({
											op: 0,
											t: "USER_UPDATE",
											d: {
												guildId,
												user: updatedUser,
											},
										}),
									);
								});

								return updatedUser;
							}

							return user?.user;
						} catch (error) {
							console.error("Error updating user:", error);
							return status("Internal Server Error", {
								error: "Internal server error",
							});
						}
					},
					{
						async beforeHandle(ctx) {
							const { limited, retryAfter } = await rateLimit(
								ctx.ip,
								10,
								3_600_000,
								`profile-change:${ctx.user?.id as string}`,
							);
							if (limited)
								return ctx.status("Too Many Requests", {
									message: "You are being rate limited.",
									retryAfter,
								});
						},
					},
				),
		),
);
