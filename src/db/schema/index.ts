import * as guildSchema from "./guild";
import * as messageSchema from "./message";
import * as userSchema from "./user";

export default {
	...userSchema,
	...guildSchema,
	...messageSchema,
};
