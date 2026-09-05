use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{Ctx, lists, voicemonitor};

use super::auth::AdminGate;

pub async fn list(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let active: Vec<_> = voicemonitor::words(&ctx, gate.chat)
        .into_iter()
        .map(|word| json!({ "key": lists::word_id(&word), "word": word }))
        .collect();
    let disabled_defaults: Vec<_> = voicemonitor::disabled_default_words(&ctx, gate.chat)
        .into_iter()
        .map(|word| json!({ "key": lists::word_id(&word), "word": word }))
        .collect();
    Json(json!({ "words": active, "disabled_defaults": disabled_defaults }))
}

#[derive(Deserialize)]
pub struct AddBody {
    word: String,
}

pub async fn add(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<AddBody>,
) -> Response {
    match voicemonitor::add_word(&ctx, gate.chat, &body.word).await {
        Ok(_changed) => list(State(ctx), gate).await.into_response(),
        Err(voicemonitor::AddWordError::Empty) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "این کلمه قابل استفاده نیست." })),
        )
            .into_response(),
        Err(voicemonitor::AddWordError::TooLong) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "این کلمه پذیرفته نمی شود: حداکثر ۶۴ نویسه و بدون «=» باشد." })),
        )
            .into_response(),
        Err(voicemonitor::AddWordError::Full) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "لیست کلمات سفارشی ویس پر است (۲۰۰ کلمه)." })),
        )
            .into_response(),
    }
}

pub async fn remove(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(key): Path<String>,
) -> impl IntoResponse {
    voicemonitor::remove_word(&ctx, gate.chat, &key).await;
    list(State(ctx), gate).await.into_response()
}

pub async fn restore_all(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    voicemonitor::restore_all_defaults(&ctx, gate.chat).await;
    list(State(ctx), gate).await
}
