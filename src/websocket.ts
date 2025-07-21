import {Cookie, Elysia} from "elysia";
import {GatewayOpCodes} from "./lib/gateway/opcodes";
import {ElysiaWS} from "elysia/ws";
import {setHeartbeat} from "./lib/gateway/events/heartbeat";
import {db} from "./db";
import {eq} from "drizzle-orm";
import {guildMembers} from "./db/schema/guild";
import log from "./lib/log";

export type Payload = {
    op: number;
    d?: any;
    s?: number;
    t?: string;
};

export type OpCodeHandler = (ws: WS, data: Payload) => void;

export type WS = ElysiaWS<{
    query: Record<string, string | undefined>;
    params: {};
    headers: Record<string, string | undefined>;
    request: Request;
    cookie: Record<string, Cookie<string | undefined>>;
    store: Record<string, Map<string, any | undefined>>;
}>;

export const connectedUsers = new Map<string, WS>();

const app = new Elysia()
    .state({heartbeats: new Map<string, NodeJS.Timeout>()})
    .ws('/gateway', {
        async open(ws) {
            log('Gateway', `Socket ${ws.id} connected. Sending HELLO opcode and awaiting identify...`);
            ws.send({
                op: 10,
                d: {
                    heartbeat_interval: 1000 * 30
                }
            });
            setHeartbeat(ws);
        },
        async message(ws, message: Payload) {
            log('Gateway', `Received message: ${JSON.stringify(message)}`);
            const handler = GatewayOpCodes[message.op];
            if (!handler) {
                log('Gateway', `Invalid opcode: ${message.op}`);
                return;
            }
            handler(ws, message);
        },
        async close(ws) {
            const users = Array.from(connectedUsers.entries());
            const userId = users.find(u => u[1].id === ws.id)?.[0];
            if (userId) {
                const members = await db.query.guildMembers.findMany({
                    where: eq(guildMembers.userId, userId),
                });
                for (const member of members) {
                    log('Gateway', `Unsubscribing ${userId} from ${member.guildId}`);
                    ws.publish(member.guildId, JSON.stringify({
                        op: 0,
                        t: "PRESENCE_UPDATE",
                        d: {
                            userId,
                            status: "UNAVAILABLE"
                        }
                    }));
                    ws.unsubscribe(member.guildId);
                }
                connectedUsers.delete(userId);
            }
        }
    });

export default app;