use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::{PeerId, PeerKind, PeerRef};
use grammers_client::update::CallbackQuery;

use super::premium::Icon;
use super::restrict::{self, Action};
use super::style::Colour;
use super::{Ctx, esc, name_of};
use crate::response::ResponseKind;
use crate::state::{ModerationCaseDetail, NewModerationCaseEvent};

pub const COMMANDS: &[&str] = &[
    "گزارش",
    "ریپورت",
    "گزارش تخلف",
    "report",
    "!report",
    "/report",
    "گزارش پیام",
    "/reportmsg",
];

pub const REASONS: &[(&str, &[&str])] = &[
    ("اسپم", &["اسپم", "هرزنامه", "spam"]),
    ("تبلیغ", &["تبلیغ", "تبلیغات", "ads"]),
    ("توهین", &["توهین", "فحش", "فحاشی", "abuse"]),
    ("کلاهبرداری", &["کلاهبرداری", "کلاهبردار", "اسکم", "scam"]),
    ("محتوای نامناسب", &["مستهجن", "نامناسب", "غیر اخلاقی", "nsfw"]),
    ("مزاحمت", &["مزاحمت", "آزار"]),
];

pub const REASON: &str = "گزارش عضو";

pub const LOG: &str = "log_report";

const ANCHOR: &str = "\u{2063}";

pub const EVERY: Duration = Duration::from_secs(60);

const PINGS: usize = 10;

const CARDS_MAX: usize = 256;
const CARD_TTL: Duration = Duration::from_secs(24 * 60 * 60);

const REPORTERS_MAX: usize = 50;

const SETTLED_TTL: Duration = Duration::from_secs(60);

const REFUSAL_TTL: Duration = Duration::from_secs(15);

#[derive(Default)]
pub struct Cards {
    open: Mutex<HashMap<i32, Card>>,
    acting: Mutex<HashSet<i64>>,
}

struct Card {
    at: Instant,
    case: i64,
    card: i32,
    subject: String,
    reason: Option<&'static str>,
    reporters: Vec<i64>,
    unwritten: Vec<Joiner>,
}

struct Joiner {
    id: i64,
    name: String,
    reason: Option<&'static str>,
}

enum Joined {
    First,
    Again { case: i64, card: i32, count: usize },
    Repeat,
}

struct Acting<'a> {
    cards: &'a Cards,
    case: i64,
}

impl Drop for Acting<'_> {
    fn drop(&mut self) {
        self.cards.acting.lock().unwrap().remove(&self.case);
    }
}

impl Cards {
    fn join(
        &self,
        message: i32,
        reporter: (i64, &str),
        subject: &str,
        reason: Option<&'static str>,
    ) -> Joined {
        let mut open = self.open.lock().unwrap();
        if let Some(card) = open.get_mut(&message)
            && card.at.elapsed() < CARD_TTL
        {
            if card.reporters.contains(&reporter.0) {
                return Joined::Repeat;
            }
            if card.reporters.len() < REPORTERS_MAX {
                card.reporters.push(reporter.0);
                if card.case == 0 {
                    card.unwritten.push(Joiner {
                        id: reporter.0,
                        name: reporter.1.to_owned(),
                        reason,
                    });
                }
            }
            return Joined::Again {
                case: card.case,
                card: card.card,
                count: card.reporters.len(),
            };
        }
        if !open.contains_key(&message) && open.len() >= CARDS_MAX {
            super::make_room(&mut open, CARDS_MAX, |card| card.at);
        }
        open.insert(
            message,
            Card {
                at: Instant::now(),
                case: 0,
                card: 0,
                subject: subject.to_owned(),
                reason,
                reporters: vec![reporter.0],
                unwritten: Vec::new(),
            },
        );
        Joined::First
    }

    fn opened(
        &self,
        message: i32,
        case: i64,
        card: i32,
    ) -> (usize, Vec<Joiner>) {
        let mut open = self.open.lock().unwrap();
        let Some(entry) = open.get_mut(&message) else {
            return (1, Vec::new());
        };
        entry.case = case;
        entry.card = card;
        (entry.reporters.len(), std::mem::take(&mut entry.unwritten))
    }

    fn forget(&self, message: i32) {
        self.open.lock().unwrap().remove(&message);
    }

    fn close(&self, case: i64) -> Option<Card> {
        let mut open = self.open.lock().unwrap();
        let message = open
            .iter()
            .find(|(_, card)| card.case == case)
            .map(|(message, _)| *message)?;
        open.remove(&message)
    }

    fn reason_of(&self, case: i64) -> Option<Option<&'static str>> {
        self.open
            .lock()
            .unwrap()
            .values()
            .find(|card| card.case == case)
            .map(|card| card.reason)
    }

    fn claim(&self, case: i64) -> Option<Acting<'_>> {
        self.acting
            .lock()
            .unwrap()
            .insert(case)
            .then(|| Acting { cards: self, case })
    }
}

fn parse(text: &str) -> Option<Option<&'static str>> {
    if super::commands::contains(COMMANDS, text) {
        return Some(None);
    }
    COMMANDS.iter().find_map(|command| {
        let tail = super::commands::strip_prefix(text, command)?;
        REASONS
            .iter()
            .find(|(_, words)| words.iter().any(|word| super::commands::matches(tail, word)))
            .map(|(label, _)| Some(*label))
    })
}

async fn refusal(ctx: &Ctx, reported: &Message, reporter: i64) -> Option<&'static str> {
    if reported.action().is_some() {
        return Some("✗ این پیام گزارش نمی شود.");
    }
    let sender = reported.sender_id();
    let is_me = |peer: PeerId| peer.kind() == PeerKind::User && peer.bare_id() == Some(ctx.me_id());
    if reported.outgoing() || sender.is_some_and(is_me) {
        return Some("✗ پیام ربات گزارش نمی شود.");
    }
    if sender.and_then(PeerId::bare_id) == Some(reporter) {
        return Some("✗ پیام خودتان را نمی شود گزارش کرد.");
    }
    if super::is_linked_post(reported) || super::can_manage(ctx, reported).await {
        return Some("✗ پیام مدیران گروه گزارش نمی شود.");
    }
    None
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let Some(reason) = parse(message.text()) else {
        return false;
    };
    let (Some(chat), Some(user)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_id().and_then(PeerId::bare_id),
    ) else {
        return false;
    };
    let Ok(Some(reported)) = message.get_reply().await else {
        return false;
    };

    if !ctx.may_report(chat, user) {
        let _ = message.delete().await;
        return true;
    }
    if let Some(refused) = refusal(ctx, &reported, user).await {
        refuse(ctx, message, chat, refused).await;
        let _ = message.delete().await;
        return true;
    }

    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        let _ = message.delete().await;
        return true;
    };

    let reporter = name_of(message);
    let subject = name_of(&reported);
    let subject_user = reported
        .sender_id()
        .is_some_and(|peer| peer.kind() == PeerKind::User);
    let state = ctx.peek(chat);
    let joined = state.as_ref().map_or(Joined::First, |state| {
        state
            .reports
            .join(reported.id(), (user, &reporter), &subject, reason)
    });
    match joined {
        Joined::Repeat => {
            let _ = message.delete().await;
            return true;
        }
        Joined::Again { case, card, count } => {
            ctx.bump(chat, super::stats::REPORTED);
            let _ = message.delete().await;
            if case != 0 {
                joined_event(ctx, chat, case, (user, &reporter), reason).await;
            }
            if card != 0 && worth_showing(count) {
                let report = Report {
                    case,
                    subject: &subject,
                    reason: state
                        .as_ref()
                        .and_then(|state| state.reports.reason_of(case))
                        .flatten(),
                    reports: count,
                };
                if let Err(error) = ctx
                    .client
                    .edit_message(chat_ref, card, open_card(&report, subject_user, ""))
                    .await
                {
                    log::warn!("report: {chat}: could not count a report on card {card}: {error}");
                }
            }
            return true;
        }
        Joined::First => {}
    }
    ctx.bump(chat, super::stats::REPORTED);

    let forget = || {
        if let Some(state) = &state {
            state.reports.forget(reported.id());
        }
    };
    let Some(admins) = super::chat_admins(ctx, chat_ref, chat).await else {
        log::warn!("report: {chat}: could not obtain a complete administrator list");
        forget();
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "گزارش ثبت نشد؛ فهرست مدیران گروه در دسترس نبود. دوباره تلاش کنید.",
        )
        .await;
        let _ = message.delete().await;
        return true;
    };
    let full_reason = reason.map(|label| format!("{REASON} · {label}"));
    let Some(case) = super::cases::create_report(
        ctx,
        message,
        &reported,
        full_reason.as_deref().unwrap_or(REASON),
    )
    .await
    else {
        forget();
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "گزارش ثبت نشد؛ ذخیره سازی پرونده در دسترس نبود. دوباره تلاش کنید.",
        )
        .await;
        let _ = message.delete().await;
        return true;
    };
    let _ = message.delete().await;

    let mut report = Report {
        case,
        subject: &subject,
        reason,
        reports: 1,
    };
    match reported
        .reply(open_card(
            &report,
            subject_user,
            &pings(ctx, chat, &admins, user),
        ))
        .await
    {
        Ok(card) => {
            let (count, unwritten) = state
                .as_ref()
                .map_or((1, Vec::new()), |state| state.reports.opened(reported.id(), case, card.id()));
            for joiner in unwritten {
                joined_event(ctx, chat, case, (joiner.id, &joiner.name), joiner.reason).await;
            }
            if count > 1 {
                report.reports = count;
                let _ = ctx
                    .client
                    .edit_message(chat_ref, card.id(), open_card(&report, subject_user, ""))
                    .await;
            }
        }
        Err(error) => {
            log::warn!("report: {chat}: could not send the card for case {case}: {error}");
            forget();
        }
    }

    let case_label = format!("#{case}");
    super::log::write(
        ctx,
        chat,
        LOG,
        super::log::Entry {
            title: "گزارش",
            target: reported
                .sender_id()
                .filter(|_| subject_user)
                .and_then(PeerId::bare_id)
                .map(|id| (id, subject.as_str())),
            actor: Some((user, &reporter)),
            reason: Some(reason.unwrap_or(REASON)),
            extra: vec![("پرونده", case_label)],
        },
    )
    .await;
    true
}

fn worth_showing(count: usize) -> bool {
    count <= 5 || count.is_multiple_of(5)
}

async fn joined_event(
    ctx: &Ctx,
    chat: i64,
    case: i64,
    reporter: (i64, &str),
    reason: Option<&str>,
) {
    if let Err(error) = ctx
        .settings
        .append_moderation_case_event(NewModerationCaseEvent {
            chat,
            case_id: case,
            kind: "reported",
            actor: Some(reporter),
            action: None,
            note: reason,
        })
        .await
    {
        log::warn!("report: case {case}: could not record another report: {error}");
    }
}

async fn refuse(ctx: &Ctx, message: &Message, chat: i64, text: &str) {
    match crate::response::send_tracked(
        &ctx.client,
        &ctx.settings,
        message,
        ResponseKind::CommandError,
        crate::response::IntendedAudience::RequesterOnly,
        text.into(),
    )
    .await
    {
        Ok(sent) => {
            if let Some(id) = sent.group_message_id {
                ctx.schedule_delete(chat, id, Instant::now() + REFUSAL_TTL);
            }
        }
        Err(error) => log::warn!("report: {chat}: could not refuse a report: {error}"),
    }
}

fn pings(ctx: &Ctx, chat: i64, admins: &HashSet<i64>, reporter: i64) -> String {
    let owner = super::owner(ctx, chat);
    let me = ctx.me_id();
    let mut ids: Vec<i64> = admins
        .iter()
        .copied()
        .filter(|id| *id != me && *id != reporter)
        .collect();
    ids.sort_unstable_by_key(|id| (Some(*id) != owner, *id));
    ids.into_iter()
        .take(PINGS)
        .map(|id| format!("<a href=\"tg://user?id={id}\">{ANCHOR}</a>"))
        .collect()
}

struct Report<'a> {
    case: i64,
    subject: &'a str,
    reason: Option<&'a str>,
    reports: usize,
}

impl Report<'_> {
    fn lines(&self) -> String {
        let mut lines = vec![format!("کاربر · {}", esc(self.subject))];
        if let Some(reason) = self.reason {
            lines.push(format!("دلیل · <b>{}</b>", esc(reason)));
        }
        if self.reports > 1 {
            lines.push(format!("گزارش ها · <b>{}</b>", self.reports));
        }
        lines.join("\n")
    }
}

fn open_card(report: &Report, user: bool, pings: &str) -> InputMessage {
    let note = if user {
        "\n\n<i>اخطار، سکوت و بن پیام را هم پاک می کنند.</i>"
    } else {
        ""
    };
    super::premium::icon_html(
        Some(Icon::DocumentActivity),
        format!(
            "<b>گزارش به مدیران</b>\n\n{}\nپرونده · <code>#{}</code>{note}{pings}",
            report.lines(),
            report.case
        ),
    )
    .reply_markup(markup(report.case, user))
}

fn settled_card(report: &Report, outcome: &str, by: &str) -> InputMessage {
    super::premium::icon_html(
        Some(Icon::Success),
        format!(
            "<b>گزارش بررسی شد</b>\n\n{}\nنتیجه · <b>{}</b>\nتوسط · {}\nپرونده · <code>#{}</code>",
            report.lines(),
            esc(outcome),
            esc(by),
            report.case
        ),
    )
}

fn markup(case: i64, user: bool) -> ReplyMarkup {
    let button = |label: &str, action: &str, colour: Option<Colour>, icon: Icon| {
        let data = format!("mc:{action}:{case}").into_bytes();
        let button = match colour {
            Some(colour) => super::style::data(label, data, colour),
            None => Button::data(label, data),
        };
        super::premium::decorate(button, Some(icon))
    };
    let mut rows = vec![vec![
        button("حذف پیام", "d", Some(Colour::Danger), Icon::Delete),
        button("بدون اقدام", "k", None, Icon::Close),
    ]];
    if user {
        rows.push(vec![
            button("اخطار", "w", None, Icon::Warning),
            button("سکوت", "m", None, Icon::Muted),
            button("بن", "b", Some(Colour::Danger), Icon::ModerationHammer),
        ]);
    }
    super::premium::buttons(&rows)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Dismiss,
    Delete,
    Warn,
    Mute,
    Ban,
}

impl Verdict {
    fn of(action: &str) -> Option<Self> {
        Some(match action {
            "k" => Self::Dismiss,
            "d" => Self::Delete,
            "w" => Self::Warn,
            "m" => Self::Mute,
            "b" => Self::Ban,
            _ => return None,
        })
    }

    fn key(self) -> &'static str {
        match self {
            Self::Dismiss => "none",
            Self::Delete => "delete",
            Self::Warn => "warn",
            Self::Mute => "mute",
            Self::Ban => "ban",
        }
    }

    fn cap(self) -> Option<&'static super::limits::Cap> {
        match self {
            Self::Dismiss => None,
            Self::Delete => Some(super::limits::CLEAN),
            Self::Warn => Some(super::limits::WARN),
            Self::Mute => Some(super::limits::MUTE),
            Self::Ban => Some(super::limits::BAN),
        }
    }
}

pub async fn on_card(ctx: &Ctx, query: &CallbackQuery, action: &str, case: i64, chat: i64) {
    let Some(verdict) = Verdict::of(action) else {
        return;
    };
    let Some(actor_id) = query.sender_id().bare_id() else {
        return;
    };
    let actor_name = query
        .sender()
        .and_then(|peer| peer.name())
        .unwrap_or("ادمین")
        .to_owned();
    let actor = (actor_id, actor_name.as_str());
    if let Some(cap) = verdict.cap()
        && !super::limits::permits(ctx, chat, actor_id, cap)
    {
        super::limits::refuse(query, cap).await;
        return;
    }

    let state = ctx.peek(chat);
    let _acting = match state.as_ref().map(|state| state.reports.claim(case)) {
        Some(None) => {
            toast(query, "مدیر دیگری همین حالا در حال رسیدگی است.").await;
            return;
        }
        claim => claim.flatten(),
    };
    let detail = match ctx.settings.moderation_case(chat, case).await {
        Ok(Some(detail)) => detail,
        Ok(None) => {
            toast(query, "پرونده این گزارش دیگر نیست.").await;
            return;
        }
        Err(error) => {
            log::warn!("report: case {case}: lookup failed: {error}");
            toast(query, "پرونده فعلا در دسترس نیست؛ دوباره تلاش کنید.").await;
            return;
        }
    };
    let reported = Reported::of(&detail);
    if detail.case.status != "open" {
        if let Some(state) = &state {
            state.reports.close(case);
        }
        let (outcome, by) = settled_by(&detail);
        let _ = query
            .answer()
            .text("این گزارش قبلا بررسی شده است.")
            .edit(settled_card(&reported.report(), outcome, by))
            .await;
        return;
    }
    let chat_ref = match query.peer_ref().await {
        Ok(Some(peer)) => Some(peer),
        _ => ctx.chat_ref(chat),
    };
    let Some(chat_ref) = chat_ref else {
        toast(query, "گروه در دسترس نیست.").await;
        return;
    };

    let outcome = match act(ctx, chat, chat_ref, &detail, verdict, actor).await {
        Ok(outcome) => outcome,
        Err(told) => {
            let _ = ctx
                .settings
                .append_moderation_case_event(NewModerationCaseEvent {
                    chat,
                    case_id: case,
                    kind: "action_failed",
                    actor: Some(actor),
                    action: Some(verdict.key()),
                    note: Some(&told),
                })
                .await;
            let _ = query.answer().alert(told).send().await;
            return;
        }
    };
    match ctx
        .settings
        .settle_report_case(chat, case, verdict.key(), Some(actor), Some(&outcome))
        .await
    {
        Ok(true) => {}
        Ok(false) => log::warn!("report: case {case} was settled elsewhere during an action"),
        Err(error) => log::warn!("report: case {case}: settlement failed: {error}"),
    }
    let reports = state
        .as_ref()
        .and_then(|state| state.reports.close(case))
        .map_or(reported.reports, |card| card.reporters.len().max(reported.reports));
    let report = Report {
        reports,
        ..reported.report()
    };
    match query
        .answer()
        .text("✓ انجام شد")
        .edit(settled_card(&report, &outcome, &actor_name))
        .await
    {
        Ok(()) => {
            if let Some(card) = card_of(query) {
                ctx.schedule_delete(chat, card, Instant::now() + SETTLED_TTL);
            }
        }
        Err(error) => log::warn!("report: case {case}: could not settle the card: {error}"),
    }
    super::log::write(
        ctx,
        chat,
        LOG,
        super::log::Entry {
            title: "گزارش بررسی شد",
            target: detail
                .case
                .subject
                .map(|id| (id, detail.case.subject_name.as_str())),
            actor: Some(actor),
            reason: Some(&outcome),
            extra: vec![("پرونده", format!("#{case}"))],
        },
    )
    .await;
}

async fn act(
    ctx: &Ctx,
    chat: i64,
    chat_ref: PeerRef,
    detail: &ModerationCaseDetail,
    verdict: Verdict,
    actor: (i64, &str),
) -> Result<String, String> {
    let case = &detail.case;
    let may_delete = super::limits::permits(ctx, chat, actor.0, super::limits::CLEAN);
    let delete = || async {
        match case.message {
            Some(id) if may_delete => ctx.delete_proven(chat, chat_ref, id).await,
            _ => Ok(false),
        }
    };
    let with_deletion = |outcome: String, deleted: bool| {
        if deleted {
            format!("{outcome} و پیام حذف شد")
        } else {
            outcome
        }
    };
    match verdict {
        Verdict::Dismiss => Ok("بدون اقدام بسته شد".to_owned()),
        Verdict::Delete => {
            if case.message.is_none() {
                return Err("پیام این گزارش برای حذف در دسترس نیست.".to_owned());
            }
            match delete().await {
                Ok(true) => Ok("پیام حذف شد".to_owned()),
                Ok(false) => Ok("پیام پیش تر حذف شده بود".to_owned()),
                Err(error) => {
                    log::warn!("report: case {}: delete failed: {error}", case.id);
                    Err("✗ پیام حذف نشد. مطمئن شوید ربات اجازه حذف پیام دارد.".to_owned())
                }
            }
        }
        Verdict::Warn => {
            let Some(user) = case.subject else {
                return Err("این گزارش کاربر مشخصی ندارد.".to_owned());
            };
            let given = super::warns::give(ctx, chat_ref, user, &case.subject_name, Some(actor))
                .await
                .map_err(str::to_owned)?;
            let deleted = matches!(delete().await, Ok(true));
            Ok(with_deletion(
                match given {
                    super::warns::Given::Counted { count, limit } => {
                        format!("اخطار {count} از {limit}")
                    }
                    super::warns::Given::Punished { limit, ban } => format!(
                        "به {limit} اخطار رسید و {}",
                        if ban { "بن شد" } else { "سکوت شد" }
                    ),
                    super::warns::Given::Unpunished { limit, told } => {
                        format!("به {limit} اخطار رسید؛ {}", told.trim_start_matches("✗ "))
                    }
                },
                deleted,
            ))
        }
        Verdict::Mute | Verdict::Ban => {
            let Some(target) = case
                .subject
                .and_then(PeerId::user)
                .map(PeerId::to_ambient_ref)
            else {
                return Err("این گزارش کاربر مشخصی ندارد.".to_owned());
            };
            let action = if verdict == Verdict::Ban {
                Action::Ban
            } else {
                Action::Mute
            };
            let wiped = restrict::apply(
                ctx,
                chat_ref,
                target,
                action,
                None,
                restrict::By {
                    actor: Some(actor),
                    reason: &case.reason,
                    target_name: &case.subject_name,
                    ..Default::default()
                },
            )
            .await
            .map_err(|failed| failed.told())?;
            let deleted = matches!(delete().await, Ok(true));
            let outcome = with_deletion(
                if verdict == Verdict::Ban {
                    "بن شد".to_owned()
                } else {
                    "سکوت شد".to_owned()
                },
                deleted,
            );
            Ok(match wiped {
                0 => outcome,
                n => format!("{outcome} · {n} پیام دیگرش هم پاک شد"),
            })
        }
    }
}

struct Reported<'a> {
    case: i64,
    subject: &'a str,
    reason: Option<&'a str>,
    reports: usize,
}

impl<'a> Reported<'a> {
    fn of(detail: &'a ModerationCaseDetail) -> Self {
        Self {
            case: detail.case.id,
            subject: &detail.case.subject_name,
            reason: reason_of(&detail.case.reason),
            reports: detail
                .events
                .iter()
                .filter(|event| event.kind == "reported")
                .count()
                .max(1),
        }
    }

    fn report(&self) -> Report<'a> {
        Report {
            case: self.case,
            subject: self.subject,
            reason: self.reason,
            reports: self.reports,
        }
    }
}

fn reason_of(reason: &str) -> Option<&str> {
    reason
        .strip_prefix(REASON)
        .and_then(|rest| rest.strip_prefix(" · "))
        .filter(|label| !label.is_empty())
}

fn settled_by(detail: &ModerationCaseDetail) -> (&str, &str) {
    let settled = detail
        .events
        .iter()
        .rev()
        .find(|event| matches!(event.kind.as_str(), "resolved" | "reversed"));
    let by = settled
        .map(|event| event.actor_name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("ادمین");
    let outcome = if detail.case.status == "reversed" {
        "لغو شد"
    } else {
        match settled.and_then(|event| event.action.as_deref()) {
            Some("delete") => "پیام حذف شد",
            Some("warn") => "اخطار گرفت",
            Some("mute") => "سکوت شد",
            Some("ban") => "بن شد",
            _ => "بدون اقدام بسته شد",
        }
    };
    (outcome, by)
}

pub async fn settled_elsewhere(ctx: &Ctx, chat: i64, case: i64, outcome: &str, by: &str) {
    let Some(card) = ctx.peek(chat).and_then(|state| state.reports.close(case)) else {
        return;
    };
    let Some(chat_ref) = ctx.chat_ref(chat).filter(|_| card.card != 0) else {
        return;
    };
    let report = Report {
        case,
        subject: &card.subject,
        reason: card.reason,
        reports: card.reporters.len(),
    };
    match ctx
        .client
        .edit_message(chat_ref, card.card, settled_card(&report, outcome, by))
        .await
    {
        Ok(()) => ctx.schedule_delete(chat, card.card, Instant::now() + SETTLED_TTL),
        Err(error) => log::warn!("report: case {case}: could not settle card {}: {error}", card.card),
    }
}

fn card_of(query: &CallbackQuery) -> Option<i32> {
    match query.raw() {
        grammers_client::tl::enums::Update::BotCallbackQuery(update) => Some(update.msg_id),
        _ => None,
    }
}

async fn toast(query: &CallbackQuery, text: &str) {
    let _ = query.answer().text(text).send().await;
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, chat: i64) {
    let Some((what, id)) = payload.split_once(':') else {
        return;
    };
    let Ok(id) = id.parse::<i32>() else {
        return;
    };
    let by = query
        .sender()
        .and_then(|peer| peer.name())
        .unwrap_or("ادمین")
        .to_owned();

    if what == "d" {
        let Some(actor) = query.sender_id().bare_id() else {
            return;
        };
        if !super::limits::permits(ctx, chat, actor, super::limits::CLEAN) {
            super::limits::refuse(query, super::limits::CLEAN).await;
            return;
        }
    }

    let text = match what {
        "d" => {
            let Ok(Some(chat_ref)) = query.peer_ref().await else {
                return;
            };
            match ctx.client.delete_messages_critical(chat_ref, &[id]).await {
                Ok(_) => format!("‹ پیام گزارش شده حذف شد · {}", esc(&by)),
                Err(e) => {
                    eprintln!("report: {chat}: could not delete {id}: {e}");
                    "انجام نشد. مطمئن شوید ربات اجازه حذف پیام دارد.".to_owned()
                }
            }
        }
        "k" => format!("‹ گزارش بررسی شد · {}", esc(&by)),
        _ => return,
    };

    let _ = query
        .answer()
        .edit(super::premium::icon_html(Some(Icon::DocumentActivity), text))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_takes_a_named_reason_and_nothing_else() {
        assert_eq!(parse("گزارش"), Some(None));
        assert_eq!(parse("/report"), Some(None));
        assert_eq!(parse("گزارش اسپم"), Some(Some("اسپم")));
        assert_eq!(parse("گزارش  فحاشی"), Some(Some("توهین")));
        assert_eq!(parse("گزارش تخلف تبلیغات"), Some(Some("تبلیغ")));
        assert_eq!(parse("گزارش غیر اخلاقی"), Some(Some("محتوای نامناسب")));
        assert_eq!(parse("/report spam"), Some(Some("اسپم")));
        assert_eq!(parse("گزارش دادم"), None);
        assert_eq!(parse("گزارش اسپم بدید"), None);
        assert_eq!(parse("گزارش روزانه"), None);
        assert_eq!(parse("گزارشش کنید"), None);
    }

    #[test]
    fn every_reason_label_is_one_of_its_own_words_or_a_plain_heading() {
        for (label, words) in REASONS {
            assert!(!words.is_empty(), "«{label}» has no words");
            for word in *words {
                assert!(
                    REASONS
                        .iter()
                        .filter(|(_, other)| other.contains(word))
                        .count()
                        == 1,
                    "«{word}» names two reasons"
                );
            }
        }
    }

    #[test]
    fn a_case_reason_gives_back_the_label_it_was_written_with() {
        assert_eq!(reason_of(REASON), None);
        assert_eq!(reason_of(&format!("{REASON} · اسپم")), Some("اسپم"));
        assert_eq!(reason_of("سقف اخطار"), None);
    }

    #[test]
    fn a_second_report_joins_the_first_card_and_a_repeat_is_ignored() {
        let cards = Cards::default();
        assert!(matches!(
            cards.join(7, (1, "a"), "spammer", Some("اسپم")),
            Joined::First
        ));
        assert!(matches!(
            cards.join(7, (2, "b"), "spammer", None),
            Joined::Again { case: 0, card: 0, count: 2 }
        ));
        assert!(matches!(
            cards.join(7, (2, "b"), "spammer", None),
            Joined::Repeat
        ));
        let (count, unwritten) = cards.opened(7, 40, 900);
        assert_eq!(count, 2);
        assert_eq!(unwritten.len(), 1);
        assert_eq!(unwritten[0].id, 2);
        assert!(matches!(
            cards.join(7, (3, "c"), "spammer", None),
            Joined::Again { case: 40, card: 900, count: 3 }
        ));
        assert_eq!(cards.reason_of(40), Some(Some("اسپم")));
        let card = cards.close(40).expect("the card is open");
        assert_eq!(card.reporters, vec![1, 2, 3]);
        assert!(matches!(
            cards.join(7, (2, "b"), "spammer", None),
            Joined::First
        ));
    }

    #[test]
    fn open_cards_stay_bounded() {
        let cards = Cards::default();
        for message in 0..(CARDS_MAX as i32 + 20) {
            cards.join(message, (1, "a"), "x", None);
        }
        assert_eq!(cards.open.lock().unwrap().len(), CARDS_MAX);
        let card = cards.open.lock().unwrap();
        let mut reporters = card.values().map(|card| card.reporters.len());
        assert!(reporters.all(|count| count == 1));
    }

    #[test]
    fn one_card_counts_a_bounded_number_of_reporters() {
        let cards = Cards::default();
        cards.join(1, (0, "a"), "x", None);
        for reporter in 1..(REPORTERS_MAX as i64 + 10) {
            cards.join(1, (reporter, "r"), "x", None);
        }
        let open = cards.open.lock().unwrap();
        let card = open.get(&1).unwrap();
        assert_eq!(card.reporters.len(), REPORTERS_MAX);
        assert_eq!(card.unwritten.len(), REPORTERS_MAX - 1);
    }

    #[test]
    fn one_tap_acts_on_a_case_at_a_time() {
        let cards = Cards::default();
        let first = cards.claim(5);
        assert!(first.is_some());
        assert!(cards.claim(5).is_none());
        assert!(cards.claim(6).is_some());
        drop(first);
        assert!(cards.claim(5).is_some());
    }

    #[test]
    fn the_card_never_names_the_reporter_and_escapes_the_reported() {
        let report = Report {
            case: 12,
            subject: "<b>x</b>",
            reason: Some("اسپم"),
            reports: 3,
        };
        let lines = report.lines();
        assert!(lines.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(lines.contains("دلیل · <b>اسپم</b>"));
        assert!(lines.contains("گزارش ها · <b>3</b>"));
        let single = Report {
            reports: 1,
            reason: None,
            ..report
        };
        assert!(!single.lines().contains("گزارش ها"));
        assert!(!single.lines().contains("دلیل"));
    }

    #[test]
    fn a_crowded_card_is_edited_in_steps() {
        let edits = (2..=REPORTERS_MAX).filter(|count| worth_showing(*count)).count();
        assert_eq!(edits, 4 + (REPORTERS_MAX / 5 - 1));
        assert!(worth_showing(3) && !worth_showing(7) && worth_showing(10));
    }

    #[test]
    fn every_button_fits_in_a_callback_payload() {
        for action in ["d", "k", "w", "m", "b"] {
            assert!(Verdict::of(action).is_some());
            assert!(format!("mc:{action}:{}", i64::MIN).len() <= 64);
        }
        assert!(Verdict::of("u").is_none());
    }
}
