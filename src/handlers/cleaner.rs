use std::time::Duration;

use grammers_client::Client;
use grammers_client::message::Message;
use grammers_client::session::types::{PeerId, PeerRef};

use crate::response::ResponseKind;

use super::Ctx;

const START: &[&str] = &["ورود کلینر", "لاگین کلینر", "کلینر", "/cleanerlogin"];
pub const ADD: &[&str] = &[
    "افزودن کلینر",
    "اضافه کردن کلینر",
    "نصب کلینر",
    "اضافه کلینر",
    "/addcleaner",
    "راه اندازی کلینر",
    "/setupcleaner",
];
pub const WIPE: &[&str] = &[
    "حذف پیام های کاربر",
    "پاکسازی پیام های کاربر",
    "حذف پیام ها",
    "پاکسازی پیام ها",
    "حذف پیام",
    "پاکسازی پیام",
    "/wipe",
    "/purgeuser",
];
pub const SWEEP: &[&str] = &[
    "پاکسازی",
    "پاک سازی",
    "پاک کردن همه",
    "/clean",
    "/sweep",
];

const SWEEP_MAX: usize = 3_000;

fn media_filter(name: &str) -> Option<(&'static str, grammers_client::tl::enums::MessagesFilter)> {
    use grammers_client::tl::types::*;
    let normalized = super::commands::normalize(name);
    let found = match normalized.as_ref() {
        "عکس" | "تصویر" => ("عکس", InputMessagesFilterPhotos {}.into()),
        "ویدیو" | "فیلم" | "ویدئو" => ("ویدیو", InputMessagesFilterVideo {}.into()),
        "مدیا" | "رسانه" => ("مدیا", InputMessagesFilterPhotoVideo {}.into()),
        "گیف" => ("گیف", InputMessagesFilterGif {}.into()),
        "ویس" | "صدا" => ("ویس", InputMessagesFilterVoice {}.into()),
        "موزیک" | "آهنگ" | "اهنگ" => ("موزیک", InputMessagesFilterMusic {}.into()),
        "فایل" | "سند" | "استیکر" => ("فایل", InputMessagesFilterDocument {}.into()),
        "لینک" => ("لینک", InputMessagesFilterUrl {}.into()),
        "مکان" | "لوکیشن" => ("مکان", InputMessagesFilterGeo {}.into()),
        "مخاطب" | "کانتکت" => ("مخاطب", InputMessagesFilterContacts {}.into()),
        "نظرسنجی" => ("نظرسنجی", InputMessagesFilterPoll {}.into()),
        "ویدیو پیام" | "ویدئو پیام" => {
            ("ویدیو پیام", InputMessagesFilterRoundVideo {}.into())
        }
        _ => return None,
    };
    Some(found)
}

const SEARCH_LIMIT: usize = 5_000;
const RESTRICTED_PAGE: usize = 200;

const DEADLINE: Duration = Duration::from_secs(300);

const CHUNK: usize = 100;

const RANGE_MAX: i32 = 10_000;
const USER_CHATS_MAX: usize = 50_000;

pub enum Installed {
    AlreadyAdmin,
    Promoted,
    JoinedNotAdmin(String),
}

pub async fn install(ctx: &Ctx, chat_ref: PeerRef) -> std::result::Result<Installed, String> {
    let (Some(cleaner), Some(user)) = (ctx.cleaner_id(), ctx.user_client()) else {
        return Err("کلینر وارد نشده است".to_owned());
    };
    let Some(target) = PeerId::user(cleaner).map(PeerId::to_ambient_ref) else {
        return Err("شناسه کلینر خوانده نشد".to_owned());
    };

    if let Ok(state) = ctx.client.get_permissions(chat_ref, target).await
        && state.is_banned()
        && let Err(e) = super::restrict::apply(
            ctx,
            chat_ref,
            target,
            super::restrict::Action::Unban,
            None,
            super::restrict::By {
                reason: "افزودن کلینر",
                target_name: "کلینر",
                ..Default::default()
            },
        )
        .await
    {
        return Err(format!("رفع بن کلینر انجام نشد · {e}"));
    }

    let member = matches!(
        ctx.client.get_permissions(chat_ref, target).await,
        Ok(state) if !state.has_left() && !state.is_banned()
    );
    if !member {
        join(ctx, &user, chat_ref).await?;
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    if let Ok(state) = ctx.client.get_permissions(chat_ref, target).await
        && state.is_admin()
    {
        return Ok(Installed::AlreadyAdmin);
    }

    match promote(ctx, chat_ref, target).await {
        Ok(()) => Ok(Installed::Promoted),
        Err(e) => Ok(Installed::JoinedNotAdmin(e.to_string())),
    }
}

pub async fn add(ctx: &Ctx, message: &Message) -> bool {
    if !super::commands::contains(ADD, message.text()) {
        return false;
    }
    if !super::limits::allows(ctx, message, super::limits::CLEAN).await {
        return true;
    }
    if ctx.cleaner_id().is_none() || ctx.user_client().is_none() {
        super::respond(
            ctx,
            message,
            ResponseKind::AdminTool,
            "کلینر وارد نشده است. مالک ربات باید در پیوی «ورود کلینر» را بفرستد.",
        )
        .await;
        return true;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    let Some(chat) = message
        .peer_id()
        .bot_api_dialog_id()
        .filter(|chat| *chat < 0)
    else {
        return false;
    };
    if let Some((_, body)) = super::cleaner_setup::check_access(ctx, chat_ref, chat).await {
        super::respond(ctx, message, ResponseKind::AdminTool, body).await;
        return true;
    }

    super::respond(
        ctx,
        message,
        ResponseKind::AdminTool,
        super::premium::icon_text(
            Some(super::premium::Icon::Hourglass),
            "در حال افزودن کلینر...",
        ),
    )
    .await;
    let body = super::cleaner_setup::add(ctx, chat_ref, chat).await;
    super::respond(ctx, message, ResponseKind::AdminTool, body).await;
    true
}

async fn join(ctx: &Ctx, user: &Client, chat_ref: PeerRef) -> std::result::Result<(), String> {
    let exported = ctx
        .client
        .invoke_outbound_critical(
            &grammers_client::tl::functions::messages::ExportChatInvite {
                legacy_revoke_permanent: false,
                request_needed: false,
                peer: chat_ref.into(),
                expire_date: None,
                usage_limit: Some(1),
                title: Some("cleaner".to_owned()),
                subscription_pricing: None,
            },
        )
        .await
        .map_err(|e| match e.to_string().contains("CHAT_ADMIN_REQUIRED") {
            true => "ربات دسترسی «افزودن اعضا» ندارد؛ آن را بدهید و دوباره بفرستید.".to_owned(),
            false => format!("ساخت لینک دعوت انجام نشد · {e}"),
        })?;
    let link = match exported {
        grammers_client::tl::enums::ExportedChatInvite::ChatInviteExported(invite) => invite.link,
        _ => return Err("لینک دعوت ساخته نشد.".to_owned()),
    };
    let mut retries = 0;
    let joined = loop {
        let result = user.accept_invite_link(&link).await;
        if retries < 2
            && let Err(error) = &result
            && let Some(delay) = join_retry_delay(error)
        {
            retries += 1;
            log::info!(
                "cleaner: waiting {}s before retrying join in {}",
                delay.as_secs(),
                chat_ref.id
            );
            tokio::time::sleep(delay).await;
            continue;
        }
        break result;
    };
    match joined {
        Ok(Some(_)) => Ok(()),

        Ok(None) => Err("درخواست عضویت فرستاده شد؛ آن را در گروه تایید کنید.".to_owned()),
        Err(e) if e.to_string().contains("USER_ALREADY_PARTICIPANT") => Ok(()),
        Err(e) if e.to_string().contains("INVITE_REQUEST_SENT") => {
            Err("درخواست عضویت فرستاده شد؛ آن را در گروه تایید کنید.".to_owned())
        }
        Err(e) if e.to_string().contains("USER_BANNED_IN_CHANNEL") => {
            Err("حساب کلینر در این گروه بن است و رفع بن نشد.".to_owned())
        }
        Err(e) if e.to_string().contains("CHANNELS_TOO_MUCH") => {
            ctx.pause_cleaner_joins(CLEANER_FULL_PAUSE);
            Err("حساب کلینر در گروه های زیادی است و جای خالی ندارد.".to_owned())
        }
        Err(e) if e.to_string().contains("FLOOD_WAIT") => {
            if let grammers_client::InvocationError::Rpc(rpc) = &e
                && let Some(seconds) = rpc.value
            {
                ctx.pause_cleaner_joins(Duration::from_secs(u64::from(seconds)));
            }
            Err(format!("تلگرام فعلا اجازه نمی دهد · {e}"))
        }
        Err(e) => Err(format!("پیوستن کلینر انجام نشد · {e}")),
    }
}

const CLEANER_FULL_PAUSE: Duration = Duration::from_secs(3600);

fn join_retry_delay(error: &grammers_client::InvocationError) -> Option<Duration> {
    let grammers_client::InvocationError::Rpc(rpc) = error else {
        return None;
    };
    let seconds = rpc.value?;
    (rpc.name == "FLOOD_WAIT" && (1..=30).contains(&seconds))
        .then(|| Duration::from_secs(u64::from(seconds) + 1))
}

pub async fn on_join(ctx: &Ctx, message: &Message) -> bool {
    let Some(cleaner) = ctx.cleaner_id() else {
        return false;
    };
    let cleaner_joined = match message.action() {
        Some(grammers_client::tl::enums::MessageAction::ChatAddUser(action)) => {
            action.users.contains(&cleaner)
        }
        Some(grammers_client::tl::enums::MessageAction::ChatJoinedByLink(_)) => {
            message.sender_id().and_then(PeerId::bare_id) == Some(cleaner)
        }
        _ => false,
    };
    if !cleaner_joined {
        return false;
    }
    let Some(joined) = super::joined_users(ctx, message)
        .await
        .into_iter()
        .find(|joined| joined.id == cleaner)
    else {
        return false;
    };
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };
    if let Err(e) = promote(ctx, chat_ref, joined.peer).await {
        eprintln!("cleaner: could not promote in {}: {e}", chat_ref.id);
    }
    if let Some(chat) = chat_ref.id.bot_api_dialog_id() {
        ctx.forget_user_chat(chat);
    }
    false
}

pub async fn purge_history(
    ctx: &Ctx,
    chat: i64,
    max_id: i32,
) -> std::result::Result<usize, String> {
    use grammers_client::session::types::PeerKind;

    let mut refused = None;
    if let Some(user) = ctx.user_client() {
        match chat_ref(ctx, &user, chat).await {
            Some(chat_ref) if chat_ref.id.kind() == PeerKind::Channel => {
                let (deleted, error) = purge_by_authors(&user, chat_ref, max_id).await;
                match error {
                    None => return Ok(deleted),
                    Some(e) => {
                        eprintln!("purge all: {chat}: cleaner stopped after {deleted}: {e}");
                        if deleted > 0 {
                            return Ok(deleted);
                        }
                        refused = Some(format!("کلینر · {}", refusal(&e)));
                    }
                }
            }
            Some(chat_ref) => match delete_range(&user, chat_ref, 1, max_id).await {
                0 => refused = Some("کلینر نتوانست".to_owned()),
                deleted => return Ok(deleted),
            },
            None => refused = Some("کلینر در این گروه نیست".to_owned()),
        }
    }

    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return Err(refused.unwrap_or_else(|| "گروه شناخته نشد".to_owned()));
    };
    match delete_range(&ctx.client, chat_ref, 1, max_id).await {
        0 => Err(match refused {
            Some(refused) => format!("{refused}\nربات هم نتوانست"),
            None => "ربات اجازه حذف ندارد".to_owned(),
        }),
        deleted => Ok(deleted),
    }
}

async fn delete_range(client: &Client, chat_ref: PeerRef, first: i32, last: i32) -> usize {
    let first = first.max(last - RANGE_MAX + 1).max(1);
    let ids: Vec<i32> = (first..=last).collect();
    let (deleted, failed) = super::delete_chunks(client, chat_ref, &ids, CHUNK, false).await;
    if let Some(e) = failed {
        eprintln!("cleaner: delete range {first}..={last} stopped: {e}");
    }
    deleted
}

const PURGE_AUTHORS_AT_ONCE: usize = 8;

const PURGE_READ_MAX: usize = 20_000;

async fn purge_by_authors(
    client: &Client,
    chat_ref: PeerRef,
    max_id: i32,
) -> (usize, Option<grammers_client::InvocationError>) {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let deleted = AtomicUsize::new(0);
    let stopped: Mutex<Option<grammers_client::InvocationError>> = Mutex::new(None);
    let wipe = |target: PeerRef| {
        let (deleted, stopped) = (&deleted, &stopped);
        async move {
            if stopped.lock().unwrap().is_some() {
                return;
            }
            let history = delete_history(client, chat_ref, target).await;
            deleted.fetch_add(history.deleted, Ordering::Relaxed);
            match history.error {
                Some(e) if stops_purge(&e) => {
                    stopped.lock().unwrap().get_or_insert(e);
                }
                Some(e) => eprintln!("purge all: skipped {:?}: {e}", target.id),
                None => {}
            }
        }
    };

    let mut seen = std::collections::HashSet::new();
    let mut authors = Vec::with_capacity(PURGE_AUTHORS_AT_ONCE);
    let mut orphans = Vec::new();
    let mut messages = client
        .iter_messages(chat_ref)
        .offset_id(max_id.saturating_add(1))
        .limit(PURGE_READ_MAX);
    loop {
        match messages.next().await {
            Ok(Some(message)) => match message.sender_ref().await {
                Ok(Some(author)) if author.id != chat_ref.id => {
                    if seen.insert(author.id) {
                        authors.push(author);
                    } else {
                        orphans.push(message.id());
                    }
                }
                _ => orphans.push(message.id()),
            },
            Ok(None) => break,
            Err(e) if stops_purge(&e) => {
                stopped.lock().unwrap().get_or_insert(e);
                break;
            }
            Err(e) => {
                eprintln!("purge all: history scan cut short: {e}");
                break;
            }
        }
        if authors.len() >= PURGE_AUTHORS_AT_ONCE {
            super::bounded(std::mem::take(&mut authors), PURGE_AUTHORS_AT_ONCE, &wipe).await;
            if stopped.lock().unwrap().is_some() {
                break;
            }
        }
    }
    super::bounded(authors, PURGE_AUTHORS_AT_ONCE, &wipe).await;
    if let Some(e) = stopped.lock().unwrap().take() {
        return (deleted.load(Ordering::Relaxed), Some(e));
    }
    let (by_id, failed) = super::delete_chunks(client, chat_ref, &orphans, CHUNK, false).await;
    if let Some(e) = failed {
        eprintln!("purge all: unauthored messages stopped: {e}");
    }
    (deleted.load(Ordering::Relaxed) + by_id, None)
}

fn stops_purge(e: &grammers_client::InvocationError) -> bool {
    matches!(
        e,
        grammers_client::InvocationError::Rpc(rpc) if rpc.code == 420 || matches!(
            rpc.name.as_str(),
            "CHAT_ADMIN_REQUIRED" | "RIGHT_FORBIDDEN" | "CHANNEL_PRIVATE" | "CHANNEL_INVALID"
        )
    )
}

pub async fn sweep(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(rest) = SWEEP
        .iter()
        .find_map(|command| super::commands::strip_prefix(text, command))
    else {
        return false;
    };
    let Some((label, filter)) = media_filter(rest) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::CLEAN).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    let (Some(user), Some(_)) = (ctx.user_client(), ctx.cleaner_id()) else {
        super::respond(
            ctx,
            message,
            ResponseKind::ModerationConfirmation,
            "برای پاکسازی قدیمی ها کلینر لازم است. «افزودن کلینر» را بفرستید.",
        )
        .await;
        return true;
    };
    let Some(chat_ref) = chat_ref(ctx, &user, chat).await else {
        super::respond(
            ctx,
            message,
            ResponseKind::ModerationConfirmation,
            "کلینر در این گروه نیست. «افزودن کلینر» را بفرستید.",
        )
        .await;
        return true;
    };

    super::respond(
        ctx,
        message,
        ResponseKind::ModerationConfirmation,
        super::premium::icon_text(
            Some(super::premium::Icon::Hourglass),
            format!("در حال پاکسازی {label}..."),
        ),
    )
    .await;
    let mut ids: Vec<i32> = Vec::new();
    let mut search = user.search_messages(chat_ref).filter(filter);
    let mut failed = None;
    while ids.len() < SWEEP_MAX {
        match search.next().await {
            Ok(Some(found)) => ids.push(found.id()),
            Ok(None) => break,
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    if let Some(e) = failed.filter(|_| ids.is_empty()) {
        super::respond(
            ctx,
            message,
            ResponseKind::ModerationConfirmation,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                format!("انجام نشد · {e}\nکلینر باید در گروه ادمین باشد."),
            ),
        )
        .await;
        return true;
    }
    if ids.is_empty() {
        super::respond(
            ctx,
            message,
            ResponseKind::ModerationConfirmation,
            format!("چیزی از نوع {label} پیدا نشد."),
        )
        .await;
        return true;
    }

    let (deleted, failed) = super::delete_chunks(&user, chat_ref, &ids, CHUNK, false).await;
    if let Some(e) = failed {
        super::respond(
            ctx,
            message,
            ResponseKind::ModerationConfirmation,
            format!("{deleted} پیام پاک شد، بعد خطا داد · {e}"),
        )
        .await;
        return true;
    }
    super::respond(
        ctx,
        message,
        ResponseKind::ModerationConfirmation,
        match ids.len() >= SWEEP_MAX {
            true => format!(
                "✓ {deleted} {label} پاک شد (سقف هر بار {SWEEP_MAX} تا). برای بقیه دوباره بفرستید."
            ),
            false => format!("✓ {deleted} {label} پاک شد."),
        },
    )
    .await;
    true
}

fn wipe_argument(text: &str) -> Option<&str> {
    WIPE.iter()
        .filter_map(|command| {
            super::commands::strip_prefix(text, command).map(|tail| (command, tail))
        })
        .max_by_key(|(command, _)| command.chars().count())
        .map(|(_, tail)| tail)
}

pub async fn wipe(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(arg) = wipe_argument(text) else {
        return false;
    };
    let arg = (!arg.is_empty()).then_some(arg);
    let Some(named) = super::named(message, arg) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::CLEAN).await {
        return true;
    }
    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        super::respond(
            ctx,
            message,
            ResponseKind::ModerationConfirmation,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.",
            ),
        )
        .await;
        return true;
    };
    let (Some(chat), Some(user_id)) = (message.peer_id().bot_api_dialog_id(), target.id.bare_id())
    else {
        return false;
    };
    if super::owner(ctx, chat) == Some(user_id) {
        super::respond(
            ctx,
            message,
            ResponseKind::ModerationConfirmation,
            super::premium::icon_text(
                Some(super::premium::Icon::Locked),
                "پیام های مالک ربات پاک نمی شود.",
            ),
        )
        .await;
        return true;
    }

    let reply = match wipe_as_cleaner(ctx, chat, user_id, arg).await {
        Err(missing) => missing.reply().into(),
        Ok(history) => {
            if let Some(e) = &history.error {
                eprintln!("wipe: {chat}: {e}");
            }
            let (icon, text) = history.reply(&name);
            super::premium::icon_text(Some(icon), text)
        }
    };
    super::respond(ctx, message, ResponseKind::ModerationConfirmation, reply).await;
    true
}

#[derive(Debug, PartialEq)]
enum Missing {
    Cleaner,
    Chat,
    Member,
}

impl Missing {
    fn reply(&self) -> &'static str {
        match self {
            Missing::Cleaner => "برای این کار کلینر لازم است. «افزودن کلینر» را بفرستید.",
            Missing::Chat => "کلینر عضو این گروه نیست. «افزودن کلینر» را بفرستید.",
            Missing::Member => {
                "کلینر این کاربر را پیدا نکرد. روی پیام او ریپلای کنید یا @username بفرستید."
            }
        }
    }
}

pub(crate) struct History {
    pub deleted: usize,
    pub more: bool,
    pub error: Option<grammers_client::InvocationError>,
}

impl History {
    fn reply(&self, name: &str) -> (super::premium::Icon, String) {
        use super::premium::Icon;
        match (&self.error, self.deleted) {
            (Some(e), 0) => (Icon::ErrorRed, refusal(e)),
            (Some(e), deleted) => (
                Icon::ErrorRed,
                format!("{deleted} پیام از {name} پاک شد، بقیه انجام نشد · {}", refusal(e)),
            ),
            (None, 0) => (Icon::Delete, format!("پیامی از {name} در این گروه پیدا نشد.")),
            (None, deleted) if self.more => (
                Icon::Delete,
                format!("{deleted} پیام از {name} پاک شد. برای بقیه دوباره بفرستید."),
            ),
            (None, deleted) => (
                Icon::Delete,
                format!("{deleted} پیام از {name} در این گروه پاک شد."),
            ),
        }
    }
}

fn refusal(e: &grammers_client::InvocationError) -> String {
    match e {
        grammers_client::InvocationError::Rpc(rpc)
            if matches!(rpc.name.as_str(), "CHAT_ADMIN_REQUIRED" | "RIGHT_FORBIDDEN") =>
        {
            "کلینر ادمین نیست یا حق حذف پیام ندارد.".to_owned()
        }
        grammers_client::InvocationError::Rpc(rpc)
            if matches!(rpc.name.as_str(), "PARTICIPANT_ID_INVALID" | "USER_ID_INVALID") =>
        {
            "کلینر این کاربر را نمی شناسد.".to_owned()
        }
        e => format!("انجام نشد · {e}"),
    }
}

pub async fn wipe_user(ctx: &Ctx, chat: i64, user_id: i64) -> bool {
    match wipe_as_cleaner(ctx, chat, user_id, None).await {
        Ok(History { error: None, .. }) => true,
        Ok(History { error: Some(e), .. }) => {
            eprintln!("wipe_user: {chat}: {e}");
            false
        }
        Err(_) => false,
    }
}

async fn wipe_as_cleaner(
    ctx: &Ctx,
    chat: i64,
    user_id: i64,
    arg: Option<&str>,
) -> std::result::Result<History, Missing> {
    let user = ctx.user_client().ok_or(Missing::Cleaner)?;
    let chat_ref = chat_ref(ctx, &user, chat).await.ok_or(Missing::Chat)?;
    let target = member_ref(&user, chat_ref, user_id, arg)
        .await
        .ok_or(Missing::Member)?;
    Ok(delete_history(&user, chat_ref, target).await)
}

pub(crate) async fn member_ref(
    user: &Client,
    chat_ref: PeerRef,
    user_id: i64,
    arg: Option<&str>,
) -> Option<PeerRef> {
    use grammers_client::tl;

    if let Some(username) = arg.and_then(|arg| arg.strip_prefix('@'))
        && let Ok(Some(peer)) = user.resolve_username(username).await
        && let Ok(Some(peer_ref)) = peer.to_ref().await
    {
        return Some(peer_ref);
    }
    if let Some(id) = PeerId::user(user_id)
        && let Ok(peer) = user.resolve_peer(id.to_ambient_ref()).await
        && let Ok(Some(peer_ref)) = peer.to_ref().await
    {
        return Some(peer_ref);
    }
    for filter in [
        tl::enums::ChannelParticipantsFilter::ChannelParticipantsKicked(
            tl::types::ChannelParticipantsKicked { q: String::new() },
        ),
        tl::enums::ChannelParticipantsFilter::ChannelParticipantsBanned(
            tl::types::ChannelParticipantsBanned { q: String::new() },
        ),
    ] {
        let mut restricted = user.iter_participants(chat_ref).filter(filter);
        let mut seen = 0;
        while seen < RESTRICTED_PAGE
            && let Ok(Some(participant)) = restricted.next().await
        {
            seen += 1;
            if participant.id().bare_id() == Some(user_id)
                && let Some(found) = participant.user()
                && let Ok(Some(peer_ref)) = found.to_ref().await
            {
                return Some(peer_ref);
            }
        }
    }
    let mut participants = user.iter_participants(chat_ref);
    let mut seen = 0;
    while let Ok(Some(participant)) = participants.next().await {
        if participant.id().bare_id() == Some(user_id) {
            return participant.user()?.to_ref().await.ok().flatten();
        }
        seen += 1;
        if seen >= SEARCH_LIMIT {
            break;
        }
    }
    None
}

const WIPE_ROUNDS: usize = 32;

pub(crate) async fn delete_history(client: &Client, chat_ref: PeerRef, target: PeerRef) -> History {
    use grammers_client::tl;

    let mut history = History {
        deleted: 0,
        more: false,
        error: None,
    };
    for _ in 0..WIPE_ROUNDS {
        let asked = client
            .invoke_outbound_critical(&tl::functions::channels::DeleteParticipantHistory {
                channel: chat_ref.into(),
                participant: target.into(),
            })
            .await;
        match asked {
            Ok(tl::enums::messages::AffectedHistory::History(done)) => {
                history.deleted += done.pts_count.max(0) as usize;
                history.more = done.offset != 0;
                if !history.more {
                    break;
                }
            }
            Err(e) => {
                history.error = Some(e);
                break;
            }
        }
    }
    history
}

pub async fn purge(ctx: &Ctx, chat: i64, first: i32, last: i32) -> Option<usize> {
    let user = ctx.user_client()?;
    let chat_ref = chat_ref(ctx, &user, chat).await?;
    let ids: Vec<i32> = (first..=last).collect();
    let (deleted, failed) = super::delete_chunks(&user, chat_ref, &ids, CHUNK, false).await;
    if let Some(e) = failed {
        eprintln!("cleaner: could not delete in {chat}: {e}");
        return (deleted > 0).then_some(deleted);
    }
    Some(deleted)
}

pub async fn set_slow(ctx: &Ctx, chat: i64, seconds: u32) -> Option<bool> {
    let user = ctx.user_client()?;
    let chat_ref = chat_ref(ctx, &user, chat).await?;
    match user
        .invoke_outbound(&grammers_client::tl::functions::channels::ToggleSlowMode {
            channel: chat_ref.into(),
            seconds: seconds as i32,
        })
        .await
    {
        Ok(_) => Some(true),
        Err(grammers_client::InvocationError::Rpc(rpc)) if rpc.name == "CHAT_NOT_MODIFIED" => {
            Some(true)
        }
        Err(e) => {
            eprintln!("cleaner: slow mode in {chat}: {e}");
            Some(false)
        }
    }
}

pub(crate) async fn chat_ref(ctx: &Ctx, user: &Client, chat: i64) -> Option<PeerRef> {
    if let Some(peer) = ctx.user_chat(chat) {
        return Some(peer);
    }
    if ctx.user_chat_absent(chat) {
        return None;
    }
    let _walking = ctx.user_chats_walk().await;
    if let Some(peer) = ctx.user_chat(chat) {
        return Some(peer);
    }
    if ctx.user_chat_absent(chat) || ctx.user_chats_walk_failed_recently() {
        return None;
    }
    let mut found = Vec::with_capacity(USER_CHATS_MAX);
    let mut requested = None;
    let mut dialogs = user.iter_dialogs();
    let mut complete = false;
    loop {
        let dialog = match dialogs.next().await {
            Ok(Some(dialog)) => dialog,
            Ok(None) => {
                complete = true;
                break;
            }
            Err(_) => break,
        };
        let peer = dialog.peer();
        let Some(id) = peer.id().bot_api_dialog_id() else {
            continue;
        };
        if found.len() >= USER_CHATS_MAX && id != chat {
            continue;
        }
        if let Ok(Some(peer_ref)) = peer.to_ref().await {
            if id == chat {
                requested = Some(peer_ref);
            }
            if found.len() < USER_CHATS_MAX {
                found.push((id, peer_ref));
            }
        }
    }
    let peer = requested.or_else(|| {
        found
            .iter()
            .find(|(id, _)| *id == chat)
            .map(|(_, peer)| *peer)
    });
    if let Some(peer) = peer
        && !found.iter().any(|(id, _)| *id == chat)
    {
        if found.len() >= USER_CHATS_MAX {
            found.pop();
        }
        found.push((chat, peer));
    }
    if complete {
        ctx.set_user_chats(found);
    } else {
        ctx.add_user_chats(found, USER_CHATS_MAX);
    }
    ctx.note_user_chats_walk(complete);
    if peer.is_none() && complete {
        ctx.remember_user_chat_absent(chat);
    }
    peer
}

async fn promote(
    ctx: &Ctx,
    chat_ref: PeerRef,
    target: PeerRef,
) -> std::result::Result<(), super::restrict::MemberMutationError> {
    let mut last = None;
    for level in 0..3 {
        match super::restrict::set_member_admin_rights(
            ctx,
            chat_ref,
            target,
            super::restrict::AdminRightsSpec {
                delete_messages: true,
                ban_users: true,
                invite_users: true,
                pin_messages: true,
                change_info: level < 2,
                manage_call: level < 2,
                add_admins: level < 1,
            },
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                let narrower_might_work = e.to_string().contains("RIGHT_FORBIDDEN");
                last = Some(e);
                if !narrower_might_work {
                    break;
                }
            }
        }
    }
    Err(
        last.unwrap_or(super::restrict::MemberMutationError::Telegram(
            grammers_client::InvocationError::Dropped,
        )),
    )
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    if !super::commands::contains(START, text) {
        return false;
    }
    let sender = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id);
    if sender.is_none() || sender != ctx.sudo_id() {
        return true;
    }
    let Some(user) = ctx.user_client() else {
        super::respond(
            ctx,
            message,
            ResponseKind::CleanerAuthentication,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "نشست کلینر ساخته نشد. مقدار TG_ID و TG_HASH را بررسی کنید.",
            ),
        )
        .await;
        return true;
    };
    let Some(_login) = ctx.try_cleaner_login() else {
        super::respond(
            ctx,
            message,
            ResponseKind::CleanerAuthentication,
            "ورود کلینر همین حالا در حال انجام است.",
        )
        .await;
        return true;
    };
    match user.is_authorized().await {
        Ok(true) => match user.get_me().await {
            Ok(me) => {
                let Some(id) = me.id().bare_id() else {
                    super::respond(
                        ctx,
                        message,
                        ResponseKind::CleanerAuthentication,
                        "شناسه حساب کلینر معتبر نیست.",
                    )
                    .await;
                    return true;
                };
                ctx.set_cleaner_id(id);
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CleanerAuthentication,
                    format!("کلینر از قبل وارد شده است · {}", me.full_name()),
                )
                .await;
                return true;
            }
            Err(error) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CleanerAuthentication,
                    format!("وضعیت حساب کلینر خوانده نشد · {error}"),
                )
                .await;
                return true;
            }
        },
        Ok(false) => {}
        Err(error) => {
            super::respond(
                ctx,
                message,
                ResponseKind::CleanerAuthentication,
                format!("وضعیت نشست کلینر خوانده نشد · {error}"),
            )
            .await;
            return true;
        }
    }

    login(ctx, &user, message, ctx.api_hash()).await;
    true
}

async fn login(ctx: &Ctx, user: &Client, message: &Message, api_hash: &str) {
    use base64::Engine;

    let started = std::time::Instant::now();
    let mut shown = Vec::new();
    while started.elapsed() < DEADLINE {
        match user.qr_login_step(api_hash).await {
            Ok(grammers_client::QrLogin::Waiting { token, expires: _ }) => {
                if token != shown {
                    let url = format!(
                        "tg://login?token={}",
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&token)
                    );
                    send_qr(ctx, message, &url).await;
                    shown = token;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(grammers_client::QrLogin::PasswordRequired(token)) => {
                let hint = match token.hint() {
                    Some(hint) => format!("\nراهنما · {}", super::esc(hint)),
                    None => String::new(),
                };
                let password = ctx.expect_password();
                super::respond(
                    ctx,
                    message,
                    ResponseKind::PersonalInformation,
                    super::premium::html(format!(
                        "<b>رمز دو مرحله ای</b>\n\n\
                         کد اسکن شد. رمز دو مرحله ای حساب را همینجا بفرستید.{hint}\n\n\
                         <i>پیام رمز بلافاصله پاک می شود.</i>"
                    )),
                )
                .await;
                let Some(password) = ctx.await_password(password, DEADLINE).await else {
                    super::respond(
                        ctx,
                        message,
                        ResponseKind::CleanerAuthentication,
                        "رمزی نیامد. دوباره «ورود کلینر» را بفرستید.",
                    )
                    .await;
                    return;
                };
                match user.check_password(token, password.trim()).await {
                    Ok(me) => {
                        let Some(id) = me.id().bare_id() else {
                            super::respond(
                                ctx,
                                message,
                                ResponseKind::CleanerAuthentication,
                                "شناسه حساب کلینر معتبر نیست.",
                            )
                            .await;
                            return;
                        };
                        ctx.set_cleaner_id(id);
                        super::respond(
                            ctx,
                            message,
                            ResponseKind::CleanerAuthentication,
                            super::premium::text(format!(
                                "✓ کلینر وارد شد · {}",
                                super::esc(&me.full_name())
                            )),
                        )
                        .await;
                    }
                    Err(e) => {
                        super::respond(
                            ctx,
                            message,
                            ResponseKind::CleanerAuthentication,
                            super::premium::icon_text(
                                Some(super::premium::Icon::ErrorRed),
                                format!("انجام نشد · {e}"),
                            ),
                        )
                        .await;
                    }
                }
                return;
            }
            Ok(grammers_client::QrLogin::Success(me)) => {
                let Some(id) = me.id().bare_id() else {
                    super::respond(
                        ctx,
                        message,
                        ResponseKind::CleanerAuthentication,
                        "شناسه حساب کلینر معتبر نیست.",
                    )
                    .await;
                    return;
                };
                ctx.set_cleaner_id(id);
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CleanerAuthentication,
                    super::premium::text(format!(
                        "✓ کلینر وارد شد · {}",
                        super::esc(&me.full_name())
                    )),
                )
                .await;
                return;
            }
            Ok(grammers_client::QrLogin::SignUpRequired) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CleanerAuthentication,
                    "این شماره هنوز حساب تلگرام ندارد؛ کلینر باید یک حساب موجود باشد.",
                )
                .await;
                return;
            }
            Err(e) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CleanerAuthentication,
                    super::premium::icon_text(
                        Some(super::premium::Icon::ErrorRed),
                        format!("انجام نشد · {e}"),
                    ),
                )
                .await;
                return;
            }
        }
    }
    super::respond(
        ctx,
        message,
        ResponseKind::CleanerAuthentication,
        super::premium::icon_text(
            Some(super::premium::Icon::Timer),
            "زمان ورود تمام شد. «ورود کلینر» را دوباره بفرستید.",
        ),
    )
    .await;
}

pub async fn take_password(ctx: &Ctx, message: &Message) -> bool {
    let sender = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id);
    if sender.is_none() || sender != ctx.sudo_id() {
        return false;
    }
    let password = message.text().trim().to_owned();
    if password.is_empty() || !ctx.give_password(password) {
        return false;
    }
    let _ = message.delete().await;
    true
}

async fn send_qr(ctx: &Ctx, message: &Message, url: &str) {
    let mut card = super::premium::html(
        "<b>ورود کلینر</b>\n\n\
         در تلگرام · Settings › Devices › Link Desktop Device\n\
         و این کد را اسکن کنید.\n\n\
         <i>کد هر چند ثانیه تازه می شود.</i>",
    );
    if let Some(png) = qr_png(url) {
        let size = png.len();
        if let Ok(uploaded) = ctx
            .client
            .upload_stream(
                &mut std::io::Cursor::new(png),
                size,
                "cleaner-qr.png".to_owned(),
            )
            .await
        {
            card = card.photo(uploaded);
        }
    }
    super::respond(ctx, message, ResponseKind::PersonalInformation, card).await;
}

fn qr_png(url: &str) -> Option<Vec<u8>> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let image = code
        .render::<image::Luma<u8>>()
        .min_dimensions(512, 512)
        .build();
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

#[cfg(test)]
mod install_tests {
    #[test]
    fn wipe_uses_longest_alias_and_preserves_target() {
        for command in [
            "حذف پیام ها",
            "حذف پيام\u{200c}ها",
            "پاکسازی پیام های کاربر",
            "/WIPE",
        ] {
            assert_eq!(
                wipe_argument(&format!("{command} @Some_User")),
                Some("@Some_User")
            );
        }
        assert_eq!(wipe_argument("حذف پیام ها"), Some(""));
        assert_eq!(wipe_argument("حذف پیامهایم"), None);
        assert_eq!(wipe_argument("/wipe@otherbot @someone"), None);
        assert!(media_filter("ويدئو").is_some());
        assert!(media_filter("عکس های من").is_none());
    }
    use super::*;

    #[test]
    fn purge_all_deletes_by_author_through_the_shared_history_loop() {
        let source = include_str!("cleaner.rs");
        let start = source.find("async fn purge_by_authors").unwrap();
        let end = source[start..].find("\nfn stops_purge").unwrap() + start;
        let implementation = &source[start..end];
        assert!(implementation.contains("delete_history(client, chat_ref, target)"));
        assert!(implementation.contains("iter_messages(chat_ref)"));
        assert!(!implementation.contains("iter_participants"));
        assert!(!implementation.contains(".invoke_outbound("));
        assert!(!source.contains(concat!("channels::Delete", "History {")));
    }

    #[test]
    fn participant_history_is_deleted_until_telegram_has_no_offset_left() {
        let source = include_str!("cleaner.rs");
        let start = source.find("pub(crate) async fn delete_history(").unwrap();
        let end = source[start..].find("\npub async fn purge").unwrap() + start;
        let implementation = &source[start..end];
        assert!(implementation.contains("for _ in 0..WIPE_ROUNDS"));
        assert!(implementation.contains("done.offset != 0"));
        assert!(implementation.contains("invoke_outbound_critical"));
        assert_eq!(source.matches("DeleteParticipantHistory {").count(), 3);
        assert!(!include_str!("restrict.rs").contains("DeleteParticipantHistory {"));
    }

    #[test]
    fn a_wipe_reply_never_claims_what_did_not_happen() {
        let rpc = |name: &str| {
            grammers_client::InvocationError::Rpc(
                grammers_client::tl::types::RpcError {
                    error_code: 400,
                    error_message: name.to_owned(),
                }
                .into(),
            )
        };
        let history = |deleted, more, error| History {
            deleted,
            more,
            error,
        };
        assert!(history(0, false, None).reply("x").1.contains("پیدا نشد"));
        assert!(history(250, false, None).reply("x").1.contains("250 پیام"));
        assert!(history(3200, true, None).reply("x").1.contains("دوباره بفرستید"));
        let refused = history(0, false, Some(rpc("CHAT_ADMIN_REQUIRED"))).reply("x").1;
        assert!(refused.contains("حق حذف پیام"));
        assert!(!refused.contains("پاک شد"));
        let partial = history(100, true, Some(rpc("FLOOD_WAIT_X"))).reply("x").1;
        assert!(partial.contains("100 پیام") && partial.contains("بقیه انجام نشد"));
        assert_ne!(Missing::Cleaner.reply(), Missing::Chat.reply());
        assert_ne!(Missing::Chat.reply(), Missing::Member.reply());
    }

    #[test]
    fn short_join_waits_are_honored_but_long_or_unrelated_errors_are_not_retried() {
        let rpc = |message: &str| {
            grammers_client::InvocationError::Rpc(
                grammers_client::tl::types::RpcError {
                    error_code: 420,
                    error_message: message.to_owned(),
                }
                .into(),
            )
        };
        assert_eq!(
            join_retry_delay(&rpc("FLOOD_WAIT_4")),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            join_retry_delay(&rpc("FLOOD_WAIT_30")),
            Some(Duration::from_secs(31))
        );
        for message in [
            "FLOOD_WAIT_31",
            "FLOOD_WAIT_3600",
            "FLOOD_WAIT",
            "FLOOD_WAIT_0",
            "CHANNELS_TOO_MUCH",
            "SLOWMODE_WAIT_5",
        ] {
            assert!(join_retry_delay(&rpc(message)).is_none(), "{message}");
        }
        assert!(join_retry_delay(&grammers_client::InvocationError::Dropped).is_none());
    }

    #[test]
    fn a_long_flood_wait_stops_a_purge_like_a_missing_right() {
        let rpc = |code: i32, message: &str| {
            grammers_client::InvocationError::Rpc(
                grammers_client::tl::types::RpcError {
                    error_code: code,
                    error_message: message.to_owned(),
                }
                .into(),
            )
        };
        assert!(stops_purge(&rpc(420, "FLOOD_WAIT_600")));
        assert!(stops_purge(&rpc(400, "CHAT_ADMIN_REQUIRED")));
        assert!(!stops_purge(&rpc(400, "PARTICIPANT_ID_INVALID")));
        assert!(!stops_purge(&grammers_client::InvocationError::Dropped));
    }
}
