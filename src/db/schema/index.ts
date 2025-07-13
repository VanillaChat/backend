import * as userSchema from "./user";
import * as guildSchema from "./guild";
import * as messageSchema from "./message";

export default {
    ...userSchema,
    ...guildSchema,
    ...messageSchema
}