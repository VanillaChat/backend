import {pgEnum, pgTable, text, timestamp} from "drizzle-orm/pg-core";
import {users} from "./user";
import {channels, guilds} from "./guild";
import {relations} from "drizzle-orm";

export const messageType = pgEnum('message_type', ['DEFAULT', 'USER_JOIN', 'USER_LEAVE']);

export const messages = pgTable('messages', {
   id: text('id').primaryKey().notNull(),
   authorId: text('author_id').notNull().references(() => users.id, { onDelete: 'no action' }),
    channelId: text('channel_id').notNull().references(() => channels.id, { onDelete: 'cascade' }),
    guildId: text('guild_id').notNull().references(() => guilds.id, { onDelete: 'cascade' }),
    content: text('content'),
    createdAt: timestamp('created_at').defaultNow(),
    updatedAt: timestamp('updated_at'),
    type: messageType('type').default('DEFAULT'),
    nonce: text('nonce').default("0")
});

export const messagesRelations = relations(messages, ({one}) => ({
    author: one(users, {
        fields: [messages.authorId],
        references: [users.id]
    }),
    guild: one(guilds, {
        fields: [messages.guildId],
        references: [guilds.id]
    }),
    channel: one(channels, {
        fields: [messages.channelId],
        references: [channels.id]
    })
}));