
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::handlers::{Ctx, limits, lists};

use super::auth::AdminGate;

const MINIAPP_LIST_CAP: usize = 300;

const LIST_COOLDOWN: Duration = Duration::from_secs(1);

async fn gated_chat_ref(
    ctx: &Ctx,
    gate: AdminGate,
    kind: lists::Kind,
) -> Result<grammers_client::session::types::PeerRef, StatusCode> {
    if !limits::permits(ctx, gate.chat, gate.user, kind.cap()) {
        return Err(StatusCode::FORBIDDEN);
    }
    ctx.chat_ref(gate.chat).ok_or(StatusCode::NOT_FOUND)
}

fn too_many_requests() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": "کمی صبر کنید و دوباره امتحان کنید." })),
    )
        .into_response()
}

pub async fn list(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(kind_name): Path<String>,
) -> Response {
    let Some(kind) = lists::Kind::from_action(&kind_name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !ctx.claim_miniapp_list_read(gate.chat, kind, LIST_COOLDOWN) {
        return too_many_requests();
    }
    let chat_ref = match gated_chat_ref(&ctx, gate, kind).await {
        Ok(chat_ref) => chat_ref,
        Err(status) => return status.into_response(),
    };
    let all = match lists::entries(&ctx, chat_ref, gate.chat, kind, MINIAPP_LIST_CAP).await {
        Ok(entries) => entries,
        Err(error) => {
            ::log::warn!(
                "miniapp: could not read {} for {}: {error}",
                kind.title(),
                gate.chat
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "لیست فعلاً در دسترس نیست؛ دوباره تلاش کنید." })),
            )
                .into_response();
        }
    };
    let truncated = all.len() >= MINIAPP_LIST_CAP;
    let entries: Vec<_> = all
        .iter()
        .map(|entry| json!({ "key": entry.key, "name": entry.name }))
        .collect();
    Json(json!({
        "kind": kind_name,
        "title": kind.title(),
        "entries": entries,
        "truncated": truncated,
    }))
    .into_response()
}

pub async fn remove(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path((kind_name, entry_key)): Path<(String, String)>,
) -> Response {
    let Some(kind) = lists::Kind::from_action(&kind_name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !ctx.claim_miniapp_list_remove(gate.chat, kind, LIST_COOLDOWN) {
        return too_many_requests();
    }
    let chat_ref = match gated_chat_ref(&ctx, gate, kind).await {
        Ok(chat_ref) => chat_ref,
        Err(status) => return status.into_response(),
    };
    match lists::remove(&ctx, chat_ref, gate.chat, kind, &entry_key).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            ::log::warn!(
                "miniapp: remove from {} for {} failed: {error}",
                kind.title(),
                gate.chat
            );
            let status = if error.commit_outcome_unknown() {
                StatusCode::ACCEPTED
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            let message = if error.commit_outcome_unknown() {
                "نتیجه حذف مورد نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
            } else {
                "مورد حذف نشد؛ دوباره تلاش کنید."
            };
            (status, Json(json!({ "error": message }))).into_response()
        }
    }
}

pub async fn clear(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(kind_name): Path<String>,
) -> Response {
    let Some(kind) = lists::Kind::from_action(&kind_name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !ctx.claim_miniapp_list_clear(gate.chat, kind, LIST_COOLDOWN) {
        return too_many_requests();
    }
    let chat_ref = match gated_chat_ref(&ctx, gate, kind).await {
        Ok(chat_ref) => chat_ref,
        Err(status) => return status.into_response(),
    };
    match lists::clear_all(&ctx, chat_ref, gate.chat, kind).await {
        Ok(removed) => Json(json!({ "removed": removed })).into_response(),
        Err(error) => {
            ::log::warn!(
                "miniapp: clear {} for {} failed: {error}",
                kind.title(),
                gate.chat
            );
            let status = if error.commit_outcome_unknown() {
                StatusCode::ACCEPTED
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            let message = if error.commit_outcome_unknown() {
                "نتیجه پاکسازی نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
            } else {
                "پاکسازی کامل نشد؛ دوباره تلاش کنید."
            };
            (status, Json(json!({ "error": message }))).into_response()
        }
    }
}
