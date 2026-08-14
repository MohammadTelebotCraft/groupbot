use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::PeerId;

use super::{Ctx, esc, name_of};
use crate::state::{Bump, Bumped, Counter, Period};

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

const IDLE_SHOWN: i64 = 10;

const BOARD: i64 = 10;

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
                    .html(section_text(ctx, chat, "sum").await)
                    .reply_markup(markup(ctx, chat, opener, "sum").await),
            )
            .await;
        return true;
    }
    if REPORT_CLEAR.contains(&text) {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
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
        if !super::limits::allows(ctx, message, super::limits::SET).await {
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
        return user_card(ctx, message, chat, text).await;
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
                    .html(section_text(ctx, chat, "idle").await)
                    .reply_markup(markup(ctx, chat, opener, "idle").await),
            )
            .await;
        return;
    }

    let _ = query
        .answer()
        .edit(
            InputMessage::new()
                .html(section_text(ctx, chat, section).await)
                .reply_markup(markup(ctx, chat, opener, section).await),
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
    ctx.settings.clear_seen(chat, user).await;
}

async fn markup(
    ctx: &Ctx,
    chat: i64,
    opener: i64,
    current: &str,
) -> grammers_client::message::ReplyMarkup {
    use grammers_client::message::{Button, ReplyMarkup};

    let mut rows: Vec<Vec<Button>> = Vec::new();
    if current == "idle" {
        let (idle, _) = ctx.settings.idle(chat, today(), IDLE_DAYS, IDLE_SHOWN).await;
        for (user, name, quiet) in idle {
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

async fn section_text(ctx: &Ctx, chat: i64, section: &str) -> String {
    clamp(section_body(ctx, chat, section).await)
}

async fn section_body(ctx: &Ctx, chat: i64, section: &str) -> String {
    let day = today();
    let title = ctx
        .settings
        .value(chat, super::TITLE)
        .unwrap_or_else(|| chat.to_string());
    let head = format!("<b>آمار {}</b>", esc(&title));

    match section {
        "top" => {
            let today_rows = ctx.settings.board(chat, Period::Today, day, BOARD).await;
            let all_rows = ctx.settings.board(chat, Period::Total, 0, BOARD).await;
            format!(
                "{head} › <b>پرچت ها</b>\n\n<b>امروز</b>\n{}\n\n<b>کل</b>\n{}",
                leaderboard(&today_rows),
                leaderboard(&all_rows)
            )
        }
        "week" | "month" => {
            let (period, stamp, label) = if section == "week" {
                (Period::Week, week_of(day), "هفته")
            } else {
                (Period::Month, month_of(day), "ماه")
            };
            let ranked = ctx.settings.board(chat, period, stamp, BOARD).await;
            let (total, active) = ctx.settings.board_totals(chat, period, stamp).await;
            format!(
                "{head} › <b>{label}</b>\n\n\
                 پیام های این {label} · <b>{total}</b>\n\
                 کاربران فعال · <b>{active}</b>\n\n{}",
                leaderboard(&ranked)
            )
        }
        "idle" => {
            let (_, idle) = ctx.settings.idle(chat, day, IDLE_DAYS, IDLE_SHOWN).await;
            format!(
                "{head} › <b>غیرفعال ها</b>\n\n\
                 کسانی که بیش از <b>{IDLE_DAYS}</b> روز پیامی نفرستاده اند ({}).\n\
                 برای اخراج روی هر نام بزنید.\n\n\
                 <i>تنها کسانی شمرده می شوند که از زمان نصب ربات پیامی فرستاده اند.</i>",
                idle
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
            let adds = ctx.settings.board(chat, Period::Adds, 0, BOARD).await;
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
            let (today_total, today_users) =
                ctx.settings.board_totals(chat, Period::Today, day).await;
            let (all_total, members) = ctx.settings.board_totals(chat, Period::Total, 0).await;
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
                 فعال امروز · <b>{today_users}</b>\n\
                 کاربران ثبت شده · <b>{members}</b>\n\
                 شلوغ ترین ساعت · <b>{busiest}</b>\n\
                 حذف شده امروز · <b>{}</b>\n\
                 پیوستن امروز · <b>{}</b>",
                tally(ctx, chat, DELETED, day),
                tally(ctx, chat, JOINED, day),
            )
        }
    }
}

fn leaderboard(rows: &[Counter]) -> String {
    if rows.is_empty() {
        return "‹ چیزی ثبت نشده".to_owned();
    }
    rows.iter()
        .enumerate()
        .map(|(place, row)| {
            format!(
                "{}. <a href=\"tg://user?id={}\">{}</a> · <b>{}</b>",
                place + 1,
                row.user,
                esc(if row.name.is_empty() { "کاربر" } else { &row.name }),
                row.count
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn user_card(ctx: &Ctx, message: &Message, chat: i64, text: &str) -> bool {
    let arg = INFO
        .iter()
        .find_map(|command| text.strip_prefix(command))
        .map(str::trim)
        .filter(|rest| !rest.is_empty())
        .map(str::to_owned);

    let Some(named) = super::named(message, arg.as_deref()) else {
        return false;
    };
    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        let _ = message
            .reply("کاربر پیدا نشد. ریپلای کنید یا @username / آیدی عددی بفرستید.")
            .await;
        return true;
    };
    let Some(user) = target.id.bare_id() else {
        return true;
    };

    let counts = ctx.settings.card(chat, user, today()).await;
    let place = counts
        .place
        .map(|place| place.to_string())
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
         پیام های امروز · <b>{}</b>\n\
         پیام های کل · <b>{}</b>\n\
         رتبه امروز · <b>{place}</b>\n\
         اعضای اضافه کرده · <b>{}</b>{}",
        esc(&name),
        esc(&username),
        counts.today,
        counts.total,
        counts.adds,
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
    true
}

pub fn count(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) {
    let (Some(chat), Some(user)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_id().and_then(PeerId::bare_id),
    ) else {
        return;
    };
    ctx.count_message(chat, user, || name_of(message));
    ctx.bump(chat, kind_of(view));
    ctx.bump(chat, HOURS[local_hour() as usize % 24]);
}

fn kind_of(view: &super::locks::View<'_>) -> &'static str {
    use grammers_client::media::Media;
    match view.media() {
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

    if !rows.is_empty() {
        ctx.settings.set_values(rows).await;
    }

    let bumps: Vec<Bump> = ctx
        .take_counts()
        .into_iter()
        .map(|((chat, user), (added, name))| Bump {
            chat,
            user,
            name,
            added,
        })
        .collect();
    if bumps.is_empty() {
        return;
    }
    for bumped in ctx
        .settings
        .bump(&bumps, day, week_of(day), month_of(day))
        .await
    {
        award_rank(ctx, &bumped).await;
    }
}

pub async fn prune(ctx: &Ctx) {
    match ctx.settings.forget_idle(today(), FORGET_DAYS).await {
        0 => {}
        dropped => println!("forgot {dropped} members quiet for {FORGET_DAYS}+ days"),
    }
}

async fn award_rank(ctx: &Ctx, bumped: &Bumped) {
    let (chat, user, total) = (bumped.chat, bumped.user, bumped.total);
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
    if bumped.awarded >= *milestone {
        return;
    }
    let name = bumped.name.as_str();

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
            ctx.settings.set_awarded(chat, user, *milestone).await;
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
        let body = daily_body(ctx, chat, day).await;
        if let Err(e) = ctx
            .client
            .send_message(chat_ref, InputMessage::new().html(body))
            .await
        {
            eprintln!("daily report: {chat}: {e}");
        }
    }
}

pub async fn daily_body(ctx: &Ctx, chat: i64, day: u64) -> String {
    let ranked = ctx.settings.board(chat, Period::Today, day, BOARD).await;
    let (sent, active) = ctx.settings.board_totals(chat, Period::Today, day).await;
    format!(
        "<b>گزارش امروز</b>\n\n\
         پیام ها · <b>{sent}</b>\n\
         کاربران فعال · <b>{active}</b>\n\
         پیوستن · <b>{}</b>\n\
         خروج · <b>{}</b>\n\
         حذف شده · <b>{}</b>\n\
         بن · <b>{}</b>\n\
         سکوت · <b>{}</b>\n\
         اخطار · <b>{}</b>\n\n\
         <b>پرچت های امروز</b>\n{}",
        tally(ctx, chat, JOINED, day),
        tally(ctx, chat, LEFT, day),
        tally(ctx, chat, DELETED, day),
        tally(ctx, chat, BANNED, day),
        tally(ctx, chat, MUTED, day),
        tally(ctx, chat, WARNED, day),
        leaderboard(&ranked),
    )
}

pub async fn adds(ctx: &Ctx, chat: i64, user: i64) -> u64 {
    if let Some(added) = ctx.cached_adds(chat, user) {
        return added;
    }
    let added = ctx.settings.adds_of(chat, user).await;
    ctx.remember_adds(chat, user, added);
    added
}

pub async fn known_name(ctx: &Ctx, chat: i64, user: i64) -> Option<String> {
    ctx.settings
        .name_of(chat, user)
        .await
        .filter(|name| *name != user.to_string())
}

pub async fn count_add(ctx: &Ctx, message: &Message, added: usize) {
    let (Some(chat), Some(user)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_id().and_then(PeerId::bare_id),
    ) else {
        return;
    };
    let total = ctx
        .settings
        .credit_add(chat, user, &name_of(message), added as u64)
        .await;
    ctx.remember_adds(chat, user, total);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_stamps_turn_over_with_their_period() {
        assert_eq!(week_of(6), week_of(0));
        assert_ne!(week_of(7), week_of(6));
        assert_eq!(month_of(29), month_of(0));
        assert_ne!(month_of(30), month_of(29));
    }
}
