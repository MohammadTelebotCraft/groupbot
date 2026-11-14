use grammers_client::message::Message;
use grammers_client::peer::Role;
use grammers_client::session::types::{PeerId, PeerKind, PeerRef};
use grammers_client::tl;

use super::{Ctx, esc, name_of};
use crate::response::ResponseKind;

pub const LOCK: &str = "bot";

pub const KICK_ADDER: &str = "bot_kick_adder";

pub const EVEN_ADMINS: &str = "bot_even_admins";

pub const ALLOWED: &str = "botok:";

pub const NAMED: &str = "botname:";

pub const ADMITTED: &str = "botby:";

const PASSED_THE_OLD_DOOR: std::ops::Range<i64> = 0..1_793_483_182;

pub const ALLOW: &[&str] = &[
    "ربات مجاز",
    "افزودن ربات مجاز",
    "اضافه کردن ربات مجاز",
    "/allowbot",
    "مجاز کردن ربات",
    "/whitelistbot",
];
pub const DISALLOW: &[&str] = &[
    "حذف ربات مجاز",
    "لغو ربات مجاز",
    "برداشتن ربات مجاز",
    "/disallowbot",
    "/unwhitelistbot",
];

const SPARED_REPORT_MAX: usize = 512;

pub fn allow_key(user: i64) -> String {
    format!("{ALLOWED}{user}")
}

pub fn is_allowed(ctx: &Ctx, chat: i64, user: i64) -> bool {
    ctx.settings.is_locked(chat, &allow_key(user))
}

fn admitted_key(user: i64) -> String {
    format!("{ADMITTED}{user}")
}

fn is_admitted(ctx: &Ctx, chat: i64, user: i64) -> bool {
    ctx.settings.is_locked(chat, &admitted_key(user))
}

fn kept(ctx: &Ctx, chat: i64, user: i64) -> bool {
    is_allowed(ctx, chat, user) || super::vip::is_vip(ctx, chat, user)
}

async fn set_admitted(ctx: &Ctx, chat: i64, user: i64, on: bool) {
    if is_admitted(ctx, chat, user) == on {
        return;
    }
    if let Err(error) = ctx.settings.try_set(chat, &admitted_key(user), on).await {
        ::log::warn!("bot lock: {chat}: could not record admitted bot {user}: {error}");
    }
}

pub async fn allow(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some((adding, arg)) = parse_allow(text) else {
        return false;
    };
    let Some(named) = super::named(message, arg) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let Some((target, target_name)) = super::resolve(ctx, message, named).await else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.",
            ),
        )
        .await;
        return true;
    };
    let Some(target_id) = target.id.bare_id() else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(Some(super::premium::Icon::ErrorRed), "کاربر پیدا نشد."),
        )
        .await;
        return true;
    };

    let changed = match ctx
        .settings
        .try_set(chat, &allow_key(target_id), adding)
        .await
    {
        Ok(changed) => changed,
        Err(error) => {
            ::log::warn!("allowed bots: write for {chat}/{target_id} failed: {error}");
            super::respond(ctx, message, ResponseKind::CommandError, if error.commit_outcome_unknown() {
                    "نتیجه ذخیره فهرست ربات های مجاز نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "فهرست ربات های مجاز ذخیره نشد؛ دوباره تلاش کنید."
                }).await;
            return true;
        }
    };
    if !adding && changed && ctx.settings.is_locked(chat, LOCK) {
        on_lock_set(ctx, chat, LOCK, true).await;
    }
    let mark = if adding { "✓" } else { "✗" };
    let what = match (adding, changed) {
        (true, true) => "به رباتای مجاز اضافه شد",
        (true, false) => "از قبل مجاز بود",
        (false, true) => "از رباتای مجاز حذف شد",
        (false, false) => "مجاز نبود",
    };
    super::respond(
        ctx,
        message,
        ResponseKind::AdminTool,
        super::premium::icon_text(
            Some(super::premium::Icon::Bot),
            format!("{mark} {target_name} {what}."),
        ),
    )
    .await;
    true
}

fn parse_allow(text: &str) -> Option<(bool, Option<&str>)> {
    for (commands, adding) in [(ALLOW, true), (DISALLOW, false)] {
        for command in commands {
            let Some(rest) = super::commands::strip_prefix(text, command) else {
                continue;
            };
            let rest = rest.trim();
            if rest.is_empty() {
                return Some((adding, None));
            }
            if rest.split_whitespace().count() == 1 && super::arg_names_a_user(rest) {
                return Some((adding, Some(rest)));
            }
        }
    }
    None
}

pub async fn on_participant_update(
    ctx: &std::sync::Arc<Ctx>,
    update: &tl::types::UpdateChannelParticipant,
) {
    use tl::enums::ChannelParticipant as P;

    let Some(chat) = PeerId::channel(update.channel_id).and_then(PeerId::bot_api_dialog_id) else {
        return;
    };

    let inside = |participant: &Option<P>| {
        matches!(
            participant,
            Some(P::Participant(_) | P::ParticipantSelf(_) | P::Admin(_) | P::Creator(_))
        )
    };
    let holds = |participant: &Option<P>| matches!(participant, Some(P::Admin(_) | P::Creator(_)));
    let (user, actor) = (update.user_id, update.actor_id);
    if !inside(&update.new_participant) {
        if inside(&update.prev_participant) {
            set_admitted(ctx, chat, user, false).await;
        }
        return;
    }
    let demoted = holds(&update.prev_participant) && !holds(&update.new_participant);
    if inside(&update.prev_participant) && !demoted {
        return;
    }
    let locked = ctx.settings.is_locked(chat, LOCK);
    if demoted && !locked {
        return;
    }

    let Some(chat_ref) = ctx
        .chat_ref(chat)
        .or_else(|| PeerId::channel(update.channel_id).map(PeerId::to_ambient_ref))
    else {
        return;
    };
    let Some(peer) = PeerId::user(user).map(PeerId::to_ambient_ref) else {
        return;
    };

    let known = ctx.known_bot(user).await;
    if known == Some(false) || (known.is_none() && !locked) {
        return;
    }
    let resolve = || async {
        match ctx.client.resolve_peer(peer).await {
            Ok(grammers_client::peer::Peer::User(joined)) if joined.is_bot() => Some(joined),
            _ => None,
        }
    };
    let mut joined = None;
    if known.is_none() {
        joined = resolve().await;
        if joined.is_none() {
            return;
        }
    }

    let by_admin = if demoted {
        is_admitted(ctx, chat, user)
    } else {
        actor != user && super::is_admin(ctx, chat_ref, chat, actor).await
    };
    if by_admin && !demoted {
        set_admitted(ctx, chat, user, true).await;
    }
    if !locked || kept(ctx, chat, user) {
        return;
    }
    if by_admin && !ctx.settings.is_locked(chat, EVEN_ADMINS) {
        return;
    }

    let joined = match joined {
        Some(joined) => joined,
        None => match resolve().await {
            Some(joined) => joined,
            None => return,
        },
    };

    let full_name = joined.full_name();
    if remove(ctx, chat, chat_ref, peer, &full_name, "قفل ربات").await != Removal::Done {
        return;
    }
    super::cleaner::wipe_user(ctx, chat, user).await;

    let name = esc(&full_name);
    if demoted {
        let _ = ctx
            .client
            .send_message(
                chat_ref,
                super::premium::html(format!("✗ قفل ربات فعال است. {name} حذف شد.")),
            )
            .await;
        return;
    }
    let removed_adder = ctx.settings.is_locked(chat, KICK_ADDER)
        && actor != user
        && kick(ctx, chat, chat_ref, actor).await;

    let _ = ctx
        .client
        .send_message(
            chat_ref,
            super::premium::html(if removed_adder {
                format!("✗ افزودن ربات ممنوع است. {name} حذف شد و اضافه کننده اش از گروه اخراج شد.")
            } else {
                format!("✗ افزودن ربات ممنوع است. {name} حذف شد.")
            }),
        )
        .await;
}

async fn remove(
    ctx: &Ctx,
    chat: i64,
    chat_ref: PeerRef,
    target: PeerRef,
    name: &str,
    reason: &'static str,
) -> Removal {
    if let Some(user) = target.id.bare_id()
        && !ctx.claim_bot_removal(chat, user)
    {
        return Removal::Duplicate;
    }
    let done = super::restrict::apply(
        ctx,
        chat_ref,
        target,
        super::restrict::Action::Ban,
        None,
        super::restrict::By {
            reason,
            target_name: name,
            case: Some(super::cases::CaseContext {
                source: "automatic",
                rule: LOCK,
                reason,
                evidence: None,
            }),
            admins_too: true,
            ..Default::default()
        },
    )
    .await;
    match done {
        Ok(_) => Removal::Done,
        Err(e) => {
            eprintln!("bot lock: {chat}: could not ban bot {}: {e}", target.id);
            if lacks_ban_right(&e) && ctx.claim_bot_lock_rights_notice(chat) {
                let _ = ctx
                    .client
                    .send_message(
                        chat_ref,
                        "✗ قفل ربات نمی تواند ربات ها را بن کند. دسترسی «مسدود کردن کاربران» را به ربات بدهید.",
                    )
                    .await;
            }
            Removal::Failed
        }
    }
}

fn lacks_ban_right(error: &super::restrict::Failed) -> bool {
    matches!(
        error,
        super::restrict::Failed::Telegram(grammers_client::InvocationError::Rpc(rpc))
            if matches!(rpc.name.as_str(), "CHAT_ADMIN_REQUIRED" | "RIGHT_FORBIDDEN")
    )
}

#[derive(Clone, Copy, PartialEq)]
enum Removal {
    Done,
    Duplicate,
    Failed,
}

pub async fn on_lock_set(ctx: &Ctx, chat: i64, key: &str, on: bool) {
    if (key == LOCK || key == EVEN_ADMINS) && on {
        let _permit = ctx.sweep_slot().await;
        sweep(ctx, chat, false).await;
    }
}

pub fn cleaner_candidate(ctx: &Ctx, message: &Message) -> bool {
    if message.outgoing() || message.peer_id().kind() != PeerKind::Channel {
        return false;
    }
    if message.peer_id().bot_api_dialog_id().is_none() {
        return false;
    }
    if !super::bot_authored(message) {
        return false;
    }
    match message.sender_id().and_then(PeerId::bare_id) {
        Some(user) => user != ctx.me_id() && !ctx.is_cleaner(user),
        None => false,
    }
}

async fn spare_reason(ctx: &Ctx, chat: i64, user: i64) -> Option<&'static str> {
    if is_allowed(ctx, chat, user) {
        return Some("allowed");
    }
    if super::vip::is_vip(ctx, chat, user) {
        return Some("vip");
    }
    if is_admitted(ctx, chat, user) {
        return Some("added by an admin");
    }
    if super::owner(ctx, chat) == Some(user) {
        return Some("owner");
    }
    if super::is_bot_admin(ctx, chat, user) {
        return Some("bot admin");
    }
    if let Some(admin) = ctx.cached_admin(chat, user) {
        return admin.then_some("admin");
    }
    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return Some("no chat ref");
    };
    match super::chat_admins(ctx, chat_ref, chat).await {
        Some(admins) => admins.contains(&user).then_some("admin"),
        None => match holds_the_group_now(ctx, chat_ref, user).await {
            Some(holds) => holds.then_some("admin"),
            None => Some("admin list unavailable"),
        },
    }
}

async fn holds_the_group_now(ctx: &Ctx, chat_ref: PeerRef, user: i64) -> Option<bool> {
    use tl::enums::ChannelParticipant as P;
    let target = ctx
        .with_session_auth(PeerId::user(user)?.to_ambient_ref())
        .await;
    let found = participant_now(ctx, chat_ref, target).await.ok()?;
    Some(matches!(found, Some(P::Admin(_) | P::Creator(_))))
}

async fn participant_now(
    ctx: &Ctx,
    chat_ref: PeerRef,
    target: PeerRef,
) -> Result<Option<tl::enums::ChannelParticipant>, ()> {
    let asked = ctx
        .client
        .invoke(&tl::functions::channels::GetParticipant {
            channel: chat_ref.into(),
            participant: target.into(),
        })
        .await;
    match asked {
        Ok(tl::enums::channels::ChannelParticipant::Participant(found)) => {
            Ok(Some(found.participant))
        }
        Err(grammers_client::InvocationError::Rpc(rpc)) if rpc.name == "USER_NOT_PARTICIPANT" => {
            Ok(None)
        }
        Err(_) => Err(()),
    }
}

fn is_banned(participant: &Option<tl::enums::ChannelParticipant>) -> bool {
    let Some(tl::enums::ChannelParticipant::Banned(banned)) = participant else {
        return false;
    };
    let tl::enums::ChatBannedRights::Rights(rights) = &banned.banned_rights;
    rights.view_messages
}

async fn kick_caller(ctx: &Ctx, chat: i64, chat_ref: PeerRef, caller: i64, name: &str, bot: i64) {
    if !ctx.claim_bot_removal(chat, caller) || !kick(ctx, chat, chat_ref, caller).await {
        return;
    }
    println!("bot lock: {chat}: kicked {caller}, who called {bot}");
    let _ = ctx
        .client
        .send_message(
            chat_ref,
            super::premium::html(format!(
                "✗ صدا زدن ربات ممنوع است. {} از گروه اخراج شد.",
                esc(name)
            )),
        )
        .await;
}

async fn ban_caller(ctx: &Ctx, chat: i64, chat_ref: PeerRef, caller: i64, name: &str, bot: i64) {
    let Some(target) = PeerId::user(caller).map(PeerId::to_ambient_ref) else {
        return;
    };
    if !ctx.claim_bot_removal(chat, caller) {
        return;
    }
    let reason = "صدا زدن ربات";
    let banned = super::restrict::apply(
        ctx,
        chat_ref,
        target,
        super::restrict::Action::Ban,
        None,
        super::restrict::By {
            reason,
            target_name: name,
            case: Some(super::cases::CaseContext {
                source: "automatic",
                rule: LOCK,
                reason,
                evidence: None,
            }),
            ..Default::default()
        },
    )
    .await;
    if let Err(e) = banned {
        eprintln!("bot lock: {chat}: could not ban {caller}, who called {bot}: {e}");
        return;
    }
    println!("bot lock: {chat}: banned {caller}, who called {bot} again");
    let _ = ctx
        .client
        .send_message(
            chat_ref,
            super::premium::html(format!(
                "✗ صدا زدن ربات ممنوع است. {} از گروه بن شد.",
                esc(name)
            )),
        )
        .await;
}

fn only_calls(text: &str) -> bool {
    let mut words = text.split_whitespace().peekable();
    words.peek().is_some() && words.all(|word| tagged_bots(word).len() == 1 && word.starts_with('@'))
}

async fn ban_if_calling_the_banned(
    ctx: &Ctx,
    chat: i64,
    chat_ref: PeerRef,
    caller: i64,
    name: &str,
    bots: &[String],
) {
    for username in bots.iter().take(3) {
        let Ok(Some(peer)) = ctx.client.resolve_username(username).await else {
            continue;
        };
        if !matches!(&peer, grammers_client::peer::Peer::User(bot) if bot.is_bot()) {
            continue;
        }
        let Ok(Some(target)) = peer.to_ref().await else {
            continue;
        };
        if participant_now(ctx, chat_ref, target)
            .await
            .is_ok_and(|found| is_banned(&found))
        {
            let bot = target.id.bare_id().unwrap_or_default();
            ban_caller(ctx, chat, chat_ref, caller, name, bot).await;
            return;
        }
    }
}

#[derive(Default)]
pub struct Calls(std::sync::Mutex<std::collections::VecDeque<Call>>);

struct Call {
    at: std::time::Instant,
    message: i32,
    caller: i64,
    name: String,
    spared: bool,
    bots: Vec<String>,
}

const CALLS_MAX: usize = 16;
const CALL_TTL: std::time::Duration = std::time::Duration::from_secs(60);
const CALLED_MAX: usize = 8;

impl Calls {
    fn note(&self, call: Call) {
        let mut calls = self.0.lock().unwrap();
        while calls.len() >= CALLS_MAX {
            calls.pop_front();
        }
        calls.push_back(call);
    }

    fn caller(
        &self,
        reply_to: Option<i32>,
        bot: Option<&str>,
        now: std::time::Instant,
    ) -> Option<(i64, String, bool)> {
        let calls = self.0.lock().unwrap();
        let fresh = || {
            calls
                .iter()
                .rev()
                .filter(|call| now.saturating_duration_since(call.at) < CALL_TTL)
        };
        let tagged = |call: &&Call| bot.is_some_and(|bot| call.bots.iter().any(|b| b == bot));
        fresh()
            .find(|call| Some(call.message) == reply_to)
            .or_else(|| fresh().find(tagged))
            .map(|call| (call.caller, call.name.clone(), call.spared))
    }
}

fn tagged_bots(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '@'))
        .filter_map(|word| word.strip_prefix('@'))
        .filter(|name| {
            (5..=32).contains(&name.len())
                && name.starts_with(|c: char| c.is_ascii_alphabetic())
                && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                && name.to_ascii_lowercase().ends_with("bot")
        })
        .map(str::to_ascii_lowercase)
        .take(CALLED_MAX)
        .collect()
}

#[derive(Debug, PartialEq)]
enum Invoker {
    Member(i64),
    Anonymous,
}

fn invoker(via: Option<&tl::enums::Peer>, group: &tl::enums::Peer) -> Option<Invoker> {
    match via? {
        tl::enums::Peer::User(user) => Some(Invoker::Member(user.user_id)),
        peer if peer == group => Some(Invoker::Anonymous),
        _ => None,
    }
}

pub async fn note_call(
    ctx: &std::sync::Arc<Ctx>,
    state: &super::ChatState,
    chat: i64,
    message: &Message,
) {
    if super::bot_authored(message) {
        return;
    }
    let bots = tagged_bots(message.text());
    if bots.is_empty() || !ctx.settings.is_locked(chat, LOCK) {
        return;
    }
    let Some(caller) = message.sender_id().and_then(PeerId::bare_id) else {
        return;
    };
    if ctx.is_cleaner(caller) {
        return;
    }
    let spared = super::is_exempt(ctx, message).await;
    let name = name_of(message);
    if !spared
        && ctx.settings.is_locked(chat, KICK_ADDER)
        && only_calls(message.text())
        && let Some(chat_ref) = ctx.chat_ref(chat)
        && ctx.claim_bot_call(chat, caller)
    {
        let (task, name, bots) = (std::sync::Arc::clone(ctx), name.clone(), bots.clone());
        ctx.spawn_owned(async move {
            ban_if_calling_the_banned(&task, chat, chat_ref, caller, &name, &bots).await;
        });
    }
    state.calls.note(Call {
        at: std::time::Instant::now(),
        message: message.id(),
        caller,
        name,
        spared,
        bots,
    });
}

pub static CLEANER_MESSAGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub async fn on_cleaner_message(ctx: &Ctx, message: &Message) {
    judge_post(ctx, message, &super::locks::View::new(message), true).await;
}

pub async fn on_bot_post(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    judge_post(ctx, message, view, false).await
}

async fn judge_post(
    ctx: &Ctx,
    message: &Message,
    view: &super::locks::View<'_>,
    cleaner: bool,
) -> bool {
    if !cleaner_candidate(ctx, message) {
        return false;
    }
    let (Some(chat), Some(user)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_id().and_then(PeerId::bare_id),
    ) else {
        return false;
    };
    if cleaner && super::install::left_or_forgotten(ctx, chat) {
        return false;
    }

    let locked = ctx.settings.is_locked(chat, LOCK);
    let even_admins = ctx.settings.is_locked(chat, EVEN_ADMINS);
    let allowed = kept(ctx, chat, user);
    let tripped = if locked {
        None
    } else {
        super::locks::scan(ctx, chat, view)
    };
    let username = match message.sender() {
        Some(grammers_client::peer::Peer::User(sender)) => {
            sender.username().map(str::to_ascii_lowercase)
        }
        _ => None,
    };
    let noted = ctx.peek(chat).and_then(|state| {
        state.calls.caller(
            message.reply_to_message_id(),
            username.as_deref(),
            std::time::Instant::now(),
        )
    });
    let named = match &message.raw {
        tl::enums::Message::Message(raw) => invoker(raw.guestchat_via_from.as_ref(), &raw.peer_id),
        _ => None,
    };
    let call = match named {
        Some(Invoker::Member(caller)) => match noted {
            Some(call) if call.0 == caller => Some(call),
            _ => {
                let spared = super::vip::is_vip(ctx, chat, caller)
                    || match ctx.chat_ref(chat) {
                        Some(chat_ref) => super::is_admin(ctx, chat_ref, chat, caller).await,
                        None => true,
                    };
                Some((caller, caller.to_string(), spared))
            }
        },
        Some(Invoker::Anonymous) => Some((0, String::new(), true)),
        None => noted,
    };
    let needed = !allowed && if locked { !even_admins } else { tripped.is_some() };
    let spare = match () {
        () if !needed => None,
        () if matches!(call, Some((_, _, true))) => Some("called by an exempt member"),
        () => spare_reason(ctx, chat, user).await,
    };

    let (reason, ban) = match post_verdict(locked, even_admins, allowed, spare.is_some(), tripped) {
        Verdict::Keep => {
            if (locked || tripped.is_some()) && ctx.claim_bot_post_log(chat, user) {
                let why = match () {
                    () if is_allowed(ctx, chat, user) => "allowed",
                    () if allowed => "vip",
                    () => spare.unwrap_or("clean post"),
                };
                println!("bot lock: {chat}: kept a post by {user} ({why})");
            }
            return false;
        }
        Verdict::Delete(reason) => (reason, false),
        Verdict::Ban(reason) => (reason, true),
    };
    if let Err(e) = message.delete_critical().await {
        eprintln!("bot lock: {chat}: could not delete a bot's message: {e}");
        if !ban {
            return true;
        }
    }
    let name = super::name_of(message);
    if !locked {
        ctx.bump(chat, super::stats::DELETED);
        super::log::write(
            ctx,
            chat,
            "log_del",
            super::log::Entry {
                title: "حذف پیام ربات",
                target: Some((user, &name)),
                reason: Some(reason),
                ..Default::default()
            },
        )
        .await;
    }
    if !ban {
        return true;
    }
    let Some(chat_ref) = ctx.chat_ref(chat) else {
        eprintln!("bot lock: {chat}: saw a bot post but have no ref to ban with");
        return true;
    };
    let removal = match bot_ref(ctx, message, user).await {
        Some(target) => remove(ctx, chat, chat_ref, target, &name, reason).await,
        None => {
            eprintln!("bot lock: {chat}: could not resolve bot {user} for the bot account");
            if ctx.claim_bot_removal(chat, user) {
                Removal::Failed
            } else {
                Removal::Duplicate
            }
        }
    };
    let banned = match removal {
        Removal::Done => true,
        Removal::Duplicate => return true,
        Removal::Failed if cleaner => ban_as_cleaner(ctx, chat, message, &name, reason).await,
        Removal::Failed => false,
    };
    if banned {
        super::cleaner::wipe_user(ctx, chat, user).await;
        println!("bot lock: {chat}: banned {user} for posting ({reason})");
        if let Some((caller, caller_name, false)) = call
            && locked
            && ctx.settings.is_locked(chat, KICK_ADDER)
        {
            kick_caller(ctx, chat, chat_ref, caller, &caller_name, user).await;
        }
    }
    true
}

async fn ban_as_cleaner(
    ctx: &Ctx,
    chat: i64,
    message: &Message,
    name: &str,
    reason: &'static str,
) -> bool {
    let (Some(cleaner), Ok(Some(chat_ref)), Ok(Some(target))) = (
        ctx.user_client(),
        message.peer_ref().await,
        message.sender_ref().await,
    ) else {
        return false;
    };
    if let Err(e) = super::restrict::ban_member_as(ctx, &cleaner, chat, chat_ref, target).await {
        eprintln!("bot lock: {chat}: the cleaner could not ban bot {} either: {e}", target.id);
        return false;
    }
    if let Some(user) = target.id.bare_id() {
        super::log::write(
            ctx,
            chat,
            "log_mod",
            super::log::Entry {
                title: "بن ربات",
                target: Some((user, name)),
                reason: Some(reason),
                ..Default::default()
            },
        )
        .await;
    }
    true
}

async fn bot_ref(ctx: &Ctx, message: &Message, user: i64) -> Option<PeerRef> {
    use grammers_client::session::types::PeerInfo;
    let id = PeerId::user(user)?;
    if let Ok(Some(PeerInfo::User {
        auth: Some(auth), ..
    })) = ctx.bot_session.peer(id).await
    {
        return Some(PeerRef { id, auth });
    }
    let Some(grammers_client::peer::Peer::User(sender)) = message.sender() else {
        return None;
    };
    let username = sender.username()?;
    match ctx.client.resolve_username(username).await {
        Ok(Some(peer)) if peer.id() == id => peer.to_ref().await.ok().flatten(),
        _ => None,
    }
}

#[derive(Debug, PartialEq)]
enum Verdict {
    Keep,
    Delete(&'static str),
    Ban(&'static str),
}

fn post_verdict(
    locked: bool,
    even_admins: bool,
    allowed: bool,
    on_sight: bool,
    tripped: Option<&'static str>,
) -> Verdict {
    match () {
        () if allowed => Verdict::Keep,
        () if locked && on_sight && !even_admins => Verdict::Keep,
        () if locked => Verdict::Ban("قفل ربات"),
        () => match tripped {
            None => Verdict::Keep,
            Some(_) if on_sight && !even_admins => Verdict::Keep,
            Some(reason) if on_sight => Verdict::Delete(reason),
            Some(reason) => Verdict::Ban(reason),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn spared_because(
    is_bot: bool,
    is_me: bool,
    is_cleaner: bool,
    is_admin: bool,
    even_admins: bool,
    vip: bool,
    allowed: bool,
    admitted: bool,
) -> Option<&'static str> {
    match () {
        () if !is_bot => Some("not a bot"),
        () if is_me => Some("myself"),
        () if is_cleaner => Some("the cleaner"),
        () if allowed => Some("allowed"),
        () if vip => Some("vip"),
        () if is_admin && !even_admins => Some("admin"),
        () if admitted && !even_admins => Some("added by an admin"),
        () => None,
    }
}

pub async fn sweep(ctx: &Ctx, chat: i64, inherit: bool) -> usize {
    if !ctx.settings.is_locked(chat, LOCK) {
        return 0;
    }
    if ctx.me_id() == 0 {
        eprintln!("bot lock: {chat}: skipping the sweep, own id still unknown");
        return 0;
    }
    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return 0;
    };
    if chat_ref.id.kind() != PeerKind::Channel {
        return 0;
    }

    let even_admins = ctx.settings.is_locked(chat, EVEN_ADMINS);
    let mut participants = ctx
        .client
        .iter_participants(chat_ref)
        .filter(tl::enums::ChannelParticipantsFilter::ChannelParticipantsBots);
    let mut doomed: Vec<(i64, PeerRef, String)> = Vec::new();
    let mut seen = 0;
    let mut spared: Vec<String> = Vec::new();
    loop {
        match participants.next().await {
            Ok(Some(participant)) => {
                seen += 1;
                let Some(user) = participant
                    .id()
                    .bare_id()
                    .filter(|_| participant.id().kind() == PeerKind::User)
                else {
                    continue;
                };
                let Some(expanded) = participant.user() else {
                    if spared.len() < SPARED_REPORT_MAX {
                        spared.push(format!("{user} (missing peer data)"));
                    }
                    continue;
                };
                let is_admin = matches!(&participant.role, Role::Admin(_) | Role::Creator(_));
                let inherited = inherit
                    && expanded.is_bot()
                    && matches!(&participant.role, Role::User(normal)
                        if PASSED_THE_OLD_DOOR.contains(&normal.date().as_second()));
                if inherited && !is_admitted(ctx, chat, user) {
                    set_admitted(ctx, chat, user, true).await;
                }
                let why = spared_because(
                    expanded.is_bot(),
                    user == ctx.me_id(),
                    ctx.is_cleaner(user),
                    is_admin,
                    even_admins,
                    super::vip::is_vip(ctx, chat, user),
                    is_allowed(ctx, chat, user),
                    is_admitted(ctx, chat, user),
                );
                if let Some(why) = why {
                    if spared.len() < SPARED_REPORT_MAX {
                        spared.push(format!("{user} ({why})"));
                    }
                    continue;
                }
                let Some(target) = PeerId::user(user).map(PeerId::to_ambient_ref) else {
                    spared.push(format!("{user} (no ref)"));
                    continue;
                };
                doomed.push((user, target, expanded.full_name()));
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("bot lock: {chat}: could not list bots: {e}");
                break;
            }
        }
    }
    let logged = from_the_log(ctx, chat, even_admins, &mut doomed).await;
    let removed = remove_all(ctx, chat, chat_ref, doomed).await;
    println!(
        "bot lock: {chat}: {seen} bot(s) listed, {logged} more from the moderation log, \
         {removed} removed{}",
        if spared.is_empty() {
            String::new()
        } else {
            format!(", spared {}", spared.join(", "))
        }
    );
    removed
}

const LOG_WINDOW_SECS: i64 = 7 * 86_400;
const LOG_SWEEP_MAX: i64 = 64;

async fn from_the_log(
    ctx: &Ctx,
    chat: i64,
    even_admins: bool,
    doomed: &mut Vec<(i64, PeerRef, String)>,
) -> usize {
    let since = super::tempmedia::unix_now() - LOG_WINDOW_SECS;
    let subjects = match ctx
        .settings
        .unbanned_case_subjects(chat, since, LOCK, LOG_SWEEP_MAX)
        .await
    {
        Ok(subjects) => subjects,
        Err(e) => {
            eprintln!("bot lock: {chat}: could not read the moderation log: {e}");
            return 0;
        }
    };
    let mut added = 0;
    for (user, name) in subjects {
        if user == ctx.me_id()
            || ctx.is_cleaner(user)
            || doomed.iter().any(|(id, ..)| *id == user)
            || ctx.known_bot(user).await != Some(true)
        {
            continue;
        }
        let allowed = kept(ctx, chat, user);
        let on_sight = !allowed && !even_admins && spare_reason(ctx, chat, user).await.is_some();
        if !matches!(post_verdict(true, even_admins, allowed, on_sight, None), Verdict::Ban(_)) {
            continue;
        }
        if let Some(target) = PeerId::user(user).map(PeerId::to_ambient_ref) {
            doomed.push((user, target, name));
            added += 1;
        }
    }
    added
}

async fn remove_all(
    ctx: &Ctx,
    chat: i64,
    chat_ref: PeerRef,
    bots: Vec<(i64, PeerRef, String)>,
) -> usize {
    let removed = std::sync::atomic::AtomicUsize::new(0);
    super::bounded(bots, super::FLEET_CAMPAIGNS, |(user, target, name)| {
        let removed = &removed;
        async move {
            if remove(ctx, chat, chat_ref, target, &name, "قفل ربات").await == Removal::Done {
                super::cleaner::wipe_user(ctx, chat, user).await;
                removed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    })
    .await;
    removed.into_inner()
}

async fn kick(ctx: &Ctx, chat: i64, chat_ref: PeerRef, user: i64) -> bool {
    let Some(peer) = PeerId::user(user).map(PeerId::to_ambient_ref) else {
        return false;
    };
    match super::restrict::kick_member(ctx, chat_ref, peer).await {
        Ok(()) => true,
        Err(e) => {
            eprintln!("bot lock: {chat}: could not kick adder {user}: {e}");
            false
        }
    }
}

pub async fn handle(ctx: &std::sync::Arc<Ctx>, message: &Message) -> bool {
    let joined_by_link = matches!(
        message.action(),
        Some(tl::enums::MessageAction::ChatJoinedByLink(_))
    );
    if !joined_by_link
        && !matches!(
            message.action(),
            Some(tl::enums::MessageAction::ChatAddUser(_))
        )
    {
        return false;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !ctx.settings.is_locked(chat, LOCK) {
        return false;
    }

    if !ctx.settings.is_locked(chat, EVEN_ADMINS) && super::is_exempt(ctx, message).await {
        return false;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    let bots: Vec<super::Joined> = super::joined_users(ctx, message)
        .await
        .into_iter()
        .filter(|joined| joined.is_bot && !kept(ctx, chat, joined.id))
        .collect();
    let removed = remove_all(
        ctx,
        chat,
        chat_ref,
        bots.into_iter()
            .map(|joined| (joined.id, joined.peer, joined.name))
            .collect(),
    )
    .await;
    if removed == 0 {
        return false;
    }

    let adder = name_of(message);
    let removed_adder = if ctx.settings.is_locked(chat, KICK_ADDER) {
        match message.sender_ref().await {
            Ok(Some(sender)) => match super::restrict::kick_member(ctx, chat_ref, sender).await {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("bot lock: {chat}: could not kick adder: {e}");
                    false
                }
            },
            _ => false,
        }
    } else {
        false
    };

    let _ = message
        .respond(if removed_adder {
            format!("✗ افزودن ربات ممنوع است. ربات حذف شد و {adder} از گروه اخراج شد.")
        } else {
            format!("✗ افزودن ربات ممنوع است. ربات حذف شد. ({adder})")
        })
        .await;
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn allow_aliases_accept_only_one_explicit_target() {
        assert_eq!(
            parse_allow("اضافه\tکردن ربات مجاز @Some_Bot"),
            Some((true, Some("@Some_Bot")))
        );
        assert_eq!(
            parse_allow("/ALLOWBOT @Some_Bot"),
            Some((true, Some("@Some_Bot")))
        );
        assert_eq!(parse_allow("حذف ربات مجاز ۱۲٣"), Some((false, Some("۱۲٣"))));
        for text in [
            "ربات مجاز است",
            "ربات مجاز @one @two",
            "ربات مجاز @one\t@two",
            "/allowbot@otherbot @one",
        ] {
            assert_eq!(parse_allow(text), None, "{text}");
        }
    }
    use super::*;

    #[test]
    fn the_sweep_never_takes_its_own_side() {
        let plain = spared_because(true, false, false, false, false, false, false, false);
        assert_eq!(plain, None, "a plain bot goes");

        assert_eq!(
            spared_because(false, false, false, false, false, false, false, false),
            Some("not a bot")
        );
        assert_eq!(
            spared_because(true, true, false, false, false, false, false, false),
            Some("myself")
        );
        assert_eq!(
            spared_because(true, false, true, false, false, false, false, false),
            Some("the cleaner")
        );
        assert_eq!(
            spared_because(true, false, false, false, false, true, false, false),
            Some("vip")
        );
        assert_eq!(
            spared_because(true, false, false, false, false, false, true, false),
            Some("allowed"),
            "a bot on the allowlist stays"
        );
        assert_eq!(
            spared_because(true, false, false, true, false, false, false, false),
            Some("admin"),
            "an admin bot is spared unless the lock says otherwise"
        );
        assert_eq!(
            spared_because(true, false, false, true, true, false, false, false),
            None,
            "with «اعمال روی ادمین ها هم» an admin bot goes too"
        );
        assert_eq!(
            spared_because(true, false, false, true, true, false, true, false),
            Some("allowed"),
            "the allowlist outranks «اعمال روی ادمین ها هم» — it is the explicit exception"
        );

        assert_eq!(
            spared_because(true, true, false, true, true, false, false, false),
            Some("myself")
        );

        assert_eq!(
            spared_because(true, false, false, false, false, false, false, true),
            Some("added by an admin"),
            "a bot an admin added stays, though it holds no rights"
        );
        assert_eq!(
            spared_because(true, false, false, false, true, false, false, true),
            None,
            "with «اعمال روی ادمین ها هم» a bot an admin added goes too"
        );
    }

    #[test]
    fn a_tag_names_only_bot_usernames() {
        assert_eq!(
            tagged_bots("@Dhm_5efaec58bot  @Dhm_cb7da654bot  @Dhm_ae3ac6f6bot"),
            ["dhm_5efaec58bot", "dhm_cb7da654bot", "dhm_ae3ac6f6bot"]
        );
        assert_eq!(tagged_bots("ask (@TriviaBot), not @someone"), ["triviabot"]);
        for text in ["mail me at x@ybot.org", "@bot", "@1abcbot", "@a@bcbot", "no tags", "@ab_cd"] {
            assert!(tagged_bots(text).is_empty(), "{text}");
        }
        assert_eq!(tagged_bots(&"@abcdbot ".repeat(20)).len(), CALLED_MAX);

        assert!(only_calls("@Dhm_ae3ac6f6bot  @Dhm_8e073a05bot  @Dhm_cb7da654bot"));
        assert!(only_calls("@TriviaBot"));
        for text in ["@xbot_is spamming", "@Dhm_ae3ac6f6bot is spam", "@someone", "", "  "] {
            assert!(!only_calls(text), "{text:?}");
        }
    }

    #[test]
    fn an_answer_finds_the_member_who_called_it() {
        let calls = Calls::default();
        let start = std::time::Instant::now();
        let call = |at: u64, message: i32, caller: i64, spared: bool, bots: &[&str]| Call {
            at: start + std::time::Duration::from_secs(at),
            message,
            caller,
            name: format!("member {caller}"),
            spared,
            bots: bots.iter().map(|b| b.to_string()).collect(),
        };
        calls.note(call(0, 10, 1, false, &["abot", "bbot"]));
        calls.note(call(5, 11, 2, true, &["abot"]));
        let at = |secs: u64| start + std::time::Duration::from_secs(secs);

        assert_eq!(calls.caller(Some(10), Some("abot"), at(6)).map(|c| c.0), Some(1));
        assert_eq!(calls.caller(None, Some("abot"), at(6)), Some((2, "member 2".into(), true)));
        assert_eq!(calls.caller(Some(99), Some("bbot"), at(6)).map(|c| c.0), Some(1));
        assert_eq!(calls.caller(None, Some("cbot"), at(6)), None);
        assert_eq!(calls.caller(None, None, at(6)), None);
        assert_eq!(calls.caller(Some(10), Some("abot"), at(70)), None);

        for n in 0..CALLS_MAX as i32 {
            calls.note(call(6, 100 + n, 3, false, &["zbot"]));
        }
        assert_eq!(calls.caller(Some(10), Some("abot"), at(7)), None, "the oldest go first");
    }

    #[test]
    fn a_guest_answer_names_its_caller() {
        use tl::enums::Peer;
        let group: Peer = tl::types::PeerChannel { channel_id: 7 }.into();
        let member: Peer = tl::types::PeerUser { user_id: 42 }.into();
        let other: Peer = tl::types::PeerChannel { channel_id: 8 }.into();
        assert_eq!(invoker(Some(&member), &group), Some(Invoker::Member(42)));
        assert_eq!(invoker(Some(&group), &group), Some(Invoker::Anonymous));
        assert_eq!(invoker(Some(&other), &group), None, "a channel names no member");
        assert_eq!(invoker(None, &group), None, "not a guest bot's answer");
    }

    #[test]
    fn a_post_bans_the_bot_unless_something_spares_it() {
        use Verdict::*;
        let link = Some("link");
        assert_eq!(post_verdict(true, false, false, false, None), Ban("قفل ربات"));
        assert_eq!(post_verdict(true, false, false, true, None), Keep, "an admin bot stays");
        assert_eq!(post_verdict(true, true, false, true, None), Ban("قفل ربات"));
        assert_eq!(post_verdict(true, true, true, false, link), Keep, "the allowlist wins");

        assert_eq!(post_verdict(false, false, false, false, None), Keep, "lock off, clean post");
        assert_eq!(post_verdict(false, false, false, false, link), Ban("link"), "not only deleted");
        assert_eq!(post_verdict(false, false, false, true, link), Keep, "lock off, admin bot");
        assert_eq!(
            post_verdict(false, true, false, true, link),
            Delete("link"),
            "«اعمال روی ادمین ها هم» takes the post, never the seat, while the lock is off"
        );
        assert_eq!(post_verdict(false, true, false, false, link), Ban("link"));
        assert_eq!(post_verdict(false, true, true, false, link), Keep);
    }

    #[tokio::test]
    #[ignore = "needs DATABASE_URL: a Ctx needs a settings mirror"]
    async fn a_bare_user_ref_takes_the_sessions_hash() {
        use grammers_client::session::types::{PeerAuth, PeerInfo};
        let (ctx, _pool) = super::super::live_ctx().await;
        const KNOWN: i64 = 5_201;
        ctx.bot_session
            .cache_peer(&PeerInfo::User {
                id: KNOWN,
                auth: Some(PeerAuth::from_hash(77)),
                bot: Some(true),
                is_self: None,
            })
            .await
            .unwrap();
        let bare = |id| PeerId::user(id).unwrap().to_ambient_ref();
        assert_eq!(ctx.with_session_auth(bare(KNOWN)).await.auth.hash(), 77);
        assert_eq!(ctx.with_session_auth(bare(5_202)).await, bare(5_202), "unknown stays bare");
        let given = PeerRef {
            id: PeerId::user(KNOWN).unwrap(),
            auth: PeerAuth::from_hash(5),
        };
        assert_eq!(ctx.with_session_auth(given).await, given, "a caller's own hash wins");
    }

    #[tokio::test]
    #[ignore = "needs DATABASE_URL: a Ctx needs a settings mirror"]
    async fn an_admins_bot_is_remembered_until_it_leaves() {
        use grammers_client::session::types::PeerInfo;
        use tl::enums::ChannelParticipant as P;

        const CHANNEL: i64 = 777_000_991;
        const CHAT: i64 = -1_000_000_000_000 - CHANNEL;
        const OWNER: i64 = 5_100;
        const BOT: i64 = 5_101;
        const SELF_ADDED: i64 = 5_102;
        const HUMAN: i64 = 5_103;
        let (ctx, _pool) = super::super::live_ctx().await;
        let ctx = std::sync::Arc::new(ctx);
        for (id, bot) in [(BOT, true), (SELF_ADDED, true), (HUMAN, false)] {
            ctx.bot_session
                .cache_peer(&PeerInfo::User {
                    id,
                    auth: None,
                    bot: Some(bot),
                    is_self: None,
                })
                .await
                .unwrap();
        }
        ctx.settings
            .try_set_value(CHAT, super::super::config::OWNER, &OWNER.to_string())
            .await
            .unwrap();
        let member = |user| {
            Some(P::Participant(tl::types::ChannelParticipant {
                user_id: user,
                date: 0,
                subscription_until_date: None,
                rank: None,
            }))
        };
        let admin = |user| {
            Some(P::Admin(tl::types::ChannelParticipantAdmin {
                can_edit: true,
                is_self: false,
                user_id: user,
                inviter_id: None,
                promoted_by: OWNER,
                date: 0,
                admin_rights: tl::types::ChatAdminRights {
                    change_info: false,
                    post_messages: false,
                    edit_messages: false,
                    delete_messages: true,
                    ban_users: false,
                    invite_users: false,
                    pin_messages: false,
                    add_admins: false,
                    anonymous: false,
                    manage_call: false,
                    other: false,
                    manage_topics: false,
                    post_stories: false,
                    edit_stories: false,
                    delete_stories: false,
                    manage_direct_messages: false,
                    manage_ranks: false,
                    manage_linked_peers: false,
                }
                .into(),
                rank: None,
            }))
        };
        let update = |actor, user, prev, new| tl::types::UpdateChannelParticipant {
            via_chatlist: false,
            channel_id: CHANNEL,
            date: 0,
            actor_id: actor,
            user_id: user,
            prev_participant: prev,
            new_participant: new,
            invite: None,
            qts: 0,
        };
        let left = |user| {
            Some(P::Left(tl::types::ChannelParticipantLeft {
                peer: tl::types::PeerUser { user_id: user }.into(),
            }))
        };

        for locked in [false, true] {
            ctx.settings.try_set(CHAT, LOCK, locked).await.unwrap();
            on_participant_update(&ctx, &update(OWNER, BOT, None, member(BOT))).await;
            assert!(is_admitted(&ctx, CHAT, BOT), "lock {locked}: the owner's bot");
            if !locked {
                on_participant_update(&ctx, &update(SELF_ADDED, SELF_ADDED, None, member(SELF_ADDED)))
                    .await;
                assert!(!is_admitted(&ctx, CHAT, SELF_ADDED), "added by nobody");
            }
            on_participant_update(&ctx, &update(OWNER, HUMAN, None, member(HUMAN))).await;
            assert!(!is_admitted(&ctx, CHAT, HUMAN), "lock {locked}: a human");
            on_participant_update(&ctx, &update(OWNER, BOT, admin(BOT), member(BOT))).await;
            assert!(is_admitted(&ctx, CHAT, BOT), "lock {locked}: demoted, still admitted");
            if !locked {
                on_participant_update(
                    &ctx,
                    &update(OWNER, SELF_ADDED, admin(SELF_ADDED), member(SELF_ADDED)),
                )
                .await;
                assert!(!is_admitted(&ctx, CHAT, SELF_ADDED), "a demoter is not an adder");
            }
            on_participant_update(&ctx, &update(BOT, BOT, member(BOT), left(BOT))).await;
            assert!(!is_admitted(&ctx, CHAT, BOT), "lock {locked}: gone, forgotten");
        }

        for key in [LOCK.to_string(), super::super::config::OWNER.to_string()] {
            ctx.settings.try_set(CHAT, &key, false).await.unwrap();
        }
    }
}
