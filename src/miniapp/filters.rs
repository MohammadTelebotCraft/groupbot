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
    if !ctx.claim_miniapp_filter_create(gate.chat, CREATE_COOLDOWN) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "کمی صبر کنید و دوباره امتحان کنید." })),
        )
            .into_response();
    }
    let phrase = body.phrase.trim();
    let refused = |text: String| {
        (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "error": text }))).into_response()
    };
    match imgfilter::create_from_phrase(&ctx, gate.chat, phrase).await {
        Ok(imgfilter::Made::Filter { .. }) => StatusCode::NO_CONTENT.into_response(),
        Ok(imgfilter::Made::Lock { name, .. }) => (
            StatusCode::OK,
            Json(json!({ "notice": format!("برای «{phrase}» قفل «{name}» فعال شد.") })),
        )
            .into_response(),
        Ok(imgfilter::Made::Face) => (
            StatusCode::OK,
            Json(json!({ "notice": format!("قفل چهره «{phrase}» فعال شد؛ چهره او از قبل شناخته شده است.") })),
        )
            .into_response(),
        Err(imgfilter::FilterError::Person) => refused(format!(
            "«{phrase}» یک شخص است و از روی نام شناخته نمی شود؛ در گروه روی عکس چهره اش ریپلای کنید و بنویسید «قفل چهره {phrase}»."
        )),
        Err(imgfilter::FilterError::Indistinct(kind)) => refused(format!(
            "«{phrase}» {} است و مدل تصویر آن را از نمونه های مشابهش تشخیص نمی دهد؛ فیلتری ساخته نشد.",
            kind.word()
        )),
        Err(imgfilter::FilterError::NotUnderstood) => refused(format!(
            "«{phrase}» برای مدل تصویر شناخته شده نیست؛ فیلتری ساخته نشد تا عکس های بی ربط حذف نشوند."
        )),
        Err(imgfilter::FilterError::NameTaken) => {
            refused(format!("«{phrase}» نام یک قفل چهره است."))
        }
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
        Err(
            error @ (imgfilter::FilterError::Settings(_) | imgfilter::FilterError::Database(_)),
        ) => {
            ::log::warn!(
                "miniapp: image filter create for {} failed: {error}",
                gate.chat
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "فیلتر ذخیره نشد؛ دوباره تلاش کنید." })),
            )
                .into_response()
        }
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
        imgfilter::Armed::Inert => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "مدل تصویر این موضوع را نمی شناسد و روشن کردنش چیزی را حذف نمی کند؛ حذفش کنید." })),
        )
            .into_response(),
        imgfilter::Armed::Database(error) => {
            ::log::warn!(
                "miniapp: image filter toggle for {} failed: {error}",
                gate.chat
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "فیلتر تغییر نکرد؛ دوباره تلاش کنید." })),
            )
                .into_response()
        }
    }
}
