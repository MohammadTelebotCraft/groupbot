
use grammers_client::message::Message;

use super::{Ctx, limits};
use crate::response::{ResponseKind, VisibilityOverride};

pub const COMMANDS: &[&str] = &["پیام ربات", "کاهش شلوغی", "/ephemeral"];

fn tail(text: &str) -> Option<&str> {
    COMMANDS.iter().find_map(|command| {
        if text == *command {
            Some("")
        } else {
            text.strip_prefix(command)
                .and_then(|rest| rest.strip_prefix(char::is_whitespace))
                .map(str::trim)
        }
    })
}

fn parse_choice(value: &str) -> Option<(ResponseKind, VisibilityOverride)> {
    let mut parts = value.split_whitespace();
    let kind = match parts.next()? {
        "notice" | "notices" | "اعلان" | "اعلان‌ها" | "اعلانها" => {
            ResponseKind::ContentRemovalNotice
        }
        "welcome" | "خوشامد" => ResponseKind::WelcomeNotice,
        _ => return None,
    };
    let visibility = match parts.next()? {
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

    let result = match action {
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
    use super::*;

    #[test]
    fn command_tail_is_closed_and_only_two_families_are_selectable() {
        assert_eq!(tail("/ephemeral"), Some(""));
        assert_eq!(tail("/ephemeral notice private"), Some("notice private"));
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
