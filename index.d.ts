declare module "bun" {
    interface Env {
        NODE_ENV: string;
        TOKEN_SECRET: string;
        REGISTRATION_CLOSED: boolean;
        DATABASE_URL: string;
        USER_GUILD_LIMIT: number;
        VERBOSE: boolean;
        CORS_ORIGINS: string;
    }
}