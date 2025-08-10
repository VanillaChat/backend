import {Elysia} from "elysia";
import {db} from "../db";
import {accounts, accountSettings, inviteCodes, users} from "../db/schema/user";
import {Snowflake} from "@theinternetfolks/snowflake";
import * as randomstring from "randomstring";
import {generateToken} from "../lib/token";
import z from "zod/v4";
import log from "../lib/log";
import {count, eq} from "drizzle-orm";
import {UserFlags} from "../lib/bitfield/UserFlags";

const LoginSchema = z.object({
    email: z.email("login.errors.invalidEmail"),
    password: z.string("login.errors.invalidPassword")
});

const RegisterSchema = LoginSchema.extend({
    username: z.string("register.errors.invalidUsername")
        .min(2, "register.errors.usernameTooShort")
        .max(64, "register.errors.usernameTooLong"),
    password: z
        .string("login.errors.invalidPassword")
        .min(8, "register.errors.passwordTooShort")
        .max(128, "register.errors.passwordTooLong")
        .refine(password => /[a-z]/.test(password), "register.errors.passwordLowercase")
        .refine(password => /[A-Z]/.test(password), "register.errors.passwordUppercase")
        .refine(password => /[1-9]/.test(password), "register.errors.passwordDigit")
        .refine(password => /[$&+,:;=?@#|'<>.^*()%!-]/.test(password), "register.errors.passwordSpecialCharacter"),
    confirmPassword: z.string("register.errors.passwordConfirmation"),
    inviteCode: z.string("register.errors.inviteCodeRequired").nullable()
}).refine(data => data.password === data.confirmPassword, {
    message: "register.errors.passwordNotConfirmed",
    path: ['confirmPassword']
});

const auth = new Elysia({ prefix: '/auth' })
    .post('/login', async ({body, status, cookie}) => {
        log('Auth', `Begin authentication user ${(body as any).email}.`);
        const validatedBody = LoginSchema.safeParse(body);
        if (!validatedBody.success) {
            log('Auth', `Authentication failed for user ${(body as any).email}: Validation failed. Stack trace below.`);
            console.log(validatedBody.error.issues);
            return status(400, {
                code: 'VALIDATION_FAILED',
                errors: validatedBody.error.issues.map((issue) => ({
                    code: issue.message,
                    path: issue.path[0]
                }))
            });
        }
        const account  = await db.query.accounts.findFirst({
            where: (accounts, {eq}) => eq(accounts.email, validatedBody.data.email),
        });
        if (!account || !await Bun.password.verify(validatedBody.data.password, account.password)) {
            log('Auth', `Failed authentication for email ${validatedBody.data.password}: Incorrect email or password.`);
            return status(401, {
                code: 'VALIDATION_FAILED',
                errors: [
                    {
                        path: 'email',
                        code: 'login.errors.incorrectDetails'
                    },
                    {
                        path: 'password',
                        code: 'login.errors.incorrectDetails'
                    }
                ]
            });
        }
        // if (!account) {
        //     return status(401, {
        //         code: 'EMAIL',
        //         message: 'email'
        //     })
        // }
        // console.log(account);
        // if (!await Bun.password.verify(body.password, account.password, 'bcrypt')) {
        //     return status(401, {
        //         code: 'PASSWORD',
        //         message: 'password'
        //     })
        // }
        if (Bun.env.NODE_ENV !== 'production') {
            cookie.token.value = account.token;
            cookie.token.expires = new Date(9999, 11, 31, 0, 0, 0);
        } else {
            cookie['__Host-Token'].value = account.token;
            cookie['__Host-Token'].httpOnly = true;
            cookie['__Host-Token'].secure = true;
            cookie['__Host-Token'].expires = new Date(9999, 11, 31, 0, 0, 0);
        }
        return status(204);
    })
    .get('/session', async (ctx) => {
        const cookie = ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token'];
        if (!cookie) return ctx.status('Unauthorized');
        const account = await db.query.accounts.findFirst({
            where: (accounts, {eq}) => eq(accounts.token, cookie.value!),
            with: {
                user: true
            }
        });
        if (!account) return ctx.status('Unauthorized');
        const settings = await db.query.accountSettings.findFirst({
            where: (accountSettings, {eq}) => eq(accountSettings.accountId, account.id),
        });
        return {
            user: account.user,
            account: {
                id: account.id,
                emailVerified: account.emailVerified,
                locale: account.locale,
                email: account.email
            },
            settings: {
                theme: settings?.theme ?? 'light'
            }
        }
    })
    .post('/logout', async (ctx) => {
        if (!ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token']) return ctx.status('Unauthorized');
        ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token'].value = '';
        ctx.cookie[Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token'].expires = new Date(0);
        return ctx.status('No Content');
    })
    .delete('/clear-db', async (ctx) => {
        if (Bun.env.NODE_ENV === 'production') return ctx.status('Internal Server Error', 'This endpoint can only be used from a development environment.');
        await db.delete(users);
        await db.delete(accounts);
        return ctx.status('No Content');
    })
    .post('/register', async ({body, status, cookie, server}) => {
        log('Auth', `Begin registering user with email ${(body as any).email}.`);
        const validatedBody = RegisterSchema.safeParse(body);
        if (!validatedBody.success) {
            log('Auth', `Failed registering user with email ${(body as any).email}. Validation failed. Stack trace below.`);
            console.log(validatedBody.error.issues);
            return status(400, {
                code: 'VALIDATION_FAILED',
                errors: validatedBody.error.issues.map((issue) => ({
                    code: issue.message,
                    path: issue.path[0]
                }))
            });
        }
        if (Number(Bun.env.REGISTRATION_CLOSED)) {
            if (!validatedBody.data.inviteCode) return status(401, {
                code: 'register.errors.inviteCodeRequired',
                path: 'inviteCode'
            });
            const code = await db.query.inviteCodes.findFirst({
                where: (codes, {eq}) => eq(codes.code, validatedBody.data.inviteCode!)
            });
            if (!code || code.used) return status(404, {
               code: 'register.errors.inviteCodeNotFound',
               path: 'inviteCode'
            });
        }
        const account = await db.query.accounts.findFirst({
            where: (accounts, {eq}) => eq(accounts.email, validatedBody.data.email)
        });
        const userCount = (await db.select({ count: count() }).from(users))[0];
        if (account) {
            log('Auth', `Failed to register user with email ${validatedBody.data.email}: Email already exists.`);
            return status(409,
                {
                    code: 'register.errors.emailAlreadyClaimed',
                    path: 'email'
                });
        }
        if (validatedBody.data.password !== validatedBody.data.confirmPassword) {
            log('Auth', `Failed to register user with email ${validatedBody.data.email}: Password does not match confirm password.`);
            return status(400,
                {
                    code: 'register.errors.passwordNotConfirmed',
                    path: 'confirmPassword',
                });
        }
        const tag = randomstring.generate({
            length: 5,
            charset: 'alphanumeric'
        });
        const user = await db.query.users.findFirst({
           where: (users, {eq, and}) => and(eq(users.username, validatedBody.data.username), eq(users.tag, tag))
        });
        if (user) {
            log('Auth', `Failed to register user with email ${validatedBody.data.email}: User/Tag combination ${validatedBody.data.username}/${tag} already exists.`);
            return status(409, {
                code: 'register.errors.usernameTagTooPopular',
                path: 'username'
            });
        }
        const hashedPassword = await Bun.password.hash(validatedBody.data.password,  'bcrypt');
        const userId = Snowflake.generate();
        const token = generateToken(userId, 0);
        try {
            await db.insert(users).values([
                {
                    id: userId,
                    username: validatedBody.data.username,
                    tag,
                    status: "ONLINE",
                    flags: userCount.count === 0 ? UserFlags.ADMIN : 0
                }
            ]).onConflictDoNothing();
            await db.insert(accountSettings).values([{ accountId: userId }]);
            let inviteCode = undefined;
            if (Number(Bun.env.REGISTRATION_CLOSED)) {
                inviteCode = await db
                    .update(inviteCodes)
                    .set({
                        used: true,
                        usedBy: userId
                    })
                    .where(eq(inviteCodes.code, validatedBody.data.inviteCode!)).returning();
                server!.publish('admins', JSON.stringify({
                    op: 0,
                    t: "INVITE_CODE_USE",
                    d: {
                        executor: {
                            username: validatedBody.data.username,
                            id: userId
                        },
                        code: inviteCode[0].id
                    }
                }));
            }
            await db.insert(accounts).values([
                {
                    id: userId,
                    email: validatedBody.data.email,
                    password: hashedPassword,
                    settingsId: userId,
                    userId,
                    token,
                    inviteCode: inviteCode?.[0]?.id
                }
            ])
                .onConflictDoNothing();
            log('Auth', `Registering user with email ${validatedBody.data.email} succeeded. Assigned ID: ${userId}`);
            if (Bun.env.NODE_ENV !== 'production') {
                cookie.token.value = token;
            } else {
                cookie['__Host-Token'].value = token;
                cookie['__Host-Token'].httpOnly = true;
                cookie['__Host-Token'].secure = true;
            }
            return status(204);
        } catch (error) {
            log('Auth', `An error occurred while trying to register user with email ${validatedBody.data.email}. Stack trace:`);
            console.error(error);
            return status(500, {
                message: 'Something went wrong.',
                code: 'common.serverError',
                path: 'global'
            })
        }
    });

export default auth;