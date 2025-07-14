import {Elysia} from "elysia";

export default new Elysia({ prefix: '/cdn' })
    .get('/*', async ({ params: { '*': path } }) => {
        const file = Bun.file(`${import.meta.dir}/../../../cdn/${path}`);
        return Buffer.from(await file.arrayBuffer());
    })