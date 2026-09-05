use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::handlers::{Ctx, limits, lists};

use super::auth::AdminGate;
use super::throttle::throttled;

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
    if throttled(gate.chat, &format!("lists:list:{kind_name}"), LIST_COOLDOWN) {
        return too_many_requests();
    }
    let chat_ref = match gated_chat_ref(&ctx, gate, kind).await {
        Ok(chat_ref) => chat_ref,
        Err(status) => return status.into_response(),
    };
    let all = lists::entries(&ctx, chat_ref, gate.chat, kind, MINIAPP_LIST_CAP).await;
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

    if throttled(gate.chat, &format!("lists:remove:{kind_name}"), LIST_COOLDOWN) {
        return too_many_requests();
    }
    let chat_ref = match gated_chat_ref(&ctx, gate, kind).await {
        Ok(chat_ref) => chat_ref,
        Err(status) => return status.into_response(),
    };
    lists::remove(&ctx, chat_ref, gate.chat, kind, &entry_key).await;
    StatusCode::NO_CONTENT.into_response()
}

pub async fn clear(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(kind_name): Path<String>,
) -> Response {
    let Some(kind) = lists::Kind::from_action(&kind_name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if throttled(gate.chat, &format!("lists:clear:{kind_name}"), LIST_COOLDOWN) {
        return too_many_requests();
    }
    let chat_ref = match gated_chat_ref(&ctx, gate, kind).await {
        Ok(chat_ref) => chat_ref,
        Err(status) => return status.into_response(),
    };
    let removed = lists::clear_all(&ctx, chat_ref, gate.chat, kind).await;
    Json(json!({ "removed": removed })).into_response()
}
