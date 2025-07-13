import {WS} from "../../../websocket";
import log from "../../log";

export function setHeartbeat(ws: WS) {
    clearHeartbeat(ws);
    ws.data.store.heartbeats.set(
        ws.id,
        setTimeout(() => {
            log('Gateway', `Expected Heartbeat opcode from ${ws.id} but did not receive it. Closing connection...`);
            ws.send({
                op: 9,
                d: true
            });
            clearHeartbeat(ws);
            setTimeout(() => {
                ws.terminate();
            }, 100);
        }, 1000 * 75)
    );
}

export function clearHeartbeat(ws: WS) {
    if (ws.data.store.heartbeats.get(ws.id)!) {
        clearTimeout(ws.data.store.heartbeats.get(ws.id));
        ws.data.store.heartbeats.delete(ws.id);
    }
}

export function heartbeatHandler(ws: WS) {
    setHeartbeat(ws);
    ws.send({
        op: 11
    });
}