use std::time::Duration;

use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;

use super::{Ctx, esc};

pub const MODE: &str = "betrayal";

pub const LIMIT: &str = "betrayal_limit";

pub const WINDOW: &str = "betrayal_window";

pub const ACTION: &str = "betrayal_action";

pub const DEFAULT_LIMIT: u32 = 5;
pub const DEFAULT_WINDOW: u32 = 5;
pub const LIMIT_RANGE: (u32, u32) = (2, 30);
pub const WINDOW_RANGE: (u32, u32) = (1, 120);

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

pub async fn on_participant_update(ctx: &Ctx, update: &tl::types::UpdateChannelParticipant) {
    let Some(chat) = PeerId::channel(update.channel_id).and_then(|id| id.bot_api_dialog_id())
    else {
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

    let Some(count) = removal_punishes(ctx, chat, actor) else {
        return;
    };

    let Some(chat_ref) = ctx
        .chat_ref(chat)
        .or_else(|| PeerId::channel(update.channel_id).map(PeerId::to_ambient_ref))
    else {
        return;
    };
    punish(ctx, chat_ref, chat, actor, count).await;
}

fn removal_punishes(ctx: &Ctx, chat: i64, actor: i64) -> Option<usize> {
    let window = Duration::from_secs(u64::from(window(ctx, chat)) * 60);
    let count = ctx.record_removal(chat, actor, window);
    (count > limit(ctx, chat) as usize && ctx.claim_betrayal(chat, actor)).then_some(count)
}

async fn punish(ctx: &Ctx, chat_ref: PeerRef, chat: i64, actor: i64, count: usize) {
    let count = if count > LIMIT_RANGE.1 as usize {
        format!("{count}+")
    } else {
        count.to_string()
    };
    let Some((peer, name)) = super::admin_ref(ctx, chat_ref, actor).await else {
        eprintln!("betrayal: {chat}: no ref for admin {actor}");
        return;
    };

    if let Err(e) = super::restrict::set_member_admin_rights(
        ctx,
        chat_ref,
        peer,
        super::restrict::AdminRightsSpec::default(),
    )
    .await
    {
        eprintln!("betrayal: {chat}: could not demote {actor}: {e}");
        return;
    }
    ctx.forget_admins(chat);

    let banned = if bans(ctx, chat) {
        match super::restrict::kick_member(ctx, chat_ref, peer).await {
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
            super::premium::icon_html(Some(if banned { super::premium::Icon::ModerationHammer } else { super::premium::Icon::User }), format!(
                "<b>ضد خیانت ادمین</b>\n\n<b>{}</b> در مدت کوتاهی {count} نفر را حذف کرد و {what}.",
                esc(&name)
            )),
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "needs DATABASE_URL: a Ctx needs a settings mirror"]
    async fn one_burst_punishes_its_admin_once() {
        const CHAT: i64 = -100_777_000_123;
        let (ctx, _pool) = super::super::live_ctx().await;
        let punished: Vec<usize> = (0..30)
            .filter_map(|_| removal_punishes(&ctx, CHAT, 7))
            .collect();
        assert_eq!(punished, [DEFAULT_LIMIT as usize + 1]);
        let other = (0..30)
            .filter_map(|_| removal_punishes(&ctx, CHAT, 8))
            .count();
        assert_eq!(other, 1);
    }
}
