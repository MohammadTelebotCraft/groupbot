use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{self, Ctx, limits};

use super::auth::CaseGate;

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
    user_id: Option<i64>,
    before_id: Option<i64>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct ResolveBody {
    action: String,
    note: Option<String>,
}

#[derive(Deserialize)]
pub struct NoteBody {
    note: String,
}

#[derive(Deserialize)]
pub struct ReverseBody {
    note: Option<String>,
}

pub async fn list(
    State(ctx): State<Arc<Ctx>>,
    gate: CaseGate,
    Query(query): Query<ListQuery>,
) -> Response {
    let status = match query.status.as_deref() {
        None | Some("all") => None,
        Some("open" | "resolved" | "reversed") => query.status.as_deref(),
        Some(_) => return (StatusCode::BAD_REQUEST, "وضعیت نامعتبر است.").into_response(),
    };
    match ctx.settings.moderation_cases(
        gate.chat,
        status,
        query.user_id,
        query.before_id,
        query.limit.unwrap_or(25).clamp(1, 100),
    ).await {
        Ok(cases) => Json(json!({ "cases": cases, "has_more": cases.len() == query.limit.unwrap_or(25).clamp(1, 100) as usize })).into_response(),
        Err(error) => {
            log::warn!("miniapp case list failed: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn get(State(ctx): State<Arc<Ctx>>, gate: CaseGate, Path(id): Path<i64>) -> Response {
    match ctx.settings.moderation_case(gate.chat, id).await {
        Ok(Some(case)) => Json(case).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            log::warn!("miniapp case {id} failed: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn resolve(
    State(ctx): State<Arc<Ctx>>,
    gate: CaseGate,
    Path(id): Path<i64>,
    Json(body): Json<ResolveBody>,
) -> Response {
    let note = body.note.as_deref();
    if handlers::cases::valid_note(note).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            "یادداشت باید بین ۱ تا ۵۰۰ نویسه باشد.",
        )
            .into_response();
    }
    let actor_name = gate.user.to_string();
    let actor = Some((gate.user, actor_name.as_str()));
    let message = match body.action.as_str() {
        "none" => handlers::cases::resolve(&ctx, gate.chat, id, actor, note).await,
        "delete" => {
            if !limits::permits(&ctx, gate.chat, gate.user, limits::CLEAN) {
                return StatusCode::FORBIDDEN.into_response();
            }
            if note.is_some()
                && let Some(note) = note.filter(|note| !note.trim().is_empty())
            {
                let _ = ctx
                    .settings
                    .add_moderation_case_note(gate.chat, id, Some(gate.user), &actor_name, note)
                    .await;
            }
            handlers::cases::resolve_delete(&ctx, gate.chat, id, actor).await
        }
        _ => return (StatusCode::BAD_REQUEST, "اقدام نامعتبر است.").into_response(),
    };
    Json(json!({ "message": message })).into_response()
}

pub async fn reverse(
    State(ctx): State<Arc<Ctx>>,
    gate: CaseGate,
    Path(id): Path<i64>,
    Json(body): Json<ReverseBody>,
) -> Response {
    let actor_name = gate.user.to_string();
    let note = body.note.as_deref();
    let message =
        handlers::cases::reverse(&ctx, gate.chat, id, Some((gate.user, &actor_name)), note).await;
    Json(json!({ "message": message })).into_response()
}

pub async fn note(
    State(ctx): State<Arc<Ctx>>,
    gate: CaseGate,
    Path(id): Path<i64>,
    Json(body): Json<NoteBody>,
) -> Response {
    let note = body.note.trim();
    if note.is_empty() || note.chars().count() > handlers::cases::NOTE_MAX {
        return (
            StatusCode::BAD_REQUEST,
            "یادداشت باید بین ۱ تا ۵۰۰ نویسه باشد.",
        )
            .into_response();
    }
    match ctx
        .settings
        .add_moderation_case_note(gate.chat, id, Some(gate.user), &gate.user.to_string(), note)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            log::warn!("miniapp case {id} note failed: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
