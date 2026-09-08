use std::sync::Arc;
use std::time::Duration;

use grammers_client::message::Message;
use grammers_client::session::types::{PeerId, PeerRef};

use super::restrict::{self, Action};
use super::{Ctx, esc, name_of};
use crate::state::{
    PendingStrictAction, StrictAction, StrictIncrement, StrictIntentState, StrictQueueClass,
    StrictViolation,
};

pub const MODE: &str = "strict";
pub const ACTION: &str = "strict_action";
pub const LIMIT: &str = "strict_limit";
pub const TIME: &str = "strict_time";

const DEFAULT_LIMIT: u32 = 1;
pub const LIMIT_RANGE: (u32, u32) = (1, 20);
pub const TIME_RANGE: (u32, u32) = (0, 10_080);
pub const LIMIT_PRESETS: &[u32] = &[1, 2, 3, 5, 10];
pub const TIME_PRESETS: &[u32] = &[0, 5, 60, 720, 1440];

const TALLY_DAYS: u64 = 7;
const ACTION_LEASE_SECS: i64 = 180;
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(120);
const SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(10);
const RECOVERY_PAGE: i64 = super::FLEET_CAMPAIGNS as i64;
const RECOVERY_LIMIT: usize = 100;
const FRESH_RECOVERY_SHARE: i64 = 3;
const RETRY_RECOVERY_SHARE: i64 = 1;
const DEAD_LETTER_RETENTION_SECS: i64 = 30 * 86_400;
const DEAD_LETTER_PURGE_PAGE: i64 = 64;

pub const PICK: &str = "strict:";
pub const FILTER: &str = "filter";
pub const PACK: &str = "pack";

pub fn pick_key(cause: &str) -> String {
    format!("{PICK}{cause}")
}

pub async fn try_sync_pick(
    ctx: &Ctx,
    chat: i64,
    lock: &str,
    on: bool,
) -> Result<(), crate::state::SettingsWriteError> {
    if ctx.settings.indexed_empty(chat, PICK) {
        return Ok(());
    }
    ctx.settings.try_set(chat, &pick_key(lock), on).await?;
    Ok(())
}

pub fn counts(ctx: &Ctx, chat: i64, cause: &str) -> bool {
    ctx.settings.indexed_empty(chat, PICK) || ctx.settings.is_locked(chat, &pick_key(cause))
}

pub fn action_of(ctx: &Ctx, chat: i64) -> Action {
    match ctx.settings.value(chat, ACTION).as_deref() {
        Some("ban") => Action::Ban,
        _ => Action::Mute,
    }
}

pub fn is_ban(ctx: &Ctx, chat: i64) -> bool {
    action_of(ctx, chat) == Action::Ban
}

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value_parsed(chat, LIMIT)
        .unwrap_or(DEFAULT_LIMIT)
}

pub fn minutes(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings.value_parsed(chat, TIME).unwrap_or(0)
}

pub fn duration(ctx: &Ctx, chat: i64) -> Option<Duration> {
    let minutes = minutes(ctx, chat);
    (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60))
}

pub fn time_label(minutes: u32) -> String {
    if minutes == 0 {
        "دائمی".to_owned()
    } else {
        super::log::duration_label(u64::from(minutes) * 60)
    }
}

pub enum Outcome {
    Nothing,
    Chances(u32),
    Announced,
}

pub async fn punish(ctx: &Ctx, message: &Message, chat: i64, cause: &str) -> Outcome {
    if !ctx.settings.is_locked(chat, MODE) || !counts(ctx, chat, cause) {
        return Outcome::Nothing;
    }
    let (Ok(Some(chat_ref)), Ok(Some(target))) =
        (message.peer_ref().await, message.sender_ref().await)
    else {
        return Outcome::Nothing;
    };
    punish_refs(ctx, chat, chat_ref, target, &name_of(message)).await
}

pub async fn punish_detached(
    ctx: &Ctx,
    chat: i64,
    chat_ref: PeerRef,
    sender: Option<i64>,
    name: &str,
    cause: &str,
) -> Outcome {
    if !ctx.settings.is_locked(chat, MODE) || !counts(ctx, chat, cause) {
        return Outcome::Nothing;
    }
    let Some(target) = sender.and_then(PeerId::user).map(PeerId::to_ambient_ref) else {
        return Outcome::Nothing;
    };
    punish_refs(ctx, chat, chat_ref, target, name).await
}

async fn punish_refs(
    ctx: &Ctx,
    chat: i64,
    chat_ref: PeerRef,
    target: PeerRef,
    name: &str,
) -> Outcome {
    let Some(user) = target.id.bare_id() else {
        return Outcome::Nothing;
    };
    let limit = limit(ctx, chat);
    let action = action_of(ctx, chat);
    let duration = duration(ctx, chat);
    let until_date = restrict::strict_until_date(duration);
    let durable_action = if action == Action::Ban {
        StrictAction::Ban
    } else {
        StrictAction::Mute
    };
    let wipe_history = ctx.settings.is_locked(
        chat,
        if action == Action::Ban {
            restrict::WIPE_BAN
        } else {
            restrict::WIPE_MUTE
        },
    );
    let increment: StrictIncrement = match ctx
        .settings
        .increment_strict(StrictViolation {
            chat,
            user,
            day: super::stats::today(),
            expiry_days: TALLY_DAYS,
            limit,
            action: durable_action,
            duration_seconds: duration.map(|duration| duration.as_secs()),
            until_date,
            target_name: name,
            wipe_history,
        })
        .await
    {
        Ok(Some(increment)) => increment,
        Ok(None) => {
            log::error!(
                "strict mode: durable capacity refused {chat}/{user}; no restriction was attempted"
            );
            return Outcome::Nothing;
        }
        Err(error) => {
            log::error!(
                "strict mode: atomic strike/intent write for {chat}/{user} failed; no restriction was attempted: {error}"
            );
            return Outcome::Nothing;
        }
    };
    match increment.intent {
        StrictIntentState::None => return Outcome::Chances(limit - increment.count),
        StrictIntentState::Active => {}
    }

    let now = unix_now();
    let pending = match ctx
        .settings
        .claim_strict_action(chat, user, now, now.saturating_add(ACTION_LEASE_SECS))
        .await
    {
        Ok(Some(pending)) => pending,
        Ok(None) => return Outcome::Nothing,
        Err(error) => {
            log::error!(
                "strict mode: accepted action for {chat}/{user} could not be claimed: {error}"
            );
            return Outcome::Nothing;
        }
    };
    let StrictExecution::Applied = execute_claimed(ctx, &pending, chat_ref, target).await else {
        return Outcome::Nothing;
    };

    let action = action_for(&pending);
    let what = if pending.action == StrictAction::Ban {
        "از گروه اخراج شد"
    } else {
        "سکوت شد"
    };
    let how_many = if pending.threshold > 1 {
        format!(" پس از {} تخلف", pending.threshold)
    } else {
        String::new()
    };
    let how_long = pending
        .duration_seconds
        .map_or_else(String::new, |seconds| {
            format!(" به مدت {}", super::log::duration_label(seconds))
        });
    if let Err(error) = ctx
        .client
        .send_message(
            chat_ref,
            super::premium::icon_html(
                action.icon(),
                format!(
                    "<b>{}</b> به دلیل ارسال مورد قفل شده{how_many} {what}{how_long}.",
                    esc(&pending.target_name)
                ),
            ),
        )
        .await
    {
        log::warn!(
            "strict mode: {}/{} action applied but announcement failed: {error}",
            pending.chat,
            pending.user
        );
    }
    Outcome::Announced
}

enum StrictExecution {
    Applied,
    Expired,
    Failed,
    Superseded,
}

fn action_for(pending: &PendingStrictAction) -> Action {
    match pending.action {
        StrictAction::Ban => Action::Ban,
        StrictAction::Mute => Action::Mute,
    }
}

async fn execute_claimed(
    ctx: &Ctx,
    pending: &PendingStrictAction,
    chat_ref: PeerRef,
    target: PeerRef,
) -> StrictExecution {
    match tokio::time::timeout(
        EXECUTION_TIMEOUT,
        execute_claimed_inner(ctx, pending, chat_ref, target),
    )
    .await
    {
        Ok(execution) => execution,
        Err(_) => {
            defer(
                ctx,
                pending,
                "member serialization or Telegram execution timed out",
            )
            .await;
            StrictExecution::Failed
        }
    }
}

async fn execute_claimed_inner(
    ctx: &Ctx,
    pending: &PendingStrictAction,
    chat_ref: PeerRef,
    target: PeerRef,
) -> StrictExecution {
    let guard = restrict::lock_member(ctx, pending.chat, pending.user).await;
    match ctx.settings.strict_action_is_current(pending).await {
        Ok(true) => {}
        Ok(false) => return StrictExecution::Superseded,
        Err(error) => {
            defer(ctx, pending, &format!("lease revalidation failed: {error}")).await;
            return StrictExecution::Failed;
        }
    }
    if strict_delivery_expired(pending.until_date, unix_now()) {
        return match tokio::time::timeout(
            SETTLEMENT_TIMEOUT,
            ctx.settings.complete_strict_action(pending),
        )
        .await
        {
            Ok(Ok(true)) => StrictExecution::Expired,
            Ok(Ok(false)) => StrictExecution::Superseded,
            Ok(Err(error)) => {
                log::error!(
                    "strict mode: {}/{} expired intent acknowledgement failed: {error}",
                    pending.chat,
                    pending.user
                );
                StrictExecution::Failed
            }
            Err(_) => {
                log::error!(
                    "strict mode: {}/{} expired intent acknowledgement timed out",
                    pending.chat,
                    pending.user
                );
                StrictExecution::Failed
            }
        };
    }
    let action = action_for(pending);
    let result = restrict::apply_strict_locked(
        ctx,
        chat_ref,
        target,
        restrict::StrictApply {
            action,
            duration: pending.duration_seconds.map(Duration::from_secs),
            until_date: pending.until_date,
            wipe_history: pending.wipe_history,
        },
        restrict::By {
            reason: "حالت سختگیرانه",
            target_name: &pending.target_name,
            ..Default::default()
        },
        &guard,
    )
    .await;
    match result {
        Ok(_) => {}
        Err(error) if user_not_participant(&error) => {
            let reason = if pending.action == StrictAction::Ban {
                match super::warns::ban_is_exact(ctx, chat_ref, target, pending.until_date).await {
                    Ok(true) => None,
                    Ok(false) => Some(
                        "Telegram reports a non-participant without the exact intended ban"
                            .to_owned(),
                    ),
                    Err(inspect) => Some(format!(
                        "Telegram reports a non-participant and exact ban inspection failed: {inspect}"
                    )),
                }
            } else {
                Some(
                    "Telegram reports a non-participant; strict mute remains pending for rejoin"
                        .to_owned(),
                )
            };
            if let Some(reason) = reason {
                await_rejoin(ctx, pending, &reason).await;
                return StrictExecution::Failed;
            }
        }
        Err(error) => {
            let reason = error.to_string();
            if restriction_retryable(&error) {
                defer(ctx, pending, &reason).await;
            } else {
                dead_letter(ctx, pending, &reason).await;
            }
            return StrictExecution::Failed;
        }
    }

    match tokio::time::timeout(
        SETTLEMENT_TIMEOUT,
        ctx.settings.complete_strict_action(pending),
    )
    .await
    {
        Ok(Ok(true)) => StrictExecution::Applied,
        Ok(Ok(false)) => {
            log::warn!(
                "strict mode: {}/{} restriction applied after its lease was superseded",
                pending.chat,
                pending.user
            );
            StrictExecution::Applied
        }
        Ok(Err(error)) => {
            log::error!(
                "strict mode: {}/{} restriction applied but acknowledgement failed: {error}",
                pending.chat,
                pending.user
            );
            defer(
                ctx,
                pending,
                "restriction applied but acknowledgement failed",
            )
            .await;
            StrictExecution::Applied
        }
        Err(_) => {
            log::error!(
                "strict mode: {}/{} restriction applied but acknowledgement timed out",
                pending.chat,
                pending.user
            );
            defer(
                ctx,
                pending,
                "restriction applied but acknowledgement timed out",
            )
            .await;
            StrictExecution::Applied
        }
    }
}

fn user_not_participant(error: &restrict::Failed) -> bool {
    matches!(
        error,
        restrict::Failed::Telegram(grammers_client::InvocationError::Rpc(rpc))
            if rpc.name == "USER_NOT_PARTICIPANT"
    )
}

async fn await_rejoin(ctx: &Ctx, pending: &PendingStrictAction, reason: &str) {
    match tokio::time::timeout(
        SETTLEMENT_TIMEOUT,
        ctx.settings.await_strict_rejoin(pending, reason),
    )
    .await
    {
        Ok(Ok(true)) => log::warn!(
            "strict mode: {}/{} quarantined until a new join: {reason}",
            pending.chat,
            pending.user
        ),
        Ok(Ok(false)) => log::warn!(
            "strict mode: {}/{} rejoin quarantine lost its lease",
            pending.chat,
            pending.user
        ),
        Ok(Err(error)) => log::error!(
            "strict mode: {}/{} could not retain rejoin quarantine: {error}",
            pending.chat,
            pending.user
        ),
        Err(_) => log::error!(
            "strict mode: {}/{} rejoin quarantine settlement timed out",
            pending.chat,
            pending.user
        ),
    }
}

fn restriction_retryable(error: &restrict::Failed) -> bool {
    match error {
        restrict::Failed::Protected | restrict::Failed::BasicGroup => false,
        restrict::Failed::State(_) => true,
        restrict::Failed::Telegram(grammers_client::InvocationError::Rpc(rpc)) => !matches!(
            rpc.name.as_str(),
            "USER_ADMIN_INVALID"
                | "USER_CREATOR"
                | "USER_NOT_PARTICIPANT"
                | "PARTICIPANT_ID_INVALID"
                | "USER_ID_INVALID"
                | "INPUT_USER_DEACTIVATED"
                | "CHANNEL_MONOFORUM_UNSUPPORTED"
                | "BANNED_RIGHTS_INVALID"
        ),
        restrict::Failed::Telegram(_) => true,
    }
}

async fn defer(ctx: &Ctx, pending: &PendingStrictAction, reason: &str) {
    let retry_at = unix_now().saturating_add(retry_delay_secs(pending.attempts));
    match tokio::time::timeout(
        SETTLEMENT_TIMEOUT,
        ctx.settings.defer_strict_action(pending, retry_at, reason),
    )
    .await
    {
        Ok(Ok(true)) => log::warn!(
            "strict mode: {}/{} attempt {} deferred: {reason}",
            pending.chat,
            pending.user,
            pending.attempts
        ),
        Ok(Ok(false)) => log::warn!(
            "strict mode: {}/{} retry lost its lease",
            pending.chat,
            pending.user
        ),
        Ok(Err(error)) => log::error!(
            "strict mode: {}/{} could not persist retry after {reason}: {error}",
            pending.chat,
            pending.user
        ),
        Err(_) => log::error!(
            "strict mode: {}/{} retry settlement timed out after {reason}",
            pending.chat,
            pending.user
        ),
    }
}

async fn dead_letter(ctx: &Ctx, pending: &PendingStrictAction, reason: &str) {
    match tokio::time::timeout(
        SETTLEMENT_TIMEOUT,
        ctx.settings
            .dead_letter_strict_action(pending, unix_now(), reason),
    )
    .await
    {
        Ok(Ok(true)) => log::error!(
            "strict mode: {}/{} terminal action failure: {reason}",
            pending.chat,
            pending.user
        ),
        Ok(Ok(false)) => log::warn!(
            "strict mode: {}/{} terminal settlement lost its lease",
            pending.chat,
            pending.user
        ),
        Ok(Err(error)) => log::error!(
            "strict mode: {}/{} could not persist terminal failure after {reason}: {error}",
            pending.chat,
            pending.user
        ),
        Err(_) => log::error!(
            "strict mode: {}/{} terminal settlement timed out after {reason}",
            pending.chat,
            pending.user
        ),
    }
}

pub async fn recover_pending(ctx: &Arc<Ctx>) {
    let mut handled = 0_usize;
    while handled < RECOVERY_LIMIT {
        let capacity = RECOVERY_PAGE.min((RECOVERY_LIMIT - handled) as i64);
        let now = unix_now();
        let lease_until = now.saturating_add(ACTION_LEASE_SECS);
        let mut pending = Vec::with_capacity(capacity as usize);
        for (class, share) in [
            (StrictQueueClass::Fresh, FRESH_RECOVERY_SHARE),
            (StrictQueueClass::Retry, RETRY_RECOVERY_SHARE),
        ] {
            let ask = share.min(capacity - pending.len() as i64);
            if ask == 0 {
                continue;
            }
            match ctx
                .settings
                .claim_pending_strict_actions(class, now, lease_until, ask)
                .await
            {
                Ok(mut claimed) => pending.append(&mut claimed),
                Err(error) => log::warn!("strict mode: {class:?} action claim failed: {error}"),
            }
        }
        for class in [StrictQueueClass::Fresh, StrictQueueClass::Retry] {
            let ask = capacity - pending.len() as i64;
            if ask == 0 {
                break;
            }
            match ctx
                .settings
                .claim_pending_strict_actions(class, now, lease_until, ask)
                .await
            {
                Ok(mut claimed) => pending.append(&mut claimed),
                Err(error) => log::warn!("strict mode: {class:?} fill claim failed: {error}"),
            }
        }
        let count = pending.len();
        if count == 0 {
            break;
        }
        handled += count;
        let owner = Arc::clone(ctx);
        super::bounded(pending, super::FLEET_CAMPAIGNS, move |pending| {
            let ctx = Arc::clone(&owner);
            async move { recover_one(&ctx, pending).await }
        })
        .await;
        if count < capacity as usize {
            break;
        }
    }

    let before = unix_now().saturating_sub(DEAD_LETTER_RETENTION_SECS);
    match ctx
        .settings
        .purge_strict_dead_letters(before, DEAD_LETTER_PURGE_PAGE)
        .await
    {
        Ok(0) => {}
        Ok(count) => log::warn!("strict mode: purged {count} expired terminal actions"),
        Err(error) => log::warn!("strict mode: terminal-action purge failed: {error}"),
    }
}

async fn recover_one(ctx: &Ctx, pending: PendingStrictAction) {
    match (
        ctx.chat_ref(pending.chat),
        PeerId::user(pending.user).map(PeerId::to_ambient_ref),
    ) {
        (None, _) => defer(ctx, &pending, "chat reference is unavailable").await,
        (_, None) => dead_letter(ctx, &pending, "invalid durable user id").await,
        (Some(chat_ref), Some(target)) => {
            execute_claimed(ctx, &pending, chat_ref, target).await;
        }
    }
}

fn retry_delay_secs(attempts: u32) -> i64 {
    let exponent = attempts.saturating_sub(1).min(6);
    (5_i64 * (1_i64 << exponent)).min(300)
}

const TELEGRAM_MIN_TEMPORARY_SECS: i64 = 30;

fn strict_delivery_expired(until_date: i32, now: i64) -> bool {
    until_date > 0 && i64::from(until_date) <= now.saturating_add(TELEGRAM_MIN_TEMPORARY_SECS)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

pub fn chances_line(chances: u32) -> String {
    format!("<i>{chances} فرصت دیگر دارید.</i>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_client::{InvocationError, sender::RpcError};

    fn rpc(code: i32, name: &str) -> restrict::Failed {
        restrict::Failed::Telegram(InvocationError::Rpc(RpcError {
            code,
            name: name.to_owned(),
            value: None,
            caused_by: None,
        }))
    }

    #[test]
    fn only_one_outcome_stays_quiet() {
        let quiet = |outcome: &Outcome| matches!(outcome, Outcome::Announced);
        assert!(!quiet(&Outcome::Nothing));
        assert!(!quiet(&Outcome::Chances(2)));
        assert!(quiet(&Outcome::Announced));
    }

    #[test]
    fn retry_policy_distinguishes_transient_and_terminal_failures() {
        assert!(!restriction_retryable(&restrict::Failed::Protected));
        assert!(!restriction_retryable(&restrict::Failed::BasicGroup));
        assert!(restriction_retryable(&rpc(500, "INTERNAL")));
        assert!(restriction_retryable(&rpc(400, "FLOOD_WAIT")));
        assert!(restriction_retryable(&rpc(400, "CHAT_ADMIN_REQUIRED")));
        assert!(restriction_retryable(&rpc(400, "NEW_TRANSIENT_FAILURE")));
        assert!(!restriction_retryable(&rpc(400, "USER_ID_INVALID")));
        assert!(!restriction_retryable(&rpc(400, "USER_NOT_PARTICIPANT")));
        assert!(user_not_participant(&rpc(400, "USER_NOT_PARTICIPANT")));
    }

    #[test]
    fn retry_backoff_is_bounded() {
        assert_eq!(retry_delay_secs(1), 5);
        assert_eq!(retry_delay_secs(2), 10);
        assert_eq!(retry_delay_secs(7), 300);
        assert_eq!(retry_delay_secs(u32::MAX), 300);
    }

    #[test]
    fn scheduler_bounds_fit_inside_the_lease() {
        let lease_secs = u64::try_from(ACTION_LEASE_SECS).expect("positive lease duration");
        assert!(EXECUTION_TIMEOUT < Duration::from_secs(lease_secs));
        assert_eq!(RECOVERY_PAGE, super::super::FLEET_CAMPAIGNS as i64);
        assert_eq!(FRESH_RECOVERY_SHARE + RETRY_RECOVERY_SHARE, RECOVERY_PAGE);
        assert!(RECOVERY_LIMIT >= RECOVERY_PAGE as usize);
    }

    #[test]
    fn finite_retry_never_extends_or_turns_permanent_near_expiry() {
        assert!(!strict_delivery_expired(1_060, 1_000));
        assert!(strict_delivery_expired(1_030, 1_000));
        assert!(strict_delivery_expired(999, 1_000));
        assert!(!strict_delivery_expired(0, i64::MAX));
    }
}
