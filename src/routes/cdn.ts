import { Elysia } from "elysia";

export default new Elysia({ prefix: "/cdn" }).get(
	"/*",
	async ({ status, params: { "*": path } }) => {
		const file = Bun.file(`${import.meta.dir}/../../../cdn/${path}`);
		if (!(await file.exists())) return status(404);
		return Buffer.from(await file.arrayBuffer());
	},
);
