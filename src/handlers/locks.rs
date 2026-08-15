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
}

impl<'a> View<'a> {
    pub fn new(message: &'a Message) -> Self {
        Self {
            message,
            media: message.media(),
            text: message.text().trim(),
            lower: std::sync::OnceLock::new(),
            digits: std::sync::OnceLock::new(),
        }
    }

    pub fn lower(&self) -> &str {
        self.lower
            .get_or_init(|| self.message.text().to_lowercase())
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

pub const LOCKS: &[Lock] = &[
    Lock { key: "links", names: &["لینک", "لینک ها", "لینکها"], matches: is_link },
    Lock { key: "photo", names: &["عکس", "تصویر"], matches: is_photo },
    Lock { key: "video", names: &["ویدیو", "فیلم", "ویدئو"], matches: is_video },
    Lock { key: "gif", names: &["گیف"], matches: is_gif },
    Lock { key: "sticker", names: &["استیکر"], matches: is_sticker },
    Lock { key: "animsticker", names: &["استیکر متحرک", "استیکرمتحرک"], matches: is_animated_sticker },
    Lock { key: "music", names: &["موزیک", "آهنگ", "اهنگ"], matches: is_music },
    Lock { key: "voice", names: &["ویس", "صدا"], matches: is_voice },
    Lock { key: "file", names: &["فایل", "سند"], matches: is_file },
    Lock { key: "contact", names: &["مخاطب", "کانتکت"], matches: is_contact },
    Lock { key: "location", names: &["مکان", "لوکیشن"], matches: is_location },
    Lock { key: "poll", names: &["نظرسنجی"], matches: is_poll },
    Lock { key: "dice", names: &["تاس", "بازی"], matches: is_dice },
    Lock { key: "forward_channel", names: &["فوروارد از کانال", "فوروارد کانال"], matches: is_forward_channel },
    Lock { key: "forward_user", names: &["فوروارد از کاربر", "فوروارد کاربر"], matches: is_forward_user },
    Lock { key: "hyperlink", names: &["لینک مخفی", "هایپرلینک", "لینک متنی"], matches: is_hyperlink },
    Lock { key: "hashtag", names: &["هشتگ"], matches: is_hashtag },
    Lock { key: "emoji", names: &["ایموجی", "شکلک"], matches: has_emoji },
    Lock { key: "premoji", names: &["ایموجی پرمیوم", "ایموجی ویژه"], matches: has_custom_emoji },
    Lock { key: "english", names: &["انگلیسی", "لاتین"], matches: has_english },
    Lock { key: "persian", names: &["فارسی", "پارسی"], matches: has_persian },
    Lock { key: "button", names: &["دکمه", "دکمه شیشه ای", "اینلاین"], matches: has_inline_button },
    Lock { key: USERNAME, names: &["یوزرنیم", "یوزر", "آیدی"], matches: is_username },
    Lock { key: MENTION, names: &["تگ", "منشن"], matches: is_mention },
    Lock { key: "media", names: &["مدیا", "رسانه"], matches: is_media },
    Lock { key: "anon", names: &["ناشناس", "هویت ناشناس", "کانال"], matches: is_anonymous_channel },
    Lock { key: "spoiler", names: &["اسپویلر", "اسپویل"], matches: is_spoiler },
    Lock { key: "story", names: &["استوری"], matches: is_story },
    Lock { key: "pin", names: &["اعلان سنجاق", "اعلان پین"], matches: is_pin_notice },
    Lock { key: "promoter", names: &["تبچی", "تبلیغ", "تبلیغات"], matches: is_promoter },
    Lock { key: "commands", names: &["دستورات عمومی", "دستورات", "کامند"], matches: is_bot_command },
    Lock { key: BOTCALL, names: &["دستور ربات", "دستور بات", "کامند ربات"], matches: is_bot_call },

    Lock { key: EDIT, names: &["ویرایش", "ادیت"], matches: never },

    Lock { key: SERVICE, names: &["سرویس", "سرویس تلگرام", "پیام سرویس"], matches: never },

    Lock { key: super::bots::LOCK, names: &["ربات", "بات"], matches: never },
];

const ALL: &[&str] = &["همه", "همه چیز", "کل"];

const FORWARD: &[&str] = &["فوروارد", "فروارد", "هدایت"];

const BOT: &[&str] = &["ربات", "بات"];

const GROUP: &[&str] = &["گروه", "کل گروه"];

pub const EDIT: &str = "edit";

pub const SERVICE: &str = "service";
pub const USERNAME: &str = "username";
pub const MENTION: &str = "mention";
pub const BOTCALL: &str = "botcall";
pub const FORWARD_CHANNEL: &str = "forward_channel";
pub const FORWARD_USER: &str = "forward_user";
const STATUS: &[&str] = &["قفل ها", "قفلها", "لیست قفل", "وضعیت قفل"];

pub async fn handle(ctx: &std::sync::Arc<Ctx>, message: &Message, view: &View<'_>) -> bool {
    let text = view.text();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if STATUS.contains(&text) {
        let active: Vec<&str> = ctx.settings.with_chat(chat, |settings| {
            LOCKS
                .iter()
                .filter(|lock| settings.is_locked(lock.key))
                .map(|lock| lock.names[0])
                .collect()
        });
        let _ = message
            .reply(if active.is_empty() {
                format!("هیچ قفلی فعال نیست. ({} قفل در دسترس)", LOCKS.len())
            } else {
                format!(
                    "قفل های فعال ({} از {}):\n{}",
                    active.len(),
                    LOCKS.len(),
                    active.join("، ")
                )
            })
            .await;
        return true;
    }

    if let Some((on, name)) = parse(text) {
        if !can_manage(ctx, message).await {
            return enforce(ctx, message, chat, false, view).await;
        }

        if !super::limits::allowed(ctx, message, super::limits::SET) {
            super::limits::deny(message, super::limits::SET).await;
            return true;
        }

        if ALL.contains(&name) {
            let mut changed = 0;
            for lock in LOCKS {
                if ctx.settings.set(chat, lock.key, on).await {
                    changed += 1;
                }
                super::strict::sync_pick(ctx, chat, lock.key, on).await;
                super::bots::on_lock_set(ctx, chat, lock.key, on).await;
            }
            let _ = message
                .reply(if on {
                    format!("✓ همه قفل ها فعال شد ({changed} تغییر، {} قفل).", LOCKS.len())
                } else {
                    format!("✗ همه قفل ها برداشته شد ({changed} تغییر).")
                })
                .await;
            return true;
        }

        if FORWARD.contains(&name) {
            return super::toggles::prompt(ctx, message, chat, &super::toggles::FORWARD, on).await;
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
        let changed = ctx.settings.set(chat, lock.key, on).await;
        super::strict::sync_pick(ctx, chat, lock.key, on).await;
        super::bots::on_lock_set(ctx, chat, lock.key, on).await;
        let label = lock.names[0];
        let _ = message
            .reply(match (on, changed) {
                (true, true) => format!("✓ قفل {label} فعال شد."),
                (true, false) => format!("✓ قفل {label} از قبل فعال بود."),
                (false, true) => format!("✗ قفل {label} برداشته شد."),
                (false, false) => format!("✗ قفل {label} از قبل باز بود."),
            })
            .await;
        return true;
    }

    enforce(ctx, message, chat, false, view).await
}

pub async fn on_edit(ctx: &std::sync::Arc<Ctx>, message: &Message) {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return;
    };
    let edits_locked = ctx.settings.is_locked(chat, EDIT);
    let view = View::new(message);
    enforce(ctx, message, chat, edits_locked, &view).await;
}

pub async fn service(ctx: &Ctx, message: &Message) {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return;
    };
    if message.action().is_none() || !ctx.settings.is_locked(chat, SERVICE) {
        return;
    }
    if let Err(e) = message.delete().await {
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

fn tripped(ctx: &Ctx, chat: i64, view: &View<'_>) -> Option<&'static Lock> {
    let armed: Vec<&'static Lock> = ctx.settings.with_chat(chat, |settings| {
        LOCKS
            .iter()
            .filter(|lock| settings.is_locked(lock.key))
            .collect()
    });
    armed.into_iter().find(|lock| (lock.matches)(view))
}

async fn enforce(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
    chat: i64,
    forced: bool,
    view: &View<'_>,
) -> bool {
    let filtered = super::filters::matches(ctx, chat, view);
    let banned_pack = super::packs::is_banned(ctx, chat, view);
    let matched = tripped(ctx, chat, view);
    if !forced && !filtered && !banned_pack && matched.is_none() {
        return false;
    }
    if super::is_exempt(ctx, message).await {
        return false;
    }

    let text = view.message.text().to_owned();

    if let Err(e) = message.delete().await {
        eprintln!("could not delete message in {chat}: {e}");
        return false;
    }
    ctx.bump(chat, super::stats::DELETED);
    let sender_name = super::name_of(message);
    let reason_for_log = match (matched, filtered, banned_pack) {
        (Some(lock), ..) => lock.names[0],
        (None, true, _) => "فیلتر کلمه",
        (None, false, true) => "پک استیکر",
        (None, false, false) => "ویرایش",
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
    let cause = match (matched, filtered, banned_pack) {
        (Some(lock), ..) => lock.key,
        (None, true, _) => super::strict::FILTER,
        (None, false, true) => super::strict::PACK,
        (None, false, false) => EDIT,
    };
    let chances = super::strict::punish(ctx, message, chat, cause).await;

    if filtered {
        super::filters::notify(ctx, message, chat, &text, chances).await;
    } else {
        let reason = match (matched, banned_pack) {
            (Some(lock), _) => lock.names[0],
            (None, true) => "پک استیکر",
            (None, false) => "ویرایش",
        };
        super::notice::send(ctx, message, chat, reason, chances).await;
    }
    true
}

pub async fn set_group_lock(
    ctx: &Ctx,
    chat_ref: grammers_client::session::types::PeerRef,
    on: bool,
) -> bool {
    let Some(chat) = chat_ref.id.bot_api_dialog_id() else {
        return false;
    };

    super::rights::apply(ctx, chat_ref, chat, on).await
}

async fn group_lock(ctx: &Ctx, message: &Message, _chat: i64, on: bool) -> bool {
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };
    let done = set_group_lock(ctx, chat_ref, on).await;
    let _ = message
        .reply(match (done, on) {
            (true, true) => "✓ گروه قفل شد. تنها ادمین ها می توانند پیام بفرستند.",
            (true, false) => "✗ قفل گروه برداشته شد.",
            (false, _) => "انجام نشد. مطمئن شوید ربات اجازه تغییر اطلاعات گروه دارد.",
        })
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
        || text_has_link(view.lower())
        || markup_of(view).is_some_and(markup_has_link)
}

fn is_hyperlink(view: &View) -> bool {
    view.entities().is_some_and(|entities| {
        entities
            .iter()
            .any(|e| matches!(e, tl::enums::MessageEntity::TextUrl(_)))
    })
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

fn is_story(view: &View) -> bool {
    let media = match &view.message.raw {
        tl::enums::Message::Message(message) => message.media.as_ref(),
        _ => None,
    };
    matches!(media, Some(tl::enums::MessageMedia::Story(_)))
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
        .map(|found| found.to_lowercase())
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

fn markup_has_telegram_link(markup: &tl::enums::ReplyMarkup) -> bool {
    markup_links(markup)
        .iter()
        .any(|url| text_has_telegram_link(url))
}

fn text_has_link(text: &str) -> bool {
    ["http://", "https://", "t.me/", "telegram.me/", "www."]
        .iter()
        .any(|needle| text.contains(needle))
}

fn text_has_telegram_link(text: &str) -> bool {
    ["t.me/", "telegram.me/", "telegram.dog/"]
        .iter()
        .any(|needle| text.contains(needle))
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
    })
}

fn command_targets_a_bot(text: &str) -> bool {
    text.split_whitespace()
        .any(|word| word.starts_with('/') && word.contains('@'))
}

fn is_bot_call(view: &View) -> bool {
    command_targets_a_bot(view.message.text())
}

fn is_promoter(view: &View) -> bool {
    is_forward_channel(view)
        || is_username(view)
        || text_has_telegram_link(view.lower())
        || markup_of(view).is_some_and(markup_has_telegram_link)
}

fn is_bot_command(view: &View) -> bool {
    view.entities().is_some_and(|entities| {
        entities
            .iter()
            .any(|e| matches!(e, tl::enums::MessageEntity::BotCommand(_)))
    }) || view.message.text().starts_with('/')
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
    view.media.is_some()
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
        Some(Media::Document(doc)) => {
            doc.is_animated() || doc.mime_type() == Some("image/gif")
        }
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

fn is_voice(view: &View) -> bool {
    match &view.media {
        Some(Media::Document(doc)) => doc.mime_type().is_some_and(|m| m.starts_with("audio/ogg")),
        _ => false,
    }
}

pub fn is_music(view: &View) -> bool {
    match &view.media {
        Some(Media::Document(doc)) => doc
            .mime_type()
            .is_some_and(|m| m.starts_with("audio/") && !m.starts_with("audio/ogg")),
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
        assert!(text_has_telegram_link("بیا t.me/joinchat/abc"));
        assert!(text_has_telegram_link("https://telegram.me/somechannel"));
        assert!(!text_has_telegram_link("example.com/t/me"));
        assert!(!text_has_telegram_link("سلام"));
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
