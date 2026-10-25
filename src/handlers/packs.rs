use grammers_client::message::Message;
use grammers_client::tl;

use super::Ctx;

pub const PREFIX: &str = "pack:";

pub const LOCK: &[&str] = &[
    "قفل پک ایموجی",
    "قفل پک استیکر",
    "قفل استیکر پک",
    "قفل استیکرپک",
    "قفل پک",
    "/lockpack",
    "/banpack",
];
pub const UNLOCK: &[&str] = &[
    "باز کردن پک ایموجی",
    "باز کردن پک استیکر",
    "بازکردن پک",
    "باز کردن پک",
    "حذف قفل پک",
    "حذف پک",
    "آنلاک پک",
    "/unlockpack",
    "/unbanpack",
];

pub(crate) fn key(set: i64) -> String {
    format!("{PREFIX}{set}")
}

pub fn set_of_media(media: &grammers_client::media::Media) -> Option<i64> {
    let grammers_client::media::Media::Sticker(sticker) = media else {
        return None;
    };
    match &sticker.raw_attrs.stickerset {
        tl::enums::InputStickerSet::Id(set) => Some(set.id),
        _ => None,
    }
}

pub fn set_of(message: &Message) -> Option<i64> {
    set_of_media(&message.media()?)
}

pub fn is_banned(ctx: &Ctx, chat: i64, view: &super::locks::View<'_>) -> bool {
    if ctx.settings.indexed_empty(chat, PREFIX) {
        return false;
    }
    view.media()
        .and_then(set_of_media)
        .is_some_and(|set| ctx.settings.value(chat, &key(set)).is_some())
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some((locking, name)) = parse_command(text) else {
        return false;
    };
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }

    let found = match name {
        Some(name) => {
            if !locking {
                if let Ok(set) = name.parse::<i64>() {
                    if let Some(title) = (set > 0)
                        .then(|| ctx.settings.value(chat, &key(set)))
                        .flatten()
                    {
                        Some((set, title))
                    } else {
                        resolve(ctx, &name).await
                    }
                } else {
                    resolve(ctx, &name).await
                }
            } else {
                resolve(ctx, &name).await
            }
        }
        None => message
            .get_reply()
            .await
            .ok()
            .flatten()
            .as_ref()
            .and_then(set_of)
            .map(|set| (set, String::new())),
    };
    let Some((set, title)) = found else {
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
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
        if let Err(error) = ctx.settings.try_set_value(chat, &key(set), &label).await {
            ::log::warn!("packs: lock write for {chat}/{set} failed: {error}");
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::CommandError,
                if error.commit_outcome_unknown() {
                    "نتیجه ذخیره قفل پک نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "قفل پک ذخیره نشد؛ دوباره تلاش کنید."
                },
            )
            .await;
            return true;
        }
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::LockManagement,
            super::premium::icon_text(
                Some(super::premium::Icon::Locked),
                format!("«{label}» قفل شد."),
            ),
        )
        .await;
    } else {
        if let Err(error) = ctx.settings.try_set(chat, &key(set), false).await {
            ::log::warn!("packs: unlock write for {chat}/{set} failed: {error}");
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::CommandError,
                if error.commit_outcome_unknown() {
                    "نتیجه حذف قفل پک نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "قفل پک حذف نشد؛ دوباره تلاش کنید."
                },
            )
            .await;
            return true;
        }
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::LockManagement,
            super::premium::icon_text(
                Some(super::premium::Icon::Unlocked),
                format!("«{label}» باز شد."),
            ),
        )
        .await;
    }
    true
}

fn short_name(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    let path = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))
        .unwrap_or(text);
    let linked = path
        .strip_prefix("t.me/addstickers/")
        .or_else(|| path.strip_prefix("telegram.me/addstickers/"));
    let name = linked.unwrap_or(text);
    let name = if let Some((name, query)) = name.split_once('?') {
        if linked.is_none() || !query.contains('=') {
            return None;
        }
        name
    } else {
        name
    };
    let name = if linked.is_some() {
        name.strip_suffix('/').unwrap_or(name)
    } else {
        name
    };
    (!name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then(|| name.to_owned())
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

fn parse_command(text: &str) -> Option<(bool, Option<String>)> {
    [(LOCK, true), (UNLOCK, false)]
        .into_iter()
        .flat_map(|(commands, locking)| {
            commands.iter().filter_map(move |command| {
                super::commands::strip_prefix(text, command)
                    .map(|tail| (command.chars().count(), locking, tail))
            })
        })
        .max_by_key(|(len, _, _)| *len)
        .and_then(|(_, locking, tail)| {
            if tail.is_empty() {
                Some((locking, None))
            } else {
                short_name(&super::digits(tail)).map(|name| (locking, Some(name)))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_aliases_keep_link_casing_and_boundaries() {
        let link = "https://t.me/addstickers/MyPack";
        for command in [
            "قفل پک ایموجی",
            "قفل پک استیکر",
            "قفل\tپک ايموجي",
            "/LOCKPACK",
        ] {
            assert_eq!(
                parse_command(&format!("{command} {link}")),
                Some((true, Some("MyPack".to_owned()))),
                "{command}"
            );
        }
        assert_eq!(parse_command("باز کردن پک ایموجی"), Some((false, None)));
        assert_eq!(
            parse_command("حذف قفل پک CatPack"),
            Some((false, Some("CatPack".to_owned())))
        );
        assert_eq!(
            parse_command("باز کردن پک 123456789"),
            Some((false, Some("123456789".to_owned())))
        );
        assert_eq!(
            parse_command("باز کردن پک ۱۲۳۴۵۶۷۸۹"),
            Some((false, Some("123456789".to_owned())))
        );
        assert_eq!(parse_command("قفل پکهای من"), None);
        assert_eq!(parse_command("/lockpack@otherbot CatPack"), None);
        for text in [
            "قفل پک CatPack چیه",
            "قفل پک https://t.me/addstickers/CatPack چیه",
            "قفل پک CatPack?",
            "قفل پک چرا؟",
            "باز کردن پک CatPack/other",
        ] {
            assert_eq!(parse_command(text), None, "{text}");
        }
    }
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
        assert_eq!(short_name("CatPack?"), None);
        assert_eq!(short_name("CatPack because"), None);
        assert_eq!(short_name("t.me/addstickers/CatPack/other"), None);
    }
}
