
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{Ctx, locks};

use super::auth::AdminGate;
use super::dashboard::dashboard;

fn write_error(chat: i64, operation: &str, error: &crate::state::SettingsWriteError) -> Response {
    ::log::warn!("miniapp: {operation} for {chat} failed: {error}");
    let (status, message) = if error.commit_outcome_unknown() {
        (
            StatusCode::ACCEPTED,
            "نتیجه ذخیره سازی نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید.",
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "قفل ذخیره نشد؛ دوباره تلاش کنید.",
        )
    };
    (status, Json(json!({ "error": message }))).into_response()
}

fn lock_json(ctx: &Ctx, chat: i64, lock: &locks::Lock) -> serde_json::Value {
    json!({
        "key": lock.key,
        "label": lock.names[0],
        "on": ctx.settings.is_locked(chat, lock.key),
        "icon": crate::handlers::premium::lock_icon(lock.key, ctx.settings.is_locked(chat, lock.key)).key(),
    })
}

pub async fn list(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let plain: Vec<_> = locks::plain()
        .map(|lock| lock_json(&ctx, gate.chat, lock))
        .collect();
    let ai: Vec<_> = locks::LOCKS
        .iter()
        .filter(|lock| locks::is_ai(lock.key))
        .map(|lock| lock_json(&ctx, gate.chat, lock))
        .collect();
    Json(json!({ "plain": plain, "ai": ai }))
}

pub async fn toggle(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(key): Path<String>,
) -> axum::response::Response {
    let Some(lock) = locks::LOCKS.iter().find(|lock| lock.key == key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let now_on = !ctx.settings.is_locked(gate.chat, lock.key);
    if let Err(error) = locks::try_set(&ctx, gate.chat, lock.key, now_on).await {
        return write_error(gate.chat, "lock write", &error);
    }
    dashboard(State(ctx), gate.into()).await.into_response()
}

#[derive(Deserialize)]
pub struct AllBody {
    on: bool,
}

pub async fn all(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<AllBody>,
) -> Response {
    for lock in locks::plain() {
        if let Err(error) = locks::try_set(&ctx, gate.chat, lock.key, body.on).await {
            return write_error(gate.chat, "bulk lock write", &error);
        }
    }
    dashboard(State(ctx), gate.into()).await.into_response()
}
