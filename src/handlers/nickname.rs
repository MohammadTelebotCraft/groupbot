use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerKind, PeerRef};

use super::Ctx;
use crate::response::ResponseKind;
use crate::state::{SettingMutation, SettingsWriteError};

pub const PREFIX: &str = "nick:";

pub const SET: &[&str] = &["تنظیم لقب", "ثبت لقب", "/setnick"];
pub const REMOVE: &[&str] = &["حذف لقب", "پاک کردن لقب", "/delnick"];

const MAX_PER_CHAT: usize = 50;
const MAX_CHARS: usize = 32;

fn key(user: i64) -> String {
    format!("{PREFIX}{user}")
}

pub fn address(ctx: &Ctx, message: &Message, reply: InputMessage) -> InputMessage {
    let Some(chat) = message.peer_id().bot_api_dialog_id().filter(|chat| *chat < 0) else {
        return reply;
    };
    let Some(user) = message
        .sender_id()
        .filter(|sender| sender.kind() == PeerKind::User)
        .and_then(PeerId::bare_id)
    else {
        return reply;
    };
    match ctx.settings.value(chat, &key(user)) {
        Some(nick) if !nick.is_empty() => reply.prepend_text(&format!("{nick}، ")),
        _ => reply,
    }
}

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if let Some(rest) = after(view.command(), REMOVE) {
        let Some(named) = super::named(message, (!rest.is_empty()).then_some(rest)) else {
            return false;
        };
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        let Some((user, name)) = person(ctx, message, named).await else {
            return true;
        };
        let key = key(user);
        if ctx.settings.value(chat, &key).is_none() {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                format!("{name} لقبی ندارد."),
            )
            .await;
            return true;
        }
        match ctx
            .settings
            .try_apply_batch(chat, &[SettingMutation::Delete { key: &key }])
            .await
        {
            Ok(_) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::SettingsChanged,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Delete),
                        format!("لقب {name} برداشته شد."),
                    ),
                )
                .await
            }
            Err(error) => {
                ::log::warn!("nickname: delete for {chat}/{user} failed: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    failure_text(&error, "لقب حذف نشد؛ دوباره تلاش کنید."),
                )
                .await
            }
        }
        return true;
    }

    if let Some(rest) = after(view.text(), SET) {
        let Some(named) = super::named(message, None) else {
            return false;
        };
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        let Some((user, name)) = person(ctx, message, named).await else {
            return true;
        };
        let nick = rest.split_whitespace().collect::<Vec<_>>().join(" ");
        let refusal = if nick.is_empty() {
            Some("لقب را بعد از دستور بنویسید؛ مثل «تنظیم لقب سلطان».".to_owned())
        } else if nick.chars().count() > MAX_CHARS {
            Some(format!("لقب حداکثر {MAX_CHARS} حرف است."))
        } else if ctx.settings.value(chat, &key(user)).is_none()
            && ctx.settings.values_with_prefix(chat, PREFIX).len() >= MAX_PER_CHAT
        {
            Some(format!(
                "این گروه {MAX_PER_CHAT} لقب دارد؛ برای لقب تازه اول یکی را حذف کنید."
            ))
        } else {
            None
        };
        if let Some(refusal) = refusal {
            super::respond(ctx, message, ResponseKind::CommandError, refusal).await;
            return true;
        }
        match ctx.settings.try_set_value(chat, &key(user), &nick).await {
            Ok(_) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::SettingsChanged,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Success),
                        format!("از این به بعد {name} را «{nick}» صدا می زنم."),
                    ),
                )
                .await
            }
            Err(error) => {
                ::log::warn!("nickname: write for {chat}/{user} failed: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    failure_text(&error, "لقب ذخیره نشد؛ دوباره تلاش کنید."),
                )
                .await
            }
        }
        return true;
    }

    false
}

async fn person(
    ctx: &Ctx,
    message: &Message,
    named: super::Named<'_>,
) -> Option<(i64, String)> {
    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.",
        )
        .await;
        return None;
    };
    Some((user_of(&target)?, name))
}

fn user_of(target: &PeerRef) -> Option<i64> {
    (target.id.kind() == PeerKind::User)
        .then(|| target.id.bare_id())
        .flatten()
}

fn after<'a>(text: &'a str, commands: &[&str]) -> Option<&'a str> {
    commands
        .iter()
        .filter_map(|command| super::commands::strip_prefix(text, command))
        .min_by_key(|rest| rest.len())
}

fn failure_text(error: &SettingsWriteError, rejected: &str) -> String {
    if error.commit_outcome_unknown() {
        "نتیجه ذخیره سازی نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید.".to_owned()
    } else {
        rejected.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phrase_carries_the_nickname_and_nothing_shorter_claims_it() {
        assert_eq!(after("تنظیم لقب  سلطان   جنگل", SET), Some("سلطان   جنگل"));
        assert_eq!(after("تنظیم لقب", SET), Some(""));
        assert_eq!(after("تنظیم لقبش", SET), None);
        assert_eq!(after("حذف لقب @ali", REMOVE), Some("@ali"));
        assert_eq!(after("لقب", SET), None);
    }
}
