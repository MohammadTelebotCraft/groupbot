use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerAuth, PeerId, PeerRef};

use super::{Ctx, esc};

pub const CHANNEL: &str = "log_channel";

pub const ON: &str = "log_on";

pub const KINDS: &[(&str, &str)] = &[
    ("log_del", "حذف پیام ها"),
    ("log_mod", "سکوت و بن"),
    ("log_warn", "اخطارها"),
    ("log_admin", "تغییر ادمین ها"),
    ("log_join", "ورود و خروج"),
];

const SET: &[&str] = &["تنظیم لاگ", "تنظیم کانال لاگ", "لاگ"];
const CLEAR: &[&str] = &["حذف لاگ", "خاموش لاگ", "حذف کانال لاگ"];

pub fn channel(ctx: &Ctx, chat: i64) -> Option<PeerRef> {
    let stored = ctx.settings.value(chat, CHANNEL)?;
    let (id, hash) = match stored.split_once('|') {
        Some((id, hash)) => (id.parse().ok()?, hash.parse().ok()),
        None => (stored.parse().ok()?, None),
    };
    let peer = PeerId::from_bot_api_dialog_id(id)?;
    Some(match hash {
        Some(hash) => PeerRef {
            id: peer,
            auth: PeerAuth::from_hash(hash),
        },

        None => peer.to_ambient_ref(),
    })
}

pub fn channel_id(ctx: &Ctx, chat: i64) -> Option<i64> {
    channel(ctx, chat).and_then(|peer| peer.id.bot_api_dialog_id())
}

async fn remember(ctx: &Ctx, chat: i64, peer: PeerRef) {
    let Some(id) = peer.id.bot_api_dialog_id() else {
        return;
    };
    ctx.settings
        .set_value(chat, CHANNEL, &format!("{id}|{}", peer.auth.hash()))
        .await;
}

#[derive(Default)]
pub struct Entry<'a> {
    pub title: &'a str,

    pub target: Option<(i64, &'a str)>,

    pub actor: Option<(i64, &'a str)>,

    pub reason: Option<&'a str>,

    pub extra: Vec<(&'a str, String)>,
}

fn person(user: i64, name: &str) -> String {
    let name = match name.trim().is_empty() || name.trim() == user.to_string() {
        true => "بدون نام".to_owned(),
        false => esc(name),
    };
    format!("<a href=\"tg://user?id={user}\">{name}</a> · <code>{user}</code>")
}

pub fn duration_label(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s} ثانیه"),
        s if s < 3600 => format!("{} دقیقه", s / 60),
        s if s < 86_400 => format!("{} ساعت", s / 3600),
        s => format!("{} روز", s / 86_400),
    }
}

pub async fn write(ctx: &Ctx, chat: i64, kind: &str, entry: Entry<'_>) {
    if !ctx.settings.is_locked(chat, ON) || !ctx.settings.is_locked(chat, kind) {
        return;
    }
    if channel(ctx, chat).is_none() {
        return;
    }

    let seconds = super::stats::local_seconds() % 86_400;
    let mut lines = vec![format!(
        "{:02}:{:02} · <b>{}</b>",
        seconds / 3600,
        (seconds % 3600) / 60,
        esc(entry.title)
    )];
    if let Some((id, name)) = entry.target {
        lines.push(format!("کاربر · {}", person(id, name)));
    }
    if let Some((id, name)) = entry.actor {
        lines.push(format!("توسط · {}", person(id, name)));
    }
    if let Some(reason) = entry.reason {
        lines.push(format!("دلیل · {}", esc(reason)));
    }
    for (label, value) in &entry.extra {
        lines.push(format!("{label} · {value}"));
    }
    ctx.queue_log(chat, lines.join("\n"));
}

pub async fn flush(ctx: &Ctx) {
    const ROOM: usize = 3_500;

    for (chat, entries) in ctx.take_logs() {
        let Some(target) = channel(ctx, chat) else {
            continue;
        };
        let title = ctx
            .settings
            .value(chat, super::TITLE)
            .unwrap_or_else(|| chat.to_string());
        let header = format!("<b>لاگ · {}</b> · <code>{chat}</code>", esc(&title));

        let mut batch = String::new();
        for entry in entries {
            if !batch.is_empty() && batch.len() + entry.len() > ROOM {
                send(ctx, target, &header, &batch).await;
                batch.clear();
            }
            if !batch.is_empty() {
                batch.push_str("\n\n");
            }
            batch.push_str(&entry);
        }
        if !batch.is_empty() {
            send(ctx, target, &header, &batch).await;
        }
    }
}

async fn send(ctx: &Ctx, target: PeerRef, header: &str, body: &str) {
    if let Err(e) = ctx
        .client
        .send_message(target, InputMessage::new().html(format!("{header}\n\n{body}")))
        .await
    {
        eprintln!("log: could not write to the log channel: {e}");
    }
}

pub async fn on_participant(ctx: &Ctx, update: &grammers_client::tl::types::UpdateChannelParticipant) {
    use grammers_client::tl::enums::ChannelParticipant as P;

    let Some(chat) = PeerId::channel(update.channel_id).and_then(|id| id.bot_api_dialog_id())
    else {
        return;
    };

    if !ctx.settings.is_locked(chat, ON) {
        return;
    }
    let by_self = update.actor_id == update.user_id;
    let (kind, title, reason) = match (&update.prev_participant, &update.new_participant) {
        (_, Some(P::Banned(_))) => (
            "log_mod",
            "اخراج یا بن",
            match by_self {
                true => "خودش",
                false => "توسط ادمین",
            },
        ),
        (Some(_), None) => (
            "log_join",
            "خروج",
            match by_self {
                true => "خودش رفت",
                false => "حذف شد",
            },
        ),
        (None, Some(P::Admin(_) | P::Creator(_))) | (Some(_), Some(P::Admin(_) | P::Creator(_))) => {
            ("log_admin", "ادمین شد", "در تلگرام")
        }
        (Some(P::Admin(_) | P::Creator(_)), Some(_)) => ("log_admin", "از ادمینی عزل شد", "در تلگرام"),
        (None, Some(_)) => (
            "log_join",
            "ورود",
            match by_self {
                true => "خودش آمد",
                false => "اضافه شد",
            },
        ),
        _ => return,
    };
    if !ctx.settings.is_locked(chat, kind) {
        return;
    }

    let target_name = name_of(ctx, chat, update.user_id).await;
    let actor_name = match by_self {
        true => String::new(),
        false => name_of(ctx, chat, update.actor_id).await,
    };
    write(
        ctx,
        chat,
        kind,
        Entry {
            title,
            target: Some((update.user_id, &target_name)),
            actor: (!by_self).then_some((update.actor_id, actor_name.as_str())),
            reason: Some(reason),
            ..Default::default()
        },
    )
    .await;
}

async fn name_of(ctx: &Ctx, chat: i64, user: i64) -> String {
    if let Some(name) = super::stats::known_name(ctx, chat, user).await {
        return name;
    }
    let (Some(chat_ref), Some(target)) = (
        ctx.chat_ref(chat),
        PeerId::user(user).map(PeerId::to_ambient_ref),
    ) else {
        return String::new();
    };
    let asked = ctx
        .client
        .invoke(&grammers_client::tl::functions::channels::GetParticipant {
            channel: chat_ref.into(),
            participant: target.into(),
        })
        .await;
    let Ok(grammers_client::tl::enums::channels::ChannelParticipant::Participant(found)) = asked
    else {
        return String::new();
    };
    found
        .users
        .iter()
        .find_map(|peer| match peer {
            grammers_client::tl::enums::User::User(peer) if peer.id == user => Some(
                [peer.first_name.as_deref(), peer.last_name.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if CLEAR.contains(&text) {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        ctx.settings.set_value(chat, CHANNEL, "").await;
        ctx.settings.set(chat, ON, false).await;
        let _ = message.reply("✗ کانال لاگ برداشته شد.").await;
        return true;
    }

    let Some(rest) = SET.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
    }) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }

    if rest.is_empty() {
        let _ = message.reply(InputMessage::new().html(status(ctx, chat))).await;
        return true;
    }

    if super::named(message, Some(rest)).is_none() {
        return false;
    }
    let target = resolve(ctx, message, rest).await;
    let Some(target) = target else {
        let _ = message
            .reply(
                "کانال پیدا نشد.\n\
                 یا «تنظیم لاگ @channel» بفرستید، یا یک پیام از کانال را در گروه فوروارد کنید \
                 و روی آن «تنظیم لاگ» بزنید.",
            )
            .await;
        return true;
    };

    remember(ctx, chat, target).await;
    ctx.settings.set(chat, ON, true).await;

    for (key, _) in KINDS {
        ctx.settings.set(chat, key, true).await;
    }

    let sent = ctx
        .client
        .send_message(
            target,
            InputMessage::new()
                .html("<b>کانال لاگ</b>\n\nاز این پس رویدادهای گروه اینجا نوشته می شود."),
        )
        .await;
    let _ = match sent {
        Ok(_) => message.reply("✓ کانال لاگ تنظیم شد.").await,
        Err(e) => {
            ctx.settings.set(chat, ON, false).await;
            message
                .reply(format!(
                    "انجام نشد · {e}\n\
                     ربات باید در آن کانال ادمین باشد. اگر کانال خصوصی است، یک پیام از آن را \
                     در گروه فوروارد کنید و روی همان «تنظیم لاگ» بزنید."
                ))
                .await
        }
    };
    true
}

async fn resolve(ctx: &Ctx, message: &Message, rest: &str) -> Option<PeerRef> {
    if let Some(name) = rest.strip_prefix('@')
        && let Ok(Some(peer)) = ctx.client.resolve_username(name).await
    {
        return peer.to_ref().await.ok().flatten();
    }

    let from_forward = match message.get_reply().await {
        Ok(Some(replied)) => match replied.forward_header() {
            Some(grammers_client::tl::enums::MessageFwdHeader::Header(header)) => {
                header.from_id.map(PeerId::from)
            }
            None => None,
        },
        _ => None,
    };
    let id = match from_forward {
        Some(id) => id,
        None => PeerId::from_bot_api_dialog_id(super::digits(rest).parse().ok()?)?,
    };

    ctx.client
        .resolve_peer(id.to_ambient_ref())
        .await
        .ok()?
        .to_ref()
        .await
        .ok()
        .flatten()
}

pub fn status(ctx: &Ctx, chat: i64) -> String {
    let kinds = KINDS
        .iter()
        .map(|(key, label)| {
            format!(
                "{} {label}",
                if ctx.settings.is_locked(chat, key) {
                    "✓"
                } else {
                    "✗"
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    match channel_id(ctx, chat) {
        Some(channel) => format!(
            "<b>کانال لاگ</b>\n\n\
             کانال · <code>{channel}</code>\n\n\
             <b>رویدادها</b>\n{kinds}\n\n\
             <i>برداشتن: «حذف لاگ»</i>"
        ),
        None => "<b>کانال لاگ</b>\n\n\
             خاموش است. ربات را در کانال خود ادمین کنید و «تنظیم لاگ @channel» را بفرستید."
            .to_owned(),
    }
}
