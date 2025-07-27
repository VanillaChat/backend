const GatewayEvent = {
    Dispatch: 0,
    Heartbeat: 1,
    Identify: 2,
    UpdatePresence: 3
}

export type DispatchEventNames = "READY" | "GUILD_CREATE";

export default GatewayEvent;