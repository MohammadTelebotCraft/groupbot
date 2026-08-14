use std::time::Duration;

use grammers_client::message::Message;

use super::restrict::{self, Action};
use super::{Ctx, esc, name_of};

pub const MODE: &str = "strict";

pub const ACTION: &str = "strict_action";

pub const LIMIT: &str = "strict_limit";

pub const TIME: &str = "strict_time";

const DEFAULT_LIMIT: u32 = 1;
pub const LIMIT_RANGE: (u32, u32) = (1, 20);
pub const TIME_RANGE: (u32, u32) = (0, 10_080);

pub const LIMIT_PRESETS: &[u32] = &[1, 2, 3, 5, 10];

pub const TIME_PRESETS: &[u32] = &[0, 5, 60, 720, 1440];

const TALLY_DAYS: u64 = 7;

pub const PICK: &str = "strict:";

pub const FILTER: &str = "filter";

pub const PACK: &str = "pack";

pub fn pick_key(cause: &str) -> String {
    format!("{PICK}{cause}")
}

pub async fn sync_pick(ctx: &Ctx, chat: i64, lock: &str, on: bool) {
    if ctx.settings.indexed_empty(chat, PICK) {
        return;
    }
    ctx.settings.set(chat, &pick_key(lock), on).await;
}

pub fn counts(ctx: &Ctx, chat: i64, cause: &str) -> bool {
    ctx.settings.indexed_empty(chat, PICK) || ctx.settings.is_locked(chat, &pick_key(cause))
}

pub fn action_of(ctx: &Ctx, chat: i64) -> Action {
    match ctx.settings.value(chat, ACTION).as_deref() {
        Some("ban") => Action::Ban,
        _ => Action::Mute,
    }
}

pub fn is_ban(ctx: &Ctx, chat: i64) -> bool {
    action_of(ctx, chat) == Action::Ban
}

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value_parsed(chat, LIMIT)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(LIMIT_RANGE.0, LIMIT_RANGE.1)
}

pub fn minutes(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value_parsed(chat, TIME)
        .unwrap_or(0)
        .clamp(TIME_RANGE.0, TIME_RANGE.1)
}

pub fn duration(ctx: &Ctx, chat: i64) -> Option<Duration> {
    let minutes = minutes(ctx, chat);
    (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60))
}

pub fn time_label(minutes: u32) -> String {
    if minutes == 0 {
        "دائمی".to_owned()
    } else {
        super::log::duration_label(u64::from(minutes) * 60)
    }
}

pub async fn punish(ctx: &Ctx, message: &Message, chat: i64, cause: &str) -> Option<u32> {
    if !ctx.settings.is_locked(chat, MODE) || !counts(ctx, chat, cause) {
        return None;
    }
    let (Ok(Some(chat_ref)), Ok(Some(target))) = (message.peer_ref().await, message.sender_ref().await)
    else {
        return None;
    };
    let user = target.id.bare_id()?;

    let limit = limit(ctx, chat);
    if limit > 1 {
        let count = ctx
            .settings
            .add_strike(chat, user, super::stats::today(), TALLY_DAYS)
            .await;
        if count < limit {
            return Some(limit - count);
        }
    }

    let action = action_of(ctx, chat);
    let duration = duration(ctx, chat);
    if let Err(e) = restrict::apply(ctx, chat_ref, target, action, duration, super::restrict::By { reason: "حالت سختگیرانه", target_name: &super::name_of(message), ..Default::default() }).await {
        eprintln!("strict mode: {chat}: could not restrict sender: {e}");
        return None;
    }
    ctx.settings.clear_strikes(chat, user).await;

    let what = if action == Action::Ban {
        "از گروه اخراج شد"
    } else {
        "سکوت شد"
    };
    let how_many = if limit > 1 {
        format!(" پس از {limit} تخلف")
    } else {
        String::new()
    };
    let how_long = match duration {
        Some(duration) => format!(" به مدت {}", super::log::duration_label(duration.as_secs())),
        None => String::new(),
    };
    let _ = ctx
        .client
        .send_message(
            chat_ref,
            grammers_client::message::InputMessage::new().html(format!(
                "<b>{}</b> به دلیل ارسال مورد قفل شده{how_many} {what}{how_long}.",
                esc(&name_of(message))
            )),
        )
        .await;
    None
}

pub fn chances_line(chances: u32) -> String {
    format!("<i>{chances} فرصت دیگر دارید.</i>")
}
