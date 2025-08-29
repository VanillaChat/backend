import type { OpCodeHandler } from "../../websocket";
import { heartbeatHandler } from "./events/heartbeat";
import { identifyHandler } from "./events/identify";
import { updatePresenceHandler } from "./events/updatePresence";
import GatewayEvent from "./GatewayEvent";

export const GatewayOpCodes: { [code: number]: OpCodeHandler } = {
	[GatewayEvent.Heartbeat]: heartbeatHandler,
	[GatewayEvent.Identify]: identifyHandler,
	[GatewayEvent.UpdatePresence]: updatePresenceHandler,
};
