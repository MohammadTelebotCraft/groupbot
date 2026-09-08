use grammers_client::message::{Button, Message, ReplyMarkup};
use grammers_client::update::CallbackQuery;

use crate::response::ResponseKind;

use super::Ctx;

pub struct Toggles {
    pub group: &'static str,

    pub title: &'static str,

    pub items: &'static [(&'static str, &'static str)],
}

pub const FORWARD: Toggles = Toggles {
    group: "fw",
    title: "<b>قفل فوروارد</b>",
    items: &[
        (super::locks::FORWARD_CHANNEL, "فوروارد از کانال"),
        (super::locks::FORWARD_USER, "فوروارد از کاربر"),
    ],
};

pub const BOT: Toggles = Toggles {
    group: "bot",
    title: "<b>قفل ربات</b>",
    items: &[
        (super::bots::LOCK, "جلوگیری از افزودن ربات"),
        (super::bots::KICK_ADDER, "اخراج اضافه کننده ربات"),
        (super::bots::EVEN_ADMINS, "اعمال روی ادمین ها هم"),
    ],
};

pub const USERNAME: Toggles = Toggles {
    group: "un",
    title: "<b>قفل یوزرنیم</b>",
    items: &[
        (super::locks::USERNAME, "یوزرنیم در متن"),
        (super::locks::BOTCALL, "دستور به ربات مثل /start@bot"),
        (super::locks::MENTION, "تگ کاربر"),
    ],
};

const GROUPS: &[&Toggles] = &[&FORWARD, &BOT, &USERNAME];

pub async fn prompt(ctx: &Ctx, message: &Message, chat: i64, toggles: &Toggles, on: bool) -> bool {
    if !on {
        let mutations: Vec<_> = toggles
            .items
            .iter()
            .map(|(key, _)| crate::state::SettingMutation::Delete { key })
            .collect();
        if let Err(error) = ctx.settings.try_apply_batch(chat, &mutations).await {
            ::log::warn!(
                "toggle group: clear for {chat}/{} failed: {error}",
                toggles.group
            );
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                if error.commit_outcome_unknown() {
                    "نتیجه حذف قفل ها نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "قفل ها حذف نشدند؛ دوباره تلاش کنید."
                },
            )
            .await;
            return true;
        }
        let cleared: Vec<&str> = toggles.items.iter().map(|(_, label)| *label).collect();
        super::respond(
            ctx,
            message,
            ResponseKind::LockManagement,
            super::premium::icon_text(
                Some(super::premium::Icon::Unlocked),
                format!("برداشته شد · {}", cleared.join("، ")),
            ),
        )
        .await;
        return true;
    }
    super::respond_shared(
        ctx,
        message,
        ResponseKind::LockManagement,
        super::premium::html(prompt_text(toggles)).reply_markup(markup(ctx, chat, toggles)),
    )
    .await;
    true
}

fn prompt_text(toggles: &Toggles) -> String {
    format!("{}\n\nهر مورد را برای تغییر بزنید.", toggles.title)
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, action: &str, chat: i64) {
    let Some((group, key)) = action.split_once(':') else {
        return;
    };
    let Some(toggles) = GROUPS.iter().find(|t| t.group == group) else {
        return;
    };

    if key == "close" {
        let _ = query
            .answer()
            .edit(super::premium::html(summary(ctx, chat, toggles)))
            .await;
        return;
    }
    if !toggles.items.iter().any(|(item, _)| *item == key) {
        return;
    }

    let now_on = !ctx.settings.is_locked(chat, key);
    match ctx.settings.try_set(chat, key, now_on).await {
        Ok(true) => super::bots::on_lock_set(ctx, chat, key, now_on).await,
        Ok(false) => {}
        Err(error) => {
            ::log::warn!("toggle: write for {chat}/{key} failed: {error}");
            let _ = query
                .answer()
                .alert(if error.commit_outcome_unknown() {
                    "نتیجه ذخیره تنظیم نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "تنظیم ذخیره نشد؛ دوباره تلاش کنید."
                })
                .send()
                .await;
            return;
        }
    }

    let _ = query
        .answer()
        .edit(super::premium::html(prompt_text(toggles)).reply_markup(markup(ctx, chat, toggles)))
        .await;
}

fn markup(ctx: &Ctx, chat: i64, toggles: &Toggles) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = toggles
        .items
        .iter()
        .map(|(key, label)| {
            let mark = if ctx.settings.is_locked(chat, key) {
                "✓"
            } else {
                "✗"
            };
            vec![super::premium::decorate(
                super::style::toggle(
                    format!("{mark}  {label}"),
                    format!("t:{}:{key}", toggles.group).into_bytes(),
                    ctx.settings.is_locked(chat, key),
                ),
                Some(super::premium::protection(
                    ctx.settings.is_locked(chat, key),
                )),
            )]
        })
        .collect();
    rows.push(vec![super::premium::decorate(
        Button::data("بستن", format!("t:{}:close", toggles.group).into_bytes()),
        Some(super::premium::Icon::Close),
    )]);
    super::premium::buttons(&rows)
}

fn summary(ctx: &Ctx, chat: i64, toggles: &Toggles) -> String {
    let lines: Vec<String> = toggles
        .items
        .iter()
        .map(|(key, label)| {
            let mark = if ctx.settings.is_locked(chat, key) {
                "✓"
            } else {
                "✗"
            };
            format!("{mark}  {label}")
        })
        .collect();
    format!("{}\n{}", toggles.title, lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_are_valid() {
        let mut groups: Vec<&str> = GROUPS.iter().map(|t| t.group).collect();
        let count = groups.len();
        groups.sort_unstable();
        groups.dedup();
        assert_eq!(groups.len(), count, "duplicate toggle group");

        for toggles in GROUPS {
            for (key, label) in toggles.items {
                assert!(!label.is_empty());
                let payload = format!("t:{}:{key}", toggles.group);
                assert!(payload.len() <= 64, "payload too long: {payload}");
            }
        }
    }
}
