use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use grammers_client::message::{Button, Message, ReplyMarkup};
use grammers_client::session::types::{PeerKind, PeerRef};
use grammers_client::tl;

use crate::response::ResponseKind;

use super::restrict::{self, Action};
use super::{Ctx, esc, filters, imgfilter, join, packs, vip};

#[derive(Debug)]
pub enum MutationError {
    Settings(crate::state::SettingsWriteError),
    Database(sqlx::Error),
    Image(imgfilter::DeleteError),
    Restriction(restrict::Failed),
    Busy,
    Interrupted,
}

impl MutationError {
    pub fn commit_outcome_unknown(&self) -> bool {
        match self {
            Self::Settings(error) => error.commit_outcome_unknown(),
            Self::Image(imgfilter::DeleteError::Setting(error)) => error.commit_outcome_unknown(),
            Self::Database(_)
            | Self::Image(_)
            | Self::Restriction(_)
            | Self::Busy
            | Self::Interrupted => false,
        }
    }

    pub fn told(&self) -> &'static str {
        match self {
            Self::Busy => "پاکسازی لیست بن یا سکوت این گروه همین حالا در جریان است.",
            _ => "پاکسازی کامل نشد؛ دوباره تلاش کنید.",
        }
    }
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Settings(error) => error.fmt(formatter),
            Self::Database(error) => error.fmt(formatter),
            Self::Image(error) => error.fmt(formatter),
            Self::Restriction(error) => error.fmt(formatter),
            Self::Busy => formatter.write_str("a member-list clear is already running"),
            Self::Interrupted => formatter.write_str("the clear stopped before it answered"),
        }
    }
}

impl std::error::Error for MutationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Settings(error) => Some(error),
            Self::Database(error) => Some(error),
            Self::Image(error) => Some(error),
            Self::Restriction(_) | Self::Busy | Self::Interrupted => None,
        }
    }
}

pub enum Cleared {
    Done(usize),
    Continuing(usize),
}

impl From<crate::state::SettingsWriteError> for MutationError {
    fn from(error: crate::state::SettingsWriteError) -> Self {
        Self::Settings(error)
    }
}

impl From<imgfilter::DeleteError> for MutationError {
    fn from(error: imgfilter::DeleteError) -> Self {
        Self::Image(error)
    }
}

impl From<sqlx::Error> for MutationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

pub const SHOW: &[(&str, Kind)] = &[
    ("لیست بن شده ها", Kind::Ban),
    ("فهرست بن شده ها", Kind::Ban),
    ("لیست سکوت شده ها", Kind::Mute),
    ("فهرست سکوت شده ها", Kind::Mute),
    ("لیست کاربران ویژه", Kind::Vip),
    ("فهرست کاربران ویژه", Kind::Vip),
    ("لیست کلمات فیلتر", Kind::Filter),
    ("فهرست کلمات فیلتر", Kind::Filter),
    ("لیست فیلتر عکس", Kind::Image),
    ("فهرست فیلتر عکس", Kind::Image),
    ("لیست پاسخ خودکار", Kind::Answer),
    ("فهرست پاسخ خودکار", Kind::Answer),
    ("لیست معافیت ها", Kind::Exempt),
    ("فهرست معافیت ها", Kind::Exempt),
    ("لیست دستورهای سفارشی", Kind::Command),
    ("فهرست دستورهای سفارشی", Kind::Command),
    ("لیست پک ها", Kind::Pack),
    ("فهرست پک ها", Kind::Pack),
    ("لیست بن", Kind::Ban),
    ("لیست سیک", Kind::Ban),
    ("لیست سکوت", Kind::Mute),
    ("لیست خفه", Kind::Mute),
    ("لیست ویژه", Kind::Vip),
    ("لیست فیلتر", Kind::Filter),
    ("لیست فیلتر تصویری", Kind::Image),
    ("لیست پاسخ", Kind::Answer),
    ("لیست معاف", Kind::Exempt),
    ("لیست دستور", Kind::Command),
    ("لیست پک", Kind::Pack),
    ("لیست استیکرپک", Kind::Pack),
    ("/banlist", Kind::Ban),
    ("/mutelist", Kind::Mute),
    ("/viplist", Kind::Vip),
    ("/filterlist", Kind::Filter),
    ("/imagefilterlist", Kind::Image),
    ("/answerlist", Kind::Answer),
    ("/exemptlist", Kind::Exempt),
    ("/commandlist", Kind::Command),
    ("/packlist", Kind::Pack),
];

pub const CLEAR: &[(&str, Kind)] = &[
    ("پاکسازی لیست بن شده ها", Kind::Ban),
    ("خالی کردن لیست بن شده ها", Kind::Ban),
    ("پاکسازی لیست سکوت شده ها", Kind::Mute),
    ("خالی کردن لیست سکوت شده ها", Kind::Mute),
    ("پاکسازی لیست کاربران ویژه", Kind::Vip),
    ("خالی کردن لیست کاربران ویژه", Kind::Vip),
    ("پاکسازی لیست کلمات فیلتر", Kind::Filter),
    ("خالی کردن لیست کلمات فیلتر", Kind::Filter),
    ("پاکسازی لیست فیلتر عکس", Kind::Image),
    ("خالی کردن لیست فیلتر عکس", Kind::Image),
    ("پاکسازی لیست پاسخ خودکار", Kind::Answer),
    ("خالی کردن لیست پاسخ خودکار", Kind::Answer),
    ("پاکسازی لیست معافیت ها", Kind::Exempt),
    ("خالی کردن لیست معافیت ها", Kind::Exempt),
    ("پاکسازی لیست دستورهای سفارشی", Kind::Command),
    ("خالی کردن لیست دستورهای سفارشی", Kind::Command),
    ("پاکسازی لیست پک ها", Kind::Pack),
    ("خالی کردن لیست پک ها", Kind::Pack),
    ("پاکسازی لیست بن", Kind::Ban),
    ("پاکسازی لیست سیک", Kind::Ban),
    ("پاکسازی لیست سکوت", Kind::Mute),
    ("پاکسازی لیست خفه", Kind::Mute),
    ("پاکسازی لیست ویژه", Kind::Vip),
    ("پاکسازی لیست فیلتر", Kind::Filter),
    ("پاکسازی لیست فیلتر تصویری", Kind::Image),
    ("پاکسازی لیست پاسخ", Kind::Answer),
    ("پاکسازی لیست معاف", Kind::Exempt),
    ("پاکسازی لیست دستور", Kind::Command),
    ("پاکسازی لیست پک", Kind::Pack),
    ("پاکسازی لیست استیکرپک", Kind::Pack),
    ("/clearbanlist", Kind::Ban),
    ("/clearmutelist", Kind::Mute),
    ("/clearviplist", Kind::Vip),
    ("/clearfilterlist", Kind::Filter),
    ("/clearimagefilterlist", Kind::Image),
    ("/clearanswerlist", Kind::Answer),
    ("/clearexemptlist", Kind::Exempt),
    ("/clearcommandlist", Kind::Command),
    ("/clearpacklist", Kind::Pack),
];

pub async fn command(ctx: &Arc<Ctx>, message: &Message) -> bool {
    let text = message.text().trim();
    let show = SHOW
        .iter()
        .find(|(name, _)| super::commands::matches(text, name));
    let clear = CLEAR
        .iter()
        .find(|(name, _)| super::commands::matches(text, name));
    let Some(&(_, kind)) = show.or(clear) else {
        return false;
    };
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !super::limits::allows(ctx, message, kind.cap()).await {
        return true;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    if clear.is_some() {
        match clear_all(ctx, chat_ref, chat, kind).await {
            Ok(Cleared::Done(removed)) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::AdminTool,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Delete),
                        format!("{removed} مورد از {} حذف شد.", kind.title()),
                    ),
                )
                .await
            }
            Ok(Cleared::Continuing(removed)) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::AdminTool,
                    super::premium::icon_html(
                        Some(super::premium::Icon::Hourglass),
                        continuing(kind, removed),
                    ),
                )
                .await
            }
            Err(error) => {
                ::log::warn!("lists: clear {} for {chat} failed: {error}", kind.title());
                super::respond(ctx, message, ResponseKind::CommandError, error.told()).await
            }
        }
        return true;
    }

    let Some(opener) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };
    let (title, markup) = match view(ctx, chat_ref, chat, kind, opener).await {
        Ok(page) => page,
        Err(error) => {
            ::log::warn!("lists: could not read {} for {chat}: {error}", kind.title());
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "لیست فعلاً در دسترس نیست؛ دوباره تلاش کنید.",
            )
            .await;
            return true;
        }
    };
    super::respond_shared(
        ctx,
        message,
        ResponseKind::AdminTool,
        super::premium::html(title).reply_markup(markup),
    )
    .await;
    true
}

pub async fn clear_all(
    ctx: &Arc<Ctx>,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
) -> Result<Cleared, MutationError> {
    match kind {
        Kind::Ban | Kind::Mute => clear_members(ctx, chat_ref, chat, kind).await,
        Kind::Image => {
            let filters = imgfilter::listing(ctx, chat).await?;
            for (name, _) in &filters {
                imgfilter::try_forget(ctx, chat, name).await?;
            }
            Ok(Cleared::Done(filters.len()))
        }
        Kind::Vip | Kind::Filter | Kind::Answer | Kind::Exempt | Kind::Command | Kind::Pack => {
            let keys = setting_keys(ctx, chat, kind);
            let mutations: Vec<_> = keys
                .iter()
                .map(|key| crate::state::SettingMutation::Delete { key })
                .collect();
            Ok(Cleared::Done(
                ctx.settings.try_apply_batch(chat, &mutations).await?,
            ))
        }
    }
}

fn continuing(kind: Kind, removed: usize) -> String {
    format!(
        "<b>پاکسازی {}</b>\n\n\
         حذف شده تا اینجا · <b>{removed}</b>\n\n\
         <i>بقیه در پس زمینه ادامه دارد و پایانش در گروه اعلام می شود.</i>",
        kind.title()
    )
}

fn setting_keys(ctx: &Ctx, chat: i64, kind: Kind) -> Vec<String> {
    let ids = |prefix: &str| -> Vec<i64> {
        ctx.settings
            .flags_with_prefix(chat, prefix)
            .into_iter()
            .filter_map(|id| id.parse().ok())
            .collect()
    };
    let mut keys: Vec<String> = match kind {
        Kind::Command => restrict::custom_triggers(ctx, chat)
            .into_iter()
            .map(|(word, _)| restrict::custom_key(&word))
            .collect(),
        Kind::Answer => super::answers::triggers(ctx, chat)
            .into_iter()
            .map(|trigger| format!("{}{trigger}", super::answers::PREFIX))
            .collect(),
        Kind::Filter => filters::words(ctx, chat)
            .iter()
            .map(|word| filters::key(word))
            .collect(),
        Kind::Pack => ctx
            .settings
            .values_with_prefix(chat, packs::PREFIX)
            .into_iter()
            .filter_map(|(set, _)| set.parse().ok())
            .map(packs::key)
            .collect(),
        Kind::Vip => ids(vip::PREFIX).into_iter().map(vip::key).collect(),
        Kind::Exempt => ids(join::EXEMPT).into_iter().map(join::exempt_key).collect(),
        Kind::Ban | Kind::Mute | Kind::Image => Vec::new(),
    };
    keys.sort_unstable();
    keys.dedup();
    keys
}

pub(super) const CLEAR_PARALLEL: usize = super::FLEET_CAMPAIGNS;

const CLEAR_ROUND: usize = 200;

const CLEAR_INLINE: Duration = Duration::from_secs(10);

const CLEAR_REASON: &str = "پنل لیست ها";

struct MemberClear(Arc<super::ChatState>);

impl MemberClear {
    fn claim(ctx: &Ctx, chat: i64) -> Option<Self> {
        let state = ctx.try_state(chat)?;
        state.claim_member_clear().then(|| Self(state))
    }
}

impl Drop for MemberClear {
    fn drop(&mut self) {
        self.0.end_member_clear();
    }
}

async fn clear_members(
    ctx: &Arc<Ctx>,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
) -> Result<Cleared, MutationError> {
    if chat_ref.id.kind() != PeerKind::Channel {
        return Ok(Cleared::Done(0));
    }
    let claim = MemberClear::claim(ctx, chat).ok_or(MutationError::Busy)?;
    let removed = Arc::new(AtomicUsize::new(0));
    let (answer, mut answered) = tokio::sync::oneshot::channel();
    let run = {
        let ctx = Arc::clone(ctx);
        let removed = Arc::clone(&removed);
        async move {
            let _claim = claim;
            let outcome = unban_all(&ctx, chat_ref, chat, kind, &removed).await;
            let total = removed.load(Ordering::Relaxed);
            if total > 0 {
                super::log::write(
                    &ctx,
                    chat,
                    "log_mod",
                    super::log::Entry {
                        title: match kind {
                            Kind::Ban => "پاکسازی لیست بن",
                            _ => "پاکسازی لیست سکوت",
                        },
                        target: None,
                        actor: None,
                        reason: Some(CLEAR_REASON),
                        extra: vec![("تعداد", total.to_string())],
                    },
                )
                .await;
            }
            if let Err(outcome) = answer.send(outcome) {
                announce(&ctx, chat_ref, chat, kind, total, outcome).await;
            }
        }
    };
    Arc::clone(ctx).spawn_owned(run);
    match hand_off(&mut answered, CLEAR_INLINE).await {
        Handoff::Answered(outcome) => outcome.map(Cleared::Done),
        Handoff::Continuing => Ok(Cleared::Continuing(removed.load(Ordering::Relaxed))),
        Handoff::Dropped => Err(MutationError::Interrupted),
    }
}

enum Handoff<T> {
    Answered(T),
    Continuing,
    Dropped,
}

async fn hand_off<T>(
    answered: &mut tokio::sync::oneshot::Receiver<T>,
    wait: Duration,
) -> Handoff<T> {
    match tokio::time::timeout(wait, &mut *answered).await {
        Ok(Ok(answer)) => Handoff::Answered(answer),
        Ok(Err(_)) => Handoff::Dropped,
        Err(_) => {
            answered.close();
            match answered.try_recv() {
                Ok(answer) => Handoff::Answered(answer),
                Err(_) => Handoff::Continuing,
            }
        }
    }
}

async fn announce(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    removed: usize,
    outcome: Result<usize, MutationError>,
) {
    let mut text = format!(
        "<b>پاکسازی {}</b>\n\nحذف شده · <b>{removed}</b>",
        kind.title()
    );
    if let Err(error) = &outcome {
        ::log::warn!("lists: clear {} for {chat} stopped: {error}", kind.title());
        text.push_str("\n\n");
        text.push_str(&match error {
            MutationError::Restriction(failed) => failed.told(),
            other => format!("✗ {}", other.told()),
        });
    }
    if let Err(error) = ctx
        .client
        .send_message(
            chat_ref,
            super::premium::icon_html(Some(super::premium::Icon::Delete), text),
        )
        .await
    {
        eprintln!("lists: {chat}: could not announce the clear: {error}");
    }
}

async fn unban_all(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    removed: &AtomicUsize,
) -> Result<usize, MutationError> {
    let stopped: std::sync::Mutex<Option<restrict::Failed>> = std::sync::Mutex::new(None);
    let mut seen = HashSet::new();
    loop {
        let round = next_round(ctx, chat_ref, chat, kind, &mut seen)
            .await
            .map_err(|error| MutationError::Restriction(restrict::Failed::Telegram(error)))?;
        if round.is_empty() {
            break;
        }
        super::bounded(round, CLEAR_PARALLEL, |peer| {
            let stopped = &stopped;
            async move {
                if stopped.lock().unwrap().is_some() {
                    return;
                }
                let _turn = ctx.list_unban().await;
                if stopped.lock().unwrap().is_some() {
                    return;
                }
                match restrict::apply_maintenance(
                    ctx,
                    chat_ref,
                    peer,
                    Action::Unban,
                    None,
                    restrict::By {
                        reason: CLEAR_REASON,
                        unlogged: true,
                        ..Default::default()
                    },
                )
                .await
                {
                    Ok(_) => {
                        removed.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) if stops_clear(&error) => {
                        stopped.lock().unwrap().get_or_insert(error);
                    }
                    Err(error) => eprintln!("lists: {chat}: skipped {:?}: {error}", peer.id),
                }
            }
        })
        .await;
        if let Some(error) = stopped.lock().unwrap().take() {
            return Err(MutationError::Restriction(error));
        }
    }
    Ok(removed.load(Ordering::Relaxed))
}

fn stops_clear(error: &restrict::Failed) -> bool {
    match error {
        restrict::Failed::Telegram(grammers_client::InvocationError::Rpc(rpc)) => {
            rpc.code == 420
                || matches!(
                    rpc.name.as_str(),
                    "CHAT_ADMIN_REQUIRED"
                        | "RIGHT_FORBIDDEN"
                        | "CHAT_WRITE_FORBIDDEN"
                        | "CHANNEL_PRIVATE"
                        | "CHANNEL_INVALID"
                        | "CHAT_INVALID"
                )
        }
        restrict::Failed::Protected => false,
        restrict::Failed::Telegram(_)
        | restrict::Failed::State(_)
        | restrict::Failed::BasicGroup => true,
    }
}

async fn next_round(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    seen: &mut HashSet<i64>,
) -> Result<Vec<PeerRef>, grammers_client::InvocationError> {
    let mut participants = ctx.client.iter_participants(chat_ref).filter(kind.filter());
    let mut round = Vec::with_capacity(CLEAR_ROUND);
    while round.len() < CLEAR_ROUND {
        let Some(participant) = participants.next().await? else {
            break;
        };
        let Some(id) = participant.id().bare_id() else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        let peer = match participant.peer() {
            Some(peer) => peer.to_ref().await.ok().flatten(),
            None => None,
        };
        match peer {
            Some(peer) => round.push(peer),
            None => eprintln!("lists: {chat}: no ref for user {id}"),
        }
    }
    Ok(round)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Ban,
    Mute,
    Vip,
    Filter,
    Image,
    Answer,
    Exempt,
    Command,
    Pack,
}

const LIMIT: usize = 20;

const MEMBER_LIST_PAGE: usize = 20_000;

impl Kind {
    pub(super) const fn throttle_id(self) -> i64 {
        match self {
            Self::Ban => 0,
            Self::Mute => 1,
            Self::Vip => 2,
            Self::Filter => 3,
            Self::Image => 4,
            Self::Answer => 5,
            Self::Exempt => 6,
            Self::Command => 7,
            Self::Pack => 8,
        }
    }

    fn is_member_list(self) -> bool {
        matches!(self, Self::Ban | Self::Mute)
    }

    pub fn from_action(action: &str) -> Option<Self> {
        match action {
            "ban" => Some(Self::Ban),
            "mute" => Some(Self::Mute),
            "vip" => Some(Self::Vip),
            "filter" => Some(Self::Filter),
            "imgf" => Some(Self::Image),
            "answer" => Some(Self::Answer),
            "free" => Some(Self::Exempt),
            "cmd" => Some(Self::Command),
            "pack" => Some(Self::Pack),
            _ => None,
        }
    }

    pub fn cap(self) -> &'static super::limits::Cap {
        match self {
            Self::Ban => super::limits::BAN,
            Self::Mute => super::limits::MUTE,
            Self::Vip => super::limits::VIP,
            Self::Exempt => super::limits::EXEMPT,
            Self::Filter | Self::Image | Self::Answer | Self::Command | Self::Pack => {
                super::limits::SET
            }
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Ban => "ban",
            Self::Mute => "mute",
            Self::Vip => "vip",
            Self::Filter => "filter",
            Self::Image => "imgf",
            Self::Answer => "answer",
            Self::Exempt => "free",
            Self::Command => "cmd",
            Self::Pack => "pack",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Ban => "بن شده ها",
            Self::Mute => "سکوت شده ها",
            Self::Vip => "کاربران ویژه",
            Self::Filter => "لیست فیلتر",
            Self::Image => "فیلتر تصویری",
            Self::Answer => "پاسخ خودکار",
            Self::Exempt => "معاف ها",
            Self::Command => "دستور های سفارشی",
            Self::Pack => "پک های استیکر",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Ban => "برای رفع بن روی هر نام بزنید.",
            Self::Mute => "برای رفع سکوت روی هر نام بزنید.",
            Self::Vip => "برای حذف از لیست ویژه روی هر مورد بزنید.",
            Self::Filter => "برای حذف کلمه روی آن بزنید.",
            Self::Image => {
                "علامت ~ یعنی فقط بررسی می کند و پاک نمی کند. برای حذف روی هر مورد بزنید."
            }
            Self::Answer => "برای حذف یک پاسخ روی آن بزنید.",
            Self::Exempt => "معاف از عضویت اجباری و اد اجباری. برای حذف روی هر مورد بزنید.",
            Self::Command => "برای حذف یک دستور سفارشی روی آن بزنید.",
            Self::Pack => "برای بازکردن قفل روی هر پک بزنید.",
        }
    }

    fn filter(self) -> tl::enums::ChannelParticipantsFilter {
        let q = String::new();
        match self {
            Self::Mute => tl::enums::ChannelParticipantsFilter::ChannelParticipantsBanned(
                tl::types::ChannelParticipantsBanned { q },
            ),
            _ => tl::enums::ChannelParticipantsFilter::ChannelParticipantsKicked(
                tl::types::ChannelParticipantsKicked { q },
            ),
        }
    }
}

pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) peer: Option<PeerRef>,
}

pub fn word_id(word: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in word.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:x}")
}

pub const CLEAR_KEY: &str = "clear";
const CLEAR_CONFIRMED: &str = "clearyes";

pub fn clearing(entry_key: &str) -> Option<bool> {
    match entry_key {
        CLEAR_KEY => Some(false),
        CLEAR_CONFIRMED => Some(true),
        _ => None,
    }
}

pub async fn confirm_clear(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    opener: i64,
) -> Result<(String, ReplyMarkup), sqlx::Error> {
    let (_, count) = listed(ctx, chat_ref, chat, kind, 0).await?;
    let count_label = count_label(kind, count);
    let title = format!(
        "<b>پنل مدیریت</b> › <b>{}</b>\n\n\
         همه <b>{count_label}</b> مورد از این لیست حذف می شود. این کار برگشت ندارد.",
        kind.title()
    );
    let markup = super::premium::buttons(&[vec![
        super::premium::decorate(
            super::style::data(
                "تایید",
                format!("p:{opener}:{chat}:l:{}:{CLEAR_CONFIRMED}", kind.action()).into_bytes(),
                super::style::Colour::Primary,
            ),
            Some(super::premium::Icon::Success),
        ),
        super::premium::decorate(
            super::style::data(
                "لغو",
                format!("p:{opener}:{chat}:l:{}", kind.action()).into_bytes(),
                super::style::Colour::Danger,
            ),
            Some(super::premium::Icon::Close),
        ),
    ]]);
    Ok((title, markup))
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

pub async fn view(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    opener: i64,
) -> Result<(String, ReplyMarkup), sqlx::Error> {
    let (entries, total) = listed(ctx, chat_ref, chat, kind, LIMIT).await?;

    let mut rows: Vec<Vec<Button>> = entries
        .iter()
        .take(LIMIT)
        .map(|entry| {
            vec![Button::data(
                format!("✗  {}", unescape(&entry.name)),
                format!("p:{opener}:{chat}:l:{}:{}", kind.action(), entry.key).into_bytes(),
            )]
        })
        .collect();
    if !entries.is_empty() {
        rows.push(vec![super::style::data(
            format!("پاکسازی {}", kind.title()),
            format!("p:{opener}:{chat}:l:{}:{CLEAR_KEY}", kind.action()).into_bytes(),
            super::style::Colour::Danger,
        )]);
    }
    rows.push(vec![super::premium::decorate(
        Button::data("بازگشت", format!("p:{opener}:{chat}:ls").into_bytes()),
        Some(super::premium::Icon::Back),
    )]);

    let shown = entries.len().min(LIMIT);
    let mut title = format!(
        "<b>پنل مدیریت</b> › <b>{}</b> ({})\n\n{}",
        kind.title(),
        count_label(kind, total),
        kind.hint()
    );
    if total > shown {
        title.push_str(&format!("\n<i>{shown} مورد اول نمایش داده شده است.</i>"));
    }
    if entries.is_empty() {
        title = format!(
            "<b>پنل مدیریت</b> › <b>{}</b>\n\nلیست خالی است.",
            kind.title()
        );
    }
    Ok((title, super::premium::buttons(&rows)))
}

pub async fn remove(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    entry_key: &str,
) -> Result<bool, MutationError> {
    if kind == Kind::Command {
        if let Some((word, _)) = restrict::custom_triggers(ctx, chat)
            .into_iter()
            .find(|(word, _)| word_id(word) == entry_key)
        {
            ctx.settings
                .try_set(chat, &restrict::custom_key(&word), false)
                .await?;
            return Ok(true);
        }
        return Ok(false);
    }
    if kind == Kind::Answer {
        if let Some(trigger) = super::answers::triggers(ctx, chat)
            .into_iter()
            .find(|trigger| word_id(trigger) == entry_key)
        {
            ctx.settings
                .try_set(chat, &format!("{}{trigger}", super::answers::PREFIX), false)
                .await?;
            return Ok(true);
        }
        return Ok(false);
    }
    if kind == Kind::Filter {
        if let Some(word) = filters::words(ctx, chat)
            .into_iter()
            .find(|word| word_id(word) == entry_key)
        {
            ctx.settings
                .try_set(chat, &filters::key(&word), false)
                .await?;
            return Ok(true);
        }
        return Ok(false);
    }
    if kind == Kind::Image {
        if let Some((name, _)) = imgfilter::listing(ctx, chat)
            .await?
            .into_iter()
            .find(|(name, _)| word_id(name) == entry_key)
        {
            imgfilter::try_forget(ctx, chat, &name).await?;
            return Ok(true);
        }
        return Ok(false);
    }
    if kind == Kind::Pack {
        if let Ok(set) = entry_key.parse::<i64>() {
            ctx.settings.try_set(chat, &packs::key(set), false).await?;
            return Ok(true);
        }
        return Ok(false);
    }
    let Ok(user_id) = entry_key.parse::<i64>() else {
        return Ok(false);
    };
    if kind == Kind::Vip {
        ctx.settings
            .try_set(chat, &vip::key(user_id), false)
            .await?;
        return Ok(true);
    }
    if kind == Kind::Exempt {
        join::set_free(ctx, chat, user_id, false).await?;
        return Ok(true);
    }

    let Some(peer) = listed_member(ctx, chat_ref, chat, kind, user_id).await? else {
        return Ok(false);
    };
    restrict::apply_maintenance(
        ctx,
        chat_ref,
        peer,
        Action::Unban,
        None,
        restrict::By {
            reason: "پنل لیست ها",
            ..Default::default()
        },
    )
    .await
    .map_err(MutationError::Restriction)?;
    Ok(true)
}

pub(crate) async fn entries(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    limit: usize,
) -> Result<Vec<Entry>, sqlx::Error> {
    if kind == Kind::Command {
        return Ok(restrict::custom_triggers(ctx, chat)
            .into_iter()
            .map(|(word, action)| Entry {
                key: word_id(&word),
                name: esc(&format!("{word} ({})", restrict::action_label(action))),
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Answer {
        return Ok(super::answers::triggers(ctx, chat)
            .into_iter()
            .map(|trigger| Entry {
                key: word_id(&trigger),
                name: esc(&trigger),
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Filter {
        return Ok(filters::words(ctx, chat)
            .into_iter()
            .map(|word| Entry {
                key: word_id(&word),
                name: esc(&word),
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Image {
        return Ok(imgfilter::listing(ctx, chat)
            .await?
            .into_iter()
            .map(|(name, label)| Entry {
                key: word_id(&name),
                name: esc(&label),
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Pack {
        let mut found: Vec<Entry> = ctx
            .settings
            .values_with_prefix(chat, packs::PREFIX)
            .into_iter()
            .map(|(set, title)| Entry {
                name: esc(&format!("{title} · {set}")),
                key: set,
                peer: None,
            })
            .collect();
        found.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        found.truncate(limit);
        return Ok(found);
    }
    if kind == Kind::Exempt {
        return Ok(ctx
            .settings
            .flags_with_prefix(chat, join::EXEMPT)
            .into_iter()
            .map(|id| Entry {
                key: id.clone(),
                name: id,
                peer: None,
            })
            .collect());
    }
    if kind == Kind::Vip {
        return Ok(ctx
            .settings
            .flags_with_prefix(chat, vip::PREFIX)
            .into_iter()
            .map(|id| Entry {
                key: id.clone(),
                name: id,
                peer: None,
            })
            .collect());
    }

    Ok(walk(ctx, chat_ref, chat, kind, limit).await.0)
}

async fn listed(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    shown: usize,
) -> Result<(Vec<Entry>, usize), sqlx::Error> {
    if kind.is_member_list() {
        return Ok(walk(ctx, chat_ref, chat, kind, shown).await);
    }
    let found = entries(ctx, chat_ref, chat, kind, MEMBER_LIST_PAGE).await?;
    let total = found.len();
    Ok((found, total))
}

fn count_label(kind: Kind, count: usize) -> String {
    if !kind.is_member_list() && count >= MEMBER_LIST_PAGE {
        format!("{count}+")
    } else {
        count.to_string()
    }
}

async fn listed_member(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    user_id: i64,
) -> Result<Option<PeerRef>, sqlx::Error> {
    use grammers_client::session::types::{PeerAuth, PeerId};

    if chat_ref.id.kind() == PeerKind::Channel
        && let Some(target) = PeerId::user(user_id).map(PeerId::to_ambient_ref)
    {
        let asked = ctx
            .client
            .invoke(&tl::functions::channels::GetParticipant {
                channel: chat_ref.into(),
                participant: target.into(),
            })
            .await;
        match asked {
            Ok(tl::enums::channels::ChannelParticipant::Participant(found)) => {
                let tl::enums::ChannelParticipant::Banned(banned) = &found.participant else {
                    return Ok(None);
                };
                let tl::enums::ChatBannedRights::Rights(rights) = &banned.banned_rights;
                if rights.view_messages != (kind == Kind::Ban) {
                    return Ok(None);
                }
                let hash = found.users.iter().find_map(|user| match user {
                    tl::enums::User::User(user) if user.id == user_id => user.access_hash,
                    _ => None,
                });
                if let (Some(id), Some(hash)) = (PeerId::user(user_id), hash) {
                    return Ok(Some(PeerRef {
                        id,
                        auth: PeerAuth::from_hash(hash),
                    }));
                }
            }
            Err(grammers_client::InvocationError::Rpc(error))
                if error.name == "USER_NOT_PARTICIPANT" =>
            {
                return Ok(None);
            }
            Err(grammers_client::InvocationError::Rpc(error)) if error.code == 420 => {
                eprintln!("lists: {chat}: could not look up {user_id}: {error}");
                return Ok(None);
            }
            Err(error) => eprintln!("lists: {chat}: could not look up {user_id}: {error}"),
        }
    }
    let key = user_id.to_string();
    Ok(entries(ctx, chat_ref, chat, kind, MEMBER_LIST_PAGE)
        .await?
        .into_iter()
        .find(|entry| entry.key == key)
        .and_then(|entry| entry.peer))
}

async fn walk(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    kind: Kind,
    limit: usize,
) -> (Vec<Entry>, usize) {
    if chat_ref.id.kind() != PeerKind::Channel {
        return (Vec::new(), 0);
    }
    let mut participants = ctx.client.iter_participants(chat_ref).filter(kind.filter());
    let mut found = Vec::with_capacity(limit.min(LIMIT * 16));
    while found.len() < limit {
        match participants.next().await {
            Ok(Some(participant)) => {
                let id = participant.id();
                let key = match id.bare_id() {
                    Some(id) => id.to_string(),
                    None => continue,
                };
                let peer = participant.peer();
                found.push(Entry {
                    key: key.clone(),
                    name: peer.and_then(|peer| peer.name()).map(esc).unwrap_or(key),
                    peer: match peer {
                        Some(peer) => peer.to_ref().await.ok().flatten(),
                        None => None,
                    },
                });
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("lists: {chat}: could not list {}: {e}", kind.action());
                break;
            }
        }
    }
    let total = participants.total().await.unwrap_or(0).max(found.len());
    (found, total)
}

#[cfg(test)]
mod tests {
    #[test]
    fn list_aliases_are_exact_and_have_a_cleanup_family() {
        for (name, kind) in SHOW {
            let normalized = name.replace('ی', "ي").replace('ک', "ك").replace(' ', "\t");
            assert!(
                SHOW.iter()
                    .any(
                        |(alias, found)| super::super::commands::matches(&normalized, alias)
                            && found == kind
                    ),
                "{name}"
            );
            assert!(CLEAR.iter().any(|(_, found)| found == kind), "{name}");
            assert!(
                !SHOW
                    .iter()
                    .any(|(alias, _)| super::super::commands::matches(
                        &format!("{name} لطفا"),
                        alias
                    )),
                "{name}"
            );
        }
    }
    use super::*;

    #[test]
    fn list_unbans_never_consume_reserved_enforcement_capacity() {
        let source = include_str!("lists.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert_eq!(
            production.matches("restrict::apply_maintenance(").count(),
            2
        );
        assert!(!production.contains("restrict::apply("));
    }

    #[test]
    fn a_clear_stops_for_the_chat_and_skips_for_a_member() {
        let rpc = |code: i32, name: &str| {
            restrict::Failed::Telegram(grammers_client::InvocationError::Rpc(
                grammers_client::tl::types::RpcError {
                    error_code: code,
                    error_message: name.to_owned(),
                }
                .into(),
            ))
        };
        for name in [
            "CHAT_ADMIN_REQUIRED",
            "RIGHT_FORBIDDEN",
            "CHANNEL_PRIVATE",
            "CHANNEL_INVALID",
            "CHAT_INVALID",
        ] {
            assert!(stops_clear(&rpc(400, name)), "{name}");
        }
        assert!(stops_clear(&rpc(420, "FLOOD_WAIT_30")));
        assert!(stops_clear(&restrict::Failed::BasicGroup));
        for name in [
            "USER_ID_INVALID",
            "PARTICIPANT_ID_INVALID",
            "INPUT_USER_DEACTIVATED",
            "USER_NOT_PARTICIPANT",
        ] {
            assert!(!stops_clear(&rpc(400, name)), "{name}");
        }
        assert!(!stops_clear(&restrict::Failed::Protected));
    }

    #[tokio::test]
    async fn a_clear_is_answered_or_handed_to_the_run_never_lost() {
        let wait = Duration::from_millis(20);

        let (answer, mut answered) = tokio::sync::oneshot::channel();
        answer.send(7).unwrap();
        assert!(matches!(
            hand_off(&mut answered, wait).await,
            Handoff::Answered(7)
        ));

        let (answer, mut answered) = tokio::sync::oneshot::channel::<i32>();
        assert!(matches!(
            hand_off(&mut answered, wait).await,
            Handoff::Continuing
        ));
        assert_eq!(answer.send(7), Err(7), "the run must learn it has to announce");

        let (answer, mut answered) = tokio::sync::oneshot::channel::<i32>();
        drop(answer);
        assert!(matches!(
            hand_off(&mut answered, wait).await,
            Handoff::Dropped
        ));
    }

    #[tokio::test]
    #[ignore = "needs DATABASE_URL: a Ctx needs a settings mirror"]
    async fn one_member_clear_per_chat_and_none_in_a_basic_group() {
        use grammers_client::session::types::{PeerAuth, PeerId};

        let (ctx, _pool) = super::super::live_ctx().await;
        let ctx = Arc::new(ctx);
        let chat = -1_009_999_000_123;
        let first = MemberClear::claim(&ctx, chat).expect("no clear is running");
        assert!(MemberClear::claim(&ctx, chat).is_none());
        drop(first);
        assert!(MemberClear::claim(&ctx, chat).is_some());

        let basic = PeerRef {
            id: PeerId::chat(123).unwrap(),
            auth: PeerAuth::default(),
        };
        for kind in [Kind::Ban, Kind::Mute] {
            assert!(matches!(
                clear_all(&ctx, basic, -123, kind).await,
                Ok(Cleared::Done(0))
            ));
            let (entries, total) = walk(&ctx, basic, -123, kind, LIMIT).await;
            assert!(entries.is_empty() && total == 0);
        }
    }

    #[test]
    fn pack_kind_round_trips_through_panel_actions() {
        assert!(matches!(Kind::from_action("pack"), Some(Kind::Pack)));
        assert_eq!(Kind::Pack.action(), "pack");
        assert_eq!(Kind::Pack.cap().key, super::super::limits::SET.key);
    }

    #[test]
    fn payloads_match_what_the_panel_parses() {
        let (opener, chat) = (1234567890_i64, -1001234567890_i64);
        for payload in [
            format!(
                "p:{opener}:{chat}:l:{}:{}",
                Kind::Filter.action(),
                "deadbeef"
            ),
            format!("p:{opener}:{chat}:l:{}:{}", Kind::Ban.action(), 42),
            format!("p:{opener}:{chat}:adv"),
        ] {
            let rest = payload.strip_prefix("p:").expect("panel prefix");
            let mut parts = rest.splitn(3, ':');
            assert_eq!(parts.next().and_then(|p| p.parse().ok()), Some(opener));
            assert_eq!(parts.next().and_then(|p| p.parse().ok()), Some(chat));
            assert!(parts.next().is_some_and(|action| !action.is_empty()));
            assert!(payload.len() <= 64, "payload too long: {payload}");
        }
    }

    #[test]
    fn the_clear_keys_cannot_be_mistaken_for_an_entry() {
        for key in [CLEAR_KEY, "clearyes"] {
            assert!(clearing(key).is_some());
            assert!(key.parse::<i64>().is_err(), "{key} could be a user id");
            assert!(
                !key.chars().all(|c| c.is_ascii_hexdigit()),
                "{key} could be a word_id hash"
            );
        }
        assert_eq!(clearing(&word_id("تبلیغ")), None);
        assert_eq!(clearing("12345"), None);
        assert_eq!(clearing(CLEAR_KEY), Some(false));
        assert_eq!(clearing("clearyes"), Some(true));
    }

    #[test]
    fn a_button_label_reads_the_name_as_typed() {
        for name in ["Tom & Jerry", "<b>", "&amp;", "a&lt;b"] {
            assert_eq!(unescape(&esc(name)), name);
        }
    }

    #[test]
    fn clear_payloads_fit_telegram() {
        for kind in [
            Kind::Ban,
            Kind::Mute,
            Kind::Vip,
            Kind::Filter,
            Kind::Image,
            Kind::Answer,
            Kind::Exempt,
            Kind::Command,
            Kind::Pack,
        ] {
            for key in [CLEAR_KEY, "clearyes"] {
                let payload = format!("p:{}:{}:l:{}:{key}", i64::MAX, i64::MIN, kind.action());
                assert!(payload.len() <= 64, "payload too long: {payload}");
            }
        }
    }
}
