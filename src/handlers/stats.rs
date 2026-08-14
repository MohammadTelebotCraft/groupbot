use std::collections::HashMap;

use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::PeerId;

use super::{Ctx, esc, name_of};

pub const TOTAL: &str = "total:";

pub const TODAY: &str = "today:";

pub const WEEK: &str = "week:";
pub const MONTH: &str = "month:";

pub const SEEN: &str = "seen:";

pub const RANK: &str = "rank:";

pub const RANKS: &str = "ranks";

pub const REPORT_AT: &str = "report_at";

pub const REPORT_DAY: &str = "report_day";

pub const REPORT_PRESETS: &[u32] = &[8 * 60, 12 * 60, 18 * 60, 21 * 60, 23 * 60, 0];

pub const REPORT_DEFAULT: u32 = 21 * 60;

pub const MILESTONES: &[(u64, &str)] = &[
    (100, "فعال"),
    (500, "پرچت"),
    (1_000, "افسانه"),
    (5_000, "استاد گروه"),
    (10_000, "اسطوره"),
];

pub fn week_of(day: u64) -> u64 {
    day / 7
}

pub fn month_of(day: u64) -> u64 {
    day / 30
}

pub const ADDS: &str = "adds:";

pub const TALLY: &str = "tally:";

pub const DELETED: &str = "deleted";
pub const JOINED: &str = "joined";
pub const LEFT: &str = "left";
pub const WARNED: &str = "warned";
pub const BANNED: &str = "banned";
pub const MUTED: &str = "muted";
pub const REPORTED: &str = "reported";
pub const CAPTCHA_PASSED: &str = "captcha_ok";
pub const CAPTCHA_FAILED: &str = "captcha_no";

pub const KINDS: &[(&str, &str)] = &[
    ("k_text", "متن"),
    ("k_photo", "عکس"),
    ("k_video", "ویدیو"),
    ("k_sticker", "استیکر"),
    ("k_gif", "گیف"),
    ("k_voice", "ویس"),
    ("k_music", "موزیک"),
    ("k_file", "فایل"),
    ("k_other", "سایر"),
];

const HOURS: [&str; 24] = [
    "h0", "h1", "h2", "h3", "h4", "h5", "h6", "h7", "h8", "h9", "h10", "h11", "h12", "h13",
    "h14", "h15", "h16", "h17", "h18", "h19", "h20", "h21", "h22", "h23",
];

const STATS: &[&str] = &["امار", "آمار", "امار گروه", "آمار گروه"];
const REPORT_SET: &[&str] = &["تنظیم گزارش روزانه", "گزارش روزانه"];
const REPORT_CLEAR: &[&str] = &["حذف گزارش روزانه", "خاموش گزارش روزانه"];
const INFO: &[&str] = &["اطلاعات", "پروفایل", "کاربر", "ایدی", "آیدی"];

pub const TEHRAN_OFFSET: u64 = 3 * 3600 + 1800;

pub fn local_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() + TEHRAN_OFFSET)
        .unwrap_or(0)
}

pub fn today() -> u64 {
    local_seconds() / 86_400
}

pub fn local_hour() -> u64 {
    (local_seconds() % 86_400) / 3600
}

pub const IDLE_DAYS: u64 = 14;

const FORGET_DAYS: u64 = 90;

pub fn idle_members(ctx: &Ctx, chat: i64, day: u64, days: u64) -> Vec<(i64, String, u64)> {
    let mut idle: Vec<(i64, String, u64)> = ctx.settings.pick_values(chat, SEEN, |user, value| {
        let (seen, name) = value.split_once('|')?;
        let seen: u64 = seen.parse().ok()?;
        let quiet = day.saturating_sub(seen);
        let user: i64 = user.parse().ok()?;
        (quiet >= days).then(|| (user, name.to_owned(), quiet))
    });
    idle.sort_unstable_by_key(|(_, _, quiet)| std::cmp::Reverse(*quiet));
    idle
}

struct Row {
    user: i64,
    count: u64,
    name: String,
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if STATS.contains(&text) {
        let Some(opener) = message.sender_id().and_then(PeerId::bare_id) else {
            return false;
        };
        let _ = message
            .reply(
                InputMessage::new()
                    .html(section_text(ctx, chat, "sum"))
                    .reply_markup(markup(ctx, chat, opener, "sum")),
            )
            .await;
        return true;
    }
    if REPORT_CLEAR.contains(&text) {
        if !super::can_manage(ctx, message).await {
            return true;
        }
        set_report_at(ctx, chat, None).await;
        let _ = message.reply("✗ گزارش روزانه خاموش شد.").await;
        return true;
    }
    if let Some(rest) = REPORT_SET.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
    }) {
        if !super::can_manage(ctx, message).await {
            return true;
        }
        let typed = super::digits(rest);
        let at = match typed.split_once([':', '.']) {
            Some((hour, minute)) => match (hour.trim().parse::<u32>(), minute.trim().parse::<u32>())
            {
                (Ok(hour), Ok(minute)) if hour < 24 && minute < 60 => Some(hour * 60 + minute),
                _ => None,
            },
            None => typed.trim().parse::<u32>().ok().filter(|h| *h < 24).map(|h| h * 60),
        };
        let _ = match at {
            Some(at) => {
                set_report_at(ctx, chat, Some(at)).await;
                message
                    .reply(format!(
                        "✓ گزارش روزانه هر روز ساعت {} در همین گروه فرستاده می شود.",
                        super::extras::clock(at)
                    ))
                    .await
            }
            None => message.reply(InputMessage::new().html(report_status(ctx, chat))).await,
        };
        return true;
    }
    if INFO.iter().any(|command| {
        text == *command || text.strip_prefix(command).is_some_and(|r| r.starts_with(' '))
    }) {
        user_card(ctx, message, chat, text).await;
        return true;
    }
    false
}

pub async fn on_callback(
    ctx: &Ctx,
    query: &grammers_client::update::CallbackQuery,
    payload: &str,
) {
    let mut parts = payload.splitn(3, ':');
    let (Some(opener), Some(chat), Some(section)) = (parts.next(), parts.next(), parts.next())
    else {
        return;
    };
    let (Ok(opener), Ok(chat)) = (opener.parse::<i64>(), chat.parse::<i64>()) else {
        return;
    };
    if query.sender_id().bare_id() != Some(opener) {
        let _ = query
            .answer()
            .alert("این آمار را شخص دیگری باز کرده است.")
            .send()
            .await;
        return;
    }

    if let Some(user) = section.strip_prefix("kick:") {
        if let Ok(user) = user.parse::<i64>() {
            kick_idle(ctx, chat, user).await;
        }
        let _ = query
            .answer()
            .edit(
                InputMessage::new()
                    .html(section_text(ctx, chat, "idle"))
                    .reply_markup(markup(ctx, chat, opener, "idle")),
            )
            .await;
        return;
    }

    let _ = query
        .answer()
        .edit(
            InputMessage::new()
                .html(section_text(ctx, chat, section))
                .reply_markup(markup(ctx, chat, opener, section)),
        )
        .await;
}

async fn kick_idle(ctx: &Ctx, chat: i64, user: i64) {
    let (Some(chat_ref), Some(target)) = (
        ctx.chat_ref(chat),
        PeerId::user(user).map(PeerId::to_ambient_ref),
    ) else {
        return;
    };
    if let Err(e) = ctx.client.kick_participant(chat_ref, target).await {
        eprintln!("stats: {chat}: could not kick idle {user}: {e}");
        return;
    }
    ctx.settings.set_value(chat, &format!("{SEEN}{user}"), "").await;
}

fn markup(
    ctx: &Ctx,
    chat: i64,
    opener: i64,
    current: &str,
) -> grammers_client::message::ReplyMarkup {
    use grammers_client::message::{Button, ReplyMarkup};

    let mut rows: Vec<Vec<Button>> = Vec::new();
    if current == "idle" {
        for (user, name, quiet) in idle_members(ctx, chat, today(), IDLE_DAYS)
            .into_iter()
            .take(10)
        {
            rows.push(vec![super::style::data(
                format!("✗  {name} · {quiet} روز"),
                format!("s:{opener}:{chat}:kick:{user}").into_bytes(),
                super::style::Colour::Danger,
            )]);
        }
    }
    for pair in SECTIONS.chunks(2) {
        rows.push(
            pair.iter()
                .map(|(key, label)| {
                    super::style::choice(
                        *label,
                        format!("s:{opener}:{chat}:{key}").into_bytes(),
                        *key == current,
                    )
                })
                .collect(),
        );
    }
    ReplyMarkup::from_buttons(&rows)
}

const SECTIONS: &[(&str, &str)] = &[
    ("sum", "📊  خلاصه"),
    ("top", "🏆  پرچت ها"),
    ("week", "📅  هفته"),
    ("month", "🗓  ماه"),
    ("idle", "😴  غیرفعال ها"),
    ("hours", "🕒  ساعت ها"),
    ("kinds", "🖼  نوع پیام"),
    ("members", "👥  اعضا"),
    ("mod", "🛡  مدیریت"),
];

const MAX_TEXT: usize = 3_500;

fn clamp(text: String) -> String {
    if text.chars().count() <= MAX_TEXT {
        return text;
    }
    let mut cut: String = text.chars().take(MAX_TEXT).collect();

    if let Some(open) = cut.rfind('<')
        && !cut[open..].contains('>')
    {
        cut.truncate(open);
    }
    format!("{cut}\n\n<i>…کوتاه شد</i>")
}

fn section_text(ctx: &Ctx, chat: i64, section: &str) -> String {
    clamp(section_body(ctx, chat, section))
}

fn section_body(ctx: &Ctx, chat: i64, section: &str) -> String {
    let day = today();
    let title = ctx
        .settings
        .value(chat, super::TITLE)
        .unwrap_or_else(|| chat.to_string());
    let head = format!("<b>آمار {}</b>", esc(&title));

    match section {
        "top" => {
            let mut today_rows = rows(ctx, chat, TODAY, day);
            let mut all_rows = rows(ctx, chat, TOTAL, day);
            today_rows.sort_unstable_by_key(|row| std::cmp::Reverse(row.count));
            all_rows.sort_unstable_by_key(|row| std::cmp::Reverse(row.count));
            format!(
                "{head} › <b>پرچت ها</b>\n\n<b>امروز</b>\n{}\n\n<b>کل</b>\n{}",
                leaderboard(&today_rows),
                leaderboard(&all_rows)
            )
        }
        "week" | "month" => {
            let prefix = if section == "week" { WEEK } else { MONTH };
            let label = if section == "week" { "هفته" } else { "ماه" };
            let mut ranked = rows(ctx, chat, prefix, day);
            ranked.sort_unstable_by_key(|row| std::cmp::Reverse(row.count));
            let total: u64 = ranked.iter().map(|row| row.count).sum();
            format!(
                "{head} › <b>{label}</b>\n\n\
                 پیام های این {label} · <b>{total}</b>\n\
                 کاربران فعال · <b>{}</b>\n\n{}",
                ranked.len(),
                leaderboard(&ranked)
            )
        }
        "idle" => {
            let idle = idle_members(ctx, chat, day, IDLE_DAYS);
            format!(
                "{head} › <b>غیرفعال ها</b>\n\n\
                 کسانی که بیش از <b>{IDLE_DAYS}</b> روز پیامی نفرستاده اند ({}).\n\
                 برای اخراج روی هر نام بزنید.\n\n\
                 <i>تنها کسانی شمرده می شوند که از زمان نصب ربات پیامی فرستاده اند.</i>",
                idle.len()
            )
        }
        "hours" => {
            let counts: Vec<u64> = (0..24)
                .map(|hour| tally(ctx, chat, &format!("h{hour}"), day))
                .collect();
            let peak = counts.iter().copied().max().unwrap_or(0);
            let busiest = counts
                .iter()
                .enumerate()
                .max_by_key(|(_, count)| **count)
                .map(|(hour, _)| hour)
                .unwrap_or(0);
            let chart = counts
                .iter()
                .enumerate()
                .filter(|(_, count)| **count > 0)
                .map(|(hour, count)| {
                    let width = count.checked_mul(12).and_then(|n| n.checked_div(peak)).unwrap_or(0) as usize;
                    format!(
                        "<code>{hour:02}</code> {}{} <b>{count}</b>",
                        "█".repeat(width.max(1)),
                        " ".repeat(12 - width.max(1))
                    )
                })
                .collect::<Vec<_>>();
            format!(
                "{head} › <b>ساعت ها</b> (امروز، به وقت تهران)\n\n{}\n\nشلوغ ترین ساعت · <b>{busiest:02}</b>",
                if chart.is_empty() {
                    "‹ هنوز پیامی امروز ثبت نشده".to_owned()
                } else {
                    chart.join("\n")
                }
            )
        }
        "kinds" => {
            let counts: Vec<(&str, u64)> = KINDS
                .iter()
                .map(|(key, label)| (*label, tally(ctx, chat, key, day)))
                .collect();
            let total: u64 = counts.iter().map(|(_, count)| count).sum::<u64>().max(1);
            let lines = counts
                .iter()
                .filter(|(_, count)| *count > 0)
                .map(|(label, count)| {
                    format!("{label} · <b>{count}</b> ({}٪)", count * 100 / total)
                })
                .collect::<Vec<_>>();
            format!(
                "{head} › <b>نوع پیام</b> (امروز)\n\n{}",
                if lines.is_empty() {
                    "‹ هنوز پیامی امروز ثبت نشده".to_owned()
                } else {
                    lines.join("\n")
                }
            )
        }
        "members" => {
            let mut adds = rows(ctx, chat, ADDS, day);
            adds.sort_unstable_by_key(|row| std::cmp::Reverse(row.count));
            format!(
                "{head} › <b>اعضا</b>\n\n\
                 پیوستن امروز · <b>{}</b>\n\
                 خروج امروز · <b>{}</b>\n\
                 احراز هویت موفق · <b>{}</b>\n\
                 احراز هویت ناموفق · <b>{}</b>\n\n\
                 <b>بیشترین عضوگیری</b>\n{}",
                tally(ctx, chat, JOINED, day),
                tally(ctx, chat, LEFT, day),
                tally(ctx, chat, CAPTCHA_PASSED, day),
                tally(ctx, chat, CAPTCHA_FAILED, day),
                leaderboard(&adds)
            )
        }
        "mod" => format!(
            "{head} › <b>مدیریت</b> (امروز)\n\n\
             پیام های حذف شده · <b>{}</b>\n\
             اخطارها · <b>{}</b>\n\
             اخراج ها · <b>{}</b>\n\
             سکوت ها · <b>{}</b>\n\
             گزارش ها · <b>{}</b>\n\n\
             قفل های فعال · <b>{}</b> از <b>{}</b>",
            tally(ctx, chat, DELETED, day),
            tally(ctx, chat, WARNED, day),
            tally(ctx, chat, BANNED, day),
            tally(ctx, chat, MUTED, day),
            tally(ctx, chat, REPORTED, day),
            super::locks::LOCKS
                .iter()
                .filter(|lock| ctx.settings.is_locked(chat, lock.key))
                .count(),
            super::locks::LOCKS.len(),
        ),
        _ => {
            let today_rows = rows(ctx, chat, TODAY, day);
            let all_rows = rows(ctx, chat, TOTAL, day);
            let today_total: u64 = today_rows.iter().map(|row| row.count).sum();
            let all_total: u64 = all_rows.iter().map(|row| row.count).sum();
            let busiest = (0..24)
                .map(|hour| (hour, tally(ctx, chat, &format!("h{hour}"), day)))
                .max_by_key(|(_, count)| *count)
                .filter(|(_, count)| *count > 0)
                .map(|(hour, _)| format!("{hour:02}"))
                .unwrap_or_else(|| "—".to_owned());
            format!(
                "{head} › <b>خلاصه</b>\n\n\
                 پیام های امروز · <b>{today_total}</b>\n\
                 پیام های کل · <b>{all_total}</b>\n\
                 فعال امروز · <b>{}</b>\n\
                 کاربران ثبت شده · <b>{}</b>\n\
                 شلوغ ترین ساعت · <b>{busiest}</b>\n\
                 حذف شده امروز · <b>{}</b>\n\
                 پیوستن امروز · <b>{}</b>",
                today_rows.len(),
                all_rows.len(),
                tally(ctx, chat, DELETED, day),
                tally(ctx, chat, JOINED, day),
            )
        }
    }
}

fn leaderboard(rows: &[Row]) -> String {
    if rows.is_empty() {
        return "‹ چیزی ثبت نشده".to_owned();
    }
    rows.iter()
        .take(10)
        .enumerate()
        .map(|(place, row)| {
            format!(
                "{}. <a href=\"tg://user?id={}\">{}</a> · <b>{}</b>",
                place + 1,
                row.user,
                esc(&row.name),
                row.count
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn user_card(ctx: &Ctx, message: &Message, chat: i64, text: &str) {
    let arg = INFO
        .iter()
        .find_map(|command| text.strip_prefix(command))
        .map(str::trim)
        .filter(|rest| !rest.is_empty())
        .map(str::to_owned);
    let Some((target, name)) = super::target(ctx, message, arg.as_deref()).await else {
        let _ = message
            .reply("کاربر پیدا نشد. ریپلای کنید یا @username / آیدی عددی بفرستید.")
            .await;
        return;
    };
    let Some(user) = target.id.bare_id() else {
        return;
    };

    let day = today();
    let today_count = count_of(ctx, chat, TODAY, user, day);
    let total_count = count_of(ctx, chat, TOTAL, user, day);
    let adds = count_of(ctx, chat, ADDS, user, day);
    let mut ranked = rows(ctx, chat, TODAY, day);
    ranked.sort_unstable_by_key(|row| std::cmp::Reverse(row.count));
    let place = ranked
        .iter()
        .position(|row| row.user == user)
        .map(|index| (index + 1).to_string())
        .unwrap_or_else(|| "بدون رتبه".to_owned());

    let role = if super::owner(ctx, chat) == Some(user) {
        "مالک ربات"
    } else if super::is_bot_admin(ctx, chat, user) {
        "ادمین ربات"
    } else if super::vip::is_vip(ctx, chat, user) {
        "کاربر ویژه"
    } else {
        "فرد عادی"
    };

    let username = ctx
        .client
        .resolve_peer(target)
        .await
        .ok()
        .and_then(|peer| peer.username().map(|u| format!("@{u}")))
        .unwrap_or_else(|| "بدون یوزرنیم".to_owned());

    let mut photos = ctx.client.iter_profile_photos(target);
    let photo_count = photos.total().await.unwrap_or(0);
    let mut card = InputMessage::new().html(format!(
        "<b>اطلاعات کاربر</b>\n\n\
         نام · <a href=\"tg://user?id={user}\">{}</a>\n\
         آیدی عددی · <code>{user}</code>\n\
         یوزرنیم · {}\n\
         تصاویر پروفایل · <b>{photo_count}</b>\n\
         مقام · <b>{role}</b>\n\n\
         <b>آمار کاربر</b>\n\
         پیام های امروز · <b>{today_count}</b>\n\
         پیام های کل · <b>{total_count}</b>\n\
         رتبه امروز · <b>{place}</b>\n\
         اعضای اضافه کرده · <b>{adds}</b>{}",
        esc(&name),
        esc(&username),
        match super::extras::note(ctx, chat, user) {
            Some(note) => format!("\n\n<b>یادداشت</b>\n{}", esc(&note)),
            None => String::new(),
        },
    ));

    if let Ok(Some(photo)) = photos.next().await
        && let Some(media) = super::welcome::encode_media(&grammers_client::media::Media::Photo(photo))
        && let Some(media) = super::welcome::decode_media(&media)
    {
        card = card.media(media);
    }
    let _ = message.reply(card).await;
}

fn rows(ctx: &Ctx, chat: i64, prefix: &str, day: u64) -> Vec<Row> {
    ctx.settings.pick_values(chat, prefix, |user, value| {
        let (count, name) = parse_value(value, prefix, day)?;
        Some(Row {
            user: user.parse().ok()?,
            count,
            name,
        })
    })
}

fn count_of(ctx: &Ctx, chat: i64, prefix: &str, user: i64, day: u64) -> u64 {
    ctx.settings
        .value(chat, &format!("{prefix}{user}"))
        .and_then(|value| parse_value(&value, prefix, day))
        .map(|(count, _)| count)
        .unwrap_or(0)
}

fn parse_value(value: &str, prefix: &str, day: u64) -> Option<(u64, String)> {
    let mut parts = value.split('|');
    let expected = match prefix {
        TODAY => Some(day),
        WEEK => Some(week_of(day)),
        MONTH => Some(month_of(day)),
        _ => None,
    };
    if let Some(expected) = expected {
        let stored: u64 = parts.next()?.parse().ok()?;
        if stored != expected {
            return None;
        }
    }
    let count = parts.next()?.parse().ok()?;
    let name = parts.next().unwrap_or("کاربر").to_owned();
    Some((count, name))
}

pub fn count(ctx: &Ctx, message: &Message) {
    let (Some(chat), Some(user)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_id().and_then(PeerId::bare_id),
    ) else {
        return;
    };
    ctx.count_message(chat, user, || name_of(message));
    ctx.bump(chat, kind_of(message));
    ctx.bump(chat, HOURS[local_hour() as usize % 24]);
}

fn kind_of(message: &Message) -> &'static str {
    use grammers_client::media::Media;
    match message.media() {
        None => "k_text",
        Some(Media::Photo(_)) => "k_photo",
        Some(Media::Sticker(_)) => "k_sticker",
        Some(Media::Document(doc)) => {
            let mime = doc.mime_type().unwrap_or_default();
            if doc.is_animated() || mime == "image/gif" {
                "k_gif"
            } else if mime.starts_with("video/") {
                "k_video"
            } else if mime.starts_with("audio/ogg") {
                "k_voice"
            } else if mime.starts_with("audio/") {
                "k_music"
            } else {
                "k_file"
            }
        }
        Some(_) => "k_other",
    }
}

pub fn tally(ctx: &Ctx, chat: i64, counter: &str, day: u64) -> u64 {
    ctx.settings
        .value(chat, &format!("{TALLY}{counter}"))
        .and_then(|value| {
            let (stored, count) = value.split_once('|')?;
            (stored.parse::<u64>().ok()? == day).then(|| count.parse().ok())?
        })
        .unwrap_or(0)
}

pub async fn flush(ctx: &Ctx) {
    let day = today();
    let mut rows: Vec<(i64, String, String)> = Vec::new();

    for ((chat, counter), added) in ctx.take_tallies() {
        let total = tally(ctx, chat, counter, day) + added;
        rows.push((chat, format!("{TALLY}{counter}"), format!("{day}|{total}")));
    }

    let pending: HashMap<(i64, i64), (u64, String)> = ctx.take_counts();
    let mut ranked: Vec<(i64, i64, String, u64)> = Vec::new();
    for ((chat, user), (added, name)) in pending {
        let total = count_of(ctx, chat, TOTAL, user, day) + added;
        let today_count = count_of(ctx, chat, TODAY, user, day) + added;
        let week_count = count_of(ctx, chat, WEEK, user, day) + added;
        let month_count = count_of(ctx, chat, MONTH, user, day) + added;

        rows.push((chat, format!("{TOTAL}{user}"), format!("{total}|{name}")));
        rows.push((
            chat,
            format!("{TODAY}{user}"),
            format!("{day}|{today_count}|{name}"),
        ));
        rows.push((
            chat,
            format!("{WEEK}{user}"),
            format!("{}|{week_count}|{name}", week_of(day)),
        ));
        rows.push((
            chat,
            format!("{MONTH}{user}"),
            format!("{}|{month_count}|{name}", month_of(day)),
        ));
        rows.push((chat, format!("{SEEN}{user}"), format!("{day}|{name}")));
        ranked.push((chat, user, name, total));
    }

    if !rows.is_empty() {
        ctx.settings.set_values(rows).await;
    }
    for (chat, user, name, total) in ranked {
        award_rank(ctx, chat, user, &name, total).await;
    }
}

pub async fn prune(ctx: &Ctx) {
    let day = today();

    let forgotten: Vec<(i64, i64)> = ctx
        .settings
        .chats()
        .into_iter()
        .flat_map(|chat| {
            idle_members(ctx, chat, day, FORGET_DAYS)
                .into_iter()
                .map(move |(user, _, _)| (chat, user))
        })
        .collect();
    let mut dropped = 0;
    if !forgotten.is_empty() {
        let forgotten: std::collections::HashSet<(i64, i64)> = forgotten.into_iter().collect();
        for prefix in [TOTAL, SEEN, ADDS, RANK] {
            dropped += ctx
                .settings
                .prune_users(prefix, |chat, user| forgotten.contains(&(chat, user)))
                .await;
        }
    }

    let dropped = dropped
        + ctx.settings.prune_stale(TODAY, &format!("{day}|")).await
        + ctx
            .settings
            .prune_stale(WEEK, &format!("{}|", week_of(day)))
            .await
        + ctx
            .settings
            .prune_stale(MONTH, &format!("{}|", month_of(day)))
            .await;
    if dropped > 0 {
        println!("pruned {dropped} stale counter rows");
    }
}

async fn award_rank(ctx: &Ctx, chat: i64, user: i64, name: &str, total: u64) {
    if !ctx.settings.is_locked(chat, RANKS) {
        return;
    }
    let Some((milestone, title)) = MILESTONES
        .iter()
        .rev()
        .find(|(needed, _)| total >= *needed)
    else {
        return;
    };
    let already: u64 = ctx
        .settings
        .value(chat, &format!("{RANK}{user}"))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if already >= *milestone {
        return;
    }

    let (Some(chat_ref), Some(target)) = (
        ctx.chat_ref(chat),
        PeerId::user(user).map(PeerId::to_ambient_ref),
    ) else {
        return;
    };
    let target = super::admin_ref(ctx, chat_ref, user)
        .await
        .map(|(peer, _)| peer)
        .unwrap_or(target);

    let builder = match ctx.client.set_admin_rights(chat_ref, target).load_current().await {
        Ok(builder) => builder,
        Err(e) => {
            eprintln!("ranks: {chat}: could not read rights of {user}: {e}");
            return;
        }
    };
    match builder.rank(*title).await {
        Ok(()) => {
            ctx.settings
                .set_value(chat, &format!("{RANK}{user}"), &milestone.to_string())
                .await;
            let _ = ctx
                .client
                .send_message(
                    chat_ref,
                    InputMessage::new().html(format!(
                        "<b>مقام جدید</b>\n\n<a href=\"tg://user?id={user}\">{}</a> با <b>{milestone}</b> پیام مقام «{title}» گرفت.",
                        esc(name)
                    )),
                )
                .await;
        }
        Err(e) => eprintln!("ranks: {chat}: could not title {user}: {e}"),
    }
}

pub fn report_status(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>گزارش روزانه</b>\n\n{}\n\n\
         <b>در گزارش چه می آید</b>\n\
         ‹ تعداد پیام ها و کاربران فعال\n\
         ‹ ورود و خروج اعضا\n\
         ‹ حذف ها، بن، سکوت و اخطار\n\
         ‹ ده نفر پرچت روز\n\n\
         <i>ساعت دلخواه: «گزارش روزانه 21:30»</i>",
        match report_at(ctx, chat) {
            Some(at) => format!(
                "هر روز ساعت <b>{}</b> به وقت تهران، در همین گروه فرستاده می شود.",
                super::extras::clock(at)
            ),
            None => "خاموش است؛ هیچ گزارشی فرستاده نمی شود.".to_owned(),
        }
    )
}

pub fn report_at(ctx: &Ctx, chat: i64) -> Option<u32> {
    ctx.settings
        .value_parsed::<u32>(chat, REPORT_AT)
        .filter(|at| *at < 1440)
}

pub async fn set_report_at(ctx: &Ctx, chat: i64, at: Option<u32>) {
    match at {
        Some(at) => {
            ctx.settings
                .set_value(chat, REPORT_AT, &(at % 1440).to_string())
                .await
        }
        None => ctx.settings.set_value(chat, REPORT_AT, "").await,
    }
}

pub async fn run_daily(ctx: &Ctx) {
    let now = ((local_seconds() % 86_400) / 60) as u32;
    let day = today();
    for chat in ctx.settings.chats() {
        let Some(at) = report_at(ctx, chat).filter(|at| (0..=2).contains(&now.wrapping_sub(*at)))
        else {
            continue;
        };
        let _ = at;
        if ctx.settings.value_parsed::<u64>(chat, REPORT_DAY) == Some(day) {
            continue;
        }
        let Some(chat_ref) = ctx.chat_ref(chat) else {
            continue;
        };

        ctx.settings
            .set_value(chat, REPORT_DAY, &day.to_string())
            .await;
        let body = daily_body(ctx, chat, day);
        if let Err(e) = ctx
            .client
            .send_message(chat_ref, InputMessage::new().html(body))
            .await
        {
            eprintln!("daily report: {chat}: {e}");
        }
    }
}

pub fn daily_body(ctx: &Ctx, chat: i64, day: u64) -> String {
    let mut ranked = rows(ctx, chat, TODAY, day);
    ranked.sort_unstable_by_key(|row| std::cmp::Reverse(row.count));
    format!(
        "<b>گزارش امروز</b>\n\n\
         پیام ها · <b>{}</b>\n\
         کاربران فعال · <b>{}</b>\n\
         پیوستن · <b>{}</b>\n\
         خروج · <b>{}</b>\n\
         حذف شده · <b>{}</b>\n\
         بن · <b>{}</b>\n\
         سکوت · <b>{}</b>\n\
         اخطار · <b>{}</b>\n\n\
         <b>پرچت های امروز</b>\n{}",
        ranked.iter().map(|row| row.count).sum::<u64>(),
        ranked.len(),
        tally(ctx, chat, JOINED, day),
        tally(ctx, chat, LEFT, day),
        tally(ctx, chat, DELETED, day),
        tally(ctx, chat, BANNED, day),
        tally(ctx, chat, MUTED, day),
        tally(ctx, chat, WARNED, day),
        leaderboard(&ranked),
    )
}

pub fn known_name(ctx: &Ctx, chat: i64, user: i64) -> Option<String> {
    let value = ctx.settings.value(chat, &format!("{TOTAL}{user}"))?;
    let (_, name) = parse_value(&value, TOTAL, today())?;
    (!name.is_empty() && name != user.to_string()).then_some(name)
}

pub fn adds(ctx: &Ctx, chat: i64, user: i64) -> u64 {
    count_of(ctx, chat, ADDS, user, today())
}

pub async fn count_add(ctx: &Ctx, message: &Message, added: usize) {
    let (Some(chat), Some(user)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_id().and_then(PeerId::bare_id),
    ) else {
        return;
    };
    let day = today();
    let total = count_of(ctx, chat, ADDS, user, day) + added as u64;
    ctx.settings
        .set_value(
            chat,
            &format!("{ADDS}{user}"),
            &format!("{total}|{}", name_of(message)),
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_rows_expire_without_cleanup() {
        assert_eq!(
            parse_value("100|5|Ali", TODAY, 100),
            Some((5, "Ali".to_owned()))
        );

        assert_eq!(parse_value("99|5|Ali", TODAY, 100), None);
        assert_eq!(
            parse_value("42|Ali", TOTAL, 100),
            Some((42, "Ali".to_owned()))
        );
    }
}
