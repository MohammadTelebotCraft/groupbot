use std::time::Duration;

use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::PeerRef;
use grammers_client::tl;

use crate::response::ResponseKind;
use crate::state::{
    DefaultRightsError, EffectiveRights, RightsMask, RightsNoticeKind, RightsSnapshot,
};

use super::{Ctx, GroupRightsGuard};

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
const NOTICE_TIMEOUT: Duration = Duration::from_secs(10);
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
const RUN_BUDGET: Duration = Duration::from_secs(55);
const DUE_PAGE: i64 = 64;
const MAX_PAGES_PER_RUN: usize = 64;

pub struct Right {
    pub key: &'static str,
    pub label: &'static str,
    pub bit: u8,
}

pub const RIGHTS: &[Right] = &[
    Right {
        key: "plain",
        label: "ارسال پیام",
        bit: 0,
    },
    Right {
        key: "photos",
        label: "ارسال عکس",
        bit: 1,
    },
    Right {
        key: "videos",
        label: "ارسال ویدیو",
        bit: 2,
    },
    Right {
        key: "rounds",
        label: "ارسال ویدیو سلفی",
        bit: 3,
    },
    Right {
        key: "audios",
        label: "ارسال آهنگ",
        bit: 4,
    },
    Right {
        key: "voices",
        label: "ارسال ویس",
        bit: 5,
    },
    Right {
        key: "docs",
        label: "ارسال فایل",
        bit: 6,
    },
    Right {
        key: "stickers",
        label: "ارسال استیکر و گیف",
        bit: 7,
    },
    Right {
        key: "polls",
        label: "ارسال نظرسنجی",
        bit: 8,
    },
    Right {
        key: "links",
        label: "پیش نمایش لینک",
        bit: 9,
    },
    Right {
        key: "reactions",
        label: "ری اکشن به پیام",
        bit: 10,
    },
    Right {
        key: "info",
        label: "تغییر اطلاعات گروه",
        bit: 11,
    },
    Right {
        key: "invite",
        label: "دعوت کاربران",
        bit: 12,
    },
    Right {
        key: "pin",
        label: "سنجاق کردن پیام",
        bit: 13,
    },
];

const OPEN_WORDS: &[&str] = &["باز", "آزاد", "روشن"];
const CLOSED_WORDS: &[&str] = &["بسته", "قفل", "خاموش"];
pub const SHOW: &[&str] = &["اختیارات گروه", "اختیارات", "مجوزها", "مجوزهای گروه"];
pub const SET: &[&str] = &["اختیار", "مجوز"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryOutcome {
    Applied,
    PendingRetry {
        retry_at: i64,
        reason: String,
    },
    AcceptedDeliveryUnknown {
        reason: String,
    },
    Superseded,
}

#[derive(Debug)]
pub enum ChangeError {
    State(DefaultRightsError),
    LiveRightsUnavailable,
}

impl std::fmt::Display for ChangeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State(error) => error.fmt(formatter),
            Self::LiveRightsUnavailable => write!(formatter, "live group rights are unavailable"),
        }
    }
}

impl std::error::Error for ChangeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            Self::LiveRightsUnavailable => None,
        }
    }
}

impl From<DefaultRightsError> for ChangeError {
    fn from(error: DefaultRightsError) -> Self {
        Self::State(error)
    }
}

pub fn right(key: &str) -> Option<&'static Right> {
    RIGHTS.iter().find(|right| right.key == key)
}

pub fn closed(snapshot: &RightsSnapshot, right: &Right) -> bool {
    snapshot.base.contains(right.bit)
}

pub async fn snapshot(ctx: &Ctx, chat: i64) -> Result<Option<RightsSnapshot>, DefaultRightsError> {
    ctx.settings.default_rights(chat).await
}

fn build(effective: EffectiveRights) -> tl::types::ChatBannedRights {
    let shut = |bit| effective.force_all || effective.base.contains(bit);
    let (photos, videos, rounds) = (shut(1), shut(2), shut(3));
    let (audios, voices, docs) = (shut(4), shut(5), shut(6));
    tl::types::ChatBannedRights {
        view_messages: false,
        send_messages: false,
        send_media: photos && videos && rounds && audios && voices && docs,
        send_photos: photos,
        send_videos: videos,
        send_roundvideos: rounds,
        send_audios: audios,
        send_voices: voices,
        send_docs: docs,
        send_plain: shut(0),
        send_stickers: shut(7),
        send_gifs: shut(7),
        send_games: shut(7),
        send_inline: shut(7),
        send_polls: shut(8),
        embed_links: shut(9),
        send_reactions: shut(10),
        change_info: shut(11),
        invite_users: shut(12),
        pin_messages: shut(13),
        manage_topics: effective.force_all,
        manage_linked_peers: effective.force_all,
        edit_rank: false,
        until_date: 0,
    }
}

pub fn muted(until_date: i32) -> tl::types::ChatBannedRights {
    let base = RightsMask::from_storage((1_i32 << 11) - 1)
        .expect("the compile-time mute mask contains only known rights");
    let mut rights = build(EffectiveRights {
        base,
        force_all: false,
    });
    rights.manage_topics = true;
    rights.until_date = until_date;
    rights
}

fn now() -> i64 {
    i64::try_from(super::stats::local_seconds()).unwrap_or(i64::MAX)
}

fn mask_from_live(message: &Message) -> Option<RightsMask> {
    let grammers_client::peer::Peer::Group(group) = message.peer()? else {
        return None;
    };
    let current = match &group.raw {
        tl::enums::Chat::Channel(channel) => channel.default_banned_rights.clone(),
        tl::enums::Chat::Chat(chat) => chat.default_banned_rights.clone(),
        _ => None,
    }?;
    let tl::enums::ChatBannedRights::Rights(current) = current;
    let flags = [
        current.send_plain || current.send_messages,
        current.send_photos || current.send_media,
        current.send_videos || current.send_media,
        current.send_roundvideos || current.send_media,
        current.send_audios || current.send_media,
        current.send_voices || current.send_media,
        current.send_docs || current.send_media,
        current.send_stickers,
        current.send_polls,
        current.embed_links,
        current.send_reactions,
        current.change_info,
        current.invite_users,
        current.pin_messages,
    ];
    RightsMask::from_storage(flags.iter().enumerate().fold(0_i32, |mask, (bit, closed)| {
        mask | if *closed { 1_i32 << bit } else { 0 }
    }))
    .ok()
}

pub async fn seed(ctx: &Ctx, message: &Message, chat: i64) -> Result<RightsSnapshot, ChangeError> {
    let _guard = ctx.group_rights(chat).await;
    if let Some(snapshot) = ctx.settings.default_rights(chat).await?
        && snapshot.seeded
    {
        return Ok(snapshot);
    }
    let base = mask_from_live(message).ok_or(ChangeError::LiveRightsUnavailable)?;
    Ok(ctx
        .settings
        .seed_default_rights(chat, base, now())
        .await?
        .snapshot)
}

pub async fn set_right(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    right: &Right,
    shut: bool,
) -> Result<DeliveryOutcome, DefaultRightsError> {
    let guard = ctx.group_rights(chat).await;
    let accepted = ctx
        .settings
        .set_default_right(chat, right.bit, shut, now())
        .await?;
    if !accepted.delivery_required {
        return Ok(DeliveryOutcome::Applied);
    }
    Ok(reconcile_guarded(ctx, &guard, Some(chat_ref)).await)
}

pub async fn set_manual_lock(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    locked: bool,
) -> Result<DeliveryOutcome, DefaultRightsError> {
    let guard = ctx.group_rights(chat).await;
    let accepted = ctx
        .settings
        .set_default_rights_manual_lock(chat, locked, now())
        .await?;
    if !accepted.delivery_required {
        return Ok(DeliveryOutcome::Applied);
    }
    Ok(reconcile_guarded(ctx, &guard, Some(chat_ref)).await)
}

pub async fn set_timed_lock(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    until: i64,
) -> Result<DeliveryOutcome, DefaultRightsError> {
    let guard = ctx.group_rights(chat).await;
    let accepted = ctx
        .settings
        .set_default_rights_timed_lock(chat, until, now())
        .await?;
    if !accepted.delivery_required {
        return Ok(DeliveryOutcome::Applied);
    }
    Ok(reconcile_guarded(ctx, &guard, Some(chat_ref)).await)
}

pub async fn set_night(
    ctx: &Ctx,
    chat_ref: Option<PeerRef>,
    chat: i64,
    window: Option<crate::state::NightWindow>,
) -> Result<DeliveryOutcome, DefaultRightsError> {
    let guard = ctx.group_rights(chat).await;
    let accepted = ctx
        .settings
        .set_default_rights_night(chat, window, now())
        .await?;
    if !accepted.delivery_required {
        return Ok(DeliveryOutcome::Applied);
    }
    Ok(reconcile_guarded(ctx, &guard, chat_ref).await)
}

async fn reconcile_guarded(
    ctx: &Ctx,
    guard: &GroupRightsGuard<'_>,
    chat_ref: Option<PeerRef>,
) -> DeliveryOutcome {
    let current = now();
    let claim = match ctx
        .settings
        .claim_default_rights(guard.chat(), current)
        .await
    {
        Ok(Some(claim)) => claim,
        Ok(None) => return DeliveryOutcome::Superseded,
        Err(error) => {
            return DeliveryOutcome::AcceptedDeliveryUnknown {
                reason: format!("could not claim delivery: {error}"),
            };
        }
    };
    let effective = match ctx.settings.claimed_default_rights(&claim, current).await {
        Ok(Some(effective)) => effective,
        Ok(None) => return DeliveryOutcome::Superseded,
        Err(error) => {
            return DeliveryOutcome::AcceptedDeliveryUnknown {
                reason: format!("could not verify claimed delivery: {error}"),
            };
        }
    };
    let Some(chat_ref) = chat_ref else {
        let reason = "chat reference is unavailable".to_owned();
        return match ctx
            .settings
            .retry_default_rights(&claim, current, &reason)
            .await
        {
            Ok(Some(retry_at)) => DeliveryOutcome::PendingRetry { retry_at, reason },
            Ok(None) => DeliveryOutcome::Superseded,
            Err(error) => DeliveryOutcome::AcceptedDeliveryUnknown {
                reason: format!("{reason}; retry could not be recorded: {error}"),
            },
        };
    };
    let request = tl::functions::messages::EditChatDefaultBannedRights {
        peer: chat_ref.into(),
        banned_rights: build(effective).into(),
    };
    let request = ctx.client.invoke_outbound(&request);
    let result = tokio::time::timeout(DELIVERY_TIMEOUT, request).await;
    let failure = match result {
        Ok(Ok(_)) => None,
        Ok(Err(grammers_client::InvocationError::Rpc(ref rpc)))
            if rpc.name == "CHAT_NOT_MODIFIED" =>
        {
            None
        }
        Err(_) => Some(format!(
            "Telegram rights request exceeded {DELIVERY_TIMEOUT:?}"
        )),
        Ok(Err(error)) => Some(error.to_string()),
    };
    let Some(reason) = failure else {
        return match ctx
            .settings
            .ack_default_rights(&claim, effective.fingerprint())
            .await
        {
            Ok(true) => DeliveryOutcome::Applied,
            Ok(false) => DeliveryOutcome::Superseded,
            Err(error) => DeliveryOutcome::AcceptedDeliveryUnknown {
                reason: format!("Telegram accepted rights, but acknowledgement failed: {error}"),
            },
        };
    };
    match ctx
        .settings
        .retry_default_rights(&claim, current, &reason)
        .await
    {
        Ok(Some(retry_at)) => DeliveryOutcome::PendingRetry { retry_at, reason },
        Ok(None) => DeliveryOutcome::Superseded,
        Err(error) => DeliveryOutcome::AcceptedDeliveryUnknown {
            reason: format!("{reason}; retry could not be recorded: {error}"),
        },
    }
}

fn notice_text(kind: RightsNoticeKind) -> &'static str {
    match kind {
        RightsNoticeKind::TimedOpened => "<b>قفل گروه</b>\n\nمهلت قفل تمام شد و گروه باز شد.",
        RightsNoticeKind::NightLocked => "<b>قفل شب</b>\n\nگروه تا صبح بسته شد.",
        RightsNoticeKind::NightOpened => "<b>قفل شب</b>\n\nگروه باز شد.",
    }
}

async fn deliver_notice_guarded(
    ctx: &Ctx,
    guard: &GroupRightsGuard<'_>,
    chat_ref: Option<PeerRef>,
) {
    let current = now();
    let claim = match ctx
        .settings
        .claim_default_rights_notice(guard.chat(), current)
        .await
    {
        Ok(Some(claim)) => claim,
        Ok(None) => return,
        Err(error) => {
            log::warn!(
                "default rights: {} notice claim failed: {error}",
                guard.chat()
            );
            return;
        }
    };
    let result = match chat_ref {
        Some(chat_ref) => tokio::time::timeout(
            NOTICE_TIMEOUT,
            ctx.client
                .send_message(chat_ref, InputMessage::new().html(notice_text(claim.kind))),
        )
        .await
        .map_err(|_| format!("Telegram notice request exceeded {NOTICE_TIMEOUT:?}"))
        .and_then(|result| result.map(|_| ()).map_err(|error| error.to_string())),
        None => Err("chat reference is unavailable".to_owned()),
    };
    match result {
        Ok(()) => match ctx.settings.ack_default_rights_notice(&claim).await {
            Ok(true) => {}
            Ok(false) => log::debug!(
                "default rights: {} notice was superseded before acknowledgement",
                guard.chat()
            ),
            Err(error) => log::warn!(
                "default rights: {} notice acknowledgement failed: {error}",
                guard.chat()
            ),
        },
        Err(reason) => match ctx
            .settings
            .retry_default_rights_notice(&claim, current, &reason)
            .await
        {
            Ok(Some(retry_at)) => log::warn!(
                "default rights: {} notice retry at {retry_at}: {reason}",
                guard.chat()
            ),
            Ok(None) => {}
            Err(error) => log::warn!(
                "default rights: {} notice retry was not recorded: {error}",
                guard.chat()
            ),
        },
    }
}

pub async fn run_due(ctx: &std::sync::Arc<Ctx>) {
    let started = tokio::time::Instant::now();
    let mut offset = 0_i64;
    for _ in 0..MAX_PAGES_PER_RUN {
        if started.elapsed() >= RUN_BUDGET {
            log::warn!(
                "default rights: wall-clock scheduler budget exhausted; work remains queued"
            );
            return;
        }
        let due = match ctx
            .settings
            .default_rights_due_page(now(), DUE_PAGE, offset)
            .await
        {
            Ok(due) => due,
            Err(error) => {
                log::warn!("default rights: due-page query failed: {error}");
                return;
            }
        };
        if due.is_empty() {
            return;
        }
        let page_full = due.len() == usize::try_from(DUE_PAGE).unwrap_or(usize::MAX);
        offset = offset.saturating_add(i64::try_from(due.len()).unwrap_or(i64::MAX));
        let owner = std::sync::Arc::clone(ctx);
        let page = super::bounded(due, super::FLEET_CONCURRENCY, move |chat| {
            let ctx = std::sync::Arc::clone(&owner);
            async move {
                let Some(guard) = ctx.try_group_rights(chat) else {
                    return;
                };
                let chat_ref = tokio::time::timeout(RESOLVE_TIMEOUT, ctx.resolve_group(chat))
                    .await
                    .ok()
                    .flatten();
                match reconcile_guarded(&ctx, &guard, chat_ref).await {
                    DeliveryOutcome::Applied | DeliveryOutcome::Superseded => {}
                    DeliveryOutcome::PendingRetry { retry_at, reason } => {
                        log::warn!("default rights: {chat} retry at {retry_at}: {reason}");
                    }
                    DeliveryOutcome::AcceptedDeliveryUnknown { reason } => {
                        log::warn!("default rights: {chat} delivery state unknown: {reason}");
                    }
                }
                deliver_notice_guarded(&ctx, &guard, chat_ref).await;
            }
        });
        let remaining = RUN_BUDGET.saturating_sub(started.elapsed());
        if tokio::time::timeout(remaining, page).await.is_err() {
            log::warn!(
                "default rights: wall-clock scheduler budget exhausted; leases will recover"
            );
            return;
        }
        if !page_full {
            return;
        }
    }
    log::warn!("default rights: bounded scheduler budget exhausted; due work remains queued");
}

pub fn status(snapshot: &RightsSnapshot) -> String {
    let lines = RIGHTS
        .iter()
        .map(|right| {
            format!(
                "{} {}",
                if closed(snapshot, right) {
                    "✗"
                } else {
                    "✓"
                },
                right.label
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let pending = snapshot
        .pending(now())
        .then_some("\n\n<i>تغییر ذخیره شده است و تحویل آن به تلگرام دوباره تلاش می شود.</i>");
    format!(
        "<b>اختیارات گروه</b>\n\n{lines}{}\n\n\
         <i>این ها اختیار اعضای عادی است و ادمین ها شامل آن نمی شوند. \
         تغییر با دستور: «اختیار عکس بسته»</i>",
        pending.unwrap_or("")
    )
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if SHOW.contains(&text) {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        let snapshot = match seed(ctx, message, chat).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                log::warn!("rights: could not seed {chat}: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    "اختیارات گروه خوانده نشد؛ دوباره تلاش کنید.",
                )
                .await;
                return true;
            }
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
            ResponseKind::LockManagement,
            super::premium::html(status(&snapshot))
                .reply_markup(super::panel::rights_markup(&snapshot, chat, opener)),
        )
        .await;
        return true;
    }
    let Some(rest) = SET.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        rest.starts_with(char::is_whitespace).then(|| rest.trim())
    }) else {
        return false;
    };
    let Some((name, state)) = rest.rsplit_once(char::is_whitespace) else {
        return false;
    };
    let shut = match (OPEN_WORDS.contains(&state), CLOSED_WORDS.contains(&state)) {
        (true, _) => false,
        (_, true) => true,
        _ => return false,
    };
    let name = name.trim();
    let Some(right) = RIGHTS
        .iter()
        .find(|right| right.label == name || right.label.ends_with(name))
    else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "چنین اختیاری نداریم.",
        )
        .await;
        return true;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };
    if let Err(error) = seed(ctx, message, chat).await {
        log::warn!("rights: could not seed {chat}: {error}");
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "اختیارات گروه خوانده نشد؛ دوباره تلاش کنید.",
        )
        .await;
        return true;
    }
    let outcome = set_right(ctx, chat_ref, chat, right, shut).await;
    let (kind, icon, text) = match outcome {
        Ok(DeliveryOutcome::Applied) => (
            ResponseKind::LockManagement,
            super::premium::permission(right.key, !shut),
            if shut {
                format!("✗ {} برای اعضای عادی بسته شد.", right.label)
            } else {
                format!("✓ {} برای اعضای عادی باز شد.", right.label)
            },
        ),
        Ok(DeliveryOutcome::PendingRetry { .. } | DeliveryOutcome::Superseded) => (
            ResponseKind::LockManagement,
            super::premium::Icon::Timer,
            "تغییر ذخیره شد، اما تلگرام هنوز آن را نپذیرفته است؛ دوباره تلاش می شود.".to_owned(),
        ),
        Ok(DeliveryOutcome::AcceptedDeliveryUnknown { reason }) => {
            log::warn!("rights: accepted mutation for {chat} has unknown delivery: {reason}");
            (
                ResponseKind::LockManagement,
                super::premium::Icon::Timer,
                "تغییر ذخیره شد، اما وضعیت تحویل آن به تلگرام مشخص نیست.".to_owned(),
            )
        }
        Err(error) if error.acceptance_unknown() => {
            log::warn!("rights: mutation outcome unknown for {chat}: {error}");
            (
                ResponseKind::CommandError,
                super::premium::Icon::Timer,
                "وضعیت ذخیره سازی مشخص نیست؛ پیش از تکرار، پنل را دوباره بررسی کنید.".to_owned(),
            )
        }
        Err(error) => {
            log::warn!("rights: mutation not accepted for {chat}: {error}");
            (
                ResponseKind::CommandError,
                super::premium::Icon::ErrorRed,
                "تغییر ذخیره نشد؛ دوباره تلاش کنید.".to_owned(),
            )
        }
    };
    super::respond(
        ctx,
        message,
        kind,
        super::premium::icon_text(Some(icon), text),
    )
    .await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanket_flags_follow_granular_rights() {
        let base = RightsMask::from_storage(((1_i32 << 7) - 1) & !1).unwrap();
        let rights = build(EffectiveRights {
            base,
            force_all: false,
        });
        assert!(rights.send_media);
        assert!(!rights.send_messages && !rights.view_messages);
        assert!(
            build(EffectiveRights {
                base: RightsMask::empty(),
                force_all: true
            })
            .send_plain
        );
    }

    #[test]
    fn mute_closes_every_way_of_speaking_but_not_admin_rights() {
        let rights = muted(1_800);
        assert!(rights.send_plain && rights.send_media && rights.send_reactions);
        assert!(rights.send_photos && rights.send_videos && rights.send_roundvideos);
        assert!(rights.send_audios && rights.send_voices && rights.send_docs);
        assert!(rights.send_stickers && rights.send_gifs && rights.send_games);
        assert!(rights.send_inline && rights.send_polls && rights.embed_links);
        assert!(rights.manage_topics);
        assert!(!rights.view_messages);
        assert!(!rights.change_info && !rights.invite_users && !rights.pin_messages);
        assert_eq!(rights.until_date, 1_800);
    }

    #[test]
    fn every_right_has_unique_key_label_and_bit() {
        let mut keys: Vec<_> = RIGHTS.iter().map(|right| right.key).collect();
        let mut labels: Vec<_> = RIGHTS.iter().map(|right| right.label).collect();
        let mut bits: Vec<_> = RIGHTS.iter().map(|right| right.bit).collect();
        let key_count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), key_count);
        let label_count = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), label_count);
        let bit_count = bits.len();
        bits.sort_unstable();
        bits.dedup();
        assert_eq!(bits.len(), bit_count);
    }
}
