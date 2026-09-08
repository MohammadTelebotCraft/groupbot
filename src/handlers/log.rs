use grammers_client::message::Message;
use grammers_client::session::types::{PeerAuth, PeerId, PeerRef};

use super::{Ctx, esc};
use crate::response::ResponseKind;
use crate::state::{SettingMutation, SettingsWriteError};

struct PendingLogBatch {
    ctx: std::sync::Arc<Ctx>,
    chat: i64,
    entries: Option<Vec<String>>,
}

impl PendingLogBatch {
    fn complete(&mut self) {
        self.entries = None;
    }
}

impl Drop for PendingLogBatch {
    fn drop(&mut self) {
        if let Some(entries) = self.entries.take() {
            self.ctx.retry_logs(self.chat, entries);
        }
    }
}

pub const CHANNEL: &str = "log_channel";

pub const ON: &str = "log_on";

pub const KINDS: &[(&str, &str)] = &[
    ("log_del", "حذف پیام ها"),
    ("log_mod", "سکوت و بن"),
    ("log_warn", "اخطارها"),
    ("log_admin", "تغییر ادمین ها"),
    ("log_join", "ورود و خروج"),
];

pub const SET: &[&str] = &["تنظیم لاگ", "تنظیم کانال لاگ", "لاگ"];
pub const CLEAR: &[&str] = &["حذف لاگ", "خاموش لاگ", "حذف کانال لاگ"];

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

fn stored_channel(peer: PeerRef) -> Option<String> {
    let id = peer.id.bot_api_dialog_id()?;
    Some(format!("{id}|{}", peer.auth.hash()))
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

pub async fn flush(ctx: &std::sync::Arc<Ctx>) -> bool {
    let _flush = ctx.log_flush.lock().await;
    const ROOM: usize = 3_500;

    let failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let owner = std::sync::Arc::clone(ctx);
    let failures = std::sync::Arc::clone(&failed);
    let batches = ctx
        .take_logs()
        .into_iter()
        .map(|(chat, entries)| PendingLogBatch {
            ctx: std::sync::Arc::clone(ctx),
            chat,
            entries: Some(entries),
        })
        .collect();
    super::bounded(batches, super::FLEET_CONCURRENCY, move |mut pending| {
        let ctx = std::sync::Arc::clone(&owner);
        let failed = std::sync::Arc::clone(&failures);
        async move {
            let chat = pending.chat;
            let Some(target) = channel(&ctx, chat) else {
                pending.complete();
                return;
            };
            let entries = pending
                .entries
                .as_ref()
                .expect("an incomplete log batch retains its entries");
            let title = ctx
                .settings
                .value(chat, super::TITLE)
                .unwrap_or_else(|| chat.to_string());
            let header = format!("<b>لاگ · {}</b> · <code>{chat}</code>", esc(&title));

            let mut batch = String::new();
            let mut delivered = true;
            for entry in entries {
                if !batch.is_empty() && batch.chars().count() + entry.chars().count() > ROOM {
                    if !send(&ctx, target, &header, &batch).await {
                        delivered = false;
                        break;
                    }
                    batch.clear();
                }
                if !batch.is_empty() {
                    batch.push_str("\n\n");
                }
                batch.push_str(entry);
            }
            if delivered && !batch.is_empty() {
                delivered = send(&ctx, target, &header, &batch).await;
            }
            if !delivered {
                failed.store(true, std::sync::atomic::Ordering::Relaxed);
            } else {
                pending.complete();
            }
        }
    })
    .await;
    !failed.load(std::sync::atomic::Ordering::Relaxed)
}

pub async fn flush_all(ctx: &std::sync::Arc<Ctx>) -> bool {
    while ctx.has_pending_logs() {
        if !flush(ctx).await {
            return false;
        }
    }
    true
}

async fn send(ctx: &Ctx, target: PeerRef, header: &str, body: &str) -> bool {
    if let Err(e) = ctx
        .client
        .send_message(
            target,
            super::premium::icon_html_on(
                super::premium::Surface::Channel,
                Some(super::premium::Icon::DocumentActivity),
                format!("{header}\n\n{body}"),
            ),
        )
        .await
    {
        eprintln!("log: could not write to the log channel: {e}");
        false
    } else {
        true
    }
}

pub async fn on_participant(
    ctx: &Ctx,
    update: &grammers_client::tl::types::UpdateChannelParticipant,
) {
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
        (None, Some(P::Admin(_) | P::Creator(_)))
        | (Some(_), Some(P::Admin(_) | P::Creator(_))) => ("log_admin", "ادمین شد", "در تلگرام"),
        (Some(P::Admin(_) | P::Creator(_)), Some(_)) => {
            ("log_admin", "از ادمینی عزل شد", "در تلگرام")
        }
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
    match super::stats::known_name(ctx, chat, user).await {
        Ok(Some(name)) => return name,
        Ok(None) => {}
        Err(error) => {
            log::warn!("log: cached name read for {chat}/{user} failed: {error}");
        }
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
        let response = match ctx
            .settings
            .try_apply_batch(
                chat,
                &[
                    SettingMutation::Delete { key: CHANNEL },
                    SettingMutation::Delete { key: ON },
                ],
            )
            .await
        {
            Ok(_) => super::premium::icon_text(
                Some(super::premium::Icon::DocumentActivity),
                "کانال لاگ برداشته شد.",
            ),
            Err(error) => {
                ::log::warn!("log: clear for {chat} failed: {error}");
                setting_failure_text(&error, "کانال لاگ برداشته نشد؛ دوباره تلاش کنید.")
            }
        };
        super::respond(ctx, message, ResponseKind::SettingsChanged, response).await;
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
        super::respond(
            ctx,
            message,
            ResponseKind::SettingsView,
            super::premium::icon_html(
                Some(super::premium::Icon::DocumentActivity),
                status(ctx, chat),
            ),
        )
        .await;
        return true;
    }

    if super::named(message, Some(rest)).is_none() {
        return false;
    }
    let target = resolve(ctx, message, rest).await;
    let Some(target) = target else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "کانال پیدا نشد.\n\
                 یا «تنظیم لاگ @channel» بفرستید، یا یک پیام از کانال را در گروه فوروارد کنید \
                 و روی آن «تنظیم لاگ» بزنید.",
            ),
        )
        .await;
        return true;
    };

    let sent = ctx
        .client
        .send_message(
            target,
            super::premium::icon_html_on(
                super::premium::Surface::Channel,
                Some(super::premium::Icon::DocumentActivity),
                "<b>کانال لاگ</b>\n\nاز این پس رویدادهای گروه اینجا نوشته می شود.",
            ),
        )
        .await;
    match sent {
        Ok(_) => match stored_channel(target) {
            Some(stored) => {
                let mut mutations = Vec::with_capacity(KINDS.len() + 2);
                mutations.push(SettingMutation::Put {
                    key: CHANNEL,
                    value: &stored,
                });
                mutations.push(SettingMutation::Put { key: ON, value: "" });
                mutations.extend(
                    KINDS
                        .iter()
                        .map(|(key, _)| SettingMutation::Put { key, value: "" }),
                );
                match ctx.settings.try_apply_batch(chat, &mutations).await {
                    Ok(_) => {
                        super::respond(
                            ctx,
                            message,
                            ResponseKind::SettingsChanged,
                            super::premium::icon_text(
                                Some(super::premium::Icon::DocumentActivity),
                                "کانال لاگ تنظیم شد.",
                            ),
                        )
                        .await;
                    }
                    Err(error) => {
                        ::log::warn!("log: setup for {chat} failed: {error}");
                        super::respond(
                            ctx,
                            message,
                            ResponseKind::CommandError,
                            setting_failure_text(
                                &error,
                                "دسترسی کانال تایید شد، اما تنظیم لاگ ذخیره نشد؛ دوباره تلاش کنید.",
                            ),
                        )
                        .await;
                    }
                }
            }
            None => {
                ::log::warn!("log: target has no Bot API dialog id for {chat}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    super::premium::icon_text(
                        Some(super::premium::Icon::ErrorRed),
                        "کانال معتبر نیست و تنظیم لاگ ذخیره نشد.",
                    ),
                )
                .await;
            }
        },
        Err(e) => {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    format!(
                        "انجام نشد · {e}\n\
                     ربات باید در آن کانال ادمین باشد. اگر کانال خصوصی است، یک پیام از آن را \
                     در گروه فوروارد کنید و روی همان «تنظیم لاگ» بزنید."
                    ),
                ),
            )
            .await;
        }
    }
    true
}

fn setting_failure_text(
    error: &SettingsWriteError,
    rejected: &str,
) -> grammers_client::message::InputMessage {
    let message = match error {
        SettingsWriteError::CommitUncertain(_) => {
            "نتیجه ذخیره سازی لاگ نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید و ربات را دوباره راه اندازی کنید."
        }
        _ => rejected,
    };
    super::premium::icon_text(Some(super::premium::Icon::ErrorRed), message)
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
                header.from_id.and_then(|peer| PeerId::try_from(peer).ok())
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
