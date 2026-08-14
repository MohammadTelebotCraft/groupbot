use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::PeerId;
use grammers_client::update::CallbackQuery;

use super::{Ctx, can_manage, esc, name_of};

pub const PREFIX: &str = "filter:";

const ADD: &[&str] = &["فیلتر کلمه", "فیلتر"];
const REMOVE: &[&str] = &["حذف فیلتر", "لغو فیلتر"];

const MAX_LEN: usize = 64;

const MAX_WORDS: usize = 200;

pub fn key(word: &str) -> String {
    format!("{PREFIX}{word}")
}

pub fn words(ctx: &Ctx, chat: i64) -> Vec<String> {
    ctx.settings.flags_with_prefix(chat, PREFIX)
}

pub fn matches(ctx: &Ctx, chat: i64, view: &super::locks::View) -> bool {
    if ctx.settings.indexed_empty(chat, PREFIX) {
        return false;
    }
    let lowercased = view.lower();
    !lowercased.is_empty()
        && ctx
            .settings
            .indexed_any(chat, PREFIX, |word| lowercased.contains(word))
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some((add, word)) = parse(text) else {
        return false;
    };
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !can_manage(ctx, message).await {
        return true;
    }

    let word = word.trim().to_lowercase();
    if word.is_empty() {
        let _ = message
            .reply("کلمه را بنویسید. مثال: فیلتر کلمه تبلیغ")
            .await;
        return true;
    }
    if word.len() > MAX_LEN || word.contains('=') {
        let _ = message
            .reply("این کلمه پذیرفته نمی شود: خیلی بلند است یا نویسه غیرمجاز دارد.")
            .await;
        return true;
    }
    if add && words(ctx, chat).len() >= MAX_WORDS {
        let _ = message
            .reply(format!("لیست فیلتر پر است ({MAX_WORDS} کلمه)."))
            .await;
        return true;
    }

    let changed = ctx.settings.set(chat, &key(&word), add).await;
    let mark = if add { "✓" } else { "✗" };
    let what = match (add, changed) {
        (true, true) => "به لیست فیلتر اضافه شد",
        (true, false) => "از قبل در لیست فیلتر بود",
        (false, true) => "از لیست فیلتر حذف شد",
        (false, false) => "در لیست فیلتر نبود",
    };
    let _ = message.reply(format!("{mark} «{word}» {what}.")).await;
    true
}

fn parse(text: &str) -> Option<(bool, &str)> {
    for (commands, add) in [(ADD, true), (REMOVE, false)] {
        for command in commands {
            let Some(rest) = text.strip_prefix(command) else {
                continue;
            };
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return Some((add, rest.trim()));
            }
        }
    }
    None
}

pub async fn notify(
    ctx: &Ctx,
    message: &Message,
    chat: i64,
    deleted_text: &str,
    chances: Option<u32>,
) {
    let Some(user) = message.sender_id().and_then(PeerId::bare_id) else {
        return;
    };
    if !ctx.may_notify(chat, user) {
        return;
    }
    let key = ctx.keep_deleted(deleted_text.to_owned());

    let _ = message
        .respond(
            InputMessage::new()
                .html(format!(
                    "<a href=\"tg://user?id={user}\">{}</a> پیام شما به دلیل داشتن کلمه فیلتر شده حذف شد.{}",
                    esc(&name_of(message)),
                    match chances {
                        Some(chances) => format!("\n{}", super::strict::chances_line(chances)),
                        None => String::new(),
                    }
                ))
                .reply_markup(ReplyMarkup::from_buttons(&[vec![Button::data(
                    "متن پیام من چه بود؟",
                    format!("f:{user}:{key}").into_bytes(),
                )]])),
        )
        .await;
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, is_admin: bool) {
    let Some((offender, key)) = payload.split_once(':') else {
        return;
    };
    let (Ok(offender), Ok(key)) = (offender.parse::<i64>(), key.parse::<u64>()) else {
        return;
    };
    if query.sender_id().bare_id() != Some(offender) && !is_admin {
        let _ = query
            .answer()
            .alert("این دکمه برای فرستنده پیام و ادمین ها است.")
            .send()
            .await;
        return;
    }

    let answer = match ctx.deleted_text(key) {
        Some(text) if text.chars().count() > 180 => {
            format!("{}…", text.chars().take(180).collect::<String>())
        }
        Some(text) => text,
        None => "متن این پیام دیگر در دسترس نیست.".to_owned(),
    };
    let _ = query.answer().alert(answer).send().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse("فیلتر کلمه تبلیغ"), Some((true, "تبلیغ")));
        assert_eq!(parse("فیلتر تبلیغ رایگان"), Some((true, "تبلیغ رایگان")));
        assert_eq!(parse("حذف فیلتر تبلیغ"), Some((false, "تبلیغ")));
        assert_eq!(parse("فیلتر"), Some((true, "")));
        assert_eq!(parse("فیلترها"), None);
        assert_eq!(parse("سلام"), None);
    }
}
