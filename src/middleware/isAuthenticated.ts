import {Cookie} from "elysia";
import {verifyToken} from "../lib/token";
import {db} from "../db";

export default async function isAuthenticated(cookie: Cookie<string | undefined>) {
    const token = cookie ? verifyToken(cookie!.value!) : null;
    const account = token
        ? await db.query.accounts.findFirst({
            where: (accounts, { eq }) => eq(accounts.id, token.userId),
            with: {
                user: true
            }
        })
        : null;
    if (!token || !account) return false;
    return account;
}