use grammers_client::media::Media;
use grammers_client::message::Message;
use grammers_client::tl;

use super::{Ctx, can_manage};

pub struct Lock {
    pub key: &'static str,

    pub names: &'static [&'static str],
    pub matches: fn(&View) -> bool,
}

pub struct View<'a> {
    message: &'a Message,
    media: Option<Media>,
    text: &'a str,
    lower: std::sync::OnceLock<String>,
    digits: std::sync::OnceLock<std::borrow::Cow<'a, str>>,
    tight: std::sync::OnceLock<Option<String>>,
    ascii: std::sync::OnceLock<Option<String>>,
}

impl<'a> View<'a> {
    pub fn new(message: &'a Message) -> Self {
        Self {
            message,
            media: message.media(),
            text: message.text().trim(),
            lower: std::sync::OnceLock::new(),
            tight: std::sync::OnceLock::new(),
            ascii: std::sync::OnceLock::new(),
            digits: std::sync::OnceLock::new(),
        }
    }

    pub fn lower(&self) -> &str {
        self.lower.get_or_init(|| folded(self.message.text()))
    }

    pub fn tight(&self) -> &str {
        match self.tight.get_or_init(|| tightened(self.lower())) {
            Some(tight) => tight,
            None => self.lower(),
        }
    }

    pub fn ascii(&self) -> &str {
        match self.ascii.get_or_init(|| latinised(self.tight())) {
            Some(ascii) => ascii,
            None => self.tight(),
        }
    }

    pub fn text(&self) -> &str {
        self.text
    }

    pub fn digits(&self) -> &str {
        self.digits.get_or_init(|| super::digits(self.text))
    }

    pub fn media(&self) -> Option<&Media> {
        self.media.as_ref()
    }

    fn entities(&self) -> Option<&Vec<tl::enums::MessageEntity>> {
        self.message.fmt_entities()
    }
}

pub fn is_ai(key: &str) -> bool {
    key == super::nsfw::LOCK
        || key == super::ocr::LOCK
        || key == super::trade::LOCK
        || super::concepts::CONCEPTS.iter().any(|c| c.key == key)
}

pub fn plain() -> impl Iterator<Item = &'static Lock> {
    LOCKS.iter().filter(|lock| !is_ai(lock.key))
}

pub async fn try_set(
    ctx: &Ctx,
    chat: i64,
    key: &'static str,
    on: bool,
) -> Result<bool, crate::state::SettingsWriteError> {
    let changed = ctx.settings.try_set(chat, key, on).await?;
    super::strict::try_sync_pick(ctx, chat, key, on).await?;
    if changed {
        super::bots::on_lock_set(ctx, chat, key, on).await;
    }
    Ok(changed)
}

pub const LOCKS: &[Lock] = &[
    Lock {
        key: "links",
        names: &["لینک", "لینک ها", "لینکها"],
        matches: is_link,
    },
    Lock {
        key: "photo",
        names: &["عکس", "تصویر"],
        matches: is_photo,
    },
    Lock {
        key: "video",
        names: &["ویدیو", "فیلم", "ویدئو"],
        matches: is_video,
    },
    Lock {
        key: "gif",
        names: &["گیف"],
        matches: is_gif,
    },
    Lock {
        key: "sticker",
        names: &["استیکر"],
        matches: is_sticker,
    },
    Lock {
        key: "animsticker",
        names: &["استیکر متحرک", "استیکرمتحرک"],
        matches: is_animated_sticker,
    },
    Lock {
        key: "music",
        names: &["موزیک", "آهنگ", "اهنگ"],
        matches: is_music,
    },
    Lock {
        key: "voice",
        names: &["ویس", "صدا"],
        matches: is_voice,
    },
    Lock {
        key: "file",
        names: &["فایل", "سند"],
        matches: is_file,
    },
    Lock {
        key: "contact",
        names: &["مخاطب", "کانتکت"],
        matches: is_contact,
    },
    Lock {
        key: "location",
        names: &["مکان", "لوکیشن"],
        matches: is_location,
    },
    Lock {
        key: "poll",
        names: &["نظرسنجی"],
        matches: is_poll,
    },
    Lock {
        key: "dice",
        names: &["تاس", "بازی"],
        matches: is_dice,
    },
    Lock {
        key: "forward_channel",
        names: &["فوروارد از کانال", "فوروارد کانال"],
        matches: is_forward_channel,
    },
    Lock {
        key: "forward_user",
        names: &["فوروارد از کاربر", "فوروارد کاربر"],
        matches: is_forward_user,
    },
    Lock {
        key: "hyperlink",
        names: &["لینک مخفی", "هایپرلینک", "لینک متنی"],
        matches: is_hyperlink,
    },
    Lock {
        key: "hashtag",
        names: &["هشتگ"],
        matches: is_hashtag,
    },
    Lock {
        key: "emoji",
        names: &["ایموجی", "شکلک"],
        matches: has_emoji,
    },
    Lock {
        key: "premoji",
        names: &["ایموجی پرمیوم", "ایموجی ویژه"],
        matches: has_custom_emoji,
    },
    Lock {
        key: "english",
        names: &["انگلیسی", "لاتین"],
        matches: has_english,
    },
    Lock {
        key: "persian",
        names: &["فارسی", "پارسی"],
        matches: has_persian,
    },
    Lock {
        key: "button",
        names: &["دکمه", "دکمه شیشه ای", "اینلاین"],
        matches: has_inline_button,
    },
    Lock {
        key: USERNAME,
        names: &["یوزرنیم", "یوزر", "آیدی"],
        matches: is_username,
    },
    Lock {
        key: MENTION,
        names: &["تگ", "منشن"],
        matches: is_mention,
    },
    Lock {
        key: "media",
        names: &["مدیا", "رسانه"],
        matches: is_media,
    },
    Lock {
        key: "anon",
        names: &["ناشناس", "هویت ناشناس", "کانال"],
        matches: is_anonymous_channel,
    },
    Lock {
        key: "spoiler",
        names: &["اسپویلر", "اسپویل"],
        matches: is_spoiler,
    },
    Lock {
        key: "story",
        names: &["استوری"],
        matches: is_story,
    },
    Lock {
        key: "pin",
        names: &["اعلان سنجاق", "اعلان پین"],
        matches: is_pin_notice,
    },
    Lock {
        key: "promoter",
        names: &["تبچی", "تبلیغ", "تبلیغات"],
        matches: is_promoter,
    },
    Lock {
        key: COMMANDS,
        names: &["دستورات عمومی", "دستورات", "کامند"],
        matches: never,
    },
    Lock {
        key: BOTCALL,
        names: &["دستور ربات", "دستور بات", "کامند ربات"],
        matches: is_bot_call,
    },
    Lock {
        key: EDIT,
        names: &["ویرایش", "ادیت"],
        matches: never,
    },
    Lock {
        key: SERVICE,
        names: &["سرویس", "سرویس تلگرام", "پیام سرویس"],
        matches: never,
    },
    Lock {
        key: super::bots::LOCK,
        names: &["ربات", "بات"],
        matches: never,
    },
    Lock {
        key: super::biolink::LOCK,
        names: &["لینک در بایو", "لینک بایو", "بایو"],
        matches: never,
    },
    Lock {
        key: super::comment::LOCK,
        names: &["کامنت", "کامنت ها", "کامنتها"],
        matches: never,
    },
    Lock {
        key: super::nsfw::LOCK,
        names: &["محتوای غیراخلاقی", "غیراخلاقی", "مستهجن"],
        matches: never,
    },
    Lock {
        key: "c_cig",
        names: &["سیگار", "دخانیات", "قلیان"],
        matches: never,
    },
    Lock {
        key: "c_alc",
        names: &["مشروب", "الکل", "مشروبات"],
        matches: never,
    },
    Lock {
        key: "c_gun",
        names: &["اسلحه", "سلاح"],
        matches: never,
    },
    Lock {
        key: "c_bet",
        names: &["قمار", "شرط بندی", "شرطبندی"],
        matches: never,
    },
    Lock {
        key: "c_drg",
        names: &["مواد", "مواد مخدر"],
        matches: never,
    },
    Lock {
        key: "c_bld",
        names: &["خون", "خونریزی"],
        matches: never,
    },
    Lock {
        key: super::ocr::LOCK,
        names: &["تبلیغ در تصویر", "تبلیغ تصویری", "لینک در تصویر"],
        matches: never,
    },
    Lock {
        key: super::trade::LOCK,
        names: &["خرید و فروش", "خرید فروش", "معامله"],
        matches: never,
    },
];

const ALL: &[&str] = &["همه", "همه چیز", "کل"];

const FORWARD: &[&str] = &["فوروارد", "فروارد", "هدایت"];

const BOT: &[&str] = &["ربات", "بات"];

const GROUP: &[&str] = &["گروه", "کل گروه"];

pub const TIMED_RANGE: (u64, u64) = (60, 30 * 86_400);

pub const EDIT: &str = "edit";

pub const SERVICE: &str = "service";
pub const USERNAME: &str = "username";
pub const MENTION: &str = "mention";
pub const BOTCALL: &str = "botcall";
pub const FORWARD_CHANNEL: &str = "forward_channel";
pub const FORWARD_USER: &str = "forward_user";
pub const STATUS: &[&str] = &["قفل ها", "قفلها", "لیست قفل", "وضعیت قفل"];

pub const COMMANDS: &str = "commands";

const PUBLIC: &[&[&str]] = &[
    super::ping::COMMANDS,
    super::report::COMMANDS,
    super::config::HELP,
    super::config::ADMIN_LIST,
    super::extras::SHOW_RULES,
    super::stats::INFO,
    super::currency::COMMANDS,
    STATUS,
];

fn is_public_command(text: &str) -> bool {
    PUBLIC.iter().any(|names| names.contains(&text))
}

async fn ignored_command(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
    chat: i64,
    text: &str,
) -> bool {
    ctx.settings.is_locked(chat, COMMANDS)
        && is_public_command(text)
        && !super::is_exempt(ctx, message).await
}

fn names_a_lock(name: &str) -> bool {
    ALL.contains(&name)
        || FORWARD.contains(&name)
        || GROUP.contains(&name)
        || BOT.contains(&name)
        || super::pinlock::NAMES.contains(&name)
        || LOCKS.iter().any(|lock| lock.names.contains(&name))
}

pub async fn handle(ctx: &std::sync::Arc<Ctx>, message: &Message, view: &View<'_>) -> bool {
    let text = view.digits();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if STATUS.contains(&text) {
        if moderate(ctx, message, chat, view).await
            || ignored_command(ctx, message, chat, text).await
        {
            return true;
        }
        let (active, models): (Vec<&str>, usize) = ctx.settings.with_chat(chat, |settings| {
            (
                plain()
                    .filter(|lock| settings.is_locked(lock.key))
                    .map(|lock| lock.names[0])
                    .collect(),
                LOCKS
                    .iter()
                    .filter(|lock| is_ai(lock.key) && settings.is_locked(lock.key))
                    .count(),
            )
        });
        let smart = match models {
            0 => String::new(),
            n => format!("\nنگهبان هوشمند · {n} روشن"),
        };
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::LockManagement,
            if active.is_empty() {
                format!(
                    "هیچ قفلی فعال نیست. ({} قفل در دسترس){smart}",
                    plain().count()
                )
            } else {
                format!(
                    "قفل های فعال ({} از {}):\n{}{smart}",
                    active.len(),
                    plain().count(),
                    active.join("، ")
                )
            },
        )
        .await;
        return true;
    }

    if let Some((on, name)) = parse(text) {
        if !can_manage(ctx, message).await {
            return moderate(ctx, message, chat, view).await;
        }
        let timed = on.then(|| timed_group(view, name)).flatten();
        if !names_a_lock(name) && timed.is_none() {
            return false;
        }
        if !super::limits::allowed(ctx, message, super::limits::SET) {
            super::limits::deny(ctx, message, super::limits::SET).await;
            return true;
        }

        if ALL.contains(&name) {
            let mut changed = 0;
            for lock in plain() {
                match try_set(ctx, chat, lock.key, on).await {
                    Ok(true) => changed += 1,
                    Ok(false) => {}
                    Err(error) => {
                        ::log::warn!("locks: bulk write for {chat}/{} failed: {error}", lock.key);
                        super::respond(ctx, message, crate::response::ResponseKind::CommandError, if error.commit_outcome_unknown() {
                                "نتیجه تنظیم همه قفل ها نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                            } else if changed == 0 {
                                "قفل ها ذخیره نشدند؛ دوباره تلاش کنید."
                            } else {
                                "تنظیم همه قفل ها کامل نشد؛ بعضی قفل ها تغییر کردند. وضعیت را بررسی کنید."
                            }).await;
                        return true;
                    }
                }
            }
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::LockManagement,
                super::premium::icon_text(
                    Some(super::premium::protection(on)),
                    if on {
                        format!(
                            "همه قفل ها فعال شد ({changed} تغییر، {} قفل).",
                            plain().count()
                        )
                    } else {
                        format!("همه قفل ها برداشته شد ({changed} تغییر).")
                    },
                ),
            )
            .await;
            return true;
        }

        if FORWARD.contains(&name) {
            return super::toggles::prompt(ctx, message, chat, &super::toggles::FORWARD, on).await;
        }
        if let Some(seconds) = timed {
            return group_lock_timed(ctx, message, chat, seconds).await;
        }
        if GROUP.contains(&name) {
            return group_lock(ctx, message, chat, on).await;
        }
        if super::pinlock::NAMES.contains(&name) {
            return super::pinlock::set(ctx, message, chat, on).await;
        }
        if LOCKS
            .iter()
            .any(|lock| lock.key == USERNAME && lock.names.contains(&name))
        {
            return super::toggles::prompt(ctx, message, chat, &super::toggles::USERNAME, on).await;
        }
        if BOT.contains(&name) {
            return super::toggles::prompt(ctx, message, chat, &super::toggles::BOT, on).await;
        }

        let Some(lock) = LOCKS.iter().find(|lock| lock.names.contains(&name)) else {
            return false;
        };
        let changed = match try_set(ctx, chat, lock.key, on).await {
            Ok(changed) => changed,
            Err(error) => {
                ::log::warn!("locks: write for {chat}/{} failed: {error}", lock.key);
                super::respond(
                    ctx,
                    message,
                    crate::response::ResponseKind::CommandError,
                    if error.commit_outcome_unknown() {
                        "نتیجه ذخیره قفل نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "قفل ذخیره نشد؛ دوباره تلاش کنید."
                    },
                )
                .await;
                return true;
            }
        };
        let label = lock.names[0];
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::LockManagement,
            super::premium::icon_text(
                Some(super::premium::lock_icon(lock.key, on)),
                match (on, changed) {
                    (true, true) => format!("قفل {label} فعال شد."),
                    (true, false) => format!("قفل {label} از قبل فعال بود."),
                    (false, true) => format!("قفل {label} برداشته شد."),
                    (false, false) => format!("قفل {label} از قبل باز بود."),
                },
            ),
        )
        .await;
        return true;
    }

    if moderate(ctx, message, chat, view).await {
        return true;
    }
    ignored_command(ctx, message, chat, text).await
}

async fn moderate(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
    chat: i64,
    view: &View<'_>,
) -> bool {
    let bio = super::biolink::tripped(ctx, chat, message).await;
    let commented = super::comment::tripped(ctx, chat, message).await;
    let forced = if bio {
        lock(super::biolink::LOCK)
    } else if commented {
        lock(super::comment::LOCK)
    } else {
        None
    };
    let acted = enforce(ctx, message, chat, forced, view).await;
    if matches!(acted, Acted::Yes) && bio {
        super::biolink::punish(ctx, message, chat).await;
    }
    !matches!(acted, Acted::No)
}

pub async fn on_edit(ctx: &std::sync::Arc<Ctx>, message: &Message, view: &View<'_>) {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return;
    };
    let edits_locked = ctx
        .settings
        .is_locked(chat, EDIT)
        .then(|| lock(EDIT))
        .flatten();
    enforce(ctx, message, chat, edits_locked, view).await;
}

pub async fn service(ctx: &Ctx, message: &Message) {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return;
    };
    if message.action().is_none() || !ctx.settings.is_locked(chat, SERVICE) {
        return;
    }
    if let Err(e) = message.delete_critical().await {
        eprintln!("service lock: could not delete in {chat}: {e}");
    }
}

pub fn scan(ctx: &Ctx, chat: i64, view: &View<'_>) -> Option<&'static str> {
    if super::filters::matches(ctx, chat, view) {
        return Some(super::strict::FILTER);
    }
    if super::packs::is_banned(ctx, chat, view) {
        return Some(super::strict::PACK);
    }
    tripped(ctx, chat, view).map(|lock| lock.names[0])
}

fn lock(key: &str) -> Option<&'static Lock> {
    LOCKS.iter().find(|lock| lock.key == key)
}

fn tripped(ctx: &Ctx, chat: i64, view: &View<'_>) -> Option<&'static Lock> {
    const {
        assert!(LOCKS.len() <= u64::BITS as usize);
    }
    let armed = ctx.settings.with_chat(chat, |settings| {
        LOCKS.iter().enumerate().fold(0u64, |bits, (index, lock)| {
            bits | (u64::from(settings.is_locked(lock.key)) << index)
        })
    });
    LOCKS
        .iter()
        .enumerate()
        .find(|(index, lock)| armed & (1u64 << index) != 0 && (lock.matches)(view))
        .map(|(_, lock)| lock)
}

enum Acted {
    No,
    Yes,
    Already,
}

#[derive(Debug, PartialEq, Eq)]
enum DeleteClaim {
    Retryable,
    Owner,
    Duplicate,
}

fn claim_after_delete(deleted: bool, claim: impl FnOnce() -> bool) -> DeleteClaim {
    if !deleted {
        return DeleteClaim::Retryable;
    }
    if claim() {
        DeleteClaim::Owner
    } else {
        DeleteClaim::Duplicate
    }
}

async fn enforce(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
    chat: i64,
    forced: Option<&'static Lock>,
    view: &View<'_>,
) -> Acted {
    let filtered = super::filters::matches(ctx, chat, view);
    let banned_pack = super::packs::is_banned(ctx, chat, view);
    let matched = tripped(ctx, chat, view).or(forced);
    if matched.is_none() && !filtered && !banned_pack {
        return Acted::No;
    }
    if super::is_exempt(ctx, message).await {
        return Acted::No;
    }

    let deleted = match message.delete_critical().await {
        Ok(()) => true,
        Err(e) => {
            eprintln!("could not delete message in {chat}: {e}");
            false
        }
    };
    match claim_after_delete(deleted, || ctx.claim_moderation(chat, message.id())) {
        DeleteClaim::Retryable => return Acted::No,
        DeleteClaim::Duplicate => return Acted::Already,
        DeleteClaim::Owner => {}
    }
    let text = view.message.text().to_owned();
    ctx.bump(chat, super::stats::DELETED);
    let sender_name = super::name_of(message);
    let (cause, reason_for_log, by_filter) = match (matched, filtered) {
        (Some(lock), _) => (lock.key, lock.names[0], false),
        (None, true) => (super::strict::FILTER, "فیلتر کلمه", true),
        (None, false) => (super::strict::PACK, "پک استیکر", false),
    };
    super::log::write(
        ctx,
        chat,
        "log_del",
        super::log::Entry {
            title: "حذف پیام",
            target: message
                .sender_id()
                .and_then(grammers_client::session::types::PeerId::bare_id)
                .map(|id| (id, sender_name.as_str())),
            reason: Some(reason_for_log),
            extra: match text.chars().take(150).collect::<String>() {
                snippet if snippet.trim().is_empty() => Vec::new(),
                snippet => vec![("متن", super::esc(&snippet))],
            },
            ..Default::default()
        },
    )
    .await;
    let chances = match super::strict::punish(ctx, message, chat, cause).await {
        super::strict::Outcome::Announced => {
            let action = super::cases::action_key(super::strict::action_of(ctx, chat));
            super::cases::record_delete(ctx, message, cause, reason_for_log, action).await;
            return Acted::Yes;
        }
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::cases::record_delete(ctx, message, cause, reason_for_log, "delete").await;

    if by_filter {
        super::filters::notify(ctx, message, chat, &text, chances).await;
    } else {
        super::notice::send(ctx, message, chat, reason_for_log, chances).await;
    }
    Acted::Yes
}

async fn group_lock(ctx: &Ctx, message: &Message, chat: i64, on: bool) -> bool {
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };
    if let Err(error) = super::rights::seed(ctx, message, chat).await {
        log::warn!("group lock: could not seed rights for {chat}: {error}");
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            "اختیارات گروه خوانده نشد؛ دوباره تلاش کنید.",
        )
        .await;
        return true;
    }
    let outcome = super::rights::set_manual_lock(ctx, chat_ref, chat, on).await;
    let done = matches!(outcome, Ok(super::rights::DeliveryOutcome::Applied));
    let pending = matches!(
        outcome,
        Ok(super::rights::DeliveryOutcome::PendingRetry { .. }
            | super::rights::DeliveryOutcome::Superseded)
    );
    let delivery_unknown = matches!(
        outcome,
        Ok(super::rights::DeliveryOutcome::AcceptedDeliveryUnknown { .. })
    );
    let unknown = matches!(outcome, Err(ref error) if error.acceptance_unknown());
    if let Err(ref error) = outcome {
        if unknown {
            log::warn!("group lock: mutation outcome unknown for {chat}: {error}");
        } else {
            log::warn!("group lock: mutation not accepted for {chat}: {error}");
        }
    }
    super::respond(
        ctx,
        message,
        crate::response::ResponseKind::LockManagement,
        super::premium::icon_text(
            Some(if done {
                super::premium::protection(on)
            } else if pending || delivery_unknown || unknown {
                super::premium::Icon::Timer
            } else {
                super::premium::Icon::ErrorRed
            }),
            match (done, pending, delivery_unknown, unknown, on) {
                (true, _, _, _, true) => "✓ گروه قفل شد. تنها ادمین ها می توانند پیام بفرستند.",
                (true, _, _, _, false) => "✗ قفل دستی گروه برداشته شد.",
                (_, true, _, _, _) => "تغییر ذخیره شد؛ تحویل آن به تلگرام دوباره تلاش می شود.",
                (_, _, true, _, _) => "تغییر ذخیره شد، اما وضعیت تحویل آن به تلگرام مشخص نیست.",
                (_, _, _, true, _) => "وضعیت ذخیره سازی مشخص نیست؛ پنل را دوباره بررسی کنید.",
                _ => "تغییر ذخیره نشد؛ دوباره تلاش کنید.",
            },
        ),
    )
    .await;
    true
}

fn timed_group(view: &View<'_>, name: &str) -> Option<u64> {
    GROUP.iter().find(|word| {
        name.strip_prefix(*word)
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    })?;

    let (on, normalised) = parse(view.digits())?;
    if !on {
        return None;
    }
    let tail = GROUP.iter().find_map(|word| {
        let rest = normalised.strip_prefix(word)?;
        rest.starts_with(char::is_whitespace)
            .then(|| rest.trim_start())
    })?;
    super::restrict::duration_of(tail)
        .map(|asked| asked.as_secs().clamp(TIMED_RANGE.0, TIMED_RANGE.1))
}

async fn group_lock_timed(ctx: &Ctx, message: &Message, chat: i64, seconds: u64) -> bool {
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };
    if let Err(error) = super::rights::seed(ctx, message, chat).await {
        log::warn!("timed group lock: could not seed rights for {chat}: {error}");
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "اختیارات گروه خوانده نشد؛ دوباره تلاش کنید.",
            ),
        )
        .await;
        return true;
    }
    let until = super::stats::local_seconds() + seconds;
    let outcome = super::rights::set_timed_lock(
        ctx,
        chat_ref,
        chat,
        i64::try_from(until).unwrap_or(i64::MAX),
    )
    .await;
    let applied = matches!(outcome, Ok(super::rights::DeliveryOutcome::Applied));
    let pending = matches!(
        outcome,
        Ok(super::rights::DeliveryOutcome::PendingRetry { .. }
            | super::rights::DeliveryOutcome::Superseded)
    );
    let delivery_unknown = matches!(
        outcome,
        Ok(super::rights::DeliveryOutcome::AcceptedDeliveryUnknown { .. })
    );
    let unknown = matches!(outcome, Err(ref error) if error.acceptance_unknown());
    if let Err(ref error) = outcome {
        if unknown {
            log::warn!("timed group lock: mutation outcome unknown for {chat}: {error}");
        } else {
            log::warn!("timed group lock: mutation not accepted for {chat}: {error}");
        }
    }
    super::respond(
        ctx,
        message,
        crate::response::ResponseKind::LockManagement,
        super::premium::icon_text(
            Some(if applied || pending || delivery_unknown || unknown {
                super::premium::Icon::Timer
            } else {
                super::premium::Icon::ErrorRed
            }),
            if applied {
                format!(
                    "گروه برای {} قفل شد. تنها ادمین ها می توانند پیام بفرستند.",
                    super::log::duration_label(seconds)
                )
            } else if pending {
                "قفل زمان دار ذخیره شد؛ تحویل آن به تلگرام دوباره تلاش می شود.".to_owned()
            } else if delivery_unknown {
                "قفل زمان دار ذخیره شد، اما وضعیت تحویل آن به تلگرام مشخص نیست.".to_owned()
            } else if unknown {
                "وضعیت ذخیره سازی مشخص نیست؛ پنل را دوباره بررسی کنید.".to_owned()
            } else {
                "قفل زمان دار ذخیره نشد؛ دوباره تلاش کنید.".to_owned()
            },
        ),
    )
    .await;
    true
}

fn parse(text: &str) -> Option<(bool, &str)> {
    const ON: &[&str] = &["قفل"];
    const OFF: &[&str] = &["بازکردن", "باز کردن", "آنلاک", "انلاک", "بازکن", "حذف قفل"];

    for (words, on) in [(ON, true), (OFF, false)] {
        for word in words {
            if let Some(rest) = text.strip_prefix(word)
                && rest.starts_with(char::is_whitespace)
            {
                return Some((on, rest.trim()));
            }
        }
    }
    None
}

fn never(_: &View) -> bool {
    false
}

fn is_link(view: &View) -> bool {
    let marked_up = view.entities().is_some_and(|entities| {
        entities
            .iter()
            .any(|e| matches!(e, tl::enums::MessageEntity::Url(_)))
    });

    marked_up
        || text_has_link(view.ascii())
        || (disguised(view) && text_has_bare_domain(view.ascii()))
        || markup_of(view).is_some_and(markup_has_link)
}

fn disguised(view: &View) -> bool {
    let text = view.message.text();
    !matches!(
        unicode_normalization::is_nfkc_quick(text.chars()),
        unicode_normalization::IsNormalized::Yes
    ) || text.chars().any(hides_a_link)
}

fn hides_a_link(c: char) -> bool {
    let hidden = invisible(c)
        && !matches!(
            c,
            '\u{200c}' | '\u{200d}' | '\u{200e}' | '\u{200f}' | '\u{0640}' | '\u{fe00}'
                ..='\u{fe0f}'
        );
    hidden
        || matches!(
            c,
            '\u{1d00}'
                | '\u{0299}'
                | '\u{1d04}'
                | '\u{1d05}'
                | '\u{1d07}'
                | '\u{a730}'
                | '\u{0262}'
                | '\u{029c}'
                | '\u{026a}'
                | '\u{1d0a}'
                | '\u{1d0b}'
                | '\u{029f}'
                | '\u{1d0d}'
                | '\u{0274}'
                | '\u{1d0f}'
                | '\u{1d18}'
                | '\u{a7af}'
                | '\u{0280}'
                | '\u{a731}'
                | '\u{1d1b}'
                | '\u{1d1c}'
                | '\u{1d20}'
                | '\u{1d21}'
                | '\u{028f}'
                | '\u{1d22}'
                | '\u{2215}'
                | '\u{2044}'
                | '\u{29f8}'
                | '\u{3002}'
                | '\u{ff61}'
                | '\u{2027}'
                | '\u{2219}'
                | '\u{22c5}'
        )
}

pub(super) fn text_has_bare_domain(text: &str) -> bool {
    let label = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    text.match_indices('.').any(|(dot, _)| {
        let before = &text[..dot];
        let mut left = before.chars().rev().take_while(|c| label(*c)).count();
        if left == 0 {
            return false;
        }
        if before[..before.len() - left]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '.' || c == '@')
        {
            return false;
        }
        left = left.min(63);
        let after = &text[dot + 1..];
        let tld = after.chars().take_while(|c| c.is_ascii_lowercase()).count();
        (2..=24).contains(&tld)
            && after[tld..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric())
            && left > 0
    })
}

fn is_hyperlink(view: &View) -> bool {
    view.entities().is_some_and(|entities| {
        entities.iter().any(|entity| match entity {
            tl::enums::MessageEntity::TextUrl(text_url) => external_text_url(&text_url.url),
            _ => false,
        })
    }) || markup_of(view).is_some_and(markup_has_hyperlink)
}

fn external_text_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    let Some(target) = lower.strip_prefix("tg://") else {
        return true;
    };
    target.split(['?', '#', '/']).next() != Some("user")
}

fn has_custom_emoji(view: &View) -> bool {
    view.entities().is_some_and(|entities| {
        entities
            .iter()
            .any(|e| matches!(e, tl::enums::MessageEntity::CustomEmoji(_)))
    })
}

fn has_emoji(view: &View) -> bool {
    text_has_emoji(view.message.text()) || has_custom_emoji(view)
}

fn has_english(view: &View) -> bool {
    text_has_english(view.message.text())
}

fn has_persian(view: &View) -> bool {
    text_has_persian(view.message.text())
}

fn text_has_emoji(text: &str) -> bool {
    const TEXT_MARKS: &[u32] = &[0x2713, 0x2714, 0x2717, 0x2718, 0x2605, 0x2606, 0x2022];

    text.chars().any(|c| {
        !TEXT_MARKS.contains(&(c as u32))
            && matches!(c as u32,
                0x1F300..=0x1FAFF
                | 0x1F000..=0x1F2FF
                | 0x2600..=0x27BF
                | 0x2B00..=0x2BFF
                | 0xFE0F
                | 0x2190..=0x21FF
            )
    })
}

fn text_has_english(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_alphabetic())
}

fn text_has_persian(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
    })
}

fn is_spoiler(view: &View) -> bool {
    let entity = view.entities().is_some_and(|entities| {
        entities
            .iter()
            .any(|e| matches!(e, tl::enums::MessageEntity::Spoiler(_)))
    });
    entity
        || matches!(&view.media, Some(Media::Photo(photo)) if photo.is_spoiler())
        || matches!(&view.media, Some(Media::Document(doc)) if doc.is_spoiler())
}

fn raw_media<'a>(view: &View<'a>) -> Option<&'a tl::enums::MessageMedia> {
    match &view.message.raw {
        tl::enums::Message::Message(message) => message.media.as_ref(),
        _ => None,
    }
}

fn is_story(view: &View) -> bool {
    matches!(raw_media(view), Some(tl::enums::MessageMedia::Story(_)))
}

fn is_pin_notice(view: &View) -> bool {
    matches!(
        view.message.action(),
        Some(tl::enums::MessageAction::PinMessage)
    )
}

fn is_hashtag(view: &View) -> bool {
    view.entities().is_some_and(|entities| {
        entities.iter().any(|e| {
            matches!(
                e,
                tl::enums::MessageEntity::Hashtag(_) | tl::enums::MessageEntity::Cashtag(_)
            )
        })
    })
}

fn has_inline_button(view: &View) -> bool {
    matches!(
        markup_of(view),
        Some(tl::enums::ReplyMarkup::ReplyInlineMarkup(_))
    )
}

fn markup_links(markup: &tl::enums::ReplyMarkup) -> Vec<String> {
    use tl::enums::KeyboardButton as B;

    let rows = match markup {
        tl::enums::ReplyMarkup::ReplyInlineMarkup(inline) => &inline.rows,
        tl::enums::ReplyMarkup::ReplyKeyboardMarkup(keyboard) => &keyboard.rows,
        _ => return Vec::new(),
    };
    rows.iter()
        .flat_map(|tl::enums::KeyboardButtonRow::Row(row)| &row.buttons)
        .filter_map(|button| match button {
            B::Url(b) => Some(&b.url),
            B::UrlAuth(b) => Some(&b.url),
            B::InputKeyboardButtonUrlAuth(b) => Some(&b.url),
            B::WebView(b) => Some(&b.url),
            B::SimpleWebView(b) => Some(&b.url),
            B::Copy(b) => Some(&b.copy_text),
            B::SwitchInline(b) => Some(&b.query),
            _ => None,
        })
        .map(|found| folded(found))
        .collect()
}

fn markup_of<'a>(view: &'a View) -> Option<&'a tl::enums::ReplyMarkup> {
    match &view.message.raw {
        tl::enums::Message::Message(message) => message.reply_markup.as_ref(),
        _ => None,
    }
}

fn fwd_of(raw: &tl::enums::Message) -> Option<&tl::types::MessageFwdHeader> {
    let tl::enums::Message::Message(message) = raw else {
        return None;
    };
    let tl::enums::MessageFwdHeader::Header(header) = message.fwd_from.as_ref()?;
    Some(header)
}

fn markup_has_link(markup: &tl::enums::ReplyMarkup) -> bool {
    markup_links(markup).iter().any(|url| text_has_link(url))
}

fn markup_has_hyperlink(markup: &tl::enums::ReplyMarkup) -> bool {
    markup_links(markup)
        .iter()
        .any(|url| external_text_url(url) && (text_has_link(url) || url.starts_with("tg://")))
}

fn markup_has_telegram_link(markup: &tl::enums::ReplyMarkup) -> bool {
    markup_links(markup)
        .iter()
        .any(|url| has_telegram_link(url))
}

pub(super) fn folded(text: &str) -> String {
    use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfkc_quick};
    match is_nfkc_quick(text.chars()) {
        IsNormalized::Yes => text.to_lowercase(),
        _ => text.nfkc().collect::<String>().to_lowercase(),
    }
}

fn invisible(c: char) -> bool {
    matches!(c,
        '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{0640}' | '\u{180e}' | '\u{feff}'
        | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206f}'
        | '\u{fe00}'..='\u{fe0f}'
        | '\u{e0000}'..='\u{e007f}')
}

fn lookalike(c: char) -> Option<char> {
    Some(match c {
        '\u{1d00}' => 'a',
        '\u{0299}' => 'b',
        '\u{1d04}' => 'c',
        '\u{1d05}' => 'd',
        '\u{1d07}' => 'e',
        '\u{a730}' => 'f',
        '\u{0262}' => 'g',
        '\u{029c}' => 'h',
        '\u{026a}' => 'i',
        '\u{1d0a}' => 'j',
        '\u{1d0b}' => 'k',
        '\u{029f}' => 'l',
        '\u{1d0d}' => 'm',
        '\u{0274}' => 'n',
        '\u{1d0f}' => 'o',
        '\u{1d18}' => 'p',
        '\u{a7af}' => 'q',
        '\u{0280}' => 'r',
        '\u{a731}' => 's',
        '\u{1d1b}' => 't',
        '\u{1d1c}' => 'u',
        '\u{1d20}' => 'v',
        '\u{1d21}' => 'w',
        '\u{028f}' => 'y',
        '\u{1d22}' => 'z',
        '\u{2215}' | '\u{2044}' | '\u{29f8}' => '/',
        '\u{3002}' | '\u{ff61}' | '\u{06d4}' | '\u{2027}' | '\u{2219}' | '\u{22c5}' => '.',
        '\u{064a}' | '\u{0649}' => '\u{06cc}',
        '\u{0643}' => '\u{06a9}',
        _ => return None,
    })
}

fn latin_twin(c: char) -> Option<char> {
    Some(match c {
        '\u{0430}' => 'a',
        '\u{0435}' => 'e',
        '\u{043e}' => 'o',
        '\u{0440}' => 'p',
        '\u{0441}' => 'c',
        '\u{0443}' => 'y',
        '\u{0445}' => 'x',
        '\u{0456}' => 'i',
        '\u{0458}' => 'j',
        '\u{0455}' => 's',
        '\u{04bb}' => 'h',
        '\u{0501}' => 'd',
        '\u{03bf}' => 'o',
        '\u{03b1}' => 'a',
        '\u{03c1}' => 'p',
        '\u{03c5}' => 'u',
        '\u{03bd}' => 'v',
        '\u{03c4}' => 't',
        '\u{03ba}' => 'k',
        '\u{03c7}' => 'x',
        '\u{0131}' => 'i',
        '\u{2170}' => 'i',
        _ => return None,
    })
}

fn rewritten(text: &str, map: impl Fn(char) -> Option<char>, drop: bool) -> Option<String> {
    if !text
        .chars()
        .any(|c| map(c).is_some() || (drop && invisible(c)))
    {
        return None;
    }
    Some(
        text.chars()
            .filter(|c| !(drop && invisible(*c)))
            .map(|c| map(c).unwrap_or(c))
            .collect(),
    )
}

pub fn tighten(text: &str) -> std::borrow::Cow<'_, str> {
    match tightened(text) {
        Some(tight) => std::borrow::Cow::Owned(tight),
        None => std::borrow::Cow::Borrowed(text),
    }
}

fn tightened(text: &str) -> Option<String> {
    rewritten(text, lookalike, true)
}

fn latinised(text: &str) -> Option<String> {
    rewritten(text, latin_twin, false)
}

pub(super) fn text_has_link(text: &str) -> bool {
    ["http://", "https://", "t.me/", "telegram.me/", "www."]
        .iter()
        .any(|needle| text.contains(needle))
}

pub fn has_telegram_link(text: &str) -> bool {
    ["t.me", "telegram.me", "telegram.dog"].iter().any(|host| {
        text.match_indices(host).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + host.len()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '@')
                && !after.is_some_and(|c| c.is_alphanumeric() || c == '-')
        })
    })
}

fn is_anonymous_channel(view: &View) -> bool {
    match view.message.sender_id() {
        Some(sender) => {
            sender.kind() == grammers_client::session::types::PeerKind::Channel
                && sender != view.message.peer_id()
        }
        None => false,
    }
}

fn is_username(view: &View) -> bool {
    view.entities().is_some_and(|entities| {
        entities
            .iter()
            .any(|e| matches!(e, tl::enums::MessageEntity::Mention(_)))
    }) || text_has_username(view.ascii())
}

pub(super) fn text_has_username(text: &str) -> bool {
    let bytes = text.as_bytes();
    let handle = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_';
    text.char_indices().any(|(at, c)| {
        if c != '@' {
            return false;
        }
        if text[..at]
            .chars()
            .next_back()
            .is_some_and(|prev| prev.is_alphanumeric() || prev == '_')
        {
            return false;
        }
        let rest = &bytes[at + 1..];
        let run = rest.iter().take_while(|c| handle(**c)).count();
        rest.first().is_some_and(|c| c.is_ascii_lowercase()) && (5..=32).contains(&run)
    })
}

fn command_targets_a_bot(text: &str) -> bool {
    text.split_whitespace()
        .any(|word| word.starts_with('/') && word.contains('@'))
}

fn is_bot_call(view: &View) -> bool {
    command_targets_a_bot(view.ascii())
}

fn is_promoter(view: &View) -> bool {
    is_forward_channel(view)
        || is_username(view)
        || has_telegram_link(view.ascii())
        || markup_of(view).is_some_and(markup_has_telegram_link)
}

fn is_mention(view: &View) -> bool {
    view.entities().is_some_and(|entities| {
        entities
            .iter()
            .any(|e| matches!(e, tl::enums::MessageEntity::MentionName(_)))
    })
}

fn is_forward_channel(view: &View) -> bool {
    fwd_of(&view.message.raw).is_some_and(|header| header.channel_post.is_some())
}

fn is_forward_user(view: &View) -> bool {
    fwd_of(&view.message.raw).is_some_and(|header| header.channel_post.is_none())
}

fn is_media(view: &View) -> bool {
    !matches!(raw_media(view), None | Some(tl::enums::MessageMedia::Empty))
}

pub fn is_photo(view: &View) -> bool {
    matches!(view.media, Some(Media::Photo(_)))
}

pub fn is_sticker(view: &View) -> bool {
    matches!(view.media, Some(Media::Sticker(_)))
}

fn is_animated_sticker(view: &View) -> bool {
    matches!(&view.media, Some(Media::Sticker(s)) if s.is_animated())
}

fn is_contact(view: &View) -> bool {
    matches!(view.media, Some(Media::Contact(_)))
}

fn is_poll(view: &View) -> bool {
    matches!(view.media, Some(Media::Poll(_)))
}

fn is_dice(view: &View) -> bool {
    matches!(view.media, Some(Media::Dice(_)))
}

fn is_location(view: &View) -> bool {
    matches!(
        view.media,
        Some(Media::Geo(_) | Media::GeoLive(_) | Media::Venue(_))
    )
}

pub fn is_gif(view: &View) -> bool {
    match &view.media {
        Some(Media::Document(doc)) => doc.is_animated() || doc.mime_type() == Some("image/gif"),
        _ => false,
    }
}

pub fn is_video(view: &View) -> bool {
    match &view.media {
        Some(Media::Document(doc)) => {
            !doc.is_animated() && doc.mime_type().is_some_and(|m| m.starts_with("video/"))
        }
        _ => false,
    }
}

pub fn is_voice(view: &View) -> bool {
    match &view.media {
        Some(Media::Document(doc)) => is_voice_document(doc),
        _ => false,
    }
}

fn is_voice_document(doc: &grammers_client::media::Document) -> bool {
    let Some(tl::enums::Document::Document(document)) = doc.raw.document.as_ref() else {
        return false;
    };
    document.mime_type.starts_with("audio/ogg")
        && document.attributes.iter().any(|attribute| {
            matches!(
                attribute,
                tl::enums::DocumentAttribute::Audio(audio) if audio.voice
            )
        })
}

pub fn is_music(view: &View) -> bool {
    match &view.media {
        Some(Media::Document(doc)) => {
            doc.mime_type().is_some_and(|m| m.starts_with("audio/")) && !is_voice_document(doc)
        }
        _ => false,
    }
}

pub fn is_file(view: &View) -> bool {
    matches!(view.media.as_ref(), Some(Media::Document(_)))
        && !is_video(view)
        && !is_gif(view)
        && !is_voice(view)
        && !is_music(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_delete_does_not_consume_the_moderation_claim() {
        let claims = std::cell::Cell::new(0);
        let claim = || {
            claims.set(claims.get() + 1);
            true
        };

        assert_eq!(
            claim_after_delete(false, claim),
            DeleteClaim::Retryable,
            "a failed Telegram delete must remain retryable"
        );
        assert_eq!(claims.get(), 0, "failure must not touch the claim store");
        assert_eq!(
            claim_after_delete(true, claim),
            DeleteClaim::Owner,
            "the next successful delivery must still be able to enforce"
        );
        assert_eq!(claims.get(), 1);
    }

    #[test]
    fn concurrent_successes_have_one_moderation_owner() {
        use std::sync::{
            Arc, Barrier,
            atomic::{AtomicBool, Ordering},
        };

        let start = Arc::new(Barrier::new(3));
        let claimed = Arc::new(AtomicBool::new(false));
        let attempts: Vec<_> = (0..2)
            .map(|_| {
                let start = Arc::clone(&start);
                let claimed = Arc::clone(&claimed);
                std::thread::spawn(move || {
                    start.wait();
                    claim_after_delete(true, || {
                        claimed
                            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                    })
                })
            })
            .collect();

        start.wait();
        let decisions: Vec<_> = attempts
            .into_iter()
            .map(|attempt| attempt.join().expect("moderation test task panicked"))
            .collect();
        assert_eq!(
            decisions
                .iter()
                .filter(|decision| **decision == DeleteClaim::Owner)
                .count(),
            1,
            "only one successful delivery may run counters, strikes and notices"
        );
        assert_eq!(
            decisions
                .iter()
                .filter(|decision| **decision == DeleteClaim::Duplicate)
                .count(),
            1,
            "the other successful delivery must stop as a duplicate"
        );
    }

    #[test]
    fn a_username_in_a_font_is_still_a_username() {
        let seen = |text: &str| text_has_username(&folded(text));

        assert!(seen(
            "@\u{1D69C}\u{1D699}\u{1D68A}\u{1D696}\u{1D68B}\u{1D698}\u{1D69D}"
        ));
        assert!(seen(
            "\u{1D42C}\u{1D429}\u{1D41A}\u{1D426}\u{1D41B}\u{1D428}\u{1D42D} @\u{1D42C}\u{1D429}\u{1D41A}\u{1D426}\u{1D41B}\u{1D428}\u{1D42D}"
        ));
        assert!(seen(
            "@\u{FF53}\u{FF50}\u{FF41}\u{FF4D}\u{FF42}\u{FF4F}\u{FF54}"
        ));
        assert!(seen("@spambot"));
        assert!(seen("سلام @spambot ببین"));
        assert!(seen("@Spam_Bot_99"));

        assert!(!seen("name@example.com"));
        assert!(!seen("a@bcdef"));
        assert!(!seen("@abc"));
        assert!(!seen("@"));
        assert!(!seen("@ spambot"));
        assert!(!seen("@1spambot"));
        assert!(!seen("قیمت 100@ تومان"));
    }

    #[test]
    fn the_fold_reaches_fonts_and_spares_persian() {
        assert_eq!(
            folded("\u{1D69D}.\u{1D696}\u{1D68E}/\u{1D69C}\u{1D699}\u{1D68A}\u{1D696}"),
            "t.me/spam"
        );
        assert_eq!(folded("\u{FF54}.\u{FF4D}\u{FF45}"), "t.me");
        assert_eq!(folded("\u{212C}\u{2130}\u{210C}\u{2102}"), "behc");
        assert_eq!(folded("ABC"), "abc");

        assert_eq!(folded("قفل عکس"), "قفل عکس");
        assert_eq!(folded("می\u{200c}شود"), "می\u{200c}شود");
        assert_eq!(folded("۱۲۳"), "۱۲۳");
        assert_eq!(folded("٠١٢"), "٠١٢");

        assert!(text_has_link(&folded(
            "\u{1D69D}.\u{1D696}\u{1D68E}/\u{1D69C}\u{1D699}\u{1D68A}\u{1D696}"
        )));
        assert!(has_telegram_link(&folded(
            "\u{1D69D}.\u{1D696}\u{1D68E}/\u{1D69C}\u{1D699}\u{1D68A}\u{1D696}"
        )));
    }

    #[test]
    fn a_bare_domain_is_a_link_when_it_was_disguised() {
        let seen = |text: &str| text_has_bare_domain(&folded(text));

        assert!(seen("example.com"));
        assert!(seen("spam.ir"));
        assert!(seen("sub.domain.co.uk"));
        assert!(seen(
            "\u{1D68E}\u{1D699}\u{1D68A}\u{1D696}.\u{1D68C}\u{1D698}\u{1D696}"
        ));
        assert!(seen("بیا اینجا example.com زود"));

        assert!(!seen("1.5"));
        assert!(!seen("قیمت 100.000 تومان"));
        assert!(!seen("تمام شد. بعدا"));
        assert!(!seen(".com"));
        assert!(!seen("a."));
        assert!(!seen("name@example.com"));
        assert!(!seen("file.x"));
    }

    #[test]
    fn an_invisible_character_no_longer_hides_a_link() {
        let tight = |t: &str| tighten(&folded(t)).into_owned();
        let link = |t: &str| text_has_link(&tight(t));

        for hidden in [
            "\u{200b}",
            "\u{200c}",
            "\u{200d}",
            "\u{200e}",
            "\u{200f}",
            "\u{00ad}",
            "\u{2060}",
            "\u{feff}",
            "\u{034f}",
            "\u{061c}",
            "\u{180e}",
            "\u{fe0f}",
            "\u{202e}",
            "\u{2066}",
            "\u{e0061}",
        ] {
            assert!(
                link(&format!("t{hidden}.me/spam")),
                "{hidden:?} still hid a link"
            );
            assert!(
                link(&format!("htt{hidden}ps://x.com/a")),
                "{hidden:?} hid a scheme"
            );
        }
        assert_eq!(tight("ف\u{640}ی\u{640}ل\u{640}تر"), "فیلتر");
        assert!(link("t.me/spam"));
        assert!(!link("سلام دنیا"));
    }

    #[test]
    fn an_invisible_character_no_longer_hides_a_username() {
        let seen = |t: &str| {
            text_has_username(
                &latinised(&tighten(&folded(t)))
                    .unwrap_or_else(|| tighten(&folded(t)).into_owned()),
            )
        };
        assert!(seen("@\u{200b}spambot"));
        assert!(seen("@spam\u{200b}bot"));
        assert!(seen("@spam\u{ad}bot"));
        assert!(seen("@\u{1d18}\u{1d0f}\u{1d0f}\u{1d0f}\u{1d0f}\u{1d0f}"));
        assert!(seen("@spambot"));
        assert!(!seen("name@example.com"));
        assert!(!seen("@abc"));
    }

    #[test]
    fn small_capitals_are_ordinary_letters() {
        let tight = |t: &str| tighten(&folded(t)).into_owned();
        assert_eq!(
            tight("\u{1d1b}.\u{1d0d}\u{1d07}/\u{1d04}\u{1d0f}\u{1d0d}"),
            "t.me/com"
        );
        assert!(text_has_link(&tight("\u{1d1b}.\u{1d0d}\u{1d07}/spam")));
        assert!(has_telegram_link(&tight("\u{1d1b}.\u{1d0d}\u{1d07}/spam")));
        assert!(text_has_link(&tight("t.me\u{2215}spam")));
        assert!(has_telegram_link(&tight("t\u{3002}me/spam")));
    }

    #[test]
    fn cyrillic_folds_for_links_and_nowhere_else() {
        let tight = |t: &str| tighten(&folded(t)).into_owned();
        let ascii = |t: &str| latinised(&tight(t)).unwrap_or_else(|| tight(t));

        assert!(has_telegram_link(&ascii("t.m\u{435}/spam")));
        assert!(text_has_username(&ascii("@\u{455}pambot")));
        assert_eq!(tight("привет"), "привет");
        assert_eq!(tight("t.m\u{435}/spam"), "t.m\u{435}/spam");
    }

    #[test]
    fn a_telegram_host_needs_no_trailing_slash() {
        assert!(has_telegram_link("لینک: t.me"));
        assert!(has_telegram_link("t.me?start=join"));
        assert!(has_telegram_link("t.me/spam"));
        assert!(has_telegram_link("telegram.me"));
        assert!(!has_telegram_link("not.me"));
        assert!(!has_telegram_link("x@t.me"));
        assert!(!has_telegram_link("t.mexican"));
        assert!(!has_telegram_link("sub.t.me"));
    }

    #[test]
    fn an_emoji_does_not_make_a_filename_a_link() {
        for ordinary in [
            '\u{200c}', '\u{200d}', '\u{200e}', '\u{200f}', '\u{fe0f}', '\u{0640}',
        ] {
            assert!(!hides_a_link(ordinary), "{ordinary:?} is ordinary text");
        }
        for hidden in [
            '\u{200b}', '\u{00ad}', '\u{2060}', '\u{feff}', '\u{034f}', '\u{202e}',
        ] {
            assert!(hides_a_link(hidden), "{hidden:?} hides something");
        }
        assert!(invisible('\u{fe0f}') && invisible('\u{200c}'));
    }

    #[test]
    fn typing_in_your_own_script_is_not_a_disguise() {
        for ordinary in [
            '\u{064a}', '\u{0643}', '\u{0649}', '\u{06d4}', '\u{0430}', '\u{0435}', '\u{043e}',
            '\u{03bf}', '\u{0131}',
        ] {
            assert!(
                !hides_a_link(ordinary),
                "{ordinary:?} is somebody's alphabet"
            );
            assert!(lookalike(ordinary).is_some() || latin_twin(ordinary).is_some());
        }
        for hidden in ['\u{1d1b}', '\u{1d0d}', '\u{a731}', '\u{2215}', '\u{3002}'] {
            assert!(hides_a_link(hidden), "{hidden:?} belongs to no alphabet");
        }
    }

    #[test]
    fn ordinary_text_is_not_copied() {
        for plain in [
            "سلام دنیا",
            "hello there",
            "t.me/spam",
            "قیمت 1000 تومان",
            "می\u{200c}شود",
        ] {
            let lower = folded(plain);
            if plain == "می\u{200c}شود" {
                assert!(tightened(&lower).is_some());
                continue;
            }
            assert!(tightened(&lower).is_none(), "{plain:?} copied for nothing");
            assert!(latinised(&lower).is_none(), "{plain:?} copied for nothing");
        }
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse("قفل لینک"), Some((true, "لینک")));
        assert_eq!(parse("بازکردن استیکر متحرک"), Some((false, "استیکر متحرک")));
        assert_eq!(parse("باز کردن همه"), Some((false, "همه")));
        assert_eq!(parse("قفل"), None);
        assert_eq!(parse("قفلی"), None);
        assert_eq!(parse("سلام"), None);
    }

    #[test]
    fn spots_a_username_inside_a_bot_command() {
        assert!(command_targets_a_bot("/start@RextesterRoBot"));
        assert!(command_targets_a_bot("سلام /help@somebot لطفا"));

        assert!(!command_targets_a_bot("/start"));
        assert!(!command_targets_a_bot("ایمیل من a@b.com است"));
        assert!(!command_targets_a_bot("@channel"));
    }

    #[test]
    fn spots_a_telegram_link() {
        assert!(has_telegram_link("بیا t.me/joinchat/abc"));
        assert!(has_telegram_link("https://telegram.me/somechannel"));
        assert!(!has_telegram_link("example.com/t/me"));
        assert!(!has_telegram_link("سلام"));
    }

    #[test]
    fn the_catch_all_covers_what_grammers_has_no_wrapper_for() {
        let dropped = [
            tl::enums::MessageMedia::PaidMedia(tl::types::MessageMediaPaidMedia {
                stars_amount: 0,
                extended_media: Vec::new(),
            }),
            tl::enums::MessageMedia::Giveaway(tl::types::MessageMediaGiveaway {
                only_new_subscribers: false,
                winners_are_visible: false,
                channels: Vec::new(),
                countries_iso2: None,
                prize_description: None,
                quantity: 0,
                months: None,
                stars: None,
                until_date: 0,
            }),
            tl::enums::MessageMedia::Unsupported,
        ];
        for media in dropped {
            assert!(
                Media::from_raw(media.clone()).is_none(),
                "grammers grew a wrapper for {media:?} — this test's premise is stale"
            );
            assert!(
                !matches!(Some(&media), None | Some(tl::enums::MessageMedia::Empty)),
                "{media:?} must read as media"
            );
        }

        assert!(Media::from_raw(tl::enums::MessageMedia::Empty).is_none());
    }

    #[test]
    fn the_commands_lock_covers_what_a_member_can_actually_send() {
        for word in [
            "پینگ",
            "گزارش",
            "راهنما",
            "دستورات",
            "قوانین",
            "لیست ادمین",
            "قفل ها",
            "نرخ ارز",
            "قیمت ارز",
        ] {
            assert!(
                is_public_command(word),
                "«{word}» is public but the lock ignores it"
            );
        }

        for word in ["قفل عکس", "سکوت", "بن", "پنل", "ترفیع", "کانفیگ", "توقف"]
        {
            assert!(!is_public_command(word), "«{word}» is admin-gated already");
        }

        assert!(LOCKS.iter().any(|lock| lock.key == COMMANDS));
    }

    #[test]
    fn the_commands_lock_never_deletes() {
        let row = LOCKS
            .iter()
            .find(|lock| lock.key == COMMANDS)
            .expect("the commands lock must be in the table");
        assert!(
            std::ptr::fn_addr_eq(row.matches, never as fn(&View) -> bool),
            "«دستورات عمومی» must not match, or enforce will delete instead of ignoring"
        );
    }

    #[test]
    fn a_command_word_does_not_smuggle_anything() {
        assert!(is_public_command("پینگ"));
        for text in ["پینگ https://spam.example", "پینگ بیا", "قفل ها و بقیه"]
        {
            assert!(
                !is_public_command(text),
                "«{text}» carries more than the command and must face the locks"
            );
        }
    }

    #[test]
    fn every_lock_name_is_unique() {
        let mut names: Vec<&str> = LOCKS.iter().flat_map(|l| l.names.iter().copied()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate lock name");
    }

    #[test]
    fn detects_scripts_and_emoji() {
        assert!(text_has_emoji("سلام 😀"));
        assert!(text_has_emoji("hello ✅"));
        assert!(!text_has_emoji("سلام دوستان"));
        assert!(!text_has_emoji("hello world 123"));

        assert!(!text_has_emoji("✓ ✗ ★ ‹"));

        assert!(text_has_english("سلام hello"));
        assert!(!text_has_english("سلام ۱۲۳"));

        assert!(text_has_persian("سلام"));
        assert!(!text_has_persian("hello world"));
        assert!(!text_has_persian("123 456"));
    }

    #[test]
    fn detects_links() {
        assert!(text_has_link("سلام https://example.com"));
        assert!(text_has_link(&"join T.ME/somegroup".to_lowercase()));
        assert!(!text_has_link("سلام دوستان"));
    }

    #[test]
    fn does_not_call_internal_user_mentions_hyperlinks() {
        assert!(!external_text_url("tg://user?id=123"));
        assert!(!external_text_url(" TG://USER?id=123 "));
        assert!(!external_text_url("tg://user/path"));
        assert!(external_text_url("https://example.com"));
        assert!(external_text_url("tg://resolve?domain=somechannel"));
        assert!(external_text_url("tg://usernames?username=alice"));
        assert!(external_text_url("tg://userland"));
    }

    fn keyboard(buttons: Vec<tl::enums::KeyboardButton>) -> tl::enums::ReplyMarkup {
        tl::types::ReplyInlineMarkup {
            rows: vec![tl::types::KeyboardButtonRow { buttons }.into()],
        }
        .into()
    }

    fn url_button(url: &str) -> tl::enums::KeyboardButton {
        tl::types::KeyboardButtonUrl {
            style: None,
            text: "بزن".to_owned(),
            url: url.to_owned(),
        }
        .into()
    }

    #[test]
    fn finds_a_link_hiding_in_a_button() {
        assert!(markup_has_link(&keyboard(vec![url_button(
            "https://example.com"
        )])));
        assert!(markup_has_telegram_link(&keyboard(vec![url_button(
            "https://t.me/somechannel"
        )])));

        assert!(markup_has_link(&keyboard(vec![url_button(
            "HTTPS://Example.COM"
        )])));

        assert!(!markup_has_link(&keyboard(vec![url_button(
            "tg://user?id=1"
        )])));
        assert!(!markup_has_link(&keyboard(Vec::new())));
    }

    #[test]
    fn hyperlink_lock_reads_external_button_targets_without_user_mentions() {
        assert!(markup_has_hyperlink(&keyboard(vec![url_button(
            "https://example.com"
        )])));
        assert!(markup_has_hyperlink(&keyboard(vec![url_button(
            "tg://resolve?domain=somechannel"
        )])));
        assert!(!markup_has_hyperlink(&keyboard(vec![url_button(
            "tg://user?id=1"
        )])));
        assert!(!markup_has_hyperlink(&keyboard(vec![url_button(
            "press this"
        )])));
    }

    #[test]
    fn every_url_bearing_button_is_read() {
        let link = "https://t.me/spam".to_owned();
        let buttons: Vec<tl::enums::KeyboardButton> = vec![
            url_button(&link),
            tl::types::KeyboardButtonUrlAuth {
                style: None,
                text: String::new(),
                fwd_text: None,
                url: link.clone(),
                button_id: 0,
            }
            .into(),
            tl::types::KeyboardButtonWebView {
                style: None,
                text: String::new(),
                url: link.clone(),
            }
            .into(),
            tl::types::KeyboardButtonSimpleWebView {
                style: None,
                text: String::new(),
                url: link.clone(),
            }
            .into(),
            tl::types::KeyboardButtonCopy {
                style: None,
                text: String::new(),
                copy_text: link.clone(),
            }
            .into(),
            tl::types::KeyboardButtonSwitchInline {
                same_peer: false,
                style: None,
                text: String::new(),
                query: link.clone(),
                peer_types: None,
            }
            .into(),
        ];
        for button in buttons {
            assert!(
                markup_has_telegram_link(&keyboard(vec![button.clone()])),
                "a link in {button:?} was not seen"
            );
        }

        assert!(!markup_has_link(&keyboard(vec![
            tl::types::KeyboardButtonCallback {
                requires_password: false,
                style: None,
                text: "https://example.com".to_owned(),
                data: b"noop".to_vec(),
            }
            .into()
        ])));
    }
}
