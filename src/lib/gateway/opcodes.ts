import {heartbeatHandler} from "./events/heartbeat";
import {OpCodeHandler} from "../../websocket";
import {identifyHandler} from "./events/identify";
import GatewayEvent from "./GatewayEvent";
import {updatePresenceHandler} from "./events/updatePresence";

export const GatewayOpCodes: {[code: number]: OpCodeHandler} = {
    [GatewayEvent.Heartbeat]: heartbeatHandler,
    [GatewayEvent.Identify]: identifyHandler,
    [GatewayEvent.UpdatePresence]: updatePresenceHandler
}