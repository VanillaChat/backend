import { relations } from "drizzle-orm";
import { pgEnum, pgTable, text, timestamp } from "drizzle-orm/pg-core";
import { channels, guilds } from "./guild";
import { users } from "./user";

export const messageType = pgEnum("message_type", [
	"DEFAULT",
	"USER_JOIN",
	"USER_LEAVE",
]);

export const messages = pgTable("messages", {
	id: text("id").primaryKey().notNull(),
	authorId: text("author_id")
		.notNull()
		.references(() => users.id, { onDelete: "no action" }),
	channelId: text("channel_id")
		.notNull()
		.references(() => channels.id, { onDelete: "cascade" }),
	guildId: text("guild_id")
		.notNull()
		.references(() => guilds.id, { onDelete: "cascade" }),
	content: text("content"),
	createdAt: timestamp("created_at").defaultNow(),
	updatedAt: timestamp("updated_at"),
	type: messageType("type").default("DEFAULT"),
	nonce: text("nonce").default("0"),
});

export const messagesRelations = relations(messages, ({ one }) => ({
	author: one(users, {
		fields: [messages.authorId],
		references: [users.id],
	}),
	guild: one(guilds, {
		fields: [messages.guildId],
		references: [guilds.id],
	}),
	channel: one(channels, {
		fields: [messages.channelId],
		references: [channels.id],
	}),
}));

export const messageReferences = pgTable("message_references", {
	id: text("id")
		.primaryKey()
		.notNull()
		.references(() => messages.id, { onDelete: "cascade" }),
	referencedMessageId: text("referenced_message_id")
		.notNull()
		.references(() => messages.id, { onDelete: "no action" }),
});

export const messageReferencesRelations = relations(
	messageReferences,
	({ one }) => ({
		referencedMessage: one(messages, {
			fields: [messageReferences.referencedMessageId],
			references: [messages.id],
		}),
		message: one(messages, {
			fields: [messageReferences.id],
			references: [messages.id],
		}),
	}),
);
