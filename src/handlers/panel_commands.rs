use grammers_client::message::Message;

use crate::response::ResponseKind;
use crate::state::SettingsWriteError;

use super::{Ctx, bots, commands, limits, log, panel, purge, stats, strict, tempmedia};

pub const TEMP_MEDIA: &[&str] = &["رسانه موقت", "رسانه های موقت", "/tempmedia"];
pub const ADMIN_LIMIT: &[&str] = &["محدودیت مدیران", "محدودیت ادمین ها", "/adminlimits"];
pub const STRICT_CAUSES: &[&str] = &["موارد تخلف", "دلایل تخلف", "/strictcauses"];
pub const LOG_EVENTS: &[&str] = &["رویداد لاگ", "رویدادهای لاگ", "/logevents"];
pub const AUTO_PURGE: &[&str] = &["پاکسازی خودکار", "پاک سازی خودکار", "/autopurge"];
pub const REPORT_PREVIEW: &[&str] = &[
    "گزارش روزانه آزمایشی",
    "پیش نمایش گزارش روزانه",
    "/reportpreview",
];
pub const BOT_KICK_ADDER: &[&str] = &["اخراج اضافه کننده ربات", "/kickbotadder"];
pub const BOT_EVEN_ADMINS: &[&str] = &["قفل ربات روی ادمین ها", "/botlockadmins"];
pub const COMMANDS: &[&[&str]] = &[
    TEMP_MEDIA,
    ADMIN_LIMIT,
    STRICT_CAUSES,
    LOG_EVENTS,
    AUTO_PURGE,
    REPORT_PREVIEW,
    BOT_KICK_ADDER,
    BOT_EVEN_ADMINS,
];

enum Action {
    Media(Option<(&'static tempmedia::Kind, bool)>),
    Limits(Option<(&'static limits::Cap, bool)>),
    Strict(Option<(&'static str, &'static str, bool)>),
    Log(Option<(&'static str, &'static str, bool)>),
    Auto(Option<bool>),
    Preview,
    Bot(&'static str, &'static str, Option<bool>),
}

fn item_choice(tail: &str, positive: &str, negative: &str) -> Option<(String, bool)> {
    let normalized = commands::normalize(tail);
    let (item, choice) = normalized.rsplit_once(' ')?;
    if item.is_empty() {
        return None;
    }
    let on = match choice {
        choice if choice == positive => true,
        choice if choice == negative => false,
        _ => return None,
    };
    Some((item.to_owned(), on))
}

fn simple_choice(tail: &str) -> Option<bool> {
    match commands::normalize(tail).as_ref() {
        "روشن" => Some(true),
        "خاموش" => Some(false),
        _ => None,
    }
}

fn parse(ctx: &Ctx, chat: i64, text: &str) -> Option<Action> {
    let (command, tail) = COMMANDS.iter().find_map(|aliases| {
        aliases
            .iter()
            .find_map(|alias| commands::strip_prefix(text, alias).map(|tail| (aliases[0], tail)))
    })?;
    match command {
        "رسانه موقت" if tail.is_empty() => Some(Action::Media(None)),
        "رسانه موقت" => {
            let (name, on) = item_choice(tail, "روشن", "خاموش")?;
            tempmedia::KINDS
                .iter()
                .find(|kind| commands::normalize(kind.label).as_ref() == name.as_str())
                .map(|kind| Action::Media(Some((kind, on))))
        }
        "محدودیت مدیران" if tail.is_empty() => Some(Action::Limits(None)),
        "محدودیت مدیران" => {
            let (name, open) = item_choice(tail, "باز", "بسته")?;
            limits::CAPS
                .iter()
                .copied()
                .find(|cap| commands::normalize(cap.label).as_ref() == name.as_str())
                .map(|cap| Action::Limits(Some((cap, open))))
        }
        "موارد تخلف" if tail.is_empty() => Some(Action::Strict(None)),
        "موارد تخلف" => {
            let (name, on) = item_choice(tail, "روشن", "خاموش")?;
            let name = name.strip_prefix("قفل ").unwrap_or(&name);
            panel::strict_causes(ctx, chat)
                .into_iter()
                .find(|(_, label)| commands::normalize(label).as_ref() == name)
                .map(|(key, label)| Action::Strict(Some((key, label, on))))
        }
        "رویداد لاگ" if tail.is_empty() => Some(Action::Log(None)),
        "رویداد لاگ" => {
            let (name, on) = item_choice(tail, "روشن", "خاموش")?;
            log::KINDS
                .iter()
                .copied()
                .find(|(_, label)| commands::normalize(label).as_ref() == name.as_str())
                .map(|(key, label)| Action::Log(Some((key, label, on))))
        }
        "پاکسازی خودکار" if tail.is_empty() => Some(Action::Auto(None)),
        "پاکسازی خودکار" => simple_choice(tail).map(|on| Action::Auto(Some(on))),
        "گزارش روزانه آزمایشی" if tail.is_empty() => Some(Action::Preview),
        "اخراج اضافه کننده ربات" => {
            (tail.is_empty() || simple_choice(tail).is_some()).then(|| {
                Action::Bot(
                    bots::KICK_ADDER,
                    "اخراج اضافه کننده ربات",
                    simple_choice(tail),
                )
            })
        }
        "قفل ربات روی ادمین ها" => {
            (tail.is_empty() || simple_choice(tail).is_some()).then(|| {
                Action::Bot(
                    bots::EVEN_ADMINS,
                    "قفل ربات روی ادمین ها",
                    simple_choice(tail),
                )
            })
        }
        _ => None,
    }
}

fn mark(on: bool) -> &'static str {
    if on { "✓" } else { "✗" }
}

fn state(on: bool) -> &'static str {
    if on { "روشن" } else { "خاموش" }
}

fn master_note(ctx: &Ctx, chat: i64, key: &str) -> &'static str {
    if ctx.settings.is_locked(chat, key) {
        ""
    } else {
        " (بخش اصلی خاموش است)"
    }
}

fn list<'a>(items: impl Iterator<Item = (&'a str, bool)>) -> String {
    items
        .map(|(label, on)| format!("{}  {label}", mark(on)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strict_selected(ctx: &Ctx, chat: i64, cause: &str) -> bool {
    ctx.settings.indexed_empty(chat, strict::PICK)
        || ctx.settings.is_locked(chat, &strict::pick_key(cause))
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let Some(action) = parse(ctx, chat, message.text().trim()) else {
        return false;
    };
    if !limits::allows(ctx, message, limits::SET).await {
        return true;
    }
    if matches!(action, Action::Limits(_)) && !super::is_owner(ctx, message) {
        super::respond(
            ctx,
            message,
            ResponseKind::PermissionDenied,
            "محدودیت مدیران فقط از مالک ربات پذیرفته می شود.",
        )
        .await;
        return true;
    }

    let viewed = match &action {
        Action::Media(None) => Some(format!(
            "رسانه موقت · {} · {} دقیقه · {}\n{}\nنمونه: رسانه موقت عکس خاموش",
            state(ctx.settings.is_locked(chat, tempmedia::MODE)),
            tempmedia::minutes(ctx, chat),
            if tempmedia::reaches_everyone(ctx, chat) {
                "همه"
            } else {
                "بدون مقام"
            },
            list(
                tempmedia::KINDS
                    .iter()
                    .map(|kind| (kind.label, tempmedia::temporary(ctx, chat, kind)))
            )
        )),
        Action::Limits(None) => Some(format!(
            "محدودیت مدیران · {}\n{}\nنمونه: محدودیت مدیران بن بسته",
            state(ctx.settings.is_locked(chat, limits::MODE)),
            list(
                limits::CAPS
                    .iter()
                    .map(|cap| (cap.label, !ctx.settings.is_locked(chat, cap.key)))
            )
        )),
        Action::Strict(None) => {
            let causes = panel::strict_causes(ctx, chat);
            Some(format!(
                "موارد تخلف · {}\n{}\nنمونه: موارد تخلف عکس خاموش",
                state(ctx.settings.is_locked(chat, strict::MODE)),
                if causes.is_empty() {
                    "هیچ قفلی روشن نیست.".to_owned()
                } else {
                    list(
                        causes
                            .iter()
                            .map(|(key, label)| (*label, strict_selected(ctx, chat, key))),
                    )
                }
            ))
        }
        Action::Log(None) => Some(format!(
            "رویدادهای لاگ · {}\n{}\nنمونه: رویداد لاگ اخطارها خاموش",
            state(ctx.settings.is_locked(chat, log::ON)),
            list(
                log::KINDS
                    .iter()
                    .map(|(key, label)| (*label, ctx.settings.is_locked(chat, key)))
            )
        )),
        Action::Auto(None) => Some(format!(
            "پاکسازی خودکار · {} · {} پیام\nنمونه: پاکسازی خودکار روشن",
            purge::auto_at(ctx, chat)
                .map(super::extras::clock)
                .unwrap_or_else(|| "خاموش".to_owned()),
            purge::auto_count(ctx, chat)
        )),
        Action::Bot(key, label, None) => Some(format!(
            "{label} · {}\nنمونه: {label} روشن",
            state(ctx.settings.is_locked(chat, key))
        )),
        _ => None,
    };
    if let Some(text) = viewed {
        super::respond(ctx, message, ResponseKind::SettingsView, text).await;
        return true;
    }

    if let Action::Preview = action {
        match stats::daily_body(ctx, chat, stats::today()).await {
            Ok(body) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::AdminTool,
                    super::premium::html(body),
                )
                .await
            }
            Err(error) => {
                ::log::warn!("daily report preview for {chat} failed: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    "آمار اکنون در دسترس نیست؛ دوباره تلاش کنید.",
                )
                .await
            }
        };
        return true;
    }

    if let Action::Strict(Some((cause, _, false))) = action {
        let selected = panel::strict_causes(ctx, chat)
            .iter()
            .filter(|(key, _)| strict_selected(ctx, chat, key))
            .count();
        if strict_selected(ctx, chat, cause) && selected <= 1 {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "حداقل یک مورد تخلف باید روشن بماند.",
            )
            .await;
            return true;
        }
    }

    let result: Result<String, SettingsWriteError> = match action {
        Action::Media(Some((kind, on))) => {
            ctx.settings.try_set(chat, kind.key, !on).await.map(|_| {
                format!(
                    "رسانه موقت · {} {}{}",
                    kind.label,
                    state(on),
                    master_note(ctx, chat, tempmedia::MODE)
                )
            })
        }
        Action::Limits(Some((cap, open))) => {
            ctx.settings.try_set(chat, cap.key, !open).await.map(|_| {
                format!(
                    "محدودیت مدیران · {} {}{}",
                    cap.label,
                    if open { "باز" } else { "بسته" },
                    master_note(ctx, chat, limits::MODE)
                )
            })
        }
        Action::Strict(Some((cause, label, on))) => {
            let changed = if strict_selected(ctx, chat, cause) != on {
                panel::strict_pick(ctx, chat, cause).await
            } else {
                Ok(())
            };
            changed.map(|_| {
                format!(
                    "موارد تخلف · {label} {}{}",
                    state(strict_selected(ctx, chat, cause)),
                    master_note(ctx, chat, strict::MODE)
                )
            })
        }
        Action::Log(Some((key, label, on))) => {
            ctx.settings.try_set(chat, key, on).await.map(|_| {
                format!(
                    "رویداد لاگ · {label} {}{}",
                    state(on),
                    master_note(ctx, chat, log::ON)
                )
            })
        }
        Action::Auto(Some(on)) => purge::set_auto_at(
            ctx,
            chat,
            on.then(|| purge::auto_at(ctx, chat).unwrap_or(purge::AUTO_DEFAULT_AT)),
        )
        .await
        .map(|_| format!("پاکسازی خودکار {}", state(on))),
        Action::Bot(key, label, Some(on)) => match ctx.settings.try_set(chat, key, on).await {
            Ok(changed) => {
                if changed {
                    bots::on_lock_set(ctx, chat, key, on).await;
                }
                Ok(format!("{label} {}", state(on)))
            }
            Err(error) => Err(error),
        },
        _ => unreachable!("status and preview returned above"),
    };
    match result {
        Ok(text) => super::respond(ctx, message, ResponseKind::SettingsChanged, text).await,
        Err(error) => {
            ::log::warn!("panel command for {chat} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                if error.commit_outcome_unknown() {
                    "نتیجه ذخیره سازی مشخص نیست؛ وضعیت را بررسی کنید."
                } else {
                    "تنظیم ذخیره نشد؛ دوباره تلاش کنید."
                },
            )
            .await
        }
    };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_consume_the_whole_tail() {
        assert_eq!(
            item_choice("عكس خاموش", "روشن", "خاموش"),
            Some(("عکس".to_owned(), false))
        );
        assert_eq!(
            item_choice("عضو ویژه باز", "باز", "بسته"),
            Some(("عضو ویژه".to_owned(), true))
        );
        assert_eq!(item_choice("عکس خاموش لطفا", "روشن", "خاموش"), None);
        assert_eq!(simple_choice("خاموش لطفا"), None);
        assert!(commands::strip_prefix("پاکسازی خودکار خوبه", COMMANDS[4][0]).is_some());
        assert!(commands::strip_prefix("پاکسازی خودکارخوبه", COMMANDS[4][0]).is_none());
    }
}
