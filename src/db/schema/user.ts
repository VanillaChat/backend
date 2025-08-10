import {boolean, integer, pgEnum, pgTable, serial, text, timestamp} from "drizzle-orm/pg-core";
import {relations} from "drizzle-orm";
import {messages} from "./message";
import {invites} from "./guild";

export const accounts = pgTable('accounts', {
    id: text('id').primaryKey().notNull(),
    email: text('email').unique().notNull(),
    password: text('password').notNull(),
    userId: text('user_id').notNull().references(() => users.id, {onDelete: 'cascade'}),
    emailVerified: boolean('email_verified').notNull().default(false),
    locale: text('locale').notNull().default('en_us'),
    token: text('token').unique().notNull(),
    passwordVersion: integer('password_version').notNull().default(0),
    settingsId: text('settings_id').unique().notNull().references(() => accountSettings.accountId, { onDelete: 'cascade' }),
    inviteCode: text('invite_code').unique().references(() => inviteCodes.id, { onDelete: 'no action' })
});

export const accountsRelations = relations(accounts, ({one}) => ({
    user: one(users, { fields: [accounts.userId], references: [users.id] }),
    settings: one(accountSettings, { fields: [accounts.settingsId], references: [accountSettings.accountId] }),
    inviteCode: one(inviteCodes, { fields: [accounts.inviteCode], references: [inviteCodes.id] })
}));

export const userStatus = pgEnum('UserStatus', ['ONLINE', 'DND', 'IDLE', 'LOOKING_TO_PLAY', 'UNAVAILABLE']);

export const users = pgTable('users', {
    username: text('username').notNull(),
    tag: text('tag').notNull(),
    id: text('id').primaryKey().notNull(),
    createdAt: timestamp('created_at').notNull().defaultNow(),
    bot: boolean('bot').notNull().default(false),
    status: userStatus('status').notNull().default('ONLINE'),
    flags: integer('flags').notNull().default(0),
    bio: text('bio'),
    avatar: text('avatar'),
    banner: text('banner')
});

export const usersRelations = relations(users, ({one, many}) => ({
    account: one(accounts, {fields: [users.id], references: [accounts.id]}),
    messages: many(messages),
    invites: many(invites),
    inviteCodes: many(inviteCodes)
}));

export const inviteCodes = pgTable('invite_codes', {
    id: text('id').primaryKey().notNull(),
    code: text('code').unique().notNull(),
    createdBy: text('created_by').notNull().references(() => users.id, { onDelete: 'no action' }),
    usedBy: text('used_by').unique(),
    used: boolean('used').default(false),
    createdAt: timestamp('created_at').defaultNow()
});

export const inviteCodesRelations = relations(inviteCodes, ({one}) => ({
    createdBy: one(users, { fields: [inviteCodes.createdBy], references: [users.id] }),
    usedBy: one(accounts, { fields: [inviteCodes.usedBy], references: [accounts.id] })
}))

export const theme = pgEnum('Theme', ['LIGHT', 'DARK', 'DIM']);

export const accountSettings = pgTable('account_settings', {
    id: serial('id').primaryKey().notNull(),
    accountId: text('account_id').unique(),
    theme: theme('theme').notNull().default('LIGHT'),
    compactMode: boolean('compact_mode').notNull().default(false),
    compactShowAvatars: boolean('compact_show_avatars').notNull().default(true),
});

export const accountSettingsRelations = relations(accountSettings, ({one}) => ({
    account: one(accounts, {fields: [accountSettings.accountId], references: [accounts.id]})
}));