pub mod admin;
pub mod auth;
pub mod cdn;
pub mod channels;
pub mod guilds;
pub mod invites;
pub mod users;

use axum::Router;
use crate::state::SharedState;

pub fn create_router() -> Router<SharedState> {
    Router::new()
        .nest("/auth", auth::router())
        .nest("/guilds", guilds::router())
        .nest("/channels", channels::router())
        .nest("/users", users::router())
        .nest("/invites", invites::router())
        .nest("/admin", admin::router())
        .nest("/cdn", cdn::router())
}
