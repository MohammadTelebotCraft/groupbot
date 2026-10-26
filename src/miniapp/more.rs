use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{
    self, Ctx, answers, bots, extras, limits, log, panel, stats, strict, tempmedia, toggles,
    voicemonitor,
};
use crate::state::{SettingMutation, SettingsWriteError};

use super::auth::AdminGate;

fn failed(chat: i64, operation: &str, error: &SettingsWriteError) -> Response {
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

fn refused(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

pub async fn tempmedia_get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let kinds: Vec<_> = tempmedia::KINDS
        .iter()
        .map(|kind| {
            json!({ "name": kind.name, "label": kind.label, "on": tempmedia::temporary(&ctx, gate.chat, kind) })
        })
        .collect();
    Json(json!({ "kinds": kinds }))
}

pub async fn tempmedia_toggle(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(name): Path<String>,
) -> Response {
    let Some(kind) = tempmedia::find(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let keep = tempmedia::temporary(&ctx, gate.chat, kind);
    if let Err(error) = ctx.settings.try_set(gate.chat, kind.key, keep).await {
        return failed(gate.chat, "temporary media toggle", &error);
    }
    tempmedia_get(State(ctx), gate).await.into_response()
}

pub async fn limits_get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let caps: Vec<_> = limits::CAPS
        .iter()
        .map(|cap| {
            json!({ "name": cap.name, "label": cap.label, "allowed": !ctx.settings.is_locked(gate.chat, cap.key) })
        })
        .collect();
    Json(json!({ "caps": caps, "can_edit": gate.is_owner }))
}

pub async fn limits_toggle(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(name): Path<String>,
) -> Response {
    if !gate.is_owner {
        return refused(
            StatusCode::FORBIDDEN,
            "محدودیت مدیران فقط از مالک ربات پذیرفته می شود.",
        );
    }
    let Some(cap) = limits::find(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let deny = !ctx.settings.is_locked(gate.chat, cap.key);
    if let Err(error) = ctx.settings.try_set(gate.chat, cap.key, deny).await {
        return failed(gate.chat, "admin limit toggle", &error);
    }
    limits_get(State(ctx), gate).await.into_response()
}

pub async fn bot_lock_get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let items: Vec<_> = toggles::BOT
        .items
        .iter()
        .map(|(key, label)| json!({ "key": key, "label": label, "on": ctx.settings.is_locked(gate.chat, key) }))
        .collect();
    Json(json!({ "items": items }))
}

pub async fn bot_lock_toggle(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(key): Path<String>,
) -> Response {
    let Some(&(key, _)) = toggles::BOT.items.iter().find(|(item, _)| *item == key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let now_on = !ctx.settings.is_locked(gate.chat, key);
    match ctx.settings.try_set(gate.chat, key, now_on).await {
        Ok(true) => bots::on_lock_set(&ctx, gate.chat, key, now_on).await,
        Ok(false) => {}
        Err(error) => return failed(gate.chat, "bot lock option", &error),
    }
    bot_lock_get(State(ctx), gate).await.into_response()
}

pub async fn rules_get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    Json(json!({ "text": extras::rules(&ctx, gate.chat).map(|text| handlers::esc(&text)) }))
}

#[derive(Deserialize)]
pub struct TextBody {
    text: String,
}

const RULES_MAX: usize = 3_500;

pub async fn rules_set(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<TextBody>,
) -> Response {
    let text = body.text.trim();
    let result = if text.is_empty() {
        ctx.settings.try_set(gate.chat, extras::RULES, false).await.map(|_| ())
    } else if text.chars().count() > RULES_MAX {
        return refused(StatusCode::BAD_REQUEST, "متن قوانین حداکثر ۳۵۰۰ نویسه است.");
    } else {
        ctx.settings
            .try_set_value(gate.chat, extras::RULES, text)
            .await
            .map(|_| ())
    };
    if let Err(error) = result {
        return failed(gate.chat, "rules write", &error);
    }
    rules_get(State(ctx), gate).await.into_response()
}

pub async fn slow_get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let current = ctx
        .settings
        .value(gate.chat, extras::SLOW_STATE)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let steps: Vec<_> = extras::SLOW_STEPS
        .iter()
        .map(|&seconds| json!({ "value": seconds, "label": extras::slow_label(seconds) }))
        .collect();
    Json(json!({ "seconds": current, "steps": steps }))
}

#[derive(Deserialize)]
pub struct SlowBody {
    seconds: u32,
}

pub async fn slow_set(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<SlowBody>,
) -> Response {
    match extras::apply_slow(&ctx, gate.chat, body.seconds).await {
        Ok(Some(true)) => slow_get(State(ctx), gate).await.into_response(),
        Ok(Some(false)) => refused(
            StatusCode::BAD_GATEWAY,
            "تلگرام اسلوموشن را نپذیرفت؛ کلینر باید در گروه ادمین باشد.",
        ),
        Ok(None) => refused(
            StatusCode::CONFLICT,
            "اسلوموشن فقط با کلینر تنظیم می شود. در گروه «افزودن کلینر» را بفرستید.",
        ),
        Err(error) => failed(gate.chat, "slow mode", &error),
    }
}

const REPORT_TEST_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
const BOT_LOOKUP_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(3);

fn too_soon() -> Response {
    refused(
        StatusCode::TOO_MANY_REQUESTS,
        "کمی صبر کنید و دوباره امتحان کنید.",
    )
}

pub async fn report_test(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> Response {
    if !ctx.claim_miniapp_telegram(gate.chat, 1, REPORT_TEST_COOLDOWN) {
        return too_soon();
    }
    let body = match stats::daily_body(&ctx, gate.chat, stats::today()).await {
        Ok(body) => body,
        Err(error) => {
            ::log::warn!("miniapp: report preview for {} failed: {error}", gate.chat);
            return refused(
                StatusCode::SERVICE_UNAVAILABLE,
                "آمار اکنون در دسترس نیست؛ دوباره تلاش کنید.",
            );
        }
    };
    let Some(chat_ref) = ctx.chat_ref(gate.chat) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match ctx
        .client
        .send_message(chat_ref, handlers::premium::html(body))
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            ::log::warn!("miniapp: report preview for {} not sent: {error}", gate.chat);
            refused(
                StatusCode::BAD_GATEWAY,
                "گزارش فرستاده نشد؛ دوباره تلاش کنید.",
            )
        }
    }
}

#[derive(Deserialize)]
pub struct AnswerBody {
    trigger: String,
    text: String,
}

pub async fn answer_add(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<AnswerBody>,
) -> Response {
    match answers::add_text(&ctx, gate.chat, &body.trigger, &body.text).await {
        Ok(trigger) => Json(json!({ "trigger": handlers::esc(&trigger) })).into_response(),
        Err(answers::AddTextError::Refused(text)) => refused(StatusCode::BAD_REQUEST, &text),
        Err(answers::AddTextError::Settings(error)) => failed(gate.chat, "answer add", &error),
    }
}

#[derive(Deserialize)]
pub struct ChannelBody {
    channel: String,
}

pub async fn log_connect(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<ChannelBody>,
) -> Response {
    match log::connect(&ctx, gate.chat, &body.channel).await {
        Ok(()) => super::misc::log_list(State(ctx), gate).await.into_response(),
        Err(log::ConnectError::NotFound) => refused(
            StatusCode::BAD_REQUEST,
            "کانال پیدا نشد. یوزرنیم کانال عمومی را بنویسید، مثل @mychannel. برای کانال خصوصی از داخل گروه «تنظیم لاگ» را روی پیام فورواردی بزنید.",
        ),
        Err(log::ConnectError::Unwritable(error)) => {
            ::log::warn!("miniapp: log channel greeting for {} failed: {error}", gate.chat);
            refused(
                StatusCode::BAD_GATEWAY,
                "ربات نتوانست در آن کانال بنویسد؛ اول ربات را در کانال ادمین کنید.",
            )
        }
        Err(log::ConnectError::Invalid) => {
            refused(StatusCode::BAD_REQUEST, "این کانال معتبر نیست.")
        }
        Err(log::ConnectError::Settings(error)) => failed(gate.chat, "log connect", &error),
    }
}

pub async fn strict_get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let all = ctx.settings.indexed_empty(gate.chat, strict::PICK);
    let causes: Vec<_> = panel::strict_causes(&ctx, gate.chat)
        .into_iter()
        .map(|(key, label)| {
            json!({
                "key": key,
                "label": label,
                "on": all || ctx.settings.is_locked(gate.chat, &strict::pick_key(key)),
            })
        })
        .collect();
    Json(json!({ "causes": causes }))
}

pub async fn strict_toggle(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(cause): Path<String>,
) -> Response {
    if let Err(error) = panel::strict_pick(&ctx, gate.chat, &cause).await {
        return failed(gate.chat, "strict pick", &error);
    }
    strict_get(State(ctx), gate).await.into_response()
}

const BOT_NAME: &str = "botname:";

pub async fn allowed_bots_get(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let bots: Vec<_> = ctx
        .settings
        .flags_with_prefix(gate.chat, bots::ALLOWED)
        .into_iter()
        .map(|id| {
            let name = ctx.settings.value(gate.chat, &format!("{BOT_NAME}{id}"));
            json!({ "id": id, "name": name.map(|name| handlers::esc(&name)) })
        })
        .collect();
    Json(json!({ "bots": bots }))
}

#[derive(Deserialize)]
pub struct BotBody {
    username: String,
}

pub async fn allowed_bots_add(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Json(body): Json<BotBody>,
) -> Response {
    let name = body.username.trim();
    let name = name
        .strip_prefix("https://t.me/")
        .or_else(|| name.strip_prefix("t.me/"))
        .unwrap_or(name)
        .trim_start_matches('@');
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return refused(StatusCode::BAD_REQUEST, "یوزرنیم ربات را بنویسید، مثل @examplebot");
    }
    if !ctx.claim_miniapp_telegram(gate.chat, 2, BOT_LOOKUP_COOLDOWN) {
        return too_soon();
    }
    let peer = match ctx.client.resolve_username(name).await {
        Ok(Some(peer)) => peer,
        _ => return refused(StatusCode::BAD_REQUEST, "رباتی با این یوزرنیم پیدا نشد."),
    };
    let grammers_client::peer::Peer::User(user) = &peer else {
        return refused(StatusCode::BAD_REQUEST, "این یوزرنیم مال یک ربات نیست.");
    };
    if !user.is_bot() {
        return refused(StatusCode::BAD_REQUEST, "این یوزرنیم مال یک ربات نیست.");
    }
    let Some(id) = user.id().bare_id() else {
        return refused(StatusCode::BAD_REQUEST, "رباتی با این یوزرنیم پیدا نشد.");
    };
    let shown = format!("@{}", user.username().unwrap_or(name));
    let allow = bots::allow_key(id);
    let name_key = format!("{BOT_NAME}{id}");
    let result = ctx
        .settings
        .try_apply_batch(
            gate.chat,
            &[
                SettingMutation::Put { key: &allow, value: "" },
                SettingMutation::Put { key: &name_key, value: &shown },
            ],
        )
        .await;
    if let Err(error) = result {
        return failed(gate.chat, "allowed bot add", &error);
    }
    allowed_bots_get(State(ctx), gate).await.into_response()
}

pub async fn allowed_bots_remove(
    State(ctx): State<Arc<Ctx>>,
    gate: AdminGate,
    Path(id): Path<i64>,
) -> Response {
    let allow = bots::allow_key(id);
    let name_key = format!("{BOT_NAME}{id}");
    let result = ctx
        .settings
        .try_apply_batch(
            gate.chat,
            &[
                SettingMutation::Delete { key: &allow },
                SettingMutation::Delete { key: &name_key },
            ],
        )
        .await;
    if let Err(error) = result {
        return failed(gate.chat, "allowed bot remove", &error);
    }
    allowed_bots_get(State(ctx), gate).await.into_response()
}

pub async fn voice_clear(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> Response {
    if let Err(error) = voicemonitor::clear_all_words(&ctx, gate.chat).await {
        return failed(gate.chat, "voice words clear", &error);
    }
    super::voice::list(State(ctx), gate).await.into_response()
}
