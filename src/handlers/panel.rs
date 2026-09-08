use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::update::CallbackQuery;

use crate::response;

use super::locks::{LOCKS, plain};
use super::style::{Colour, choice, data as coloured, toggle};
use super::{
    Ctx, answers, betrayal, biolink, captcha, flood, help, imgfilter, join, limits, lists, log,
    notice, raid, setting, strict, tempmedia, voicemonitor, warns, welcome,
};

pub const OPEN: &[&str] = &["پنل", "تنظیمات", "پنل ربات"];
pub const OPEN_LISTS: &[&str] = &["لیست", "لیست ها", "لیست لیست ها"];

pub const TO_PRIVATE: &[&str] = &["پنل پیوی", "پنل پی وی", "پنل خصوصی"];

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
        if strict::is_ban(ctx, chat) {
            "بن"
        } else {
            "سکوت"
        },
        strict::time_label(strict::minutes(ctx, chat)),
    )
}

async fn to_private(ctx: &Ctx, message: &Message) -> bool {
    if !super::limits::allows(ctx, message, super::limits::SET).await {
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
            super::premium::icon_html(
                Some(super::premium::Icon::Settings),
                format!(
                    "<b>پنل مدیریت</b> › <b>{}</b>\n\nبخشی را باز کنید.",
                    super::esc(&title)
                ),
            )
            .reply_markup(root_markup(ctx, chat, opener)),
        )
        .await;

    match sent {
        Ok(_) => {
            super::respond(
                ctx,
                message,
                response::ResponseKind::AdminTool,
                super::premium::icon_text(
                    Some(super::premium::Icon::TelegramSend),
                    "پنل به پیوی شما فرستاده شد.",
                ),
            )
            .await
        }
        Err(e) => {
            eprintln!("panel: {chat}: could not send private panel: {e}");
            super::respond(
                ctx,
                message,
                response::ResponseKind::CommandError,
                "ابتدا ربات را در پیوی خود استارت کنید، سپس دوباره امتحان کنید.",
            )
            .await
        }
    }
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

    const PAGE: usize = 20;

    let found = match ctx.settings.panels_for(user, PAGE as i64 + 1).await {
        Ok(found) => found,
        Err(error) => {
            ::log::warn!("panel: group lookup for {user} failed: {error}");
            let _ = message
                .reply("فهرست گروه ها خوانده نشد؛ دوباره تلاش کنید.")
                .await;
            return true;
        }
    };
    let more = found.len() > PAGE;
    let mine: Vec<(i64, String)> = found
        .into_iter()
        .take(PAGE)
        .map(|chat| {
            let name = ctx
                .settings
                .value(chat, super::TITLE)
                .unwrap_or_else(|| chat.to_string());
            (chat, name)
        })
        .collect();

    if mine.is_empty() {
        let _ = message
            .reply("گروهی برای مدیریت پیدا نشد. در گروه خود «کانفیگ» را بفرستید.")
            .await;
        return true;
    }

    let rows: Vec<Vec<Button>> = mine
        .iter()
        .map(|(chat, name)| {
            vec![Button::data(
                format!("{}  ›", super::esc(name)),
                payload(user, *chat, "root"),
            )]
        })
        .collect();
    let shown = if more {
        format!("{}+", mine.len())
    } else {
        mine.len().to_string()
    };
    let _ = message
        .reply(
            super::premium::icon_html(
                Some(super::premium::Icon::Settings),
                format!("<b>پنل مدیریت</b>\n\nگروه را انتخاب کنید ({shown})."),
            )
            .reply_markup(super::premium::buttons(&rows)),
        )
        .await;
    true
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    if TO_PRIVATE.contains(&text) {
        return to_private(ctx, message).await;
    }
    if OPEN_LISTS.contains(&text) {
        return open_lists(ctx, message).await;
    }
    if !OPEN.contains(&text) {
        return false;
    }
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let Some(opener) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };
    if let Err(error) = super::rights::seed(ctx, message, chat).await {
        ::log::warn!("panel: could not seed default rights for {chat}: {error}");
    }
    super::respond(
        ctx,
        message,
        response::ResponseKind::SettingsView,
        super::premium::icon_html(Some(super::premium::Icon::Settings), ROOT_TITLE)
            .reply_markup(root_markup(ctx, chat, opener)),
    )
    .await;
    true
}

async fn open_lists(ctx: &Ctx, message: &Message) -> bool {
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let Some(opener) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };
    super::respond(
        ctx,
        message,
        response::ResponseKind::AdminTool,
        super::premium::icon_html(Some(super::premium::Icon::ArchiveHistory), LISTS_TITLE)
            .reply_markup(lists_markup(chat, opener)),
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

fn built(ctx: &Ctx, chat: i64, opener: i64, ids: &[&str], back: &str, here: &str) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = Vec::new();
    for id in ids {
        rows.extend(rows_for(ctx, chat, opener, id));
    }
    rows.push(back_row(opener, chat, back, here));
    super::premium::buttons(&rows)
}

pub async fn typed_number(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    if !ctx.maybe_expecting_number() {
        return false;
    }
    let Some(input_chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let Some(user) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };

    let Some((target_chat, id)) = ctx.expected_number(input_chat, user) else {
        return false;
    };
    let Some((found, (min, max))) = setting::number(id) else {
        return false;
    };
    let Some(value) = parse_number(view.digits(), (min, max) == setting::CLOCK) else {
        return false;
    };
    if !ctx.take_expected_number(input_chat, user, target_chat, id) {
        return false;
    }
    let (kind, reply) = match setting::store(ctx, target_chat, found, value.clamp(min, max)).await {
        Ok(status) => {
            let suffix = match status {
                setting::ApplyStatus::Applied => "",
                setting::ApplyStatus::PendingRetry => {
                    " · ذخیره شد، تحویل به تلگرام در انتظار تلاش دوباره است"
                }
                setting::ApplyStatus::DeliveryUnknown => {
                    " · ذخیره شد، اما وضعیت تحویل به تلگرام مشخص نیست"
                }
            };
            (
                response::ResponseKind::SettingsChanged,
                super::premium::icon_text(
                    setting::number_icon(found.id).or(Some(super::premium::Icon::Success)),
                    format!(
                        "✓ {} · {}{}",
                        found.label,
                        setting::shown(ctx, target_chat, found)
                            .await
                            .unwrap_or_else(|_| "ذخیره شد".to_owned()),
                        suffix,
                    ),
                ),
            )
        }
        Err(error) if error.invalid_night_window() => (
            response::ResponseKind::CommandError,
            InputMessage::new().text("شروع و پایان قفل شب نمی تواند یکسان باشد."),
        ),
        Err(error) if error.acceptance_unknown() => {
            ::log::warn!(
                "panel: exact-value write outcome for {target_chat}/{} is unknown: {error}",
                found.key
            );
            (
                response::ResponseKind::CommandError,
                InputMessage::new().text("وضعیت ذخیره سازی مشخص نیست؛ پنل را دوباره بررسی کنید."),
            )
        }
        Err(error) => {
            ::log::warn!(
                "panel: exact value for {target_chat}/{} failed: {error}",
                found.key
            );
            (
                response::ResponseKind::CommandError,
                InputMessage::new().text("تنظیم ذخیره نشد؛ دوباره تلاش کنید."),
            )
        }
    };
    super::respond(ctx, message, kind, reply).await;
    true
}

fn parse_number(text: &str, clock: bool) -> Option<u32> {
    if clock
        && let Some((hour, minute)) = text.split_once([':', '.'])
        && let (Ok(hour), Ok(minute)) = (hour.trim().parse::<u32>(), minute.trim().parse::<u32>())
        && hour < 24
        && minute < 60
    {
        return Some(hour * 60 + minute);
    }
    text.parse().ok()
}

pub const PAGES: &[&str] = &[
    "root",
    "locks",
    "adv",
    "sec",
    "msg",
    "response",
    "tm",
    "ls",
    "s",
    "rd",
    "sp",
    "bt",
    "fl",
    "wn",
    "cp",
    "nt",
    "an",
    "wc",
    "ng",
    "sl",
    "jn",
    "ad",
    "gp",
    "gr",
    "lg",
    "dr",
    "ap",
    "tmed",
    "lim",
    "bl",
    "close",
    "page",
    "in",
    "on",
    "off",
    "ng_toggle",
    "ap_toggle",
    "dr_toggle",
    "dr_now",
    "lg_off",
    "jn_off",
    "wc_off",
    "nsw",
    "cq",
    "ai",
    "imf",
    "vw",
];

pub fn is_page(action: &str) -> bool {
    PAGES.contains(&action)
}

fn payload(opener: i64, chat: i64, action: &str) -> Vec<u8> {
    format!("p:{opener}:{chat}:{action}").into_bytes()
}

async fn edit_callback(
    ctx: &Ctx,
    query: &CallbackQuery,
    ephemeral_origin: bool,
    kind: response::ResponseKind,
    message: InputMessage,
) {
    if query.is_from_inline() {
        if let Err(error) = query.answer().edit(message).await {
            eprintln!("panel: inline callback edit failed: {error}");
        }
        return;
    }

    let chat = query.peer_id().bot_api_dialog_id().unwrap_or_default();
    let fallback_peer = query
        .peer_id()
        .bot_api_dialog_id()
        .and_then(|chat| ctx.chat_ref(chat));
    if let Err(error) = response::redraw_panel_callback(
        &ctx.client,
        query,
        chat,
        kind,
        message,
        fallback_peer,
        ephemeral_origin,
    )
    .await
    {
        eprintln!("panel: callback response failed: {error}");
    }
}

async fn night_for_panel(
    ctx: &Ctx,
    query: &CallbackQuery,
    chat: i64,
) -> Result<Option<(u32, u32)>, ()> {
    match super::extras::night(ctx, chat).await {
        Ok(window) => Ok(window),
        Err(error) => {
            ::log::warn!("panel: could not read night schedule for {chat}: {error}");
            let _ = query
                .answer()
                .alert("قفل شب خوانده نشد؛ دوباره تلاش کنید.")
                .send()
                .await;
            Err(())
        }
    }
}

fn help_payload(opener: i64, chat: i64, action: &str) -> Vec<u8> {
    format!("h:{opener}:{chat}:{action}").into_bytes()
}

pub const FROM_PANEL: &str = "p";

fn help_topic(opener: i64, chat: i64, from_panel: bool, topic: &str) -> Vec<u8> {
    let origin = if from_panel { FROM_PANEL } else { "i" };
    help_payload(opener, chat, &format!("{origin}:{topic}"))
}

fn help_origin(action: &str) -> (bool, &str) {
    match action.split_once(':') {
        Some((FROM_PANEL, topic)) => (true, topic),
        Some((_, topic)) => (false, topic),
        None => (false, action),
    }
}

pub fn back_row(opener: i64, chat: i64, back: &str, here: &str) -> Vec<Button> {
    let mut row = vec![super::premium::decorate(
        Button::data("بازگشت", payload(opener, chat, back)),
        Some(super::premium::Icon::Back),
    )];
    if help::find(here).is_some() {
        row.push(super::premium::decorate(
            Button::data("راهنما", help_topic(opener, chat, true, here)),
            Some(super::premium::Icon::Help),
        ));
    }
    row
}

pub async fn on_help(ctx: &Ctx, query: &CallbackQuery, payload_text: &str, ephemeral_origin: bool) {
    let mut parts = payload_text.splitn(3, ':');
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
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Locked),
                "این پنل را شخص دیگری باز کرده است. خودتان «پنل» را بفرستید.",
            ))
            .send()
            .await;
        return;
    }
    if action == help::CLOSE_ID {
        edit_callback(
            ctx,
            query,
            ephemeral_origin,
            response::ResponseKind::Help,
            super::premium::icon_html(
                Some(super::premium::Icon::Close),
                "راهنما بسته شد.\n\n<i>هر وقت لازم شد «راهنما» را بفرستید.</i>",
            ),
        )
        .await;
        return;
    }

    if action == help::INDEX_ID {
        edit_callback(
            ctx,
            query,
            ephemeral_origin,
            response::ResponseKind::Help,
            super::premium::html(help::index()).reply_markup(index_markup(opener, chat)),
        )
        .await;
        return;
    }

    let (from_panel, topic) = help_origin(action);
    let Some(found) = help::rendered(topic) else {
        return;
    };

    let back = match from_panel && is_page(topic) {
        true => payload(opener, chat, topic),
        false => help_payload(opener, chat, help::INDEX_ID),
    };
    let mut row = vec![super::premium::decorate(
        Button::data("بازگشت", back),
        Some(super::premium::Icon::Back),
    )];
    if from_panel {
        row.push(Button::data(
            "📖  فهرست",
            help_payload(opener, chat, help::INDEX_ID),
        ));
    } else if chat < 0
        && is_page(topic)
        && let Some(section) = help::find(topic)
    {
        row.push(super::premium::decorate(
            Button::data(
                format!("{}  ›", section.title),
                payload(opener, chat, topic),
            ),
            section.icon(),
        ));
    }
    edit_callback(
        ctx,
        query,
        ephemeral_origin,
        response::ResponseKind::Help,
        super::premium::html(found).reply_markup(super::premium::buttons(&[row])),
    )
    .await;
}

pub fn help_button(opener: i64, chat: i64) -> Button {
    super::premium::decorate(
        Button::data("راهنما", help_payload(opener, chat, help::INDEX_ID)),
        Some(super::premium::Icon::Help),
    )
}

pub fn index_markup(opener: i64, chat: i64) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = help::INDEX
        .chunks(2)
        .map(|pair| {
            pair.iter()
                .filter_map(|id| help::find(id))
                .map(|topic| {
                    super::premium::decorate(
                        Button::data(
                            format!("{}  ›", topic.title),
                            help_topic(opener, chat, false, topic.id),
                        ),
                        topic.icon(),
                    )
                })
                .collect()
        })
        .collect();
    rows.push(vec![super::premium::decorate(
        Button::data("بستن", help_payload(opener, chat, help::CLOSE_ID)),
        Some(super::premium::Icon::Close),
    )]);
    super::premium::buttons(&rows)
}

async fn apply_response_policy_action(
    ctx: &Ctx,
    chat: i64,
    action: &str,
) -> Result<Option<&'static str>, crate::state::SettingsWriteError> {
    if action == "resp_reset" {
        response::reset(&ctx.settings, chat).await?;
        return Ok(Some("response"));
    }
    let (kind, visibility) = if let Some(value) = action.strip_prefix("resp_notice:") {
        (response::ResponseKind::ContentRemovalNotice, value)
    } else if let Some(value) = action.strip_prefix("resp_welcome:") {
        (response::ResponseKind::WelcomeNotice, value)
    } else {
        return Ok(None);
    };
    let Ok(visibility) = visibility.parse::<response::VisibilityOverride>() else {
        return Ok(None);
    };
    if visibility == response::VisibilityOverride::Default {
        return Ok(None);
    }
    response::set_kind(&ctx.settings, chat, kind, visibility).await?;
    Ok(Some("response"))
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, ephemeral_origin: bool) {
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
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Locked),
                "این پنل را شخص دیگری باز کرده است. خودتان «پنل» را بفرستید.",
            ))
            .send()
            .await;
        return;
    }

    if let Some(id) = action.strip_prefix("in:") {
        let Some((found, (min, max))) = setting::number(id) else {
            return;
        };
        if let Some(user) = query.sender_id().bare_id() {
            let input_chat = query.peer_id().bot_api_dialog_id().unwrap_or(chat);
            ctx.expect_number(input_chat, user, chat, found.id);
        }
        let _ = query
            .answer()
            .alert(format!("{} را بفرستید ({min} تا {max}).", found.label))
            .send()
            .await;
        return;
    }

    if let Some(rest) = action.strip_prefix("l:") {
        list_callback(ctx, query, rest, chat, opener, ephemeral_origin).await;
        return;
    }

    if action.starts_with(limits::MODE) && super::owner(ctx, chat) != Some(opener) {
        let _ = query
            .answer()
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Locked),
                "محدودیت مدیران فقط از مالک ربات پذیرفته می شود.",
            ))
            .send()
            .await;
        return;
    }

    let action = match setting::apply(ctx, chat, action).await {
        Ok(Some((section, status))) => {
            match status {
                setting::ApplyStatus::Applied => {}
                setting::ApplyStatus::PendingRetry => {
                    let _ = query
                        .answer()
                        .alert("تغییر ذخیره شد و تحویل آن به تلگرام دوباره تلاش می شود.")
                        .send()
                        .await;
                }
                setting::ApplyStatus::DeliveryUnknown => {
                    let _ = query
                        .answer()
                        .alert("تغییر ذخیره شد، اما وضعیت تحویل آن به تلگرام مشخص نیست.")
                        .send()
                        .await;
                }
            }
            section
        }
        Ok(None) => action,
        Err(error) if error.invalid_night_window() => {
            let _ = query
                .answer()
                .alert("شروع و پایان قفل شب نمی تواند یکسان باشد.")
                .send()
                .await;
            return;
        }
        Err(error) if error.acceptance_unknown() => {
            ::log::warn!("panel: settings write outcome for {chat} is unknown: {error}");
            let _ = query
                .answer()
                .alert("وضعیت ذخیره سازی مشخص نیست؛ پنل را دوباره بررسی کنید.")
                .send()
                .await;
            return;
        }
        Err(error) => {
            ::log::warn!("panel: settings write for {chat} failed: {error}");
            let _ = query
                .answer()
                .alert("تنظیم ذخیره نشد؛ دوباره تلاش کنید.")
                .send()
                .await;
            return;
        }
    };

    if action == "resp_reset"
        || action.starts_with("resp_notice:")
        || action.starts_with("resp_welcome:")
    {
        match apply_response_policy_action(ctx, chat, action).await {
            Ok(Some(page)) => {
                let (title, markup) = response_page(ctx, chat, opener, page);
                edit_callback(
                    ctx,
                    query,
                    ephemeral_origin,
                    response::ResponseKind::AdminTool,
                    super::premium::icon_html_on(
                        super::premium::Surface::Edit,
                        Some(super::premium::Icon::Chat),
                        title,
                    )
                    .reply_markup(markup),
                )
                .await;
            }
            Ok(None) => {}
            Err(error) => {
                ::log::warn!("panel: response policy write for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert("تنظیم پیام های ربات ذخیره نشد؛ دوباره تلاش کنید.")
                    .send()
                    .await;
            }
        }
        return;
    }

    let (title, markup): (String, ReplyMarkup) = match action {
        "root" => (ROOT_TITLE.to_owned(), root_markup(ctx, chat, opener)),
        "locks" => (locks_title(0), locks_markup(ctx, chat, opener, 0)),
        "ai" => (ai_title(ctx, chat), ai_markup(ctx, chat, opener)),
        "nsw" => (nsfw_title(ctx, chat), nsfw_markup(ctx, chat, opener)),
        "cq" => (
            concepts_title(ctx, chat),
            concepts_markup(ctx, chat, opener),
        ),
        "imf" => match image_filters_page(ctx, chat, opener).await {
            Ok(page) => page,
            Err(error) => {
                ::log::warn!("panel: could not read image filters for {chat}: {error}");
                let _ = query
                    .answer()
                    .alert("فیلترهای تصویری فعلاً در دسترس نیستند؛ دوباره تلاش کنید.")
                    .send()
                    .await;
                return;
            }
        },
        "vw" => (
            voicemonitor::words_title(ctx, chat),
            voicemonitor::words_markup(ctx, chat, opener),
        ),
        part if part.starts_with("vw:") => {
            if let Err(error) = voicemonitor::remove_word(ctx, chat, &part[3..]).await {
                ::log::warn!("panel: voice-word removal for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه حذف کلمه نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "کلمه حذف نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            (
                voicemonitor::words_title(ctx, chat),
                voicemonitor::words_markup(ctx, chat, opener),
            )
        }
        part if part.starts_with("vwr:") => {
            if let Err(error) = voicemonitor::restore_word(ctx, chat, &part[4..]).await {
                ::log::warn!("panel: voice-word restore for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه بازیابی کلمه نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "کلمه بازیابی نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            (
                voicemonitor::words_title(ctx, chat),
                voicemonitor::words_markup(ctx, chat, opener),
            )
        }
        picked if picked.starts_with("imf:") => {
            if let imgfilter::Armed::Database(error) =
                imgfilter::toggle_live(ctx, chat, &picked["imf:".len()..]).await
            {
                ::log::warn!("panel: image filter toggle for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert("فیلتر تغییر نکرد؛ دوباره تلاش کنید.")
                    .send()
                    .await;
                return;
            }
            match image_filters_page(ctx, chat, opener).await {
                Ok(page) => page,
                Err(error) => {
                    ::log::warn!("panel: could not refresh image filters for {chat}: {error}");
                    let _ = query
                        .answer()
                        .alert(
                            "تغییر انجام شد، اما تازه سازی پنل ممکن نیست؛ دوباره آن را باز کنید.",
                        )
                        .send()
                        .await;
                    return;
                }
            }
        }
        "adv" => (
            ADVANCED_TITLE.to_owned(),
            advanced_markup(ctx, chat, opener),
        ),
        "sec" => (
            "<b>پنل مدیریت</b> › <b>امنیت و ورود</b>\n\nچه کسی بنویسد، و با متخلف چه شود."
                .to_owned(),
            security_markup(ctx, chat, opener),
        ),
        "msg" => (
            "<b>پنل مدیریت</b> › <b>پیام و پاسخ</b>\n\nربات چه بگوید و به که.".to_owned(),
            messages_markup(ctx, chat, opener),
        ),
        "response" => response_page(ctx, chat, opener, "response"),
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
                if let Err(error) = ctx.settings.try_set(chat, kind.key, keep).await {
                    ::log::warn!("panel: temporary media toggle for {chat} failed: {error}");
                    let _ = query
                        .answer()
                        .alert(if error.commit_outcome_unknown() {
                            "نتیجه ذخیره رسانه موقت نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                        } else {
                            "تنظیم رسانه موقت ذخیره نشد؛ دوباره تلاش کنید."
                        })
                        .send()
                        .await;
                    return;
                }
            }
            (
                temp_media_title(ctx, chat),
                temp_media_markup(ctx, chat, opener),
            )
        }
        "lim" => (limits_title(ctx, chat), limits_markup(ctx, chat, opener)),
        picked if picked.starts_with("lim:") => {
            if let Some(cap) = limits::find(&picked["lim:".len()..]) {
                let deny = !ctx.settings.is_locked(chat, cap.key);
                if let Err(error) = ctx.settings.try_set(chat, cap.key, deny).await {
                    ::log::warn!("panel: admin limit toggle for {chat} failed: {error}");
                    let _ = query
                        .answer()
                        .alert(if error.commit_outcome_unknown() {
                            "نتیجه ذخیره محدودیت مدیر نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                        } else {
                            "محدودیت مدیر ذخیره نشد؛ دوباره تلاش کنید."
                        })
                        .send()
                        .await;
                    return;
                }
            }
            (limits_title(ctx, chat), limits_markup(ctx, chat, opener))
        }
        "ls" => (LISTS_TITLE.to_owned(), lists_markup(chat, opener)),
        "s" => (strict_title(ctx, chat), strict_markup(ctx, chat, opener)),
        "rd" => (raid_title(ctx, chat), raid_markup(ctx, chat, opener)),
        "sp" => (
            strict_picks_title(ctx, chat),
            strict_picks_markup(ctx, chat, opener),
        ),
        "bt" => (
            betrayal_title(ctx, chat),
            betrayal_markup(ctx, chat, opener),
        ),
        "fl" => (flood_title(ctx, chat), flood_markup(ctx, chat, opener)),
        "bl" => (biolink_title(ctx, chat), biolink_markup(ctx, chat, opener)),
        "wn" => (warns_title(ctx, chat), warns_markup(ctx, chat, opener)),
        "cp" => (captcha_title(ctx, chat), captcha_markup(ctx, chat, opener)),
        "nt" => (notice_title(ctx, chat), notice_markup(ctx, chat, opener)),
        "an" => (answers_title(ctx, chat), answers_markup(ctx, chat, opener)),
        "wc" => (welcome_title(ctx, chat), welcome_markup(ctx, chat, opener)),
        "ng" => {
            let Ok(window) = night_for_panel(ctx, query, chat).await else {
                return;
            };
            (night_title(window), night_markup(window, chat, opener))
        }
        "ngf" => {
            let Ok(window) = night_for_panel(ctx, query, chat).await else {
                return;
            };
            (
                night_title(window),
                clock_markup(window, chat, opener, true),
            )
        }
        "ngt" => {
            let Ok(window) = night_for_panel(ctx, query, chat).await else {
                return;
            };
            (
                night_title(window),
                clock_markup(window, chat, opener, false),
            )
        }
        "ng_toggle" => {
            let Ok(window) = night_for_panel(ctx, query, chat).await else {
                return;
            };

            match super::extras::set_night(ctx, chat, window.is_none().then_some((23 * 60, 7 * 60)))
                .await
            {
                Ok(super::rights::DeliveryOutcome::Applied) => {}
                Ok(
                    super::rights::DeliveryOutcome::PendingRetry { .. }
                    | super::rights::DeliveryOutcome::Superseded,
                ) => {
                    let _ = query
                        .answer()
                        .alert("تغییر ذخیره شد و تحویل آن به تلگرام دوباره تلاش می شود.")
                        .send()
                        .await;
                }
                Ok(super::rights::DeliveryOutcome::AcceptedDeliveryUnknown { reason }) => {
                    ::log::warn!("panel: night toggle delivery for {chat} unknown: {reason}");
                    let _ = query
                        .answer()
                        .alert("تغییر ذخیره شد، اما وضعیت تحویل آن به تلگرام مشخص نیست.")
                        .send()
                        .await;
                }
                Err(error) if error.acceptance_unknown() => {
                    ::log::warn!("panel: night toggle outcome for {chat} unknown: {error}");
                    let _ = query
                        .answer()
                        .alert("وضعیت ذخیره سازی مشخص نیست؛ پنل را دوباره بررسی کنید.")
                        .send()
                        .await;
                    return;
                }
                Err(error) => {
                    ::log::warn!("panel: could not toggle night schedule for {chat}: {error}");
                    let _ = query
                        .answer()
                        .alert("قفل شب تغییر نکرد؛ دوباره تلاش کنید.")
                        .send()
                        .await;
                    return;
                }
            }
            let Ok(window) = night_for_panel(ctx, query, chat).await else {
                return;
            };
            (night_title(window), night_markup(window, chat, opener))
        }
        part if part.starts_with("ngfh:")
            || part.starts_with("ngfm:")
            || part.starts_with("ngth:")
            || part.starts_with("ngtm:") =>
        {
            let Ok(window) = night_for_panel(ctx, query, chat).await else {
                return;
            };
            let (from, to) = window.unwrap_or((23 * 60, 7 * 60));
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
                match super::extras::set_night(ctx, chat, Some(window)).await {
                    Ok(super::rights::DeliveryOutcome::Applied) => {}
                    Ok(
                        super::rights::DeliveryOutcome::PendingRetry { .. }
                        | super::rights::DeliveryOutcome::Superseded,
                    ) => {
                        let _ = query
                            .answer()
                            .alert("زمان ذخیره شد و تحویل آن به تلگرام دوباره تلاش می شود.")
                            .send()
                            .await;
                    }
                    Ok(super::rights::DeliveryOutcome::AcceptedDeliveryUnknown { reason }) => {
                        ::log::warn!("panel: night edit delivery for {chat} unknown: {reason}");
                        let _ = query
                            .answer()
                            .alert("زمان ذخیره شد، اما وضعیت تحویل آن به تلگرام مشخص نیست.")
                            .send()
                            .await;
                    }
                    Err(super::extras::NightUpdateError::InvalidWindow) => {
                        let _ = query
                            .answer()
                            .alert("شروع و پایان قفل شب نمی تواند یکسان باشد.")
                            .send()
                            .await;
                        return;
                    }
                    Err(error) if error.acceptance_unknown() => {
                        ::log::warn!("panel: night edit outcome for {chat} unknown: {error}");
                        let _ = query
                            .answer()
                            .alert("وضعیت ذخیره سازی مشخص نیست؛ پنل را دوباره بررسی کنید.")
                            .send()
                            .await;
                        return;
                    }
                    Err(error) => {
                        ::log::warn!("panel: could not edit night schedule for {chat}: {error}");
                        let _ = query
                            .answer()
                            .alert("زمان قفل شب ذخیره نشد؛ دوباره تلاش کنید.")
                            .send()
                            .await;
                        return;
                    }
                }
            }
            let Ok(window) = night_for_panel(ctx, query, chat).await else {
                return;
            };
            (
                night_title(window),
                clock_markup(window, chat, opener, editing_start),
            )
        }
        "sl" => (slow_title(ctx, chat), slow_markup(ctx, chat, opener)),
        step if step.starts_with("sl:") => {
            if let Ok(seconds) = step[3..].parse::<u32>() {
                match super::extras::apply_slow(ctx, chat, seconds).await {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        let _ = query
                            .answer()
                            .alert(super::premium::plain_label(
                                Some(super::premium::Icon::Locked),
                                "اسلوموشن فقط با کلینر تنظیم می شود. «افزودن کلینر» را بفرستید.",
                            ))
                            .send()
                            .await;
                        return;
                    }
                    Err(error) => {
                        ::log::warn!("panel: slow-mode state write for {chat} failed: {error}");
                        let _ = query
                            .answer()
                            .alert(if error.commit_outcome_unknown() {
                                "نتیجه ثبت اسلوموشن نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                            } else {
                                "اسلوموشن تغییر کرد، اما تنظیم آن ذخیره نشد؛ دوباره تلاش کنید."
                            })
                            .send()
                            .await;
                        return;
                    }
                }
            }
            (slow_title(ctx, chat), slow_markup(ctx, chat, opener))
        }
        "jn" => (join_title(ctx, chat), join_markup(ctx, chat, opener)),
        "ad" => (adds_title(ctx, chat), adds_markup(ctx, chat, opener)),
        "gp" => (prompt_title(ctx, chat), prompt_markup(ctx, chat, opener)),
        "gr" => match super::rights::snapshot(ctx, chat).await {
            Ok(Some(snapshot)) => (
                super::rights::status(&snapshot),
                rights_markup(&snapshot, chat, opener),
            ),
            Ok(None) => {
                let _ = query
                    .answer()
                    .alert("اختیارات گروه خوانده نشد؛ پنل را داخل گروه دوباره باز کنید.")
                    .send()
                    .await;
                return;
            }
            Err(error) => {
                ::log::warn!("panel: could not read rights for {chat}: {error}");
                let _ = query
                    .answer()
                    .alert("اختیارات گروه خوانده نشد؛ دوباره تلاش کنید.")
                    .send()
                    .await;
                return;
            }
        },
        part if part.starts_with("gr:") => {
            let Some(right) = super::rights::right(&part[3..]) else {
                return;
            };
            let Some(chat_ref) = ctx.chat_ref(chat) else {
                return;
            };
            let before = match super::rights::snapshot(ctx, chat).await {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => {
                    let _ = query
                        .answer()
                        .alert("اختیارات گروه موجود نیست؛ پنل را داخل گروه دوباره باز کنید.")
                        .send()
                        .await;
                    return;
                }
                Err(error) => {
                    ::log::warn!("panel: rights read before mutation for {chat} failed: {error}");
                    let _ = query
                        .answer()
                        .alert("اختیارات گروه خوانده نشد؛ دوباره تلاش کنید.")
                        .send()
                        .await;
                    return;
                }
            };
            let shut = !super::rights::closed(&before, right);
            let delivery_alert = match super::rights::set_right(ctx, chat_ref, chat, right, shut)
                .await
            {
                Ok(super::rights::DeliveryOutcome::Applied) => None,
                Ok(
                    super::rights::DeliveryOutcome::PendingRetry { .. }
                    | super::rights::DeliveryOutcome::Superseded,
                ) => Some("تغییر ذخیره شد و تحویل آن به تلگرام دوباره تلاش می شود."),
                Ok(super::rights::DeliveryOutcome::AcceptedDeliveryUnknown { reason }) => {
                    ::log::warn!("panel: accepted rights delivery for {chat} unknown: {reason}");
                    Some("تغییر ذخیره شد، اما وضعیت تحویل آن به تلگرام مشخص نیست.")
                }
                Err(error) if error.acceptance_unknown() => {
                    ::log::warn!("panel: rights mutation outcome for {chat} unknown: {error}");
                    let _ = query
                        .answer()
                        .alert("وضعیت ذخیره سازی مشخص نیست؛ پنل را دوباره بررسی کنید.")
                        .send()
                        .await;
                    return;
                }
                Err(error) => {
                    ::log::warn!("panel: rights mutation for {chat} failed: {error}");
                    let _ = query
                        .answer()
                        .alert("تغییر ذخیره نشد؛ دوباره تلاش کنید.")
                        .send()
                        .await;
                    return;
                }
            };
            let snapshot = match super::rights::snapshot(ctx, chat).await {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => {
                    let _ = query
                        .answer()
                        .alert("تغییر ذخیره شد، اما وضعیت تازه پنل خوانده نشد.")
                        .send()
                        .await;
                    return;
                }
                Err(error) => {
                    ::log::warn!(
                        "panel: rights refresh after accepted mutation for {chat} failed: {error}"
                    );
                    let _ = query
                        .answer()
                        .alert("تغییر ذخیره شد، اما وضعیت تازه پنل خوانده نشد.")
                        .send()
                        .await;
                    return;
                }
            };
            if let Some(alert) = delivery_alert {
                let _ = query.answer().alert(alert).send().await;
            }
            (
                super::rights::status(&snapshot),
                rights_markup(&snapshot, chat, opener),
            )
        }
        "lg" => (log::status(ctx, chat), log_markup(ctx, chat, opener)),
        "dr" => (report_title(ctx, chat), report_markup(ctx, chat, opener)),
        "ap" => (auto_title(ctx, chat), auto_markup(ctx, chat, opener)),
        "ap_toggle" => {
            let now_on = super::purge::auto_at(ctx, chat).is_none();
            if let Err(error) = super::purge::set_auto_at(
                ctx,
                chat,
                now_on.then_some(super::purge::AUTO_DEFAULT_AT),
            )
            .await
            {
                ::log::warn!("panel: auto purge toggle for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه ذخیره پاکسازی خودکار نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "پاکسازی خودکار ذخیره نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            (auto_title(ctx, chat), auto_markup(ctx, chat, opener))
        }

        "dr_toggle" => {
            let now_on = super::stats::report_at(ctx, chat).is_none();
            if let Err(error) = super::stats::set_report_at(
                ctx,
                chat,
                now_on.then_some(super::stats::REPORT_DEFAULT),
            )
            .await
            {
                ::log::warn!("panel: daily report toggle for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه ذخیره گزارش روزانه نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "گزارش روزانه ذخیره نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            (report_title(ctx, chat), report_markup(ctx, chat, opener))
        }

        "dr_now" => {
            let body = match super::stats::daily_body(ctx, chat, super::stats::today()).await {
                Ok(body) => body,
                Err(error) => {
                    ::log::warn!("panel: report preview for {chat} failed: {error}");
                    let _ = query
                        .answer()
                        .alert("آمار اکنون در دسترس نیست؛ دوباره تلاش کنید.")
                        .send()
                        .await;
                    return;
                }
            };
            let _ = query.answer().send().await;
            if let Some(chat_ref) = ctx.chat_ref(chat) {
                let _ = ctx
                    .client
                    .send_message(chat_ref, super::premium::html(body))
                    .await;
            }
            (report_title(ctx, chat), report_markup(ctx, chat, opener))
        }
        part if part.starts_with("lg:") => {
            let key = &part[3..];
            if log::KINDS.iter().any(|(k, _)| *k == key) {
                let now_on = !ctx.settings.is_locked(chat, key);
                if let Err(error) = ctx.settings.try_set(chat, key, now_on).await {
                    ::log::warn!("panel: log toggle for {chat}/{key} failed: {error}");
                    let _ = query
                        .answer()
                        .alert(if error.commit_outcome_unknown() {
                            "نتیجه ذخیره تنظیم گزارش نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                        } else {
                            "تنظیم گزارش ذخیره نشد؛ دوباره تلاش کنید."
                        })
                        .send()
                        .await;
                    return;
                }
            }
            (log::status(ctx, chat), log_markup(ctx, chat, opener))
        }
        "lg_off" => {
            let result = ctx
                .settings
                .try_apply_batch(
                    chat,
                    &[
                        crate::state::SettingMutation::Delete { key: log::CHANNEL },
                        crate::state::SettingMutation::Delete { key: log::ON },
                    ],
                )
                .await;
            if let Err(error) = result {
                ::log::warn!("panel: log disable for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه خاموش کردن گزارش نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "گزارش خاموش نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            (log::status(ctx, chat), log_markup(ctx, chat, opener))
        }
        "jn_off" => {
            if let Err(error) = join::set_channel(ctx, chat, "").await {
                ::log::warn!("panel: join gate disable for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه خاموش کردن عضویت اجباری نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "عضویت اجباری خاموش نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            (join_title(ctx, chat), join_markup(ctx, chat, opener))
        }
        "wc_off" => {
            if let Err(error) = welcome::try_clear_stored(ctx, chat).await {
                ::log::warn!("panel: welcome disable for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه خاموش کردن خوشامد نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "خوشامد خاموش نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            (welcome_title(ctx, chat), welcome_markup(ctx, chat, opener))
        }
        pick if pick.starts_with("sp:") => {
            if let Err(error) = strict_pick(ctx, chat, &pick[3..]).await {
                ::log::warn!("panel: strict pick for {chat} failed: {error}");
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه ذخیره موارد تخلف نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "موارد تخلف ذخیره نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            (
                strict_picks_title(ctx, chat),
                strict_picks_markup(ctx, chat, opener),
            )
        }
        "on" | "off" => {
            let on = action == "on";
            for lock in plain() {
                if let Err(error) = super::locks::try_set(ctx, chat, lock.key, on).await {
                    ::log::warn!(
                        "panel: bulk lock write for {chat}/{} failed: {error}",
                        lock.key
                    );
                    let _ = query
                        .answer()
                        .alert(if error.commit_outcome_unknown() {
                            "نتیجه تنظیم قفل ها نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                        } else {
                            "تنظیم همه قفل ها کامل نشد؛ وضعیت را بررسی کنید."
                        })
                        .send()
                        .await;
                    return;
                }
            }
            (locks_title(0), locks_markup(ctx, chat, opener, 0))
        }
        "close" => {
            edit_callback(
                ctx,
                query,
                ephemeral_origin,
                response::ResponseKind::AdminTool,
                super::premium::icon_html(
                    Some(super::premium::Icon::Close),
                    format!("<b>پنل بسته شد.</b>\n\n{}", summary(ctx, chat)),
                ),
            )
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
            if let Err(error) = super::locks::try_set(ctx, chat, lock.key, now_on).await {
                ::log::warn!("panel: lock write for {chat}/{} failed: {error}", lock.key);
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه ذخیره قفل نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "قفل ذخیره نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            if super::locks::is_ai(lock.key) {
                (ai_title(ctx, chat), ai_markup(ctx, chat, opener))
            } else {
                let page = page.min(last_page());
                (locks_title(page), locks_markup(ctx, chat, opener, page))
            }
        }
    };

    edit_callback(
        ctx,
        query,
        ephemeral_origin,
        response::ResponseKind::AdminTool,
        super::premium::icon_html_on(
            super::premium::Surface::Edit,
            super::premium::section_icon(action.split(':').next().unwrap_or(action)),
            title,
        )
        .reply_markup(markup),
    )
    .await;
}

async fn list_callback(
    ctx: &Ctx,
    query: &CallbackQuery,
    rest: &str,
    chat: i64,
    opener: i64,
    ephemeral_origin: bool,
) {
    let (kind_name, entry_key) = match rest.split_once(':') {
        Some((kind, key)) => (kind, Some(key)),
        None => (rest, None),
    };
    let Some(kind) = lists::Kind::from_action(kind_name) else {
        return;
    };
    if !limits::permits(ctx, chat, opener, kind.cap()) {
        limits::refuse(query, kind.cap()).await;
        return;
    }
    let Ok(Some(chat_ref)) = query.peer_ref().await else {
        return;
    };

    let page = match entry_key.and_then(|key| lists::clearing(key).map(|c| (key, c))) {
        Some((_, false)) => lists::confirm_clear(ctx, chat_ref, chat, kind, opener).await,
        Some((_, true)) => {
            if let Err(error) = lists::clear_all(ctx, chat_ref, chat, kind).await {
                ::log::warn!("panel: clear {} for {chat} failed: {error}", kind.title());
                let _ = query
                    .answer()
                    .alert(if error.commit_outcome_unknown() {
                        "نتیجه پاکسازی نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                    } else {
                        "پاکسازی کامل نشد؛ دوباره تلاش کنید."
                    })
                    .send()
                    .await;
                return;
            }
            lists::view(ctx, chat_ref, chat, kind, opener).await
        }
        None => {
            if let Some(entry_key) = entry_key {
                match lists::remove(ctx, chat_ref, chat, kind, entry_key).await {
                    Ok(true) => {}
                    Ok(false) => {
                        let _ = query
                            .answer()
                            .alert("این مورد دیگر در لیست نیست.")
                            .send()
                            .await;
                        return;
                    }
                    Err(error) => {
                        ::log::warn!(
                            "panel: remove from {} for {chat} failed: {error}",
                            kind.title()
                        );
                        let _ = query
                            .answer()
                            .alert(if error.commit_outcome_unknown() {
                                "نتیجه حذف مورد نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                            } else {
                                "مورد حذف نشد؛ دوباره تلاش کنید."
                            })
                            .send()
                            .await;
                        return;
                    }
                }
            }
            lists::view(ctx, chat_ref, chat, kind, opener).await
        }
    };
    let (title, markup) = match page {
        Ok(page) => page,
        Err(error) => {
            ::log::warn!("panel: could not read {} for {chat}: {error}", kind.title());
            let _ = query
                .answer()
                .alert("لیست فعلاً در دسترس نیست؛ دوباره تلاش کنید.")
                .send()
                .await;
            return;
        }
    };
    edit_callback(
        ctx,
        query,
        ephemeral_origin,
        response::ResponseKind::AdminTool,
        super::premium::icon_html_on(
            super::premium::Surface::Edit,
            super::premium::section_icon(kind_name),
            title,
        )
        .reply_markup(markup),
    )
    .await;
}

fn miniapp_button(ctx: &Ctx, chat: i64) -> Option<Button> {
    let link = ctx.miniapp_link()?;
    Some(Button::url(
        "🌐  مدیریت گروه در وب",
        format!("{link}?startapp={chat}"),
    ))
}

fn root_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let active = plain()
        .filter(|lock| ctx.settings.is_locked(chat, lock.key))
        .count();
    let watching = ai_keys()
        .filter(|key| ctx.settings.is_locked(chat, key))
        .count();
    let mut rows = vec![
        vec![super::premium::decorate(
            Button::data(
                format!("قفل ها  ({active} از {})  ›", plain().count()),
                payload(opener, chat, "locks"),
            ),
            Some(super::premium::Icon::Locked),
        )],
        vec![Button::data(
            if watching == 0 {
                "🤖  نگهبان هوشمند  ›".to_owned()
            } else {
                format!("🤖  نگهبان هوشمند  ({watching} روشن)  ›")
            },
            payload(opener, chat, "ai"),
        )],
        vec![super::premium::decorate(
            toggle(
                "حالت سختگیرانه  ›",
                payload(opener, chat, "s"),
                ctx.settings.is_locked(chat, strict::MODE),
            ),
            Some(super::premium::Icon::ModerationHammer),
        )],
        vec![super::premium::decorate(
            Button::data("تنظیمات پیشرفته  ›", payload(opener, chat, "adv")),
            Some(super::premium::Icon::Settings),
        )],
        vec![super::premium::decorate(
            Button::data("لیست ها  ›", payload(opener, chat, "ls")),
            Some(super::premium::Icon::ArchiveHistory),
        )],
    ];
    if let Some(button) = miniapp_button(ctx, chat) {
        rows.push(vec![button]);
    }
    rows.push(vec![
        super::premium::decorate(
            Button::data("راهنما", help_payload(opener, chat, help::INDEX_ID)),
            Some(super::premium::Icon::Help),
        ),
        super::premium::decorate(
            Button::data("بستن", payload(opener, chat, "close")),
            Some(super::premium::Icon::Close),
        ),
    ]);
    super::premium::buttons(&rows)
}

fn lists_markup(chat: i64, opener: i64) -> ReplyMarkup {
    super::premium::buttons(&[
        vec![
            super::premium::decorate(
                Button::data("بن شده ها", payload(opener, chat, "l:ban")),
                Some(super::premium::Icon::ModerationHammer),
            ),
            super::premium::decorate(
                Button::data("سکوت شده ها", payload(opener, chat, "l:mute")),
                Some(super::premium::Icon::Muted),
            ),
        ],
        vec![
            super::premium::decorate(
                Button::data("کاربران ویژه", payload(opener, chat, "l:vip")),
                Some(super::premium::Icon::PremiumStar),
            ),
            super::premium::decorate(
                Button::data("لیست فیلتر", payload(opener, chat, "l:filter")),
                Some(super::premium::Icon::Chat),
            ),
        ],
        vec![
            Button::data("🎫  لیست معاف", payload(opener, chat, "l:free")),
            super::premium::decorate(
                Button::data("لیست پاسخ", payload(opener, chat, "l:answer")),
                Some(super::premium::Icon::Chat),
            ),
        ],
        vec![
            super::premium::decorate(
                Button::data("فیلتر تصویری", payload(opener, chat, "l:imgf")),
                Some(super::premium::Icon::Image),
            ),
            Button::data("🗝  دستور سفارشی", payload(opener, chat, "l:cmd")),
        ],
        vec![Button::data(
            "🎨  پک های استیکر",
            payload(opener, chat, "l:pack"),
        )],
        back_row(opener, chat, "root", "ls"),
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
    built(
        ctx,
        chat,
        opener,
        &["rd_on", "rd_lim", "rd_win", "rd_time"],
        "sec",
        "rd",
    )
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
    if imgfilter::any(ctx, chat) {
        causes.push((imgfilter::CAUSE, "فیلتر تصویری"));
    }
    if ctx.settings.is_locked(chat, voicemonitor::MODE) {
        causes.push((voicemonitor::MODE, "واژه در ویس"));
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
    rows.push(back_row(opener, chat, "s", "sp"));
    super::premium::buttons(&rows)
}

async fn strict_pick(
    ctx: &Ctx,
    chat: i64,
    cause: &str,
) -> Result<(), crate::state::SettingsWriteError> {
    let causes = strict_causes(ctx, chat);
    if !causes.iter().any(|(key, _)| *key == cause) {
        return Ok(());
    }
    let key = strict::pick_key(cause);
    let stored = ctx.settings.flags_with_prefix(chat, strict::PICK);
    let selected: Vec<&str> = causes
        .iter()
        .map(|(key, _)| *key)
        .filter(|candidate| stored.iter().any(|stored| stored == candidate))
        .collect();

    if stored.is_empty() {
        let remaining: Vec<String> = causes
            .iter()
            .map(|(candidate, _)| strict::pick_key(candidate))
            .filter(|candidate| candidate != &key)
            .collect();
        if remaining.is_empty() {
            return Ok(());
        }
        let mutations: Vec<crate::state::SettingMutation<'_>> = remaining
            .iter()
            .map(|key| crate::state::SettingMutation::Put { key, value: "" })
            .collect();
        ctx.settings.try_apply_batch(chat, &mutations).await?;
        return Ok(());
    }

    if !stored.iter().any(|stored| stored == cause) {
        ctx.settings.try_set(chat, &key, true).await?;
    } else if selected.len() > 1 {
        ctx.settings.try_set(chat, &key, false).await?;
    }
    Ok(())
}

fn strict_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows = rows_for(ctx, chat, opener, "strict");
    rows.push(vec![super::premium::decorate(
        Button::data("موارد تخلف  ›", payload(opener, chat, "sp")),
        Some(super::premium::Icon::Warning),
    )]);
    for id in ["s_lim", "s_time", "s_act"] {
        rows.extend(rows_for(ctx, chat, opener, id));
    }
    rows.push(back_row(opener, chat, "root", "s"));
    super::premium::buttons(&rows)
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
    built(
        ctx,
        chat,
        opener,
        &["bt_on", "bt_lim", "bt_win", "bt_act"],
        "sec",
        "bt",
    )
}

fn biolink_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>لینک در بایو</b>\n\n\
         عضوی که در بایوی پروفایلش لینک یا آیدی کانال دارد، پیامش حذف می شود.\n\n\
         با متخلف · <b>{}</b>\n\n\
         <i>بایو هر کاربر یک بار خوانده و ده دقیقه به خاطر سپرده می شود، پس اولین پیام هر کس رد می شود.</i>",
        biolink::action_label(biolink::action_of(ctx, chat)),
    )
}

fn biolink_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(ctx, chat, opener, &["bl_on", "bl_act"], "sec", "bl")
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
        if flood::bans(ctx, chat) {
            "بن"
        } else {
            "سکوت"
        },
    )
}

fn flood_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(
        ctx,
        chat,
        opener,
        &["fl_on", "fl_lim", "fl_win", "fl_act"],
        "sec",
        "fl",
    )
}

fn night_title(window: Option<(u32, u32)>) -> String {
    match window {
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

fn night_markup(window: Option<(u32, u32)>, chat: i64, opener: i64) -> ReplyMarkup {
    let (from, to) = window.unwrap_or((23 * 60, 7 * 60));
    let on = window.is_some();
    super::premium::buttons(&[
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
        back_row(opener, chat, "tm", "ng"),
    ])
}

fn clock_markup(
    window: Option<(u32, u32)>,
    chat: i64,
    opener: i64,
    editing_start: bool,
) -> ReplyMarkup {
    let (from, to) = window.unwrap_or((23 * 60, 7 * 60));
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
    rows.push(vec![super::premium::decorate(
        Button::data("بازگشت", payload(opener, chat, "ng")),
        Some(super::premium::Icon::Back),
    )]);
    super::premium::buttons(&rows)
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
    rows.push(back_row(opener, chat, "tm", "sl"));
    super::premium::buttons(&rows)
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
                format!(
                    "‹ {}",
                    super::esc(text.chars().take(120).collect::<String>().as_str())
                )
            },
            if has_media {
                "\n‹ همراه با رسانه"
            } else {
                ""
            }
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
    let mut rows = vec![vec![toggle(
        format!("{}  خوشامد", if on { "✓ روشن" } else { "✗ خاموش" }),
        payload(opener, chat, "wc"),
        on,
    )]];
    rows.extend(rows_for(ctx, chat, opener, "wct"));
    rows.push(vec![coloured(
        "حذف خوشامد",
        payload(opener, chat, "wc_off"),
        Colour::Danger,
    )]);
    rows.push(back_row(opener, chat, "msg", "wc"));
    super::premium::buttons(&rows)
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
    rows.push(vec![Button::data(
        "ساعت پاکسازی",
        payload(opener, chat, "ap"),
    )]);
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
    rows.push(back_row(opener, chat, "tm", "ap"));
    super::premium::buttons(&rows)
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
    rows.push(back_row(opener, chat, "tm", "dr"));
    super::premium::buttons(&rows)
}

pub fn rights_markup(
    snapshot: &crate::state::RightsSnapshot,
    chat: i64,
    opener: i64,
) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = super::rights::RIGHTS
        .iter()
        .map(|right| {
            let open = !super::rights::closed(snapshot, right);
            vec![super::premium::decorate(
                toggle(
                    format!("{}  ·  {}", right.label, if open { "باز" } else { "بسته" }),
                    payload(opener, chat, &format!("gr:{}", right.key)),
                    open,
                ),
                Some(super::premium::permission(right.key, open)),
            )]
        })
        .collect();
    rows.push(back_row(opener, chat, "adv", "gr"));
    super::premium::buttons(&rows)
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
    rows.push(back_row(opener, chat, "adv", "lg"));
    super::premium::buttons(&rows)
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
    built(ctx, chat, opener, &["gpe", "gpt"], "sec", "gp")
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
        super::premium::decorate(
            Button::data("اعلان شرط", payload(opener, chat, "gp")),
            Some(super::premium::Icon::Chat),
        ),
        Button::data("\u{1F3AB}  لیست معاف", payload(opener, chat, "l:free")),
    ]);
    rows.push(back_row(opener, chat, "sec", "ad"));
    super::premium::buttons(&rows)
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
    super::premium::buttons(&[
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
            super::premium::decorate(
                Button::data("اعلان شرط", payload(opener, chat, "gp")),
                Some(super::premium::Icon::Chat),
            ),
            Button::data("🎫  لیست معاف", payload(opener, chat, "l:free")),
        ],
        back_row(opener, chat, "sec", "jn"),
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
    rows.push(vec![super::premium::decorate(
        Button::data("لیست پاسخ ها", payload(opener, chat, "l:answer")),
        Some(super::premium::Icon::Chat),
    )]);
    rows.push(back_row(opener, chat, "msg", "an"));
    super::premium::buttons(&rows)
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
    built(ctx, chat, opener, &["nt_on", "nt_t"], "msg", "nt")
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
    built(
        ctx,
        chat,
        opener,
        &["cp_on", "cp_t", "cp_n", "cp_act"],
        "sec",
        "cp",
    )
}

fn warns_title(ctx: &Ctx, chat: i64) -> String {
    format!(
        "<b>پنل مدیریت</b> › <b>اخطار</b>\n\n\
         با <b>{}</b> اخطار · {}\n\n\
         <i>دستورها: «اخطار» ، «حذف اخطار» ، «اخطارها»</i>",
        warns::limit(ctx, chat),
        if warns::bans(ctx, chat) {
            "اخراج"
        } else {
            "سکوت"
        },
    )
}

fn warns_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    built(ctx, chat, opener, &["wn_lim", "wn_act"], "sec", "wn")
}

fn section(label: &str, target: Vec<u8>, on: bool) -> Button {
    toggle(
        format!("{label}  ›  {}", if on { "✓" } else { "✗" }),
        target,
        on,
    )
}

fn advanced_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows = vec![
        vec![super::premium::decorate(
            Button::data("امنیت و ورود  ›", payload(opener, chat, "sec")),
            Some(super::premium::Icon::Locked),
        )],
        vec![super::premium::decorate(
            Button::data("پیام و پاسخ  ›", payload(opener, chat, "msg")),
            Some(super::premium::Icon::Chat),
        )],
        vec![super::premium::decorate(
            Button::data("پاکسازی و زمان  ›", payload(opener, chat, "tm")),
            Some(super::premium::Icon::Timer),
        )],
        vec![super::premium::decorate(
            Button::data("اختیارات گروه  ›", payload(opener, chat, "gr")),
            Some(super::premium::Icon::Locked),
        )],
        vec![super::premium::decorate(
            section(
                "کانال لاگ",
                payload(opener, chat, "lg"),
                log::channel_id(ctx, chat).is_some(),
            ),
            Some(super::premium::Icon::DocumentActivity),
        )],
    ];
    if super::owner(ctx, chat) == Some(opener) {
        rows.push(vec![super::premium::decorate(
            section(
                "محدودیت مدیران",
                payload(opener, chat, "lim"),
                ctx.settings.is_locked(chat, limits::MODE),
            ),
            Some(super::premium::Icon::Locked),
        )]);
    }
    rows.push(vec![
        super::premium::decorate(
            Button::data("بازگشت", payload(opener, chat, "root")),
            Some(super::premium::Icon::Back),
        ),
        super::premium::decorate(
            Button::data("بستن", payload(opener, chat, "close")),
            Some(super::premium::Icon::Close),
        ),
    ]);
    super::premium::buttons(&rows)
}

fn limits_title(ctx: &Ctx, chat: i64) -> String {
    let closed: Vec<&str> = limits::CAPS
        .iter()
        .filter(|cap| ctx.settings.is_locked(chat, cap.key))
        .map(|cap| cap.label)
        .collect();
    format!(
        "<b>پنل مدیریت</b> › <b>محدودیت مدیران</b>\n\n\
         وضعیت · <b>{}</b>\n\
         بسته · <b>{}</b>\n\n\
         <i>این محدودیت ها روی مالک ربات اعمال نمی شود.</i>",
        if ctx.settings.is_locked(chat, limits::MODE) {
            "روشن"
        } else {
            "خاموش"
        },
        if closed.is_empty() {
            "هیچ کدام".to_owned()
        } else {
            closed.join("، ")
        }
    )
}

fn limits_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows = rows_for(ctx, chat, opener, "lim_on");
    rows.extend(limits::CAPS.chunks(2).map(|pair| {
        pair.iter()
            .map(|cap| {
                let open = !ctx.settings.is_locked(chat, cap.key);
                toggle(
                    format!("{}  {}", if open { "✓" } else { "✗" }, cap.label),
                    payload(opener, chat, &format!("{}:{}", limits::MODE, cap.name)),
                    open,
                )
            })
            .collect()
    }));
    rows.push(back_row(opener, chat, "adv", "lim"));
    super::premium::buttons(&rows)
}

fn security_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let adds = join::required_adds(ctx, chat);
    super::premium::buttons(&[
        vec![super::premium::decorate(
            section(
                "عضویت اجباری",
                payload(opener, chat, "jn"),
                join::channel(ctx, chat).is_some(),
            ),
            Some(super::premium::Icon::Locked),
        )],
        vec![super::premium::decorate(
            toggle(
                format!(
                    "اد اجباری  ›  {}",
                    match adds {
                        0 => "✗".to_owned(),
                        n => format!("✓ {n}"),
                    }
                ),
                payload(opener, chat, "ad"),
                adds > 0,
            ),
            Some(super::premium::Icon::Group),
        )],
        vec![super::premium::decorate(
            section(
                "احراز هویت",
                payload(opener, chat, "cp"),
                ctx.settings.is_locked(chat, captcha::MODE),
            ),
            Some(super::premium::Icon::Locked),
        )],
        rows_for(ctx, chat, opener, "lb_on")
            .into_iter()
            .next()
            .unwrap_or_default(),
        vec![super::premium::decorate(
            section(
                "ضد رگبار",
                payload(opener, chat, "fl"),
                ctx.settings.is_locked(chat, flood::MODE),
            ),
            Some(super::premium::Icon::Timer),
        )],
        vec![super::premium::decorate(
            section(
                "ضد هجوم",
                payload(opener, chat, "rd"),
                ctx.settings.is_locked(chat, raid::MODE),
            ),
            Some(super::premium::Icon::Locked),
        )],
        vec![super::premium::decorate(
            section(
                "ضد خیانت ادمین",
                payload(opener, chat, "bt"),
                ctx.settings.is_locked(chat, betrayal::MODE),
            ),
            Some(super::premium::Icon::Locked),
        )],
        vec![super::premium::decorate(
            section(
                "لینک در بایو",
                payload(opener, chat, "bl"),
                biolink::is_locked(ctx, chat),
            ),
            Some(super::premium::Icon::Locked),
        )],
        rows_for(ctx, chat, opener, "vm_on")
            .into_iter()
            .next()
            .unwrap_or_default(),
        vec![super::premium::decorate(
            Button::data(
                format!(
                    "کلمات فیلتر ویس  ({})  ›",
                    voicemonitor::words(ctx, chat).len()
                ),
                payload(opener, chat, "vw"),
            ),
            Some(super::premium::Icon::Voice),
        )],
        vec![super::premium::decorate(
            Button::data("اخطار  ›", payload(opener, chat, "wn")),
            Some(super::premium::Icon::Warning),
        )],
        rows_for(ctx, chat, opener, "wp_ban")
            .into_iter()
            .next()
            .unwrap_or_default(),
        rows_for(ctx, chat, opener, "wp_mute")
            .into_iter()
            .next()
            .unwrap_or_default(),
        back_row(opener, chat, "adv", "sec"),
    ])
}

fn messages_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let greets = ctx
        .settings
        .value(chat, welcome::TEXT)
        .is_some_and(|text| !text.is_empty());
    super::premium::buttons(&[
        vec![super::premium::decorate(
            section("خوشامد", payload(opener, chat, "wc"), greets),
            Some(super::premium::Icon::Welcome),
        )],
        vec![super::premium::decorate(
            section(
                "اعلان حذف",
                payload(opener, chat, "nt"),
                ctx.settings.is_locked(chat, notice::MODE),
            ),
            Some(super::premium::Icon::Chat),
        )],
        vec![super::premium::decorate(
            Button::data("پاسخ خودکار  ›", payload(opener, chat, "an")),
            Some(super::premium::Icon::Chat),
        )],
        vec![super::premium::decorate(
            Button::data("پیام های ربات  ›", payload(opener, chat, "response")),
            Some(super::premium::Icon::Chat),
        )],
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
        back_row(opener, chat, "adv", "msg"),
    ])
}

fn response_page(ctx: &Ctx, chat: i64, opener: i64, _page: &str) -> (String, ReplyMarkup) {
    (
        "<b>پنل مدیریت</b> › <b>پیام های ربات</b>\n\nفقط اعلان های حذف و خوشامد را می توانید خصوصی کنید؛ بقیه پیام های ربات عادی می مانند."
            .to_owned(),
        response_markup(ctx, chat, opener),
    )
}

fn response_visibility_label(visibility: response::VisibilityOverride) -> &'static str {
    match visibility {
        response::VisibilityOverride::Default => "عمومی",
        response::VisibilityOverride::Public => "عمومی",
        response::VisibilityOverride::Private => "خصوصی",
    }
}

fn response_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let view = response::policy_view(&ctx.settings, chat);
    let notice = view.overrides[0].visibility;
    let welcome = view.overrides[1].visibility;
    let mut rows = vec![vec![Button::data(
        format!("اعلان ها · {}", response_visibility_label(notice)),
        payload(opener, chat, "response"),
    )]];
    rows.push(vec![
        Button::data(
            format!(
                "{} عمومی",
                if notice == response::VisibilityOverride::Public {
                    "✓"
                } else {
                    "·"
                }
            ),
            payload(opener, chat, "resp_notice:public"),
        ),
        Button::data(
            format!(
                "{} خصوصی",
                if notice == response::VisibilityOverride::Private {
                    "✓"
                } else {
                    "·"
                }
            ),
            payload(opener, chat, "resp_notice:private"),
        ),
    ]);
    rows.push(vec![Button::data(
        format!("خوشامد · {}", response_visibility_label(welcome)),
        payload(opener, chat, "response"),
    )]);
    rows.push(vec![
        Button::data(
            format!(
                "{} عمومی",
                if welcome == response::VisibilityOverride::Public {
                    "✓"
                } else {
                    "·"
                }
            ),
            payload(opener, chat, "resp_welcome:public"),
        ),
        Button::data(
            format!(
                "{} خصوصی",
                if welcome == response::VisibilityOverride::Private {
                    "✓"
                } else {
                    "·"
                }
            ),
            payload(opener, chat, "resp_welcome:private"),
        ),
    ]);
    rows.push(vec![Button::data(
        "بازگشت هر دو به عمومی",
        payload(opener, chat, "resp_reset"),
    )]);
    rows.push(back_row(opener, chat, "adv", "msg"));
    super::premium::buttons(&rows)
}

fn timing_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    super::premium::buttons(&[
        vec![
            super::premium::decorate(
                Button::data("قفل شب  ›", payload(opener, chat, "ng")),
                Some(super::premium::Icon::Night),
            ),
            super::premium::decorate(
                Button::data("اسلوموشن  ›", payload(opener, chat, "sl")),
                Some(super::premium::Icon::Timer),
            ),
        ],
        vec![super::premium::decorate(
            section(
                "پاکسازی خودکار",
                payload(opener, chat, "ap"),
                super::purge::auto_at(ctx, chat).is_some(),
            ),
            Some(super::premium::Icon::Delete),
        )],
        vec![super::premium::decorate(
            section(
                "گزارش روزانه",
                payload(opener, chat, "dr"),
                super::stats::report_at(ctx, chat).is_some(),
            ),
            Some(super::premium::Icon::DocumentActivity),
        )],
        vec![super::premium::decorate(
            section(
                "رسانه موقت",
                payload(opener, chat, "tmed"),
                ctx.settings.is_locked(chat, tempmedia::MODE),
            ),
            Some(super::premium::Icon::Timer),
        )],
        back_row(opener, chat, "adv", "tm"),
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
    rows.push(back_row(opener, chat, "tm", "tmed"));
    super::premium::buttons(&rows)
}

fn ai_keys() -> impl Iterator<Item = &'static str> {
    std::iter::once(super::nsfw::LOCK)
        .chain(std::iter::once(super::ocr::LOCK))
        .chain(std::iter::once(super::trade::LOCK))
        .chain(super::concepts::CONCEPTS.iter().map(|c| c.key))
}

fn ai_title(ctx: &Ctx, chat: i64) -> String {
    let on: Vec<&str> = ai_keys()
        .filter(|key| ctx.settings.is_locked(chat, key))
        .filter_map(|key| LOCKS.iter().find(|lock| lock.key == key))
        .map(|lock| lock.names[0])
        .collect();
    let watching = !on.is_empty();
    format!(
        "<b>پنل مدیریت</b> › <b>نگهبان هوشمند</b>\n\n\
         روشن · <b>{}</b>\n\n\
         <i>{}</i>",
        if watching {
            on.join("، ")
        } else {
            "هیچ کدام".to_owned()
        },
        if watching {
            "هر تصویر یک بار گرفته و بررسی می شود، هر چند مورد که روشن باشد، و نتیجه برای همان \
             فایل نگه داشته می شود. قفل خرید و فروش هم متن پیام را می فهمد، نه فقط کلمه ها را."
        } else {
            "هیچ کدام روشن نیست و تا وقتی روشن نشوند هیچ پیام یا تصویری بررسی نمی شود."
        }
    )
}

fn ai_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let row = |icon: &str, label: &str, key: &'static str| {
        let on = ctx.settings.is_locked(chat, key);
        if key == super::trade::LOCK {
            super::premium::decorate(
                toggle(
                    format!("{label}  {}", if on { "✓" } else { "✗" }),
                    payload(opener, chat, key),
                    on,
                ),
                Some(super::premium::protection(on)),
            )
        } else {
            toggle(
                format!("{icon}  {label}  {}", if on { "✓" } else { "✗" }),
                payload(opener, chat, key),
                on,
            )
        }
    };
    let icons = ["🚬", "🍷", "🔫", "🎰", "💊", "🩸"];
    let mut rows = vec![
        vec![row("🔞", "غیراخلاقی", super::nsfw::LOCK)],
        vec![row("📢", "تبلیغ در تصویر", super::ocr::LOCK)],
        vec![row("", "خرید و فروش", super::trade::LOCK)],
    ];
    rows.extend(
        super::concepts::CONCEPTS
            .chunks(2)
            .enumerate()
            .map(|(pair, concepts)| {
                concepts
                    .iter()
                    .enumerate()
                    .map(|(at, concept)| {
                        row(
                            icons.get(pair * 2 + at).copied().unwrap_or("•"),
                            concept.names[0],
                            concept.key,
                        )
                    })
                    .collect()
            }),
    );
    rows.push(vec![
        Button::data("⚙️  تنظیم غیراخلاقی  ›", payload(opener, chat, "nsw")),
        Button::data("⚙️  تنظیم موضوعی  ›", payload(opener, chat, "cq")),
    ]);
    rows.push(vec![super::premium::decorate(
        section(
            "فیلتر تصویری",
            payload(opener, chat, "imf"),
            imgfilter::any(ctx, chat),
        ),
        Some(super::premium::Icon::Image),
    )]);
    rows.push(vec![super::premium::decorate(
        section(
            "برخورد با تکرار تخلف",
            payload(opener, chat, "s"),
            ctx.settings.is_locked(chat, strict::MODE),
        ),
        Some(super::premium::Icon::ModerationHammer),
    )]);
    rows.push(back_row(opener, chat, "root", "ai"));
    super::premium::buttons(&rows)
}

fn nsfw_title(ctx: &Ctx, chat: i64) -> String {
    let armed = ctx.settings.is_locked(chat, super::nsfw::LOCK);
    format!(
        "<b>پنل مدیریت</b> › <b>محتوای غیراخلاقی</b>\n\n\
         قفل · <b>{}</b>\n\n\
         <i>{}</i>",
        if armed { "روشن" } else { "خاموش" },
        "با روشن کردن این قفل، محتوای مستهجن به صورت خودکار حذف می شود. تنظیم امتیاز یا برچسب لازم نیست."
    )
}

fn nsfw_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let armed = ctx.settings.is_locked(chat, super::nsfw::LOCK);
    let mut rows = vec![vec![toggle(
        format!("🔞  قفل غیراخلاقی  ·  {}", if armed { "✓" } else { "✗" }),
        payload(opener, chat, super::nsfw::LOCK),
        armed,
    )]];
    rows.push(back_row(opener, chat, "ai", "nsw"));
    super::premium::buttons(&rows)
}

fn concepts_title(ctx: &Ctx, chat: i64) -> String {
    let armed: Vec<&str> = super::concepts::CONCEPTS
        .iter()
        .filter(|concept| ctx.settings.is_locked(chat, concept.key))
        .map(|concept| concept.names[0])
        .collect();
    format!(
        "<b>پنل مدیریت</b> › <b>قفل موضوعی</b>\n\n         روشن · <b>{}</b>\n         حذف واقعی · <b>{}</b>\n         حساسیت · <b>{}</b>\n\n         <i>تشخیص موضوعی از قفل غیراخلاقی کم دقت تر است. اگر می خواهید اول نتیجه را ببینید بدون اینکه چیزی پاک شود، «فقط بررسی» را روشن کنید.</i>",
        if armed.is_empty() {
            "هیچ کدام".to_owned()
        } else {
            armed.join("، ")
        },
        if ctx.settings.is_locked(chat, super::concepts::SHADOW) {
            "فقط بررسی"
        } else {
            "حذف"
        },
        super::concepts::limit(ctx, chat),
    )
}

fn concepts_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = super::concepts::CONCEPTS
        .chunks(2)
        .map(|pair| {
            pair.iter()
                .map(|concept| {
                    let on = ctx.settings.is_locked(chat, concept.key);
                    toggle(
                        format!("{}  {}", if on { "✓" } else { "✗" }, concept.names[0]),
                        payload(opener, chat, concept.key),
                        on,
                    )
                })
                .collect()
        })
        .collect();
    rows.push(vec![toggle(
        format!(
            "🚫  تبلیغ در تصویر  ·  {}",
            if ctx.settings.is_locked(chat, super::ocr::LOCK) {
                "✓"
            } else {
                "✗"
            }
        ),
        payload(opener, chat, super::ocr::LOCK),
        ctx.settings.is_locked(chat, super::ocr::LOCK),
    )]);
    rows.extend(rows_for(ctx, chat, opener, "cq_shadow"));
    rows.extend(rows_for(ctx, chat, opener, "ad_shadow"));
    rows.extend(rows_for(ctx, chat, opener, "tr_shadow"));
    rows.extend(rows_for(ctx, chat, opener, "cq_lim"));
    rows.extend(rows_for(ctx, chat, opener, "tr_lim"));
    rows.push(back_row(opener, chat, "ai", "cq"));
    super::premium::buttons(&rows)
}

async fn image_filters_page(
    ctx: &Ctx,
    chat: i64,
    opener: i64,
) -> Result<(String, ReplyMarkup), sqlx::Error> {
    let rows = imgfilter::panel_rows(ctx, chat).await?;
    let live = rows.iter().filter(|(_, live, _)| *live).count();

    let title = match rows.is_empty() {
        true => "<b>پنل مدیریت</b> › <b>فیلتر تصویری</b>\n\n\
             هنوز فیلتری ساخته نشده.\n\n\
             <i>«قفل تصویر ‹چیزی›» یا «فیلتر متنی ‹چیزی›» را بفرستید تا همان لحظه فعال شود. \
             برای چیزی که نمی شود اسمش را گفت، روی یک عکس ریپلای کنید و «فیلتر این ‹نام›» \
             بفرستید.</i>"
            .to_owned(),
        false => format!(
            "<b>پنل مدیریت</b> › <b>فیلتر تصویری</b>\n\n\
             ساخته شده · <b>{} از {}</b>\n\
             فعال · <b>{live}</b>\n\n\
             <i>هر کدام را بزنید تا روشن یا خاموش شود. برای حذف، از لیست ها › فیلتر تصویری.</i>",
            rows.len(),
            imgfilter::MAX_FILTERS,
        ),
    };

    let mut buttons: Vec<Vec<Button>> = rows
        .iter()
        .map(|(name, live, _)| {
            let mark = if *live { "✓" } else { "✗" };
            vec![toggle(
                format!("{mark}  {name}"),
                payload(opener, chat, &format!("imf:{}", lists::word_id(name))),
                *live,
            )]
        })
        .collect();

    if !rows.is_empty() {
        buttons.push(vec![Button::data(
            "🗑  حذف فیلتر  ›",
            payload(opener, chat, "l:imgf"),
        )]);
    }
    buttons.push(back_row(opener, chat, "ai", "imf"));
    Ok((title, super::premium::buttons(&buttons)))
}

fn last_page() -> usize {
    plain().count().div_ceil(PER_PAGE) - 1
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
    let grid: Vec<&super::locks::Lock> = plain().collect();
    let shown = &grid[start..(start + PER_PAGE).min(grid.len())];

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
        paging.push(super::premium::decorate(
            Button::data("قبلی", payload(opener, chat, &format!("page:{}", page - 1))),
            Some(super::premium::Icon::Back),
        ));
    }
    if page < last_page() {
        paging.push(super::premium::decorate(
            Button::data(
                "بعدی ›",
                payload(opener, chat, &format!("page:{}", page + 1)),
            ),
            Some(super::premium::Icon::Back),
        ));
    }
    if !paging.is_empty() {
        rows.push(paging);
    }

    rows.push(vec![section(
        "🔞  محتوای غیراخلاقی",
        payload(opener, chat, "nsw"),
        ctx.settings.is_locked(chat, super::nsfw::LOCK),
    )]);
    rows.push(vec![section(
        "🚭  قفل موضوعی",
        payload(opener, chat, "cq"),
        super::concepts::CONCEPTS
            .iter()
            .any(|concept| ctx.settings.is_locked(chat, concept.key)),
    )]);
    rows.push(vec![
        super::premium::decorate(
            coloured("قفل همه", payload(opener, chat, "on"), Colour::Danger),
            Some(super::premium::Icon::Locked),
        ),
        super::premium::decorate(
            coloured(
                "باز کردن همه",
                payload(opener, chat, "off"),
                Colour::Success,
            ),
            Some(super::premium::Icon::Unlocked),
        ),
    ]);
    rows.push(back_row(opener, chat, "root", "locks"));
    super::premium::buttons(&rows)
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

    super::premium::decorate(
        toggle(
            format!("{mark}  {}", lock.names[0]),
            payload(opener, chat, &format!("{}:{page}", lock.key)),
            ctx.settings.is_locked(chat, lock.key),
        ),
        Some(super::premium::lock_icon(
            lock.key,
            ctx.settings.is_locked(chat, lock.key),
        )),
    )
}

fn summary(ctx: &Ctx, chat: i64) -> String {
    let active: Vec<&str> = plain()
        .filter(|lock| ctx.settings.is_locked(chat, lock.key))
        .map(|lock| lock.names[0])
        .collect();
    if active.is_empty() {
        format!(
            "<b>قفل ها</b>\n\nهیچ قفلی فعال نیست ({} در دسترس).",
            plain().count()
        )
    } else {
        format!(
            "<b>قفل ها</b> ({} از {})\n{}",
            active.len(),
            plain().count(),
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

    #[test]
    fn the_help_never_draws_a_gated_button() {
        let payloads = help_index_payloads(7, 7);
        assert!(!payloads.is_empty(), "the index must draw some buttons");
        for data in &payloads {
            assert!(
                data.starts_with("h:"),
                "«{data}» is a help button carrying a panel payload, so the admin gate \
                 will refuse it for anyone who cannot manage the chat"
            );
        }
        assert!(
            payloads
                .iter()
                .any(|d| d == &format!("h:7:7:{}", help::CLOSE_ID)),
            "the index must offer a close, and it must be a help action"
        );
    }

    #[test]
    fn response_policy_callbacks_fit_telegram_limit() {
        let opener = i64::MAX;
        let chat = i64::MIN;
        let actions = [
            "response",
            "resp_notice:public",
            "resp_notice:private",
            "resp_welcome:public",
            "resp_welcome:private",
            "resp_reset",
        ];
        for action in actions {
            assert!(
                payload(opener, chat, action).len() <= 64,
                "{action} is too long"
            );
        }
    }

    fn help_index_payloads(opener: i64, chat: i64) -> Vec<String> {
        let mut out = Vec::new();
        collect_data(&index_markup(opener, chat), &mut out);
        out
    }

    fn collect_data(markup: &ReplyMarkup, out: &mut Vec<String>) {
        use grammers_client::tl::enums::{KeyboardButton, KeyboardButtonRow, ReplyMarkup as Raw};
        let Raw::ReplyInlineMarkup(inline) = &markup.raw else {
            panic!("the help index must be an inline keyboard");
        };
        for KeyboardButtonRow::Row(row) in &inline.rows {
            for button in &row.buttons {
                if let KeyboardButton::Callback(callback) = button {
                    out.push(String::from_utf8_lossy(&callback.data).into_owned());
                }
            }
        }
    }
    use super::*;

    #[test]
    fn reads_numbers_and_clock_times() {
        assert_eq!(parse_number("30", true), Some(30));
        assert_eq!(parse_number("23:37", true), Some(23 * 60 + 37));
        assert_eq!(parse_number("7.5", true), Some(7 * 60 + 5));
        assert_eq!(parse_number("24:00", true), None);
        assert_eq!(parse_number("12:99", true), None);
        assert_eq!(parse_number("سلام", true), None);
    }

    #[test]
    fn only_a_clock_setting_reads_a_clock() {
        assert_eq!(parse_number("12:30", false), None);
        assert_eq!(parse_number("12:30", true), Some(12 * 60 + 30));
        assert_eq!(parse_number("12", false), Some(12));
    }

    #[test]
    fn every_declared_setting_is_rendered_somewhere() {
        let source = include_str!("panel.rs");
        for declared in setting::SETTINGS {
            let named = source.contains(&format!("\"{}\"", declared.id))
                || source.contains(&format!("\"{}:", declared.id));
            assert!(
                named,
                "{} is declared but no panel page renders it",
                declared.id
            );
        }
    }

    #[test]
    fn no_lock_key_shadows_a_panel_action() {
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

    #[test]
    fn a_filter_row_payload_fits_a_callback() {
        let hash = lists::word_id(&"ی".repeat(32));
        assert!(hash.len() <= 16, "the hash grew past a u64 in hex: {hash}");

        let worst = format!("p:{}:{}:imf:{}", i64::MAX, i64::MIN, "f".repeat(16));
        assert!(
            worst.len() <= 64,
            "a filter row payload is {} bytes, over the limit: {worst}",
            worst.len()
        );

        assert!(PAGES.contains(&"imf"));
    }

    #[test]
    fn the_filter_page_does_not_shadow_the_filter_settings_prefix() {
        assert_ne!(imgfilter::PREFIX.trim_end_matches(':'), "imf");
        for declared in setting::SETTINGS {
            assert_ne!(
                declared.id, "imf",
                "a declared setting would swallow the page"
            );
        }
    }

    #[test]
    fn back_goes_where_the_reader_came_from() {
        assert_eq!(help_origin("p:locks"), (true, "locks"));
        assert!(is_page("locks"));

        assert_eq!(help_origin("i:locks"), (false, "locks"));

        assert_eq!(help_origin("i:usr"), (false, "usr"));
        assert_eq!(help_origin("p:usr"), (true, "usr"));
        assert!(
            !is_page("usr"),
            "usr has no panel page, so back must be the index"
        );

        assert_eq!(help_origin("locks"), (false, "locks"));
    }

    #[test]
    fn the_section_and_the_split_agree_about_what_a_model_is() {
        let mut shown: Vec<&str> = ai_keys().collect();
        let mut split: Vec<&str> = LOCKS
            .iter()
            .map(|lock| lock.key)
            .filter(|key| super::super::locks::is_ai(key))
            .collect();
        shown.sort_unstable();
        split.sort_unstable();
        assert_eq!(shown, split, "ai_keys and locks::is_ai have drifted apart");
    }
}
