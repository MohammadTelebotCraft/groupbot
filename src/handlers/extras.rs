use grammers_client::message::Message;
use grammers_client::tl;

use crate::response::ResponseKind;
use crate::state::{DefaultRightsError, NightWindow};

use super::{Ctx, esc, name_of};

pub const RULES: &str = "rules";

pub const SHOW_RULES: &[&str] = &["قوانین", "قانون"];
pub const SET_RULES: &[&str] = &["تنظیم قوانین", "تنظیم قانون"];
pub const NOTE_CMD: &[&str] = &["یادداشت"];
pub const NOTE_SET: &[&str] = &["تنظیم یادداشت", "ثبت یادداشت"];
pub const NOTE_CLEAR: &[&str] = &["حذف یادداشت"];
pub const PIN: &[&str] = &["سنجاق", "پین"];
pub const PIN_QUIET: &[&str] = &["سنجاق بی صدا", "پین بی صدا"];
pub const UNPIN: &[&str] = &["حذف سنجاق", "حذف پین", "برداشتن سنجاق"];
pub const SLOW: &[&str] = &["اسلوموشن", "اسلومود", "کندی"];
pub const NIGHT_CMD: &[&str] = &["قفل شب"];
pub const TAG_ALL: &[&str] = &["تگ همه", "منشن همه", "فراخوان", "تگ", "منشن"];
pub const TAG_STOP: &[&str] = &["توقف", "استاپ"];

const TAG_SEPARATOR: &str = " ⊹ ";

const TAG_PER_MESSAGE: usize = 6;
const TAG_BATCH_RANGE: (usize, usize) = (1, 50);

const TAG_MAX_MEMBERS: usize = 10_000;

const TAG_MAX_WALK: usize = TAG_MAX_MEMBERS * 2;

const TAG_PACE: std::time::Duration = std::time::Duration::from_secs(4);

const TAG_FLOOD_MAX: std::time::Duration = std::time::Duration::from_secs(300);

const TAG_ROOM: usize = 3_500;

pub const SLOW_STEPS: &[u32] = &[0, 10, 30, 60, 300, 900, 3600];

pub const SLOW_STATE: &str = "slow_state";

pub fn slow_label(seconds: u32) -> String {
    match seconds {
        0 => "خاموش".to_owned(),
        s if s < 60 => format!("{s} ثانیه"),
        s if s < 3600 => format!("{} دقیقه", s / 60),
        s => format!("{} ساعت", s / 3600),
    }
}

pub async fn apply_slow(
    ctx: &Ctx,
    chat: i64,
    seconds: u32,
) -> Result<Option<bool>, crate::state::SettingsWriteError> {
    let seconds = SLOW_STEPS
        .iter()
        .rev()
        .find(|step| **step <= seconds)
        .copied()
        .unwrap_or(0);
    let Some(done) = super::cleaner::set_slow(ctx, chat, seconds).await else {
        return Ok(None);
    };
    if done {
        ctx.settings
            .try_set_value(chat, SLOW_STATE, &seconds.to_string())
            .await?;
    }
    Ok(Some(done))
}

pub fn rules(ctx: &Ctx, chat: i64) -> Option<String> {
    ctx.settings.value(chat, RULES).filter(|r| !r.is_empty())
}

pub async fn read_note(ctx: &Ctx, chat: i64, user: i64) -> Result<Option<String>, sqlx::Error> {
    ctx.settings
        .note(chat, user)
        .await
        .map(|note| note.filter(|value| !value.is_empty()))
}

pub async fn night(ctx: &Ctx, chat: i64) -> Result<Option<(u32, u32)>, DefaultRightsError> {
    Ok(ctx.settings.default_rights(chat).await?.and_then(|state| {
        state
            .night
            .map(|window| (u32::from(window.from), u32::from(window.to)))
    }))
}

#[derive(Debug)]
pub enum NightUpdateError {
    State(DefaultRightsError),
    InvalidWindow,
}

impl std::fmt::Display for NightUpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State(error) => error.fmt(formatter),
            Self::InvalidWindow => write!(formatter, "night window endpoints must differ"),
        }
    }
}

impl std::error::Error for NightUpdateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            Self::InvalidWindow => None,
        }
    }
}

impl From<DefaultRightsError> for NightUpdateError {
    fn from(error: DefaultRightsError) -> Self {
        Self::State(error)
    }
}

impl NightUpdateError {
    pub fn acceptance_unknown(&self) -> bool {
        matches!(self, Self::State(error) if error.acceptance_unknown())
    }
}

pub async fn set_night(
    ctx: &Ctx,
    chat: i64,
    window: Option<(u32, u32)>,
) -> Result<super::rights::DeliveryOutcome, NightUpdateError> {
    let window = window
        .map(|(from, to)| NightWindow::new(from, to).ok_or(NightUpdateError::InvalidWindow))
        .transpose()?;
    Ok(super::rights::set_night(ctx, ctx.chat_ref(chat), chat, window).await?)
}

pub fn clock(minutes: u32) -> String {
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
}

pub async fn handle(
    ctx: &std::sync::Arc<Ctx>,
    message: &Message,
    view: &super::locks::View<'_>,
) -> bool {
    let text = view.digits();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if SHOW_RULES.contains(&text) {
        match rules(ctx, chat) {
            Some(rules) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::UtilityResult,
                    super::premium::icon_html(
                        Some(super::premium::Icon::File),
                        format!("<b>قوانین گروه</b>\n\n{rules}"),
                    ),
                )
                .await
            }
            None => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    "قوانینی ثبت نشده است.",
                )
                .await
            }
        };
        return true;
    }

    let admin = |cap| super::limits::allows(ctx, message, cap);

    if TAG_STOP.contains(&text) {
        let Some(state) = ctx.peek(chat).filter(|state| state.tagging_now()) else {
            return false;
        };
        if !admin(super::limits::SET).await {
            return true;
        }
        state.stop_tagging();
        super::respond(
            ctx,
            message,
            ResponseKind::UtilityResult,
            super::premium::icon_text(Some(super::premium::Icon::Pause), "تگ متوقف شد."),
        )
        .await;
        return true;
    }

    if let Some(rest) = after(text, TAG_ALL) {
        let Some(per_message) = batch_size(rest) else {
            return false;
        };
        if !admin(super::limits::SET).await {
            return true;
        }
        tag_all(ctx, message, chat, per_message).await;
        return true;
    }

    if let Some(rest) = after(text, SET_RULES) {
        if !admin(super::limits::SET).await {
            return true;
        }
        let body = if rest.is_empty() {
            message
                .get_reply()
                .await
                .ok()
                .flatten()
                .map(|replied| replied.text().to_owned())
                .unwrap_or_default()
        } else {
            rest.to_owned()
        };
        if body.is_empty() {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "متن قوانین را بنویسید یا روی آن ریپلای کنید.",
            )
            .await;
            return true;
        }
        match ctx.settings.try_set_value(chat, RULES, &body).await {
            Ok(_) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::SettingsChanged,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Success),
                        "قوانین ذخیره شد.",
                    ),
                )
                .await
            }
            Err(error) => {
                ::log::warn!("rules: write for {chat} failed: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    "قوانین ذخیره نشد؛ دوباره تلاش کنید.",
                )
                .await
            }
        }
        return true;
    }

    if let Some(rest) = after(text, NOTE_CLEAR) {
        let Some(named) = super::named(message, none_if_empty(rest)) else {
            return false;
        };
        if !admin(super::limits::SET).await {
            return true;
        }
        let Some((target, name)) = super::resolve(ctx, message, named).await else {
            return true;
        };
        let removed = if let Some(user) = target.id.bare_id() {
            ctx.settings.set_note(chat, user, "").await
        } else {
            return true;
        };
        match removed {
            Ok(()) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::SettingsChanged,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Delete),
                        format!("یادداشت {name} حذف شد."),
                    ),
                )
                .await
            }
            Err(error) => {
                ::log::warn!("notes: delete for {chat} failed: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    "یادداشت حذف نشد؛ دوباره تلاش کنید.",
                )
                .await
            }
        }
        return true;
    }

    let written = after(text, NOTE_SET);
    if let Some(rest) = written.or_else(|| after(text, NOTE_CMD)) {
        if written.is_none() && !rest.is_empty() {
            return false;
        }
        let Some(named) = super::named(message, None) else {
            return false;
        };
        if !admin(super::limits::SET).await {
            return true;
        }
        let Some((target, name)) = super::resolve(ctx, message, named).await else {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "روی پیام کاربر ریپلای کنید.",
            )
            .await;
            return true;
        };
        let Some(user) = target.id.bare_id() else {
            return true;
        };
        if written.is_some() && rest.is_empty() {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "متن یادداشت را بعد از دستور بنویسید.",
            )
            .await;
            return true;
        }
        if rest.is_empty() {
            match read_note(ctx, chat, user).await {
                Ok(Some(note)) => {
                    super::respond(
                        ctx,
                        message,
                        ResponseKind::PersonalInformation,
                        super::premium::html(format!(
                            "<b>یادداشت {}</b>\n\n{}",
                            esc(&name),
                            esc(&note)
                        )),
                    )
                    .await
                }
                Ok(None) => {
                    super::respond(
                        ctx,
                        message,
                        ResponseKind::PersonalInformation,
                        "یادداشتی ثبت نشده است.",
                    )
                    .await
                }
                Err(error) => {
                    ::log::warn!("notes: read for {chat}/{user} failed: {error}");
                    super::respond(
                        ctx,
                        message,
                        ResponseKind::CommandError,
                        "یادداشت خوانده نشد؛ دوباره تلاش کنید.",
                    )
                    .await
                }
            }
            return true;
        }
        match ctx.settings.set_note(chat, user, rest).await {
            Ok(()) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::SettingsChanged,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Success),
                        format!("یادداشت برای {name} ذخیره شد."),
                    ),
                )
                .await
            }
            Err(error) => {
                ::log::warn!("notes: write for {chat}/{user} failed: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    "یادداشت ذخیره نشد؛ دوباره تلاش کنید.",
                )
                .await
            }
        }
        return true;
    }

    if PIN.contains(&text) || PIN_QUIET.contains(&text) || UNPIN.contains(&text) {
        if message.reply_to_message_id().is_none() {
            return false;
        }
        if !admin(super::limits::PIN).await {
            return true;
        }
        return pin(
            ctx,
            message,
            chat,
            UNPIN.contains(&text),
            PIN_QUIET.contains(&text),
        )
        .await;
    }

    if let Some(rest) = after(text, SLOW) {
        let asked = match super::numbers_in(rest).as_deref() {
            Some(&[asked]) => asked,
            _ => return false,
        };
        if !admin(super::limits::SET).await {
            return true;
        }
        return slow_mode(ctx, message, chat, asked).await;
    }

    if let Some(rest) = after(text, NIGHT_CMD) {
        return set_night_from(ctx, message, chat, rest).await;
    }
    false
}

async fn set_night_from(ctx: &Ctx, message: &Message, chat: i64, rest: &str) -> bool {
    if rest.is_empty() {
        return false;
    }
    if rest == "خاموش" || rest.starts_with("خاموش ") {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        if let Err(error) = super::rights::seed(ctx, message, chat).await {
            log::warn!("night: could not seed default rights for {chat}: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "اختیارات گروه خوانده نشد؛ دوباره تلاش کنید.",
            )
            .await;
            return true;
        }
        let result = set_night(ctx, chat, None).await;
        match result {
            Ok(super::rights::DeliveryOutcome::Applied) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::SettingsChanged,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Unlocked),
                        "قفل شب خاموش شد.",
                    ),
                )
                .await
            }
            Ok(
                super::rights::DeliveryOutcome::PendingRetry { .. }
                | super::rights::DeliveryOutcome::Superseded,
            ) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::SettingsChanged,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Timer),
                        "قفل شب خاموش شد و بازکردن گروه در انتظار تلاش دوباره است.",
                    ),
                )
                .await
            }
            Ok(super::rights::DeliveryOutcome::AcceptedDeliveryUnknown { reason }) => {
                log::warn!("night: accepted disable for {chat} has unknown delivery: {reason}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::SettingsChanged,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Timer),
                        "قفل شب خاموش شد، اما وضعیت بازشدن گروه در تلگرام مشخص نیست.",
                    ),
                )
                .await
            }
            Err(error) if error.acceptance_unknown() => {
                log::warn!("night: schedule disable outcome unknown for {chat}: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Timer),
                        "وضعیت ذخیره سازی مشخص نیست؛ پنل را دوباره بررسی کنید.",
                    ),
                )
                .await
            }
            Err(error) => {
                log::warn!("night: could not disable schedule for {chat}: {error}");
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    super::premium::icon_text(
                        Some(super::premium::Icon::ErrorRed),
                        "قفل شب خاموش نشد؛ دوباره تلاش کنید.",
                    ),
                )
                .await
            }
        }
        return true;
    }

    const SEPARATOR: &str = "تا";
    let mut times: Vec<u32> = Vec::new();
    for word in super::digits(rest).split_whitespace() {
        if word == SEPARATOR {
            continue;
        }
        let Some(minutes) = clock_at(word) else {
            return false;
        };
        times.push(minutes);
    }
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    if let Err(error) = super::rights::seed(ctx, message, chat).await {
        log::warn!("night: could not seed default rights for {chat}: {error}");
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "اختیارات گروه خوانده نشد؛ دوباره تلاش کنید.",
        )
        .await;
        return true;
    }
    let [from, to] = times[..] else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "مثال: «قفل شب 23 تا 7» یا «قفل شب 23:30 تا 7:15»",
        )
        .await;
        return true;
    };
    if from == to {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "شروع و پایان یکی است. مثال: «قفل شب 23 تا 7»",
        )
        .await;
        return true;
    }
    let times = [from, to];
    let result = set_night(ctx, chat, Some((times[0], times[1]))).await;
    match result {
        Ok(super::rights::DeliveryOutcome::Applied) => {
            super::respond(
                ctx,
                message,
                ResponseKind::SettingsChanged,
                super::premium::icon_text(
                    Some(super::premium::Icon::Night),
                    format!(
                        "قفل شب از {} تا {} (به وقت تهران).",
                        clock(times[0]),
                        clock(times[1])
                    ),
                ),
            )
            .await
        }
        Ok(
            super::rights::DeliveryOutcome::PendingRetry { .. }
            | super::rights::DeliveryOutcome::Superseded,
        ) => {
            super::respond(
                ctx,
                message,
                ResponseKind::SettingsChanged,
                super::premium::icon_text(
                    Some(super::premium::Icon::Timer),
                    "زمان قفل شب ذخیره شد و تحویل آن به تلگرام دوباره تلاش می شود.",
                ),
            )
            .await
        }
        Ok(super::rights::DeliveryOutcome::AcceptedDeliveryUnknown { reason }) => {
            log::warn!("night: accepted schedule for {chat} has unknown delivery: {reason}");
            super::respond(
                ctx,
                message,
                ResponseKind::SettingsChanged,
                super::premium::icon_text(
                    Some(super::premium::Icon::Timer),
                    "زمان قفل شب ذخیره شد، اما وضعیت تحویل آن به تلگرام مشخص نیست.",
                ),
            )
            .await
        }
        Err(error) if error.acceptance_unknown() => {
            log::warn!("night: schedule write outcome unknown for {chat}: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                super::premium::icon_text(
                    Some(super::premium::Icon::Timer),
                    "وضعیت ذخیره سازی مشخص نیست؛ پنل را دوباره بررسی کنید.",
                ),
            )
            .await
        }
        Err(error) => {
            log::warn!("night: could not save schedule for {chat}: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    "قفل شب ذخیره نشد؛ دوباره تلاش کنید.",
                ),
            )
            .await
        }
    }
    true
}

async fn slow_mode(ctx: &Ctx, message: &Message, chat: i64, asked: u32) -> bool {
    let done = match apply_slow(ctx, chat, asked).await {
        Ok(done) => done,
        Err(error) => {
            log::warn!("slow mode: state write for {chat} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                if error.commit_outcome_unknown() {
                    "اسلوموشن در تلگرام تغییر کرد، اما نتیجه ثبت آن نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "اسلوموشن در تلگرام تغییر کرد، اما تنظیم آن ذخیره نشد؛ دوباره تلاش کنید."
                },
            )
            .await;
            return true;
        }
    };
    let now = ctx
        .settings
        .value(chat, SLOW_STATE)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    match (done, now) {
        (Some(true), 0) => {
            super::respond(
                ctx,
                message,
                ResponseKind::SettingsChanged,
                super::premium::icon_text(Some(super::premium::Icon::Timer), "اسلوموشن خاموش شد."),
            )
            .await
        }
        (Some(true), _) => {
            super::respond(
                ctx,
                message,
                ResponseKind::SettingsChanged,
                super::premium::icon_text(
                    Some(super::premium::Icon::Timer),
                    format!("اسلوموشن روی {} تنظیم شد.", slow_label(now)),
                ),
            )
            .await
        }
        (Some(false), _) => {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    "انجام نشد. مطمئن شوید کلینر در گروه ادمین است و اجازه تغییر اطلاعات دارد.",
                ),
            )
            .await
        }
        (None, _) => super::respond(
            ctx,
            message,
            ResponseKind::PermissionDenied,
            "اسلوموشن را فقط کلینر می تواند تنظیم کند؛ ربات ها به این بخش تلگرام دسترسی ندارند.\n\
                     «افزودن کلینر» را بفرستید.",
        )
        .await,
    }
    true
}

async fn pin(ctx: &Ctx, message: &Message, chat: i64, unpin: bool, quiet: bool) -> bool {
    let (Ok(Some(replied)), Ok(Some(chat_ref))) =
        (message.get_reply().await, message.peer_ref().await)
    else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "روی پیام موردنظر ریپلای کنید.",
        )
        .await;
        return true;
    };
    let result = ctx
        .client
        .invoke_outbound(&tl::functions::messages::UpdatePinnedMessage {
            silent: quiet,
            unpin,
            pm_oneside: false,
            peer: chat_ref.into(),
            id: replied.id(),
        })
        .await;
    match result {
        Ok(_) if unpin => {
            super::respond(
                ctx,
                message,
                ResponseKind::ModerationConfirmation,
                super::premium::icon_text(Some(super::premium::Icon::Pin), "سنجاق برداشته شد."),
            )
            .await
        }
        Ok(_) => {
            super::respond(
                ctx,
                message,
                ResponseKind::ModerationConfirmation,
                super::premium::icon_text(
                    Some(super::premium::Icon::Pin),
                    format!("پیام سنجاق شد. توسط {}", name_of(message)),
                ),
            )
            .await
        }
        Err(e) => {
            eprintln!("pin: {chat}: {e}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    "انجام نشد. مطمئن شوید ربات اجازه سنجاق کردن دارد.",
                ),
            )
            .await
        }
    }
    true
}

fn batch_size(tail: &str) -> Option<usize> {
    match super::numbers_in(tail).as_deref() {
        Some([]) => Some(TAG_PER_MESSAGE),
        Some([size]) => Some((*size as usize).clamp(TAG_BATCH_RANGE.0, TAG_BATCH_RANGE.1)),
        _ => None,
    }
}

fn mention_of(
    participant: &grammers_client::peer::Participant,
    caller: Option<i64>,
) -> Option<(String, usize)> {
    let expanded = participant.user()?;
    let user = expanded.id().bare_id()?;
    if expanded.is_bot() || Some(user) == caller {
        return None;
    }
    let name = expanded.full_name();
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((
        format!("<a href=\"tg://user?id={user}\">{}</a>", esc(name)),
        width_of(name),
    ))
}

fn width_of(text: &str) -> usize {
    text.encode_utf16().count()
}

fn fits(body: usize, width: usize) -> bool {
    body == 0 || body + width_of(TAG_SEPARATOR) + width <= TAG_ROOM
}

struct TagRun {
    state: std::sync::Arc<super::ChatState>,
    token: u64,
}

impl TagRun {
    fn live(&self) -> bool {
        self.state.tagging(self.token)
    }

    fn finish(&self) -> bool {
        self.state.finish_tagging(self.token)
    }
}

impl Drop for TagRun {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

enum Ending {
    Exhausted,
    Ceiling,
    Failed,
    Stopped,
}

async fn tag_all(ctx: &std::sync::Arc<Ctx>, message: &Message, chat: i64, per_message: usize) {
    let chat_ref = match ctx.chat_ref(chat) {
        Some(chat_ref) => chat_ref,
        None => match message.peer_ref().await {
            Ok(Some(chat_ref)) => chat_ref,
            _ => return,
        },
    };
    let caller = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id);

    let anchor = message
        .reply_to_message_id()
        .unwrap_or_else(|| message.id());

    let Some(permit) = ctx.tag_slot() else {
        super::respond(
            ctx,
            message,
            ResponseKind::UtilityResult,
            super::premium::icon_text(
                Some(super::premium::Icon::Timer),
                "چند گروه دیگر همین حالا در حال تگ اند. کمی بعد دوباره بفرستید.",
            ),
        )
        .await;
        return;
    };

    let Some(state) = ctx.peek(chat) else {
        return;
    };
    let token = state.claim_tagging(message.id());
    let run = TagRun { state, token };
    let ctx = std::sync::Arc::clone(ctx);

    std::sync::Arc::clone(&ctx).spawn_owned(async move {
        let _permit = permit;
        let mut participants = ctx.client.iter_participants(chat_ref);
        let mut batch: Vec<String> = Vec::with_capacity(per_message);
        let mut body = 0usize;
        let mut tagged = 0usize;
        let mut seen: std::collections::HashSet<i64> =
            std::collections::HashSet::with_capacity(per_message * 2);
        let mut ending = Ending::Exhausted;

        loop {
            if !run.live() {
                ending = Ending::Stopped;
                break;
            }
            let next = participants.next().await;
            if let Err(e) = &next {
                eprintln!("tag: {chat}: {e}");
                ending = Ending::Failed;
            }
            let done = matches!(next, Ok(None) | Err(_));
            let mut carried = None;
            if let Ok(Some(participant)) = next
                && participant.id().bare_id().is_some_and(|id| seen.insert(id))
                && let Some((mention, width)) = mention_of(&participant, caller)
            {
                if fits(body, width) {
                    body += if body == 0 {
                        width
                    } else {
                        width_of(TAG_SEPARATOR) + width
                    };
                    batch.push(mention);
                } else {
                    carried = Some((mention, width));
                }
            }

            let full = batch.len() >= per_message || carried.is_some();
            if batch.is_empty() || (!full && !done) {
                if done {
                    break;
                }
                continue;
            }

            let body_text = batch.join(TAG_SEPARATOR);
            let count = batch.len();
            batch.clear();
            body = 0;
            if let Some((mention, width)) = carried {
                body = width;
                batch.push(mention);
            }

            match send_batch(&ctx, chat_ref, &body_text, anchor).await {
                Ok(()) => tagged += count,
                Err(e) => {
                    eprintln!("tag: {chat}: {e}");
                    ending = Ending::Failed;
                    break;
                }
            }
            if done && batch.is_empty() {
                break;
            }
            if tagged >= TAG_MAX_MEMBERS || seen.len() >= TAG_MAX_WALK {
                ending = Ending::Ceiling;
                break;
            }

            tokio::time::sleep(TAG_PACE).await;
        }

        if !run.finish() {
            ending = Ending::Stopped;
        }
        let closing = match ending {
            Ending::Stopped => return,
            Ending::Exhausted if tagged == 0 => "کسی برای تگ کردن پیدا نشد.".to_owned(),
            Ending::Exhausted => format!("✓ {tagged} نفر تگ شدند."),
            Ending::Ceiling => format!("✓ {tagged} نفر تگ شدند. سقف یک تگ همین است."),
            Ending::Failed => {
                format!("✗ تگ نیمه کاره ماند. تا اینجا {tagged} نفر صدا زده شدند.")
            }
        };
        let _ = ctx
            .client
            .send_message(
                chat_ref,
                super::premium::html(closing).reply_to(Some(anchor)),
            )
            .await;
    });
}

async fn send_batch(
    ctx: &Ctx,
    chat_ref: grammers_client::session::types::PeerRef,
    body: &str,
    anchor: i32,
) -> Result<(), grammers_client::InvocationError> {
    let one = || {
        ctx.client
            .send_message(chat_ref, super::premium::html(body).reply_to(Some(anchor)))
    };
    let Err(e) = one().await else {
        return Ok(());
    };
    let grammers_client::InvocationError::Rpc(rpc) = &e else {
        return Err(e);
    };
    if !matches!(
        rpc.name.as_str(),
        "FLOOD_WAIT" | "FLOOD_PREMIUM_WAIT" | "SLOWMODE_WAIT"
    ) {
        return Err(e);
    }
    let Some(seconds) = rpc.value else {
        return Err(e);
    };
    if u64::from(seconds) > TAG_FLOOD_MAX.as_secs() {
        return Err(e);
    }
    tokio::time::sleep(std::time::Duration::from_secs(u64::from(seconds))).await;
    one().await.map(|_| ())
}

fn after<'a>(text: &'a str, commands: &[&str]) -> Option<&'a str> {
    commands.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
    })
}

fn clock_at(word: &str) -> Option<u32> {
    let (hours, minutes) = match word.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (word.parse::<u32>().ok()?, 0),
    };
    Some((hours % 24) * 60 + minutes.min(59))
}

fn none_if_empty(text: &str) -> Option<&str> {
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_tag_carries_the_default_batch() {
        assert_eq!(batch_size(""), Some(TAG_PER_MESSAGE));
        for command in TAG_ALL {
            let rest = after(command, TAG_ALL).expect("the alias matches itself");
            assert_eq!(
                batch_size(rest),
                Some(TAG_PER_MESSAGE),
                "«{command}» on its own must mean the default batch"
            );
        }
    }

    #[test]
    fn the_batch_size_must_be_the_whole_tail() {
        assert_eq!(batch_size("30"), Some(30));
        assert_eq!(
            batch_size("۳۰"),
            Some(30),
            "the tail is read through the digit fold"
        );

        for tail in ["چیه", "کن", "30 بیاید", "بیاید 30", "همه چیه", "-1"] {
            assert_eq!(batch_size(tail), None, "«{tail}» is not a count");
        }
    }

    #[test]
    fn a_batch_is_clamped_to_something_one_message_can_hold() {
        assert_eq!(batch_size("0"), Some(TAG_BATCH_RANGE.0));
        assert_eq!(batch_size("100000"), Some(TAG_BATCH_RANGE.1));
        assert!(TAG_BATCH_RANGE.0 >= 1 && TAG_BATCH_RANGE.1 <= 100);
    }

    #[test]
    fn a_batch_stops_at_the_message_ceiling() {
        assert!(fits(0, TAG_ROOM * 2), "the first mention always goes in");
        assert!(fits(TAG_ROOM - 20, 10));
        assert!(!fits(TAG_ROOM - 20, 20));

        assert_eq!(TAG_SEPARATOR, " \u{22b9} ");
        assert!(!TAG_SEPARATOR.contains('\u{200c}'));
    }

    #[test]
    fn a_stop_only_answers_while_a_run_is_going() {
        let state = super::super::ChatState::default();
        assert!(!state.tagging_now(), "nothing to stop, so nothing to say");

        let first = state.claim_tagging(10);
        assert!(state.tagging_now());
        assert!(state.tagging(first));

        let second = state.claim_tagging(11);
        assert!(!state.tagging(first), "the older run is superseded");
        assert!(state.tagging(second));

        assert!(
            !state.finish_tagging(first),
            "a superseded run must not end its successor, and must be told it did not"
        );
        assert!(state.tagging(second));

        state.stop_tagging();
        assert!(!state.tagging_now());
        assert!(!state.tagging(second));
        assert!(
            !state.finish_tagging(second),
            "a stop that beat the last batch home owns the reply, not the run"
        );

        let third = state.claim_tagging(12);
        assert!(state.finish_tagging(third));
        assert!(
            !state.tagging_now(),
            "a run that ends leaves nothing behind"
        );
    }

    #[test]
    fn the_newer_tag_wins_however_the_two_are_scheduled() {
        let state = super::super::ChatState::default();
        let newer = state.claim_tagging(101);
        let older = state.claim_tagging(100);
        assert!(state.tagging(newer), "the later message owns the run");
        assert!(!state.tagging(older));

        let state = super::super::ChatState::default();
        let older = state.claim_tagging(100);
        let newer = state.claim_tagging(101);
        assert!(state.tagging(newer));
        assert!(!state.tagging(older));
    }

    #[test]
    fn the_ceiling_is_measured_the_way_telegram_measures_it() {
        assert_eq!(width_of("ab"), 2);
        assert_eq!(width_of("سلام"), 4);
        assert_eq!(width_of("🙂"), 2, "one char, two units");

        let emoji_name = "🙂".repeat(64);
        assert_eq!(emoji_name.chars().count(), 64);
        assert_eq!(width_of(&emoji_name), 128);

        let width = width_of(&emoji_name);
        let mut body = width;
        let mut fitted = 1;
        while fits(body, width) {
            body += width_of(TAG_SEPARATOR) + width;
            fitted += 1;
        }
        assert!(fitted < 50, "the ceiling must bind before the batch count");
        assert!(body <= TAG_ROOM);
    }
}
