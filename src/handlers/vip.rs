use grammers_client::message::Message;

use super::{Ctx};

pub const ADD: &[&str] = &["ویژه", "افزودن ویژه", "تنظیم ویژه"];
pub const REMOVE: &[&str] = &["حذف ویژه", "لغو ویژه"];

pub const PREFIX: &str = "vip:";

pub fn is_vip(ctx: &Ctx, chat: i64, user: i64) -> bool {
    ctx.settings.is_locked(chat, &key(user))
}

pub fn key(user: i64) -> String {
    format!("{PREFIX}{user}")
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    let Some((add, arg)) = parse(text) else {
        return false;
    };

    let Some(named) = super::named(message, arg) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::VIP).await {
        return true;
    }

    let Some((target, target_name)) = super::resolve(ctx, message, named).await else {
        let _ = message
            .reply("کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.")
            .await;
        return true;
    };
    let Some(target_id) = target.id.bare_id() else {
        let _ = message.reply("کاربر پیدا نشد.").await;
        return true;
    };

    let changed = ctx.settings.set(chat, &key(target_id), add).await;
    let mark = if add { "✓" } else { "✗" };
    let what = match (add, changed) {
        (true, true) => "به لیست ویژه اضافه شد",
        (true, false) => "از قبل در لیست ویژه بود",
        (false, true) => "از لیست ویژه حذف شد",
        (false, false) => "در لیست ویژه نبود",
    };
    let _ = message.reply(format!("{mark} {target_name} {what}.")).await;
    true
}

fn parse(text: &str) -> Option<(bool, Option<&str>)> {
    for (commands, add) in [(ADD, true), (REMOVE, false)] {
        for command in commands {
            let Some(rest) = text.strip_prefix(command) else {
                continue;
            };
            let rest = rest.trim();
            if rest.is_empty() {
                return Some((add, None));
            }

            if text[command.len()..].starts_with(char::is_whitespace) && !rest.contains(' ') {
                return Some((add, Some(rest)));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse("ویژه"), Some((true, None)));
        assert_eq!(parse("تنظیم ویژه"), Some((true, None)));
        assert_eq!(parse("ویژه @someone"), Some((true, Some("@someone"))));
        assert_eq!(parse("حذف ویژه 12345"), Some((false, Some("12345"))));
        assert_eq!(parse("حذف سکوت"), None);
        assert_eq!(parse("سلام"), None);
    }
}
