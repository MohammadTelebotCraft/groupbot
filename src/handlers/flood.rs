use std::time::Duration;

use grammers_client::message::Message;

use super::restrict::{self, Action};
use super::{Ctx, esc, name_of};
use crate::response::ResponseKind;

pub const MODE: &str = "flood";

pub const LIMIT: &str = "flood_limit";

pub const WINDOW: &str = "flood_window";

pub const ACTION: &str = "flood_action";
pub const LIMIT_RANGE: (u32, u32) = (2, 50);
pub const WINDOW_RANGE: (u32, u32) = (2, 120);
const DEFAULT_LIMIT: u32 = 8;
const DEFAULT_WINDOW: u32 = 10;

pub const LIMIT_PRESETS: &[u32] = &[3, 5, 8, 12, 20];
pub const WINDOW_PRESETS: &[u32] = &[3, 5, 10, 30, 60];

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    number(ctx, chat, LIMIT, DEFAULT_LIMIT, LIMIT_RANGE)
}

pub fn window(ctx: &Ctx, chat: i64) -> u32 {
    number(ctx, chat, WINDOW, DEFAULT_WINDOW, WINDOW_RANGE)
}

pub fn bans(ctx: &Ctx, chat: i64) -> bool {
    ctx.settings.value(chat, ACTION).as_deref() == Some("ban")
}

fn number(ctx: &Ctx, chat: i64, key: &str, default: u32, range: (u32, u32)) -> u32 {
    ctx.settings
        .with_chat(chat, |settings| settings.number(key, default, range))
}

pub const COMMANDS: &[&str] = &["ضد رگبار", "ضدرگبار", "ضد فلاد"];

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let text = view.digits();
    let Some(rest) = COMMANDS.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim().to_owned())
    }) else {
        return false;
    };
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let numbers = match super::numbers_in(&rest) {
        Some(numbers) => numbers,
        None => return false,
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }

    if numbers.len() != 2 {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "مثال: «ضد رگبار 10 5» یعنی ۱۰ پیام در ۵ ثانیه. برای خاموش کردن از پنل استفاده کنید.",
        )
        .await;
        return true;
    }

    let limit_value = numbers[0].clamp(LIMIT_RANGE.0, LIMIT_RANGE.1).to_string();
    let window_value = numbers[1].clamp(WINDOW_RANGE.0, WINDOW_RANGE.1).to_string();
    if let Err(error) = ctx
        .settings
        .try_apply_batch(
            chat,
            &[
                crate::state::SettingMutation::Put {
                    key: LIMIT,
                    value: &limit_value,
                },
                crate::state::SettingMutation::Put {
                    key: WINDOW,
                    value: &window_value,
                },
                crate::state::SettingMutation::Put {
                    key: MODE,
                    value: "",
                },
            ],
        )
        .await
    {
        ::log::warn!("flood: setup for {chat} failed: {error}");
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            if error.commit_outcome_unknown() {
                "نتیجه ذخیره ضد رگبار نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
            } else {
                "ضد رگبار ذخیره نشد؛ دوباره تلاش کنید."
            },
        )
        .await;
        return true;
    }
    super::respond(
        ctx,
        message,
        ResponseKind::AntiSpamControl,
        super::premium::icon_text(
            Some(super::premium::Icon::Timer),
            format!(
                "ضد رگبار روشن شد: بیش از {} پیام در {} ثانیه.",
                limit(ctx, chat),
                window(ctx, chat)
            ),
        ),
    )
    .await;
    true
}

pub async fn check(ctx: &Ctx, message: &Message) -> bool {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    let Some((limit, window, bans)) = ctx.settings.with_chat(chat, |settings| {
        if !settings.is_locked(MODE) {
            return None;
        }
        Some((
            settings.number(LIMIT, DEFAULT_LIMIT, LIMIT_RANGE),
            settings.number(WINDOW, DEFAULT_WINDOW, WINDOW_RANGE),
            settings.value(ACTION) == Some("ban"),
        ))
    }) else {
        return false;
    };
    let Some(user) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };

    let count = ctx.record_message(chat, user, Duration::from_secs(u64::from(window)));
    if count <= limit as usize {
        return false;
    }
    if super::is_exempt(ctx, message).await {
        return false;
    }

    let (Ok(Some(chat_ref)), Ok(Some(target))) =
        (message.peer_ref().await, message.sender_ref().await)
    else {
        log::warn!("flood: {chat}: no usable ref for {user}, cannot restrict");
        return false;
    };
    let action = if bans { Action::Ban } else { Action::Mute };
    if let Err(e) = restrict::apply(
        ctx,
        chat_ref,
        target,
        action,
        None,
        restrict::By {
            reason: "ضد رگبار",
            target_name: &name_of(message),
            case: Some(super::cases::context(
                message,
                "automatic",
                MODE,
                "ضد رگبار",
            )),
            ..Default::default()
        },
    )
    .await
    {
        eprintln!("flood: {chat}: could not restrict {user}: {e}");
        return false;
    }

    if ctx.may_notify_flood(chat, user) {
        let what = if action == Action::Ban {
            "از گروه اخراج شد"
        } else {
            "سکوت شد"
        };
        let _ = message
            .respond(super::premium::icon_html(
                action.icon(),
                format!(
                    "<b>ضد رگبار</b>\n\n<b>{}</b> پیام های پیاپی فرستاد و {what}.",
                    esc(&name_of(message))
                ),
            ))
            .await;
    }
    true
}
