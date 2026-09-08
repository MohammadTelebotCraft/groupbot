
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{self, Ctx, extras, join, log, purge, rights, stats, welcome};
use crate::state::SettingsWriteError;

use super::auth::AdminGate;
use super::dashboard::dashboard;

fn write_error(chat: i64, operation: &str, error: &dyn std::fmt::Display) -> Response {
    ::log::warn!("miniapp: {operation} for {chat} failed: {error}");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "تغییر ذخیره نشد؛ دوباره تلاش کنید." })),
    )
        .into_response()
}

fn uncertain_write(chat: i64, operation: &str, error: &dyn std::fmt::Display) -> Response {
    ::log::warn!("miniapp: {operation} outcome for {chat} is unknown: {error}");
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "accepted": "unknown",
            "error": "وضعیت ذخیره‌سازی مشخص نیست؛ پیش از تکرار دوباره بررسی کنید."
        })),
    )
        .into_response()
}

fn settings_write_error(chat: i64, operation: &str, error: &SettingsWriteError) -> Response {
    match error {
        SettingsWriteError::CommitUncertain(_) => uncertain_write(chat, operation, error),
        _ => write_error(chat, operation, error),
    }
}


pub async fn rights_list(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> Response {
    let snapshot = match rights::snapshot(&ctx, gate.chat).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return StatusCode::CONFLICT.into_response(),
        Err(error) => return write_error(gate.chat, "rights read", &error),
    };
    let rows: Vec<_> = rights::RIGHTS
        .iter()
        .map(|right| {
            let open = !rights::closed(&snapshot, right);
            json!({
                "key": right.key,
                "label": right.label,
                "open": open,
                "icon": handlers::premium::permission(right.key, open).key(),
            })
        })
        .collect();
    Json(json!({ "rights": rows, "pending": snapshot.pending(i64::try_from(stats::local_seconds()).unwrap_or(i64::MAX)) })).into_response()
}

pub async fn rights_toggle(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(key): Path<String>,
) -> Response {
    let Some(right) = rights::right(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(chat_ref) = ctx.chat_ref(gate.chat) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let snapshot = match rights::snapshot(&ctx, gate.chat).await {
        Ok(Some(snapshot)) if snapshot.seeded => snapshot,
        Ok(_) => return StatusCode::CONFLICT.into_response(),
        Err(error) => return write_error(gate.chat, "rights read", &error),
    };
    let shut = !rights::closed(&snapshot, right);
    match rights::set_right(&ctx, chat_ref, gate.chat, right, shut).await {
        Ok(rights::DeliveryOutcome::Applied) => {}
        Ok(rights::DeliveryOutcome::PendingRetry { retry_at, .. }) => {
            return (
                StatusCode::ACCEPTED,
                Json(json!({ "pending": true, "retry_at": retry_at })),
            )
                .into_response();
        }
        Ok(rights::DeliveryOutcome::AcceptedDeliveryUnknown { reason }) => {
            ::log::warn!(
                "miniapp: rights delivery state for {} is unknown: {reason}",
                gate.chat
            );
            return (
                StatusCode::ACCEPTED,
                Json(json!({ "pending": true, "delivery": "unknown" })),
            )
                .into_response();
        }
        Ok(rights::DeliveryOutcome::Superseded) => {
            return (StatusCode::ACCEPTED, Json(json!({ "pending": true }))).into_response();
        }
        Err(error) if error.acceptance_unknown() => {
            return uncertain_write(gate.chat, "rights write", &error);
        }
        Err(error) => return write_error(gate.chat, "rights write", &error),
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
    if let Err(error) = ctx.settings.try_set(gate.chat, &key, now_on).await {
        return settings_write_error(gate.chat, "log toggle", &error);
    }
    log_list(State(ctx), gate).await.into_response()
}

pub async fn log_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> Response {
    let result = ctx
        .settings
        .try_apply_batch(
            gate.chat,
            &[
                crate::state::SettingMutation::Delete { key: log::CHANNEL },
                crate::state::SettingMutation::Delete { key: log::ON },
            ],
        )
        .await;
    if let Err(error) = result {
        return settings_write_error(gate.chat, "log disable", &error);
    }
    log_list(State(ctx), gate).await.into_response()
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
) -> Response {
    if let Err(error) = join::set_channel(&ctx, gate.chat, &body.channel).await {
        return settings_write_error(gate.chat, "join gate write", &error);
    }
    join_gate_get(State(ctx), gate).await.into_response()
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
) -> Response {
    if let Err(error) = ctx
        .settings
        .try_apply_batch(
            gate.chat,
            &[
                crate::state::SettingMutation::Put {
                    key: welcome::TEXT,
                    value: &body.text,
                },
                crate::state::SettingMutation::Delete {
                    key: welcome::ENTITIES,
                },
            ],
        )
        .await
    {
        return settings_write_error(gate.chat, "welcome write", &error);
    }
    welcome_get(State(ctx), gate).await.into_response()
}

pub async fn welcome_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> Response {
    if let Err(error) = welcome::try_clear_stored(&ctx, gate.chat).await {
        return settings_write_error(gate.chat, "welcome disable", &error);
    }
    welcome_get(State(ctx), gate).await.into_response()
}


pub async fn night_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> Response {
    match extras::set_night(&ctx, gate.chat, None).await {
        Ok(rights::DeliveryOutcome::Applied) => {}
        Ok(rights::DeliveryOutcome::PendingRetry { retry_at, .. }) => {
            return (
                StatusCode::ACCEPTED,
                Json(json!({ "pending": true, "retry_at": retry_at })),
            )
                .into_response();
        }
        Ok(rights::DeliveryOutcome::AcceptedDeliveryUnknown { reason }) => {
            ::log::warn!(
                "miniapp: night delivery state for {} is unknown: {reason}",
                gate.chat
            );
            return (
                StatusCode::ACCEPTED,
                Json(json!({ "pending": true, "delivery": "unknown" })),
            )
                .into_response();
        }
        Ok(rights::DeliveryOutcome::Superseded) => {
            return (StatusCode::ACCEPTED, Json(json!({ "pending": true }))).into_response();
        }
        Err(error) if error.acceptance_unknown() => {
            return uncertain_write(gate.chat, "night disable", &error);
        }
        Err(error) => return write_error(gate.chat, "night disable", &error),
    }
    dashboard(State(ctx), gate.into()).await.into_response()
}

pub async fn report_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> Response {
    if let Err(error) = stats::set_report_at(&ctx, gate.chat, None).await {
        return settings_write_error(gate.chat, "daily report disable", &error);
    }
    dashboard(State(ctx), gate.into()).await.into_response()
}

pub async fn purge_off(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> Response {
    if let Err(error) = purge::set_auto_at(&ctx, gate.chat, None).await {
        return settings_write_error(gate.chat, "auto purge disable", &error);
    }
    dashboard(State(ctx), gate.into()).await.into_response()
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
        Err(handlers::promote::DemoteError::State(error)) if error.commit_outcome_unknown() => {
            ::log::warn!(
                "miniapp: Telegram demoted {user_id} in {}, but settings commit is unknown: {error}",
                gate.chat
            );
            (
                StatusCode::ACCEPTED,
                Json(json!({
                    "accepted": "unknown",
                    "error": "کاربر در تلگرام عزل شد، اما نتیجه ثبت آن نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                })),
            )
                .into_response()
        }
        Err(handlers::promote::DemoteError::State(error)) => {
            ::log::warn!(
                "miniapp: Telegram demoted {user_id} in {}, but settings were not saved: {error}",
                gate.chat
            );
            admin_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "کاربر در تلگرام عزل شد، اما وضعیت ربات ذخیره نشد؛ دوباره تلاش نکنید و گزارش دهید.",
            )
        }
        Err(_) => admin_error(
            StatusCode::BAD_GATEWAY,
            "انجام نشد. ربات فقط می تواند ادمین هایی را عزل کند که خودش اضافه کرده است.",
        ),
    }
}
