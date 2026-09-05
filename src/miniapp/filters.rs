use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{Ctx, imgfilter};

use super::auth::AdminGate;
use super::throttle::throttled;

#[derive(Deserialize)]
pub struct CreateBody {
    phrase: String,
}

const CREATE_COOLDOWN: Duration = Duration::from_secs(3);

pub async fn create(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<CreateBody>,
) -> Response {
    if throttled(gate.chat, "filters:create", CREATE_COOLDOWN) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "کمی صبر کنید و دوباره امتحان کنید." })),
        )
            .into_response();
    }
    match imgfilter::create_from_phrase(&ctx, gate.chat, body.phrase.trim()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(imgfilter::FilterError::Invalid) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "این نام پذیرفته نمی شود: تا ۳۲ حرف، بدون : و = و <." })),
        )
            .into_response(),
        Err(imgfilter::FilterError::Full) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "لیست فیلتر تصویری پر است (۸ مورد)." })),
        )
            .into_response(),
        Err(imgfilter::FilterError::ModelUnavailable) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "مدل عمومی تصویر روی این سرور نصب نیست." })),
        )
            .into_response(),
    }
}

pub async fn toggle(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(entry_key): Path<String>,
) -> Response {
    match imgfilter::toggle_live(&ctx, gate.chat, &entry_key).await {
        imgfilter::Armed::Toggled => StatusCode::NO_CONTENT.into_response(),
        imgfilter::Armed::Missing => StatusCode::NOT_FOUND.into_response(),
    }
}
