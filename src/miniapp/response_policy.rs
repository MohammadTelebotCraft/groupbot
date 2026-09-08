
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::Ctx;
use crate::response::{ResponseKind, VisibilityOverride, policy_view, reset, set_kind};

use super::auth::AdminGate;

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum PolicyAction {
    SetKind {
        kind: ResponseKind,
        visibility: VisibilityOverride,
    },
    Reset,
}

pub async fn get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    axum::Json(policy_view(&ctx.settings, gate.chat))
}

pub async fn apply(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    axum::Json(action): axum::Json<PolicyAction>,
) -> Response {
    let result = match action {
        PolicyAction::SetKind { kind, visibility } => {
            if !ResponseKind::OVERRIDABLE.contains(&kind)
                || visibility == VisibilityOverride::Default
            {
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({
                        "error": "این نوع پاسخ قابل تنظیم نیست."
                    })),
                )
                    .into_response();
            }
            set_kind(&ctx.settings, gate.chat, kind, visibility).await
        }
        PolicyAction::Reset => reset(&ctx.settings, gate.chat).await,
    };

    match result {
        Ok(_) => axum::Json(policy_view(&ctx.settings, gate.chat)).into_response(),
        Err(error) => {
            ::log::warn!(
                "miniapp: response policy write for {} failed: {error}",
                gate.chat
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({
                    "error": "تنظیم پیام های ربات ذخیره نشد؛ دوباره تلاش کنید."
                })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_are_a_closed_typed_surface() {
        assert!(matches!(
            serde_json::from_str::<PolicyAction>(
                r#"{"action":"set_kind","kind":"welcome_notice","visibility":"private"}"#
            ),
            Ok(PolicyAction::SetKind {
                kind: ResponseKind::WelcomeNotice,
                visibility: VisibilityOverride::Private
            })
        ));
        assert!(
            serde_json::from_str::<PolicyAction>(r#"{"action":"set_mode","mode":"all"}"#).is_err()
        );
        assert!(
            serde_json::from_str::<PolicyAction>(
                r#"{"action":"set_setting","key":"arbitrary","value":"true"}"#
            )
            .is_err()
        );
    }
}
