use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::PeerRef;
use grammers_client::tl;

use super::restrict::{self, Action};
use super::{Ctx, can_manage, esc, filters, join, vip};

const SHOW: &[(&str, Kind)] = &[
    ("لیست بن", Kind::Ban),
    ("لیست سیک", Kind::Ban),
    ("لیست سکوت", Kind::Mute),
    ("لیست خفه", Kind::Mute),
    ("لیست ویژه", Kind::Vip),
    ("لیست فیلتر", Kind::Filter),
    ("لیست پاسخ", Kind::Answer),
    ("لیست معاف", Kind::Exempt),
];

const CLEAR: &[(&str, Kind)] = &[
    ("پاکسازی لیست بن", Kind::Ban),
    ("پاکسازی لیست سیک", Kind::Ban),
    ("پاکسازی لیست سکوت", Kind::Mute),
    ("پاکسازی لیست خفه", Kind::Mute),
    ("پاکسازی لیست ویژه", Kind::Vip),
    ("پاکسازی لیست فیلتر", Kind::Filter),
    ("پاکسازی لیست پاسخ", Kind::Answer),
    ("پاکسازی لیست معاف", Kind::Exempt),
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
    if !can_manage(ctx, message).await {
        return true;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    if clear.is_some() {
        let removed = clear_all(ctx, chat_ref, chat, kind).await;
        let _ = message
            .reply(format!("✓ {removed} مورد از {} حذف شد.", kind.title()))
            .await;
        return true;
    }

    let Some(opener) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };
    let (title, markup) = view(ctx, chat_ref, chat, kind, opener).await;
    let _ = message
        .reply(InputMessage::new().html(title).reply_markup(markup))
        .await;
    true
}

pub async fn clear_all(ctx: &Ctx, chat_ref: PeerRef, chat: i64, kind: Kind) -> usize {
    let entries = entries(ctx, chat_ref, chat, kind).await;
    let mut removed = 0;
    for entry in entries {
        if matches!(kind, Kind::Filter | Kind::Vip | Kind::Answer | Kind::Exempt) {
            remove(ctx, chat_ref, chat, kind, &entry.key).await;
            removed += 1;
            continue;
        }
        let Some(peer) = entry.peer else {
            eprintln!("lists: {chat}: no ref for user {}", entry.key);
            continue;
        };
        match restrict::apply(ctx, chat_ref, peer, Action::Unban, None, restrict::By { reason: "پنل لیست ها", ..Default::default() }).await {
            Ok(()) => removed += 1,
            Err(e) => eprintln!("lists: {chat}: could not clear {}: {e}", entry.key),
        }
    }
    removed
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Ban,
    Mute,
    Vip,
    Filter,
    Answer,
    Exempt,
}

const LIMIT: usize = 20;

impl Kind {
    pub fn from_action(action: &str) -> Option<Self> {
        match action {
            "ban" => Some(Self::Ban),
            "mute" => Some(Self::Mute),
            "vip" => Some(Self::Vip),
            "filter" => Some(Self::Filter),
            "answer" => Some(Self::Answer),
            "free" => Some(Self::Exempt),
            _ => None,
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Ban => "ban",
            Self::Mute => "mute",
            Self::Vip => "vip",
            Self::Filter => "filter",
            Self::Answer => "answer",
            Self::Exempt => "free",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Ban => "بن شده ها",
            Self::Mute => "سکوت شده ها",
            Self::Vip => "کاربران ویژه",
            Self::Filter => "لیست فیلتر",
            Self::Answer => "پاسخ خودکار",
            Self::Exempt => "معاف ها",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Ban => "برای رفع بن روی هر نام بزنید.",
            Self::Mute => "برای رفع سکوت روی هر نام بزنید.",
            Self::Vip => "برای حذف از لیست ویژه روی هر مورد بزنید.",
            Self::Filter => "برای حذف کلمه روی آن بزنید.",
            Self::Answer => "برای حذف یک پاسخ روی آن بزنید.",
            Self::Exempt => "معاف از عضویت اجباری و اد اجباری. برای حذف روی هر مورد بزنید.",
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

struct Entry {
    key: String,
    name: String,
    peer: Option<PeerRef>,
}

fn word_id(word: &str) -> String {
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
) -> (String, ReplyMarkup) {
    let count = entries(ctx, chat_ref, chat, kind).await.len();
    let title = format!(
        "<b>پنل مدیریت</b> › <b>{}</b>\n\n\
         همه <b>{count}</b> مورد از این لیست حذف می شود. این کار برگشت ندارد.",
        kind.title()
    );
    let markup = ReplyMarkup::from_buttons(&[
        vec![
            super::style::data(
                "✅  تایید",
                format!("p:{opener}:{chat}:l:{}:{CLEAR_CONFIRMED}", kind.action()).into_bytes(),
                super::style::Colour::Success,
            ),
            Button::data(
                "❌  لغو",
                format!("p:{opener}:{chat}:l:{}", kind.action()).into_bytes(),
            ),
        ],
    ]);
    (title, markup)
}

pub async fn view(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    opener: i64,
) -> (String, ReplyMarkup) {
    let entries = entries(ctx, chat_ref, chat, kind).await;

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
    rows.push(vec![Button::data(
        "‹ بازگشت",
        format!("p:{opener}:{chat}:ls").into_bytes(),
    )]);

    let shown = entries.len().min(LIMIT);
    let mut title = format!(
        "<b>پنل مدیریت</b> › <b>{}</b> ({})\n\n{}",
        kind.title(),
        entries.len(),
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
    (title, ReplyMarkup::from_buttons(&rows))
}

pub async fn remove(ctx: &Ctx, chat_ref: PeerRef, chat: i64, kind: Kind, entry_key: &str) {
    if kind == Kind::Answer {
        if let Some(trigger) = super::answers::triggers(ctx, chat)
            .into_iter()
            .find(|trigger| word_id(trigger) == entry_key)
        {
            ctx.settings
                .set(chat, &format!("{}{trigger}", super::answers::PREFIX), false)
                .await;
        }
        return;
    }
    if kind == Kind::Filter {
        if let Some(word) = filters::words(ctx, chat)
            .into_iter()
            .find(|word| word_id(word) == entry_key)
        {
            ctx.settings.set(chat, &filters::key(&word), false).await;
        }
        return;
    }
    let Ok(user_id) = entry_key.parse::<i64>() else {
        return;
    };
    if kind == Kind::Vip {
        ctx.settings.set(chat, &vip::key(user_id), false).await;
        return;
    }
    if kind == Kind::Exempt {
        join::set_free(ctx, chat, user_id, false).await;
        return;
    }

    let Some(peer) = entries(ctx, chat_ref, chat, kind)
        .await
        .into_iter()
        .find(|entry| entry.key == entry_key)
        .and_then(|entry| entry.peer)
    else {
        eprintln!("lists: {chat}: no ref for user {user_id}");
        return;
    };
    if let Err(e) = restrict::apply(ctx, chat_ref, peer, Action::Unban, None, restrict::By { reason: "پنل لیست ها", ..Default::default() }).await {
        eprintln!("lists: {chat}: could not lift restriction on {user_id}: {e}");
    }
}

async fn entries(ctx: &Ctx, chat_ref: PeerRef, chat: i64, kind: Kind) -> Vec<Entry> {
    if kind == Kind::Answer {
        return super::answers::triggers(ctx, chat)
            .into_iter()
            .map(|trigger| Entry {
                key: word_id(&trigger),
                name: esc(&trigger),
                peer: None,
            })
            .collect();
    }
    if kind == Kind::Filter {
        return filters::words(ctx, chat)
            .into_iter()
            .map(|word| Entry {
                key: word_id(&word),
                name: esc(&word),
                peer: None,
            })
            .collect();
    }
    if kind == Kind::Exempt {
        return ctx
            .settings
            .flags_with_prefix(chat, join::EXEMPT)
            .into_iter()
            .map(|id| Entry {
                key: id.clone(),
                name: id,
                peer: None,
            })
            .collect();
    }
    if kind == Kind::Vip {
        return ctx
            .settings
            .flags_with_prefix(chat, vip::PREFIX)
            .into_iter()
            .map(|id| Entry {
                key: id.clone(),
                name: id,
                peer: None,
            })
            .collect();
    }

    let mut participants = ctx.client.iter_participants(chat_ref).filter(kind.filter());
    let mut found = Vec::new();
    loop {
        match participants.next().await {
            Ok(Some(participant)) => {
                let user = participant.user;
                found.push(Entry {
                    key: user.id().bare_id_unchecked().to_string(),
                    name: esc(&user.full_name()),
                    peer: user.to_ref().await.ok().flatten(),
                });
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("lists: {chat}: could not list {}: {e}", kind.action());
                break;
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_match_what_the_panel_parses() {
        let (opener, chat) = (1234567890_i64, -1001234567890_i64);
        for payload in [
            format!("p:{opener}:{chat}:l:{}:{}", Kind::Filter.action(), "deadbeef"),
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
        for kind in [Kind::Ban, Kind::Mute, Kind::Vip, Kind::Filter, Kind::Answer, Kind::Exempt] {
            for key in [CLEAR_KEY, "clearyes"] {
                let payload = format!(
                    "p:{}:{}:l:{}:{key}",
                    i64::MAX,
                    i64::MIN,
                    kind.action()
                );
                assert!(payload.len() <= 64, "payload too long: {payload}");
            }
        }
    }
}
