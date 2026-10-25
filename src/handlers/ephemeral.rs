use grammers_client::message::Message;

use super::{Ctx, limits};
use crate::response::{ResponseKind, VisibilityOverride};

pub const COMMANDS: &[&str] = &[
    "پیام های ربات",
    "پیام ربات",
    "کاهش شلوغی",
    "/ephemeral",
    "/botmessages",
];

fn tail(text: &str) -> Option<&str> {
    COMMANDS.iter().find_map(|command| {
        let rest = super::commands::strip_prefix(text, command)?;
        (command.starts_with('/') || valid_action(rest)).then_some(rest)
    })
}

fn valid_action(action: &str) -> bool {
    let normalized = super::commands::normalize(action);
    matches!(
        normalized.as_ref(),
        "" | "status" | "وضعیت" | "reset" | "بازنشانی" | "off" | "خاموش"
    ) || parse_choice(&normalized).is_some()
}

fn parse_choice(value: &str) -> Option<(ResponseKind, VisibilityOverride)> {
    let normalized = super::commands::normalize(value);
    let mut parts = normalized.rsplitn(2, ' ');
    let state = parts.next()?;
    let family = parts.next()?;
    let kind = match family {
        "notice" | "notices" | "اعلان" | "اعلان ها" | "اعلانها" => {
            ResponseKind::ContentRemovalNotice
        }
        "welcome" | "خوشامد" | "خوش آمد" => ResponseKind::WelcomeNotice,
        _ => return None,
    };
    let visibility = match state {
        "private" | "خصوصی" => VisibilityOverride::Private,
        "public" | "normal" | "عمومی" | "عادی" => VisibilityOverride::Public,
        _ => return None,
    };
    parts.next().is_none().then_some((kind, visibility))
}

fn status(ctx: &Ctx, chat: i64) -> String {
    let view = crate::response::policy_view(&ctx.settings, chat);
    let notice = &view.overrides[0];
    let welcome = &view.overrides[1];
    format!(
        "<b>پیام های ربات</b>\n\nاعلان ها: <b>{}</b>\nخوشامد: <b>{}</b>\n\nبقیه پیام ها همیشه عادی هستند.\n\n<code>/ephemeral notice private</code>\n<code>/ephemeral welcome public</code>\n<code>/ephemeral reset</code>",
        visibility_label(notice.visibility),
        visibility_label(welcome.visibility),
    )
}

fn visibility_label(value: VisibilityOverride) -> &'static str {
    match value {
        VisibilityOverride::Private => "خصوصی",
        VisibilityOverride::Default | VisibilityOverride::Public => "عمومی",
    }
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let Some(action) = tail(message.text().trim()) else {
        return false;
    };
    if !limits::allows(ctx, message, limits::SET).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return true;
    };

    let normalized = super::commands::normalize(action);
    let result = match normalized.as_ref() {
        "" | "status" | "وضعیت" => None,
        "reset" | "بازنشانی" => match crate::response::reset(&ctx.settings, chat).await {
            Ok(_) => Some("اعلان ها و خوشامد به حالت عمومی برگشتند؛ بقیه پیام ها عادی می مانند."),
            Err(error) => {
                ::log::warn!("response policy: reset failed: {error}");
                Some("سیاست پیام ها ذخیره نشد؛ دوباره تلاش کنید.")
            }
        },
        "off" | "خاموش" => match crate::response::reset(&ctx.settings, chat).await {
            Ok(_) => Some("اعلان ها و خوشامد عمومی شدند؛ بقیه پیام ها عادی می مانند."),
            Err(error) => {
                ::log::warn!("response policy: reset failed: {error}");
                Some("حالت پیام ها ذخیره نشد؛ دوباره تلاش کنید.")
            }
        },
        value => match parse_choice(value) {
            Some((kind, visibility)) => {
                match crate::response::set_kind(&ctx.settings, chat, kind, visibility).await {
                    Ok(_) => Some(if visibility == VisibilityOverride::Private {
                        "این پیام از این پس فقط برای همان کاربر نمایش داده می شود."
                    } else {
                        "این پیام از این پس به شکل عادی در گروه ارسال می شود."
                    }),
                    Err(error) => {
                        ::log::warn!("response policy: visibility write failed: {error}");
                        Some("حالت پیام ها ذخیره نشد؛ دوباره تلاش کنید.")
                    }
                }
            }
            None => Some(
                "فقط اعلان ها و خوشامد قابل انتخاب اند. نمونه: /ephemeral notice private یا /ephemeral welcome public",
            ),
        },
    };

    if let Some(result) = result {
        super::respond(ctx, message, ResponseKind::SettingsChanged, result).await;
    } else {
        super::respond(
            ctx,
            message,
            ResponseKind::SettingsView,
            grammers_client::message::InputMessage::new().html(status(ctx, chat)),
        )
        .await;
    }
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn visibility_aliases_normalize_closed_choices() {
        assert_eq!(
            tail("پيام\u{200c}های\tربات اعلان\u{200c}ها خصوصی"),
            Some("اعلان\u{200c}ها خصوصی")
        );
        assert_eq!(
            parse_choice("اعلان\u{200c}ها خصوصي"),
            Some((
                ResponseKind::ContentRemovalNotice,
                VisibilityOverride::Private
            ))
        );
        assert_eq!(
            parse_choice("WELCOME\tPUBLIC"),
            Some((ResponseKind::WelcomeNotice, VisibilityOverride::Public))
        );
        assert_eq!(
            parse_choice("خوش آمد عمومی"),
            Some((ResponseKind::WelcomeNotice, VisibilityOverride::Public))
        );
        assert_eq!(parse_choice("اعلان ها خصوصی بیشتر"), None);
        assert_eq!(tail("/ephemeral@otherbot reset"), None);
    }
    use super::*;

    #[test]
    fn command_tail_is_closed_and_only_two_families_are_selectable() {
        assert_eq!(tail("/ephemeral"), Some(""));
        assert_eq!(tail("/ephemeral notice private"), Some("notice private"));
        assert_eq!(tail("/ephemeral unknown"), Some("unknown"));
        assert_eq!(tail("پیام ربات خوبه"), None);
        assert_eq!(tail("پیام های ربات چیه"), None);
        assert_eq!(tail("کاهش شلوغی رو میگی"), None);
        assert_eq!(tail("/ephemerally"), None);
        assert_eq!(
            parse_choice("notice private"),
            Some((
                ResponseKind::ContentRemovalNotice,
                VisibilityOverride::Private
            ))
        );
        assert_eq!(
            parse_choice("welcome public"),
            Some((ResponseKind::WelcomeNotice, VisibilityOverride::Public))
        );
        assert_eq!(parse_choice("help private"), None);
    }
}
