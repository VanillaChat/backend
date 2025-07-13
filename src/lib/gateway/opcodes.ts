import {heartbeatHandler} from "./events/heartbeat";
import {OpCodeHandler} from "../../websocket";
import {identifyHandler} from "./events/identify";
import GatewayEvent from "./GatewayEvent";

export const GatewayOpCodes: {[code: number]: OpCodeHandler} = {
    [GatewayEvent.Heartbeat]: heartbeatHandler,
    [GatewayEvent.Identify]: identifyHandler
}