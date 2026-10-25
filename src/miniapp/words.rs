use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{Ctx, filters};
use crate::state::SettingsWriteError;

use super::auth::AdminGate;

pub async fn get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    Json(json!({ "exact": ctx.settings.is_locked(gate.chat, filters::EXACT) }))
}

fn settings_error(chat: i64, operation: &str, error: &SettingsWriteError) -> Response {
    ::log::warn!("miniapp: {operation} for {chat} failed: {error}");
    let (status, message) = if error.commit_outcome_unknown() {
        (
            StatusCode::ACCEPTED,
            "نتیجه ذخیره سازی نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید.",
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "تغییر ذخیره نشد؛ دوباره تلاش کنید.",
        )
    };
    (status, Json(json!({ "error": message }))).into_response()
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
    match filters::add(&ctx, gate.chat, &body.word).await {
        Ok(added) => Json(json!({ "added": added })).into_response(),
        Err(filters::AddError::Invalid) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "این کلمه پذیرفته نمی شود: خیلی بلند است یا نویسه غیرمجاز دارد." })),
        )
            .into_response(),
        Err(filters::AddError::Full) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "لیست فیلتر پر است (۲۰۰ کلمه)." })),
        )
            .into_response(),
        Err(filters::AddError::Settings(error)) => {
            settings_error(gate.chat, "filter word add", &error)
        }
    }
}

#[derive(Deserialize)]
pub struct ExactBody {
    on: bool,
}

pub async fn exact(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<ExactBody>,
) -> Response {
    if let Err(error) = ctx
        .settings
        .try_set(gate.chat, filters::EXACT, body.on)
        .await
    {
        return settings_error(gate.chat, "filter mode write", &error);
    }
    get(State(ctx), gate).await.into_response()
}
