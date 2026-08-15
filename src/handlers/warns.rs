use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::PeerRef;

use super::restrict::{self, Action};
use super::{Ctx, esc, name_of};

pub const LIMIT: &str = "warn_limit";

pub const ACTION: &str = "warn_action";
const DEFAULT_LIMIT: u32 = 3;
pub const LIMIT_RANGE: (u32, u32) = (1, 100);
pub const LIMIT_PRESETS: &[u32] = &[2, 3, 5, 7, 10];

const WARN: &[&str] = &["اخطار", "وارن"];
const UNWARN: &[&str] = &["حذف اخطار", "پاک اخطار", "رفع اخطار"];
const SHOW: &[&str] = &["اخطارها", "لیست اخطار"];

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value(chat, LIMIT)
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(LIMIT_RANGE.0, LIMIT_RANGE.1)
}

pub fn bans(ctx: &Ctx, chat: i64) -> bool {
    ctx.settings.value(chat, ACTION).as_deref() != Some("mute")
}

pub async fn set_limit(ctx: &Ctx, chat: i64, value: u32) {
    let value = value.clamp(LIMIT_RANGE.0, LIMIT_RANGE.1);
    ctx.settings.set_value(chat, LIMIT, &value.to_string()).await;
}

pub async fn count(ctx: &Ctx, chat: i64, user: i64) -> u32 {
    ctx.settings.warns_of(chat, user).await
}

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let text = view.digits();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    let matched = WARN
        .iter()
        .map(|c| (c, Some(true)))
        .chain(UNWARN.iter().map(|c| (c, Some(false))))
        .chain(SHOW.iter().map(|c| (c, None)))
        .find_map(|(command, adding)| {
            let rest = text.strip_prefix(command)?;
            (rest.is_empty() || rest.starts_with(char::is_whitespace))
                .then(|| (adding, rest.trim().to_owned()))
        });
    let Some((adding, arg)) = matched else {
        return false;
    };

    let arg = (!arg.is_empty()).then_some(arg);
    let Some(named) = super::named(message, arg.as_deref()) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::WARN).await {
        return true;
    }

    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        let _ = message
            .reply("کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.")
            .await;
        return true;
    };
    let Some(user) = target.id.bare_id() else {
        return true;
    };

    let current = count(ctx, chat, user).await;
    let limit = limit(ctx, chat);

    match adding {
        None => {
            let _ = message
                .reply(InputMessage::new().html(format!(
                    "<b>اخطارها</b>\n\n{} · <b>{current}</b> از <b>{limit}</b>",
                    esc(&name)
                )))
                .await;
        }
        Some(false) => {
            let next = current.saturating_sub(1);
            store(ctx, chat, user, next).await;
            let _ = message
                .reply(InputMessage::new().html(format!(
                    "✗ یک اخطار از {} کم شد · <b>{next}</b> از <b>{limit}</b>",
                    esc(&name)
                )))
                .await;
        }
        Some(true) => {
            ctx.bump(chat, super::stats::WARNED);
            let next = current + 1;
            let by = super::sender_of(message);
            super::log::write(
                ctx,
                chat,
                "log_warn",
                super::log::Entry {
                    title: "اخطار",
                    target: Some((user, &name)),
                    actor: by.as_ref().map(|(id, name)| (*id, name.as_str())),
                    extra: vec![("شماره", format!("{next} از {limit}"))],
                    ..Default::default()
                },
            )
            .await;
            if next >= limit {
                punish(ctx, message, chat, target, &name, limit).await;
                store(ctx, chat, user, 0).await;
            } else {
                store(ctx, chat, user, next).await;
                let _ = message
                    .reply(InputMessage::new().html(format!(
                        "<b>اخطار</b>\n\n{} · <b>{next}</b> از <b>{limit}</b>\nتوسط · {}",
                        esc(&name),
                        esc(&name_of(message))
                    )))
                    .await;
            }
        }
    }
    true
}

async fn store(ctx: &Ctx, chat: i64, user: i64, value: u32) {
    ctx.settings.set_warns(chat, user, value).await;
}

async fn punish(ctx: &Ctx, message: &Message, chat: i64, target: PeerRef, name: &str, limit: u32) {
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return;
    };
    let action = if bans(ctx, chat) {
        Action::Ban
    } else {
        Action::Mute
    };
    let what = if action == Action::Ban {
        "از گروه اخراج شد"
    } else {
        "سکوت شد"
    };

    let reply = match restrict::apply(ctx, chat_ref, target, action, None, restrict::By { reason: "سقف اخطار", target_name: name, ..Default::default() }).await {
        Ok(()) => format!(
            "<b>اخطار</b>\n\n{} به <b>{limit}</b> اخطار رسید و {what}.",
            esc(name)
        ),
        Err(e) => {
            eprintln!("warns: {chat}: could not punish: {e}");
            esc(&e.told())
        }
    };
    let _ = message.reply(InputMessage::new().html(reply)).await;
}
