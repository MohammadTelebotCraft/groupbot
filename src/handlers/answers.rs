use grammers_client::message::Message;

use super::{Ctx, can_manage, esc, welcome};

mod stored;
use stored::StoredAnswer;

pub const PREFIX: &str = "answer:";

pub const AUDIENCE: &str = "answer_audience";

pub const ADD: &[&str] = &[
    "اضافه کردن پاسخ",
    "اضافه پاسخ",
    "ثبت پاسخ",
    "تنظیم پاسخ",
    "افزودن پاسخ",
    "/setanswer",
    "/addanswer",
    "/answer",
    "پاسخ",
    "/autoreply",
];
pub const REMOVE: &[&str] = &[
    "حذف پاسخ",
    "پاک پاسخ",
    "لغو پاسخ",
    "/delanswer",
    "/removeanswer",
    "پاک کردن پاسخ",
    "/delautoreply",
];

const MAX_ANSWERS: usize = 200;
const MAX_TRIGGER_CHARS: usize = 64;
const MAX_BODY_CHARS: usize = 3_500;

#[derive(Clone, Copy, PartialEq)]
pub enum Audience {
    All,
    Admins,
    Vips,
}

pub fn audience(ctx: &Ctx, chat: i64) -> Audience {
    match ctx.settings.value(chat, AUDIENCE).as_deref() {
        Some("admins") => Audience::Admins,
        Some("vips") => Audience::Vips,
        _ => Audience::All,
    }
}

impl Audience {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "همه کاربران",
            Self::Admins => "فقط ادمین ها",
            Self::Vips => "فقط کاربران ویژه",
        }
    }
}

pub fn triggers(ctx: &Ctx, chat: i64) -> Vec<String> {
    let mut found: Vec<String> = ctx
        .settings
        .values_with_prefix(chat, PREFIX)
        .into_iter()
        .map(|(trigger, _)| trigger)
        .collect();
    found.sort_unstable();
    found
}

fn key(trigger: &str) -> String {
    format!("{PREFIX}{trigger}")
}

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if let Some((adding, command, rest)) = parse(text)
        && edit(ctx, message, chat, adding, command, rest).await
    {
        return true;
    }
    answer(ctx, message, chat, view).await
}

async fn answer(ctx: &Ctx, message: &Message, chat: i64, view: &super::locks::View<'_>) -> bool {
    if ctx.settings.indexed_empty(chat, PREFIX) {
        return false;
    }
    let trigger = view.lower().trim();
    let Some(stored) = ctx.settings.value(chat, &key(trigger)) else {
        return false;
    };

    let allowed = match audience(ctx, chat) {
        Audience::All => true,
        Audience::Admins => can_manage(ctx, message).await,
        Audience::Vips => {
            let vip = message
                .sender_id()
                .and_then(grammers_client::session::types::PeerId::bare_id)
                .is_some_and(|user| super::vip::is_vip(ctx, chat, user));
            vip || can_manage(ctx, message).await
        }
    };
    if !allowed {
        return false;
    }

    let Some(mut answer) = StoredAnswer::decode(&stored) else {
        log::warn!("answers: invalid stored formatting for {chat}/{trigger}");
        return true;
    };

    if !answer.media.is_empty()
        && let Some(decoded) = welcome::decode_media(&answer.media)
    {
        let Err(e) = message.reply(answer.message().media(decoded)).await else {
            return true;
        };
        if !welcome::reference_expired(&e) {
            eprintln!("answers: {chat}: {e}");
            return true;
        }
        eprintln!("answers: {chat}: media for «{trigger}» expired, keeping the text");
        answer.media.clear();
        if let Err(error) = ctx
            .settings
            .try_set_value(chat, &key(trigger), &answer.encode())
            .await
        {
            ::log::warn!("answers: could not remove expired media for {chat}: {error}");
        }
        if answer.body.trim().is_empty() {
            return true;
        }
    }
    let _ = message.reply(answer.message()).await;
    true
}

async fn edit(
    ctx: &Ctx,
    message: &Message,
    chat: i64,
    adding: bool,
    command: &'static str,
    trigger: &str,
) -> bool {
    if !trigger.is_empty() && !super::phrase_carries_text(command) {
        return false;
    }
    if adding && !has_body(message, trigger) {
        return false;
    }
    let raw_trigger = trigger.trim();
    let mut trigger = raw_trigger.to_lowercase();
    if trigger.is_empty() {
        if !super::phrase_carries_text(command) {
            return false;
        }
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::SettingsView,
            "روی پیام پاسخ ریپلای کنید و بنویسید: «تنظیم پاسخ سلام»\n\
                 برای حذف: «حذف پاسخ سلام»",
        )
        .await;
        return true;
    }
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }

    if !adding {
        if trigger.chars().count() > MAX_TRIGGER_CHARS {
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::CommandError,
                format!("تریگر پاسخ باید حداکثر {MAX_TRIGGER_CHARS} نویسه باشد."),
            )
            .await;
            return true;
        }
        match ctx.settings.try_set(chat, &key(&trigger), false).await {
            Ok(existed) => {
                super::respond(
                    ctx,
                    message,
                    crate::response::ResponseKind::SettingsChanged,
                    if existed {
                        format!("✗ پاسخ «{trigger}» حذف شد.")
                    } else {
                        format!("«{trigger}» در لیست نبود.")
                    },
                )
                .await;
            }
            Err(error) => {
                ::log::warn!("answers: delete for {chat} failed: {error}");
                let text = if error.commit_outcome_unknown() {
                    "نتیجه حذف پاسخ نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "پاسخ حذف نشد؛ دوباره تلاش کنید."
                };
                super::respond(
                    ctx,
                    message,
                    crate::response::ResponseKind::CommandError,
                    text,
                )
                .await;
            }
        }
        return true;
    }

    let answer = match inline_answer(raw_trigger) {
        Some((trigger_only, body)) => {
            trigger = trigger_only;
            StoredAnswer::capture(
                message
                    .media()
                    .and_then(|media| welcome::encode_media(&media))
                    .unwrap_or_default(),
                message.text(),
                body,
                message
                    .fmt_entities()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                true,
            )
        }

        None if message.media().is_some() => StoredAnswer::media_only(
            message
                .media()
                .and_then(|media| welcome::encode_media(&media))
                .unwrap_or_default(),
        ),
        None => {
            let Ok(Some(replied)) = message.get_reply().await else {
                super::respond(
                    ctx,
                    message,
                    crate::response::ResponseKind::CommandError,
                    "روی پیام پاسخ ریپلای کنید، یا بنویسید:\n\
                         «تنظیم پاسخ سلام = درود بر شما»",
                )
                .await;
                return true;
            };
            StoredAnswer::capture(
                replied
                    .media()
                    .and_then(|media| welcome::encode_media(&media))
                    .unwrap_or_default(),
                replied.text(),
                replied.text(),
                replied
                    .fmt_entities()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                false,
            )
        }
    };
    if trigger.chars().count() > MAX_TRIGGER_CHARS {
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            format!("تریگر پاسخ باید حداکثر {MAX_TRIGGER_CHARS} نویسه باشد."),
        )
        .await;
        return true;
    }
    let normalized_trigger = super::commands::normalize(&trigger);
    if super::commands::reserved_trigger(&trigger)
        || super::restrict::custom_triggers(ctx, chat)
            .iter()
            .any(|(word, _)| super::commands::normalize(word) == normalized_trigger)
    {
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            format!("«{trigger}» با یک دستور دیگر تداخل دارد؛ نام دیگری انتخاب کنید."),
        )
        .await;
        return true;
    }
    if answer.media.is_empty() && answer.body.is_empty() {
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            "آن پیام محتوایی برای فرستادن ندارد.",
        )
        .await;
        return true;
    }

    if answer.body.chars().count() > MAX_BODY_CHARS {
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            format!("متن پاسخ باید حداکثر {MAX_BODY_CHARS} نویسه باشد."),
        )
        .await;
        return true;
    }
    let stored = answer.encode();
    if stored.len() > crate::state::MAX_SETTING_VALUE_BYTES {
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            "رسانه یا متن پاسخ بیش از حد بزرگ است.",
        )
        .await;
        return true;
    }
    let trigger_key = key(&trigger);
    if ctx.settings.value(chat, &trigger_key).is_none() && triggers(ctx, chat).len() >= MAX_ANSWERS
    {
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            format!("لیست پاسخ ها پر است ({MAX_ANSWERS} مورد)."),
        )
        .await;
        return true;
    }

    if let Err(error) = ctx
        .settings
        .try_set_value(chat, &trigger_key, &stored)
        .await
    {
        ::log::warn!("answers: write for {chat} failed: {error}");
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                if error.commit_outcome_unknown() {
                    "نتیجه ذخیره پاسخ نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "ذخیره پاسخ انجام نشد؛ ظرفیت تنظیمات یا پایگاه داده را بررسی کنید."
                },
            ),
        )
        .await;
        return true;
    }
    super::respond(
        ctx,
        message,
        crate::response::ResponseKind::SettingsChanged,
        super::premium::icon_html(
            Some(super::premium::Icon::Chat),
            format!(
                "پاسخ «{}» ذخیره شد · مخاطب: <b>{}</b>",
                esc(&trigger),
                audience(ctx, chat).label()
            ),
        ),
    )
    .await;
    true
}

pub enum AddTextError {
    Refused(String),
    Settings(crate::state::SettingsWriteError),
}

pub async fn add_text(ctx: &Ctx, chat: i64, trigger: &str, body: &str) -> Result<String, AddTextError> {
    let refused = |text: String| Err(AddTextError::Refused(text));
    let trigger = trigger.trim().to_lowercase();
    let body = body.trim();
    if trigger.is_empty() || body.is_empty() {
        return refused("کلمه و متن پاسخ هر دو لازم است.".to_owned());
    }
    if trigger.chars().count() > MAX_TRIGGER_CHARS {
        return refused(format!("تریگر پاسخ باید حداکثر {MAX_TRIGGER_CHARS} نویسه باشد."));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return refused(format!("متن پاسخ باید حداکثر {MAX_BODY_CHARS} نویسه باشد."));
    }
    let normalized_trigger = super::commands::normalize(&trigger);
    if super::commands::reserved_trigger(&trigger)
        || super::restrict::custom_triggers(ctx, chat)
            .iter()
            .any(|(word, _)| super::commands::normalize(word) == normalized_trigger)
    {
        return refused(format!("«{trigger}» با یک دستور دیگر تداخل دارد؛ نام دیگری انتخاب کنید."));
    }
    let stored = StoredAnswer::capture(String::new(), body, body, &[], false).encode();
    if stored.len() > crate::state::MAX_SETTING_VALUE_BYTES {
        return refused("متن پاسخ بیش از حد بزرگ است.".to_owned());
    }
    let trigger_key = key(&trigger);
    if ctx.settings.value(chat, &trigger_key).is_none() && triggers(ctx, chat).len() >= MAX_ANSWERS {
        return refused(format!("لیست پاسخ ها پر است ({MAX_ANSWERS} مورد)."));
    }
    ctx.settings
        .try_set_value(chat, &trigger_key, &stored)
        .await
        .map_err(AddTextError::Settings)?;
    Ok(trigger)
}

fn has_body(message: &Message, trigger: &str) -> bool {
    inline_answer(trigger).is_some()
        || message.media().is_some()
        || message.reply_to_message_id().is_some()
}

fn inline_answer(rest: &str) -> Option<(String, &str)> {
    let (trigger, answer) = rest.split_once('=')?;
    let (trigger, answer) = (trigger.trim(), answer.trim());
    (!trigger.is_empty() && !answer.is_empty()).then(|| (trigger.to_lowercase(), answer))
}

fn parse(text: &str) -> Option<(bool, &'static str, &str)> {
    for (commands, adding) in [(ADD, true), (REMOVE, false)] {
        for command in commands {
            let Some(rest) = super::commands::strip_prefix(text, command) else {
                continue;
            };
            return Some((adding, command, rest));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_inline_answers() {
        assert_eq!(
            inline_answer("سلام = درود بر شما"),
            Some(("سلام".to_owned(), "درود بر شما"))
        );
        assert_eq!(inline_answer("سلام"), None);
        assert_eq!(inline_answer("= درود"), None);
        assert_eq!(inline_answer("سلام ="), None);
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse("تنظیم پاسخ سلام"), Some((true, "تنظیم پاسخ", "سلام")));
        assert_eq!(parse("پاسخ خوش آمدید"), Some((true, "پاسخ", "خوش آمدید")));
        assert_eq!(parse("حذف پاسخ سلام"), Some((false, "حذف پاسخ", "سلام")));
        assert_eq!(parse("پاسخگو"), None);
        assert_eq!(parse("سلام"), None);
    }

    #[test]
    fn aliases_and_keyboard_variants_preserve_inline_answer_content() {
        let raw = "ثبت\u{200c}پاسخ HELLO = <a href=\"https://Example.com/AbC\">Hi كيان ۱۲</a>";
        let (_, command, rest) = parse(raw).unwrap();
        assert_eq!(command, "ثبت پاسخ");
        assert_eq!(
            inline_answer(rest),
            Some((
                "hello".to_owned(),
                "<a href=\"https://Example.com/AbC\">Hi كيان ۱۲</a>"
            ))
        );
        assert_eq!(
            parse("تنظيم\t پاسخ خودكار سلام = Hi"),
            Some((true, "تنظیم پاسخ", "خودكار سلام = Hi"))
        );
        assert_eq!(
            parse("حذف پاسخ خودکار"),
            Some((false, "حذف پاسخ", "خودکار"))
        );
        assert_eq!(
            parse("/AddAnswer HELLO = Hi"),
            Some((true, "/addanswer", "HELLO = Hi"))
        );
        assert_eq!(
            parse("/DELANSWER HELLO"),
            Some((false, "/delanswer", "HELLO"))
        );
        assert!(super::super::phrase_carries_text("/answer"));
        assert_eq!(parse("/answers HELLO = Hi"), None);
        assert_eq!(parse("ثبت پاسخگو"), None);
    }

    #[test]
    fn a_one_word_alias_does_not_carry_a_trigger() {
        assert!(!super::super::phrase_carries_text("پاسخ"));
        assert!(super::super::phrase_carries_text("تنظیم پاسخ"));
        assert!(super::super::phrase_carries_text("افزودن پاسخ"));
        for command in REMOVE {
            assert!(
                super::super::phrase_carries_text(command),
                "«{command}» would silently claim ordinary speech"
            );
        }
    }
}
