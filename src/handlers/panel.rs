use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::update::CallbackQuery;

use super::locks::LOCKS;
use super::style::{Colour, choice, data as coloured, toggle};
use super::{
    Ctx, answers, betrayal, can_manage, captcha, flood, join, lists, log, notice, raid, setting,
    strict, tempmedia, warns, welcome,
};

const OPEN: &[&str] = &["پنل", "تنظیمات", "پنل ربات"];

const TO_PRIVATE: &[&str] = &["پنل پیوی", "پنل پی وی", "پنل خصوصی"];

const ROOT_TITLE: &str = "<b>پنل مدیریت</b>\n\nبخشی را باز کنید.";

const PER_PAGE: usize = 10;
const LISTS_TITLE: &str = "<b>پنل مدیریت</b> › <b>لیست ها</b>\n\n\
     هر لیست را باز کنید؛ با زدن روی هر مورد حذف می شود.";
const ADVANCED_TITLE: &str = "<b>پنل مدیریت</b> › <b>تنظیمات پیشرفته</b>\n\n\
     بخشی را باز کنید.";
fn strict_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>حالت سختگیرانه</b>\n\n\
         فرستنده مورد قفل شده، علاوه بر حذف پیام، سکوت یا بن می شود.\n\n\
         با <b>{}</b> تخلف · {} · <b>{}</b>",
        strict::limit(ctx, chat),
        if strict::is_ban(ctx, chat) { "بن" } else { "سکوت" },
        strict::time_label(strict::minutes(ctx, chat)),
    )
}

async fn to_private(ctx: &Ctx, message: &Message) -> bool {
    if !can_manage(ctx, message).await {
        return true;
    }
    let (Some(chat), Ok(Some(user)), Some(opener)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_ref().await,
        message
            .sender_id()
            .and_then(grammers_client::session::types::PeerId::bare_id),
    ) else {
        return false;
    };

    let title = ctx
        .settings
        .value(chat, super::TITLE)
        .unwrap_or_else(|| chat.to_string());
    let sent = ctx
        .client
        .send_message(
            user,
            InputMessage::new()
                .html(format!(
                    "<b>پنل مدیریت</b> › <b>{}</b>\n\nبخشی را باز کنید.",
                    super::esc(&title)
                ))
                .reply_markup(root_markup(ctx, chat, opener)),
        )
        .await;

    let _ = match sent {
        Ok(_) => message.reply("✓ پنل به پیوی شما فرستاده شد.").await,
        Err(e) => {
            eprintln!("panel: {chat}: could not send private panel: {e}");
            message
                .reply("ابتدا ربات را در پیوی خود استارت کنید، سپس دوباره امتحان کنید.")
                .await
        }
    };
    true
}

pub async fn handle_private(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let opens = OPEN.contains(&text)
        || text.starts_with("پنل")
        || text.starts_with("تنظیمات")
        || text.starts_with("/panel");
    if !opens {
        return false;
    }
    let Some(user) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };

    let mut mine: Vec<(i64, String)> = Vec::new();
    for chat in ctx.settings.chats() {
        if super::owner(ctx, chat) != Some(user) && !super::is_bot_admin(ctx, chat, user) {
            continue;
        }
        let name = ctx
            .settings
            .value(chat, super::TITLE)
            .unwrap_or_else(|| chat.to_string());
        mine.push((chat, name));
    }

    if mine.is_empty() {
        let _ = message
            .reply("گروهی برای مدیریت پیدا نشد. در گروه خود «کانفیگ» را بفرستید.")
            .await;
        return true;
    }

    let rows: Vec<Vec<Button>> = mine
        .iter()
        .take(20)
        .map(|(chat, name)| {
            vec![Button::data(
                format!("{}  ›", super::esc(name)),
                payload(user, *chat, "root"),
            )]
        })
        .collect();
    let _ = message
        .reply(
            InputMessage::new()
                .html(format!(
                    "<b>پنل مدیریت</b>\n\nگروه را انتخاب کنید ({}).",
                    mine.len()
                ))
                .reply_markup(ReplyMarkup::from_buttons(&rows)),
        )
        .await;
    true
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    if TO_PRIVATE.contains(&text) {
        return to_private(ctx, message).await;
    }
    if !OPEN.contains(&text) {
        return false;
    }
    if !can_manage(ctx, message).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let Some(opener) = message.sender_id().and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };
    let _ = message
        .reply(
            InputMessage::new()
                .html(ROOT_TITLE)
                .reply_markup(root_markup(ctx, chat, opener)),
        )
        .await;
    true
}

fn custom_row(opener: i64, chat: i64, id: &str, current: impl std::fmt::Display) -> Vec<Button> {
    vec![Button::data(
        format!("✎  عدد دلخواه · {current}"),
        payload(opener, chat, &format!("in:{id}")),
    )]
}

fn rows_for(ctx: &Ctx, chat: i64, opener: i64, id: &str) -> Vec<Vec<Button>> {
    match setting::find(id) {
        Some(found) => setting::rows(ctx, chat, found, &|action| payload(opener, chat, action)),
        None => Vec::new(),
    }
}

fn built(ctx: &Ctx, chat: i64, opener: i64, ids: &[&str], back: &str) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = Vec::new();
    for id in ids {
        rows.extend(rows_for(ctx, chat, opener, id));
    }
    rows.push(vec![Button::data("‹ بازگشت", payload(opener, chat, back))]);
    ReplyMarkup::from_buttons(&rows)
}

pub async fn typed_number(ctx: &Ctx, message: &Message) -> bool {
    if !ctx.maybe_expecting_number() {
        return false;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let Some(user) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };

    let text = super::digits(message.text().trim());
    let Some(value) = parse_number(&text) else {
        return false;
    };
    let Some(id) = ctx.take_expected_number(chat, user) else {
        return false;
    };
    let Some((found, (min, max))) = setting::number(id) else {
        return false;
    };
    setting::store(ctx, chat, found, value.clamp(min, max)).await;
    let _ = message
        .reply(format!(
            "✓ {} · {}",
            found.label,
            setting::shown(ctx, chat, found)
        ))
        .await;
    true
}

fn parse_number(text: &str) -> Option<u32> {
    if let Some((hour, minute)) = text.split_once([':', '.'])
        && let (Ok(hour), Ok(minute)) = (hour.trim().parse::<u32>(), minute.trim().parse::<u32>())
        && hour < 24
        && minute < 60
    {
        return Some(hour * 60 + minute);
    }
    text.parse().ok()
}

fn payload(opener: i64, chat: i64, action: &str) -> Vec<u8> {
    format!("p:{opener}:{chat}:{action}").into_bytes()
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str) {
    let mut parts = payload.splitn(3, ':');
    let (Some(opener), Some(chat), Some(action)) = (parts.next(), parts.next(), parts.next())
    else {
        return;
    };
    let (Ok(opener), Ok(chat)) = (opener.parse::<i64>(), chat.parse::<i64>()) else {
        return;
    };
    if query.sender_id().bare_id() != Some(opener) {
        let _ = query
            .answer()
            .alert("این پنل را شخص دیگری باز کرده است. خودتان «پنل» را بفرستید.")
            .send()
            .await;
        return;
    }

    if let Some(id) = action.strip_prefix("in:") {
        let Some((found, (min, max))) = setting::number(id) else {
            return;
        };
        if let Some(user) = query.sender_id().bare_id() {
            ctx.expect_number(chat, user, found.id);
        }
        let _ = query
            .answer()
            .alert(format!("{} را بفرستید ({min} تا {max}).", found.label))
            .send()
            .await;
        return;
    }

    if let Some(rest) = action.strip_prefix("l:") {
        list_callback(ctx, query, rest, chat, opener).await;
        return;
    }

    let action = match setting::apply(ctx, chat, action).await {
        Some(section) => section,
        None => action,
    };

    let (title, markup): (String, ReplyMarkup) = match action {
        "root" => (ROOT_TITLE.to_owned(), root_markup(ctx, chat, opener)),
        "locks" => (locks_title(0), locks_markup(ctx, chat, opener, 0)),
        "adv" => (ADVANCED_TITLE.to_owned(), advanced_markup(ctx, chat, opener)),
        "sec" => (
            "<b>پنل مدیریت</b> › <b>امنیت و ورود</b>\n\nچه کسی بنویسد، و با متخلف چه شود."
                .to_owned(),
            security_markup(ctx, chat, opener),
        ),
        "msg" => (
            "<b>پنل مدیریت</b> › <b>پیام و پاسخ</b>\n\nربات چه بگوید و به که."
                .to_owned(),
            messages_markup(ctx, chat, opener),
        ),
        "tm" => (
            "<b>پنل مدیریت</b> › <b>پاکسازی و زمان</b>\n\nکارهایی که سر ساعت انجام می شوند."
                .to_owned(),
            timing_markup(ctx, chat, opener),
        ),
        "tmed" => (
            temp_media_title(ctx, chat),
            temp_media_markup(ctx, chat, opener),
        ),

        picked if picked.starts_with("tmed:") => {
            if let Some(kind) = tempmedia::find(&picked["tmed:".len()..]) {
                let keep = tempmedia::temporary(ctx, chat, kind);
                ctx.settings.set(chat, kind.key, keep).await;
            }
            (
                temp_media_title(ctx, chat),
                temp_media_markup(ctx, chat, opener),
            )
        }
        "ls" => (LISTS_TITLE.to_owned(), lists_markup(chat, opener)),
        "s" => (strict_title(ctx, chat), strict_markup(ctx, chat, opener)),
        "rd" => (raid_title(ctx, chat), raid_markup(ctx, chat, opener)),
        "sp" => (
            strict_picks_title(ctx, chat),
            strict_picks_markup(ctx, chat, opener),
        ),
        "bt" => (betrayal_title(ctx, chat), betrayal_markup(ctx, chat, opener)),
        "fl" => (flood_title(ctx, chat), flood_markup(ctx, chat, opener)),
        "wn" => (warns_title(ctx, chat), warns_markup(ctx, chat, opener)),
        "cp" => (captcha_title(ctx, chat), captcha_markup(ctx, chat, opener)),
        "nt" => (notice_title(ctx, chat), notice_markup(ctx, chat, opener)),
        "an" => (answers_title(ctx, chat), answers_markup(ctx, chat, opener)),
        "wc" => (welcome_title(ctx, chat), welcome_markup(ctx, chat, opener)),
        "ng" => (night_title(ctx, chat), night_markup(ctx, chat, opener)),
        "ngf" => (night_title(ctx, chat), clock_markup(ctx, chat, opener, true)),
        "ngt" => (night_title(ctx, chat), clock_markup(ctx, chat, opener, false)),
        "ng_toggle" => {
            let window = super::extras::night(ctx, chat);

            super::extras::set_night(ctx, chat, window.is_none().then_some((23 * 60, 7 * 60)))
                .await;
            (night_title(ctx, chat), night_markup(ctx, chat, opener))
        }
        part if part.starts_with("ngfh:")
            || part.starts_with("ngfm:")
            || part.starts_with("ngth:")
            || part.starts_with("ngtm:") =>
        {
            let (from, to) = super::extras::night(ctx, chat).unwrap_or((23 * 60, 7 * 60));
            let editing_start = part.starts_with("ngf");
            let is_hour = part[3..4].starts_with('h');
            if let Ok(value) = part[5..].parse::<u32>() {
                let current = if editing_start { from } else { to };
                let updated = if is_hour {
                    (value % 24) * 60 + current % 60
                } else {
                    (current / 60) * 60 + value.min(59)
                };
                let window = if editing_start {
                    (updated, to)
                } else {
                    (from, updated)
                };
                super::extras::set_night(ctx, chat, Some(window)).await;
            }
            (
                night_title(ctx, chat),
                clock_markup(ctx, chat, opener, editing_start),
            )
        }
        "sl" => (slow_title(ctx, chat), slow_markup(ctx, chat, opener)),
        step if step.starts_with("sl:") => {
            if let Ok(seconds) = step[3..].parse::<u32>() {
                super::extras::apply_slow(ctx, chat, seconds).await;
            }
            (slow_title(ctx, chat), slow_markup(ctx, chat, opener))
        }
        "jn" => (join_title(ctx, chat), join_markup(ctx, chat, opener)),
        "ad" => (adds_title(ctx, chat), adds_markup(ctx, chat, opener)),
        "gp" => (prompt_title(ctx, chat), prompt_markup(ctx, chat, opener)),
        "gr" => (
            super::rights::status(ctx, chat),
            rights_markup(ctx, chat, opener),
        ),
        part if part.starts_with("gr:") => {
            let right = &part[3..];
            if super::rights::RIGHTS.iter().any(|r| r.key == right) {
                let shut = !super::rights::closed(ctx, chat, right);
                super::rights::set_closed(ctx, chat, right, shut).await;

                if let Some(chat_ref) = ctx.chat_ref(chat) {
                    super::rights::apply(ctx, chat_ref, chat, false).await;
                }
            }
            (
                super::rights::status(ctx, chat),
                rights_markup(ctx, chat, opener),
            )
        }
        "lg" => (log::status(ctx, chat), log_markup(ctx, chat, opener)),
        "dr" => (report_title(ctx, chat), report_markup(ctx, chat, opener)),
        "ap" => (auto_title(ctx, chat), auto_markup(ctx, chat, opener)),
        "ap_toggle" => {
            let now_on = super::purge::auto_at(ctx, chat).is_none();
            super::purge::set_auto_at(
                ctx,
                chat,
                now_on.then_some(super::purge::AUTO_DEFAULT_AT),
            )
            .await;
            (auto_title(ctx, chat), auto_markup(ctx, chat, opener))
        }

        "dr_toggle" => {
            let now_on = super::stats::report_at(ctx, chat).is_none();
            super::stats::set_report_at(
                ctx,
                chat,
                now_on.then_some(super::stats::REPORT_DEFAULT),
            )
            .await;
            (report_title(ctx, chat), report_markup(ctx, chat, opener))
        }

        "dr_now" => {
            let body = super::stats::daily_body(ctx, chat, super::stats::today()).await;
            let _ = query.answer().send().await;
            if let Some(chat_ref) = ctx.chat_ref(chat) {
                let _ = ctx
                    .client
                    .send_message(chat_ref, InputMessage::new().html(body))
                    .await;
            }
            (report_title(ctx, chat), report_markup(ctx, chat, opener))
        }
        part if part.starts_with("lg:") => {
            let key = &part[3..];
            if log::KINDS.iter().any(|(k, _)| *k == key) {
                let now_on = !ctx.settings.is_locked(chat, key);
                ctx.settings.set(chat, key, now_on).await;
            }
            (log::status(ctx, chat), log_markup(ctx, chat, opener))
        }
        "lg_off" => {
            ctx.settings.set_value(chat, log::CHANNEL, "").await;
            ctx.settings.set(chat, log::ON, false).await;
            (log::status(ctx, chat), log_markup(ctx, chat, opener))
        }
        "jn_off" => {
            join::set_channel(ctx, chat, "").await;
            (join_title(ctx, chat), join_markup(ctx, chat, opener))
        }
        "wc_off" => {
            ctx.settings.set_value(chat, welcome::TEXT, "").await;
            ctx.settings.set_value(chat, welcome::MEDIA, "").await;
            (welcome_title(ctx, chat), welcome_markup(ctx, chat, opener))
        }
        pick if pick.starts_with("sp:") => {
            strict_pick(ctx, chat, &pick[3..]).await;
            (
                strict_picks_title(ctx, chat),
                strict_picks_markup(ctx, chat, opener),
            )
        }
        "on" | "off" => {
            let on = action == "on";
            for lock in LOCKS {
                ctx.settings.set(chat, lock.key, on).await;
                strict::sync_pick(ctx, chat, lock.key, on).await;
            }
            (locks_title(0), locks_markup(ctx, chat, opener, 0))
        }
        "close" => {
            let _ = query
                .answer()
                .edit(InputMessage::new().html(summary(ctx, chat)))
                .await;
            return;
        }

        page if page.starts_with("page:") => {
            let page = page[5..].parse::<usize>().unwrap_or(0).min(last_page());
            (locks_title(page), locks_markup(ctx, chat, opener, page))
        }
        key => {
            let (key, page) = match key.split_once(':') {
                Some((key, page)) => (key, page.parse::<usize>().unwrap_or(0)),
                None => (key, 0),
            };
            let Some(lock) = LOCKS.iter().find(|lock| lock.key == key) else {
                return;
            };
            let now_on = !ctx.settings.is_locked(chat, lock.key);
            ctx.settings.set(chat, lock.key, now_on).await;
            strict::sync_pick(ctx, chat, lock.key, now_on).await;
            let page = page.min(last_page());
            (locks_title(page), locks_markup(ctx, chat, opener, page))
        }
    };

    let _ = query
        .answer()
        .edit(InputMessage::new().html(title).reply_markup(markup))
        .await;
}

async fn list_callback(ctx: &Ctx, query: &CallbackQuery, rest: &str, chat: i64, opener: i64) {
    let (kind_name, entry_key) = match rest.split_once(':') {
        Some((kind, key)) => (kind, Some(key)),
        None => (rest, None),
    };
    let Some(kind) = lists::Kind::from_action(kind_name) else {
        return;
    };
    let Ok(Some(chat_ref)) = query.peer_ref().await else {
        return;
    };

    let (title, markup) = match entry_key.and_then(|key| lists::clearing(key).map(|c| (key, c))) {
        Some((_, false)) => lists::confirm_clear(ctx, chat_ref, chat, kind, opener).await,
        Some((_, true)) => {
            lists::clear_all(ctx, chat_ref, chat, kind).await;
            lists::view(ctx, chat_ref, chat, kind, opener).await
        }
        None => {
            if let Some(entry_key) = entry_key {
                lists::remove(ctx, chat_ref, chat, kind, entry_key).await;
            }
            lists::view(ctx, chat_ref, chat, kind, opener).await
        }
    };
    let _ = query
        .answer()
        .edit(InputMessage::new().html(title).reply_markup(markup))
        .await;
}

fn root_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let active = LOCKS
        .iter()
        .filter(|lock| ctx.settings.is_locked(chat, lock.key))
        .count();
    ReplyMarkup::from_buttons(&[
        vec![Button::data(
            format!("🔒  قفل ها  ({active} از {})  ›", LOCKS.len()),
            payload(opener, chat, "locks"),
        )],
        vec![toggle(
            "🔥  حالت سختگیرانه  ›",
            payload(opener, chat, "s"),
            ctx.settings.is_locked(chat, strict::MODE),
        )],
        vec![Button::data("⚙️  تنظیمات پیشرفته  ›", payload(opener, chat, "adv"))],
        vec![Button::data("📋  لیست ها  ›", payload(opener, chat, "ls"))],
        vec![Button::data("بستن", payload(opener, chat, "close"))],
    ])
}

fn lists_markup(chat: i64, opener: i64) -> ReplyMarkup {
    ReplyMarkup::from_buttons(&[
        vec![
            Button::data("🚫  بن شده ها", payload(opener, chat, "l:ban")),
            Button::data("🔇  سکوت شده ها", payload(opener, chat, "l:mute")),
        ],
        vec![
            Button::data("⭐  کاربران ویژه", payload(opener, chat, "l:vip")),
            Button::data("🧹  لیست فیلتر", payload(opener, chat, "l:filter")),
        ],
        vec![
            Button::data("🎫  لیست معاف", payload(opener, chat, "l:free")),
            Button::data("💬  لیست پاسخ", payload(opener, chat, "l:answer")),
        ],
        vec![Button::data("‹ بازگشت", payload(opener, chat, "root"))],
    ])
}

fn raid_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>ضد هجوم</b>\n\n\
         اگر بیش از <b>{}</b> نفر در <b>{}</b> ثانیه وارد شوند، تازه واردها <b>{}</b> سکوت می شوند.\n\n\
         <i>برای عدد دلخواه دکمه پایین هر ردیف را بزنید.</i>",
        raid::limit(ctx, chat),
        raid::window(ctx, chat),
        strict::time_label(raid::minutes(ctx, chat)),
    )
}

fn raid_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(ctx, chat, opener, &["rd_on", "rd_lim", "rd_win", "rd_time"], "sec")
}

fn strict_causes(ctx: &Ctx, chat: i64) -> Vec<(&'static str, &'static str)> {
    let mut causes: Vec<(&str, &str)> = LOCKS
        .iter()
        .filter(|lock| lock.key != super::locks::SERVICE)
        .filter(|lock| ctx.settings.is_locked(chat, lock.key))
        .map(|lock| (lock.key, lock.names[0]))
        .collect();
    if !ctx.settings.indexed_empty(chat, "filter:") {
        causes.push((strict::FILTER, "کلمه فیلتر"));
    }
    if !ctx.settings.indexed_empty(chat, "pack:") {
        causes.push((strict::PACK, "پک استیکر"));
    }
    causes
}

fn strict_picks_title(ctx: &Ctx, chat: i64) -> String {
    let causes = strict_causes(ctx, chat);
    if causes.is_empty() {
        return "<b>پنل مدیریت</b> › <b>موارد تخلف</b>\n\n\
                هیچ قفلی روشن نیست. اول از «قفل ها» یکی را روشن کنید."
            .to_owned();
    }
    let all = ctx.settings.indexed_empty(chat, strict::PICK);
    let chosen = causes
        .iter()
        .filter(|(key, _)| all || ctx.settings.is_locked(chat, &strict::pick_key(key)))
        .count();
    format!(
        "<b>پنل مدیریت</b> › <b>موارد تخلف</b>\n\n\
         فقط موارد ✓ به شمارش تخلف اضافه می شوند · <b>{chosen}</b> از <b>{}</b>",
        causes.len()
    )
}

fn strict_picks_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let all = ctx.settings.indexed_empty(chat, strict::PICK);
    let mut rows: Vec<Vec<Button>> = strict_causes(ctx, chat)
        .chunks(2)
        .map(|pair| {
            pair.iter()
                .map(|(key, name)| {
                    let on = all || ctx.settings.is_locked(chat, &strict::pick_key(key));
                    toggle(
                        format!("{}  {name}", if on { "✓" } else { "✗" }),
                        payload(opener, chat, &format!("sp:{key}")),
                        on,
                    )
                })
                .collect()
        })
        .collect();
    rows.push(vec![Button::data("‹ بازگشت", payload(opener, chat, "s"))]);
    ReplyMarkup::from_buttons(&rows)
}

async fn strict_pick(ctx: &Ctx, chat: i64, cause: &str) {
    let causes = strict_causes(ctx, chat);
    if !causes.iter().any(|(key, _)| *key == cause) {
        return;
    }
    if ctx.settings.indexed_empty(chat, strict::PICK) {
        for (key, _) in &causes {
            ctx.settings.set(chat, &strict::pick_key(key), true).await;
        }
    }
    let key = strict::pick_key(cause);
    if !ctx.settings.is_locked(chat, &key) {
        ctx.settings.set(chat, &key, true).await;
        return;
    }

    ctx.settings.set(chat, &key, false).await;
    if ctx.settings.indexed_empty(chat, strict::PICK) {
        ctx.settings.set(chat, &key, true).await;
    }
}

fn strict_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows = rows_for(ctx, chat, opener, "strict");
    rows.push(vec![Button::data(
        "\u{1F512}  موارد تخلف  \u{203A}",
        payload(opener, chat, "sp"),
    )]);
    for id in ["s_lim", "s_time", "s_act"] {
        rows.extend(rows_for(ctx, chat, opener, id));
    }
    rows.push(vec![Button::data("\u{2039} بازگشت", payload(opener, chat, "root"))]);
    ReplyMarkup::from_buttons(&rows)
}

fn betrayal_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>ضد خیانت ادمین</b>\n\n\
         اگر ادمینی در {} دقیقه بیش از {} نفر را حذف کند، خودش {} می شود.",
        betrayal::window(ctx, chat),
        betrayal::limit(ctx, chat),
        if betrayal::bans(ctx, chat) {
            "عزل و بن"
        } else {
            "عزل"
        }
    )
}

fn betrayal_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(ctx, chat, opener, &["bt_on", "bt_lim", "bt_win", "bt_act"], "sec")
}

fn flood_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>ضد رگبار</b>

\
         بیش از <b>{}</b> پیام در <b>{}</b> ثانیه · {}

\
         <i>برای عدد دلخواه: «ضد رگبار 10 5»</i>",
        flood::limit(ctx, chat),
        flood::window(ctx, chat),
        if flood::bans(ctx, chat) { "بن" } else { "سکوت" },
    )
}

fn flood_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(ctx, chat, opener, &["fl_on", "fl_lim", "fl_win", "fl_act"], "sec")
}

fn night_title(ctx: &Ctx, chat: i64) -> String {
    match super::extras::night(ctx, chat) {
        Some((from, to)) => format!(
            "<b>پنل مدیریت</b> › <b>قفل شب</b>

\
             هر شب از <b>{}</b> تا <b>{}</b> گروه بسته می شود (به وقت تهران).

\
             <i>با دستور: «قفل شب 23:30 تا 7»</i>",
            super::extras::clock(from),
            super::extras::clock(to)
        ),
        None => "<b>پنل مدیریت</b> › <b>قفل شب</b>

\
                 خاموش است. ساعت شروع و پایان را انتخاب کنید.

\
                 <i>با دستور: «قفل شب 23:30 تا 7»</i>"
            .to_owned(),
    }
}

fn night_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let (from, to) = super::extras::night(ctx, chat).unwrap_or((23 * 60, 7 * 60));
    let on = super::extras::night(ctx, chat).is_some();
    ReplyMarkup::from_buttons(&[
        vec![
            Button::data(
                format!("شروع · {}", super::extras::clock(from)),
                payload(opener, chat, "ngf"),
            ),
            Button::data(
                format!("پایان · {}", super::extras::clock(to)),
                payload(opener, chat, "ngt"),
            ),
        ],
        vec![toggle(
            if on { "✓ روشن" } else { "✗ خاموش" },
            payload(opener, chat, "ng_toggle"),
            on,
        )],
        vec![Button::data("‹ بازگشت", payload(opener, chat, "tm"))],
    ])
}

fn clock_markup(ctx: &Ctx, chat: i64, opener: i64, editing_start: bool) -> ReplyMarkup {
    let (from, to) = super::extras::night(ctx, chat).unwrap_or((23 * 60, 7 * 60));
    let current = if editing_start { from } else { to };
    let (hour, minute) = (current / 60, current % 60);
    let key = if editing_start { "ngf" } else { "ngt" };

    let mut rows: Vec<Vec<Button>> = (0..24)
        .collect::<Vec<u32>>()
        .chunks(6)
        .map(|block| {
            block
                .iter()
                .map(|&h| {
                    choice(
                        format!("{h:02}"),
                        payload(opener, chat, &format!("{key}h:{h}")),
                        h == hour,
                    )
                })
                .collect()
        })
        .collect();
    rows.push(
        [0, 15, 30, 45]
            .iter()
            .map(|&m| {
                choice(
                    format!(":{m:02}"),
                    payload(opener, chat, &format!("{key}m:{m}")),
                    m == minute,
                )
            })
            .collect(),
    );
    rows.push(custom_row(opener, chat, key, super::extras::clock(current)));
    rows.push(vec![Button::data(
        "‹ بازگشت",
        payload(opener, chat, "ng"),
    )]);
    ReplyMarkup::from_buttons(&rows)
}

fn slow_title(ctx: &Ctx, chat: i64) -> String {
    let current = ctx
        .settings
        .value(chat, super::extras::SLOW_STATE)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    format!(
        "<b>پنل مدیریت</b> › <b>اسلوموشن</b>

\
         فاصله مجاز بین پیام های هر کاربر · <b>{}</b>

\
         <i>با دستور: «اسلوموشن 30»</i>",
        super::extras::slow_label(current)
    )
}

fn slow_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let current = ctx
        .settings
        .value(chat, super::extras::SLOW_STATE)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let mut rows: Vec<Vec<Button>> = super::extras::SLOW_STEPS
        .chunks(3)
        .map(|block| {
            block
                .iter()
                .map(|&seconds| {
                    choice(
                        super::extras::slow_label(seconds),
                        payload(opener, chat, &format!("sl:{seconds}")),
                        seconds == current,
                    )
                })
                .collect()
        })
        .collect();
    rows.push(vec![Button::data("‹ بازگشت", payload(opener, chat, "tm"))]);
    ReplyMarkup::from_buttons(&rows)
}

fn welcome_title(ctx: &Ctx, chat: i64) -> String {
    let text = ctx.settings.value(chat, welcome::TEXT).unwrap_or_default();
    let has_media = ctx
        .settings
        .value(chat, welcome::MEDIA)
        .is_some_and(|media| !media.is_empty());
    let preview = if text.is_empty() && !has_media {
        "خاموش است.".to_owned()
    } else {
        format!(
            "{}{}",
            if text.is_empty() {
                "‹ بدون متن".to_owned()
            } else {
                format!("‹ {}", super::esc(text.chars().take(120).collect::<String>().as_str()))
            },
            if has_media { "\n‹ همراه با رسانه" } else { "" }
        )
    };
    format!(
        "<b>پنل مدیریت</b> › <b>خوشامد</b>\n\n{preview}\n\n\
         <b>تگ ها</b>\n\
         <code>{{نام}}</code> · <code>{{منشن}}</code> · <code>{{آیدی}}</code> · \
         <code>{{یوزرنیم}}</code> · <code>{{گروه}}</code>\n\n\
         <i>تنظیم: روی یک پیام ریپلای کنید و «تنظیم خوشامد» بفرستید.</i>"
    )
}

fn welcome_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let on = ctx
        .settings
        .value(chat, welcome::TEXT)
        .is_some_and(|text| !text.is_empty())
        || ctx
            .settings
            .value(chat, welcome::MEDIA)
            .is_some_and(|media| !media.is_empty());
    ReplyMarkup::from_buttons(&[
        vec![toggle(
            format!("{}  خوشامد", if on { "✓ روشن" } else { "✗ خاموش" }),
            payload(opener, chat, "wc"),
            on,
        )],
        vec![coloured(
            "حذف خوشامد",
            payload(opener, chat, "wc_off"),
            Colour::Danger,
        )],
        vec![Button::data("‹ بازگشت", payload(opener, chat, "msg"))],
    ])
}

fn auto_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>پاکسازی خودکار</b>\n\n{}\n\n\
         <i>هر روز در ساعت تعیین شده، پیام های قدیمی تر پاک می شوند. \
         با کلینر، پیام های کهنه هم پاک می شوند.</i>",
        match super::purge::auto_at(ctx, chat) {
            Some(at) => match super::purge::auto_count(ctx, chat) {
                0 => format!(
                    "هر روز ساعت <b>{}</b> · <b>همه</b> پیام های گروه پاک می شود.",
                    super::extras::clock(at)
                ),
                count => format!(
                    "هر روز ساعت <b>{}</b> · <b>{count}</b> پیام آخر پاک می شود.",
                    super::extras::clock(at)
                ),
            },
            None => "خاموش است.".to_owned(),
        }
    )
}

fn auto_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let at = super::purge::auto_at(ctx, chat);
    let count = super::purge::auto_count(ctx, chat);
    let mut rows = vec![vec![toggle(
        match at {
            Some(at) => format!("✓  روشن · هر روز {}", super::extras::clock(at)),
            None => "✗  خاموش".to_owned(),
        },
        payload(opener, chat, "ap_toggle"),
        at.is_some(),
    )]];
    rows.push(vec![Button::data("ساعت پاکسازی", payload(opener, chat, "ap"))]);
    rows.extend(super::purge::AUTO_AT_PRESETS.chunks(3).map(|chunk| {
        chunk
            .iter()
            .map(|&value| {
                choice(
                    super::extras::clock(value),
                    payload(opener, chat, &format!("apt:{value}")),
                    at == Some(value),
                )
            })
            .collect()
    }));
    rows.push(vec![Button::data(
        "چند پیام هر بار",
        payload(opener, chat, "ap"),
    )]);
    rows.extend(super::purge::AUTO_COUNT_PRESETS.chunks(3).map(|chunk| {
        chunk
            .iter()
            .map(|&value| {
                choice(
                    value.to_string(),
                    payload(opener, chat, &format!("apc:{value}")),
                    count == value,
                )
            })
            .collect()
    }));
    rows.push(custom_row(opener, chat, "apc", count));
    rows.push(vec![Button::data("‹ بازگشت", payload(opener, chat, "tm"))]);
    ReplyMarkup::from_buttons(&rows)
}

fn report_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › {}",
        super::stats::report_status(ctx, chat)
    )
}

fn report_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let current = super::stats::report_at(ctx, chat);
    let mut rows = vec![vec![toggle(
        match current {
            Some(at) => format!("✓  روشن · هر روز {}", super::extras::clock(at)),
            None => "✗  خاموش".to_owned(),
        },
        payload(opener, chat, "dr_toggle"),
        current.is_some(),
    )]];

    rows.extend(super::stats::REPORT_PRESETS.chunks(3).map(|chunk| {
        chunk
            .iter()
            .map(|&at| {
                choice(
                    super::extras::clock(at),
                    payload(opener, chat, &format!("dr:{at}")),
                    current == Some(at),
                )
            })
            .collect()
    }));
    rows.push(custom_row(
        opener,
        chat,
        "dr",
        match current {
            Some(at) => super::extras::clock(at),
            None => "✗".to_owned(),
        },
    ));
    rows.push(vec![Button::data(
        "📤  ارسال آزمایشی",
        payload(opener, chat, "dr_now"),
    )]);
    rows.push(vec![Button::data("‹ بازگشت", payload(opener, chat, "tm"))]);
    ReplyMarkup::from_buttons(&rows)
}

pub fn rights_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = super::rights::RIGHTS
        .iter()
        .map(|right| {
            let open = !super::rights::closed(ctx, chat, right.key);
            vec![toggle(
                format!(
                    "{}  ·  {}",
                    right.label,
                    if open { "باز" } else { "بسته" }
                ),
                payload(opener, chat, &format!("gr:{}", right.key)),
                open,
            )]
        })
        .collect();
    rows.push(vec![Button::data("‹ بازگشت", payload(opener, chat, "adv"))]);
    ReplyMarkup::from_buttons(&rows)
}

fn log_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = log::KINDS
        .iter()
        .map(|(key, label)| {
            let on = ctx.settings.is_locked(chat, key);
            vec![toggle(
                format!("{}  {label}", if on { "✓" } else { "✗" }),
                payload(opener, chat, &format!("lg:{key}")),
                on,
            )]
        })
        .collect();
    if log::channel_id(ctx, chat).is_some() {
        rows.push(vec![coloured(
            "حذف کانال لاگ",
            payload(opener, chat, "lg_off"),
            Colour::Danger,
        )]);
    }
    rows.push(vec![Button::data("‹ بازگشت", payload(opener, chat, "adv"))]);
    ReplyMarkup::from_buttons(&rows)
}

fn prompt_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>اعلان شرط</b>\n\n\
         پیامی که به کسی که شرط ورود را انجام نداده نشان داده می شود.\n\n\
         فاصله بین دو اعلان · <b>{}</b>\n\
         حذف خودکار اعلان · <b>{}</b>\n\n\
         <i>عدد دقیق با دستور: «تنظیم اعلان شرط 120 30»</i>",
        join::seconds_label(join::prompt_every(ctx, chat), "هر بار"),
        join::seconds_label(join::prompt_ttl(ctx, chat), "بدون حذف"),
    )
}

fn prompt_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(ctx, chat, opener, &["gpe", "gpt"], "sec")
}

fn adds_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>اد اجباری</b>\n\n{}\n\n\
         <i>عدد دقیق با دستور: «تنظیم اد اجباری 7»</i>",
        match join::required_adds(ctx, chat) {
            0 => "خاموش است.".to_owned(),
            n => format!("هر عضو باید <b>{n}</b> نفر اضافه کند تا بتواند بنویسد."),
        }
    )
}

fn adds_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows = rows_for(ctx, chat, opener, "ad");
    rows.push(vec![
        Button::data("\u{1F4E3}  اعلان شرط", payload(opener, chat, "gp")),
        Button::data("\u{1F3AB}  لیست معاف", payload(opener, chat, "l:free")),
    ]);
    rows.push(vec![Button::data("\u{2039} بازگشت", payload(opener, chat, "sec"))]);
    ReplyMarkup::from_buttons(&rows)
}

fn join_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>عضویت اجباری</b>\n\n{}\n\n\
         <i>تنظیم: «تنظیم عضویت اجباری @channel» را در گروه بفرستید. \
         ربات باید در آن کانال ادمین باشد.</i>",
        match join::channel(ctx, chat) {
            Some(name) => format!("کانال · @{}", super::esc(&name)),
            None => "خاموش است.".to_owned(),
        }
    )
}

fn join_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let on = join::channel(ctx, chat).is_some();
    ReplyMarkup::from_buttons(&[
        vec![toggle(
            format!("{}  عضویت اجباری", if on { "✓ روشن" } else { "✗ خاموش" }),
            payload(opener, chat, "jn"),
            on,
        )],
        vec![coloured(
            "حذف عضویت اجباری",
            payload(opener, chat, "jn_off"),
            Colour::Danger,
        )],
        vec![
            Button::data("📣  اعلان شرط", payload(opener, chat, "gp")),
            Button::data("🎫  لیست معاف", payload(opener, chat, "l:free")),
        ],
        vec![Button::data("‹ بازگشت", payload(opener, chat, "sec"))],
    ])
}

fn answers_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>پاسخ خودکار</b>\n\n\
         <b>{}</b> پاسخ ذخیره شده · مخاطب: <b>{}</b>\n\n\
         <i>افزودن: روی پیام ریپلای کنید و «تنظیم پاسخ سلام» بفرستید.</i>",
        answers::triggers(ctx, chat).len(),
        answers::audience(ctx, chat).label()
    )
}

fn answers_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows = rows_for(ctx, chat, opener, "an_act");
    rows.push(vec![Button::data("لیست پاسخ ها", payload(opener, chat, "l:answer"))]);
    rows.push(vec![Button::data("\u{2039} بازگشت", payload(opener, chat, "msg"))]);
    ReplyMarkup::from_buttons(&rows)
}

fn notice_title(ctx: &Ctx, chat: i64) -> String {
    let ttl = notice::ttl(ctx, chat);
    format!(
        "<b>پنل مدیریت</b> › <b>اعلان حذف</b>\n\n\
         پس از حذف پیام قفل شده، فرستنده تگ می شود و دلیلش گفته می شود.\n\
         پاک شدن خودکار اعلان · <b>{}</b>\n\n\
         <i>با دستور: «تنظیم اعلان 15»</i>",
        if ttl == 0 {
            "بدون پاک شدن".to_owned()
        } else {
            format!("{ttl} ثانیه")
        }
    )
}

fn notice_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(ctx, chat, opener, &["nt_on", "nt_t"], "msg")
}

fn captcha_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>احراز هویت</b>\n\n\
         تازه واردها تا انتخاب ایموجی درست در سکوت می مانند.\n\
         مهلت <b>{}</b> ثانیه · <b>{}</b> گزینه · پس از آن {}",
        captcha::timeout(ctx, chat),
        captcha::choices(ctx, chat),
        if captcha::kicks(ctx, chat) {
            "اخراج"
        } else {
            "در سکوت می ماند"
        }
    )
}

fn captcha_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(ctx, chat, opener, &["cp_on", "cp_t", "cp_n", "cp_act"], "sec")
}

fn warns_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>اخطار</b>\n\n\
         با <b>{}</b> اخطار · {}\n\n\
         <i>دستورها: «اخطار» ، «حذف اخطار» ، «اخطارها»</i>",
        warns::limit(ctx, chat),
        if warns::bans(ctx, chat) { "اخراج" } else { "سکوت" },
    )
}

fn warns_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(ctx, chat, opener, &["wn_lim", "wn_act"], "sec")
}

fn section(label: &str, target: Vec<u8>, on: bool) -> Button {
    toggle(
        format!("{label}  ›  {}", if on { "✓" } else { "✗" }),
        target,
        on,
    )
}

fn advanced_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    ReplyMarkup::from_buttons(&[
        vec![Button::data("🛡  امنیت و ورود  ›", payload(opener, chat, "sec"))],
        vec![Button::data("💬  پیام و پاسخ  ›", payload(opener, chat, "msg"))],
        vec![Button::data("🧹  پاکسازی و زمان  ›", payload(opener, chat, "tm"))],
        vec![Button::data("🛂  اختیارات گروه  ›", payload(opener, chat, "gr"))],
        vec![section(
            "🧾  کانال لاگ",
            payload(opener, chat, "lg"),
            log::channel_id(ctx, chat).is_some(),
        )],
        vec![
            Button::data("‹ بازگشت", payload(opener, chat, "root")),
            Button::data("بستن", payload(opener, chat, "close")),
        ],
    ])
}

fn security_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let adds = join::required_adds(ctx, chat);
    ReplyMarkup::from_buttons(&[
        vec![section(
            "🔗  عضویت اجباری",
            payload(opener, chat, "jn"),
            join::channel(ctx, chat).is_some(),
        )],
        vec![toggle(
            format!(
                "➕  اد اجباری  ›  {}",
                match adds {
                    0 => "✗".to_owned(),
                    n => format!("✓ {n}"),
                }
            ),
            payload(opener, chat, "ad"),
            adds > 0,
        )],
        vec![section(
            "🧩  احراز هویت",
            payload(opener, chat, "cp"),
            ctx.settings.is_locked(chat, captcha::MODE),
        )],
        vec![section(
            "⚡  ضد رگبار",
            payload(opener, chat, "fl"),
            ctx.settings.is_locked(chat, flood::MODE),
        )],
        vec![section(
            "🚨  ضد هجوم",
            payload(opener, chat, "rd"),
            ctx.settings.is_locked(chat, raid::MODE),
        )],
        vec![section(
            "🛡  ضد خیانت ادمین",
            payload(opener, chat, "bt"),
            ctx.settings.is_locked(chat, betrayal::MODE),
        )],
        vec![Button::data("⚠️  اخطار  ›", payload(opener, chat, "wn"))],
        vec![Button::data("‹ بازگشت", payload(opener, chat, "adv"))],
    ])
}

fn messages_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let greets = ctx
        .settings
        .value(chat, welcome::TEXT)
        .is_some_and(|text| !text.is_empty());
    ReplyMarkup::from_buttons(&[
        vec![section("👋  خوشامد", payload(opener, chat, "wc"), greets)],
        vec![section(
            "📣  اعلان حذف",
            payload(opener, chat, "nt"),
            ctx.settings.is_locked(chat, notice::MODE),
        )],
        vec![Button::data("💬  پاسخ خودکار  ›", payload(opener, chat, "an"))],
        vec![toggle(
            format!(
                "🏅  مقام خودکار  ·  {}",
                if ctx.settings.is_locked(chat, super::stats::RANKS) {
                    "✓"
                } else {
                    "✗"
                }
            ),
            payload(opener, chat, "rk_on"),
            ctx.settings.is_locked(chat, super::stats::RANKS),
        )],
        vec![Button::data("‹ بازگشت", payload(opener, chat, "adv"))],
    ])
}

fn timing_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    ReplyMarkup::from_buttons(&[
        vec![
            Button::data("🌙  قفل شب  ›", payload(opener, chat, "ng")),
            Button::data("🐌  اسلوموشن  ›", payload(opener, chat, "sl")),
        ],
        vec![section(
            "🧹  پاکسازی خودکار",
            payload(opener, chat, "ap"),
            super::purge::auto_at(ctx, chat).is_some(),
        )],
        vec![section(
            "📊  گزارش روزانه",
            payload(opener, chat, "dr"),
            super::stats::report_at(ctx, chat).is_some(),
        )],
        vec![section(
            "⏳  رسانه موقت",
            payload(opener, chat, "tmed"),
            ctx.settings.is_locked(chat, tempmedia::MODE),
        )],
        vec![Button::data("‹ بازگشت", payload(opener, chat, "adv"))],
    ])
}

fn temp_media_title(ctx: &Ctx, chat: i64) -> String {
    let kept: Vec<&str> = tempmedia::KINDS
        .iter()
        .filter(|kind| !tempmedia::temporary(ctx, chat, kind))
        .map(|kind| kind.label)
        .collect();
    format!(
        "<b>پنل مدیریت</b> › <b>رسانه موقت</b>\n\n\
         وضعیت · <b>{}</b>\n\
         زمان حذف · <b>{}</b>\n\
         شامل · <b>{}</b>\n\
         بدون حذف · <b>{}</b>\n\n\
         <i>رسانه های انتخاب شده پس از این مدت خودشان حذف می شوند.</i>",
        if ctx.settings.is_locked(chat, tempmedia::MODE) {
            "روشن"
        } else {
            "خاموش"
        },
        strict::time_label(tempmedia::minutes(ctx, chat)),
        if tempmedia::reaches_everyone(ctx, chat) {
            "همه"
        } else {
            "بدون مقام"
        },
        if kept.is_empty() {
            "هیچ کدام".to_owned()
        } else {
            kept.join("، ")
        }
    )
}

fn temp_media_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows = rows_for(ctx, chat, opener, "tmed_on");
    rows.extend(tempmedia::KINDS.chunks(2).map(|pair| {
        pair.iter()
            .map(|kind| {
                let on = tempmedia::temporary(ctx, chat, kind);
                toggle(
                    format!("{}  {}", if on { "✓" } else { "✗" }, kind.label),
                    payload(opener, chat, &format!("{}:{}", tempmedia::MODE, kind.name)),
                    on,
                )
            })
            .collect()
    }));
    rows.extend(rows_for(ctx, chat, opener, "tmed_min"));
    rows.extend(rows_for(ctx, chat, opener, "tmed_who"));
    rows.push(vec![Button::data("‹ بازگشت", payload(opener, chat, "tm"))]);
    ReplyMarkup::from_buttons(&rows)
}

fn last_page() -> usize {
    LOCKS.len().div_ceil(PER_PAGE) - 1
}

fn locks_title(page: usize) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>قفل ها</b> (صفحه {} از {})\n\nهر مورد را برای تغییر بزنید.",
        page + 1,
        last_page() + 1
    )
}

fn locks_markup(ctx: &Ctx, chat: i64, opener: i64, page: usize) -> ReplyMarkup {
    let start = page * PER_PAGE;
    let shown = &LOCKS[start..(start + PER_PAGE).min(LOCKS.len())];

    let mut rows: Vec<Vec<Button>> = shown
        .chunks(2)
        .map(|pair| {
            pair.iter()
                .map(|lock| lock_button(ctx, chat, lock, opener, page))
                .collect()
        })
        .collect();

    let mut paging = Vec::new();
    if page > 0 {
        paging.push(Button::data(
            "‹ قبلی",
            payload(opener, chat, &format!("page:{}", page - 1)),
        ));
    }
    if page < last_page() {
        paging.push(Button::data(
            "بعدی ›",
            payload(opener, chat, &format!("page:{}", page + 1)),
        ));
    }
    if !paging.is_empty() {
        rows.push(paging);
    }

    rows.push(vec![
        coloured("🔒  قفل همه", payload(opener, chat, "on"), Colour::Danger),
        coloured("🔓  باز کردن همه", payload(opener, chat, "off"), Colour::Success),
    ]);
    rows.push(vec![Button::data("‹ بازگشت", payload(opener, chat, "root"))]);
    ReplyMarkup::from_buttons(&rows)
}

fn lock_button(
    ctx: &Ctx,
    chat: i64,
    lock: &super::locks::Lock,
    opener: i64,
    page: usize,
) -> Button {
    let mark = if ctx.settings.is_locked(chat, lock.key) {
        "✓"
    } else {
        "✗"
    };

    toggle(
        format!("{mark}  {}", lock.names[0]),
        payload(opener, chat, &format!("{}:{page}", lock.key)),
        ctx.settings.is_locked(chat, lock.key),
    )
}

fn summary(ctx: &Ctx, chat: i64) -> String {
    let active: Vec<&str> = LOCKS
        .iter()
        .filter(|lock| ctx.settings.is_locked(chat, lock.key))
        .map(|lock| lock.names[0])
        .collect();
    if active.is_empty() {
        format!("<b>قفل ها</b>\n\nهیچ قفلی فعال نیست ({} در دسترس).", LOCKS.len())
    } else {
        format!(
            "<b>قفل ها</b> ({} از {})\n{}",
            active.len(),
            LOCKS.len(),
            active
                .iter()
                .map(|name| format!("✓ {name}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_numbers_and_clock_times() {
        assert_eq!(parse_number("30"), Some(30));
        assert_eq!(parse_number("23:37"), Some(23 * 60 + 37));
        assert_eq!(parse_number("7.5"), Some(7 * 60 + 5));
        assert_eq!(parse_number("24:00"), None);
        assert_eq!(parse_number("12:99"), None);
        assert_eq!(parse_number("سلام"), None);
    }

    #[test]
    fn no_lock_key_shadows_a_panel_action() {
        const PAGES: &[&str] = &[
            "root", "locks", "adv", "sec", "msg", "tm", "ls", "s", "rd", "sp", "bt", "fl",
            "wn", "cp", "nt", "an", "wc", "ng", "sl", "jn", "ad", "gp", "gr", "lg", "dr",
            "ap", "tmed", "close", "page", "in", "on", "off", "ng_toggle", "ap_toggle",
            "dr_toggle", "dr_now", "lg_off", "jn_off", "wc_off",
        ];

        for declared in setting::SETTINGS {
            assert!(
                PAGES.contains(&declared.section),
                "{} redraws {}, which is not a page",
                declared.id,
                declared.section
            );
        }

        let mut taken: Vec<&str> = PAGES.to_vec();
        for declared in setting::SETTINGS {
            taken.push(declared.id);
            if let setting::Kind::Pick { options, .. } = &declared.kind {
                taken.extend(options.iter().map(|pick| pick.id));
            }
        }

        for lock in LOCKS {
            assert!(
                !taken.contains(&lock.key),
                "lock key {} collides with a panel action",
                lock.key
            );

            assert!(
                format!("p:{}:{}:{}", i64::MAX, lock.key, last_page()).len() <= 64,
                "payload too long for {}",
                lock.key
            );
        }
    }
}
