use std::sync::Arc;
use std::time::Duration;

use grammers_client::message::{Button, Message, ReplyMarkup};
use grammers_client::session::types::PeerRef;
use grammers_client::update::CallbackQuery;

use super::restrict::{self, Action};
use super::{Ctx, esc};
use crate::state::{
    CaptchaAnswerClaim, CaptchaFailureAction, CaptchaPhase, CaptchaQueueClass, CaptchaReservation,
    CaptchaReservationOutcome, CaptchaWork,
};

pub const MODE: &str = "captcha";

pub const NAMES: &[&str] = &["احراز هویت", "احراز"];

const ON_WORDS: &[&str] = &["روشن", "فعال"];
const OFF_WORDS: &[&str] = &["خاموش", "غیرفعال"];

pub const TIMEOUT: &str = "captcha_timeout";

pub const ACTION: &str = "captcha_action";

const DEFAULT_TIMEOUT: u32 = 120;
pub const TIMEOUT_RANGE: (u32, u32) = (30, 900);
pub const TIMEOUT_PRESETS: &[u32] = &[60, 120, 300, 600];

const EMOJI: &[&str] = &[
    "🍎", "🚗", "⚽", "🌙", "🎈", "🐱", "🌷", "🔑", "⭐", "🍉", "🐟", "🍌", "🚀", "🎩", "🥁", "🦋",
    "🍇", "🐘", "☂️", "🍕", "🐝", "🎸", "🧊", "🕰️",
];

pub const CHOICES: &str = "captcha_choices";
const DEFAULT_CHOICES: u32 = 3;
pub const CHOICES_RANGE: (u32, u32) = (2, 6);
pub const CHOICES_PRESETS: &[u32] = &[2, 3, 4, 5, 6];

const GLOBAL: i64 = 0;
const ARMING_LEASE_SECS: i64 = 300;
const ACTION_LEASE_SECS: i64 = 180;
const TELEGRAM_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const KICK_INTERMEDIATE_SECS: i64 = 300;
const AUTO_UNMUTE_GRACE_SECS: u64 = 300;
const MAX_ATTEMPTS: u32 = 8;
const QUARANTINE_RETRY_SECS: i64 = 3_600;
const MAX_TERMINAL_REASON_CHARS: usize = 512;
const DUE_BATCH: i64 = 512;
const DUE_PAGE: i64 = super::FLEET_CAMPAIGNS as i64;
const FRESH_SHARE: i64 = 2;
const RETRY_SHARE: i64 = 1;
const QUARANTINE_SHARE: i64 = 1;

fn parse(text: &str) -> Option<bool> {
    let tail = NAMES.iter().find_map(|name| {
        let rest = text.strip_prefix(name)?;
        rest.starts_with(char::is_whitespace)
            .then(|| rest.trim_start())
    })?;
    if ON_WORDS.contains(&tail) {
        return Some(true);
    }
    if OFF_WORDS.contains(&tail) {
        return Some(false);
    }
    None
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let Some(on) = parse(message.text().trim()) else {
        return false;
    };
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }

    if let Err(error) = ctx.settings.try_set(chat, MODE, on).await {
        ::log::warn!("captcha: mode write for {chat} failed: {error}");
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            if error.commit_outcome_unknown() {
                "نتیجه ذخیره تنظیم احراز هویت نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
            } else {
                "تنظیم احراز هویت ذخیره نشد؛ دوباره تلاش کنید."
            },
        )
        .await;
        return true;
    }
    super::respond(
        ctx,
        message,
        crate::response::ResponseKind::VerificationSetup,
        super::premium::icon_text(
            Some(super::premium::protection(on)),
            if on {
                format!(
                    "✓ احراز هویت روشن شد. عضو تازه تا {} ثانیه فرصت دارد.",
                    timeout(ctx, chat)
                )
            } else {
                "✗ احراز هویت خاموش شد.".to_owned()
            },
        ),
    )
    .await;
    true
}

pub fn choices(ctx: &Ctx, chat: i64) -> usize {
    ctx.settings
        .value_parsed(chat, CHOICES)
        .unwrap_or(DEFAULT_CHOICES) as usize
}

pub fn timeout(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value_parsed(chat, TIMEOUT)
        .unwrap_or(DEFAULT_TIMEOUT)
}

pub fn kicks(ctx: &Ctx, chat: i64) -> bool {
    ctx.settings.value(chat, ACTION).as_deref() != Some("mute")
}

pub async fn on_join(ctx: &std::sync::Arc<Ctx>, message: &Message) -> bool {
    let joined = matches!(
        message.action(),
        Some(
            grammers_client::tl::enums::MessageAction::ChatAddUser(_)
                | grammers_client::tl::enums::MessageAction::ChatJoinedByLink(_)
        )
    );
    if !joined {
        return false;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if let Err(error) = resume_durable_rejoin(ctx, message).await {
        ctx.persistence_failed(&format!(
            "join persistence for {chat} could not resume quarantined moderation: {error}"
        ));
    }
    let joined_users = super::joined_users(ctx, message).await;
    if !ctx.settings.is_locked(chat, MODE) {
        return false;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    let mut challenged = false;
    for joined in joined_users {
        if joined.is_bot
            || super::is_bot_admin(ctx, chat, joined.id)
            || super::owner(ctx, chat) == Some(joined.id)
        {
            continue;
        }
        if challenge(ctx, message, chat, chat_ref, joined).await {
            challenged = true;
        }
    }
    challenged
}

pub(super) async fn resume_durable_rejoin(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
) -> Result<(), sqlx::Error> {
    if !matches!(
        message.action(),
        Some(
            grammers_client::tl::enums::MessageAction::ChatAddUser(_)
                | grammers_client::tl::enums::MessageAction::ChatJoinedByLink(_)
        )
    ) {
        return Ok(());
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return Ok(());
    };
    let users: Vec<i64> = match message.action() {
        Some(grammers_client::tl::enums::MessageAction::ChatAddUser(action)) => {
            action.users.clone()
        }
        Some(grammers_client::tl::enums::MessageAction::ChatJoinedByLink(_)) => message
            .sender_id()
            .and_then(grammers_client::session::types::PeerId::bare_id)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    let now = unix_now();
    for user in users {
        resume_member_rejoin(ctx, chat, user, now).await?;
    }
    Ok(())
}

pub(super) async fn resume_member_rejoin(
    ctx: &Ctx,
    chat: i64,
    user: i64,
    now: i64,
) -> Result<(), sqlx::Error> {
    let _member = restrict::lock_member(ctx, chat, user).await;
    ctx.settings
        .resume_warning_on_rejoin(chat, user, now)
        .await?;
    ctx.settings
        .resume_strict_on_rejoin(chat, user, now)
        .await?;
    Ok(())
}

async fn challenge(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
    chat: i64,
    chat_ref: PeerRef,
    joined: super::Joined,
) -> bool {
    let (user, target) = (joined.id, joined.peer);
    let (answer, offered) = pick(chat, user, choices(ctx, chat));
    let seconds = timeout(ctx, chat);
    let now = unix_now();
    let due_at = now.saturating_add(i64::from(seconds));
    let safety_seconds = u64::from(seconds) + AUTO_UNMUTE_GRACE_SECS;
    let restriction_until =
        i32::try_from(now.saturating_add(i64::try_from(safety_seconds).unwrap_or(i64::MAX)))
            .unwrap_or(i32::MAX);
    let failure_action = if kicks(ctx, chat) {
        CaptchaFailureAction::Kick
    } else {
        CaptchaFailureAction::Mute
    };
    let member_guard = restrict::lock_member(ctx, chat, user).await;
    let reservation = match ctx
        .settings
        .reserve_captcha(crate::state::CaptchaReservationInput {
            chat,
            user,
            answer,
            source_message_id: message.id(),
            due_at,
            retry_at: now.saturating_add(ARMING_LEASE_SECS),
            restriction_until,
            failure_action,
        })
        .await
    {
        Ok(
            CaptchaReservationOutcome::Reserved(reservation)
            | CaptchaReservationOutcome::Resume(reservation),
        ) => reservation,
        Ok(CaptchaReservationOutcome::Duplicate) => return true,
        Ok(CaptchaReservationOutcome::CapacityReached) => {
            ctx.persistence_failed(&format!(
                "captcha capacity refused safety-critical challenge for {chat}/{user}"
            ));
            return true;
        }
        Err(error) => {
            ctx.persistence_failed(&format!(
                "captcha could not durably reserve challenge for {chat}/{user}: {error}"
            ));
            return true;
        }
    };

    let safety_mute = Duration::from_secs(safety_seconds);
    let applied = tokio::time::timeout(
        TELEGRAM_RPC_TIMEOUT,
        restrict::apply_locked_until(
            ctx,
            chat_ref,
            target,
            safety_mute,
            restriction_until,
            restrict::By {
                reason: "احراز هویت",
                target_name: &joined.name,
                case: Some(super::cases::context(
                    message,
                    "automatic",
                    MODE,
                    "احراز هویت",
                )),
                ..Default::default()
            },
            &member_guard,
        ),
    )
    .await;
    let applied = match applied {
        Ok(applied) => applied,
        Err(_) => {
            log::error!("captcha: {chat}: timed out while installing the finite mask for {user}");
            drop(member_guard);
            abort_setup(ctx, chat_ref, target, &reservation, None).await;
            return false;
        }
    };
    if let Err(error) = applied {
        eprintln!("captcha: {chat}: could not mute {user}: {error}");
        drop(member_guard);
        abort_setup(ctx, chat_ref, target, &reservation, None).await;
        return false;
    }
    if let Some(message) = reservation.superseded_message {
        delete_challenge(ctx, chat_ref, message).await;
    }

    let callback_generation = reservation.callback_generation();

    let caption = format!(
        "<b>احراز هویت</b>\n\n\
         <a href=\"tg://user?id={user}\">{}</a> همان ایموجی که در تصویر است را بزنید.\n\
         <i>{seconds} ثانیه فرصت دارید.</i>",
        esc(&joined.name),
    );
    let cached = ctx
        .settings
        .value(GLOBAL, &photo_key(answer))
        .filter(|value| !value.is_empty())
        .and_then(|value| super::welcome::decode_media(&value));

    let reused = match cached {
        None => None,
        Some(media) => {
            let input = super::premium::icon_html_on(
                super::premium::Surface::Caption,
                Some(super::premium::Icon::Locked),
                &caption,
            )
            .reply_markup(buttons(user, callback_generation, &offered))
            .media(media);
            match message.reply(input).await {
                Ok(sent) => Some(sent),
                Err(e) if super::welcome::reference_expired(&e) => {
                    eprintln!("captcha: cached photo expired, uploading a new one: {e}");
                    if let Err(error) = ctx
                        .settings
                        .try_set(GLOBAL, &photo_key(answer), false)
                        .await
                    {
                        ::log::warn!(
                            "captcha: could not remove expired cached photo {answer}: {error}"
                        );
                    }
                    None
                }
                Err(e) => {
                    eprintln!("captcha: {chat}: could not send the challenge: {e}");
                    drop(member_guard);
                    abort_setup(ctx, chat_ref, target, &reservation, None).await;
                    return false;
                }
            }
        }
    };

    let sent = match reused {
        Some(sent) => sent,
        None => {
            let mut input = super::premium::icon_html_on(
                super::premium::Surface::Caption,
                Some(super::premium::Icon::Locked),
                &caption,
            )
            .reply_markup(buttons(user, callback_generation, &offered));
            if let Some(uploaded) = upload(ctx, answer).await {
                input = input.photo(uploaded);
            }
            let fresh = match message.reply(input).await {
                Ok(fresh) => fresh,
                Err(error) => {
                    log::error!("captcha: {chat}: could not send challenge for {user}: {error}");
                    drop(member_guard);
                    abort_setup(ctx, chat_ref, target, &reservation, None).await;
                    return false;
                }
            };
            if let Some(media) = fresh.media()
                && let Some(encoded) = super::welcome::encode_media(&media)
                && let Err(error) = ctx
                    .settings
                    .try_set_value(GLOBAL, &photo_key(answer), &encoded)
                    .await
            {
                ::log::warn!("captcha: could not cache photo {answer}: {error}");
            }
            fresh
        }
    };

    let attached_at = unix_now();
    match ctx
        .settings
        .attach_captcha_message(
            &reservation,
            sent.id(),
            attached_at.saturating_add(i64::from(seconds)),
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            log::error!("captcha: {chat}: reserved challenge for {user} disappeared before attach");
            drop(member_guard);
            abort_setup(ctx, chat_ref, target, &reservation, Some(sent.id())).await;
            return false;
        }
        Err(error) => {
            log::error!("captcha: {chat}: could not attach challenge for {user}: {error}");
            drop(member_guard);
            abort_setup(ctx, chat_ref, target, &reservation, Some(sent.id())).await;
            return false;
        }
    }

    match ctx.settings.activate_captcha(&reservation, sent.id()).await {
        Ok(true) => {
            drop(member_guard);
            true
        }
        Ok(false) => {
            log::error!("captcha: {chat}: challenge for {user} disappeared before activation");
            drop(member_guard);
            abort_setup(ctx, chat_ref, target, &reservation, Some(sent.id())).await;
            false
        }
        Err(error) => {
            log::error!("captcha: {chat}: could not activate challenge for {user}: {error}");
            drop(member_guard);
            abort_setup(ctx, chat_ref, target, &reservation, Some(sent.id())).await;
            false
        }
    }
}

pub(super) async fn resume_interrupted_setup(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
) -> Result<(), sqlx::Error> {
    if !matches!(
        message.action(),
        Some(
            grammers_client::tl::enums::MessageAction::ChatAddUser(_)
                | grammers_client::tl::enums::MessageAction::ChatJoinedByLink(_)
        )
    ) {
        return Ok(());
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return Ok(());
    };
    if !ctx.settings.is_locked(chat, MODE) {
        return Ok(());
    }
    let interrupted = ctx
        .settings
        .interrupted_captcha_users(chat, message.id())
        .await?;
    if interrupted.is_empty() {
        return Ok(());
    }
    let joined_users = super::joined_users(ctx, message).await;
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        ctx.persistence_failed(&format!(
            "stale join {chat}/{} could not resolve its chat while reconstructing captcha setup",
            message.id()
        ));
        return Ok(());
    };

    for joined in joined_users {
        if interrupted.binary_search(&joined.id).is_ok() {
            let _ = challenge(ctx, message, chat, chat_ref, joined).await;
        }
    }
    let still_interrupted = ctx
        .settings
        .interrupted_captcha_users(chat, message.id())
        .await?;
    if !still_interrupted.is_empty() {
        ctx.persistence_failed(&format!(
            "stale join {chat}/{} left interrupted captcha setup for users {still_interrupted:?}",
            message.id()
        ));
    }
    Ok(())
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, chat: i64) {
    let mut fields = payload.split(':');
    let (Some(user), Some(second)) = (fields.next(), fields.next()) else {
        return;
    };
    let third = fields.next();
    if fields.next().is_some() {
        return;
    }
    let (generation, index) = match third {
        Some(index) => (second.parse::<i64>().ok(), index),
        None => (None, second),
    };
    let (Ok(user), Ok(index)) = (user.parse::<i64>(), index.parse::<usize>()) else {
        return;
    };
    if third.is_some() && generation.is_none() {
        return;
    }
    if query.sender_id().bare_id() != Some(user) {
        let _ = query
            .answer()
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Locked),
                "این آزمون برای شما نیست.",
            ))
            .send()
            .await;
        return;
    }

    let now = unix_now();
    let claim = ctx
        .settings
        .claim_captcha_answer(
            chat,
            user,
            index,
            generation,
            now,
            now.saturating_add(ACTION_LEASE_SECS),
        )
        .await;
    match claim {
        Ok(CaptchaAnswerClaim::Claimed(work)) => {
            let (Ok(Some(chat_ref)), Ok(Some(target))) =
                (query.peer_ref().await, query.sender_ref().await)
            else {
                defer(ctx, &work).await;
                return;
            };
            if complete_pass(ctx, chat_ref, target, &work).await {
                let _ = query
                    .answer()
                    .edit(super::premium::icon_html(
                        Some(super::premium::Icon::Success),
                        format!(
                            "<b>احراز هویت</b>\n\n✓ <a href=\"tg://user?id={user}\">کاربر</a> تایید شد. خوش آمدید."
                        ),
                    ))
                    .await;
            } else {
                let _ = query
                    .answer()
                    .alert(super::premium::plain_label(
                        Some(super::premium::Icon::Timer),
                        "پاسخ درست است؛ رفع سکوت در حال انجام است.",
                    ))
                    .send()
                    .await;
            }
        }
        Ok(CaptchaAnswerClaim::Incorrect) => {
            let _ = query
                .answer()
                .alert(super::premium::plain_label(
                    Some(super::premium::Icon::Warning),
                    "درست نبود. دوباره تلاش کنید.",
                ))
                .send()
                .await;
        }
        Ok(CaptchaAnswerClaim::NotReady | CaptchaAnswerClaim::AlreadyClaimed) => {
            let _ = query
                .answer()
                .alert(super::premium::plain_label(
                    Some(super::premium::Icon::Timer),
                    "در حال بررسی است. چند لحظه صبر کنید.",
                ))
                .send()
                .await;
        }
        Ok(CaptchaAnswerClaim::Missing | CaptchaAnswerClaim::Expired) => {
            let _ = query
                .answer()
                .alert(super::premium::plain_label(
                    Some(super::premium::Icon::Timer),
                    "این آزمون منقضی شده است.",
                ))
                .send()
                .await;
        }
        Err(error) => {
            log::error!("captcha: {chat}: answer claim for {user} failed: {error}");
            let _ = query
                .answer()
                .alert(super::premium::plain_label(
                    Some(super::premium::Icon::Warning),
                    "بررسی پاسخ موقتا ممکن نیست. دوباره تلاش کنید.",
                ))
                .send()
                .await;
        }
    }
}

pub async fn sweep(ctx: &Arc<Ctx>) {
    let mut handled = 0_i64;
    while handled < DUE_BATCH {
        let capacity = DUE_PAGE.min(DUE_BATCH - handled);
        let now = unix_now();
        let lease_until = now.saturating_add(ACTION_LEASE_SECS);
        let mut work = Vec::with_capacity(capacity as usize);
        for (class, share) in [
            (CaptchaQueueClass::Fresh, FRESH_SHARE),
            (CaptchaQueueClass::Retry, RETRY_SHARE),
            (CaptchaQueueClass::Quarantine, QUARANTINE_SHARE),
        ] {
            let ask = share.min(capacity - work.len() as i64);
            if ask == 0 {
                continue;
            }
            match ctx
                .settings
                .claim_due_captchas(class, now, lease_until, ask)
                .await
            {
                Ok(mut claimed) => work.append(&mut claimed),
                Err(error) => log::error!("captcha: {class:?} durable claim failed: {error}"),
            }
        }

        for class in [
            CaptchaQueueClass::Fresh,
            CaptchaQueueClass::Retry,
            CaptchaQueueClass::Quarantine,
        ] {
            let ask = capacity - work.len() as i64;
            if ask == 0 {
                break;
            }
            match ctx
                .settings
                .claim_due_captchas(class, now, lease_until, ask)
                .await
            {
                Ok(mut claimed) => work.append(&mut claimed),
                Err(error) => log::error!("captcha: {class:?} fill claim failed: {error}"),
            }
        }

        let count = work.len();
        if count == 0 {
            return;
        }
        handled += count as i64;
        let owner = Arc::clone(ctx);
        super::bounded(work, super::FLEET_CAMPAIGNS, move |work| {
            let ctx = Arc::clone(&owner);
            async move { process_due(&ctx, work).await }
        })
        .await;
        if count < capacity as usize {
            return;
        }
    }
}

async fn process_due(ctx: &Ctx, work: CaptchaWork) {
    use grammers_client::session::types::PeerId;

    let Some(target) = PeerId::user(work.user).map(PeerId::to_ambient_ref) else {
        log::error!(
            "captcha: {}/{} has an invalid durable user id",
            work.chat,
            work.user
        );
        finish(ctx, &work).await;
        return;
    };
    let Some(chat_ref) = ctx.chat_ref(work.chat) else {
        retry_or_abandon(ctx, &work, None, "chat reference is unavailable", true).await;
        return;
    };

    match work.phase {
        CaptchaPhase::Arming => {
            match release_work_restriction(ctx, chat_ref, target, &work).await {
                Ok(release) => {
                    log_release(&work, release);
                    if ctx.settings.is_locked(work.chat, MODE) {
                        quarantine(
                            ctx,
                            &work,
                            "interrupted captcha setup awaits exact join replay",
                        )
                        .await;
                    } else {
                        let completed = finish(ctx, &work).await;
                        if completed {
                            delete_optional_challenge(ctx, chat_ref, work.message_id).await;
                        }
                    }
                }
                Err((error, retryable)) => {
                    retry_or_abandon(ctx, &work, Some((chat_ref, target)), &error, retryable).await
                }
            }
        }
        CaptchaPhase::Passing => {
            match release_work_restriction(ctx, chat_ref, target, &work).await {
                Ok(release) => {
                    log_release(&work, release);
                    let completed = finish(ctx, &work).await;
                    if completed {
                        ctx.bump(work.chat, super::stats::CAPTCHA_PASSED);
                        delete_optional_challenge(ctx, chat_ref, work.message_id).await;
                    }
                }
                Err((error, retryable)) => {
                    retry_or_abandon(ctx, &work, Some((chat_ref, target)), &error, retryable).await
                }
            }
        }
        CaptchaPhase::Expiring => {
            let target_name = work.user.to_string();
            let guard = restrict::lock_member(ctx, work.chat, work.user).await;
            match ctx.settings.captcha_work_is_current(&work).await {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    drop(guard);
                    retry_or_abandon(
                        ctx,
                        &work,
                        Some((chat_ref, target)),
                        &format!("lease revalidation failed: {error}"),
                        true,
                    )
                    .await;
                    return;
                }
            }
            let ownership = match inspect_restriction(
                ctx,
                chat_ref,
                target,
                work.restriction_until,
                work.kick_until,
            )
            .await
            {
                Ok(ownership) => ownership,
                Err((error, retryable)) => {
                    drop(guard);
                    retry_or_abandon(
                        ctx,
                        &work,
                        Some((chat_ref, target)),
                        &format!("membership fence lookup failed: {error}"),
                        retryable,
                    )
                    .await;
                    return;
                }
            };
            let disposition = expiry_disposition(work.failure_action, ownership);
            if disposition == ExpiryDisposition::Superseded {
                log::info!(
                    "captcha: {}/{} expiry ownership was superseded ({ownership:?}); no failure action applied",
                    work.chat,
                    work.user
                );
                let completed = finish(ctx, &work).await;
                drop(guard);
                if completed {
                    delete_optional_challenge(ctx, chat_ref, work.message_id).await;
                }
                return;
            }
            let outcome = match work.failure_action {
                CaptchaFailureAction::Kick => match disposition {
                    ExpiryDisposition::AlreadyApplied => Ok(()),
                    ExpiryDisposition::CompleteKick => match tokio::time::timeout(
                        TELEGRAM_RPC_TIMEOUT,
                        restrict::clear_member_restriction_locked(ctx, chat_ref, target, &guard),
                    )
                    .await
                    {
                        Err(_) => Err(("interrupted kick cleanup timed out".to_owned(), true)),
                        Ok(result) => classify_kick_result(result),
                    },
                    ExpiryDisposition::Apply => {
                        begin_durable_kick(ctx, chat_ref, target, &work, &guard).await
                    }
                    ExpiryDisposition::Superseded => Err((
                        "captcha ownership was superseded before terminal dispatch".to_owned(),
                        false,
                    )),
                },
                CaptchaFailureAction::Mute => {
                    if disposition == ExpiryDisposition::AlreadyApplied {
                        Ok(())
                    } else {
                        match tokio::time::timeout(
                            TELEGRAM_RPC_TIMEOUT,
                            restrict::apply_captcha_failure_locked(
                                ctx,
                                chat_ref,
                                target,
                                Action::Mute,
                                None,
                                restrict::By {
                                    reason: "شکست احراز هویت",
                                    target_name: &target_name,
                                    ..Default::default()
                                },
                                &guard,
                            ),
                        )
                        .await
                        {
                            Err(_) => Err(("permanent mute request timed out".to_owned(), true)),
                            Ok(Ok(_)) => Ok(()),
                            Ok(Err(error)) => {
                                let retryable = restriction_retryable(&error);
                                Err((error.to_string(), retryable))
                            }
                        }
                    }
                }
            };
            match outcome {
                Ok(()) => {
                    let action = match work.failure_action {
                        CaptchaFailureAction::Kick => Action::Kick,
                        CaptchaFailureAction::Mute => Action::Mute,
                    };
                    if let Err(error) = audit_failure(ctx, &work, action, &target_name).await {
                        drop(guard);
                        retry_or_abandon(
                            ctx,
                            &work,
                            Some((chat_ref, target)),
                            &format!("action applied but workflow audit failed: {error}"),
                            true,
                        )
                        .await;
                        return;
                    }
                    let completed = finish(ctx, &work).await;
                    drop(guard);
                    if completed {
                        ctx.bump(work.chat, super::stats::CAPTCHA_FAILED);
                        delete_optional_challenge(ctx, chat_ref, work.message_id).await;
                    }
                }
                Err((error, retryable)) => {
                    drop(guard);
                    retry_or_abandon(ctx, &work, Some((chat_ref, target)), &error, retryable).await
                }
            }
        }
    }
}

async fn complete_pass(ctx: &Ctx, chat_ref: PeerRef, target: PeerRef, work: &CaptchaWork) -> bool {
    let release = match release_work_restriction(ctx, chat_ref, target, work).await {
        Ok(release) => release,
        Err((error, retryable)) => {
            log::error!(
                "captcha: {}: could not safely release {} after a correct answer: {error}",
                work.chat,
                work.user
            );
            retry_or_abandon(ctx, work, Some((chat_ref, target)), &error, retryable).await;
            return false;
        }
    };
    log_release(work, release);
    let completed = finish(ctx, work).await;
    if completed {
        ctx.bump(work.chat, super::stats::CAPTCHA_PASSED);
    }
    completed
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestrictionRelease {
    Cleared,
    AlreadyClear,
    Absent,
    OwnershipSuperseded,
    LifecycleSuperseded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestrictionOwnership {
    Owned,
    PermanentMute,
    TemporaryKick,
    AlreadyClear,
    Absent,
    Superseded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpiryDisposition {
    Apply,
    CompleteKick,
    AlreadyApplied,
    Superseded,
}

fn expiry_disposition(
    action: CaptchaFailureAction,
    ownership: RestrictionOwnership,
) -> ExpiryDisposition {
    match (action, ownership) {
        (_, RestrictionOwnership::Owned | RestrictionOwnership::AlreadyClear) => {
            ExpiryDisposition::Apply
        }
        (CaptchaFailureAction::Kick, RestrictionOwnership::TemporaryKick) => {
            ExpiryDisposition::CompleteKick
        }
        (CaptchaFailureAction::Kick, RestrictionOwnership::Absent)
        | (CaptchaFailureAction::Mute, RestrictionOwnership::PermanentMute) => {
            ExpiryDisposition::AlreadyApplied
        }
        _ => ExpiryDisposition::Superseded,
    }
}

fn owns_exact_captcha_mask(
    current: &grammers_client::tl::enums::ChatBannedRights,
    restriction_until: Option<i32>,
) -> bool {
    let Some(until) = restriction_until else {
        return false;
    };
    let expected: grammers_client::tl::enums::ChatBannedRights = super::rights::muted(until).into();
    current == &expected
}

fn owns_exact_temporary_kick_mask(
    current: &grammers_client::tl::enums::ChatBannedRights,
    kick_until: Option<i32>,
) -> bool {
    let Some(kick_until) = kick_until else {
        return false;
    };
    if kick_until == 0 {
        return false;
    }
    let expected: grammers_client::tl::enums::ChatBannedRights =
        restrict::kick_intermediate_rights(kick_until).into();
    current == &expected
}

fn restriction_ownership(
    participant: &grammers_client::tl::enums::ChannelParticipant,
    restriction_until: Option<i32>,
    kick_until: Option<i32>,
) -> RestrictionOwnership {
    use grammers_client::tl::enums::ChannelParticipant;

    match participant {
        ChannelParticipant::Banned(banned) => banned_restriction_ownership(
            banned.left,
            &banned.banned_rights,
            restriction_until,
            kick_until,
        ),
        ChannelParticipant::Participant(_) | ChannelParticipant::ParticipantSelf(_) => {
            RestrictionOwnership::AlreadyClear
        }
        ChannelParticipant::Left(_) => RestrictionOwnership::Absent,
        _ => RestrictionOwnership::Superseded,
    }
}

fn banned_restriction_ownership(
    left: bool,
    rights: &grammers_client::tl::enums::ChatBannedRights,
    restriction_until: Option<i32>,
    kick_until: Option<i32>,
) -> RestrictionOwnership {
    if left {
        return if owns_exact_temporary_kick_mask(rights, kick_until) {
            RestrictionOwnership::TemporaryKick
        } else {
            RestrictionOwnership::Superseded
        };
    }
    if owns_exact_captcha_mask(rights, restriction_until) {
        RestrictionOwnership::Owned
    } else if owns_exact_captcha_mask(rights, Some(0)) {
        RestrictionOwnership::PermanentMute
    } else {
        RestrictionOwnership::Superseded
    }
}

async fn inspect_restriction(
    ctx: &Ctx,
    chat_ref: PeerRef,
    target: PeerRef,
    restriction_until: Option<i32>,
    kick_until: Option<i32>,
) -> Result<RestrictionOwnership, (String, bool)> {
    use grammers_client::tl;

    let query = tl::functions::channels::GetParticipant {
        channel: chat_ref.into(),
        participant: target.into(),
    };
    let request = ctx.client.invoke_outbound(&query);
    match tokio::time::timeout(TELEGRAM_RPC_TIMEOUT, request).await {
        Err(_) => Err(("participant inspection timed out".to_owned(), true)),
        Ok(result) => match result {
            Ok(tl::enums::channels::ChannelParticipant::Participant(found)) => Ok(
                restriction_ownership(&found.participant, restriction_until, kick_until),
            ),
            Err(grammers_client::InvocationError::Rpc(error))
                if error.name == "USER_NOT_PARTICIPANT" =>
            {
                Ok(RestrictionOwnership::Absent)
            }
            Err(error) => Err((error.to_string(), invocation_retryable(&error))),
        },
    }
}

async fn release_exact_restriction(
    ctx: &Ctx,
    chat_ref: PeerRef,
    target: PeerRef,
    restriction_until: Option<i32>,
    guard: &restrict::MemberGuard<'_>,
) -> Result<RestrictionRelease, (String, bool)> {
    match inspect_restriction(ctx, chat_ref, target, restriction_until, None).await? {
        RestrictionOwnership::Owned => {
            match tokio::time::timeout(
                TELEGRAM_RPC_TIMEOUT,
                restrict::clear_member_restriction_locked(ctx, chat_ref, target, guard),
            )
            .await
            {
                Err(_) => Err(("restriction release timed out".to_owned(), true)),
                Ok(result) => result
                    .map(|()| RestrictionRelease::Cleared)
                    .map_err(|error| (error.to_string(), invocation_retryable(&error))),
            }
        }
        RestrictionOwnership::AlreadyClear => Ok(RestrictionRelease::AlreadyClear),
        RestrictionOwnership::Absent => Ok(RestrictionRelease::Absent),
        RestrictionOwnership::PermanentMute
        | RestrictionOwnership::TemporaryKick
        | RestrictionOwnership::Superseded => Ok(RestrictionRelease::OwnershipSuperseded),
    }
}

async fn release_work_restriction(
    ctx: &Ctx,
    chat_ref: PeerRef,
    target: PeerRef,
    work: &CaptchaWork,
) -> Result<RestrictionRelease, (String, bool)> {
    let guard = restrict::lock_member(ctx, work.chat, work.user).await;
    match ctx.settings.captcha_work_is_current(work).await {
        Ok(true) => {}
        Ok(false) => return Ok(RestrictionRelease::LifecycleSuperseded),
        Err(error) => return Err((format!("lease revalidation failed: {error}"), true)),
    }
    release_exact_restriction(ctx, chat_ref, target, work.restriction_until, &guard).await
}

async fn release_reservation_restriction(
    ctx: &Ctx,
    chat_ref: PeerRef,
    target: PeerRef,
    reservation: &CaptchaReservation,
) -> Result<RestrictionRelease, (String, bool)> {
    let guard = restrict::lock_member(ctx, reservation.chat, reservation.user).await;
    match ctx
        .settings
        .captcha_reservation_is_current(reservation)
        .await
    {
        Ok(true) => {}
        Ok(false) => return Ok(RestrictionRelease::LifecycleSuperseded),
        Err(error) => return Err((format!("reservation revalidation failed: {error}"), true)),
    }
    release_exact_restriction(
        ctx,
        chat_ref,
        target,
        Some(reservation.restriction_until),
        &guard,
    )
    .await
}

fn log_release(work: &CaptchaWork, release: RestrictionRelease) {
    if matches!(
        release,
        RestrictionRelease::OwnershipSuperseded | RestrictionRelease::LifecycleSuperseded
    ) {
        log::info!(
            "captcha: {}/{} {:?} cleanup was superseded ({release:?}); no rights were changed",
            work.chat,
            work.user,
            work.phase
        );
    }
}

async fn retry_or_abandon(
    ctx: &Ctx,
    work: &CaptchaWork,
    refs: Option<(PeerRef, PeerRef)>,
    reason: &str,
    retryable: bool,
) {
    if retryable && work.attempts < MAX_ATTEMPTS {
        log::warn!(
            "captcha: {}/{} {:?} attempt {} failed: {reason}",
            work.chat,
            work.user,
            work.phase,
            work.attempts
        );
        defer(ctx, work).await;
        return;
    }

    if retryable {
        quarantine(ctx, work, reason).await;
        return;
    }

    if matches!(work.phase, CaptchaPhase::Arming | CaptchaPhase::Passing) {
        let Some((chat_ref, target)) = refs else {
            quarantine(
                ctx,
                work,
                "peer references unavailable for terminal compensation",
            )
            .await;
            return;
        };
        match release_work_restriction(ctx, chat_ref, target, work).await {
            Ok(release) => log_release(work, release),
            Err((error, _)) => {
                quarantine(
                    ctx,
                    work,
                    &format!("terminal compensation remained unconfirmed: {error}"),
                )
                .await;
                return;
            }
        }
    }

    log::error!(
        "captcha: {}/{} terminal {:?} failure after {} attempts: {reason}",
        work.chat,
        work.user,
        work.phase,
        work.attempts
    );
    if finish(ctx, work).await
        && let Some((chat_ref, _)) = refs
    {
        delete_optional_challenge(ctx, chat_ref, work.message_id).await;
    }
}

async fn quarantine(ctx: &Ctx, work: &CaptchaWork, reason: &str) {
    let now = unix_now();
    let retry_at = now.saturating_add(QUARANTINE_RETRY_SECS);
    let reason: String = reason.chars().take(MAX_TERMINAL_REASON_CHARS).collect();
    match ctx
        .settings
        .quarantine_captcha(work, retry_at, now, &reason)
        .await
    {
        Ok(true) => log::error!(
            "captcha: {}/{} quarantined {:?} after {} attempts; retry at {retry_at}: {reason}",
            work.chat,
            work.user,
            work.phase,
            work.attempts
        ),
        Ok(false) => log::warn!(
            "captcha: {}/{} was superseded before quarantine",
            work.chat,
            work.user
        ),
        Err(error) => log::error!(
            "captcha: {}/{} could not persist quarantine after {reason}: {error}",
            work.chat,
            work.user
        ),
    }
}

async fn audit_failure(
    ctx: &Ctx,
    work: &CaptchaWork,
    action: Action,
    target_name: &str,
) -> Result<i64, sqlx::Error> {
    let workflow_key = work.workflow_key();
    let case = super::cases::CaseContext {
        source: "automatic",
        rule: MODE,
        reason: "شکست احراز هویت",
        evidence: work.message_id.map(|message| super::cases::Evidence {
            message: Some(message),
            media_kind: None,
            text: None,
            hash: None,
        }),
    };
    super::cases::record_workflow_restriction(
        ctx,
        super::cases::WorkflowRestrictionRecord {
            key: &workflow_key,
            restriction: super::cases::RestrictionRecord {
                chat: work.chat,
                target: work.user,
                target_name,
                actor: None,
                action,
                duration: None,
                case: &case,
                result: Ok(()),
            },
        },
    )
    .await
}

fn invocation_retryable(error: &grammers_client::InvocationError) -> bool {
    match error {
        grammers_client::InvocationError::Rpc(rpc) => {
            rpc.code >= 500
                || matches!(
                    rpc.name.as_str(),
                    "FLOOD_WAIT"
                        | "FLOOD_PREMIUM_WAIT"
                        | "SLOWMODE_WAIT"
                        | "CHAT_ADMIN_REQUIRED"
                        | "RIGHT_FORBIDDEN"
                        | "CHAT_WRITE_FORBIDDEN"
                        | "CHANNEL_PRIVATE"
                )
        }
        _ => true,
    }
}

async fn begin_durable_kick(
    ctx: &Ctx,
    chat_ref: PeerRef,
    target: PeerRef,
    work: &CaptchaWork,
    guard: &restrict::MemberGuard<'_>,
) -> Result<(), (String, bool)> {
    let kick_until =
        i32::try_from(unix_now().saturating_add(KICK_INTERMEDIATE_SECS)).unwrap_or(i32::MAX);
    match ctx.settings.prepare_captcha_kick(work, kick_until).await {
        Ok(true) => {}
        Ok(false) => return Err(("captcha kick lease was superseded".to_owned(), false)),
        Err(error) => {
            return Err((
                format!("could not persist kick intermediate: {error}"),
                true,
            ));
        }
    }
    match tokio::time::timeout(
        TELEGRAM_RPC_TIMEOUT,
        restrict::kick_member_exact_locked(ctx, chat_ref, target, kick_until, guard),
    )
    .await
    {
        Err(_) => Err(("kick request timed out".to_owned(), true)),
        Ok(result) => classify_kick_result(result),
    }
}

fn classify_kick_result(
    result: Result<(), grammers_client::InvocationError>,
) -> Result<(), (String, bool)> {
    match result {
        Ok(()) => Ok(()),
        Err(grammers_client::InvocationError::Rpc(error))
            if error.name == "USER_NOT_PARTICIPANT" =>
        {
            Ok(())
        }
        Err(error) => Err((error.to_string(), invocation_retryable(&error))),
    }
}

fn restriction_retryable(error: &restrict::Failed) -> bool {
    match error {
        restrict::Failed::Protected | restrict::Failed::BasicGroup => false,
        restrict::Failed::State(_) => true,
        restrict::Failed::Telegram(error) => invocation_retryable(error),
    }
}

async fn defer(ctx: &Ctx, work: &CaptchaWork) {
    let retry_at = unix_now().saturating_add(retry_delay_secs(work.attempts));
    match ctx.settings.defer_captcha(work, retry_at).await {
        Ok(true) => {}
        Ok(false) => log::warn!(
            "captcha: {}/{} {:?} was superseded before retry",
            work.chat,
            work.user,
            work.phase
        ),
        Err(error) => log::error!(
            "captcha: {}/{} could not persist retry: {error}",
            work.chat,
            work.user
        ),
    }
}

async fn finish(ctx: &Ctx, work: &CaptchaWork) -> bool {
    match ctx.settings.finish_captcha(work).await {
        Ok(done) => done,
        Err(error) => {
            log::error!(
                "captcha: {}/{} {:?} completed remotely but durable completion failed: {error}",
                work.chat,
                work.user,
                work.phase
            );
            false
        }
    }
}

async fn abort_setup(
    ctx: &Ctx,
    chat_ref: PeerRef,
    target: PeerRef,
    reservation: &CaptchaReservation,
    message: Option<i32>,
) {
    ctx.persistence_failed(&format!(
        "captcha setup for {}/{} did not reach activation",
        reservation.chat, reservation.user
    ));
    match release_reservation_restriction(ctx, chat_ref, target, reservation).await {
        Ok(release) => {
            if matches!(release, RestrictionRelease::LifecycleSuperseded) {
                log::info!(
                    "captcha: {}/{} setup cleanup was superseded; no rights were changed",
                    reservation.chat,
                    reservation.user
                );
            }
            if !ctx.settings.is_locked(reservation.chat, MODE) {
                abandon(ctx, reservation).await;
            }
            delete_optional_challenge(ctx, chat_ref, message).await;
        }
        Err((error, _)) => {
            log::error!(
                "captcha: {}: setup compensation remains unconfirmed for {}; durable row retained: {error}",
                reservation.chat,
                reservation.user
            );
        }
    }
}

async fn abandon(ctx: &Ctx, reservation: &CaptchaReservation) {
    if let Err(error) = ctx.settings.abandon_captcha(reservation).await {
        log::error!(
            "captcha: {}: could not abandon unarmed challenge for {}: {error}",
            reservation.chat,
            reservation.user
        );
    }
}

async fn delete_challenge(ctx: &Ctx, chat_ref: PeerRef, message: i32) {
    match tokio::time::timeout(
        TELEGRAM_RPC_TIMEOUT,
        ctx.client.delete_messages_critical(chat_ref, &[message]),
    )
    .await
    {
        Err(_) => log::warn!("captcha: timed out deleting challenge {message}"),
        Ok(Err(error)) => log::warn!("captcha: could not delete challenge {message}: {error}"),
        Ok(Ok(_)) => {}
    }
}

async fn delete_optional_challenge(ctx: &Ctx, chat_ref: PeerRef, message: Option<i32>) {
    if let Some(message) = message {
        delete_challenge(ctx, chat_ref, message).await;
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn retry_delay_secs(attempts: u32) -> i64 {
    5_i64.saturating_mul(1_i64 << attempts.min(6)).min(300)
}

fn photo_key(index: usize) -> String {
    format!("emoji_photo:{index}")
}

async fn upload(ctx: &Ctx, index: usize) -> Option<grammers_client::media::Uploaded> {
    let png = super::emoji_image::render(EMOJI[index])?;
    let size = png.len();
    let mut reader = std::io::Cursor::new(png);
    ctx.client
        .upload_stream(&mut reader, size, format!("captcha{index}.png"))
        .await
        .ok()
}

fn pick(chat: i64, user: i64, count: usize) -> (usize, Vec<usize>) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    (chat, user).hash(&mut hasher);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    let seed = hasher.finish();

    let count = count.min(EMOJI.len());
    let mut choices = Vec::with_capacity(count);
    let mut step = seed;
    while choices.len() < count {
        step = step.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let candidate = (step >> 33) as usize % EMOJI.len();
        if !choices.contains(&candidate) {
            choices.push(candidate);
        }
    }
    let answer = choices[(seed as usize) % count];
    (answer, choices)
}

fn buttons(user: i64, generation: i64, choices: &[usize]) -> ReplyMarkup {
    super::premium::buttons(&[choices
        .iter()
        .map(|&index| {
            Button::data(
                EMOJI[index],
                format!("c:{user}:{generation}:{index}").into_bytes(),
            )
        })
        .collect()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_client::{InvocationError, sender::RpcError};

    fn rpc(code: i32, name: &str) -> InvocationError {
        InvocationError::Rpc(RpcError {
            code,
            name: name.to_owned(),
            value: None,
            caused_by: None,
        })
    }

    #[test]
    fn the_state_word_must_be_the_whole_tail() {
        assert_eq!(parse("احراز هویت روشن"), Some(true));
        assert_eq!(parse("احراز هویت خاموش"), Some(false));
        assert_eq!(parse("احراز روشن"), Some(true));
        assert_eq!(parse("احراز خاموش"), Some(false));
        assert_eq!(parse("احراز فعال"), Some(true));
        assert_eq!(parse("احراز غیرفعال"), Some(false));

        assert_eq!(parse("احراز هویت"), None);
        assert_eq!(parse("احراز"), None);
        assert_eq!(parse("احرازی روشن"), None);
        assert_eq!(parse("احراز هویت رو روشن کن"), None);
        assert_eq!(parse("احراز روشن شد؟"), None);
        assert_eq!(parse("سلام"), None);
    }

    #[test]
    fn choices_are_distinct_and_contain_the_answer() {
        for user in 1..50 {
            let count = 2 + (user as usize % 5);
            let (answer, choices) = pick(-100, user, count);
            assert_eq!(choices.len(), count);
            let mut seen = choices.clone();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), count, "duplicate choice offered");
            assert!(choices.contains(&answer), "answer is not among the choices");
        }
    }

    #[test]
    fn retries_back_off_but_remain_bounded() {
        assert_eq!(retry_delay_secs(0), 5);
        assert_eq!(retry_delay_secs(1), 10);
        assert_eq!(retry_delay_secs(5), 160);
        assert_eq!(retry_delay_secs(6), 300);
        assert_eq!(retry_delay_secs(u32::MAX), 300);
    }

    #[test]
    fn captcha_retry_policy_distinguishes_repairable_and_terminal_failures() {
        assert!(invocation_retryable(&rpc(500, "INTERNAL")));
        assert!(invocation_retryable(&rpc(400, "CHAT_ADMIN_REQUIRED")));
        assert!(!invocation_retryable(&rpc(400, "USER_NOT_PARTICIPANT")));
        assert!(!restriction_retryable(&restrict::Failed::Protected));
    }

    #[test]
    fn captcha_only_claims_the_exact_finite_mask_it_installed() {
        let installed: grammers_client::tl::enums::ChatBannedRights =
            super::super::rights::muted(1_234).into();
        let newer: grammers_client::tl::enums::ChatBannedRights =
            super::super::rights::muted(1_235).into();

        assert!(owns_exact_captcha_mask(&installed, Some(1_234)));
        assert!(!owns_exact_captcha_mask(&newer, Some(1_234)));
        assert!(
            !owns_exact_captcha_mask(&installed, None),
            "legacy rows cannot safely claim ownership without their persisted expiry"
        );

        let permanent: grammers_client::tl::enums::ChatBannedRights =
            super::super::rights::muted(0).into();
        assert_eq!(
            banned_restriction_ownership(false, &permanent, Some(1_234), None),
            RestrictionOwnership::PermanentMute,
            "a crash replay must recognize the exact permanent terminal mask"
        );
    }

    #[test]
    fn natural_mask_expiry_does_not_acknowledge_current_failure_work() {
        assert_eq!(
            expiry_disposition(
                CaptchaFailureAction::Kick,
                RestrictionOwnership::AlreadyClear
            ),
            ExpiryDisposition::Apply
        );
        assert_eq!(
            expiry_disposition(
                CaptchaFailureAction::Mute,
                RestrictionOwnership::AlreadyClear
            ),
            ExpiryDisposition::Apply
        );
    }

    #[test]
    fn permanent_mute_crash_window_replay_recognizes_its_terminal_state() {
        assert_eq!(
            expiry_disposition(
                CaptchaFailureAction::Mute,
                RestrictionOwnership::PermanentMute
            ),
            ExpiryDisposition::AlreadyApplied
        );
        assert_eq!(
            expiry_disposition(
                CaptchaFailureAction::Kick,
                RestrictionOwnership::PermanentMute
            ),
            ExpiryDisposition::Superseded
        );
    }

    #[test]
    fn interrupted_kick_replay_only_completes_the_exact_temporary_tl_mask() {
        let temporary: grammers_client::tl::enums::ChatBannedRights =
            restrict::kick_intermediate_rights(1_234).into();
        assert!(owns_exact_temporary_kick_mask(&temporary, Some(1_234)));
        assert_eq!(
            expiry_disposition(
                CaptchaFailureAction::Kick,
                RestrictionOwnership::TemporaryKick
            ),
            ExpiryDisposition::CompleteKick
        );

        let grammers_client::tl::enums::ChatBannedRights::Rights(mut wider) = temporary;
        wider.send_messages = true;
        assert!(
            !owns_exact_temporary_kick_mask(&wider.into(), Some(1_234)),
            "a wider administrator ban must never be cleared as kick cleanup"
        );
        assert!(
            !owns_exact_temporary_kick_mask(&restrict::kick_intermediate_rights(0).into(), Some(0)),
            "a permanent ban is not grammers' temporary kick intermediate"
        );
    }

    #[test]
    fn exhausted_retryable_work_moves_to_a_bounded_low_frequency_cadence() {
        assert!(QUARANTINE_RETRY_SECS > retry_delay_secs(MAX_ATTEMPTS));
        assert_eq!(
            "x".repeat(MAX_TERMINAL_REASON_CHARS + 1)
                .chars()
                .take(MAX_TERMINAL_REASON_CHARS)
                .count(),
            MAX_TERMINAL_REASON_CHARS
        );
    }

    #[test]
    fn sweep_reserves_capacity_for_every_queue_class() {
        assert_eq!(FRESH_SHARE + RETRY_SHARE + QUARANTINE_SHARE, DUE_PAGE);
        const { assert!(FRESH_SHARE > 0) };
        const { assert!(RETRY_SHARE > 0) };
        const { assert!(QUARANTINE_SHARE > 0) };
        assert_eq!(DUE_PAGE, super::super::FLEET_CAMPAIGNS as i64);
    }
}
