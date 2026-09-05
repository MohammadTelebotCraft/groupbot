use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post};

use crate::handlers::Ctx;

use super::{dashboard, filters, lists, locks, misc, overview, settings, voice};

pub fn router() -> Router<Arc<Ctx>> {
    Router::new()
        .route("/dashboard", get(dashboard::dashboard))
        .route("/groups", get(overview::groups))
        .route("/health", get(overview::health))
        .route("/activity", get(overview::activity))
        .route("/settings/apply", post(settings::apply))
        .route("/locks", get(locks::list))
        .route("/locks/{key}/toggle", post(locks::toggle))
        .route("/locks/all", post(locks::all))
        .route("/lists/{kind}", get(lists::list).delete(lists::clear))
        .route("/lists/{kind}/{entry_key}", delete(lists::remove))
        .route("/filters", post(filters::create))
        .route("/filters/{entry_key}/toggle", post(filters::toggle))
        .route("/voice", get(voice::list))
        .route("/voice/words", post(voice::add))
        .route("/voice/words/{key}", delete(voice::remove))
        .route("/voice/restore", post(voice::restore_all))
        .route("/admins", get(misc::admins_list))
        .route("/admins/{user_id}", delete(misc::remove_admin))
        .route("/rights", get(misc::rights_list))
        .route("/rights/{key}/toggle", post(misc::rights_toggle))
        .route("/log", get(misc::log_list))
        .route("/log/{kind}/toggle", post(misc::log_toggle))
        .route("/log/off", post(misc::log_off))
        .route(
            "/join-gate",
            get(misc::join_gate_get).post(misc::join_gate_set),
        )
        .route("/welcome", get(misc::welcome_get).post(misc::welcome_set))
        .route("/welcome/off", post(misc::welcome_off))
        .route("/night/off", post(misc::night_off))
        .route("/report/off", post(misc::report_off))
        .route("/purge/off", post(misc::purge_off))
}
