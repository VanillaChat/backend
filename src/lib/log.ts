export default function log(module: string, data: string) {
	if (!Number(Bun.env.VERBOSE)) return;
	console.log(`[${module}]`, data);
}
