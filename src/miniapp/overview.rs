use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use serde_json::{Value, json};

use crate::handlers::{self, Ctx, extras, install, log, stats};

use super::auth::{AdminGate, UserGate};

const GROUP_CAP: i64 = 60;

fn title_of(ctx: &Ctx, chat: i64) -> String {
    handlers::esc(
        &ctx.settings
            .value(chat, handlers::TITLE)
            .unwrap_or_else(|| chat.to_string()),
    )
}

pub async fn groups(State(ctx): State<Arc<Ctx>>, gate: UserGate) -> impl IntoResponse {
    let chats = ctx.settings.panels_for(gate.user, GROUP_CAP).await;
    let groups: Vec<Value> = chats
        .into_iter()
        .map(|chat| {
            json!({
                "id": chat,
                "title": title_of(&ctx, chat),
                "is_owner": handlers::owner(&ctx, chat) == Some(gate.user),

                "known": ctx.chat_ref(chat).is_some(),
            })
        })
        .collect();
    Json(json!({ "groups": groups }))
}

fn issue(severity: &str, title: &str, detail: &str, fix: &str) -> Value {
    json!({ "severity": severity, "title": title, "detail": detail, "fix": fix })
}

pub async fn health(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let chat = gate.chat;
    let mut issues: Vec<Value> = Vec::new();

    let standing = match ctx.chat_ref(chat) {
        Some(chat_ref) => install::standing(&ctx, chat_ref).await,
        None => install::Standing::Unknown,
    };
    let missing = install::missing(&standing);
    match standing {
        install::Standing::Gone | install::Standing::Member => {
            issues.push(issue(
                "bad",
                "ربات دیگر ادمین نیست",
                "هیچ قفل و فیلتری کار نمی کند تا دوباره ادمین شود",
                "rights",
            ));
        }
        install::Standing::Basic { admin: false } => {
            issues.push(issue(
                "bad",
                "ربات در این گروه ادمین نیست",
                "هیچ قفل و فیلتری کار نمی کند",
                "rights",
            ));
        }
        _ => {
            for (bit, need) in install::NEEDED.iter().enumerate() {
                if missing & (1 << bit) != 0 {
                    issues.push(issue(
                        "bad",
                        &format!("ربات دسترسی «{}» ندارد", need.label),
                        &format!("{} از کار افتاده", need.for_what),
                        "rights",
                    ));
                }
            }
        }
    }

    if ctx.settings.value(chat, extras::NIGHT).is_some() && extras::night(&ctx, chat).is_none() {
        issues.push(issue(
            "warn",
            "قفل شب نیمه تنظیم است",
            "ساعت شروع و پایان یکی است، پس هیچ وقت اجرا نمی شود",
            "feature:ng",
        ));
    }

    let any_kind = log::KINDS
        .iter()
        .any(|(key, _)| ctx.settings.is_locked(chat, key));
    if any_kind && log::channel_id(&ctx, chat).is_none() {
        issues.push(issue(
            "warn",
            "لاگ کانال ندارد",
            "رویدادها روشن است اما جایی ثبت نمی شود؛ از داخل گروه «تنظیم لاگ» بفرستید",
            "log",
        ));
    }

    Json(json!({
        "issues": issues,
        "bot_admin": !matches!(
            standing,
            install::Standing::Gone | install::Standing::Member | install::Standing::Basic { admin: false }
        ),
    }))
}

const COUNTERS: &[(&str, &str)] = &[
    (stats::DELETED, "پیام حذف شده"),
    (stats::MUTED, "سکوت"),
    (stats::BANNED, "بن"),
    (stats::WARNED, "اخطار"),
    (stats::JOINED, "عضو تازه"),
    (stats::LEFT, "خروج"),
    (stats::CAPTCHA_PASSED, "احراز موفق"),
    (stats::CAPTCHA_FAILED, "احراز ناموفق"),
];

const DAYS: u64 = 7;

pub async fn activity(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let today = stats::today();
    let mut days: Vec<Value> = Vec::with_capacity(DAYS as usize);
    for back in 0..DAYS {
        let day = today - back;
        let tallies = ctx.settings.tallies(gate.chat, day).await;
        let counters: Vec<Value> = COUNTERS
            .iter()
            .map(|(key, label)| {
                json!({ "key": key, "label": label, "count": tallies.get(*key).copied().unwrap_or(0) })
            })
            .collect();
        days.push(json!({ "day": day, "ago": back, "counters": counters }));
    }
    Json(json!({ "days": days }))
}
