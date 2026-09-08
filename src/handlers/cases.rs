use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use grammers_client::media::Media;
use grammers_client::message::Message;
use grammers_client::session::types::PeerId;
use grammers_client::update::CallbackQuery;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::state::{
    ModerationCase, ModerationCaseDetail, ModerationCaseTransition, NewModerationCase,
    NewModerationCaseEvent, WarningCaseReversal, WarningCaseReversalResult,
};

use super::restrict::{self, Action};
use super::{Ctx, esc, name_of};

pub const LIST: &[&str] = &["پرونده ها", "پرونده‌ها", "لیست پرونده"];
pub const SHOW: &str = "پرونده";
pub const HISTORY: &str = "تاریخچه";
pub const CLOSE: &str = "بستن پرونده";
pub const REVERSE: &str = "لغو پرونده";
#[cfg(test)]
pub const COMMANDS: &[&str] = &[
    "پرونده",
    "پرونده ها",
    "پرونده‌ها",
    "لیست پرونده",
    "تاریخچه",
    "بستن پرونده",
    "لغو پرونده",
];
pub const NOTE_MAX: usize = 500;
pub const WRITTEN: &str = "moderation_case_written";
pub const WRITE_FAILED: &str = "moderation_case_write_failed";
const EVIDENCE_MAX: usize = 512;

#[derive(Clone, Debug)]
pub struct Evidence {
    pub message: Option<i32>,
    pub media_kind: Option<String>,
    pub text: Option<String>,
    pub hash: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct CaseContext {
    pub source: &'static str,
    pub rule: &'static str,
    pub reason: &'static str,
    pub evidence: Option<Evidence>,
}

pub fn evidence(message: &Message) -> Evidence {
    let raw = message.text().trim();
    let text = (!raw.is_empty()).then(|| raw.chars().take(EVIDENCE_MAX).collect());
    let hash = (!raw.is_empty()).then(|| {
        let normalized: String = raw.nfkc().flat_map(char::to_lowercase).collect();
        Sha256::digest(normalized.as_bytes()).to_vec()
    });
    Evidence {
        message: Some(message.id()),
        media_kind: media_kind(message.media().as_ref()).map(str::to_owned),
        text,
        hash,
    }
}

pub async fn reply_evidence(message: &Message, purpose: &str) -> Option<Evidence> {
    match message.get_reply().await {
        Ok(reply) => reply.map(|reply| evidence(&reply)),
        Err(error) => {
            log::warn!("cases: could not fetch replied-message evidence for {purpose}: {error}");
            None
        }
    }
}

fn media_kind(media: Option<&Media>) -> Option<&'static str> {
    match media {
        None => None,
        Some(Media::Photo(_)) => Some("photo"),
        Some(Media::Sticker(_)) => Some("sticker"),
        Some(Media::Document(document)) if document.is_animated() => Some("gif"),
        Some(Media::Document(document)) => match document.mime_type().unwrap_or_default() {
            mime if mime.starts_with("video/") => Some("video"),
            mime if mime.starts_with("audio/ogg") => Some("voice"),
            mime if mime.starts_with("audio/") => Some("audio"),
            _ => Some("document"),
        },
        Some(_) => Some("other"),
    }
}

pub fn context(
    message: &Message,
    source: &'static str,
    rule: &'static str,
    reason: &'static str,
) -> CaseContext {
    CaseContext {
        source,
        rule,
        reason,
        evidence: Some(evidence(message)),
    }
}

pub struct RestrictionRecord<'a> {
    pub chat: i64,
    pub target: i64,
    pub target_name: &'a str,
    pub actor: Option<(i64, &'a str)>,
    pub action: Action,
    pub duration: Option<Duration>,
    pub case: &'a CaseContext,
    pub result: Result<(), &'a restrict::Failed>,
}

pub struct WorkflowRestrictionRecord<'a> {
    pub key: &'a str,
    pub restriction: RestrictionRecord<'a>,
}

pub async fn record_restriction(ctx: &Ctx, record: RestrictionRecord<'_>) -> Option<i64> {
    let (_, draft) = restriction_draft(record);
    create(ctx, draft).await
}

pub async fn record_workflow_restriction(
    ctx: &Ctx,
    record: WorkflowRestrictionRecord<'_>,
) -> Result<i64, sqlx::Error> {
    let (chat, draft) = restriction_draft(record.restriction);
    match ctx
        .settings
        .create_workflow_moderation_case(record.key, draft)
        .await
    {
        Ok(created) => {
            if created.inserted {
                ctx.bump(chat, WRITTEN);
            }
            Ok(created.id)
        }
        Err(error) => {
            ctx.bump(chat, WRITE_FAILED);
            Err(error)
        }
    }
}

fn restriction_draft(record: RestrictionRecord<'_>) -> (i64, NewModerationCase) {
    let RestrictionRecord {
        chat,
        target,
        target_name,
        actor,
        action,
        duration,
        case,
        result,
    } = record;
    let action_name = action_key(action);
    let success = result.is_ok();
    let until = duration.map(|duration| {
        now().saturating_add(i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
    });
    let evidence = case.evidence.clone().unwrap_or(Evidence {
        message: None,
        media_kind: None,
        text: None,
        hash: None,
    });
    (
        chat,
        NewModerationCase {
            chat,
            subject: Some(target),
            subject_name: target_name.to_owned(),
            source: case.source.to_owned(),
            rule: case.rule.to_owned(),
            reason: case.reason.to_owned(),
            message: evidence.message,
            media_kind: evidence.media_kind,
            evidence: evidence.text,
            evidence_hash: evidence.hash,
            action: action_name.to_owned(),
            action_until: until,
            status: if success { "resolved" } else { "open" }.to_owned(),
            actor: actor.map(|value| value.0),
            actor_name: actor.map_or_else(String::new, |value| value.1.to_owned()),
            event_kind: if success {
                "action_succeeded"
            } else {
                "action_failed"
            }
            .to_owned(),
            event_note: result.err().map(ToString::to_string),
        },
    )
}

pub async fn record_delete(
    ctx: &Ctx,
    message: &Message,
    rule: &str,
    reason: &str,
    action: &str,
) -> Option<i64> {
    let chat = message.peer_id().bot_api_dialog_id()?;
    let proof = evidence(message);
    let id = create(
        ctx,
        NewModerationCase {
            chat,
            subject: message.sender_id().and_then(PeerId::bare_id),
            subject_name: name_of(message),
            source: "automatic".to_owned(),
            rule: rule.to_owned(),
            reason: reason.to_owned(),
            message: proof.message,
            media_kind: proof.media_kind,
            evidence: proof.text,
            evidence_hash: proof.hash,
            action: action.to_owned(),
            action_until: None,
            status: "resolved".to_owned(),
            actor: None,
            actor_name: String::new(),
            event_kind: "action_succeeded".to_owned(),
            event_note: None,
        },
    )
    .await?;
    if action != "delete" {
        let _ = ctx
            .settings
            .append_moderation_case_event(NewModerationCaseEvent {
                chat,
                case_id: id,
                kind: "action_succeeded",
                actor: None,
                action: Some("delete"),
                note: None,
            })
            .await;
    }
    Some(id)
}

pub struct DetachedRecord<'a> {
    pub chat: i64,
    pub subject: Option<i64>,
    pub subject_name: &'a str,
    pub message: i32,
    pub rule: &'a str,
    pub reason: &'a str,
    pub action: &'a str,
}

pub async fn record_detached(ctx: &Ctx, record: DetachedRecord<'_>) -> Option<i64> {
    let DetachedRecord {
        chat,
        subject,
        subject_name,
        message,
        rule,
        reason,
        action,
    } = record;
    let id = create(
        ctx,
        NewModerationCase {
            chat,
            subject,
            subject_name: subject_name.to_owned(),
            source: "automatic".to_owned(),
            rule: rule.to_owned(),
            reason: reason.to_owned(),
            message: Some(message),
            media_kind: Some("media".to_owned()),
            evidence: None,
            evidence_hash: None,
            action: action.to_owned(),
            action_until: None,
            status: "resolved".to_owned(),
            actor: None,
            actor_name: String::new(),
            event_kind: "action_succeeded".to_owned(),
            event_note: None,
        },
    )
    .await?;
    if action != "delete" {
        let _ = ctx
            .settings
            .append_moderation_case_event(NewModerationCaseEvent {
                chat,
                case_id: id,
                kind: "action_succeeded",
                actor: None,
                action: Some("delete"),
                note: None,
            })
            .await;
    }
    Some(id)
}

pub async fn record_warning(
    ctx: &Ctx,
    message: &Message,
    target: i64,
    target_name: &str,
    count: u32,
) -> Option<i64> {
    let chat = message.peer_id().bot_api_dialog_id()?;
    let actor = super::sender_of(message);
    let proof = reply_evidence(message, "manual warning").await;
    let proof = proof.unwrap_or(Evidence {
        message: None,
        media_kind: None,
        text: None,
        hash: None,
    });
    create(
        ctx,
        NewModerationCase {
            chat,
            subject: Some(target),
            subject_name: target_name.to_owned(),
            source: "moderator".to_owned(),
            rule: "manual_warn".to_owned(),
            reason: "اخطار ادمین".to_owned(),
            message: proof.message,
            media_kind: proof.media_kind,
            evidence: proof.text,
            evidence_hash: proof.hash,
            action: "warn".to_owned(),
            action_until: None,
            status: "resolved".to_owned(),
            actor: actor.as_ref().map(|value| value.0),
            actor_name: actor.map_or_else(String::new, |value| value.1),
            event_kind: "action_succeeded".to_owned(),
            event_note: Some(format!("اخطار شماره {count}")),
        },
    )
    .await
}

pub async fn create_report(ctx: &Ctx, command: &Message, reported: &Message) -> Option<i64> {
    let chat = command.peer_id().bot_api_dialog_id()?;
    let reporter = command.sender_id().and_then(PeerId::bare_id);
    let proof = evidence(reported);
    create(
        ctx,
        NewModerationCase {
            chat,
            subject: reported.sender_id().and_then(PeerId::bare_id),
            subject_name: name_of(reported),
            source: "report".to_owned(),
            rule: "member_report".to_owned(),
            reason: "گزارش عضو".to_owned(),
            message: proof.message,
            media_kind: proof.media_kind,
            evidence: proof.text,
            evidence_hash: proof.hash,
            action: "none".to_owned(),
            action_until: None,
            status: "open".to_owned(),
            actor: reporter,
            actor_name: name_of(command),
            event_kind: "reported".to_owned(),
            event_note: None,
        },
    )
    .await
}

async fn create(ctx: &Ctx, draft: NewModerationCase) -> Option<i64> {
    let chat = draft.chat;
    match ctx.settings.create_moderation_case(draft).await {
        Ok(id) => {
            ctx.bump(chat, WRITTEN);
            Some(id)
        }
        Err(error) => {
            ctx.bump(chat, WRITE_FAILED);
            log::warn!("moderation case could not be persisted: {error}");
            None
        }
    }
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let history = history_argument(text).and_then(|arg| super::named(message, arg));
    let command = LIST.contains(&text)
        || history.is_some()
        || exact_case_id(text, SHOW)
        || case_id_and_note(text, CLOSE)
        || case_id_and_note(text, REVERSE);
    if !command {
        return false;
    }
    if !super::limits::allows(ctx, message, super::limits::CASE).await {
        return true;
    }
    let actor = super::sender_of(message);
    let actor = actor.as_ref().map(|(id, name)| (*id, name.as_str()));
    if LIST.contains(&text) {
        show_list(ctx, message, chat, None).await;
    } else if let Some(rest) = text.strip_prefix(CLOSE).map(str::trim) {
        let Some((id, note)) = id_and_note(rest) else {
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::CommandError,
                "مثال: بستن پرونده ۱۲۳ [یادداشت]",
            )
            .await;
            return true;
        };
        let reply = resolve(ctx, chat, id, actor, note.as_deref()).await;
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::ModerationConfirmation,
            reply,
        )
        .await;
    } else if let Some(rest) = text.strip_prefix(REVERSE).map(str::trim) {
        let Some((id, note)) = id_and_note(rest) else {
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::CommandError,
                "مثال: لغو پرونده ۱۲۳ [یادداشت]",
            )
            .await;
            return true;
        };
        let reply = reverse(ctx, chat, id, actor, note.as_deref()).await;
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::ModerationConfirmation,
            reply,
        )
        .await;
    } else if let Some(rest) = text.strip_prefix(SHOW).map(str::trim) {
        match rest.parse::<i64>() {
            Ok(id) => show_one(ctx, message, chat, id).await,
            Err(_) => {
                super::respond(
                    ctx,
                    message,
                    crate::response::ResponseKind::CommandError,
                    "مثال: پرونده ۱۲۳",
                )
                .await;
            }
        }
    } else {
        let Some(named) = history else {
            return false;
        };
        let Some((target, _)) = super::resolve(ctx, message, named).await else {
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::CommandError,
                super::premium::icon_text(Some(super::premium::Icon::ErrorRed), "کاربر پیدا نشد."),
            )
            .await;
            return true;
        };
        show_list(ctx, message, chat, target.id.bare_id()).await;
    }
    true
}

fn history_argument(text: &str) -> Option<Option<&str>> {
    if text == HISTORY {
        return Some(None);
    }
    let tail = text
        .strip_prefix(HISTORY)?
        .strip_prefix(char::is_whitespace)?;
    let tail = tail.trim();
    (tail.split_whitespace().count() == 1 && super::arg_names_a_user(tail)).then_some(Some(tail))
}

fn exact_case_id(text: &str, command: &str) -> bool {
    text.strip_prefix(command)
        .and_then(|tail| tail.strip_prefix(char::is_whitespace))
        .is_some_and(|tail| tail.trim().parse::<i64>().is_ok())
}

fn case_id_and_note(text: &str, command: &str) -> bool {
    text.strip_prefix(command)
        .and_then(|tail| tail.strip_prefix(char::is_whitespace))
        .is_some_and(|tail| id_and_note(tail.trim()).is_some())
}

async fn show_list(ctx: &Ctx, message: &Message, chat: i64, subject: Option<i64>) {
    match ctx
        .settings
        .moderation_cases_open_first(chat, subject, 10)
        .await
    {
        Ok(rows) if rows.is_empty() => {
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::ModerationCaseDetails,
                "پرونده‌ای پیدا نشد.",
            )
            .await;
        }
        Ok(rows) => {
            let body = rows.iter().map(case_line).collect::<Vec<_>>().join("\n");
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::ModerationCaseDetails,
                super::premium::icon_html(
                    Some(super::premium::Icon::DocumentActivity),
                    format!("<b>پرونده‌ها</b>\n\n{body}"),
                ),
            )
            .await;
        }
        Err(error) => {
            log::warn!("case list for {chat} failed: {error}");
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::ModerationCaseDetails,
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    "پرونده‌ها خوانده نشد. کمی بعد دوباره امتحان کنید.",
                ),
            )
            .await;
        }
    }
}

async fn show_one(ctx: &Ctx, message: &Message, chat: i64, id: i64) {
    match ctx.settings.moderation_case(chat, id).await {
        Ok(Some(detail)) => {
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::ModerationCaseDetails,
                super::premium::icon_html(
                    Some(super::premium::Icon::DocumentActivity),
                    render_detail(&detail),
                ),
            )
            .await;
        }
        Ok(None) => {
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::ModerationCaseDetails,
                "پرونده پیدا نشد.",
            )
            .await;
        }
        Err(error) => {
            log::warn!("case {id} read failed: {error}");
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::ModerationCaseDetails,
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    "پرونده خوانده نشد. کمی بعد دوباره امتحان کنید.",
                ),
            )
            .await;
        }
    }
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, chat: i64) {
    let Some((action, raw_id)) = payload.split_once(':') else {
        return;
    };
    let Ok(id) = raw_id.parse::<i64>() else {
        return;
    };
    let actor_name = query
        .sender()
        .and_then(|peer| peer.name())
        .unwrap_or("ادمین")
        .to_owned();
    let actor = query
        .sender_id()
        .bare_id()
        .map(|id| (id, actor_name.as_str()));
    if action == "d"
        && actor.is_some_and(|(id, _)| !super::limits::permits(ctx, chat, id, super::limits::CLEAN))
    {
        super::limits::refuse(query, super::limits::CLEAN).await;
        return;
    }
    let answer = match action {
        "k" => resolve(ctx, chat, id, actor, None).await,
        "d" => resolve_delete(ctx, chat, id, actor).await,
        "u" => reverse(ctx, chat, id, actor, None).await,
        _ => return,
    };
    if let Err(error) = crate::response::send_callback(
        &ctx.client,
        &ctx.settings,
        query,
        chat,
        crate::response::ResponseKind::ModerationCaseDetails,
        answer.into(),
    )
    .await
    {
        ::log::warn!("case callback response failed: {error}");
    }
}

pub async fn resolve(
    ctx: &Ctx,
    chat: i64,
    id: i64,
    actor: Option<(i64, &str)>,
    note: Option<&str>,
) -> String {
    if let Err(message) = valid_note(note) {
        return message.to_owned();
    }
    transition_case(
        ctx,
        ModerationCaseTransition {
            chat,
            case_id: id,
            from: "open",
            to: "resolved",
            event_kind: "resolved",
            actor,
            action: Some("none"),
            note,
        },
    )
    .await
}

pub async fn resolve_delete(ctx: &Ctx, chat: i64, id: i64, actor: Option<(i64, &str)>) -> String {
    let detail = match ctx.settings.moderation_case(chat, id).await {
        Ok(Some(detail)) => detail,
        Ok(None) => return "پرونده پیدا نشد.".to_owned(),
        Err(error) => {
            log::warn!("case {id}: lookup before delete failed: {error}");
            return "پرونده فعلا در دسترس نیست؛ دوباره تلاش کنید.".to_owned();
        }
    };
    if detail.case.status != "open" {
        return "این پرونده قبلا بررسی شده است.".to_owned();
    }
    let (Some(chat_ref), Some(message)) = (ctx.chat_ref(chat), detail.case.message) else {
        return "پیام این پرونده برای حذف در دسترس نیست.".to_owned();
    };
    if let Err(error) = ctx
        .client
        .delete_messages_critical(chat_ref, &[message])
        .await
    {
        log::warn!("case {id}: reported message delete failed: {error}");
        let _ = ctx
            .settings
            .append_moderation_case_event(NewModerationCaseEvent {
                chat,
                case_id: id,
                kind: "action_failed",
                actor,
                action: Some("delete"),
                note: Some("Telegram action rejected"),
            })
            .await;
        return "حذف پیام انجام نشد.".to_owned();
    }
    transition_case(
        ctx,
        ModerationCaseTransition {
            chat,
            case_id: id,
            from: "open",
            to: "resolved",
            event_kind: "resolved",
            actor,
            action: Some("delete"),
            note: None,
        },
    )
    .await
}

pub async fn reverse(
    ctx: &Ctx,
    chat: i64,
    id: i64,
    actor: Option<(i64, &str)>,
    note: Option<&str>,
) -> String {
    if let Err(message) = valid_note(note) {
        return message.to_owned();
    }
    let detail = match ctx.settings.moderation_case(chat, id).await {
        Ok(Some(detail)) => detail,
        Ok(None) => return "پرونده پیدا نشد.".to_owned(),
        Err(error) => {
            log::warn!("case {id}: lookup before reversal failed: {error}");
            return "پرونده فعلا در دسترس نیست؛ دوباره تلاش کنید.".to_owned();
        }
    };
    if detail.case.status == "reversed" {
        return "این پرونده قبلا لغو شده است.".to_owned();
    }
    if detail.case.status != "resolved" {
        return "این پرونده هنوز باز است.".to_owned();
    }
    let Some(user) = detail.case.subject else {
        return "این پرونده کاربر مشخصی ندارد.".to_owned();
    };
    let Some(actor_id) = actor.map(|value| value.0) else {
        return "مدیر شناخته نشد.".to_owned();
    };
    let needed = match detail.case.action.as_str() {
        "warn" => super::limits::WARN,
        "mute" => super::limits::MUTE,
        "ban" => super::limits::BAN,
        "delete" | "kick" | "none" => return "این اقدام قابل بازگردانی نیست.".to_owned(),
        _ => return "اقدام این پرونده قابل بازگردانی نیست.".to_owned(),
    };
    if !super::limits::permits(ctx, chat, actor_id, needed) {
        return format!("دسترسی {} برای شما بسته است.", needed.label);
    }
    if detail.case.action == "warn" {
        return match ctx
            .settings
            .reverse_warning_case(WarningCaseReversal {
                chat,
                case_id: id,
                user,
                actor: actor_id,
                actor_name: actor.map_or("", |value| value.1),
                note,
            })
            .await
        {
            Ok(WarningCaseReversalResult::Reversed) => "✓ پرونده لغو شد.".to_owned(),
            Ok(WarningCaseReversalResult::CaseChanged) => {
                "این پرونده قبلا تغییر کرده یا پیدا نشد.".to_owned()
            }
            Ok(WarningCaseReversalResult::NoWarning) => {
                "اخطاری برای بازگردانی باقی نمانده است.".to_owned()
            }
            Err(error) => {
                log::warn!("case {id}: atomic warning reversal failed: {error}");
                "اخطار بازگردانده نشد؛ دوباره تلاش کنید.".to_owned()
            }
        };
    }
    let action = detail.case.action.as_str();
    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return "گروه در دسترس نیست.".to_owned();
    };
    let Some(target) = PeerId::user(user).map(PeerId::to_ambient_ref) else {
        return "کاربر پیدا نشد.".to_owned();
    };
    let reverse = if action == "mute" {
        Action::Unmute
    } else {
        Action::Unban
    };
    let undo = restrict::apply(
        ctx,
        chat_ref,
        target,
        reverse,
        None,
        restrict::By {
            actor,
            reason: "لغو پرونده",
            target_name: &detail.case.subject_name,
            ..Default::default()
        },
    )
    .await
    .map(|_| ());
    if let Err(error) = undo {
        let told = error.told();
        let _ = ctx
            .settings
            .append_moderation_case_event(NewModerationCaseEvent {
                chat,
                case_id: id,
                kind: "action_failed",
                actor,
                action: Some("reverse"),
                note: Some("Telegram action rejected"),
            })
            .await;
        return told;
    }
    transition_case(
        ctx,
        ModerationCaseTransition {
            chat,
            case_id: id,
            from: "resolved",
            to: "reversed",
            event_kind: "reversed",
            actor,
            action: Some("reverse"),
            note,
        },
    )
    .await
}

async fn transition_case(ctx: &Ctx, transition: ModerationCaseTransition<'_>) -> String {
    let case_id = transition.case_id;
    let to = transition.to;
    match ctx.settings.transition_moderation_case(transition).await {
        Ok(true) => if to == "reversed" {
            "✓ پرونده لغو شد."
        } else {
            "✓ پرونده بسته شد."
        }
        .to_owned(),
        Ok(false) => "این پرونده قبلا تغییر کرده یا پیدا نشد.".to_owned(),
        Err(error) => {
            log::warn!("case {case_id} transition failed: {error}");
            "تغییر پرونده ذخیره نشد. کمی بعد دوباره امتحان کنید.".to_owned()
        }
    }
}

fn id_and_note(input: &str) -> Option<(i64, Option<String>)> {
    let mut parts = input.splitn(2, char::is_whitespace);
    let id = parts.next()?.parse().ok()?;
    let note = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some((id, note))
}

pub(crate) fn valid_note(note: Option<&str>) -> Result<(), &'static str> {
    match note {
        Some(note) if note.trim().is_empty() => Err("یادداشت خالی است."),
        Some(note) if note.chars().count() > NOTE_MAX => Err("یادداشت بیشتر از ۵۰۰ نویسه است."),
        _ => Ok(()),
    }
}

fn case_line(case: &ModerationCase) -> String {
    format!(
        "<code>#{}</code> · {} · {} · {}",
        case.id,
        esc(&case.subject_name),
        status_label(&case.status),
        esc(&case.reason)
    )
}

pub fn render_detail(detail: &ModerationCaseDetail) -> String {
    let case = &detail.case;
    let mut lines = vec![
        format!("<b>پرونده #{}</b>", case.id),
        String::new(),
        format!("کاربر · {}", esc(&case.subject_name)),
        format!("وضعیت · {}", status_label(&case.status)),
        format!("دلیل · {}", esc(&case.reason)),
        format!("اقدام · {}", action_label(&case.action)),
    ];
    if let Some(text) = &case.evidence {
        lines.push(format!("متن · {}", esc(text)));
    }
    if !detail.events.is_empty() {
        lines.push(String::new());
        lines.push("<b>رویدادها</b>".to_owned());
        lines.extend(detail.events.iter().map(|event| {
            let actor = if event.actor_name.is_empty() {
                "ربات"
            } else {
                &event.actor_name
            };
            let note = event
                .note
                .as_deref()
                .map_or(String::new(), |note| format!(" · {}", esc(note)));
            format!("‹ {} · {}{note}", event_label(&event.kind), esc(actor))
        }));
    }
    lines.join("\n")
}

fn status_label(status: &str) -> &'static str {
    match status {
        "open" => "باز",
        "resolved" => "بسته",
        "reversed" => "لغوشده",
        _ => "نامشخص",
    }
}

fn action_label(action: &str) -> &'static str {
    match action {
        "none" => "بدون اقدام",
        "delete" => "حذف",
        "warn" => "اخطار",
        "mute" => "سکوت",
        "ban" => "بن",
        "kick" => "کیک",
        _ => "نامشخص",
    }
}

fn event_label(kind: &str) -> &'static str {
    match kind {
        "reported" => "گزارش شد",
        "action_succeeded" => "اقدام انجام شد",
        "action_failed" => "اقدام ناموفق",
        "resolved" => "بررسی شد",
        "reversed" => "لغو شد",
        "note" => "یادداشت",
        _ => "رویداد",
    }
}

pub fn action_key(action: Action) -> &'static str {
    match action {
        Action::Mute => "mute",
        Action::Ban => "ban",
        Action::Kick => "kick",
        Action::Unmute => "unmute",
        Action::Unban => "unban",
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

pub async fn run_retention(ctx: Arc<Ctx>, mut stopping: tokio::sync::watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
    loop {
        tokio::select! {
            _ = stopping.changed() => break,
            _ = interval.tick() => {
                let _epoch = ctx.background_epoch().await;
                loop {
                    match ctx.settings.purge_expired_moderation_cases().await {
                        Ok(10_000) => continue,
                        Ok(_) => break,
                        Err(error) => {
                            log::warn!("moderation case retention failed: {error}");
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_only_claims_a_real_user_argument_or_bare_reply_form() {
        assert_eq!(history_argument("تاریخچه"), Some(None));
        assert_eq!(history_argument("تاریخچه @ali"), Some(Some("@ali")));
        assert_eq!(history_argument("تاریخچه ۱۲۳۴"), Some(Some("۱۲۳۴")));
        assert_eq!(history_argument("تاریخچه خوبی نداری😭"), None);
        assert_eq!(history_argument("تاریخچه‌ خوبی نداری"), None);
        assert_eq!(history_argument("تاریخچه @ali امروز"), None);
    }

    #[test]
    fn other_case_commands_have_closed_numeric_grammars() {
        assert!(exact_case_id("پرونده 123", SHOW));
        assert!(!exact_case_id("پرونده خوبی نیست", SHOW));
        assert!(case_id_and_note("بستن پرونده 123 اشتباه بود", CLOSE));
        assert!(!case_id_and_note("بستن پرونده کار سختی است", CLOSE));
    }

    #[test]
    fn evidence_truncation_is_unicode_safe() {
        let text: String = std::iter::repeat_n('ش', EVIDENCE_MAX + 20).collect();
        let clipped: String = text.chars().take(EVIDENCE_MAX).collect();
        assert_eq!(clipped.chars().count(), EVIDENCE_MAX);
        assert!(clipped.is_char_boundary(clipped.len()));
    }

    #[test]
    fn notes_are_bounded() {
        assert!(valid_note(Some("دلیل")).is_ok());
        assert!(valid_note(Some("")).is_err());
        assert!(valid_note(Some(&"x".repeat(NOTE_MAX + 1))).is_err());
    }
}
