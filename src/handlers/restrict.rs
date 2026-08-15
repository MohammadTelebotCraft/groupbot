use std::time::Duration;

use grammers_client::message::Message;

use super::{Ctx, name_of};

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

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let text = view.digits();
    let Some(parsed) = parse(text) else {
        return false;
    };
    let (action, arg) = (parsed.action, parsed.target);

    let Some(named) = super::named(message, arg) else {
        return false;
    };

    let needed = match action {
        Ban | Unban => super::limits::BAN,
        Mute | Unmute => super::limits::MUTE,
    };
    if !super::limits::allows(ctx, message, needed).await {
        return true;
    }

    let (Some((target, target_name)), Ok(Some(chat_ref))) = (
        super::resolve(ctx, message, named).await,
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
            ..Default::default()
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
            let kick_only = action == Ban
                && chat_ref.id.kind() != grammers_client::session::types::PeerKind::Channel;
            let what = match action {
                Mute => "✓ سکوت شد",
                Unmute => "✗ سکوتش برداشته شد",
                Ban if kick_only => "✓ از گروه اخراج شد",
                Ban => "✓ بن شد",
                Unban => "✗ بنش برداشته شد",
            };
            let note = match kick_only {
                true => "\nدر گروه معمولی بن دائمی نیست. برای بن به سوپرگروه ارتقا دهید.",
                false => "",
            };
            let how_long = match (parsed.duration, honoured(parsed.duration)) {
                (Some(_), Some(held)) => {
                    format!(" به مدت {}", super::log::duration_label(held.as_secs()))
                }
                (Some(_), None) => " به صورت دائمی".to_owned(),
                _ => String::new(),
            };
            message
                .reply(format!("{target_name} {what}{how_long}.\nتوسط: {by}{note}"))
                .await
        }
        Err(e) => {
            eprintln!("restrict failed: {e}");
            message.reply(e.told()).await
        }
    };
    true
}

const SHORTEST: Duration = Duration::from_secs(30);
const LONGEST: Duration = Duration::from_secs(366 * 86_400);

pub fn honoured(duration: Option<Duration>) -> Option<Duration> {
    match duration {
        Some(asked) if asked > LONGEST => None,
        Some(asked) => Some(asked.max(SHORTEST)),
        None => None,
    }
}

#[derive(Debug)]
pub enum Failed {
    Protected,

    BasicGroup,
    Telegram(grammers_client::InvocationError),
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::Protected => f.write_str("target is an admin"),
            Failed::BasicGroup => f.write_str("basic group, no per-user restrictions"),
            Failed::Telegram(e) => e.fmt(f),
        }
    }
}

impl Failed {
    pub fn told(&self) -> String {
        match self {
            Failed::Protected => PROTECTED.to_owned(),
            Failed::BasicGroup => BASIC_GROUP.to_owned(),
            Failed::Telegram(grammers_client::InvocationError::Rpc(rpc)) => {
                told_rpc(&rpc.name, rpc.value)
            }
            Failed::Telegram(_) => {
                "✗ ارتباط با تلگرام برقرار نشد. چند لحظه بعد دوباره بفرستید.".to_owned()
            }
        }
    }
}

const PROTECTED: &str = "✗ او ادمین است. تا از ادمینی عزل نشود محدود نمی شود.";

pub const BASIC_GROUP: &str =
    "✗ این گروه معمولی است. برای سکوت و بن باید به سوپرگروه ارتقا پیدا کند.";

fn told_rpc(name: &str, value: Option<u32>) -> String {
    match name {
        "USER_ADMIN_INVALID" | "USER_CREATOR" => PROTECTED.to_owned(),
        "CHAT_INVALID" => BASIC_GROUP.to_owned(),
        "CHAT_ADMIN_REQUIRED" | "RIGHT_FORBIDDEN" | "CHAT_WRITE_FORBIDDEN" => {
            "✗ ربات ادمین نیست یا اجازه «بن کاربران» ندارد.".to_owned()
        }
        "USER_NOT_PARTICIPANT" | "PARTICIPANT_ID_INVALID" => {
            "✗ این کاربر عضو گروه نیست.".to_owned()
        }
        "USER_ID_INVALID" | "PEER_ID_INVALID" | "INPUT_USER_DEACTIVATED" => {
            "✗ این کاربر پیدا نشد یا حسابش پاک شده است.".to_owned()
        }
        "CHANNEL_PRIVATE" | "CHANNEL_INVALID" => {
            "✗ ربات دیگر به این گروه دسترسی ندارد.".to_owned()
        }
        "CHANNEL_MONOFORUM_UNSUPPORTED" => {
            "✗ این چت از محدود کردن کاربران پشتیبانی نمی کند.".to_owned()
        }

        "BANNED_RIGHTS_INVALID" => "✗ انجام نشد · تنظیم دسترسی نامعتبر بود.".to_owned(),
        "FLOOD_WAIT" | "FLOOD_PREMIUM_WAIT" | "SLOWMODE_WAIT" => match value {
            Some(secs) => format!(
                "✗ تلگرام موقتا ربات را محدود کرده. {} دیگر دوباره بفرستید.",
                super::log::duration_label(u64::from(secs))
            ),
            None => "✗ تلگرام موقتا ربات را محدود کرده. کمی بعد دوباره بفرستید.".to_owned(),
        },

        other => format!("✗ انجام نشد · {other}\nمطمئن شوید ربات ادمین است و اجازه محدود کردن کاربران دارد."),
    }
}

pub async fn apply(
    ctx: &Ctx,
    chat: grammers_client::session::types::PeerRef,
    target: grammers_client::session::types::PeerRef,
    action: Action,
    duration: Option<Duration>,
    by: By<'_>,
) -> Result<(), Failed> {
    if matches!(action, Mute | Ban)
        && !by.admins_too
        && let (Some(chat_id), Some(user)) = (chat.id.bot_api_dialog_id(), target.id.bare_id())
        && super::is_admin(ctx, chat, chat_id, user).await
    {
        return Err(Failed::Protected);
    }

    let duration = honoured(duration);
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

    pub admins_too: bool,
}

fn until_date(duration: Option<Duration>) -> i32 {
    let Some(duration) = duration else {
        return 0;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    i32::try_from(now + duration.as_secs()).unwrap_or(i32::MAX)
}

async fn apply_rights(
    ctx: &Ctx,
    chat: grammers_client::session::types::PeerRef,
    target: grammers_client::session::types::PeerRef,
    action: Action,
    duration: Option<Duration>,
) -> Result<(), Failed> {
    if action == Mute {
        if chat.id.kind() != grammers_client::session::types::PeerKind::Channel {
            return Err(Failed::BasicGroup);
        }
        return ctx
            .client
            .invoke(&grammers_client::tl::functions::channels::EditBanned {
                channel: chat.into(),
                participant: target.into(),
                banned_rights: super::rights::muted(until_date(duration)).into(),
            })
            .await
            .map(drop)
            .map_err(Failed::Telegram);
    }

    let mut rights = ctx.client.set_banned_rights(chat, target);
    if let Some(duration) = duration {
        rights = rights.duration(duration);
    }
    match action {
        Ban => rights.view_messages(false).await,
        _ => rights.await,
    }
    .map_err(Failed::Telegram)
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
    fn the_argument_must_name_a_user() {
        use super::super::arg_names_a_user as names;
        let word = "میکنم";

        assert!(!names(p(&format!("بن {word}")).target.unwrap()));
        assert!(names(p("بن @someone").target.unwrap()));
        assert!(names(p("بن 12345").target.unwrap()));
        assert!(names(p("بن ۱۲۳۴۵").target.unwrap()));

        assert_eq!(p("بن").target, None);
        assert_eq!(p("بن 10 دقیقه").target, None);
    }

    #[test]
    fn a_failure_says_which_one_it_was() {
        assert_eq!(told_rpc("USER_ADMIN_INVALID", None), PROTECTED);
        assert!(told_rpc("CHAT_ADMIN_REQUIRED", None).contains("ربات ادمین نیست"));
        assert!(told_rpc("USER_NOT_PARTICIPANT", None).contains("عضو گروه نیست"));
        assert!(told_rpc("FLOOD_WAIT", Some(120)).contains("2 دقیقه"));
        assert!(told_rpc("FLOOD_WAIT", None).contains("کمی بعد"));

        assert_eq!(told_rpc("CHAT_INVALID", None), BASIC_GROUP);
        assert_eq!(Failed::BasicGroup.told(), BASIC_GROUP);

        assert_ne!(told_rpc("ADMIN_RANK_EMPTY", None), PROTECTED);

        assert!(told_rpc("SOMETHING_NEW", None).contains("SOMETHING_NEW"));
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
    fn a_duration_telegram_would_read_as_forever_is_corrected() {
        let secs = |d: Option<Duration>| d.map(|d| d.as_secs());

        assert_eq!(secs(honoured(Some(Duration::from_secs(1)))), Some(30));
        assert_eq!(secs(honoured(Some(Duration::from_secs(29)))), Some(30));

        assert_eq!(secs(honoured(Some(Duration::from_secs(30)))), Some(30));
        assert_eq!(secs(honoured(Some(Duration::from_secs(600)))), Some(600));

        assert_eq!(honoured(Some(Duration::from_secs(367 * 86_400))), None);
        assert_eq!(honoured(None), None);

        assert_eq!(secs(honoured(p("سکوت 1 ثانیه").duration)), Some(30));
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
