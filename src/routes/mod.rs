pub mod admin;
pub mod auth;
pub mod cdn;
pub mod channels;
pub mod guilds;
pub mod invites;
pub mod users;
pub mod voice;

use crate::state::SharedState;
use axum::Router;

pub fn create_router() -> Router<SharedState> {
    Router::new()
        .nest("/auth", auth::router())
        .nest("/guilds", guilds::router())
        .nest("/channels", channels::router())
        .nest("/users", users::router())
        .nest("/invites", invites::router())
        .nest("/admin", admin::router())
        .nest("/cdn", cdn::router())
        .merge(voice::router())
}
