use std::sync::Arc;
use std::time::Duration;

use grammers_client::message::Message;
use grammers_client::session::types::{PeerId, PeerRef};

use super::restrict::{self, Action};
use super::{Ctx, esc, name_of};
use crate::state::{PendingWarningAction, WarningPenalty, WarningQueueClass};

pub const LIMIT: &str = "warn_limit";

pub const ACTION: &str = "warn_action";
const DEFAULT_LIMIT: u32 = 3;
const ACTION_LEASE_SECS: i64 = 180;
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(120);
const SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(10);
const RECOVERY_PAGE: i64 = super::FLEET_CAMPAIGNS as i64;
const RECOVERY_LIMIT: usize = 100;
const FRESH_RECOVERY_SHARE: i64 = 3;
const RETRY_RECOVERY_SHARE: i64 = 1;
pub const LIMIT_RANGE: (u32, u32) = (1, 100);
pub const LIMIT_PRESETS: &[u32] = &[2, 3, 5, 7, 10];

pub const WARN: &[&str] = &["اخطار", "وارن"];
pub const UNWARN: &[&str] = &["حذف اخطار", "پاک اخطار", "رفع اخطار"];
pub const SHOW: &[&str] = &["اخطارها", "لیست اخطار"];

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value_parsed(chat, LIMIT)
        .unwrap_or(DEFAULT_LIMIT)
}

pub fn bans(ctx: &Ctx, chat: i64) -> bool {
    ctx.settings.value(chat, ACTION).as_deref() != Some("mute")
}

pub async fn set_limit(
    ctx: &Ctx,
    chat: i64,
    value: u32,
) -> Result<(), crate::state::SettingsWriteError> {
    let value = value.clamp(LIMIT_RANGE.0, LIMIT_RANGE.1);
    ctx.settings
        .try_set_value(chat, LIMIT, &value.to_string())
        .await
        .map(|_| ())
}

pub async fn count(ctx: &Ctx, chat: i64, user: i64) -> Result<u32, sqlx::Error> {
    ctx.settings.warns_of(chat, user).await
}

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let text = view.digits();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    let matched = WARN
        .iter()
        .map(|c| (c, Some(true)))
        .chain(UNWARN.iter().map(|c| (c, Some(false))))
        .chain(SHOW.iter().map(|c| (c, None)))
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
    if !super::limits::allows(ctx, message, super::limits::WARN).await {
        return true;
    }

    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.",
            ),
        )
        .await;
        return true;
    };
    let Some(user) = target.id.bare_id() else {
        return true;
    };

    let limit = limit(ctx, chat);

    match adding {
        None => {
            let current = match count(ctx, chat, user).await {
                Ok(current) => current,
                Err(error) => {
                    eprintln!("warns: {chat}/{user}: could not read count: {error}");
                    super::respond(
                        ctx,
                        message,
                        crate::response::ResponseKind::CommandError,
                        super::premium::icon_text(
                            Some(super::premium::Icon::ErrorRed),
                            "شمارش اخطارها فعلا در دسترس نیست؛ دوباره تلاش کنید.",
                        ),
                    )
                    .await;
                    return true;
                }
            };
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::WarningLookup,
                super::premium::icon_html(
                    Some(super::premium::Icon::Warning),
                    format!(
                        "<b>اخطارها</b>\n\n{} · <b>{current}</b> از <b>{limit}</b>",
                        esc(&name)
                    ),
                ),
            )
            .await;
        }
        Some(false) => {
            let restriction_guard = restrict::lock_member(ctx, chat, user).await;
            let next = match ctx.settings.decrement_warning(chat, user).await {
                Ok(next) => next,
                Err(error) => {
                    drop(restriction_guard);
                    eprintln!("warns: {chat}/{user}: could not remove warning: {error}");
                    super::respond(
                        ctx,
                        message,
                        crate::response::ResponseKind::CommandError,
                        super::premium::icon_text(
                            Some(super::premium::Icon::ErrorRed),
                            "اخطار ذخیره نشد؛ دوباره تلاش کنید.",
                        ),
                    )
                    .await;
                    return true;
                }
            };
            drop(restriction_guard);
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::ModerationConfirmation,
                super::premium::icon_html(
                    Some(super::premium::Icon::Warning),
                    format!(
                        "✗ یک اخطار از {} کم شد · <b>{next}</b> از <b>{limit}</b>",
                        esc(&name)
                    ),
                ),
            )
            .await;
        }
        Some(true) => {
            let requested_penalty = if bans(ctx, chat) {
                WarningPenalty::Ban
            } else {
                WarningPenalty::Mute
            };
            let increment = match ctx
                .settings
                .increment_warning(chat, user, limit, requested_penalty)
                .await
            {
                Ok(increment) => increment,
                Err(error) => {
                    eprintln!("warns: {chat}/{user}: could not store warning: {error}");
                    super::respond(
                        ctx,
                        message,
                        crate::response::ResponseKind::CommandError,
                        super::premium::icon_text(
                            Some(super::premium::Icon::ErrorRed),
                            "اخطار ذخیره نشد؛ دوباره تلاش کنید.",
                        ),
                    )
                    .await;
                    return true;
                }
            };
            let next = increment.count;
            if increment.added {
                ctx.bump(chat, super::stats::WARNED);
                let by = super::sender_of(message);
                super::log::write(
                    ctx,
                    chat,
                    "log_warn",
                    super::log::Entry {
                        title: "اخطار",
                        target: Some((user, &name)),
                        actor: by.as_ref().map(|(id, name)| (*id, name.as_str())),
                        extra: vec![("شماره", format!("{next} از {limit}"))],
                        ..Default::default()
                    },
                )
                .await;
            }
            if increment.pending.is_some() {
                let now = unix_now();
                let claimed = match ctx
                    .settings
                    .claim_warning_action(chat, user, now, now.saturating_add(ACTION_LEASE_SECS))
                    .await
                {
                    Ok(claimed) => claimed,
                    Err(error) => {
                        eprintln!("warns: {chat}/{user}: could not claim punishment: {error}");
                        None
                    }
                };
                if let Some(claimed) = claimed {
                    punish(ctx, message, target, &name, limit, &claimed).await;
                }
            } else {
                super::cases::record_warning(ctx, message, user, &name, next).await;
                super::announce(
                    ctx,
                    message,
                    crate::response::ResponseKind::WarningNotice,
                    super::premium::icon_html(
                        Some(super::premium::Icon::Warning),
                        format!(
                            "<b>اخطار</b>\n\n{} · <b>{next}</b> از <b>{limit}</b>\nتوسط · {}",
                            esc(&name),
                            esc(&name_of(message))
                        ),
                    ),
                )
                .await;
            }
        }
    }
    true
}

async fn punish(
    ctx: &Ctx,
    message: &Message,
    target: PeerRef,
    name: &str,
    limit: u32,
    pending: &PendingWarningAction,
) {
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        settle(
            ctx,
            pending,
            DeliveryOutcome::Retryable("chat reference is unavailable".to_owned()),
        )
        .await;
        return;
    };
    let evidence = super::cases::reply_evidence(message, "warning threshold action").await;
    let actor = super::sender_of(message);
    let case = super::cases::CaseContext {
        source: "moderator",
        rule: "manual_warn",
        reason: "سقف اخطار",
        evidence,
    };
    let execution = execute_claimed(
        ctx,
        pending,
        chat_ref,
        target,
        name,
        actor.as_ref().map(|(id, name)| (*id, name.as_str())),
        &case,
    )
    .await;
    let action = match pending.penalty {
        WarningPenalty::Ban => Action::Ban,
        WarningPenalty::Mute => Action::Mute,
    };
    let what = if action == Action::Ban {
        "از گروه اخراج شد"
    } else {
        "سکوت شد"
    };

    let (outcome_icon, reply) = match execution {
        WarningExecution::Applied { wiped } => (
            action.icon(),
            format!(
                "<b>اخطار</b>\n\n{} به <b>{limit}</b> اخطار رسید و {what}.{}",
                esc(name),
                match wiped {
                    0 => String::new(),
                    n => format!("\n\n🧹 {n} پیام او هم پاک شد."),
                }
            ),
        ),
        WarningExecution::Failed { told } => (Some(super::premium::Icon::ErrorRed), esc(&told)),
        WarningExecution::Superseded => return,
    };
    super::announce(
        ctx,
        message,
        crate::response::ResponseKind::ModerationAnnouncement,
        super::premium::icon_html(outcome_icon, reply),
    )
    .await;
}

pub async fn recover_pending(ctx: &Arc<Ctx>) {
    let mut handled = 0_usize;
    while handled < RECOVERY_LIMIT {
        let capacity = RECOVERY_PAGE.min((RECOVERY_LIMIT - handled) as i64);
        let now = unix_now();
        let lease_until = now.saturating_add(ACTION_LEASE_SECS);
        let mut pending = Vec::with_capacity(capacity as usize);
        for (class, share) in [
            (WarningQueueClass::Fresh, FRESH_RECOVERY_SHARE),
            (WarningQueueClass::Retry, RETRY_RECOVERY_SHARE),
        ] {
            let ask = share.min(capacity - pending.len() as i64);
            if ask == 0 {
                continue;
            }
            match ctx
                .settings
                .claim_pending_warning_actions(class, now, lease_until, ask)
                .await
            {
                Ok(mut claimed) => pending.append(&mut claimed),
                Err(error) => {
                    log::warn!("warns: {class:?} pending-action claim failed: {error}")
                }
            }
        }

        for class in [WarningQueueClass::Fresh, WarningQueueClass::Retry] {
            let ask = capacity - pending.len() as i64;
            if ask == 0 {
                break;
            }
            match ctx
                .settings
                .claim_pending_warning_actions(class, now, lease_until, ask)
                .await
            {
                Ok(mut claimed) => pending.append(&mut claimed),
                Err(error) => log::warn!("warns: {class:?} fill claim failed: {error}"),
            }
        }

        let count = pending.len();
        if count == 0 {
            return;
        }
        handled += count;
        let owner = Arc::clone(ctx);
        super::bounded(pending, super::FLEET_CAMPAIGNS, move |pending| {
            let ctx = Arc::clone(&owner);
            async move { recover_one(&ctx, pending).await }
        })
        .await;
        if count < capacity as usize {
            return;
        }
    }
}

async fn recover_one(ctx: &Ctx, pending: PendingWarningAction) {
    match (
        ctx.chat_ref(pending.chat),
        PeerId::user(pending.user).map(PeerId::to_ambient_ref),
    ) {
        (None, _) => {
            settle(
                ctx,
                &pending,
                DeliveryOutcome::Retryable("chat reference is unavailable".to_owned()),
            )
            .await;
        }
        (_, None) => {
            settle(
                ctx,
                &pending,
                DeliveryOutcome::Permanent("invalid durable user id".to_owned()),
            )
            .await;
        }
        (Some(chat_ref), Some(target)) => {
            let case = super::cases::CaseContext {
                source: "recovery",
                rule: "manual_warn",
                reason: "سقف اخطار",
                evidence: None,
            };
            let _ = execute_claimed(ctx, &pending, chat_ref, target, "", None, &case).await;
        }
    }
}

enum WarningExecution {
    Applied { wiped: usize },
    Failed { told: String },
    Superseded,
}

async fn execute_claimed(
    ctx: &Ctx,
    pending: &PendingWarningAction,
    chat_ref: PeerRef,
    target: PeerRef,
    target_name: &str,
    actor: Option<(i64, &str)>,
    case: &super::cases::CaseContext,
) -> WarningExecution {
    match tokio::time::timeout(
        EXECUTION_TIMEOUT,
        execute_claimed_inner(ctx, pending, chat_ref, target, target_name, actor, case),
    )
    .await
    {
        Ok(execution) => execution,
        Err(_) => {
            settle(
                ctx,
                pending,
                DeliveryOutcome::Retryable(
                    "member serialization or Telegram execution timed out".to_owned(),
                ),
            )
            .await;
            WarningExecution::Failed {
                told: "اعمال اخطار بیش از حد طول کشید و دوباره بررسی می شود.".to_owned(),
            }
        }
    }
}

async fn execute_claimed_inner(
    ctx: &Ctx,
    pending: &PendingWarningAction,
    chat_ref: PeerRef,
    target: PeerRef,
    target_name: &str,
    actor: Option<(i64, &str)>,
    case: &super::cases::CaseContext,
) -> WarningExecution {
    let guard = restrict::lock_member(ctx, pending.chat, pending.user).await;
    match ctx.settings.warning_action_is_current(pending).await {
        Ok(true) => {}
        Ok(false) => return WarningExecution::Superseded,
        Err(error) => {
            settle(
                ctx,
                pending,
                DeliveryOutcome::Retryable(format!("lease revalidation failed: {error}")),
            )
            .await;
            return WarningExecution::Failed {
                told: "وضعیت اخطار بررسی نشد؛ دوباره تلاش کنید.".to_owned(),
            };
        }
    }
    let action = match pending.penalty {
        WarningPenalty::Ban => Action::Ban,
        WarningPenalty::Mute => Action::Mute,
    };
    let outcome = match restrict::apply_locked(
        ctx,
        chat_ref,
        target,
        action,
        None,
        restrict::By {
            reason: "سقف اخطار",
            target_name,
            ..Default::default()
        },
        &guard,
    )
    .await
    {
        Ok(wiped) => {
            let workflow = pending.workflow_key();
            let audit = super::cases::record_workflow_restriction(
                ctx,
                super::cases::WorkflowRestrictionRecord {
                    key: &workflow,
                    restriction: super::cases::RestrictionRecord {
                        chat: pending.chat,
                        target: pending.user,
                        target_name,
                        actor,
                        action,
                        duration: None,
                        case,
                        result: Ok(()),
                    },
                },
            )
            .await;
            match audit {
                Ok(_) => {
                    settle(ctx, pending, DeliveryOutcome::Applied).await;
                    return WarningExecution::Applied { wiped };
                }
                Err(error) => DeliveryOutcome::Retryable(format!(
                    "restriction applied but workflow audit failed: {error}"
                )),
            }
        }
        Err(error) if user_not_participant(&error) => {
            if pending.penalty == WarningPenalty::Ban {
                match ban_is_exact(ctx, chat_ref, target, 0).await {
                    Ok(true) => {
                        let workflow = pending.workflow_key();
                        match super::cases::record_workflow_restriction(
                            ctx,
                            super::cases::WorkflowRestrictionRecord {
                                key: &workflow,
                                restriction: super::cases::RestrictionRecord {
                                    chat: pending.chat,
                                    target: pending.user,
                                    target_name,
                                    actor,
                                    action,
                                    duration: None,
                                    case,
                                    result: Ok(()),
                                },
                            },
                        )
                        .await
                        {
                            Ok(_) => {
                                settle(ctx, pending, DeliveryOutcome::Applied).await;
                                return WarningExecution::Applied { wiped: 0 };
                            }
                            Err(audit) => DeliveryOutcome::Retryable(format!(
                                "permanent ban confirmed but workflow audit failed: {audit}"
                            )),
                        }
                    }
                    Ok(false) => DeliveryOutcome::AwaitingRejoin(
                        "Telegram reports a non-participant without an exact permanent ban"
                            .to_owned(),
                    ),
                    Err(inspect) => DeliveryOutcome::AwaitingRejoin(format!(
                        "Telegram reports a non-participant and ban inspection failed: {inspect}"
                    )),
                }
            } else {
                DeliveryOutcome::AwaitingRejoin(
                    "Telegram reports a non-participant; mute remains pending for rejoin"
                        .to_owned(),
                )
            }
        }
        Err(error) => {
            eprintln!("warns: {}: could not punish: {error}", pending.chat);
            let told = error.told();
            let outcome = classify_failure(error);
            settle(ctx, pending, outcome).await;
            return WarningExecution::Failed { told };
        }
    };
    settle(ctx, pending, outcome).await;
    WarningExecution::Failed {
        told: "اخطار اعمال شد اما ثبت سابقه کامل نشد؛ دوباره بررسی می شود.".to_owned(),
    }
}

enum DeliveryOutcome {
    Applied,
    Retryable(String),
    AwaitingRejoin(String),
    Permanent(String),
}

fn user_not_participant(error: &restrict::Failed) -> bool {
    matches!(
        error,
        restrict::Failed::Telegram(grammers_client::InvocationError::Rpc(rpc))
            if rpc.name == "USER_NOT_PARTICIPANT"
    )
}

pub(super) async fn ban_is_exact(
    ctx: &Ctx,
    chat: PeerRef,
    target: PeerRef,
    until_date: i32,
) -> Result<bool, grammers_client::InvocationError> {
    use grammers_client::tl;

    let participant = ctx
        .client
        .invoke_outbound(&tl::functions::channels::GetParticipant {
            channel: chat.into(),
            participant: target.into(),
        })
        .await?;
    let tl::enums::channels::ChannelParticipant::Participant(participant) = participant;
    let tl::enums::ChannelParticipant::Banned(banned) = participant.participant else {
        return Ok(false);
    };
    let tl::enums::ChatBannedRights::Rights(rights) = banned.banned_rights;
    Ok(rights.view_messages && rights.until_date == until_date)
}

fn classify_failure(error: restrict::Failed) -> DeliveryOutcome {
    if user_not_participant(&error) {
        return DeliveryOutcome::AwaitingRejoin(error.to_string());
    }
    let retryable = match &error {
        restrict::Failed::Protected | restrict::Failed::BasicGroup => false,
        restrict::Failed::State(_) => true,
        restrict::Failed::Telegram(grammers_client::InvocationError::Rpc(rpc)) => {
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
        restrict::Failed::Telegram(_) => true,
    };
    if retryable {
        DeliveryOutcome::Retryable(error.to_string())
    } else {
        DeliveryOutcome::Permanent(error.to_string())
    }
}

async fn settle(ctx: &Ctx, pending: &PendingWarningAction, outcome: DeliveryOutcome) {
    if tokio::time::timeout(SETTLEMENT_TIMEOUT, settle_inner(ctx, pending, outcome))
        .await
        .is_err()
    {
        log::error!(
            "warns: {}/{} durable settlement exceeded its bounded deadline",
            pending.chat,
            pending.user
        );
    }
}

async fn settle_inner(ctx: &Ctx, pending: &PendingWarningAction, outcome: DeliveryOutcome) {
    match outcome {
        DeliveryOutcome::Applied => match ctx.settings.complete_warning_action(pending).await {
            Ok(true) => {}
            Ok(false) => log::warn!(
                "warns: {}/{} completion lost its lease",
                pending.chat,
                pending.user
            ),
            Err(error) => log::warn!(
                "warns: recovered {}/{} but acknowledgement failed: {error}",
                pending.chat,
                pending.user
            ),
        },
        DeliveryOutcome::Retryable(reason) => {
            let retry_at = unix_now().saturating_add(retry_delay_secs(pending.attempts));
            match ctx
                .settings
                .defer_warning_action(pending, retry_at, &reason)
                .await
            {
                Ok(true) => log::warn!(
                    "warns: {}/{} attempt {} deferred: {reason}",
                    pending.chat,
                    pending.user,
                    pending.attempts
                ),
                Ok(false) => log::warn!(
                    "warns: {}/{} retry lost its lease",
                    pending.chat,
                    pending.user
                ),
                Err(error) => log::warn!(
                    "warns: {}/{} could not persist retry after {reason}: {error}",
                    pending.chat,
                    pending.user
                ),
            }
        }
        DeliveryOutcome::AwaitingRejoin(reason) => {
            match ctx.settings.await_warning_rejoin(pending, &reason).await {
                Ok(true) => log::warn!(
                    "warns: {}/{} quarantined until a new join: {reason}",
                    pending.chat,
                    pending.user
                ),
                Ok(false) => log::warn!(
                    "warns: {}/{} rejoin quarantine lost its lease",
                    pending.chat,
                    pending.user
                ),
                Err(error) => log::error!(
                    "warns: {}/{} could not retain rejoin quarantine: {error}",
                    pending.chat,
                    pending.user
                ),
            }
        }
        DeliveryOutcome::Permanent(reason) => {
            log::error!(
                "warns: {}/{} terminal action failure: {reason}",
                pending.chat,
                pending.user
            );
            match ctx.settings.terminate_warning_action(pending).await {
                Ok(true) => {}
                Ok(false) => log::warn!(
                    "warns: {}/{} terminal acknowledgement lost its lease",
                    pending.chat,
                    pending.user
                ),
                Err(error) => log::warn!(
                    "warns: {}/{} could not acknowledge terminal failure: {error}",
                    pending.chat,
                    pending.user
                ),
            }
        }
    }
}

fn retry_delay_secs(attempts: u32) -> i64 {
    (5_i64 * (1_i64 << attempts.min(6))).min(300)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
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
    fn warning_retry_policy_distinguishes_repairable_and_terminal_failures() {
        assert!(matches!(
            classify_failure(rpc(500, "INTERNAL")),
            DeliveryOutcome::Retryable(_)
        ));
        assert!(matches!(
            classify_failure(rpc(400, "CHAT_ADMIN_REQUIRED")),
            DeliveryOutcome::Retryable(_)
        ));
        assert!(matches!(
            classify_failure(rpc(400, "USER_NOT_PARTICIPANT")),
            DeliveryOutcome::AwaitingRejoin(_)
        ));
        assert!(matches!(
            classify_failure(restrict::Failed::Protected),
            DeliveryOutcome::Permanent(_)
        ));
    }

    #[test]
    fn warning_retries_back_off_with_a_finite_ceiling() {
        assert_eq!(retry_delay_secs(0), 5);
        assert_eq!(retry_delay_secs(1), 10);
        assert_eq!(retry_delay_secs(6), 300);
        assert_eq!(retry_delay_secs(u32::MAX), 300);
    }

    #[test]
    fn recovery_reserves_capacity_for_fresh_and_retry_work() {
        assert_eq!(FRESH_RECOVERY_SHARE + RETRY_RECOVERY_SHARE, RECOVERY_PAGE);
        const { assert!(FRESH_RECOVERY_SHARE > 0) };
        const { assert!(RETRY_RECOVERY_SHARE > 0) };
    }

    #[test]
    fn warning_execution_and_settlement_fit_inside_lease() {
        let total = EXECUTION_TIMEOUT + SETTLEMENT_TIMEOUT;
        assert!(total < Duration::from_secs(ACTION_LEASE_SECS as u64));
    }
}
