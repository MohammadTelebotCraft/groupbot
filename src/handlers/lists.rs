use grammers_client::message::{Button, Message, ReplyMarkup};
use grammers_client::session::types::PeerRef;
use grammers_client::tl;

use crate::response::ResponseKind;

use super::restrict::{self, Action};
use super::{Ctx, esc, filters, imgfilter, join, packs, vip};

#[derive(Debug)]
pub enum MutationError {
    Settings(crate::state::SettingsWriteError),
    Database(sqlx::Error),
    Image(imgfilter::DeleteError),
    Restriction(restrict::Failed),
}

impl MutationError {
    pub fn commit_outcome_unknown(&self) -> bool {
        match self {
            Self::Settings(error) => error.commit_outcome_unknown(),
            Self::Image(imgfilter::DeleteError::Setting(error)) => error.commit_outcome_unknown(),
            Self::Database(_) | Self::Image(_) | Self::Restriction(_) => false,
        }
    }
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Settings(error) => error.fmt(formatter),
            Self::Database(error) => error.fmt(formatter),
            Self::Image(error) => error.fmt(formatter),
            Self::Restriction(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for MutationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Settings(error) => Some(error),
            Self::Database(error) => Some(error),
            Self::Image(error) => Some(error),
            Self::Restriction(_) => None,
        }
    }
}

impl From<crate::state::SettingsWriteError> for MutationError {
    fn from(error: crate::state::SettingsWriteError) -> Self {
        Self::Settings(error)
    }
}

impl From<imgfilter::DeleteError> for MutationError {
    fn from(error: imgfilter::DeleteError) -> Self {
        Self::Image(error)
    }
}

impl From<sqlx::Error> for MutationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

pub const SHOW: &[(&str, Kind)] = &[
    ("لیست بن", Kind::Ban),
    ("لیست سیک", Kind::Ban),
    ("لیست سکوت", Kind::Mute),
    ("لیست خفه", Kind::Mute),
    ("لیست ویژه", Kind::Vip),
    ("لیست فیلتر", Kind::Filter),
    ("لیست فیلتر تصویری", Kind::Image),
    ("لیست پاسخ", Kind::Answer),
    ("لیست معاف", Kind::Exempt),
    ("لیست دستور", Kind::Command),
    ("لیست پک", Kind::Pack),
    ("لیست استیکرپک", Kind::Pack),
];

pub const CLEAR: &[(&str, Kind)] = &[
    ("پاکسازی لیست بن", Kind::Ban),
    ("پاکسازی لیست سیک", Kind::Ban),
    ("پاکسازی لیست سکوت", Kind::Mute),
    ("پاکسازی لیست خفه", Kind::Mute),
    ("پاکسازی لیست ویژه", Kind::Vip),
    ("پاکسازی لیست فیلتر", Kind::Filter),
    ("پاکسازی لیست فیلتر تصویری", Kind::Image),
    ("پاکسازی لیست پاسخ", Kind::Answer),
    ("پاکسازی لیست معاف", Kind::Exempt),
    ("پاکسازی لیست دستور", Kind::Command),
    ("پاکسازی لیست پک", Kind::Pack),
    ("پاکسازی لیست استیکرپک", Kind::Pack),
];

pub async fn command(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let show = SHOW.iter().find(|(name, _)| *name == text);
    let clear = CLEAR.iter().find(|(name, _)| *name == text);
    let Some(&(_, kind)) = show.or(clear) else {
        return false;
    };
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !super::limits::allows(ctx, message, kind.cap()).await {
        return true;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    if clear.is_some() {
        match clear_all(ctx, chat_ref, chat, kind).await {
            Ok(removed) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::AdminTool,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Delete),
                        format!("{removed} مورد از {} حذف شد.", kind.title()),
                    ),
                )
                .await
            }
            Err(error) => {
                ::log::warn!("lists: clear {} for {chat} failed: {error}", kind.title());
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    "پاکسازی کامل نشد؛ دوباره تلاش کنید.",
                )
                .await
            }
        }
        return true;
    }

    let Some(opener) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };
    let (title, markup) = match view(ctx, chat_ref, chat, kind, opener).await {
        Ok(page) => page,
        Err(error) => {
            ::log::warn!("lists: could not read {} for {chat}: {error}", kind.title());
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "لیست فعلاً در دسترس نیست؛ دوباره تلاش کنید.",
            )
            .await;
            return true;
        }
    };
    super::respond_shared(
        ctx,
        message,
        ResponseKind::AdminTool,
        super::premium::html(title).reply_markup(markup),
    )
    .await;
    true
}

pub async fn clear_all(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
) -> Result<usize, MutationError> {
    let mut removed = 0;
    loop {
        let entries = entries(ctx, chat_ref, chat, kind, MEMBER_LIST_PAGE).await?;
        if entries.is_empty() {
            break;
        }
        let mut page_removed = 0;
        for entry in entries {
            if matches!(
                kind,
                Kind::Filter
                    | Kind::Image
                    | Kind::Vip
                    | Kind::Answer
                    | Kind::Exempt
                    | Kind::Command
                    | Kind::Pack
            ) {
                if remove(ctx, chat_ref, chat, kind, &entry.key).await? {
                    page_removed += 1;
                }
                continue;
            }
            let Some(peer) = entry.peer else {
                eprintln!("lists: {chat}: no ref for user {}", entry.key);
                continue;
            };
            restrict::apply_maintenance(
                ctx,
                chat_ref,
                peer,
                Action::Unban,
                None,
                restrict::By {
                    reason: "پنل لیست ها",
                    ..Default::default()
                },
            )
            .await
            .map_err(MutationError::Restriction)?;
            page_removed += 1;
        }
        removed += page_removed;
        if page_removed == 0 || !matches!(kind, Kind::Ban | Kind::Mute) {
            break;
        }
    }
    Ok(removed)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Ban,
    Mute,
    Vip,
    Filter,
    Image,
    Answer,
    Exempt,
    Command,
    Pack,
}

const LIMIT: usize = 20;

const MEMBER_LIST_PAGE: usize = 20_000;

impl Kind {
    pub(super) const fn throttle_id(self) -> i64 {
        match self {
            Self::Ban => 0,
            Self::Mute => 1,
            Self::Vip => 2,
            Self::Filter => 3,
            Self::Image => 4,
            Self::Answer => 5,
            Self::Exempt => 6,
            Self::Command => 7,
            Self::Pack => 8,
        }
    }

    pub fn from_action(action: &str) -> Option<Self> {
        match action {
            "ban" => Some(Self::Ban),
            "mute" => Some(Self::Mute),
            "vip" => Some(Self::Vip),
            "filter" => Some(Self::Filter),
            "imgf" => Some(Self::Image),
            "answer" => Some(Self::Answer),
            "free" => Some(Self::Exempt),
            "cmd" => Some(Self::Command),
            "pack" => Some(Self::Pack),
            _ => None,
        }
    }

    pub fn cap(self) -> &'static super::limits::Cap {
        match self {
            Self::Ban => super::limits::BAN,
            Self::Mute => super::limits::MUTE,
            Self::Vip => super::limits::VIP,
            Self::Exempt => super::limits::EXEMPT,
            Self::Filter | Self::Image | Self::Answer | Self::Command | Self::Pack => {
                super::limits::SET
            }
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Ban => "ban",
            Self::Mute => "mute",
            Self::Vip => "vip",
            Self::Filter => "filter",
            Self::Image => "imgf",
            Self::Answer => "answer",
            Self::Exempt => "free",
            Self::Command => "cmd",
            Self::Pack => "pack",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Ban => "بن شده ها",
            Self::Mute => "سکوت شده ها",
            Self::Vip => "کاربران ویژه",
            Self::Filter => "لیست فیلتر",
            Self::Image => "فیلتر تصویری",
            Self::Answer => "پاسخ خودکار",
            Self::Exempt => "معاف ها",
            Self::Command => "دستور های سفارشی",
            Self::Pack => "پک های استیکر",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Ban => "برای رفع بن روی هر نام بزنید.",
            Self::Mute => "برای رفع سکوت روی هر نام بزنید.",
            Self::Vip => "برای حذف از لیست ویژه روی هر مورد بزنید.",
            Self::Filter => "برای حذف کلمه روی آن بزنید.",
            Self::Image => {
                "علامت ~ یعنی فقط بررسی می کند و پاک نمی کند. برای حذف روی هر مورد بزنید."
            }
            Self::Answer => "برای حذف یک پاسخ روی آن بزنید.",
            Self::Exempt => "معاف از عضویت اجباری و اد اجباری. برای حذف روی هر مورد بزنید.",
            Self::Command => "برای حذف یک دستور سفارشی روی آن بزنید.",
            Self::Pack => "برای بازکردن قفل روی هر پک بزنید.",
        }
    }

    fn filter(self) -> tl::enums::ChannelParticipantsFilter {
        let q = String::new();
        match self {
            Self::Mute => tl::enums::ChannelParticipantsFilter::ChannelParticipantsBanned(
                tl::types::ChannelParticipantsBanned { q },
            ),
            _ => tl::enums::ChannelParticipantsFilter::ChannelParticipantsKicked(
                tl::types::ChannelParticipantsKicked { q },
            ),
        }
    }
}

pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) peer: Option<PeerRef>,
}

pub fn word_id(word: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in word.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:x}")
}

pub const CLEAR_KEY: &str = "clear";
const CLEAR_CONFIRMED: &str = "clearyes";

pub fn clearing(entry_key: &str) -> Option<bool> {
    match entry_key {
        CLEAR_KEY => Some(false),
        CLEAR_CONFIRMED => Some(true),
        _ => None,
    }
}

pub async fn confirm_clear(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    opener: i64,
) -> Result<(String, ReplyMarkup), sqlx::Error> {
    let count = entries(ctx, chat_ref, chat, kind, MEMBER_LIST_PAGE)
        .await?
        .len();
    let count_label = if count >= MEMBER_LIST_PAGE {
        format!("{count}+")
    } else {
        count.to_string()
    };
    let title = format!(
        "<b>پنل مدیریت</b> › <b>{}</b>\n\n\
         همه <b>{count_label}</b> مورد از این لیست حذف می شود. این کار برگشت ندارد.",
        kind.title()
    );
    let markup = super::premium::buttons(&[vec![
        super::premium::decorate(
            super::style::data(
                "تایید",
                format!("p:{opener}:{chat}:l:{}:{CLEAR_CONFIRMED}", kind.action()).into_bytes(),
                super::style::Colour::Success,
            ),
            Some(super::premium::Icon::Success),
        ),
        super::premium::decorate(
            Button::data(
                "لغو",
                format!("p:{opener}:{chat}:l:{}", kind.action()).into_bytes(),
            ),
            Some(super::premium::Icon::Close),
        ),
    ]]);
    Ok((title, markup))
}

pub async fn view(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    opener: i64,
) -> Result<(String, ReplyMarkup), sqlx::Error> {
    let entries = entries(ctx, chat_ref, chat, kind, MEMBER_LIST_PAGE).await?;

    let mut rows: Vec<Vec<Button>> = entries
        .iter()
        .take(LIMIT)
        .map(|entry| {
            vec![Button::data(
                format!("✗  {}", entry.name),
                format!("p:{opener}:{chat}:l:{}:{}", kind.action(), entry.key).into_bytes(),
            )]
        })
        .collect();
    if !entries.is_empty() {
        rows.push(vec![super::style::data(
            format!("پاکسازی {}", kind.title()),
            format!("p:{opener}:{chat}:l:{}:{CLEAR_KEY}", kind.action()).into_bytes(),
            super::style::Colour::Danger,
        )]);
    }
    rows.push(vec![super::premium::decorate(
        Button::data("بازگشت", format!("p:{opener}:{chat}:ls").into_bytes()),
        Some(super::premium::Icon::Back),
    )]);

    let shown = entries.len().min(LIMIT);
    let entry_count = if entries.len() >= MEMBER_LIST_PAGE {
        format!("{}+", entries.len())
    } else {
        entries.len().to_string()
    };
    let mut title = format!(
        "<b>پنل مدیریت</b> › <b>{}</b> ({})\n\n{}",
        kind.title(),
        entry_count,
        kind.hint()
    );
    if entries.len() > shown {
        title.push_str(&format!("\n<i>{shown} مورد اول نمایش داده شده است.</i>"));
    }
    if entries.is_empty() {
        title = format!(
            "<b>پنل مدیریت</b> › <b>{}</b>\n\nلیست خالی است.",
            kind.title()
        );
    }
    Ok((title, super::premium::buttons(&rows)))
}

pub async fn remove(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    entry_key: &str,
) -> Result<bool, MutationError> {
    if kind == Kind::Command {
        if let Some((word, _)) = restrict::custom_triggers(ctx, chat)
            .into_iter()
            .find(|(word, _)| word_id(word) == entry_key)
        {
            ctx.settings
                .try_set(chat, &restrict::custom_key(&word), false)
                .await?;
            return Ok(true);
        }
        return Ok(false);
    }
    if kind == Kind::Answer {
        if let Some(trigger) = super::answers::triggers(ctx, chat)
            .into_iter()
            .find(|trigger| word_id(trigger) == entry_key)
        {
            ctx.settings
                .try_set(chat, &format!("{}{trigger}", super::answers::PREFIX), false)
                .await?;
            return Ok(true);
        }
        return Ok(false);
    }
    if kind == Kind::Filter {
        if let Some(word) = filters::words(ctx, chat)
            .into_iter()
            .find(|word| word_id(word) == entry_key)
        {
            ctx.settings
                .try_set(chat, &filters::key(&word), false)
                .await?;
            return Ok(true);
        }
        return Ok(false);
    }
    if kind == Kind::Image {
        if let Some((name, _)) = imgfilter::listing(ctx, chat)
            .await?
            .into_iter()
            .find(|(name, _)| word_id(name) == entry_key)
        {
            imgfilter::try_forget(ctx, chat, &name).await?;
            return Ok(true);
        }
        return Ok(false);
    }
    if kind == Kind::Pack {
        if let Ok(set) = entry_key.parse::<i64>() {
            ctx.settings.try_set(chat, &packs::key(set), false).await?;
            return Ok(true);
        }
        return Ok(false);
    }
    let Ok(user_id) = entry_key.parse::<i64>() else {
        return Ok(false);
    };
    if kind == Kind::Vip {
        ctx.settings
            .try_set(chat, &vip::key(user_id), false)
            .await?;
        return Ok(true);
    }
    if kind == Kind::Exempt {
        join::set_free(ctx, chat, user_id, false).await?;
        return Ok(true);
    }

    let Some(peer) = entries(ctx, chat_ref, chat, kind, MEMBER_LIST_PAGE)
        .await?
        .into_iter()
        .find(|entry| entry.key == entry_key)
        .and_then(|entry| entry.peer)
    else {
        return Ok(false);
    };
    restrict::apply_maintenance(
        ctx,
        chat_ref,
        peer,
        Action::Unban,
        None,
        restrict::By {
            reason: "پنل لیست ها",
            ..Default::default()
        },
    )
    .await
    .map_err(MutationError::Restriction)?;
    Ok(true)
}

pub(crate) async fn entries(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    limit: usize,
) -> Result<Vec<Entry>, sqlx::Error> {
    if kind == Kind::Command {
        return Ok(restrict::custom_triggers(ctx, chat)
            .into_iter()
            .map(|(word, action)| Entry {
                key: word_id(&word),
                name: esc(&format!("{word} ({})", restrict::action_label(action))),
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Answer {
        return Ok(super::answers::triggers(ctx, chat)
            .into_iter()
            .map(|trigger| Entry {
                key: word_id(&trigger),
                name: esc(&trigger),
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Filter {
        return Ok(filters::words(ctx, chat)
            .into_iter()
            .map(|word| Entry {
                key: word_id(&word),
                name: esc(&word),
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Image {
        return Ok(imgfilter::listing(ctx, chat)
            .await?
            .into_iter()
            .map(|(name, label)| Entry {
                key: word_id(&name),
                name: esc(&label),
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Pack {
        let mut found: Vec<Entry> = ctx
            .settings
            .values_with_prefix(chat, packs::PREFIX)
            .into_iter()
            .map(|(set, title)| Entry {
                key: set,
                name: esc(&title),
                peer: None,
            })
            .collect();
        found.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        found.truncate(limit);
        return Ok(found);
    }
    if kind == Kind::Exempt {
        return Ok(ctx
            .settings
            .flags_with_prefix(chat, join::EXEMPT)
            .into_iter()
            .map(|id| Entry {
                key: id.clone(),
                name: id,
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Vip {
        return Ok(ctx
            .settings
            .flags_with_prefix(chat, vip::PREFIX)
            .into_iter()
            .map(|id| Entry {
                key: id.clone(),
                name: id,
                peer: None,
            })
            .collect());
    }

    let mut participants = ctx.client.iter_participants(chat_ref).filter(kind.filter());
    let mut found = Vec::with_capacity(limit.min(LIMIT * 16));
    loop {
        match participants.next().await {
            Ok(Some(participant)) => {
                if found.len() >= limit {
                    break;
                }
                let id = participant.id();
                let key = match id.bare_id() {
                    Some(id) => id.to_string(),
                    None => continue,
                };
                let peer = participant.peer();
                found.push(Entry {
                    key: key.clone(),
                    name: peer.and_then(|peer| peer.name()).map(esc).unwrap_or(key),
                    peer: match peer {
                        Some(peer) => peer.to_ref().await.ok().flatten(),
                        None => None,
                    },
                });
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("lists: {chat}: could not list {}: {e}", kind.action());
                break;
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_unbans_never_consume_reserved_enforcement_capacity() {
        let source = include_str!("lists.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert_eq!(
            production.matches("restrict::apply_maintenance(").count(),
            2
        );
        assert!(!production.contains("restrict::apply("));
    }

    #[test]
    fn pack_kind_round_trips_through_panel_actions() {
        assert!(matches!(Kind::from_action("pack"), Some(Kind::Pack)));
        assert_eq!(Kind::Pack.action(), "pack");
        assert_eq!(Kind::Pack.cap().key, super::super::limits::SET.key);
    }

    #[test]
    fn payloads_match_what_the_panel_parses() {
        let (opener, chat) = (1234567890_i64, -1001234567890_i64);
        for payload in [
            format!(
                "p:{opener}:{chat}:l:{}:{}",
                Kind::Filter.action(),
                "deadbeef"
            ),
            format!("p:{opener}:{chat}:l:{}:{}", Kind::Ban.action(), 42),
            format!("p:{opener}:{chat}:adv"),
        ] {
            let rest = payload.strip_prefix("p:").expect("panel prefix");
            let mut parts = rest.splitn(3, ':');
            assert_eq!(parts.next().and_then(|p| p.parse().ok()), Some(opener));
            assert_eq!(parts.next().and_then(|p| p.parse().ok()), Some(chat));
            assert!(parts.next().is_some_and(|action| !action.is_empty()));
            assert!(payload.len() <= 64, "payload too long: {payload}");
        }
    }

    #[test]
    fn the_clear_keys_cannot_be_mistaken_for_an_entry() {
        for key in [CLEAR_KEY, "clearyes"] {
            assert!(clearing(key).is_some());
            assert!(key.parse::<i64>().is_err(), "{key} could be a user id");
            assert!(
                !key.chars().all(|c| c.is_ascii_hexdigit()),
                "{key} could be a word_id hash"
            );
        }
        assert_eq!(clearing(&word_id("تبلیغ")), None);
        assert_eq!(clearing("12345"), None);
        assert_eq!(clearing(CLEAR_KEY), Some(false));
        assert_eq!(clearing("clearyes"), Some(true));
    }

    #[test]
    fn clear_payloads_fit_telegram() {
        for kind in [
            Kind::Ban,
            Kind::Mute,
            Kind::Vip,
            Kind::Filter,
            Kind::Image,
            Kind::Answer,
            Kind::Exempt,
            Kind::Command,
            Kind::Pack,
        ] {
            for key in [CLEAR_KEY, "clearyes"] {
                let payload = format!("p:{}:{}:l:{}:{key}", i64::MAX, i64::MIN, kind.action());
                assert!(payload.len() <= 64, "payload too long: {payload}");
            }
        }
    }
}
