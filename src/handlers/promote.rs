use std::time::{Duration, Instant};

use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::PeerRef;
use grammers_client::update::CallbackQuery;

use super::{Ctx, bot_admin_key, esc, is_owner, sender_is_creator};

const COMMANDS: &[&str] = &["افزودن ادمین", "ادمین کن", "اضافه کردن ادمین"];
const DEMOTE: &[&str] = &["حذف ادمین", "عزل ادمین", "برکناری ادمین"];
const TAG: &[&str] = &["تنظیم تگ", "تنظیم مقام"];
const TAG_CLEAR: &[&str] = &["حذف تگ", "حذف مقام"];

const TAG_MAX: usize = 16;

pub const PENDING_TTL: Duration = Duration::from_secs(600);

#[derive(Clone)]
pub struct Pending {
    pub target: PeerRef,
    pub name: String,

    pub rights: u32,
    pub started: Instant,
}

async fn tag(ctx: &Ctx, message: &Message, text: &str) -> bool {
    let Some((rest, clearing)) = TAG
        .iter()
        .map(|command| (command, false))
        .chain(TAG_CLEAR.iter().map(|command| (command, true)))
        .find_map(|(command, clearing)| {
            let rest = text.strip_prefix(command)?;
            (rest.is_empty() || rest.starts_with(char::is_whitespace))
                .then(|| (rest.trim(), clearing))
        })
    else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    let (arg, title) = match rest.split_once(char::is_whitespace) {
        Some((first, tail)) if first.starts_with('@') || first.parse::<i64>().is_ok() => {
            (Some(first), tail.trim())
        }
        _ if rest.starts_with('@') || rest.parse::<i64>().is_ok() => (Some(rest), ""),
        _ => (None, rest),
    };
    let title = if clearing { "" } else { title };
    let Some(named) = super::named(message, arg) else {
        return false;
    };
    if title.chars().count() > TAG_MAX {
        let _ = message
            .reply(format!("تگ باید حداکثر {TAG_MAX} حرف باشد."))
            .await;
        return true;
    }
    if title.is_empty() && !clearing {
        let _ = message
            .reply("متن تگ را بنویسید، مثل «تنظیم تگ مدیر ارشد».")
            .await;
        return true;
    }

    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        let _ = message
            .reply("کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.")
            .await;
        return true;
    };
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    let target = match target.id.bare_id() {
        Some(id) => super::admin_ref(ctx, chat_ref, id)
            .await
            .map(|(peer, _)| peer)
            .unwrap_or(target),
        None => target,
    };

    let existing = ctx.client.set_admin_rights(chat_ref, target).load_current().await;
    let outcome = match existing {
        Ok(builder) => builder.rank(title).await,
        Err(e) => {
            eprintln!("tag: {chat}: no current rights for {name}: {e}");
            ctx.client
                .set_admin_rights(chat_ref, target)
                .rank(title)
                .await
        }
    };
    let _ = match outcome {
        Ok(()) if clearing => message.reply(format!("✗ تگ {name} برداشته شد.")).await,
        Ok(()) => message.reply(format!("✓ تگ {name} · {title}")).await,
        Err(e) => {
            eprintln!("tag: {chat}: could not title {name}: {e}");
            message
                .reply(format!(
                    "انجام نشد · {e}\nربات باید ادمین باشد و اجازه «افزودن ادمین» داشته باشد."
                ))
                .await
        }
    };
    true
}

const RIGHTS: &[(&str, u32)] = &[
    ("حذف پیام", 1 << 0),
    ("بن کاربران", 1 << 1),
    ("افزودن اعضا", 1 << 2),
    ("سنجاق پیام", 1 << 3),
    ("مدیریت تماس", 1 << 4),
    ("تغییر اطلاعات", 1 << 5),
    ("افزودن ادمین", 1 << 6),
];

const DEFAULT_RIGHTS: u32 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3);

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = super::digits(message.text().trim());
    if tag(ctx, message, text.as_ref()).await {
        return true;
    }
    let matched = COMMANDS
        .iter()
        .map(|command| (command, true))
        .chain(DEMOTE.iter().map(|command| (command, false)))
        .find_map(|(command, adding)| {
            let rest = text.strip_prefix(command)?;
            (rest.is_empty() || rest.starts_with(char::is_whitespace))
                .then(|| (adding, rest.trim().to_owned()))
        });
    let Some((adding, arg)) = matched else {
        return false;
    };

    let arg = (!arg.is_empty()).then_some(arg);
    let Some(named) = super::named(message, arg.as_deref()) else {
        return false;
    };
    if !is_owner(ctx, message) && !sender_is_creator(ctx, message).await {
        return true;
    }

    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        let _ = message
            .reply("کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.")
            .await;
        return true;
    };

    if !adding {
        demote(ctx, message, target, &name).await;
        return true;
    }

    let Some(opener) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };
    let key = ctx.pending_admin_new(Pending {
        target,
        name: esc(&name),
        rights: DEFAULT_RIGHTS,
        started: Instant::now(),
    });
    let Some(pending) = ctx.pending_admin(key) else {
        return true;
    };

    let _ = message
        .reply(
            InputMessage::new()
                .html(title(&pending))
                .reply_markup(markup(&pending, opener, key)),
        )
        .await;
    true
}

async fn demote(ctx: &Ctx, message: &Message, target: PeerRef, name: &str) {
    let (Ok(Some(chat_ref)), Some(chat), Some(user_id)) = (
        message.peer_ref().await,
        message.peer_id().bot_api_dialog_id(),
        target.id.bare_id(),
    ) else {
        return;
    };

    let peer = match super::admin_ref(ctx, chat_ref, user_id).await {
        Some((peer, _)) => peer,
        None => target,
    };

    if let Err(e) = ctx.client.set_admin_rights(chat_ref, peer).await {
        eprintln!("promote: {chat}: could not demote {user_id}: {e}");
        let _ = message
            .reply("انجام نشد. ربات فقط می تواند ادمین هایی را عزل کند که خودش اضافه کرده است.")
            .await;
        return;
    }
    ctx.settings.set(chat, &bot_admin_key(user_id), false).await;
    ctx.forget_admins(chat);
    let by = super::sender_of(message);
    super::log::write(
        ctx,
        chat,
        "log_admin",
        super::log::Entry {
            title: "عزل ادمین",
            target: Some((user_id, name)),
            actor: by.as_ref().map(|(id, name)| (*id, name.as_str())),
            ..Default::default()
        },
    )
    .await;

    let _ = message
        .reply(InputMessage::new().html(format!(
            "✗ <b>{}</b> از ادمینی گروه و ربات عزل شد.",
            esc(name)
        )))
        .await;
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, chat: i64) {
    let mut parts = payload.splitn(3, ':');
    let (Some(opener), Some(key), Some(what)) = (parts.next(), parts.next(), parts.next()) else {
        return;
    };
    let (Ok(opener), Ok(key)) = (opener.parse::<i64>(), key.parse::<u64>()) else {
        return;
    };
    if query.sender_id().bare_id() != Some(opener) {
        let _ = query
            .answer()
            .alert("این صفحه را شخص دیگری باز کرده است.")
            .send()
            .await;
        return;
    }
    let Some(mut pending) = ctx.pending_admin(key) else {
        let _ = query
            .answer()
            .alert("این درخواست منقضی شده است. دوباره «افزودن ادمین» را بفرستید.")
            .send()
            .await;
        return;
    };

    match what {
        "no" => {
            ctx.pending_admin_done(key);
            let _ = query
                .answer()
                .edit(InputMessage::new().html("✗ افزودن ادمین لغو شد."))
                .await;
        }
        "ok" => {
            ctx.pending_admin_done(key);
            confirm(ctx, query, chat, &pending).await;
        }
        index => {
            let Some(bit) = index.parse::<usize>().ok().and_then(|i| RIGHTS.get(i)) else {
                return;
            };
            pending.rights ^= bit.1;
            ctx.pending_admin_set_rights(key, pending.rights);
            let _ = query
                .answer()
                .edit(
                    InputMessage::new()
                        .html(title(&pending))
                        .reply_markup(markup(&pending, opener, key)),
                )
                .await;
        }
    }
}

async fn confirm(ctx: &Ctx, query: &CallbackQuery, chat: i64, pending: &Pending) {
    let Ok(Some(chat_ref)) = query.peer_ref().await else {
        return;
    };
    let has = |bit: u32| pending.rights & bit != 0;

    let result = ctx
        .client
        .set_admin_rights(chat_ref, pending.target)
        .delete_messages(has(RIGHTS[0].1))
        .ban_users(has(RIGHTS[1].1))
        .invite_users(has(RIGHTS[2].1))
        .pin_messages(has(RIGHTS[3].1))
        .manage_call(has(RIGHTS[4].1))
        .change_info(has(RIGHTS[5].1))
        .add_admins(has(RIGHTS[6].1))
        .await;

    if let Err(e) = result {
        eprintln!("promote: {chat}: could not promote {}: {e}", pending.name);
        let _ = query
            .answer()
            .edit(InputMessage::new().html(
                "انجام نشد. مطمئن شوید ربات ادمین است و اجازه «افزودن ادمین» دارد.",
            ))
            .await;
        return;
    }

    if let Some(id) = pending.target.id.bare_id() {
        ctx.settings.set(chat, &bot_admin_key(id), true).await;
    }
    ctx.forget_admins(chat);

    let granted: Vec<&str> = RIGHTS
        .iter()
        .filter(|(_, bit)| has(*bit))
        .map(|(label, _)| *label)
        .collect();
    let by = query.sender_id().bare_id();
    super::log::write(
        ctx,
        chat,
        "log_admin",
        super::log::Entry {
            title: "ادمین جدید",
            target: pending
                .target
                .id
                .bare_id()
                .map(|id| (id, pending.name.as_str())),
            actor: by.map(|id| (id, "")),
            extra: vec![(
                "دسترسی ها",
                match granted.is_empty() {
                    true => "بدون دسترسی خاص".to_owned(),
                    false => granted.join(" · "),
                },
            )],
            ..Default::default()
        },
    )
    .await;
    let _ = query
        .answer()
        .edit(InputMessage::new().html(format!(
            "<b>ادمین جدید</b>\n\n<b>{}</b> ادمین گروه و ربات شد.\n\n<b>دسترسی ها</b>\n{}",
            pending.name,
            if granted.is_empty() {
                "‹ بدون دسترسی خاص".to_owned()
            } else {
                granted
                    .iter()
                    .map(|label| format!("✓ {label}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        )))
        .await;
}

fn title(pending: &Pending) -> String {
    format!(
        "<b>افزودن ادمین</b>\n\nکاربر · <b>{}</b>\n\nدسترسی ها را انتخاب و تایید کنید.",
        pending.name
    )
}

fn markup(pending: &Pending, opener: i64, key: u64) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = RIGHTS
        .chunks(2)
        .enumerate()
        .map(|(row, pair)| {
            pair.iter()
                .enumerate()
                .map(|(column, (label, bit))| {
                    let index = row * 2 + column;
                    let mark = if pending.rights & bit != 0 { "✓" } else { "✗" };
                    Button::data(
                        format!("{mark}  {label}"),
                        format!("a:{opener}:{key}:{index}").into_bytes(),
                    )
                })
                .collect()
        })
        .collect();
    rows.push(vec![
        super::style::data(
            "✅  تایید",
            format!("a:{opener}:{key}:ok").into_bytes(),
            super::style::Colour::Success,
        ),
        super::style::data(
            "❌  لغو",
            format!("a:{opener}:{key}:no").into_bytes(),
            super::style::Colour::Danger,
        ),
    ]);
    ReplyMarkup::from_buttons(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rights_bits_are_distinct_and_indexable() {
        let mut seen = 0u32;
        for (index, (label, bit)) in RIGHTS.iter().enumerate() {
            assert!(!label.is_empty());
            assert_eq!(seen & bit, 0, "duplicate bit at index {index}");
            seen |= bit;

            let payload = format!("a:{}:{}:{index}", i64::MAX, u64::MAX);
            assert!(payload.len() <= 64, "payload too long: {payload}");
        }
        assert_eq!(DEFAULT_RIGHTS & !seen, 0, "default grants an unknown right");
    }
}
