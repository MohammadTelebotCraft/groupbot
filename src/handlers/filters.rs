use grammers_client::message::{Button, Message};
use grammers_client::session::types::PeerId;
use grammers_client::update::CallbackQuery;

use super::{Ctx, esc, name_of};
use crate::response::ResponseKind;

pub const PREFIX: &str = "filter:";

pub const ADD: &[&str] = &[
    "فیلتر کلمه",
    "افزودن فیلتر",
    "اضافه کردن فیلتر",
    "اضافه فیلتر",
    "تنظیم فیلتر",
    "/filter",
    "/addfilter",
    "فیلتر",
    "/addword",
];
pub const REMOVE: &[&str] = &[
    "حذف فیلتر",
    "لغو فیلتر",
    "پاک فیلتر",
    "/unfilter",
    "/delfilter",
    "/removefilter",
    "رفع فیلتر",
    "پاک کردن فیلتر",
];

pub const EXACT_ON: &[&str] = &[
    "فیلتر دقیق",
    "حالت فیلتر دقیق",
    "/filterexact",
    "/exactfilter",
];
pub const EXACT_OFF: &[&str] = &[
    "فیلتر عادی",
    "حالت فیلتر عادی",
    "/filternormal",
    "/normalfilter",
];

pub const EXACT: &str = "filter_exact";

const MAX_LEN: usize = 64;

const MAX_WORDS: usize = 200;

pub fn key(word: &str) -> String {
    format!("{PREFIX}{word}")
}

fn normalise(raw: &str) -> Option<String> {
    let word = super::locks::tighten(&super::locks::folded(raw.trim())).into_owned();
    (!word.is_empty() && word.chars().count() <= MAX_LEN && !word.contains('=')).then_some(word)
}

pub enum AddError {
    Invalid,
    Full,
    Settings(crate::state::SettingsWriteError),
}

pub async fn add(ctx: &Ctx, chat: i64, raw: &str) -> Result<bool, AddError> {
    let word = normalise(raw).ok_or(AddError::Invalid)?;
    if words(ctx, chat).len() >= MAX_WORDS {
        return Err(AddError::Full);
    }
    ctx.settings
        .try_set(chat, &key(&word), true)
        .await
        .map_err(AddError::Settings)
}

pub fn words(ctx: &Ctx, chat: i64) -> Vec<String> {
    ctx.settings.flags_with_prefix(chat, PREFIX)
}

pub fn matches(ctx: &Ctx, chat: i64, view: &super::locks::View) -> bool {
    if ctx.settings.indexed_empty(chat, PREFIX) {
        return false;
    }
    let exact = ctx.settings.is_locked(chat, EXACT);
    view.tight_texts()
        .filter(|haystack| !haystack.is_empty())
        .any(|haystack| {
            ctx.settings.indexed_any(chat, PREFIX, |word| {
                found(haystack, &super::locks::tighten(word), exact)
            })
        })
}

fn found(text: &str, word: &str, exact: bool) -> bool {
    if word.is_empty() {
        return false;
    }
    if !exact {
        return text.contains(word);
    }
    let joins = |c: char| c.is_alphanumeric() || c == '\u{200c}';
    let mut from = 0;
    while let Some(at) = text[from..].find(word) {
        let start = from + at;
        let end = start + word.len();
        if !text[..start].chars().next_back().is_some_and(joins)
            && !text[end..].chars().next().is_some_and(joins)
        {
            return true;
        }
        from = start + word.chars().next().map_or(1, char::len_utf8);
    }
    false
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    if let Some(on) = mode_toggle(text) {
        let Some(chat) = message.peer_id().bot_api_dialog_id() else {
            return false;
        };
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        if let Err(error) = ctx.settings.try_set(chat, EXACT, on).await {
            ::log::warn!("filters: exact-mode write for {chat} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                if error.commit_outcome_unknown() {
                    "نتیجه ذخیره حالت فیلتر نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "حالت فیلتر ذخیره نشد؛ دوباره تلاش کنید."
                },
            )
            .await;
            return true;
        }
        super::respond(ctx, message, ResponseKind::FilterManagement, match on {
                true => "✓ فیلتر دقیق فعال شد · کلمه فقط وقتی می گیرد که جدا آمده باشد، نه داخل کلمه دیگر.",
                false => "✓ فیلتر عادی فعال شد · کلمه هر جای متن باشد می گیرد.",
            }).await;
        return true;
    }
    let Some((add, command, word)) = parse(text) else {
        return false;
    };
    let phrase = super::phrase_carries_text(command);
    if !word.is_empty() && !phrase {
        return false;
    }
    let asked = match word.is_empty() {
        true => message
            .get_reply()
            .await
            .ok()
            .flatten()
            .map(|replied| replied.text().trim().to_owned()),
        false => Some(word.to_owned()),
    }
    .filter(|word| !word.is_empty());
    if asked.is_none() && !phrase {
        return false;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Some(word) = asked else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "کلمه را بعد از دستور بنویسید، مثل «افزودن فیلتر ممد»، یا روی پیام آن ریپلای کنید.",
        )
        .await;
        return true;
    };

    let Some(word) = normalise(&word) else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "این کلمه پذیرفته نمی شود: خیلی بلند است یا نویسه غیرمجاز دارد.",
            ),
        )
        .await;
        return true;
    };
    if add && words(ctx, chat).len() >= MAX_WORDS {
        super::respond(
            ctx,
            message,
            ResponseKind::FilterManagement,
            format!("لیست فیلتر پر است ({MAX_WORDS} کلمه)."),
        )
        .await;
        return true;
    }

    let stored = match add {
        true => word.clone(),
        false => words(ctx, chat)
            .into_iter()
            .find(|held| super::locks::tighten(held) == word)
            .unwrap_or_else(|| word.clone()),
    };
    let changed = match ctx.settings.try_set(chat, &key(&stored), add).await {
        Ok(changed) => changed,
        Err(error) => {
            ::log::warn!("filters: list write for {chat} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                if error.commit_outcome_unknown() {
                    "نتیجه ذخیره فیلتر نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "فیلتر ذخیره نشد؛ دوباره تلاش کنید."
                },
            )
            .await;
            return true;
        }
    };
    let mark = if add { "✓" } else { "✗" };
    let what = match (add, changed) {
        (true, true) => "به لیست فیلتر اضافه شد",
        (true, false) => "از قبل در لیست فیلتر بود",
        (false, true) => "از لیست فیلتر حذف شد",
        (false, false) => "در لیست فیلتر نبود",
    };
    super::respond(
        ctx,
        message,
        ResponseKind::FilterManagement,
        super::premium::icon_text(
            Some(super::premium::Icon::Chat),
            format!("{mark} «{word}» {what}."),
        ),
    )
    .await;
    true
}

fn mode_toggle(text: &str) -> Option<bool> {
    if EXACT_ON
        .iter()
        .any(|command| super::commands::matches(text, command))
    {
        return Some(true);
    }
    EXACT_OFF
        .iter()
        .any(|command| super::commands::matches(text, command))
        .then_some(false)
}

fn parse(text: &str) -> Option<(bool, &'static str, &str)> {
    for (commands, add) in [(ADD, true), (REMOVE, false)] {
        for command in commands {
            let Some(rest) = super::commands::strip_prefix(text, command) else {
                continue;
            };
            return Some((add, command, rest));
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
    if !ctx.may_notify_filtered(chat, user) {
        return;
    }
    let key = ctx.keep_deleted(deleted_text.to_owned());

    let _ = message
        .respond(
            super::premium::icon_html(Some(super::premium::Icon::Chat), format!(
                    "<a href=\"tg://user?id={user}\">{}</a> پیام شما به دلیل داشتن کلمه فیلتر شده حذف شد.{}",
                    esc(&name_of(message)),
                    match chances {
                        Some(chances) => format!("\n{}", super::strict::chances_line(chances)),
                        None => String::new(),
                    }
                ))
                .reply_markup(super::premium::buttons(&[vec![Button::data(
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
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Locked),
                "این دکمه برای فرستنده پیام و ادمین ها است.",
            ))
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
    fn exact_matching_spares_the_word_inside_a_word() {
        assert!(found("صحبت های دیروز", "بت", false));
        assert!(!found("صحبت های دیروز", "بت", true));
        assert!(found("این بت پرستی است", "بت", true));
        assert!(found("بت", "بت", true), "the whole message is the word");
        assert!(found("گفت: بت!", "بت", true));
        assert!(!found("صحبت\u{200c}ها", "بت", true), "the half-space joins");
        assert!(found("صحبت درباره بت", "بت", true));
        assert!(found("این تبلیغ رایگان است", "تبلیغ رایگان", true));
        assert!(!found("فروشی", "فروش", true));
        assert!(found("فروشی", "فروش", false));
    }

    #[test]
    fn parses_commands() {
        assert_eq!(
            parse("فیلتر کلمه تبلیغ"),
            Some((true, "فیلتر کلمه", "تبلیغ"))
        );
        assert_eq!(
            parse("افزودن فیلتر ممد"),
            Some((true, "افزودن فیلتر", "ممد"))
        );
        assert_eq!(
            parse("اضافه کردن فیلتر ممد"),
            Some((true, "اضافه کردن فیلتر", "ممد"))
        );
        assert!(super::super::phrase_carries_text("افزودن فیلتر"));
        assert_eq!(
            parse("فیلتر تبلیغ رایگان"),
            Some((true, "فیلتر", "تبلیغ رایگان"))
        );
        assert_eq!(
            parse("حذف فیلتر تبلیغ"),
            Some((false, "حذف فیلتر", "تبلیغ"))
        );
        assert_eq!(parse("فیلتر"), Some((true, "فیلتر", "")));

        assert!(!super::super::phrase_carries_text("فیلتر"));
        assert!(super::super::phrase_carries_text("فیلتر کلمه"));
        assert_eq!(parse("فیلترها"), None);
        assert_eq!(parse("سلام"), None);
    }

    #[test]
    fn filter_aliases_preserve_words_and_require_exact_mode_commands() {
        assert_eq!(
            parse("اضافه\u{200c}فيلتر Mixed كلمه ۱۲"),
            Some((true, "اضافه فیلتر", "Mixed كلمه ۱۲"))
        );
        assert_eq!(
            parse("/FILTER Mixed Case"),
            Some((true, "/filter", "Mixed Case"))
        );
        assert_eq!(
            parse("/Unfilter Mixed Case"),
            Some((false, "/unfilter", "Mixed Case"))
        );
        assert_eq!(parse("/filtered Word"), None);
        assert_eq!(mode_toggle("فيلتر\t دقيق"), Some(true));
        assert_eq!(mode_toggle("/FilterNormal"), Some(false));
        assert_eq!(mode_toggle("فیلتر دقیق چیه"), None);
        assert_eq!(mode_toggle("/filterexact extra"), None);
        assert!(!super::super::phrase_carries_text("فیلتر"));
        assert!(super::super::phrase_carries_text("/filter"));
    }
}
