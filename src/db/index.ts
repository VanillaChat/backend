import { drizzle } from "drizzle-orm/node-postgres";
import schema from "./schema";

export const db = drizzle(Bun.env.DATABASE_URL || "", {
	schema,
});
