use grammers_client::message::Message;
use grammers_client::tl;

use super::{Ctx};

pub const PREFIX: &str = "pack:";

const LOCK: &[&str] = &["قفل پک", "قفل پک ایموجی", "قفل استیکرپک"];
const UNLOCK: &[&str] = &["بازکردن پک", "باز کردن پک", "حذف پک", "آنلاک پک"];

fn key(set: i64) -> String {
    format!("{PREFIX}{set}")
}

pub fn set_of(message: &Message) -> Option<i64> {
    let media = message.media()?;
    let grammers_client::media::Media::Sticker(sticker) = media else {
        return None;
    };
    match &sticker.raw_attrs.stickerset {
        tl::enums::InputStickerSet::Id(set) => Some(set.id),
        _ => None,
    }
}

pub fn is_banned(ctx: &Ctx, chat: i64, message: &Message) -> bool {
    if ctx.settings.indexed_empty(chat, PREFIX) {
        return false;
    }
    set_of(message).is_some_and(|set| ctx.settings.value(chat, &key(set)).is_some())
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let locking = LOCK.iter().any(|c| text == *c || starts(text, c));
    let unlocking = UNLOCK.iter().any(|c| text == *c || starts(text, c));
    if !locking && !unlocking {
        return false;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }

    let arg = LOCK
        .iter()
        .chain(UNLOCK.iter())
        .find_map(|c| text.strip_prefix(*c))
        .map(str::trim)
        .unwrap_or("");

    let found = match message.get_reply().await.ok().flatten().as_ref().and_then(set_of) {
        Some(set) => Some((set, String::new())),
        None => match short_name(arg) {
            Some(name) => resolve(ctx, &name).await,
            None => None,
        },
    };
    let Some((set, title)) = found else {
        let _ = message
            .reply(
                "روی یک استیکر از آن پک ریپلای کنید، یا لینک پک را بفرستید:\n\
                 «قفل پک https://t.me/addstickers/NAME»",
            )
            .await;
        return true;
    };

    let label = if title.is_empty() {
        format!("پک {set}")
    } else {
        title
    };
    if locking {
        ctx.settings.set_value(chat, &key(set), &label).await;
        let _ = message.reply(format!("✓ «{label}» قفل شد.")).await;
    } else {
        ctx.settings.set(chat, &key(set), false).await;
        let _ = message.reply(format!("✗ «{label}» باز شد.")).await;
    }
    true
}

fn short_name(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let name = text
        .rsplit_once("addstickers/")
        .map(|(_, name)| name)
        .unwrap_or(text);
    let name = name.split(['?', '/', ' ']).next()?.trim();
    (!name.is_empty() && !name.contains("://")).then(|| name.to_owned())
}

async fn resolve(ctx: &Ctx, short_name: &str) -> Option<(i64, String)> {
    let set = ctx
        .client
        .invoke(&tl::functions::messages::GetStickerSet {
            stickerset: tl::types::InputStickerSetShortName {
                short_name: short_name.to_owned(),
            }
            .into(),
            hash: 0,
        })
        .await
        .ok()?;
    match set {
        tl::enums::messages::StickerSet::Set(set) => {
            let tl::enums::StickerSet::Set(info) = set.set;
            Some((info.id, info.title))
        }
        _ => None,
    }
}

fn starts(text: &str, command: &str) -> bool {
    text.strip_prefix(command)
        .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pack_names_from_links() {
        assert_eq!(
            short_name("https://t.me/addstickers/CatPack"),
            Some("CatPack".to_owned())
        );
        assert_eq!(
            short_name("t.me/addstickers/CatPack?x=1"),
            Some("CatPack".to_owned())
        );
        assert_eq!(short_name("CatPack"), Some("CatPack".to_owned()));
        assert_eq!(short_name(""), None);
    }
}
