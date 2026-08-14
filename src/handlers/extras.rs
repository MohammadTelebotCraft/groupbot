use grammers_client::message::{InputMessage, Message};
use grammers_client::tl;

use super::{Ctx, esc, name_of};

pub const RULES: &str = "rules";

pub const NOTE: &str = "note:";

pub const NIGHT: &str = "night";
pub const NIGHT_STATE: &str = "night_state";

const SHOW_RULES: &[&str] = &["قوانین", "قانون"];
const SET_RULES: &[&str] = &["تنظیم قوانین", "تنظیم قانون"];
const NOTE_CMD: &[&str] = &["یادداشت"];
const NOTE_CLEAR: &[&str] = &["حذف یادداشت"];
const PIN: &[&str] = &["سنجاق", "پین"];
const PIN_QUIET: &[&str] = &["سنجاق بی صدا", "پین بی صدا"];
const UNPIN: &[&str] = &["حذف سنجاق", "حذف پین", "برداشتن سنجاق"];
const SLOW: &[&str] = &["اسلوموشن", "اسلومود", "کندی"];
const NIGHT_CMD: &[&str] = &["قفل شب"];
const TAG_ALL: &[&str] = &["تگ همه", "منشن همه", "فراخوان", "تگ", "منشن"];

const TAG_PER_MESSAGE: usize = 5;

const TAG_DEFAULT: usize = 50;
const TAG_MAX: usize = 200;

pub const SLOW_STEPS: &[u32] = &[0, 10, 30, 60, 300, 900, 3600];

pub const SLOW_STATE: &str = "slow_state";

pub fn slow_label(seconds: u32) -> String {
    match seconds {
        0 => "خاموش".to_owned(),
        s if s < 60 => format!("{s} ثانیه"),
        s if s < 3600 => format!("{} دقیقه", s / 60),
        s => format!("{} ساعت", s / 3600),
    }
}

pub async fn apply_slow(ctx: &Ctx, chat: i64, seconds: u32) -> bool {
    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return false;
    };
    let seconds = SLOW_STEPS
        .iter()
        .rev()
        .find(|step| **step <= seconds)
        .copied()
        .unwrap_or(0);
    match ctx
        .client
        .invoke(&tl::functions::channels::ToggleSlowMode {
            channel: chat_ref.into(),
            seconds: seconds as i32,
        })
        .await
    {
        Ok(_) => {
            ctx.settings
                .set_value(chat, SLOW_STATE, &seconds.to_string())
                .await;
            true
        }
        Err(e) => {
            eprintln!("slow mode: {chat}: {e}");
            false
        }
    }
}

pub fn rules(ctx: &Ctx, chat: i64) -> Option<String> {
    ctx.settings.value(chat, RULES).filter(|r| !r.is_empty())
}

pub fn note(ctx: &Ctx, chat: i64, user: i64) -> Option<String> {
    ctx.settings
        .value(chat, &format!("{NOTE}{user}"))
        .filter(|n| !n.is_empty())
}

pub fn night(ctx: &Ctx, chat: i64) -> Option<(u32, u32)> {
    let value = ctx.settings.value(chat, NIGHT)?;
    let (from, to) = value.split_once('|')?;
    Some((from.parse().ok()?, to.parse().ok()?))
}

pub async fn set_night(ctx: &Ctx, chat: i64, window: Option<(u32, u32)>) {
    match window {
        Some((from, to)) => {
            ctx.settings
                .set_value(chat, NIGHT, &format!("{}|{}", from % 1440, to % 1440))
                .await
        }
        None => ctx.settings.set_value(chat, NIGHT, "").await,
    }
}

pub fn clock(minutes: u32) -> String {
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = super::digits(message.text().trim());
    let text = text.as_ref();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if SHOW_RULES.contains(&text) {
        let _ = match rules(ctx, chat) {
            Some(rules) => {
                message
                    .reply(InputMessage::new().html(format!("<b>قوانین گروه</b>\n\n{rules}")))
                    .await
            }
            None => message.reply("قوانینی ثبت نشده است.").await,
        };
        return true;
    }

    let admin = |cap| super::limits::allows(ctx, message, cap);

    if let Some(rest) = after(text, TAG_ALL) {
        let matched = TAG_ALL
            .iter()
            .find(|command| text.starts_with(**command))
            .copied()
            .unwrap_or_default();
        let wanted = match super::numbers_in(rest).as_deref() {
            Some([]) if !super::phrase_carries_text(matched) => return false,
            Some([]) => TAG_DEFAULT,
            Some([count]) => *count as usize,
            _ => return false,
        };
        if !admin(super::limits::SET).await {
            return true;
        }
        tag_all(ctx, message, wanted).await;
        return true;
    }

    if let Some(rest) = after(text, SET_RULES) {
        if !admin(super::limits::SET).await {
            return true;
        }
        let body = if rest.is_empty() {
            message
                .get_reply()
                .await
                .ok()
                .flatten()
                .map(|replied| replied.text().to_owned())
                .unwrap_or_default()
        } else {
            rest.to_owned()
        };
        if body.is_empty() {
            let _ = message
                .reply("متن قوانین را بنویسید یا روی آن ریپلای کنید.")
                .await;
            return true;
        }
        ctx.settings.set_value(chat, RULES, &body).await;
        let _ = message.reply("✓ قوانین ذخیره شد.").await;
        return true;
    }

    if let Some(rest) = after(text, NOTE_CLEAR) {
        let Some(named) = super::named(message, none_if_empty(rest)) else {
            return false;
        };
        if !admin(super::limits::SET).await {
            return true;
        }
        let Some((target, name)) = super::resolve(ctx, message, named).await else {
            return true;
        };
        if let Some(user) = target.id.bare_id() {
            ctx.settings.set_value(chat, &format!("{NOTE}{user}"), "").await;
        }
        let _ = message.reply(format!("✗ یادداشت {name} حذف شد.")).await;
        return true;
    }

    if let Some(rest) = after(text, NOTE_CMD) {
        let Some(named) = super::named(message, None) else {
            return false;
        };
        if !admin(super::limits::SET).await {
            return true;
        }
        let Some((target, name)) = super::resolve(ctx, message, named).await else {
            let _ = message.reply("روی پیام کاربر ریپلای کنید.").await;
            return true;
        };
        let Some(user) = target.id.bare_id() else {
            return true;
        };
        if rest.is_empty() {
            let _ = match note(ctx, chat, user) {
                Some(note) => {
                    message
                        .reply(InputMessage::new().html(format!(
                            "<b>یادداشت {}</b>\n\n{}",
                            esc(&name),
                            esc(&note)
                        )))
                        .await
                }
                None => message.reply("یادداشتی ثبت نشده است.").await,
            };
            return true;
        }
        ctx.settings
            .set_value(chat, &format!("{NOTE}{user}"), rest)
            .await;
        let _ = message
            .reply(format!("✓ یادداشت برای {name} ذخیره شد."))
            .await;
        return true;
    }

    if PIN.contains(&text) || PIN_QUIET.contains(&text) || UNPIN.contains(&text) {
        if message.reply_to_message_id().is_none() {
            return false;
        }
        if !admin(super::limits::PIN).await {
            return true;
        }
        return pin(ctx, message, chat, UNPIN.contains(&text), PIN_QUIET.contains(&text)).await;
    }

    if let Some(rest) = after(text, SLOW) {
        let asked = match super::numbers_in(rest).as_deref() {
            Some(&[asked]) => asked,
            _ => return false,
        };
        if !admin(super::limits::SET).await {
            return true;
        }
        return slow_mode(ctx, message, chat, asked).await;
    }

    if let Some(rest) = after(text, NIGHT_CMD) {
        if !admin(super::limits::SET).await {
            return true;
        }
        return set_night_from(ctx, message, chat, rest).await;
    }
    false
}

async fn set_night_from(ctx: &Ctx, message: &Message, chat: i64, rest: &str) -> bool {
    if rest.is_empty() {
        return false;
    }
    if rest.starts_with("خاموش") {
        set_night(ctx, chat, None).await;
        let _ = message.reply("✗ قفل شب خاموش شد.").await;
        return true;
    }

    const SEPARATOR: &str = "تا";
    let mut times: Vec<u32> = Vec::new();
    for word in super::digits(rest).split_whitespace() {
        if word == SEPARATOR {
            continue;
        }
        let Some(minutes) = clock_at(word) else {
            return false;
        };
        times.push(minutes);
    }
    let [from, to] = times[..] else {
        let _ = message.reply("مثال: «قفل شب 23 تا 7» یا «قفل شب 23:30 تا 7:15»").await;
        return true;
    };
    let times = [from, to];
    set_night(ctx, chat, Some((times[0], times[1]))).await;
    let _ = message
        .reply(format!(
            "✓ قفل شب از {} تا {} (به وقت تهران).",
            clock(times[0]),
            clock(times[1])
        ))
        .await;
    true
}

async fn slow_mode(ctx: &Ctx, message: &Message, chat: i64, asked: u32) -> bool {
    let done = apply_slow(ctx, chat, asked).await;
    let now = ctx
        .settings
        .value(chat, SLOW_STATE)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let _ = match (done, now) {
        (true, 0) => message.reply("✗ اسلوموشن خاموش شد.").await,
        (true, _) => {
            message
                .reply(format!("✓ اسلوموشن روی {} تنظیم شد.", slow_label(now)))
                .await
        }
        (false, _) => {
            message
                .reply("انجام نشد. مطمئن شوید ربات اجازه تغییر اطلاعات گروه دارد.")
                .await
        }
    };
    true
}

async fn pin(ctx: &Ctx, message: &Message, chat: i64, unpin: bool, quiet: bool) -> bool {
    let (Ok(Some(replied)), Ok(Some(chat_ref))) =
        (message.get_reply().await, message.peer_ref().await)
    else {
        let _ = message.reply("روی پیام موردنظر ریپلای کنید.").await;
        return true;
    };
    let result = ctx
        .client
        .invoke(&tl::functions::messages::UpdatePinnedMessage {
            silent: quiet,
            unpin,
            pm_oneside: false,
            peer: chat_ref.into(),
            id: replied.id(),
        })
        .await;
    let _ = match result {
        Ok(_) if unpin => message.reply("✗ سنجاق برداشته شد.").await,
        Ok(_) => {
            message
                .reply(format!("✓ پیام سنجاق شد. توسط {}", name_of(message)))
                .await
        }
        Err(e) => {
            eprintln!("pin: {chat}: {e}");
            message
                .reply("انجام نشد. مطمئن شوید ربات اجازه سنجاق کردن دارد.")
                .await
        }
    };
    true
}

pub async fn run_night(ctx: &Ctx) {
    let now = (super::stats::local_seconds() % 86_400) / 60;

    for chat in ctx.settings.chats() {
        let Some((from, to)) = night(ctx, chat) else {
            continue;
        };

        let now = now as u32;
        let inside = if from <= to {
            (from..to).contains(&now)
        } else {
            now >= from || now < to
        };
        let was = ctx.settings.value(chat, NIGHT_STATE).as_deref() == Some("on");
        if inside == was {
            continue;
        }
        let Some(chat_ref) = ctx.chat_ref(chat) else {
            continue;
        };
        if super::locks::set_group_lock(ctx, chat_ref, inside).await {
            ctx.settings
                .set_value(chat, NIGHT_STATE, if inside { "on" } else { "off" })
                .await;
            let _ = ctx
                .client
                .send_message(
                    chat_ref,
                    InputMessage::new().html(if inside {
                        "<b>قفل شب</b>\n\nگروه تا صبح بسته شد."
                    } else {
                        "<b>قفل شب</b>\n\nگروه باز شد."
                    }),
                )
                .await;
        }
    }
}

async fn tag_all(ctx: &Ctx, message: &Message, wanted: usize) {
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return;
    };
    let wanted = wanted.clamp(1, TAG_MAX);

    let caller = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id);

    let anchor = message.get_reply().await.ok().flatten();
    let anchor = anchor.as_ref().unwrap_or(message);

    let mut participants = ctx.client.iter_participants(chat_ref);
    let mut batch: Vec<String> = Vec::with_capacity(TAG_PER_MESSAGE);
    let mut tagged = 0;
    loop {
        let next = participants.next().await;
        let done = matches!(next, Ok(None) | Err(_));
        if let Ok(Some(participant)) = next {
            let user = participant.user.id().bare_id_unchecked();
            if participant.user.is_bot() || Some(user) == caller {
                continue;
            }
            batch.push(format!(
                "<a href=\"tg://user?id={user}\">{}</a>",
                esc(&participant.user.full_name())
            ));
        }
        let full = batch.len() >= TAG_PER_MESSAGE || tagged + batch.len() >= wanted;
        if batch.is_empty() || (!full && !done) {
            if done {
                break;
            }
            continue;
        }
        let body = batch.join(" · ");
        let _ = anchor.reply(InputMessage::new().html(body)).await;
        tagged += batch.len();
        batch.clear();
        if done || tagged >= wanted {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    if tagged == 0 {
        let _ = message.reply("کسی برای تگ کردن پیدا نشد.").await;
    }
}

fn after<'a>(text: &'a str, commands: &[&str]) -> Option<&'a str> {
    commands.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
    })
}

fn clock_at(word: &str) -> Option<u32> {
    let (hours, minutes) = match word.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (word.parse::<u32>().ok()?, 0),
    };
    Some((hours % 24) * 60 + minutes.min(59))
}

fn none_if_empty(text: &str) -> Option<&str> {
    (!text.is_empty()).then_some(text)
}
