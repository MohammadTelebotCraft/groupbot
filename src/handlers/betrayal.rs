use std::time::Duration;

use grammers_client::message::InputMessage;
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;

use super::{Ctx, esc};

pub const MODE: &str = "betrayal";

pub const LIMIT: &str = "betrayal_limit";

pub const WINDOW: &str = "betrayal_window";

pub const ACTION: &str = "betrayal_action";

pub const DEFAULT_LIMIT: u32 = 5;
pub const DEFAULT_WINDOW: u32 = 5;
const LIMIT_RANGE: (u32, u32) = (2, 30);
const WINDOW_RANGE: (u32, u32) = (1, 120);

pub const LIMIT_PRESETS: &[u32] = &[3, 5, 8, 12, 20];
pub const WINDOW_PRESETS: &[u32] = &[1, 5, 10, 30, 60];

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    number(ctx, chat, LIMIT, DEFAULT_LIMIT, LIMIT_RANGE)
}

pub fn window(ctx: &Ctx, chat: i64) -> u32 {
    number(ctx, chat, WINDOW, DEFAULT_WINDOW, WINDOW_RANGE)
}

pub fn bans(ctx: &Ctx, chat: i64) -> bool {
    ctx.settings.value(chat, ACTION).as_deref() == Some("ban")
}

fn number(ctx: &Ctx, chat: i64, key: &str, default: u32, range: (u32, u32)) -> u32 {
    ctx.settings
        .value(chat, key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
        .clamp(range.0, range.1)
}

pub async fn set(ctx: &Ctx, chat: i64, key: &str, value: u32) {
    let range = if key == LIMIT { LIMIT_RANGE } else { WINDOW_RANGE };
    let value = value.clamp(range.0, range.1);
    ctx.settings.set_value(chat, key, &value.to_string()).await;
}

pub async fn on_participant_update(ctx: &Ctx, update: &tl::types::UpdateChannelParticipant) {
    let Some(chat) = PeerId::channel(update.channel_id).and_then(|id| id.bot_api_dialog_id()) else {
        return;
    };
    if !ctx.settings.is_locked(chat, MODE) {
        return;
    }

    let removed = matches!(
        update.new_participant,
        Some(tl::enums::ChannelParticipant::Banned(_)) | None
    );
    let actor = update.actor_id;
    if !removed || actor == update.user_id {
        return;
    }

    if super::owner(ctx, chat) == Some(actor) {
        return;
    }

    let window = Duration::from_secs(u64::from(window(ctx, chat)) * 60);
    let count = ctx.record_removal(chat, actor, window);
    if count <= limit(ctx, chat) as usize {
        return;
    }

    let Some(chat_ref) = ctx.chat_ref(chat).or_else(|| {
        PeerId::channel(update.channel_id).map(PeerId::to_ambient_ref)
    }) else {
        return;
    };
    punish(ctx, chat_ref, chat, actor, count).await;
}

async fn punish(ctx: &Ctx, chat_ref: PeerRef, chat: i64, actor: i64, count: usize) {
    let Some((peer, name)) = super::admin_ref(ctx, chat_ref, actor).await else {
        eprintln!("betrayal: {chat}: no ref for admin {actor}");
        return;
    };

    if let Err(e) = ctx.client.set_admin_rights(chat_ref, peer).await {
        eprintln!("betrayal: {chat}: could not demote {actor}: {e}");
        return;
    }
    ctx.forget_admins(chat);

    let banned = if bans(ctx, chat) {
        match ctx.client.kick_participant(chat_ref, peer).await {
            Ok(()) => true,
            Err(e) => {
                eprintln!("betrayal: {chat}: could not ban {actor}: {e}");
                false
            }
        }
    } else {
        false
    };

    let what = if banned {
        "عزل و از گروه اخراج شد"
    } else {
        "عزل شد"
    };
    let _ = ctx
        .client
        .send_message(
            chat_ref,
            InputMessage::new().html(format!(
                "<b>ضد خیانت ادمین</b>\n\n<b>{}</b> در مدت کوتاهی {count} نفر را حذف کرد و {what}.",
                esc(&name)
            )),
        )
        .await;
}
