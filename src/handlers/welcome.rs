use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::PeerId;
use grammers_client::tl;
use grammers_client::tl::{Deserializable, Serializable};

use super::{Ctx, can_manage, esc};

pub const TEXT: &str = "welcome_text";

pub const MEDIA: &str = "welcome_media";

const SET: &[&str] = &["تنظیم خوشامد", "خوشامد", "خوش امد"];
const CLEAR: &[&str] = &["حذف خوشامد", "خاموش خوشامد"];
const SHOW: &[&str] = &["نمایش خوشامد", "تست خوشامد"];

const TAGS: &[(&str, &str)] = &[
    ("{نام}", "display name"),
    ("{منشن}", "clickable mention"),
    ("{آیدی}", "numeric id"),
    ("{یوزرنیم}", "@username, or the name when there is none"),
    ("{گروه}", "group title"),
];

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if CLEAR.contains(&text) {
        if !can_manage(ctx, message).await {
            return true;
        }
        ctx.settings.set_value(chat, TEXT, "").await;
        ctx.settings.set_value(chat, MEDIA, "").await;
        let _ = message.reply("✗ خوشامد خاموش شد.").await;
        return true;
    }

    if SHOW.contains(&text) {
        if !can_manage(ctx, message).await {
            return true;
        }
        match template(ctx, chat) {
            Some(_) => send(ctx, message, chat, None).await,
            None => {
                let _ = message.reply(help()).await;
            }
        }
        return true;
    }

    let Some((command, rest)) = SET.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| (*command, rest.trim()))
    }) else {
        return false;
    };

    if !rest.is_empty() && !super::phrase_carries_text(command) {
        return false;
    }
    if rest.is_empty() && message.media().is_none() && message.reply_to_message_id().is_none() {
        return false;
    }
    let rest = rest.to_owned();
    if !can_manage(ctx, message).await {
        return true;
    }

    let replied = message.get_reply().await.ok().flatten();
    let body = if rest.is_empty() {
        replied
            .as_ref()
            .map(|m| m.text().to_owned())
            .unwrap_or_default()
    } else {
        rest
    };

    let media = message
        .media()
        .or_else(|| replied.as_ref().and_then(|m| m.media()));

    if body.is_empty() && media.is_none() {
        let _ = message.reply(help()).await;
        return true;
    }

    ctx.settings.set_value(chat, TEXT, &body).await;

    let stored_media = media.as_ref().and_then(encode_media).unwrap_or_default();
    ctx.settings.set_value(chat, MEDIA, &stored_media).await;

    let _ = message
        .reply(format!(
            "✓ خوشامد ذخیره شد{}.",
            if stored_media.is_empty() {
                ""
            } else {
                " همراه با رسانه"
            }
        ))
        .await;
    true
}

pub async fn on_join(ctx: &Ctx, message: &Message) -> bool {
    let joined = matches!(
        message.action(),
        Some(tl::enums::MessageAction::ChatAddUser(_) | tl::enums::MessageAction::ChatJoinedByLink(_))
    );
    if !joined {
        return false;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if template(ctx, chat).is_none() {
        return false;
    }

    let joined = super::joined_users(ctx, message).await;
    if joined.is_empty() {
        send(ctx, message, chat, None).await;
        return true;
    }
    for person in joined {
        if person.is_bot {
            continue;
        }
        send(ctx, message, chat, Some((person.id, person.name))).await;
    }
    true
}

fn template(ctx: &Ctx, chat: i64) -> Option<String> {
    let text = ctx.settings.value(chat, TEXT).unwrap_or_default();
    let media = ctx.settings.value(chat, MEDIA).unwrap_or_default();
    (!text.is_empty() || !media.is_empty()).then_some(text)
}

async fn send(ctx: &Ctx, message: &Message, chat: i64, who: Option<(i64, String)>) {
    let Some(text) = template(ctx, chat) else {
        return;
    };
    let mut input = InputMessage::new().html(fill(message, &text, who));

    if let Some(stored) = ctx.settings.value(chat, MEDIA).filter(|m| !m.is_empty())
        && let Some(media) = decode_media(&stored)
    {
        input = input.media(media);
    }
    let _ = message.respond(input).await;
}

fn fill(message: &Message, template: &str, who: Option<(i64, String)>) -> String {
    let (id, name) = match who {
        Some((id, name)) => (id, name),
        None => (
            message.sender_id().and_then(PeerId::bare_id).unwrap_or_default(),
            super::name_of(message),
        ),
    };
    let username = message
        .sender()
        .filter(|_| who_is_sender(message, id))
        .and_then(|peer| peer.username().map(|u| format!("@{u}")))
        .unwrap_or_else(|| name.clone());
    let group = message
        .peer()
        .and_then(|peer| peer.name())
        .unwrap_or("گروه")
        .to_owned();

    template
        .replace("{نام}", &esc(&name))
        .replace("{name}", &esc(&name))
        .replace(
            "{منشن}",
            &format!("<a href=\"tg://user?id={id}\">{}</a>", esc(&name)),
        )
        .replace(
            "{mention}",
            &format!("<a href=\"tg://user?id={id}\">{}</a>", esc(&name)),
        )
        .replace("{آیدی}", &id.to_string())
        .replace("{id}", &id.to_string())
        .replace("{یوزرنیم}", &esc(&username))
        .replace("{username}", &esc(&username))
        .replace("{گروه}", &esc(&group))
        .replace("{group}", &esc(&group))
}

fn who_is_sender(message: &Message, id: i64) -> bool {
    message.sender_id().and_then(PeerId::bare_id) == Some(id)
}

fn help() -> String {
    let tags = TAGS
        .iter()
        .map(|(tag, what)| format!("‹ <code>{tag}</code> · {what}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<b>خوشامد</b>\n\n\
         روی یک پیام ریپلای کنید و «تنظیم خوشامد» بفرستید، یا متن را بعد از دستور بنویسید.\n\
         اگر پیام رسانه داشته باشد، همان رسانه با همان file id دوباره ارسال می شود.\n\n\
         <b>تگ ها</b>\n{tags}\n\n\
         <i>خاموش کردن: «حذف خوشامد»</i>"
    )
}

pub fn encode_media(media: &grammers_client::media::Media) -> Option<String> {
    input_media(media).map(|input| hex(&input.to_bytes()))
}

pub fn decode_media(stored: &str) -> Option<tl::enums::InputMedia> {
    tl::enums::InputMedia::from_bytes(&unhex(stored)?).ok()
}

fn input_media(media: &grammers_client::media::Media) -> Option<tl::enums::InputMedia> {
    use grammers_client::media::Media;
    Some(match media {
        Media::Photo(photo) => photo.to_raw_input_media().into(),
        Media::Document(document) => document.to_raw_input_media().into(),
        Media::Sticker(sticker) => sticker.document.to_raw_input_media().into(),
        Media::Contact(contact) => contact.to_raw_input_media().into(),
        Media::Poll(poll) => poll.to_raw_input_media().into(),

        _ => return None,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    text.len().is_multiple_of(2)
        .then(|| {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
                .collect::<Option<Vec<u8>>>()
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrips() {
        let bytes = vec![0u8, 1, 254, 255, 42];
        assert_eq!(unhex(&hex(&bytes)), Some(bytes));
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex("abc"), None);
    }
}
