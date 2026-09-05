use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{self, Ctx, extras, join, log, purge, rights, stats, welcome};

use super::auth::AdminGate;
use super::dashboard::dashboard;

pub async fn rights_list(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let rows: Vec<_> = rights::RIGHTS
        .iter()
        .map(|right| {
            json!({
                "key": right.key,
                "label": right.label,
                "open": !rights::closed(&ctx, gate.chat, right.key),
            })
        })
        .collect();
    Json(json!({ "rights": rows }))
}

pub async fn rights_toggle(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(key): Path<String>,
) -> Response {
    if !rights::RIGHTS.iter().any(|r| r.key == key) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let shut = !rights::closed(&ctx, gate.chat, &key);
    rights::set_closed(&ctx, gate.chat, &key, shut).await;
    if let Some(chat_ref) = ctx.chat_ref(gate.chat) {
        rights::apply(&ctx, chat_ref, gate.chat, false).await;
    }
    rights_list(State(ctx), gate).await.into_response()
}

pub async fn log_list(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let kinds: Vec<_> = log::KINDS
        .iter()
        .map(|(key, label)| {
            json!({ "key": key, "label": label, "on": ctx.settings.is_locked(gate.chat, key) })
        })
        .collect();

    let channel = ctx
        .settings
        .value(gate.chat, log::CHANNEL)
        .map(|value| handlers::esc(&value));
    Json(json!({
        "channel": channel,
        "on": ctx.settings.is_locked(gate.chat, log::ON),
        "kinds": kinds,
    }))
}

pub async fn log_toggle(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(key): Path<String>,
) -> Response {
    if !log::KINDS.iter().any(|(k, _)| *k == key) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let now_on = !ctx.settings.is_locked(gate.chat, &key);
    ctx.settings.set(gate.chat, &key, now_on).await;
    log_list(State(ctx), gate).await.into_response()
}

pub async fn log_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    ctx.settings.set(gate.chat, log::CHANNEL, false).await;
    ctx.settings.set(gate.chat, log::ON, false).await;
    log_list(State(ctx), gate).await
}

pub async fn join_gate_get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let channel = ctx
        .settings
        .value(gate.chat, join::CHANNEL)
        .map(|value| handlers::esc(&value));
    Json(json!({ "channel": channel }))
}

#[derive(Deserialize)]
pub struct ChannelBody {
    channel: String,
}

pub async fn join_gate_set(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<ChannelBody>,
) -> impl IntoResponse {
    join::set_channel(&ctx, gate.chat, &body.channel).await;
    join_gate_get(State(ctx), gate).await
}

pub async fn welcome_get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let text = ctx
        .settings
        .value(gate.chat, welcome::TEXT)
        .map(|value| handlers::esc(&value));
    Json(json!({
        "text": text,
        "has_media": ctx.settings.value(gate.chat, welcome::MEDIA).is_some(),
    }))
}

#[derive(Deserialize)]
pub struct TextBody {
    text: String,
}

pub async fn welcome_set(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<TextBody>,
) -> impl IntoResponse {
    ctx.settings.set_value(gate.chat, welcome::TEXT, &body.text).await;
    welcome_get(State(ctx), gate).await
}

pub async fn welcome_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    ctx.settings.set(gate.chat, welcome::TEXT, false).await;
    ctx.settings.set(gate.chat, welcome::MEDIA, false).await;
    welcome_get(State(ctx), gate).await
}

pub async fn night_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    extras::set_night(&ctx, gate.chat, None).await;
    dashboard(State(ctx), gate).await
}

pub async fn report_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    stats::set_report_at(&ctx, gate.chat, None).await;
    dashboard(State(ctx), gate).await
}

pub async fn purge_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    purge::set_auto_at(&ctx, gate.chat, None).await;
    dashboard(State(ctx), gate).await
}

pub async fn admins_list(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> Response {
    let Some(chat_ref) = ctx.chat_ref(gate.chat) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let admins: Vec<_> = handlers::admin_entries(&ctx, chat_ref)
        .await
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "name": entry.name,
                "is_creator": entry.is_creator,
                "is_bot": entry.is_bot,
            })
        })
        .collect();
    Json(json!({ "admins": admins, "can_remove": gate.is_owner })).into_response()
}

fn admin_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

pub async fn remove_admin(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(user_id): Path<i64>,
) -> Response {
    if !gate.is_owner {
        return admin_error(StatusCode::FORBIDDEN, "فقط مالک می تواند ادمین عزل کند.");
    }
    let Some(chat_ref) = ctx.chat_ref(gate.chat) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(target) = handlers::admin_entries(&ctx, chat_ref)
        .await
        .into_iter()
        .find(|entry| entry.id == user_id)
    else {
        return admin_error(StatusCode::NOT_FOUND, "این کاربر در لیست ادمین ها نیست.");
    };

    if target.is_creator || handlers::owner(&ctx, gate.chat) == Some(user_id) {
        return admin_error(StatusCode::BAD_REQUEST, "نمی توان مالک را عزل کرد.");
    }

    match handlers::promote::demote_by_id(
        &ctx,
        chat_ref,
        gate.chat,
        user_id,
        &target.name,
        Some((gate.user, "")),
    )
    .await
    {
        Ok(()) => admins_list(State(ctx), gate).await,
        Err(_) => admin_error(
            StatusCode::BAD_GATEWAY,
            "انجام نشد. ربات فقط می تواند ادمین هایی را عزل کند که خودش اضافه کرده است.",
        ),
    }
}
