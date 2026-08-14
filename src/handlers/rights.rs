use grammers_client::message::{InputMessage, Message};
use grammers_client::tl;

use super::{Ctx, can_manage};

pub const PREFIX: &str = "perm:";

const SEEDED: &str = "perm_seeded";

pub struct Right {
    pub key: &'static str,
    pub label: &'static str,
}

pub const RIGHTS: &[Right] = &[
    Right { key: "plain", label: "ارسال پیام" },
    Right { key: "photos", label: "ارسال عکس" },
    Right { key: "videos", label: "ارسال ویدیو" },
    Right { key: "rounds", label: "ارسال ویدیو سلفی" },
    Right { key: "audios", label: "ارسال آهنگ" },
    Right { key: "voices", label: "ارسال ویس" },
    Right { key: "docs", label: "ارسال فایل" },
    Right { key: "stickers", label: "ارسال استیکر و گیف" },
    Right { key: "polls", label: "ارسال نظرسنجی" },
    Right { key: "links", label: "پیش نمایش لینک" },
    Right { key: "reactions", label: "ری اکشن به پیام" },
    Right { key: "info", label: "تغییر اطلاعات گروه" },
    Right { key: "invite", label: "دعوت کاربران" },
    Right { key: "pin", label: "سنجاق کردن پیام" },
];

const OPEN_WORDS: &[&str] = &["باز", "آزاد", "روشن"];
const CLOSED_WORDS: &[&str] = &["بسته", "قفل", "خاموش"];
const SHOW: &[&str] = &["اختیارات گروه", "اختیارات", "مجوزها", "مجوزهای گروه"];
const SET: &[&str] = &["اختیار", "مجوز"];

pub fn key_of(right: &str) -> String {
    format!("{PREFIX}{right}")
}

pub fn closed(ctx: &Ctx, chat: i64, right: &str) -> bool {
    ctx.settings.is_locked(chat, &key_of(right))
}

pub async fn set_closed(ctx: &Ctx, chat: i64, right: &str, shut: bool) {
    ctx.settings.set(chat, &key_of(right), shut).await;
}

pub fn banned_rights(ctx: &Ctx, chat: i64, force_all: bool) -> tl::types::ChatBannedRights {
    build(&|right: &str| closed(ctx, chat, right), force_all)
}

fn build(closed: &dyn Fn(&str) -> bool, force_all: bool) -> tl::types::ChatBannedRights {
    let shut = |right: &str| force_all || closed(right);

    let (photos, videos, rounds) = (shut("photos"), shut("videos"), shut("rounds"));
    let (audios, voices, docs) = (shut("audios"), shut("voices"), shut("docs"));
    tl::types::ChatBannedRights {
        view_messages: false,

        send_messages: false,

        send_media: photos && videos && rounds && audios && voices && docs,
        send_photos: photos,
        send_videos: videos,
        send_roundvideos: rounds,
        send_audios: audios,
        send_voices: voices,
        send_docs: docs,
        send_plain: shut("plain"),

        send_stickers: shut("stickers"),
        send_gifs: shut("stickers"),
        send_games: shut("stickers"),
        send_inline: shut("stickers"),
        send_polls: shut("polls"),
        embed_links: shut("links"),
        send_reactions: shut("reactions"),
        change_info: shut("info"),
        invite_users: shut("invite"),
        pin_messages: shut("pin"),

        manage_topics: force_all,
        manage_linked_peers: force_all,

        edit_rank: false,

        until_date: 0,
    }
}

pub async fn apply(
    ctx: &Ctx,
    chat_ref: grammers_client::session::types::PeerRef,
    chat: i64,
    force_all: bool,
) -> bool {
    let rights = banned_rights(ctx, chat, force_all);
    match ctx
        .client
        .invoke(&tl::functions::messages::EditChatDefaultBannedRights {
            peer: chat_ref.into(),
            banned_rights: rights.into(),
        })
        .await
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("rights: {chat}: {e}");
            false
        }
    }
}

pub async fn seed(ctx: &Ctx, message: &Message, chat: i64) {
    if ctx.settings.is_locked(chat, SEEDED) {
        return;
    }
    let Some(current) = live(message) else {
        return;
    };
    let tl::enums::ChatBannedRights::Rights(current) = current;
    for (right, shut) in [
        ("plain", current.send_plain || current.send_messages),
        ("photos", current.send_photos || current.send_media),
        ("videos", current.send_videos || current.send_media),
        ("rounds", current.send_roundvideos || current.send_media),
        ("audios", current.send_audios || current.send_media),
        ("voices", current.send_voices || current.send_media),
        ("docs", current.send_docs || current.send_media),
        ("stickers", current.send_stickers),
        ("polls", current.send_polls),
        ("links", current.embed_links),
        ("reactions", current.send_reactions),
        ("info", current.change_info),
        ("invite", current.invite_users),
        ("pin", current.pin_messages),
    ] {
        set_closed(ctx, chat, right, shut).await;
    }
    ctx.settings.set(chat, SEEDED, true).await;
}

fn live(message: &Message) -> Option<tl::enums::ChatBannedRights> {
    let grammers_client::peer::Peer::Group(group) = message.peer()? else {
        return None;
    };
    match &group.raw {
        tl::enums::Chat::Channel(channel) => channel.default_banned_rights.clone(),
        tl::enums::Chat::Chat(chat) => chat.default_banned_rights.clone(),
        _ => None,
    }
}

pub fn status(ctx: &Ctx, chat: i64) -> String {
    let lines = RIGHTS
        .iter()
        .map(|right| {
            format!(
                "{} {}",
                if closed(ctx, chat, right.key) { "✗" } else { "✓" },
                right.label
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<b>اختیارات گروه</b>\n\n{lines}\n\n\
         <i>این ها اختیار اعضای عادی است و ادمین ها شامل آن نمی شوند. \
         تغییر با دستور: «اختیار عکس بسته»</i>"
    )
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if SHOW.contains(&text) {
        if !can_manage(ctx, message).await {
            return true;
        }
        seed(ctx, message, chat).await;
        let Some(opener) = message
            .sender_id()
            .and_then(grammers_client::session::types::PeerId::bare_id)
        else {
            return false;
        };
        let _ = message
            .reply(
                InputMessage::new()
                    .html(status(ctx, chat))
                    .reply_markup(super::panel::rights_markup(ctx, chat, opener)),
            )
            .await;
        return true;
    }

    let Some(rest) = SET.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        rest.starts_with(char::is_whitespace).then(|| rest.trim())
    }) else {
        return false;
    };

    let Some((name, state)) = rest.rsplit_once(char::is_whitespace) else {
        return false;
    };
    let shut = match (OPEN_WORDS.contains(&state), CLOSED_WORDS.contains(&state)) {
        (true, _) => false,
        (_, true) => true,
        _ => return false,
    };
    let name = name.trim();
    let Some(right) = RIGHTS
        .iter()
        .find(|right| right.label == name || right.label.ends_with(name))
    else {
        let _ = message
            .reply(InputMessage::new().html(format!(
                "چنین اختیاری نداریم.\n\n{}",
                status(ctx, chat)
            )))
            .await;
        return true;
    };
    if !can_manage(ctx, message).await {
        return true;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    seed(ctx, message, chat).await;
    set_closed(ctx, chat, right.key, shut).await;
    let done = apply(ctx, chat_ref, chat, false).await;
    let _ = message
        .reply(match (done, shut) {
            (true, true) => format!("✗ {} برای اعضای عادی بسته شد.", right.label),
            (true, false) => format!("✓ {} برای اعضای عادی باز شد.", right.label),
            (false, _) => {
                "انجام نشد. مطمئن شوید ربات اجازه تغییر اطلاعات گروه دارد.".to_owned()
            }
        })
        .await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanket_flags_follow_the_granular_ones() {
        let closed_none = |_: &str| false;
        let one_closed = |right: &str| right == "photos";
        let all_media = |right: &str| {
            ["photos", "videos", "rounds", "audios", "voices", "docs"].contains(&right)
        };
        for (shut, expect_media) in [
            (&closed_none as &dyn Fn(&str) -> bool, false),
            (&one_closed, false),
            (&all_media, true),
        ] {
            let rights = build(shut, false);
            assert_eq!(rights.send_media, expect_media);

            assert!(!rights.send_messages);
            assert!(!rights.view_messages);
            assert_eq!(rights.until_date, 0);
        }
        assert!(build(&all_media, false).send_photos);
        assert!(!build(&closed_none, false).send_photos);

        let locked = build(&closed_none, true);
        assert!(locked.send_plain && locked.send_photos && locked.send_media);
    }

    #[test]
    fn every_right_has_a_unique_key_and_label() {
        let mut keys: Vec<&str> = RIGHTS.iter().map(|right| right.key).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "duplicate right key");
    }
}
