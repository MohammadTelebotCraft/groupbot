use std::time::Duration;

use grammers_client::message::Message;

use super::{Ctx, can_manage, name_of};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Mute,
    Unmute,
    Ban,
    Unban,
}

use Action::*;

const COMMANDS: &[(&str, Action)] = &[
    ("حذف سکوت", Unmute),
    ("حذف خفه", Unmute),
    ("حذف بن", Unban),
    ("حذف سیک", Unban),
    ("رفع سکوت", Unmute),
    ("رفع خفه", Unmute),
    ("رفع بن", Unban),
    ("رفع سیک", Unban),
    ("سکوت", Mute),
    ("خفه", Mute),
    ("بن", Ban),
    ("سیک", Ban),
];

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = super::digits(message.text().trim());
    let Some(parsed) = parse(&text) else {
        return false;
    };
    let (action, arg) = (parsed.action, parsed.target);

    if !can_manage(ctx, message).await {
        return true;
    }

    let (Some((target, target_name)), Ok(Some(chat_ref))) = (
        super::target(ctx, message, arg).await,
        message.peer_ref().await,
    ) else {
        let _ = message
            .reply("کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.")
            .await;
        return true;
    };

    let by = super::sender_of(message);
    let result = apply(
        ctx,
        chat_ref,
        target,
        action,
        parsed.duration,
        By {
            actor: by.as_ref().map(|(id, name)| (*id, name.as_str())),
            reason: "دستور ادمین",
            target_name: &target_name,
        },
    )
    .await;
    if result.is_ok()
        && let Some(chat) = message.peer_id().bot_api_dialog_id()
    {
        match action {
            Ban => ctx.bump(chat, super::stats::BANNED),
            Mute => ctx.bump(chat, super::stats::MUTED),
            _ => {}
        }
    }

    let by = name_of(message);
    let _ = match result {
        Ok(()) => {
            let what = match action {
                Mute => "✓ سکوت شد",
                Unmute => "✗ سکوتش برداشته شد",
                Ban => "✓ بن شد",
                Unban => "✗ بنش برداشته شد",
            };
            let how_long = match parsed.duration_text {
                Some(text) if parsed.duration.is_some() => format!(" به مدت {text}"),
                _ => String::new(),
            };
            message
                .reply(format!("{target_name} {what}{how_long}.\nتوسط: {by}"))
                .await
        }
        Err(e) => {
            eprintln!("restrict failed: {e}");
            message
                .reply("انجام نشد. مطمئن شوید ربات ادمین است و اجازه محدود کردن کاربران دارد.")
                .await
        }
    };
    true
}

pub async fn apply(
    ctx: &Ctx,
    chat: grammers_client::session::types::PeerRef,
    target: grammers_client::session::types::PeerRef,
    action: Action,
    duration: Option<Duration>,
    by: By<'_>,
) -> Result<(), grammers_client::InvocationError> {
    let done = apply_rights(ctx, chat, target, action, duration).await;

    if done.is_ok()
        && let (Some(chat_id), Some(user)) = (chat.id.bot_api_dialog_id(), target.id.bare_id())
    {
        let mut extra = Vec::new();
        if let Some(duration) = duration {
            extra.push(("مدت", super::log::duration_label(duration.as_secs())));
        } else if matches!(action, Mute | Ban) {
            extra.push(("مدت", "دائمی".to_owned()));
        }
        super::log::write(
            ctx,
            chat_id,
            "log_mod",
            super::log::Entry {
                title: match action {
                    Mute => "سکوت",
                    Ban => "بن",
                    Unmute => "رفع سکوت",
                    Unban => "رفع بن",
                },
                target: Some((user, by.target_name)),
                actor: by.actor,
                reason: Some(by.reason),
                extra,
            },
        )
        .await;
    }
    done
}

#[derive(Default)]
pub struct By<'a> {
    pub actor: Option<(i64, &'a str)>,
    pub reason: &'a str,
    pub target_name: &'a str,
}

async fn apply_rights(
    ctx: &Ctx,
    chat: grammers_client::session::types::PeerRef,
    target: grammers_client::session::types::PeerRef,
    action: Action,
    duration: Option<Duration>,
) -> Result<(), grammers_client::InvocationError> {
    let mut rights = ctx.client.set_banned_rights(chat, target);
    if let Some(duration) = duration {
        rights = rights.duration(duration);
    }
    match action {
        Mute => {
            rights
                .send_messages(false)
                .send_media(false)
                .send_stickers(false)
                .send_gifs(false)
                .send_inline(false)
                .send_polls(false)
                .await
        }
        Ban => rights.view_messages(false).await,
        Unmute | Unban => rights.await,
    }
}

#[derive(Debug, PartialEq)]
struct Parsed<'a> {
    action: Action,

    target: Option<&'a str>,

    duration: Option<Duration>,

    duration_text: Option<&'a str>,
}

const UNITS: &[(&str, u64)] = &[
    ("ثانیه", 1),
    ("دقیقه", 60),
    ("ساعت", 3600),
    ("روز", 86_400),
    ("هفته", 604_800),
    ("ماه", 2_592_000),
    ("s", 1),
    ("m", 60),
    ("h", 3600),
    ("d", 86_400),
    ("w", 604_800),
];

fn parse(text: &str) -> Option<Parsed<'_>> {
    let (action, rest) = COMMANDS.iter().find_map(|&(cmd, action)| {
        let rest = text.strip_prefix(cmd)?;

        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            Some((action, rest.trim()))
        } else {
            None
        }
    })?;

    let mut parsed = Parsed {
        action,
        target: None,
        duration: None,
        duration_text: None,
    };

    let words: Vec<&str> = rest.split_whitespace().collect();
    let mut i = 0;
    while i < words.len() {
        if let Some((secs, end)) = duration_at(&words, i) {
            parsed.duration = Some(Duration::from_secs(secs));
            parsed.duration_text = Some(slice_of(rest, &words, i, end));
            i = end;
            continue;
        }
        if parsed.target.is_none() {
            parsed.target = Some(words[i]);
        }
        i += 1;
    }
    Some(parsed)
}

fn duration_at(words: &[&str], i: usize) -> Option<(u64, usize)> {
    let word = words[i];

    let split = word.find(|c: char| !c.is_ascii_digit()).unwrap_or(word.len());
    let (digits, tail) = word.split_at(split);
    let amount: u64 = digits.parse().ok()?;
    if !tail.is_empty() {
        return unit_secs(tail).map(|secs| (amount * secs, i + 1));
    }
    let unit = words.get(i + 1)?;
    unit_secs(unit).map(|secs| (amount * secs, i + 2))
}

fn unit_secs(word: &str) -> Option<u64> {
    UNITS
        .iter()
        .find(|(name, _)| *name == word)
        .map(|(_, secs)| *secs)
}

fn slice_of<'a>(rest: &'a str, words: &[&str], from: usize, to: usize) -> &'a str {
    let start = word_offset(rest, words, from);
    let last = to - 1;
    let end = word_offset(rest, words, last) + words[last].len();
    &rest[start..end]
}

fn word_offset(rest: &str, words: &[&str], index: usize) -> usize {
    let mut offset = 0;
    for word in words.iter().take(index) {
        offset = rest[offset..].find(word).unwrap_or(0) + offset + word.len();
    }
    rest[offset..].find(words[index]).unwrap_or(0) + offset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Parsed<'_> {
        parse(text).expect("should parse")
    }

    #[test]
    fn parses_commands() {
        assert_eq!(p("سکوت").action, Mute);
        assert_eq!(p("حذف سکوت").action, Unmute);
        assert_eq!(p("سیک").action, Ban);
        assert_eq!(p("خفه @someone").target, Some("@someone"));
        assert_eq!(p("حذف سیک 12345").target, Some("12345"));
        assert!(parse("سکوتی").is_none());
        assert!(parse("سلام").is_none());
    }

    #[test]
    fn parses_durations() {
        assert_eq!(p("سکوت 10 دقیقه").duration, Some(Duration::from_secs(600)));
        assert_eq!(p("سکوت 10 دقیقه").duration_text, Some("10 دقیقه"));
        assert_eq!(p("خفه 2ساعت").duration, Some(Duration::from_secs(7200)));
        assert_eq!(p("بن @ali 1 روز").duration, Some(Duration::from_secs(86_400)));
        assert_eq!(p("بن @ali 1 روز").target, Some("@ali"));
        assert_eq!(p("سکوت 1 هفته").duration, Some(Duration::from_secs(604_800)));

        assert_eq!(p("سکوت 12345").target, Some("12345"));
        assert_eq!(p("سکوت 12345").duration, None);

        assert_eq!(p("سکوت 5 دقیقه @ali").target, Some("@ali"));
    }
}
