use std::time::{Duration, Instant};

use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::PeerRef;
use grammers_client::update::CallbackQuery;

use super::restrict::{self, Action};
use super::{Ctx, esc};

pub const MODE: &str = "captcha";

pub const TIMEOUT: &str = "captcha_timeout";

pub const ACTION: &str = "captcha_action";

const DEFAULT_TIMEOUT: u32 = 120;
pub const TIMEOUT_RANGE: (u32, u32) = (30, 900);
pub const TIMEOUT_PRESETS: &[u32] = &[60, 120, 300, 600];

const EMOJI: &[&str] = &[
    "🍎", "🚗", "⚽", "🌙", "🎈", "🐱", "🌷", "🔑", "⭐", "🍉", "🐟", "🍌", "🚀", "🎩", "🥁",
    "🦋", "🍇", "🐘", "☂️", "🍕", "🐝", "🎸", "🧊", "🕰️",
];

pub const CHOICES: &str = "captcha_choices";
const DEFAULT_CHOICES: u32 = 3;
pub const CHOICES_RANGE: (u32, u32) = (2, 6);
pub const CHOICES_PRESETS: &[u32] = &[2, 3, 4, 5, 6];

const GLOBAL: i64 = 0;

pub fn choices(ctx: &Ctx, chat: i64) -> usize {
    ctx.settings
        .value(chat, CHOICES)
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_CHOICES)
        .clamp(CHOICES_RANGE.0, CHOICES_RANGE.1) as usize
}

#[derive(Clone)]
pub struct Pending {
    pub answer: usize,
    pub message_id: i32,
    pub started: Instant,
}

pub fn timeout(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value(chat, TIMEOUT)
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT)
        .clamp(TIMEOUT_RANGE.0, TIMEOUT_RANGE.1)
}

pub fn kicks(ctx: &Ctx, chat: i64) -> bool {
    ctx.settings.value(chat, ACTION).as_deref() != Some("mute")
}

pub async fn set_timeout(ctx: &Ctx, chat: i64, value: u32) {
    let value = value.clamp(TIMEOUT_RANGE.0, TIMEOUT_RANGE.1);
    ctx.settings
        .set_value(chat, TIMEOUT, &value.to_string())
        .await;
}

pub async fn on_join(ctx: &std::sync::Arc<Ctx>, message: &Message) -> bool {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !ctx.settings.is_locked(chat, MODE) {
        return false;
    }
    let joined = matches!(
        message.action(),
        Some(
            grammers_client::tl::enums::MessageAction::ChatAddUser(_)
                | grammers_client::tl::enums::MessageAction::ChatJoinedByLink(_)
        )
    );
    if !joined {
        return false;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    let mut challenged = false;
    for joined in super::joined_users(ctx, message).await {
        if joined.is_bot || super::is_bot_admin(ctx, chat, joined.id) || super::owner(ctx, chat) == Some(joined.id) {
            continue;
        }
        if challenge(ctx, message, chat, chat_ref, joined).await {
            challenged = true;
        }
    }
    challenged
}

async fn challenge(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
    chat: i64,
    chat_ref: PeerRef,
    joined: super::Joined,
) -> bool {
    let (user, target) = (joined.id, joined.peer);

    if let Err(e) = restrict::apply(ctx, chat_ref, target, Action::Mute, None, restrict::By { reason: "احراز هویت", target_name: &joined.name, ..Default::default() }).await {
        eprintln!("captcha: {chat}: could not mute {user}: {e}");
        return false;
    }

    let (answer, offered) = pick(chat, user, choices(ctx, chat));
    let seconds = timeout(ctx, chat);
    let caption = format!(
        "<b>احراز هویت</b>\n\n\
         <a href=\"tg://user?id={user}\">{}</a> همان ایموجی که در تصویر است را بزنید.\n\
         <i>{seconds} ثانیه فرصت دارید.</i>",
        esc(&joined.name),
    );
    let markup = buttons(user, &offered);

    let mut input = InputMessage::new().html(&caption).reply_markup(markup);
    let cached = ctx
        .settings
        .value(GLOBAL, &photo_key(answer))
        .filter(|value| !value.is_empty())
        .and_then(|value| super::welcome::decode_media(&value));
    let fresh = cached.is_none();
    match cached {
        Some(media) => input = input.media(media),

        None => {
            if let Some(uploaded) = upload(ctx, answer).await {
                input = input.photo(uploaded);
            }
        }
    }

    let Ok(sent) = message.reply(input).await else {
        return false;
    };
    if fresh
        && let Some(media) = sent.media()
        && let Some(encoded) = super::welcome::encode_media(&media)
    {
        ctx.settings
            .set_value(GLOBAL, &photo_key(answer), &encoded)
            .await;
    }

    ctx.captcha_start(
        chat,
        user,
        Pending {
            answer,
            message_id: sent.id(),
            started: Instant::now(),
        },
    );

    let ctx_clone = std::sync::Arc::clone(ctx);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(u64::from(seconds))).await;
        expire(&ctx_clone, chat_ref, chat, user, target).await;
    });
    true
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, chat: i64) {
    let Some((user, index)) = payload.split_once(':') else {
        return;
    };
    let (Ok(user), Ok(index)) = (user.parse::<i64>(), index.parse::<usize>()) else {
        return;
    };
    if query.sender_id().bare_id() != Some(user) {
        let _ = query
            .answer()
            .alert("این آزمون برای شما نیست.")
            .send()
            .await;
        return;
    }
    let Some(pending) = ctx.captcha_pending(chat, user) else {
        let _ = query.answer().alert("این آزمون منقضی شده است.").send().await;
        return;
    };

    if index != pending.answer {
        let _ = query
            .answer()
            .alert("درست نبود. دوباره تلاش کنید.")
            .send()
            .await;
        return;
    }

    ctx.captcha_done(chat, user);
    ctx.bump(chat, super::stats::CAPTCHA_PASSED);
    let (Ok(Some(chat_ref)), Ok(Some(target))) = (query.peer_ref().await, query.sender_ref().await)
    else {
        return;
    };
    if let Err(e) = restrict::apply(ctx, chat_ref, target, Action::Unmute, None, restrict::By { reason: "احراز هویت", ..Default::default() }).await {
        eprintln!("captcha: {chat}: could not unmute {user}: {e}");
    }
    let _ = query
        .answer()
        .edit(InputMessage::new().html(format!(
            "<b>احراز هویت</b>\n\n✓ <a href=\"tg://user?id={user}\">کاربر</a> تایید شد. خوش آمدید."
        )))
        .await;
}

async fn expire(ctx: &Ctx, chat_ref: PeerRef, chat: i64, user: i64, target: PeerRef) {
    let Some(pending) = ctx.captcha_pending(chat, user) else {
        return;
    };
    ctx.captcha_done(chat, user);
    ctx.bump(chat, super::stats::CAPTCHA_FAILED);

    if kicks(ctx, chat)
        && let Err(e) = ctx.client.kick_participant(chat_ref, target).await
    {
        eprintln!("captcha: {chat}: could not kick {user}: {e}");
    }
    let _ = ctx
        .client
        .delete_messages(chat_ref, &[pending.message_id])
        .await;
}

fn photo_key(index: usize) -> String {
    format!("emoji_photo:{index}")
}

async fn upload(ctx: &Ctx, index: usize) -> Option<grammers_client::media::Uploaded> {
    let png = super::emoji_image::render(EMOJI[index])?;
    let size = png.len();
    let mut reader = std::io::Cursor::new(png);
    ctx.client
        .upload_stream(&mut reader, size, format!("captcha{index}.png"))
        .await
        .ok()
}

fn pick(chat: i64, user: i64, count: usize) -> (usize, Vec<usize>) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    (chat, user).hash(&mut hasher);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    let seed = hasher.finish();

    let count = count.min(EMOJI.len());
    let mut choices = Vec::with_capacity(count);
    let mut step = seed;
    while choices.len() < count {
        step = step.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let candidate = (step >> 33) as usize % EMOJI.len();
        if !choices.contains(&candidate) {
            choices.push(candidate);
        }
    }
    let answer = choices[(seed as usize) % count];
    (answer, choices)
}

fn buttons(user: i64, choices: &[usize]) -> ReplyMarkup {
    ReplyMarkup::from_buttons(&[choices
        .iter()
        .map(|&index| Button::data(EMOJI[index], format!("c:{user}:{index}").into_bytes()))
        .collect()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_are_distinct_and_contain_the_answer() {
        for user in 1..50 {
            let count = 2 + (user as usize % 5);
            let (answer, choices) = pick(-100, user, count);
            assert_eq!(choices.len(), count);
            let mut seen = choices.clone();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), count, "duplicate choice offered");
            assert!(choices.contains(&answer), "answer is not among the choices");
        }
    }
}
