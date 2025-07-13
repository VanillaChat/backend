import {integer, pgTable, serial, text, timestamp} from "drizzle-orm/pg-core";
import {relations} from "drizzle-orm";
import {users} from "./user";
import {messages} from "./message";

export const guilds = pgTable('guilds', {
    id: text('id').primaryKey().notNull(),
    name: text('name').notNull().default('New Server'),
    brief: text('brief').notNull().default('A server to talk'),
    icon: text('icon'),
    createdAt: timestamp('created_at').defaultNow(),
    ownerId: text('owner_id').notNull()
});

export const guildsRelations = relations(guilds, ({one, many}) => ({
    ownerId: one(guildMembers, {
        fields: [guilds.ownerId],
        references: [guildMembers.id]
    }),
    members: many(guildMembers),
    channels: many(channels),
    messages: many(messages),
    invites: many(invites)
}));

export const guildMembers = pgTable('guild_members', {
    id: serial('id').notNull().primaryKey(),
    guildId: text('guild_id').notNull().references(() => guilds.id, { onDelete: 'cascade' }),
    userId: text('user_id').notNull().references(() => users.id, { onDelete: 'cascade' }),
    nickname: text('nickname')
});

export const guildMembersRelations = relations(guildMembers, ({one}) => ({
    guild: one(guilds, {
        fields: [guildMembers.guildId],
        references: [guilds.id]
    }),
    user: one(users, {
        fields: [guildMembers.userId],
        references: [users.id]
    })
}));

export const channels = pgTable('guild_channels', {
    id: text('id').primaryKey().notNull(),
    name: text('name').notNull().default('General'),
    guildId: text('guild_id').notNull().references(() => guilds.id, { onDelete: 'cascade' }),
    createdAt: timestamp('created_at').defaultNow()
});

export const channelsRelations = relations(channels, ({one, many}) => ({
    guild: one(guilds, {
        fields: [channels.guildId],
        references: [guilds.id]
    }),
    messages: many(messages),
    invites: many(invites)
}))

export const invites = pgTable('guild_invites', {
    id: text('id').notNull().primaryKey(),
    guildId: text('guild_id').notNull().references(() => guilds.id, { onDelete: 'cascade' }) ,
    code: text('code').unique().notNull(),
    uses: integer('uses').notNull().default(0),
    maxUses: integer('max_uses').notNull().default(0),
    creatorId: text('creator_id').references(() => users.id, { onDelete: 'no action' }),
    channelId: text('channel_id').notNull().references(() => channels.id, { onDelete: 'cascade' })
});

export const invitesRelations = relations(invites, ({one}) => ({
    guild: one(guilds, {
        fields: [invites.guildId],
        references: [guilds.id]
    }),
    channel: one(channels, {
        fields: [invites.channelId],
        references: [channels.id]
    }),
    creator: one(users, {
        fields: [invites.creatorId],
        references: [users.id]
    })
}))