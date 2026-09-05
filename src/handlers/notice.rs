use std::time::{Duration, Instant};

use grammers_client::message::{InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::PeerId;

use super::{Ctx, esc, name_of};

pub const MODE: &str = "notice";

pub const TTL: &str = "notice_ttl";

const DEFAULT_TTL: u32 = 15;
pub const TTL_RANGE: (u32, u32) = (0, 300);
pub const TTL_PRESETS: &[u32] = &[0, 10, 15, 30, 60];

pub fn ttl(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value(chat, TTL)
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_TTL)
        .clamp(TTL_RANGE.0, TTL_RANGE.1)
}

pub async fn set_ttl(ctx: &Ctx, chat: i64, value: u32) {
    let value = value.clamp(TTL_RANGE.0, TTL_RANGE.1);
    ctx.settings.set_value(chat, TTL, &value.to_string()).await;
}

pub async fn send(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
    chat: i64,
    reason: &str,
    chances: Option<u32>,
) {
    send_with_markup(ctx, message, chat, reason, chances, None).await;
}

pub async fn send_with_markup(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
    chat: i64,
    reason: &str,
    chances: Option<u32>,
    markup: Option<ReplyMarkup>,
) {
    if !ctx.settings.is_locked(chat, MODE) {
        return;
    }
    let Some(user) = message.sender_id().and_then(PeerId::bare_id) else {
        return;
    };
    if !ctx.may_notify_lock(chat, user) {
        return;
    }

    let tail = match chances {
        Some(chances) => super::strict::chances_line(chances),
        None => "<i>لطفا دوباره نفرستید.</i>".to_owned(),
    };
    let mut card = InputMessage::new().html(format!(
            "<a href=\"tg://user?id={user}\">{}</a> پیام شما حذف شد · <b>{}</b> در این گروه قفل است.\n{tail}",
            esc(&name_of(message)),
            esc(reason)
        ));
    if let Some(markup) = markup {
        card = card.reply_markup(markup);
    }
    let sent = message.respond(card).await;

    let Ok(sent) = sent else {
        return;
    };
    let seconds = ttl(ctx, chat);
    if seconds == 0 {
        return;
    }

    let id = sent.id();
    ctx.schedule_delete(
        chat,
        id,
        Instant::now() + Duration::from_secs(u64::from(seconds)),
    );
}
