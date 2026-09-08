
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
    match setting::apply(&ctx, gate.chat, &body.action).await {
        Ok(Some((_, setting::ApplyStatus::Applied))) => {}
        Ok(Some((_, setting::ApplyStatus::PendingRetry))) => {
            return (StatusCode::ACCEPTED, axum::Json(json!({ "pending": true }))).into_response();
        }
        Ok(Some((_, setting::ApplyStatus::DeliveryUnknown))) => {
            return (
                StatusCode::ACCEPTED,
                axum::Json(json!({ "pending": true, "delivery": "unknown" })),
            )
                .into_response();
        }
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": "مقدار پذیرفته نشد." })),
            )
                .into_response();
        }
        Err(error) if error.invalid_night_window() => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({ "error": "شروع و پایان قفل شب نمی‌تواند یکسان باشد." })),
            )
                .into_response();
        }
        Err(error) if error.acceptance_unknown() => {
            ::log::warn!(
                "miniapp: settings write outcome for {} is unknown: {error}",
                gate.chat
            );
            return (
                StatusCode::ACCEPTED,
                axum::Json(json!({ "accepted": "unknown" })),
            )
                .into_response();
        }
        Err(error) => {
            ::log::warn!("miniapp: settings write for {} failed: {error}", gate.chat);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({ "error": "تنظیم ذخیره نشد؛ دوباره تلاش کنید." })),
            )
                .into_response();
        }
    }
    dashboard(State(ctx), gate.into()).await.into_response()
}
