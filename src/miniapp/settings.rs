use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{Ctx, limits, setting};

use super::auth::AdminGate;
use super::dashboard::dashboard;

#[derive(Deserialize)]
pub struct ApplyBody {
    action: String,
}

pub async fn apply(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    axum::Json(body): axum::Json<ApplyBody>,
) -> Response {
    if body.action.starts_with(limits::MODE) && !gate.is_owner {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({ "error": "محدودیت مدیران فقط از مالک ربات پذیرفته می شود." })),
        )
            .into_response();
    }

    if setting::apply(&ctx, gate.chat, &body.action).await.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": "مقدار پذیرفته نشد." })),
        )
            .into_response();
    }
    dashboard(State(ctx), gate).await.into_response()
}
