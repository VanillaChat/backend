import {Context, Elysia} from "elysia";
import {logger} from "@chneau/elysia-logger";
// import {cors} from "@elysiajs/cors";
import auth from "./routes/auth";
import websocket from "./websocket";
import guilds from "./routes/guilds";
import channels from "./routes/channels";
import invites from "./routes/invites";
import admin from "./routes/admin";

const app = new Elysia()
    .use(logger())
    // .use(cors({
    //     origin: Bun.env.CORS_ORIGINS.split(';'),
    //     preflight: true
    // }))
    .use(auth)
    .use(guilds)
    .use(channels)
    .use(invites)
    .use(admin)
    .use(websocket)
    .get('/', () => {
        return "Hello World!";
    })
    .all('*', (ctx: Context) => ctx.status(404, '404 Not Found!'))
    .listen(3000);

console.log(`[Core] Server listening on port ${app.server!.port}`);