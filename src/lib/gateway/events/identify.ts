import {connectedUsers, WS} from "../../../websocket";
import {verifyToken} from "../../token";
import {db} from "../../../db";
import {clearHeartbeat} from "./heartbeat";
import {eq} from "drizzle-orm";
import {guildMembers} from "../../../db/schema/guild";
import log from "../../log";

export const identifyHandler = async (ws: WS) => {
    log('Gateway', `IDENTIFY opcode received from ${ws.id}.`);
    const cookies = new Bun.CookieMap(ws.data.request.headers.get("cookie")!);
    const cookie = cookies.get(Bun.env.NODE_ENV === 'production' ? '__Host-Token' : 'token');
    const token = cookie ? verifyToken(cookie) : null;
    const account = token
        ? await db.query.accounts.findFirst({
            where: (accounts, { eq }) => eq(accounts.id, token.userId),
            with: {
                user: true,
                settings: true
            }
        })
        : null;
    if (!token || !account) {
        log('Gateway', `Cannot authenticate ${ws.id} due to invalid token. Closing connection...`);
        ws.send({
            op: 9,
            d: false
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
                    channels: true
                }
            }
        }
    });
    for (const member of members) {
        ws.subscribe(member.guildId);
    }

    const obj: Record<string, any> = {
        op: 0,
        t: "READY",
        d: {
            account: {
                id: account.id,
                emailVerified: account.emailVerified,
                locale: account.locale,
                email: account.email
            },
            settings: account.settings,
            user: account.user,
            guilds: members.map(member => member.guild)
        }
    };

    if ((account.user.flags & 1 << 0) === 1 << 0) {
        ws.subscribe("admins");
        const inviteCodes = await db.query.inviteCodes.findMany({
            with: {
                createdBy: true,
                usedBy: {
                    columns: {userId: true},
                    with: {
                        user: true
                    }
                }
            }
        });
        obj.d['appSettings'] = {
            inviteCodes
        }
    }

    ws.send(obj);
    connectedUsers.set(account.id, ws);
}