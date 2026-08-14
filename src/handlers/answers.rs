use grammers_client::message::{InputMessage, Message};

use super::{Ctx, can_manage, esc, welcome};

pub const PREFIX: &str = "answer:";

pub const AUDIENCE: &str = "answer_audience";

const ADD: &[&str] = &["تنظیم پاسخ", "افزودن پاسخ", "پاسخ"];
const REMOVE: &[&str] = &["حذف پاسخ", "پاک پاسخ"];

const SEPARATOR: char = '|';

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

    if let Some((adding, rest)) = parse(text) {
        return edit(ctx, message, chat, adding, rest).await;
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

    let (media, body) = stored.split_once(SEPARATOR).unwrap_or(("", stored.as_str()));
    let mut input = InputMessage::new().html(body);

    if !media.is_empty()
        && let Some(media) = welcome::decode_media(media)
    {
        input = input.media(media);
    }
    let _ = message.reply(input).await;
    true
}

async fn edit(ctx: &Ctx, message: &Message, chat: i64, adding: bool, trigger: &str) -> bool {
    if adding && !has_body(message, trigger) {
        return false;
    }
    if !can_manage(ctx, message).await {
        return true;
    }
    let mut trigger = trigger.trim().to_lowercase();
    if trigger.is_empty() {
        let _ = message
            .reply(
                "روی پیام پاسخ ریپلای کنید و بنویسید: «تنظیم پاسخ سلام»\n\
                 برای حذف: «حذف پاسخ سلام»",
            )
            .await;
        return true;
    }

    if !adding {
        let existed = ctx.settings.set(chat, &key(&trigger), false).await;
        let _ = message
            .reply(if existed {
                format!("✗ پاسخ «{trigger}» حذف شد.")
            } else {
                format!("«{trigger}» در لیست نبود.")
            })
            .await;
        return true;
    }

    let (media, body) = match inline_answer(&trigger) {
        Some((trigger_only, body)) => {
            trigger = trigger_only;
            (String::new(), body)
        }

        None if message.media().is_some() => (
            message
                .media()
                .and_then(|media| welcome::encode_media(&media))
                .unwrap_or_default(),
            String::new(),
        ),
        None => {
            let Ok(Some(replied)) = message.get_reply().await else {
                let _ = message
                    .reply(
                        "روی پیام پاسخ ریپلای کنید، یا بنویسید:\n\
                         «تنظیم پاسخ سلام = درود بر شما»",
                    )
                    .await;
                return true;
            };
            (
                replied
                    .media()
                    .and_then(|media| welcome::encode_media(&media))
                    .unwrap_or_default(),
                replied.text().to_owned(),
            )
        }
    };
    if media.is_empty() && body.is_empty() {
        let _ = message.reply("آن پیام محتوایی برای فرستادن ندارد.").await;
        return true;
    }

    ctx.settings
        .set_value(chat, &key(&trigger), &format!("{media}{SEPARATOR}{body}"))
        .await;
    let _ = message
        .reply(InputMessage::new().html(format!(
            "✓ پاسخ «{}» ذخیره شد · مخاطب: <b>{}</b>",
            esc(&trigger),
            audience(ctx, chat).label()
        )))
        .await;
    true
}

fn has_body(message: &Message, trigger: &str) -> bool {
    inline_answer(&trigger.trim().to_lowercase()).is_some()
        || message.media().is_some()
        || message.reply_to_message_id().is_some()
}

fn inline_answer(rest: &str) -> Option<(String, String)> {
    let (trigger, answer) = rest.split_once('=')?;
    let (trigger, answer) = (trigger.trim(), answer.trim());
    (!trigger.is_empty() && !answer.is_empty())
        .then(|| (trigger.to_lowercase(), answer.to_owned()))
}

fn parse(text: &str) -> Option<(bool, &str)> {
    for (commands, adding) in [(ADD, true), (REMOVE, false)] {
        for command in commands {
            let Some(rest) = text.strip_prefix(command) else {
                continue;
            };
            if rest.starts_with(char::is_whitespace) {
                return Some((adding, rest.trim()));
            }
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
            Some(("سلام".to_owned(), "درود بر شما".to_owned()))
        );
        assert_eq!(inline_answer("سلام"), None);
        assert_eq!(inline_answer("= درود"), None);
        assert_eq!(inline_answer("سلام ="), None);
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse("تنظیم پاسخ سلام"), Some((true, "سلام")));
        assert_eq!(parse("پاسخ خوش آمدید"), Some((true, "خوش آمدید")));
        assert_eq!(parse("حذف پاسخ سلام"), Some((false, "سلام")));
        assert_eq!(parse("پاسخگو"), None);
        assert_eq!(parse("سلام"), None);
    }
}
