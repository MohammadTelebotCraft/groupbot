use std::collections::HashMap;

use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::PeerId;
use grammers_client::update::CallbackQuery;

use super::{Ctx, esc};

pub const COMMANDS: &[&str] = &["داشبورد", "پنل کل", "آمار کل", "سودو"];

const BUSIEST: i64 = 10;

const SECTIONS: &[(&str, &str)] = &[
    ("sum", "📊  خلاصه"),
    ("top", "🔥  شلوغ ترین ها"),
    ("use", "🎛  امکانات"),
];

fn tracked() -> Vec<(&'static str, &'static str)> {
    vec![
        (super::captcha::MODE, "احراز هویت"),
        (super::flood::MODE, "ضد رگبار"),
        (super::raid::MODE, "ضد هجوم"),
        (super::betrayal::MODE, "ضد خیانت ادمین"),
        (super::strict::MODE, "حالت سختگیرانه"),
        (super::stats::RANKS, "مقام خودکار"),
        (super::restrict::WIPE_BAN, "پاکسازی با بن"),
        (super::restrict::WIPE_MUTE, "پاکسازی با سکوت"),
        (super::tempmedia::MODE, "رسانه موقت"),
        (super::nsfw::LOCK, "حذف غیراخلاقی"),
        (super::limits::MODE, "محدودیت مدیران"),
        (super::stats::REPORT_AT, "گزارش روزانه"),
        (super::purge::AUTO_AT, "پاکسازی خودکار"),
    ]
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    if !COMMANDS.contains(&message.text().trim()) {
        return false;
    }
    let Some(opener) = message.sender_id().and_then(PeerId::bare_id) else {
        return false;
    };
    if Some(opener) != super::cleaner::sudo() {
        return false;
    }

    let _ = message
        .reply(
            InputMessage::new()
                .html(page(ctx, "sum").await)
                .reply_markup(markup(opener, "sum")),
        )
        .await;
    true
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str) {
    let Some((opener, section)) = payload.split_once(':') else {
        return;
    };
    let Ok(opener) = opener.parse::<i64>() else {
        return;
    };

    if query.sender_id().bare_id() != Some(opener) || Some(opener) != super::cleaner::sudo() {
        let _ = query.answer().send().await;
        return;
    }

    let _ = query
        .answer()
        .edit(
            InputMessage::new()
                .html(page(ctx, section).await)
                .reply_markup(markup(opener, section)),
        )
        .await;
}

async fn page(ctx: &Ctx, section: &str) -> String {
    let day = super::stats::today();
    let fleet = ctx.settings.fleet(day).await;

    let head = "<b>داشبورد کل</b>";
    match section {
        "top" => {
            let busiest = ctx.settings.busiest(day, BUSIEST).await;
            format!("{head} › <b>شلوغ ترین ها</b>\n\n{}", board(ctx, &busiest))
        }
        "use" => {
            let tracked = tracked();
            let keys: Vec<&str> = tracked.iter().map(|(key, _)| *key).collect();
            let counts = ctx.settings.adoption(&keys).await;
            format!(
                "{head} › <b>امکانات</b>\n\n{}\n\n<i>از {} گروه.</i>",
                uptake(&tracked, &counts, fleet.chats),
                fleet.chats
            )
        }
        _ => {
            let quiet = fleet.chats.saturating_sub(fleet.active_today);
            format!(
                "{head} › <b>خلاصه</b>\n\n\
                 👥  گروه ها · <b>{}</b>\n\
                 ⚙️  کانفیگ شده · <b>{}</b>\n\
                 🟢  فعال امروز · <b>{}</b>\n\
                 💤  ساکت امروز · <b>{quiet}</b>\n\n\
                 💬  پیام های امروز · <b>{}</b>\n\
                 📚  پیام های ثبت شده · <b>{}</b>\n\
                 🧑  کاربران شمرده شده · <b>{}</b>\n\
                 📈  میانگین هر گروه · <b>{}</b>\n\n\
                 🖥  در حافظه این پروسه · <b>{}</b>\n\
                 ⏱  روشن از · <b>{}</b>\n\
                 🧹  کلینر · <b>{}</b>",
                fleet.chats,
                fleet.configured,
                fleet.active_today,
                fleet.messages_today,
                fleet.messages_total,
                fleet.members,
                fleet.messages_total / fleet.chats.max(1),
                ctx.settings.chats().len(),
                super::ping::uptime(ctx.started.elapsed()),
                match ctx.user_client().is_some() {
                    true => "وارد شده",
                    false => "وارد نشده",
                },
            )
        }
    }
}

fn board(ctx: &Ctx, rows: &[(i64, u64)]) -> String {
    if rows.is_empty() {
        return "‹ امروز هیچ گروهی پیامی ثبت نکرده.".to_owned();
    }
    rows.iter()
        .enumerate()
        .map(|(place, (chat, count))| {
            let title = ctx
                .settings
                .value(*chat, super::TITLE)
                .unwrap_or_else(|| chat.to_string());
            format!("{}. {} · <b>{count}</b>", place + 1, esc(&title))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn uptake(
    tracked: &[(&'static str, &'static str)],
    counts: &HashMap<String, u64>,
    chats: u64,
) -> String {
    tracked
        .iter()
        .map(|(key, label)| {
            let on = counts.get(*key).copied().unwrap_or(0);
            format!("{label} · <b>{on}</b> ({}٪)", on * 100 / chats.max(1))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn markup(opener: i64, current: &str) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = vec![
        SECTIONS
            .iter()
            .map(|(key, label)| {
                super::style::choice(
                    *label,
                    format!("sd:{opener}:{key}").into_bytes(),
                    *key == current,
                )
            })
            .collect(),
    ];
    rows.push(vec![Button::data(
        "↻  تازه سازی",
        format!("sd:{opener}:{current}").into_bytes(),
    )]);
    ReplyMarkup::from_buttons(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_payload_fits_a_callback() {
        for (key, label) in SECTIONS {
            assert!(!label.is_empty());
            let payload = format!("sd:{}:{key}", i64::MIN);
            assert!(payload.len() <= 64, "payload too long: {payload}");
        }
    }

    #[test]
    fn tracked_keys_are_real_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for (key, label) in tracked() {
            assert!(seen.insert(key), "«{label}» tracks {key} twice");
            let declared = super::super::setting::SETTINGS
                .iter()
                .any(|setting| setting.key == key)
                || super::super::locks::LOCKS
                    .iter()
                    .any(|lock| lock.key == key)
                || [super::super::stats::REPORT_AT, super::super::purge::AUTO_AT].contains(&key);
            assert!(declared, "«{label}» tracks an unknown key: {key}");
        }
    }

    #[test]
    fn section_ids_are_distinct() {
        let ids: std::collections::HashSet<&str> = SECTIONS.iter().map(|(key, _)| *key).collect();
        assert_eq!(ids.len(), SECTIONS.len());
    }
}
