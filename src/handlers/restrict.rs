use std::time::Duration;

use grammers_client::message::Message;

use super::{Ctx, esc, name_of};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Mute,
    Unmute,
    Ban,
    Unban,

    Kick,
}

use Action::*;

pub const WIPE_BAN: &str = "wipe_ban_on";

pub const WIPE_MUTE: &str = "wipe_mute_on";

fn wipe_key(action: Action) -> Option<&'static str> {
    match action {
        Ban => Some(WIPE_BAN),
        Mute => Some(WIPE_MUTE),
        Unmute | Unban | Kick => None,
    }
}

const WIPE_ROUNDS: usize = 32;

const WIPE_CHUNK: usize = 100;

pub fn remember(ctx: &Ctx, chat: i64, message: &Message) {
    let armed = ctx
        .settings
        .with_chat(chat, |s| s.is_locked(WIPE_BAN) || s.is_locked(WIPE_MUTE));
    if !armed {
        return;
    }
    let Some(user) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return;
    };
    ctx.remember_said(chat, user, message.id());
}

pub async fn wipe_history(
    ctx: &Ctx,
    chat: grammers_client::session::types::PeerRef,
    chat_id: i64,
    target: grammers_client::session::types::PeerRef,
) -> usize {
    use grammers_client::session::types::PeerKind;

    if chat.id.kind() != PeerKind::Channel {
        return 0;
    }
    let Some(user) = target.id.bare_id() else {
        return 0;
    };

    let mut deleted = cleaner_wipe(ctx, chat_id, target).await;

    let mine = ctx.take_said(chat_id, user);
    for chunk in mine.chunks(WIPE_CHUNK) {
        match ctx.client.delete_messages(chat, chunk).await {
            Ok(gone) => deleted += gone,
            Err(e) => {
                eprintln!("wipe: {chat_id}: could not delete {} ids: {e}", chunk.len());
                break;
            }
        }
    }
    deleted
}

async fn cleaner_wipe(
    ctx: &Ctx,
    chat_id: i64,
    target: grammers_client::session::types::PeerRef,
) -> usize {
    use grammers_client::tl;

    let Some(user) = ctx.user_client() else {
        return 0;
    };

    let Some(chat_ref) = super::cleaner::chat_ref(ctx, &user, chat_id).await else {
        eprintln!("wipe: {chat_id}: cleaner could not resolve the chat");
        return 0;
    };
    let Some(user_id) = target.id.bare_id() else {
        return 0;
    };

    let Some(target) = super::cleaner::member_ref(&user, chat_ref, user_id, None).await else {
        eprintln!("wipe: {chat_id}: cleaner could not find participant {user_id}");
        return 0;
    };
    let mut deleted = 0;
    for _ in 0..WIPE_ROUNDS {
        let asked = user
            .invoke_outbound(&tl::functions::channels::DeleteParticipantHistory {
                channel: chat_ref.into(),
                participant: target.into(),
            })
            .await;
        match asked {
            Ok(tl::enums::messages::AffectedHistory::History(done)) => {
                deleted += done.pts_count.max(0) as usize;
                if done.offset == 0 {
                    break;
                }
            }
            Err(e) => {
                eprintln!("wipe: {chat_id}: cleaner could not clear history: {e}");
                break;
            }
        }
    }
    deleted
}

pub const COMMANDS: &[(&str, Action)] = &[
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
    let Some(parsed) = parse(view.digits()) else {
        return false;
    };
    run(ctx, message, parsed).await
}

pub async fn handle_custom_setup(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if let Some(rest) = strip_command(text, CUSTOM_SET) {
        return set_custom(ctx, message, chat, rest).await;
    }
    if let Some(rest) = strip_command(text, CUSTOM_REMOVE) {
        return remove_custom(ctx, message, chat, rest).await;
    }
    false
}

pub async fn handle_custom(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if ctx.settings.indexed_empty(chat, CUSTOM_PREFIX) {
        return false;
    }
    let text = view.digits();

    let words: Vec<&str> = text.split_whitespace().take(MAX_TRIGGER_WORDS + 1).collect();
    let candidates = trigger_candidates(&words);

    let raw = view.text();
    let legacy = (raw != text).then(|| {
        let words: Vec<&str> = raw.split_whitespace().take(MAX_TRIGGER_WORDS + 1).collect();
        let candidates = trigger_candidates(&words);
        (words, candidates)
    });

    let found = ctx.settings.with_chat(chat, |settings| {
        let hit = |candidates: Vec<(usize, String)>| {
            candidates.into_iter().find_map(|(count, candidate)| {
                let action = action_of_code(settings.value(&custom_key(&candidate))?)?;
                Some((count, action))
            })
        };
        match hit(candidates) {
            Some(found) => Some((false, found)),
            None => legacy
                .as_ref()
                .and_then(|(_, candidates)| hit(candidates.clone()))
                .map(|found| (true, found)),
        }
    });
    let Some((was_legacy, (consumed, action))) = found else {
        return false;
    };

    let (text, words) = match (was_legacy, &legacy) {
        (true, Some((words, _))) => (raw, words),
        _ => (text, &words),
    };
    run(ctx, message, parse_rest(action, tail_after(text, words, consumed))).await
}

fn tail_after<'a>(text: &'a str, words: &[&str], consumed: usize) -> &'a str {
    if consumed >= words.len() {
        return "";
    }
    &text[word_offset(text, words, consumed)..]
}

fn trigger_candidates(words: &[&str]) -> Vec<(usize, String)> {
    let longest = words.len().min(MAX_TRIGGER_WORDS);
    (1..=longest)
        .rev()
        .map(|count| (count, words[..count].join(" ").to_lowercase()))
        .collect()
}

async fn run(ctx: &Ctx, message: &Message, parsed: Parsed<'_>) -> bool {
    let (action, arg) = (parsed.action, parsed.target);

    let Some(named) = super::named(message, arg) else {
        return false;
    };

    let needed = match action {
        Ban | Unban | Kick => super::limits::BAN,
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

    let duration = if action == Kick { None } else { parsed.duration };

    let by = super::sender_of(message);
    let result = apply(
        ctx,
        chat_ref,
        target,
        action,
        duration,
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
        Ok(wiped) => {
            let kick_only = action == Ban
                && chat_ref.id.kind() != grammers_client::session::types::PeerKind::Channel;
            let what = match action {
                Mute => "✓ سکوت شد",
                Unmute => "✗ سکوتش برداشته شد",
                Ban if kick_only => "✓ از گروه اخراج شد",
                Ban => "✓ بن شد",
                Unban => "✗ بنش برداشته شد",
                Kick => "✓ از گروه اخراج شد",
            };
            let note = match kick_only {
                true => "\nدر گروه معمولی بن دائمی نیست. برای بن به سوپرگروه ارتقا دهید.",
                false => "",
            };
            let how_long = match (duration, honoured(duration)) {
                (Some(_), Some(held)) => {
                    format!(" به مدت {}", super::log::duration_label(held.as_secs()))
                }
                (Some(_), None) => " به صورت دائمی".to_owned(),
                _ => String::new(),
            };

            let asked_to_wipe = wipe_key(action).is_some_and(|key| {
                message
                    .peer_id()
                    .bot_api_dialog_id()
                    .is_some_and(|chat| ctx.settings.is_locked(chat, key))
            });
            let swept = match (wiped, asked_to_wipe) {
                (0, false) => String::new(),
                (0, true) => "\n🧹 پیامی از او برای پاک کردن پیدا نشد.".to_owned(),
                (n, _) => format!("\n🧹 {n} پیام او هم پاک شد."),
            };
            message
                .reply(format!(
                    "{target_name} {what}{how_long}.\nتوسط: {by}{note}{swept}"
                ))
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
        "CHANNEL_PRIVATE" | "CHANNEL_INVALID" => "✗ ربات دیگر به این گروه دسترسی ندارد.".to_owned(),
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

        other => format!(
            "✗ انجام نشد · {other}\nمطمئن شوید ربات ادمین است و اجازه محدود کردن کاربران دارد."
        ),
    }
}

pub async fn apply(
    ctx: &Ctx,
    chat: grammers_client::session::types::PeerRef,
    target: grammers_client::session::types::PeerRef,
    action: Action,
    duration: Option<Duration>,
    by: By<'_>,
) -> Result<usize, Failed> {
    if matches!(action, Mute | Ban | Kick)
        && !by.admins_too
        && let (Some(chat_id), Some(user)) = (chat.id.bot_api_dialog_id(), target.id.bare_id())
        && super::is_admin(ctx, chat, chat_id, user).await
    {
        return Err(Failed::Protected);
    }

    let duration = honoured(duration);
    let done = apply_rights(ctx, chat, target, action, duration).await;

    let wiped = if done.is_ok()
        && let Some(key) = wipe_key(action)
        && let Some(chat_id) = chat.id.bot_api_dialog_id()
        && ctx.settings.is_locked(chat_id, key)
    {
        wipe_history(ctx, chat, chat_id, target).await
    } else {
        0
    };

    if done.is_ok()
        && let (Some(chat_id), Some(user)) = (chat.id.bot_api_dialog_id(), target.id.bare_id())
    {
        let mut extra = Vec::new();
        if let Some(duration) = duration {
            extra.push(("مدت", super::log::duration_label(duration.as_secs())));
        } else if matches!(action, Mute | Ban) {
            extra.push(("مدت", "دائمی".to_owned()));
        }
        if wiped > 0 {
            extra.push(("پیام های پاک شده", wiped.to_string()));
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
                    Kick => "کیک",
                },
                target: Some((user, by.target_name)),
                actor: by.actor,
                reason: Some(by.reason),
                extra,
            },
        )
        .await;
    }

    if done.is_ok()
        && matches!(action, Unmute | Unban)
        && let Some(user) = target.id.bare_id()
    {
        ctx.forget_bio(user);
    }
    done.map(|()| wiped)
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
            .invoke_outbound(&grammers_client::tl::functions::channels::EditBanned {
                channel: chat.into(),
                participant: target.into(),
                banned_rights: super::rights::muted(until_date(duration)).into(),
            })
            .await
            .map(drop)
            .map_err(Failed::Telegram);
    }

    if action == Kick {
        return ctx
            .client
            .kick_participant(chat, target)
            .await
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
    Some(parse_rest(action, rest))
}

fn parse_rest(action: Action, rest: &str) -> Parsed<'_> {
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
    parsed
}

fn duration_at(words: &[&str], i: usize) -> Option<(u64, usize)> {
    let word = words[i];

    let split = word
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(word.len());
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

pub(crate) fn duration_of(tail: &str) -> Option<Duration> {
    let words: Vec<&str> = tail.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let (seconds, next) = duration_at(&words, 0)?;
    (next == words.len()).then(|| Duration::from_secs(seconds))
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

pub const CUSTOM_PREFIX: &str = "cmd:";
const MAX_CUSTOM_COMMANDS: usize = 20;
const MAX_CUSTOM_TRIGGER_CHARS: usize = 40;

const MAX_TRIGGER_WORDS: usize = 4;

pub const CUSTOM_SET: &[&str] = &["تنظیم دستور", "افزودن دستور"];
pub const CUSTOM_REMOVE: &[&str] = &["حذف دستور", "پاک دستور"];

const ACTION_WORDS: &[(&str, Action)] = &[
    ("بن", Ban),
    ("سیک", Ban),
    ("کیک", Kick),
    ("سکوت", Mute),
    ("خفه", Mute),
];

pub fn custom_key(word: &str) -> String {
    format!("{CUSTOM_PREFIX}{word}")
}

fn action_code(action: Action) -> &'static str {
    match action {
        Ban => "ban",
        Kick => "kick",
        Mute => "mute",
        Unmute | Unban => "",
    }
}

fn action_of_code(code: &str) -> Option<Action> {
    match code {
        "ban" => Some(Ban),
        "kick" => Some(Kick),
        "mute" => Some(Mute),
        _ => None,
    }
}

pub fn action_label(action: Action) -> &'static str {
    match action {
        Ban => "بن",
        Kick => "کیک",
        Mute => "سکوت",
        Unmute | Unban => "",
    }
}

pub fn custom_triggers(ctx: &Ctx, chat: i64) -> Vec<(String, Action)> {
    let mut found: Vec<(String, Action)> = ctx
        .settings
        .values_with_prefix(chat, CUSTOM_PREFIX)
        .into_iter()
        .filter_map(|(word, code)| action_of_code(&code).map(|action| (word, action)))
        .collect();
    found.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    found
}

fn normalize_phrase(phrase: &str) -> String {
    super::digits(phrase)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn strip_command<'a>(text: &'a str, commands: &[&str]) -> Option<&'a str> {
    commands.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
    })
}

fn action_word(label: &str) -> Option<Action> {
    ACTION_WORDS
        .iter()
        .find(|(name, _)| *name == label)
        .map(|&(_, action)| action)
}

fn is_builtin_command(word: &str) -> bool {
    COMMANDS.iter().any(|&(builtin, _)| builtin == word)
}

async fn set_custom(ctx: &Ctx, message: &Message, chat: i64, rest: &str) -> bool {
    if rest.is_empty() {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        let _ = message
            .reply(
                "بنویسید: «تنظیم دستور <کلمه یا عبارت> <بن یا کیک یا سکوت>»\n\
                 مثال: «تنظیم دستور زنجیر بن» یا «تنظیم دستور بیرونش کن کیک»",
            )
            .await;
        return true;
    }

    let Some((word, label)) = rest.rsplit_once(char::is_whitespace) else {
        return false;
    };
    let Some(action) = action_word(label) else {
        return false;
    };

    let word = normalize_phrase(word);

    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    if word.is_empty() {
        let _ = message
            .reply("بعد از «تنظیم دستور» یک کلمه یا عبارت بنویسید، مثل «زنجیر».")
            .await;
        return true;
    }
    if word.split_whitespace().count() > MAX_TRIGGER_WORDS {
        let _ = message
            .reply(format!(
                "کلمه دستور باید حداکثر {MAX_TRIGGER_WORDS} کلمه باشد."
            ))
            .await;
        return true;
    }
    if word.chars().count() > MAX_CUSTOM_TRIGGER_CHARS {
        let _ = message
            .reply(format!(
                "کلمه دستور باید حداکثر {MAX_CUSTOM_TRIGGER_CHARS} نویسه باشد."
            ))
            .await;
        return true;
    }
    if is_builtin_command(&word) {
        let _ = message
            .reply(format!("«{}» از قبل یک دستور آماده ربات است.", esc(&word)))
            .await;
        return true;
    }
    let key = custom_key(&word);
    if ctx.settings.value(chat, &key).is_none()
        && custom_triggers(ctx, chat).len() >= MAX_CUSTOM_COMMANDS
    {
        let _ = message
            .reply(format!(
                "لیست دستورهای سفارشی پر است ({MAX_CUSTOM_COMMANDS} مورد)."
            ))
            .await;
        return true;
    }
    if !ctx.settings.set_value(chat, &key, action_code(action)).await {
        let _ = message
            .reply("ذخیره دستور انجام نشد؛ ظرفیت تنظیمات یا پایگاه داده را بررسی کنید.")
            .await;
        return true;
    }
    let _ = message
        .reply(format!(
            "✓ «{}» دستور {} شد.",
            esc(&word),
            action_label(action)
        ))
        .await;
    true
}

async fn remove_custom(ctx: &Ctx, message: &Message, chat: i64, rest: &str) -> bool {
    if rest.is_empty() {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        let _ = message
            .reply("بنویسید: «حذف دستور <کلمه یا عبارت>»")
            .await;
        return true;
    }

    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let word = normalize_phrase(rest);
    let mut existed = ctx.settings.set(chat, &custom_key(&word), false).await;

    if !existed {
        let legacy = rest.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if legacy != word {
            existed = ctx.settings.set(chat, &custom_key(&legacy), false).await;
        }
    }
    let _ = message
        .reply(if existed {
            format!("✗ دستور «{}» حذف شد.", esc(&word))
        } else {
            format!("«{}» در لیست دستورهای سفارشی نبود.", esc(&word))
        })
        .await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Parsed<'_> {
        parse(text).expect("should parse")
    }

    #[test]
    fn a_duration_must_be_the_whole_tail() {
        let secs = |text: &str| duration_of(&super::super::digits(text)).map(|d| d.as_secs());

        assert_eq!(secs("2 ساعت"), Some(7_200));
        assert_eq!(secs("۲ ساعت"), Some(7_200));
        assert_eq!(secs("2ساعت"), Some(7_200));
        assert_eq!(secs("30 دقیقه"), Some(1_800));
        assert_eq!(secs("1 روز"), Some(86_400));
        assert_eq!(secs("2h"), Some(7_200));

        assert_eq!(secs("2 ساعت لطفا"), None);
        assert_eq!(secs("رو بردار"), None);
        assert_eq!(secs("2"), None);
        assert_eq!(secs(""), None);
        assert_eq!(secs("ساعت"), None);
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
        assert_eq!(
            p("بن @ali 1 روز").duration,
            Some(Duration::from_secs(86_400))
        );
        assert_eq!(p("بن @ali 1 روز").target, Some("@ali"));
        assert_eq!(
            p("سکوت 1 هفته").duration,
            Some(Duration::from_secs(604_800))
        );

        assert_eq!(p("سکوت 12345").target, Some("12345"));
        assert_eq!(p("سکوت 12345").duration, None);

        assert_eq!(p("سکوت 5 دقیقه @ali").target, Some("@ali"));
    }

    #[test]
    fn strip_command_respects_the_delimiter_rule() {
        assert_eq!(
            strip_command("تنظیم دستور زنجیر بن", CUSTOM_SET),
            Some("زنجیر بن")
        );
        assert_eq!(strip_command("تنظیم دستور", CUSTOM_SET), Some(""));

        assert_eq!(strip_command("تنظیم دستورخاصی چیزی", CUSTOM_SET), None);
        assert_eq!(strip_command("سلام", CUSTOM_SET), None);
        assert_eq!(strip_command("حذف دستور زنجیر", CUSTOM_REMOVE), Some("زنجیر"));
    }

    #[test]
    fn custom_action_words_resolve_and_round_trip_through_storage() {
        assert_eq!(action_word("بن"), Some(Ban));
        assert_eq!(action_word("سیک"), Some(Ban));
        assert_eq!(action_word("کیک"), Some(Kick));
        assert_eq!(action_word("سکوت"), Some(Mute));
        assert_eq!(action_word("خفه"), Some(Mute));
        assert_eq!(action_word("توهین"), None);
        assert_eq!(action_word(""), None);

        for &(_, action) in ACTION_WORDS {
            assert_eq!(action_of_code(action_code(action)), Some(action));
            assert!(!action_label(action).is_empty());
        }
        assert_eq!(action_of_code("unban"), None);
    }

    #[test]
    fn a_custom_word_cannot_shadow_a_builtin_command() {
        for (builtin, _) in COMMANDS {
            assert!(is_builtin_command(builtin));
        }
        assert!(!is_builtin_command("زنجیر"));
    }

    #[test]
    fn a_phrase_registers_under_the_key_a_lookup_would_use() {
        assert_eq!(normalize_phrase("بن۲"), "بن2");
        assert_eq!(normalize_phrase("  بیرونش   کن  "), "بیرونش کن");

        let digitized = super::super::digits("بیرونش۲ کن @ali");
        let words: Vec<&str> = digitized.split_whitespace().collect();
        let (_, looked_up) = trigger_candidates(&words)
            .into_iter()
            .find(|(count, _)| *count == 2)
            .expect("a two-word candidate exists");
        assert_eq!(normalize_phrase("بیرونش۲ کن"), looked_up);
    }

    #[test]
    fn trigger_candidates_try_longest_first_and_stay_bounded() {
        let words = ["بیرون", "کن", "الان", "لطفا", "زود"];
        let candidates = trigger_candidates(&words);

        assert_eq!(candidates.len(), MAX_TRIGGER_WORDS);
        assert_eq!(candidates[0], (4, "بیرون کن الان لطفا".to_owned()));
        assert_eq!(candidates[MAX_TRIGGER_WORDS - 1], (1, "بیرون".to_owned()));

        assert_eq!(trigger_candidates(&[]).len(), 0);
    }

    #[test]
    fn tail_after_finds_what_follows_a_multi_word_trigger() {
        let text = "بیرون کن @ali 1 روز";
        let words: Vec<&str> = text.split_whitespace().collect();

        assert_eq!(tail_after(text, &words, 2), "@ali 1 روز");
        assert_eq!(tail_after(text, &words, 1), "کن @ali 1 روز");
        assert_eq!(tail_after(text, &words, words.len()), "");
    }

    #[test]
    fn a_full_length_trigger_keeps_its_tail_in_a_longer_message() {
        let text = "a b c d e f";
        let words: Vec<&str> = text.split_whitespace().take(MAX_TRIGGER_WORDS + 1).collect();

        assert_eq!(words.len(), MAX_TRIGGER_WORDS + 1);
        assert_eq!(tail_after(text, &words, MAX_TRIGGER_WORDS), "e f");
    }

    #[test]
    fn a_custom_trigger_shares_the_built_in_tail_grammar() {
        let parsed = parse_rest(Kick, "@ali");
        assert_eq!(parsed.target, Some("@ali"));
        assert_eq!(parsed.duration, None);

        let parsed = parse_rest(Ban, "@ali 1 روز");
        assert_eq!(parsed.target, Some("@ali"));
        assert_eq!(parsed.duration, Some(Duration::from_secs(86_400)));
    }
}
