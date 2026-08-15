use std::time::Duration;

use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;

use super::restrict::{self, Action};
use super::{Ctx, esc};

pub const MODE: &str = "raid";

pub const LIMIT: &str = "raid_limit";

pub const WINDOW: &str = "raid_window";

pub const TIME: &str = "raid_time";

pub const LIMIT_RANGE: (u32, u32) = (2, 200);
pub const WINDOW_RANGE: (u32, u32) = (5, 600);
pub const TIME_RANGE: (u32, u32) = (1, 10_080);
const DEFAULT_LIMIT: u32 = 10;
const DEFAULT_WINDOW: u32 = 30;
const DEFAULT_TIME: u32 = 60;

pub const LIMIT_PRESETS: &[u32] = &[5, 10, 20, 30, 50];
pub const WINDOW_PRESETS: &[u32] = &[10, 30, 60, 120, 300];
pub const TIME_PRESETS: &[u32] = &[10, 60, 720, 1440];

const EVERY_JOINER: i64 = 0;

struct Newcomer {
    id: i64,
    peer: PeerRef,
    name: String,
}

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    number(ctx, chat, LIMIT, DEFAULT_LIMIT, LIMIT_RANGE)
}

pub fn window(ctx: &Ctx, chat: i64) -> u32 {
    number(ctx, chat, WINDOW, DEFAULT_WINDOW, WINDOW_RANGE)
}

pub fn minutes(ctx: &Ctx, chat: i64) -> u32 {
    number(ctx, chat, TIME, DEFAULT_TIME, TIME_RANGE)
}

fn number(ctx: &Ctx, chat: i64, key: &str, default: u32, range: (u32, u32)) -> u32 {
    ctx.settings
        .value_parsed(chat, key)
        .unwrap_or(default)
        .clamp(range.0, range.1)
}

pub async fn check(ctx: &std::sync::Arc<Ctx>, message: &Message, chat: i64) {
    if !ctx.settings.is_locked(chat, MODE) {
        return;
    }
    let joined = super::joined_users(ctx, message).await;
    if joined.is_empty() {
        return;
    }
    if super::can_manage(ctx, message).await {
        return;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return;
    };

    surge(
        ctx,
        chat,
        chat_ref,
        joined
            .into_iter()
            .map(|member| Newcomer {
                id: member.id,
                peer: member.peer,
                name: member.name,
            })
            .collect(),
    )
    .await;
}

pub async fn on_participant_update(ctx: &Ctx, update: &tl::types::UpdateChannelParticipant) {
    use tl::enums::ChannelParticipant as P;

    let Some(chat) = PeerId::channel(update.channel_id).and_then(PeerId::bot_api_dialog_id) else {
        return;
    };
    if !ctx.settings.is_locked(chat, MODE) {
        return;
    }

    let inside = |participant: &Option<P>| {
        matches!(
            participant,
            Some(P::Participant(_) | P::ParticipantSelf(_) | P::Admin(_) | P::Creator(_))
        )
    };
    if !inside(&update.new_participant) || inside(&update.prev_participant) {
        return;
    }

    let (chat, user, actor) = (chat, update.user_id, update.actor_id);
    let Some(chat_ref) = ctx
        .chat_ref(chat)
        .or_else(|| PeerId::channel(update.channel_id).map(PeerId::to_ambient_ref))
    else {
        return;
    };

    if actor != user && super::is_admin(ctx, chat_ref, chat, actor).await {
        return;
    }
    let Some(peer) = PeerId::user(user).map(PeerId::to_ambient_ref) else {
        return;
    };

    surge(
        ctx,
        chat,
        chat_ref,
        vec![Newcomer {
            id: user,
            peer,
            name: user.to_string(),
        }],
    )
    .await;
}

async fn surge(ctx: &Ctx, chat: i64, chat_ref: PeerRef, arrivals: Vec<Newcomer>) {
    let window = Duration::from_secs(u64::from(window(ctx, chat)));

    let mut count = 0;
    let mut fresh = Vec::new();
    for member in arrivals {
        if !ctx.first_sighting(chat, member.id, window) {
            continue;
        }
        count = ctx.record_message(chat, EVERY_JOINER, window);
        fresh.push(member);
    }
    if fresh.is_empty() || count <= limit(ctx, chat) as usize {
        return;
    }

    let held = Duration::from_secs(u64::from(minutes(ctx, chat)) * 60);
    let mut muted = 0;
    for member in &fresh {
        match restrict::apply(ctx, chat_ref, member.peer, Action::Mute, Some(held), restrict::By { reason: "ضد هجوم", target_name: &member.name, ..Default::default() }).await {
            Ok(()) => muted += 1,
            Err(e) => eprintln!("raid: {chat}: could not mute {}: {e}", member.id),
        }
    }
    if !ctx.may_notify(chat, EVERY_JOINER) {
        return;
    }

    let told = if muted == 0 {
        "<b>ضد هجوم</b>\n\nورود ناگهانی اعضا · ربات نتوانست کسی را سکوت کند.\n\
         <i>مطمئن شوید ربات ادمین است و اجازه محدود کردن کاربران دارد.</i>"
            .to_owned()
    } else {
        format!(
            "<b>ضد هجوم</b>\n\n\
             ورود ناگهانی {count} عضو در {} ثانیه · <b>{muted}</b> نفر تا {} سکوت شدند.\n\
             <i>ادمین می تواند با «رفع سکوت» آزادشان کند.</i>",
            window.as_secs(),
            esc(&super::log::duration_label(held.as_secs()))
        )
    };
    let _ = ctx
        .client
        .send_message(chat_ref, InputMessage::new().html(told))
        .await;
}
