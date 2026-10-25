use axum::Router;
use axum::routing::{delete, get, post};

use super::{
    MiniAppState, cases, dashboard, filters, lists, locks, misc, more, overview, response_policy,
    settings, voice, words,
};

pub fn router() -> Router<MiniAppState> {
    Router::new()
        .route("/dashboard", get(dashboard::dashboard))
        .route("/groups", get(overview::groups))
        .route("/health", get(overview::health))
        .route("/activity", get(overview::activity))
        .route("/settings/apply", post(settings::apply))
        .route("/response-policy", get(response_policy::get))
        .route("/response-policy/apply", post(response_policy::apply))
        .route("/locks", get(locks::list))
        .route("/locks/{key}/toggle", post(locks::toggle))
        .route("/locks/all", post(locks::all))
        .route("/locks/set", post(locks::set))
        .route("/lists/{kind}", get(lists::list).delete(lists::clear))
        .route("/lists/{kind}/{entry_key}", delete(lists::remove))
        .route("/filters", post(filters::create))
        .route("/filters/{entry_key}/toggle", post(filters::toggle))
        .route("/filter-words", get(words::get).post(words::add))
        .route("/filter-words/exact", post(words::exact))
        .route("/voice", get(voice::list))
        .route("/voice/words", post(voice::add))
        .route("/voice/words/{key}", delete(voice::remove))
        .route("/voice/restore", post(voice::restore_all))
        .route("/cases", get(cases::list))
        .route("/cases/{id}", get(cases::get))
        .route("/cases/{id}/resolve", post(cases::resolve))
        .route("/cases/{id}/reverse", post(cases::reverse))
        .route("/cases/{id}/notes", post(cases::note))
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
        .route("/tempmedia", get(more::tempmedia_get))
        .route("/tempmedia/{name}/toggle", post(more::tempmedia_toggle))
        .route("/limits", get(more::limits_get))
        .route("/limits/{name}/toggle", post(more::limits_toggle))
        .route("/bot-lock", get(more::bot_lock_get))
        .route("/bot-lock/{key}/toggle", post(more::bot_lock_toggle))
        .route("/rules", get(more::rules_get).post(more::rules_set))
        .route("/slow", get(more::slow_get).post(more::slow_set))
        .route("/report/test", post(more::report_test))
        .route("/answers", post(more::answer_add))
        .route("/log/channel", post(more::log_connect))
        .route("/voice/clear", post(more::voice_clear))
        .route("/strict-causes", get(more::strict_get))
        .route("/strict-causes/{cause}/toggle", post(more::strict_toggle))
        .route("/allowed-bots", get(more::allowed_bots_get).post(more::allowed_bots_add))
        .route("/allowed-bots/{id}", delete(more::allowed_bots_remove))
}
