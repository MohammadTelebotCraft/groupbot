use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::PeerId;
use grammers_client::tl;
use grammers_client::tl::{Deserializable, Serializable};

use super::{Ctx, esc};
use crate::response::ResponseKind;
use crate::state::{SettingMutation, SettingsWriteError};

pub const TEXT: &str = "welcome_text";

pub const MEDIA: &str = "welcome_media";

pub const ENTITIES: &str = "welcome_entities";

pub const TTL: &str = "welcome_ttl";

const DEFAULT_TTL: u32 = 0;
pub const TTL_RANGE: (u32, u32) = (0, 3600);
pub const TTL_PRESETS: &[u32] = &[0, 10, 30, 60, 300, 900];

pub const INSTALL_TTL: u32 = 10;

pub const SET: &[&str] = &["تنظیم خوشامد", "خوشامد", "خوش امد"];
pub const CLEAR: &[&str] = &["حذف خوشامد", "خاموش خوشامد"];
pub const SHOW: &[&str] = &["نمایش خوشامد", "تست خوشامد"];

const TAGS: &[(&str, &str)] = &[
    ("{نام}", "display name"),
    ("{منشن}", "clickable mention"),
    ("{آیدی}", "numeric id"),
    ("{یوزرنیم}", "@username, or the name when there is none"),
    ("{گروه}", "group title"),
];

pub fn ttl(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value(chat, TTL)
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_TTL)
        .clamp(TTL_RANGE.0, TTL_RANGE.1)
}

fn u16len(text: &str) -> i32 {
    text.encode_utf16().count() as i32
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if CLEAR.contains(&text) {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        match try_clear_stored(ctx, chat).await {
            Ok(()) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::SettingsChanged,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Pause),
                        "خوشامد خاموش شد.",
                    ),
                )
                .await
            }
            Err(error) => {
                ::log::warn!("welcome: clear for {chat} failed: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    setting_failure_text(&error),
                )
                .await
            }
        };
        return true;
    }

    if SHOW.contains(&text) {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        match template(ctx, chat) {
            Some(_) => send(ctx, message, chat, None, None).await,
            None => {
                super::respond(ctx, message, ResponseKind::CommandError, help()).await;
            }
        }
        return true;
    }

    let Some((command, after, rest)) = SET.iter().find_map(|command| {
        let after = text.strip_prefix(command)?;
        (after.is_empty() || after.starts_with(char::is_whitespace))
            .then(|| (*command, after, after.trim()))
    }) else {
        return false;
    };
    if !rest.is_empty() && !super::phrase_carries_text(command) {
        return false;
    }
    if rest.is_empty() && message.media().is_none() && message.reply_to_message_id().is_none() {
        return false;
    }
    let inline = rest.to_owned();
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }

    let replied = message.get_reply().await.ok().flatten();
    let (body, entities) = if inline.is_empty() {
        match replied.as_ref() {
            Some(replied) => (
                replied.text().to_owned(),
                replied.fmt_entities().cloned().unwrap_or_default(),
            ),
            None => (String::new(), Vec::new()),
        }
    } else {
        let raw = message.text();
        let skipped = (raw.len() - raw.trim_start().len())
            + command.len()
            + (after.len() - after.trim_start().len());
        let shift = u16len(&raw[..skipped]);
        let carried = message
            .fmt_entities()
            .map(|entities| rebase(entities, shift, u16len(&inline)))
            .unwrap_or_default();
        (inline, carried)
    };

    let media = message
        .media()
        .or_else(|| replied.as_ref().and_then(|m| m.media()));

    if body.is_empty() && media.is_none() {
        super::respond(ctx, message, ResponseKind::CommandError, help()).await;
        return true;
    }

    let stored_media = media.as_ref().and_then(encode_media).unwrap_or_default();
    let stored_entities = keep(&entities);
    let encoded_entities = if stored_entities.is_empty() {
        None
    } else {
        Some(hex(&stored_entities.to_bytes()))
    };
    let entity_mutation =
        encoded_entities
            .as_deref()
            .map_or(SettingMutation::Delete { key: ENTITIES }, |value| {
                SettingMutation::Put {
                    key: ENTITIES,
                    value,
                }
            });
    if let Err(error) = ctx
        .settings
        .try_apply_batch(
            chat,
            &[
                SettingMutation::Put {
                    key: TEXT,
                    value: &body,
                },
                SettingMutation::Put {
                    key: MEDIA,
                    value: &stored_media,
                },
                entity_mutation,
            ],
        )
        .await
    {
        ::log::warn!("welcome: setup for {chat} failed: {error}");
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            setting_failure_text(&error),
        )
        .await;
        return true;
    }
    let carried = encoded_entities.is_some();

    super::respond(
        ctx,
        message,
        ResponseKind::SettingsChanged,
        super::premium::icon_text(
            Some(super::premium::Icon::Welcome),
            format!(
                "خوشامد ذخیره شد{}{}.",
                if stored_media.is_empty() {
                    ""
                } else {
                    " همراه با رسانه"
                },
                if carried {
                    " با قالب بندی"
                } else {
                    ""
                }
            ),
        ),
    )
    .await;
    true
}

pub async fn try_clear_stored(ctx: &Ctx, chat: i64) -> Result<(), SettingsWriteError> {
    ctx.settings
        .try_apply_batch(
            chat,
            &[
                SettingMutation::Delete { key: TEXT },
                SettingMutation::Delete { key: MEDIA },
                SettingMutation::Delete { key: ENTITIES },
            ],
        )
        .await?;
    Ok(())
}

fn setting_failure_text(error: &SettingsWriteError) -> InputMessage {
    let message = match error {
        SettingsWriteError::CommitUncertain(_) => {
            "نتیجه ذخیره سازی نامشخص است؛ تا راه اندازی دوباره ربات، دوباره تلاش نکنید."
        }
        _ => "خوشامد ذخیره نشد؛ دوباره تلاش کنید.",
    };
    super::premium::icon_text(Some(super::premium::Icon::ErrorRed), message)
}

fn keep(entities: &[tl::enums::MessageEntity]) -> Vec<tl::enums::MessageEntity> {
    entities
        .iter()
        .filter(|entity| {
            !matches!(
                entity,
                tl::enums::MessageEntity::MentionName(_)
                    | tl::enums::MessageEntity::InputMessageEntityMentionName(_)
            )
        })
        .cloned()
        .collect()
}

fn rebase(
    entities: &[tl::enums::MessageEntity],
    shift: i32,
    length: i32,
) -> Vec<tl::enums::MessageEntity> {
    entities
        .iter()
        .filter_map(|entity| {
            let (offset, len) = bounds(entity)?;
            let moved = offset - shift;
            (moved >= 0 && moved + len <= length).then(|| at(entity, moved))
        })
        .collect()
}

fn bounds(entity: &tl::enums::MessageEntity) -> Option<(i32, i32)> {
    use tl::enums::MessageEntity as E;
    Some(match entity {
        E::Unknown(e) => (e.offset, e.length),
        E::Mention(e) => (e.offset, e.length),
        E::Hashtag(e) => (e.offset, e.length),
        E::BotCommand(e) => (e.offset, e.length),
        E::Url(e) => (e.offset, e.length),
        E::Email(e) => (e.offset, e.length),
        E::Bold(e) => (e.offset, e.length),
        E::Italic(e) => (e.offset, e.length),
        E::Code(e) => (e.offset, e.length),
        E::Pre(e) => (e.offset, e.length),
        E::TextUrl(e) => (e.offset, e.length),
        E::MentionName(e) => (e.offset, e.length),
        E::InputMessageEntityMentionName(e) => (e.offset, e.length),
        E::Phone(e) => (e.offset, e.length),
        E::Cashtag(e) => (e.offset, e.length),
        E::Underline(e) => (e.offset, e.length),
        E::Strike(e) => (e.offset, e.length),
        E::BankCard(e) => (e.offset, e.length),
        E::Spoiler(e) => (e.offset, e.length),
        E::CustomEmoji(e) => (e.offset, e.length),
        E::Blockquote(e) => (e.offset, e.length),
        _ => return None,
    })
}

fn placed(entity: &tl::enums::MessageEntity, offset: i32, length: i32) -> tl::enums::MessageEntity {
    use tl::enums::MessageEntity as E;
    let mut moved = entity.clone();
    match &mut moved {
        E::Unknown(e) => (e.offset, e.length) = (offset, length),
        E::Mention(e) => (e.offset, e.length) = (offset, length),
        E::Hashtag(e) => (e.offset, e.length) = (offset, length),
        E::BotCommand(e) => (e.offset, e.length) = (offset, length),
        E::Url(e) => (e.offset, e.length) = (offset, length),
        E::Email(e) => (e.offset, e.length) = (offset, length),
        E::Bold(e) => (e.offset, e.length) = (offset, length),
        E::Italic(e) => (e.offset, e.length) = (offset, length),
        E::Code(e) => (e.offset, e.length) = (offset, length),
        E::Pre(e) => (e.offset, e.length) = (offset, length),
        E::TextUrl(e) => (e.offset, e.length) = (offset, length),
        E::MentionName(e) => (e.offset, e.length) = (offset, length),
        E::InputMessageEntityMentionName(e) => (e.offset, e.length) = (offset, length),
        E::Phone(e) => (e.offset, e.length) = (offset, length),
        E::Cashtag(e) => (e.offset, e.length) = (offset, length),
        E::Underline(e) => (e.offset, e.length) = (offset, length),
        E::Strike(e) => (e.offset, e.length) = (offset, length),
        E::BankCard(e) => (e.offset, e.length) = (offset, length),
        E::Spoiler(e) => (e.offset, e.length) = (offset, length),
        E::CustomEmoji(e) => (e.offset, e.length) = (offset, length),
        E::Blockquote(e) => (e.offset, e.length) = (offset, length),
        _ => {}
    }
    moved
}

fn at(entity: &tl::enums::MessageEntity, offset: i32) -> tl::enums::MessageEntity {
    let length = bounds(entity).map(|(_, length)| length).unwrap_or(0);
    placed(entity, offset, length)
}

pub async fn on_join(ctx: &Ctx, message: &Message) -> bool {
    let joined = matches!(
        message.action(),
        Some(
            tl::enums::MessageAction::ChatAddUser(_)
                | tl::enums::MessageAction::ChatJoinedByLink(_)
        )
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
        return true;
    }
    for person in joined {
        if person.is_bot {
            continue;
        }
        send(
            ctx,
            message,
            chat,
            Some((person.id, person.name)),
            Some(person.peer),
        )
        .await;
    }
    true
}

fn template(ctx: &Ctx, chat: i64) -> Option<String> {
    let text = ctx.settings.value(chat, TEXT).unwrap_or_default();
    let media = ctx.settings.value(chat, MEDIA).unwrap_or_default();
    (!text.is_empty() || !media.is_empty()).then_some(text)
}

fn stored_entities(ctx: &Ctx, chat: i64) -> Option<Vec<tl::enums::MessageEntity>> {
    let stored = ctx
        .settings
        .value(chat, ENTITIES)
        .filter(|e| !e.is_empty())?;
    Vec::<tl::enums::MessageEntity>::from_bytes(&unhex(&stored)?).ok()
}

fn compose(
    ctx: &Ctx,
    message: &Message,
    chat: i64,
    text: &str,
    who: Option<(i64, String)>,
) -> Option<InputMessage> {
    let (id, name) = subject(message, &who);
    match stored_entities(ctx, chat) {
        Some(entities) => {
            let (filled, entities) = fill_entities(message, text, entities, id, &name);
            (!filled.trim().is_empty())
                .then(|| InputMessage::new().text(filled).fmt_entities(entities))
        }
        None => {
            let filled = fill(message, text, who);
            (!filled.trim().is_empty()).then(|| InputMessage::new().html(&filled))
        }
    }
}

async fn send(
    ctx: &Ctx,
    message: &Message,
    chat: i64,
    who: Option<(i64, String)>,
    receiver: Option<grammers_client::session::types::PeerRef>,
) {
    let Some(text) = template(ctx, chat) else {
        return;
    };
    let body = compose(ctx, message, chat, &text, who);

    if let Some(stored) = ctx.settings.value(chat, MEDIA).filter(|m| !m.is_empty())
        && let Some(media) = decode_media(&stored)
    {
        let input = body.clone().unwrap_or_default().media(media);
        match deliver(ctx, message, receiver, input).await {
            Ok(Some(sent)) => {
                expire(ctx, chat, sent);
                return;
            }
            Ok(None) => return,
            Err(e) => {
                if !e
                    .downcast_ref::<grammers_client::InvocationError>()
                    .is_some_and(reference_expired)
                {
                    eprintln!("welcome: {chat}: {e}");
                    return;
                }
                eprintln!("welcome: {chat}: stored media expired, keeping the text only");
                if let Err(error) = ctx.settings.try_set(chat, MEDIA, false).await {
                    ::log::warn!("welcome: could not remove expired media for {chat}: {error}");
                }
            }
        }
    }
    let Some(body) = body else {
        return;
    };
    if let Ok(Some(sent)) = deliver(ctx, message, receiver, body).await {
        expire(ctx, chat, sent);
    }
}

async fn deliver(
    ctx: &Ctx,
    message: &Message,
    receiver: Option<grammers_client::session::types::PeerRef>,
    body: InputMessage,
) -> Result<Option<i32>, Box<dyn std::error::Error + Send + Sync>> {
    let Some(receiver) = receiver else {
        return Ok(Some(message.respond(body).await?.id()));
    };
    Ok(crate::response::send_tracked_to_user(
        &ctx.client,
        &ctx.settings,
        message,
        receiver,
        ResponseKind::WelcomeNotice,
        crate::response::IntendedAudience::RequesterOnly,
        body,
    )
    .await?
    .group_message_id)
}

fn expire(ctx: &Ctx, chat: i64, sent: i32) {
    let seconds = ttl(ctx, chat);
    if seconds == 0 {
        return;
    }
    ctx.schedule_delete(
        chat,
        sent,
        std::time::Instant::now() + std::time::Duration::from_secs(u64::from(seconds)),
    );
}

fn subject(message: &Message, who: &Option<(i64, String)>) -> (i64, String) {
    match who {
        Some((id, name)) => (*id, name.clone()),
        None => (
            message
                .sender_id()
                .and_then(PeerId::bare_id)
                .unwrap_or_default(),
            super::name_of(message),
        ),
    }
}

fn username_of(message: &Message, id: i64, name: &str) -> String {
    message
        .sender()
        .filter(|_| who_is_sender(message, id))
        .and_then(|peer| peer.username().map(|u| format!("@{u}")))
        .unwrap_or_else(|| name.to_owned())
}

fn group_of(message: &Message) -> String {
    message
        .peer()
        .and_then(|peer| peer.name())
        .unwrap_or("گروه")
        .to_owned()
}

fn fill(message: &Message, template: &str, who: Option<(i64, String)>) -> String {
    let (id, name) = subject(message, &who);
    let username = username_of(message, id, &name);
    let group = group_of(message);

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

fn fill_entities(
    message: &Message,
    template: &str,
    carried: Vec<tl::enums::MessageEntity>,
    id: i64,
    name: &str,
) -> (String, Vec<tl::enums::MessageEntity>) {
    let username = username_of(message, id, name);
    let group = group_of(message);
    fill_walk(template, carried, id, name, &username, &group)
}

fn fill_walk(
    template: &str,
    carried: Vec<tl::enums::MessageEntity>,
    id: i64,
    name: &str,
    username: &str,
    group: &str,
) -> (String, Vec<tl::enums::MessageEntity>) {
    let ident = id.to_string();
    let tags: &[(&str, &str, bool)] = &[
        ("{نام}", name, false),
        ("{name}", name, false),
        ("{منشن}", name, true),
        ("{mention}", name, true),
        ("{آیدی}", &ident, false),
        ("{id}", &ident, false),
        ("{یوزرنیم}", username, false),
        ("{username}", username, false),
        ("{گروه}", group, false),
        ("{group}", group, false),
    ];

    let source_len = u16len(template) as usize;
    let mut map = vec![0i32; source_len + 1];
    let mut out = String::with_capacity(template.len());
    let mut mentions = Vec::new();
    let mut source = 0usize;
    let mut cursor = 0usize;

    while cursor < template.len() {
        let rest = &template[cursor..];
        let hit = tags
            .iter()
            .find(|(tag, _, _)| rest.starts_with(tag))
            .copied();
        match hit {
            Some((tag, value, mention)) => {
                let tag_units = u16len(tag) as usize;
                let value_units = u16len(value);
                let start = u16len(&out);
                map[source] = start;
                out.push_str(value);
                for step in 1..=tag_units {
                    map[source + step] = start + value_units;
                }
                if mention && value_units > 0 {
                    mentions.push(
                        tl::types::MessageEntityMentionName {
                            offset: start,
                            length: value_units,
                            user_id: id,
                        }
                        .into(),
                    );
                }
                source += tag_units;
                cursor += tag.len();
            }
            None => {
                let next = template[cursor..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1);
                let piece = &template[cursor..cursor + next];
                for step in 0..u16len(piece) as usize {
                    map[source + step] = u16len(&out) + step as i32;
                }
                out.push_str(piece);
                source += u16len(piece) as usize;
                cursor += next;
            }
        }
    }
    map[source_len] = u16len(&out);

    let mut moved: Vec<tl::enums::MessageEntity> = carried
        .iter()
        .filter_map(|entity| {
            let (offset, length) = bounds(entity)?;
            let (from, to) = (offset as usize, (offset + length) as usize);
            if to > source_len {
                return None;
            }
            let (start, end) = (map[from], map[to]);
            (end > start).then(|| placed(entity, start, end - start))
        })
        .collect();
    moved.extend(mentions);
    (out, moved)
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
         اگر پیام رسانه داشته باشد، همان رسانه با همان file id دوباره ارسال می شود.\n\
         قالب بندی و ایموجی پرمیوم همان پیام هم نگه داشته می شود.\n\n\
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

pub fn reference_expired(e: &grammers_client::InvocationError) -> bool {
    matches!(e, grammers_client::InvocationError::Rpc(rpc) if rpc.name.starts_with("FILE_REFERENCE"))
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
    text.len()
        .is_multiple_of(2)
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

    fn emoji(offset: i32, length: i32, document_id: i64) -> tl::enums::MessageEntity {
        tl::types::MessageEntityCustomEmoji {
            offset,
            length,
            document_id,
        }
        .into()
    }

    fn bold(offset: i32, length: i32) -> tl::enums::MessageEntity {
        tl::types::MessageEntityBold { offset, length }.into()
    }

    fn walk(
        template: &str,
        carried: Vec<tl::enums::MessageEntity>,
        id: i64,
        name: &str,
    ) -> (String, Vec<tl::enums::MessageEntity>) {
        fill_walk(template, carried, id, name, "@who", "گروه")
    }

    #[test]
    fn hex_roundtrips() {
        let bytes = vec![0u8, 1, 254, 255, 42];
        assert_eq!(unhex(&hex(&bytes)), Some(bytes));
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex("abc"), None);
    }

    #[test]
    fn entities_survive_the_settings_row() {
        let entities = vec![bold(0, 5), emoji(6, 2, 5_368_324_170_671_202_286)];
        let stored = hex(&entities.to_bytes());
        let back = Vec::<tl::enums::MessageEntity>::from_bytes(&unhex(&stored).unwrap()).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(bounds(&back[0]), Some((0, 5)));
        assert_eq!(bounds(&back[1]), Some((6, 2)));
        assert!(matches!(back[1], tl::enums::MessageEntity::CustomEmoji(_)));
    }

    #[test]
    fn rebasing_drops_what_leaves_the_slice() {
        let entities = vec![bold(0, 3), bold(4, 5), emoji(6, 2, 1)];
        let moved = rebase(&entities, 4, 5);
        assert_eq!(moved.len(), 2);
        assert_eq!(bounds(&moved[0]), Some((0, 5)));
        assert_eq!(bounds(&moved[1]), Some((2, 2)));
    }

    #[test]
    fn the_fill_walk_counts_in_utf16() {
        let template = "{name} 🎉";
        assert_eq!(u16len(template), 9);
        let carried = vec![emoji(7, 2, 99)];

        let (text, moved) = walk(template, carried.clone(), 7, "x");
        assert_eq!(text, "x 🎉");
        assert_eq!(bounds(&moved[0]), Some((2, 2)));

        let (text, moved) = walk(template, carried, 7, "🙂");
        assert_eq!(text, "🙂 🎉");
        assert_eq!(bounds(&moved[0]), Some((3, 2)));
    }

    #[test]
    fn the_mention_tag_becomes_an_entity() {
        let (text, moved) = walk("سلام {منشن}", Vec::new(), 4242, "علی");
        assert_eq!(text, "سلام علی");
        assert_eq!(moved.len(), 1);
        match &moved[0] {
            tl::enums::MessageEntity::MentionName(e) => {
                assert_eq!(e.user_id, 4242);
                assert_eq!((e.offset, e.length), (5, 3));
            }
            other => panic!("expected a mention, got {other:?}"),
        }
    }

    #[test]
    fn a_repeated_tag_shifts_each_time() {
        let carried = vec![bold(13, 1)];
        let (text, moved) = walk("{name}-{name}!", carried, 1, "ab");
        assert_eq!(text, "ab-ab!");
        assert_eq!(bounds(&moved[0]), Some((5, 1)));
    }

    #[test]
    fn an_entity_past_the_end_is_dropped() {
        let (_, moved) = walk("{name}", vec![bold(40, 3)], 1, "x");
        assert!(moved.is_empty());
    }

    #[test]
    fn a_stored_mention_is_not_kept() {
        let entities = vec![
            bold(0, 2),
            tl::types::MessageEntityMentionName {
                offset: 3,
                length: 4,
                user_id: 7,
            }
            .into(),
            emoji(8, 2, 3),
        ];
        let kept = keep(&entities);
        assert_eq!(kept.len(), 2);
        assert!(
            !kept
                .iter()
                .any(|e| matches!(e, tl::enums::MessageEntity::MentionName(_)))
        );
    }

    #[test]
    fn the_ttl_presets_sit_inside_the_range() {
        for preset in TTL_PRESETS {
            assert!(*preset >= TTL_RANGE.0 && *preset <= TTL_RANGE.1);
        }
    }

    #[test]
    fn the_installed_ttl_is_reachable_from_the_panel() {
        assert!(
            (TTL_RANGE.0..=TTL_RANGE.1).contains(&INSTALL_TTL),
            "INSTALL_TTL is outside the range and would be clamped"
        );
        assert!(
            TTL_PRESETS.contains(&INSTALL_TTL),
            "INSTALL_TTL is not a preset, so no button on the panel matches it"
        );
        const {
            assert!(
                INSTALL_TTL >= crate::DEFERRED_SWEEP_SECS,
                "the deferred sweep is slower than INSTALL_TTL, so it cannot be honoured on time"
            )
        };

        assert_eq!(
            DEFAULT_TTL, 0,
            "the absent-row default must stay harmless; write a row instead"
        );
        assert_ne!(
            DEFAULT_TTL, INSTALL_TTL,
            "if these are equal the written row is doing nothing and can be dropped"
        );
    }
}
