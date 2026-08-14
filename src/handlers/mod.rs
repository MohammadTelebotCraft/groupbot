pub mod answers;
pub mod autoconfig;
pub mod betrayal;
pub mod bots;
pub mod callbacks;
pub mod captcha;
pub mod cleaner;
pub mod emoji_image;
pub mod config;
pub mod extras;
pub mod filters;
pub mod flood;
pub mod join;
pub mod lists;
pub mod log;
pub mod locks;
pub mod notice;
pub mod packs;
pub mod panel;
pub mod ping;
pub mod promote;
pub mod purge;
pub mod report;
pub mod rights;
pub mod restrict;
pub mod stats;
pub mod strict;
pub mod style;
pub mod vip;
pub mod warns;
pub mod welcome;
pub mod toggles;
pub mod tune;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use grammers_client::Client;
use grammers_client::message::Message;
use grammers_client::peer::Peer;
use grammers_client::update::Update;
use grammers_client::session::types::{PeerId, PeerKind, PeerRef};

use crate::state::Settings;

pub struct Ctx {
    pub client: Client,
    pub settings: Arc<Settings>,

    admin_cache: RwLock<HashMap<i64, (Instant, HashSet<i64>)>>,

    removals: RwLock<HashMap<(i64, i64), Vec<Instant>>>,

    messages: RwLock<HashMap<(i64, i64), Vec<Instant>>>,

    notices: RwLock<HashMap<(u8, i64, i64), Instant>>,

    deleted: RwLock<HashMap<u64, (Instant, String)>>,
    next_deleted_key: AtomicU64,

    chat_refs: RwLock<HashMap<i64, PeerRef>>,

    pub started: Instant,

    pending_admins: RwLock<HashMap<u64, promote::Pending>>,

    captchas: RwLock<HashMap<(i64, i64), captcha::Pending>>,

    counts: std::sync::Mutex<HashMap<(i64, i64), (u64, String)>>,

    tallies: std::sync::Mutex<HashMap<(i64, &'static str), u64>>,

    admin_fetches: tokio::sync::Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,

    user: RwLock<Option<Client>>,

    cleaner_id: AtomicI64,

    me_id: AtomicI64,

    members: RwLock<HashMap<(i64, i64), Instant>>,

    pending_logs: std::sync::Mutex<HashMap<i64, Vec<String>>>,

    joined: RwLock<HashMap<(i64, i32), Vec<Joined>>>,

    user_chats: RwLock<HashMap<i64, PeerRef>>,

    pending_password: RwLock<Option<(Instant, Option<String>)>>,

    pending_numbers: RwLock<HashMap<(i64, i64), PendingNumber>>,

    join_refs: RwLock<HashMap<String, PeerRef>>,
}

pub const NOTICE_EVERY: Duration = Duration::from_secs(120);

const DELETED_TTL: Duration = Duration::from_secs(3600);
const DELETED_MAX: usize = 5_000;
const NOTICES_MAX: usize = 10_000;

type PendingNumber = (Instant, &'static str);

const MEMBER_TRUST: Duration = Duration::from_secs(60);

const PENDING_PASSWORD_TTL: Duration = Duration::from_secs(300);

const PENDING_NUMBER_TTL: Duration = Duration::from_secs(120);

const REMOVALS_MAX: usize = 10_000;

const ADMIN_CACHE_TTL: Duration = Duration::from_secs(1800);

const ADMIN_CACHE_MAX: usize = 10_000;

impl Ctx {
    pub fn new(client: Client, settings: Arc<Settings>) -> Self {
        Self {
            client,
            settings,
            admin_cache: RwLock::new(HashMap::new()),
            removals: RwLock::new(HashMap::new()),
            messages: RwLock::new(HashMap::new()),
            notices: RwLock::new(HashMap::new()),
            deleted: RwLock::new(HashMap::new()),
            next_deleted_key: AtomicU64::new(1),
            chat_refs: RwLock::new(HashMap::new()),
            started: Instant::now(),
            pending_admins: RwLock::new(HashMap::new()),
            captchas: RwLock::new(HashMap::new()),
            counts: std::sync::Mutex::new(HashMap::new()),
            tallies: std::sync::Mutex::new(HashMap::new()),
            admin_fetches: tokio::sync::Mutex::new(HashMap::new()),
            user: RwLock::new(None),
            cleaner_id: AtomicI64::new(0),
            me_id: AtomicI64::new(0),
            joined: RwLock::new(HashMap::new()),
            members: RwLock::new(HashMap::new()),
            pending_logs: std::sync::Mutex::new(HashMap::new()),
            user_chats: RwLock::new(HashMap::new()),
            pending_password: RwLock::new(None),
            pending_numbers: RwLock::new(HashMap::new()),
            join_refs: RwLock::new(HashMap::new()),
        }
    }

    pub fn user_client(&self) -> Option<Client> {
        self.user.read().unwrap().clone()
    }

    pub fn set_user_client(&self, client: Client) {
        *self.user.write().unwrap() = Some(client);
    }

    pub fn set_me_id(&self, user: i64) {
        self.me_id.store(user, Ordering::Relaxed);
    }

    pub fn channel_member(&self, chat: i64, user: i64) -> bool {
        self.members
            .read()
            .unwrap()
            .get(&(chat, user))
            .is_some_and(|seen| seen.elapsed() < MEMBER_TRUST)
    }

    pub fn remember_member(&self, chat: i64, user: i64) {
        const MAX: usize = 100_000;
        let mut members = self.members.write().unwrap();
        if members.len() >= MAX {
            members.retain(|_, seen| seen.elapsed() < MEMBER_TRUST);
        }
        members.insert((chat, user), Instant::now());
    }

    pub fn forget_member(&self, chat: i64, user: i64) {
        self.members.write().unwrap().remove(&(chat, user));
    }

    pub fn queue_log(&self, chat: i64, entry: String) {
        const MAX_PER_CHAT: usize = 200;
        let mut queued = self.pending_logs.lock().unwrap();
        let entries = queued.entry(chat).or_default();
        if entries.len() >= MAX_PER_CHAT {
            entries.remove(0);
        }
        entries.push(entry);
    }

    pub fn take_logs(&self) -> Vec<(i64, Vec<String>)> {
        std::mem::take(&mut *self.pending_logs.lock().unwrap())
            .into_iter()
            .collect()
    }

    fn joined_cached(&self, key: (i64, i32)) -> Option<Vec<Joined>> {
        self.joined.read().unwrap().get(&key).cloned()
    }

    fn remember_joined(&self, key: (i64, i32), joined: Vec<Joined>) {
        const MAX: usize = 1_000;
        let mut cache = self.joined.write().unwrap();

        if cache.len() >= MAX {
            cache.clear();
        }
        cache.insert(key, joined);
    }

    pub fn me_id(&self) -> i64 {
        self.me_id.load(Ordering::Relaxed)
    }

    pub fn set_cleaner_id(&self, user: i64) {
        self.cleaner_id.store(user, Ordering::Relaxed);
    }

    pub fn is_cleaner(&self, user: i64) -> bool {
        user != 0 && self.cleaner_id.load(Ordering::Relaxed) == user
    }

    pub fn user_chat(&self, chat: i64) -> Option<PeerRef> {
        self.user_chats.read().unwrap().get(&chat).copied()
    }

    pub fn forget_user_chats(&self) {
        self.user_chats.write().unwrap().clear();
    }

    pub fn set_user_chats(&self, chats: Vec<(i64, PeerRef)>) {
        let mut known = self.user_chats.write().unwrap();
        known.clear();
        known.extend(chats);
    }

    pub fn cleaner_id(&self) -> Option<i64> {
        match self.cleaner_id.load(Ordering::Relaxed) {
            0 => None,
            id => Some(id),
        }
    }

    pub fn expect_password(&self) {
        *self.pending_password.write().unwrap() = Some((Instant::now(), None));
    }

    pub fn give_password(&self, password: String) -> bool {
        let mut pending = self.pending_password.write().unwrap();
        match pending.as_mut() {
            Some((armed, slot)) if armed.elapsed() < PENDING_PASSWORD_TTL && slot.is_none() => {
                *slot = Some(password);
                true
            }
            _ => false,
        }
    }

    pub async fn await_password(&self, timeout: Duration) -> Option<String> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if let Some((_, Some(password))) = self.pending_password.write().unwrap().take() {
                return Some(password);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        self.pending_password.write().unwrap().take();
        None
    }

    pub fn expect_number(&self, chat: i64, user: i64, setting: &'static str) {
        let mut pending = self.pending_numbers.write().unwrap();
        pending.retain(|_, (armed, _)| armed.elapsed() < PENDING_NUMBER_TTL);
        pending.insert((chat, user), (Instant::now(), setting));
    }

    pub fn take_expected_number(&self, chat: i64, user: i64) -> Option<&'static str> {
        let mut pending = self.pending_numbers.write().unwrap();
        let (armed, setting) = pending.remove(&(chat, user))?;
        (armed.elapsed() < PENDING_NUMBER_TTL).then_some(setting)
    }

    pub fn join_ref(&self, name: &str) -> Option<PeerRef> {
        self.join_refs.read().unwrap().get(name).copied()
    }

    pub fn remember_join_ref(&self, name: &str, peer: PeerRef) {
        let mut refs = self.join_refs.write().unwrap();
        if refs.len() < 10_000 {
            refs.insert(name.to_owned(), peer);
        }
    }

    async fn admin_fetch_lock(&self, chat: i64) -> Arc<tokio::sync::Mutex<()>> {
        let mut fetches = self.admin_fetches.lock().await;

        fetches.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(fetches.entry(chat).or_default())
    }

    pub fn pending_admin_new(&self, pending: promote::Pending) -> u64 {
        let key = self.next_deleted_key.fetch_add(1, Ordering::Relaxed);
        let mut pendings = self.pending_admins.write().unwrap();
        pendings.retain(|_, p| p.started.elapsed() < promote::PENDING_TTL);
        pendings.insert(key, pending);
        key
    }

    pub fn pending_admin(&self, key: u64) -> Option<promote::Pending> {
        self.pending_admins
            .read()
            .unwrap()
            .get(&key)
            .filter(|p| p.started.elapsed() < promote::PENDING_TTL)
            .cloned()
    }

    pub fn pending_admin_set_rights(&self, key: u64, rights: u32) {
        if let Some(pending) = self.pending_admins.write().unwrap().get_mut(&key) {
            pending.rights = rights;
        }
    }

    pub fn pending_admin_done(&self, key: u64) {
        self.pending_admins.write().unwrap().remove(&key);
    }

    pub fn may_notify(&self, chat: i64, user: i64) -> bool {
        self.throttle(0, chat, user, NOTICE_EVERY)
    }

    pub fn may_notify_every(&self, chat: i64, user: i64, every: Duration) -> bool {
        every.is_zero() || self.throttle(3, chat, user, every)
    }

    pub fn may_notify_lock(&self, chat: i64, user: i64) -> bool {
        self.throttle(2, chat, user, Duration::from_secs(20))
    }

    pub fn may_report(&self, chat: i64, user: i64) -> bool {
        self.throttle(1, chat, user, report::EVERY)
    }

    fn throttle(&self, kind: u8, chat: i64, user: i64, every: Duration) -> bool {
        let mut notices = self.notices.write().unwrap();
        if let Some(last) = notices.get(&(kind, chat, user))
            && last.elapsed() < every
        {
            return false;
        }
        if notices.len() >= NOTICES_MAX {
            notices.retain(|_, last| last.elapsed() < NOTICE_EVERY);
        }
        notices.insert((kind, chat, user), Instant::now());
        true
    }

    pub fn keep_deleted(&self, text: String) -> u64 {
        let key = self.next_deleted_key.fetch_add(1, Ordering::Relaxed);
        let mut deleted = self.deleted.write().unwrap();
        if deleted.len() >= DELETED_MAX {
            deleted.retain(|_, (kept, _)| kept.elapsed() < DELETED_TTL);
        }
        deleted.insert(key, (Instant::now(), text));
        key
    }

    pub fn deleted_text(&self, key: u64) -> Option<String> {
        let deleted = self.deleted.read().unwrap();
        deleted
            .get(&key)
            .filter(|(kept, _)| kept.elapsed() < DELETED_TTL)
            .map(|(_, text)| text.clone())
    }

    pub fn record_message(&self, chat: i64, user: i64, window: Duration) -> usize {
        let mut messages = self.messages.write().unwrap();
        if messages.len() >= REMOVALS_MAX {
            messages.retain(|_, times| times.iter().any(|t| t.elapsed() < window));
        }
        let times = messages.entry((chat, user)).or_default();
        times.retain(|t| t.elapsed() < window);
        times.push(Instant::now());
        times.len()
    }

    pub fn record_removal(&self, chat: i64, actor: i64, window: Duration) -> usize {
        let mut removals = self.removals.write().unwrap();
        if removals.len() >= REMOVALS_MAX {
            removals.retain(|_, times| times.iter().any(|t| t.elapsed() < window));
        }
        let times = removals.entry((chat, actor)).or_default();
        times.retain(|t| t.elapsed() < window);
        times.push(Instant::now());
        times.len()
    }

    pub fn remember_chat(&self, chat: i64, peer: PeerRef) {
        const MAX: usize = 50_000;
        let mut refs = self.chat_refs.write().unwrap();
        if refs.len() < MAX {
            refs.insert(chat, peer);
        }
    }

    pub fn count_message(&self, chat: i64, user: i64, name: impl FnOnce() -> String) {
        let mut counts = self.counts.lock().unwrap();
        let entry = counts
            .entry((chat, user))
            .or_insert_with(|| (0, name()));
        entry.0 += 1;
    }

    pub fn bump(&self, chat: i64, counter: &'static str) {
        *self.tallies.lock().unwrap().entry((chat, counter)).or_insert(0) += 1;
    }

    fn cached_admin(&self, chat: i64, user: i64) -> Option<bool> {
        let cache = self.admin_cache.read().unwrap();
        let (fetched, admins) = cache.get(&chat)?;
        (fetched.elapsed() < ADMIN_CACHE_TTL).then(|| admins.contains(&user))
    }

    pub fn take_tallies(&self) -> HashMap<(i64, &'static str), u64> {
        std::mem::take(&mut *self.tallies.lock().unwrap())
    }

    pub fn take_counts(&self) -> HashMap<(i64, i64), (u64, String)> {
        std::mem::take(&mut self.counts.lock().unwrap())
    }

    pub fn captcha_start(&self, chat: i64, user: i64, pending: captcha::Pending) {
        let mut captchas = self.captchas.write().unwrap();

        captchas.retain(|_, p| p.started.elapsed() < Duration::from_secs(900));
        captchas.insert((chat, user), pending);
    }

    pub fn captcha_pending(&self, chat: i64, user: i64) -> Option<captcha::Pending> {
        self.captchas.read().unwrap().get(&(chat, user)).cloned()
    }

    pub fn captcha_done(&self, chat: i64, user: i64) {
        self.captchas.write().unwrap().remove(&(chat, user));
    }

    pub fn chat_ref(&self, chat: i64) -> Option<PeerRef> {
        self.chat_refs.read().unwrap().get(&chat).copied()
    }

    pub fn forget_admins(&self, chat: i64) {
        self.admin_cache.write().unwrap().remove(&chat);
    }

    fn cache_admins(&self, chat: i64, admins: HashSet<i64>) {
        let mut cache = self.admin_cache.write().unwrap();
        if cache.len() >= ADMIN_CACHE_MAX {
            cache.retain(|_, (fetched, _)| fetched.elapsed() < ADMIN_CACHE_TTL);
        }
        cache.insert(chat, (Instant::now(), admins));
    }

    fn cached_admins(&self, chat: i64) -> Option<HashSet<i64>> {
        let cache = self.admin_cache.read().unwrap();
        let (fetched, admins) = cache.get(&chat)?;
        (fetched.elapsed() < ADMIN_CACHE_TTL).then(|| admins.clone())
    }
}

pub async fn chat_admins(ctx: &Ctx, chat_ref: PeerRef, chat: i64) -> Option<HashSet<i64>> {
    admins_of(ctx, chat_ref, chat, true).await
}

async fn admins_of(ctx: &Ctx, chat_ref: PeerRef, chat: i64, loud: bool) -> Option<HashSet<i64>> {
    if let Some(admins) = ctx.cached_admins(chat) {
        return Some(admins);
    }

    let guard = ctx.admin_fetch_lock(chat).await;
    let _fetching = guard.lock().await;
    if let Some(admins) = ctx.cached_admins(chat) {
        return Some(admins);
    }

    let mut participants = ctx.client.iter_participants(chat_ref).filter(
        grammers_client::tl::enums::ChannelParticipantsFilter::ChannelParticipantsAdmins,
    );
    let mut admins = HashSet::new();
    let mut failed = false;
    loop {
        match participants.next().await {
            Ok(Some(participant)) => {
                admins.insert(participant.user.id().bare_id_unchecked());
            }
            Ok(None) => break,
            Err(e) => {
                if loud {
                    eprintln!("could not list admins of {chat}: {e}");
                }
                failed = true;
                break;
            }
        }
    }

    if failed || admins.is_empty() {
        return None;
    }
    ctx.cache_admins(chat, admins.clone());
    Some(admins)
}

pub async fn dispatch(ctx: &Arc<Ctx>, update: Update) {
    let message = match update {
        Update::NewMessage(message) if !message.outgoing() => message,

        Update::MessageEdited(message) if !message.outgoing() => {
            if message.peer_id().kind() != PeerKind::User {
                locks::on_edit(ctx, &message).await;
            }
            return;
        }
        Update::CallbackQuery(query) => {
            callbacks::handle(ctx, &query).await;
            return;
        }

        Update::Raw(raw) => {
            invalidate_admins(ctx, &raw);
            if let grammers_client::tl::enums::Update::ChannelParticipant(update) = &raw.raw {
                betrayal::on_participant_update(ctx, update).await;
                log::on_participant(ctx, update).await;
            }
            autoconfig::on_raw(ctx, &raw).await;
            return;
        }
        _ => return,
    };
    let message = &message;

    if age_seconds(message) > STALE_AFTER {
        return;
    }

    if message.peer_id().kind() == PeerKind::User {
        let _ = cleaner::take_password(ctx, message).await
            || config::start(ctx, message).await
            || config::help(message).await
            || cleaner::handle(ctx, message).await
            || panel::handle_private(ctx, message).await;
        return;
    }

    if let Some(chat) = chat_id(message)
        && ctx.chat_ref(chat).is_none()
        && let Ok(Some(peer)) = message.peer_ref().await
    {
        ctx.remember_chat(chat, peer);

        if let Some(title) = message.peer().and_then(|peer| peer.name())
            && ctx.settings.value(chat, TITLE).as_deref() != Some(title)
        {
            ctx.settings.set_value(chat, TITLE, title).await;
        }
    }

    stats::count(ctx, message);
    if matches!(
        message.action(),
        Some(grammers_client::tl::enums::MessageAction::ChatDeleteUser(_))
    ) && let Some(chat) = chat_id(message)
    {
        ctx.bump(chat, stats::LEFT);
    }

    if let Some(chat) = chat_id(message)
        && matches!(
            message.action(),
            Some(
                grammers_client::tl::enums::MessageAction::ChatAddUser(_)
                    | grammers_client::tl::enums::MessageAction::ChatJoinedByLink(_)
            )
        )
    {
        ctx.bump(chat, stats::JOINED);
        if let Some(grammers_client::tl::enums::MessageAction::ChatAddUser(action)) =
            message.action()
        {
            stats::count_add(ctx, message, action.users.len()).await;
        }
    }
    flood::check(ctx, message).await;

    if panel::typed_number(ctx, message).await {
        return;
    }

    if join::enforce(ctx, message).await {
        return;
    }

    let bot_authored = bot_authored(message);

    let _ = cleaner::on_join(ctx, message).await
        || autoconfig::on_message(ctx, message).await
        || bots::handle(ctx, message).await
        || captcha::on_join(ctx, message).await
        || welcome::on_join(ctx, message).await
        || (!bot_authored
            && (welcome::handle(ctx, message).await
                || config::handle(ctx, message).await
                || panel::handle(ctx, message).await
                || restrict::handle(ctx, message).await
                || lists::command(ctx, message).await
                || ping::handle(ctx, message).await
                || promote::handle(ctx, message).await
                || stats::handle(ctx, message).await
                || report::handle(ctx, message).await
                || packs::handle(ctx, message).await
                || extras::handle(ctx, message).await
                || tune::handle(ctx, message).await
                || warns::handle(ctx, message).await
                || filters::handle(ctx, message).await
                || purge::handle(ctx, message).await
                || purge::handle_all(ctx, message).await
                || flood::handle(ctx, message).await
                || join::handle(ctx, message).await
                || cleaner::add(ctx, message).await
                || cleaner::wipe(ctx, message).await
                || cleaner::sweep(ctx, message).await
                || log::handle(ctx, message).await
                || rights::handle(ctx, message).await
                || vip::handle(ctx, message).await))
        || locks::handle(ctx, message).await
        || (!bot_authored && answers::handle(ctx, message).await);

    locks::service(ctx, message).await;
}

fn bot_authored(message: &Message) -> bool {
    message.via_bot_id().is_some()
        || matches!(message.sender(), Some(Peer::User(user)) if user.is_bot())
}

fn invalidate_admins(ctx: &Ctx, raw: &grammers_client::update::Raw) {
    use grammers_client::tl;
    use tl::enums::ChannelParticipant as P;

    let admin_change = |participant: &Option<P>| {
        matches!(participant, Some(P::Admin(_) | P::Creator(_)))
    };
    let chat = match &raw.raw {
        tl::enums::Update::ChannelParticipant(u) => {
            if !admin_change(&u.prev_participant) && !admin_change(&u.new_participant) {
                return;
            }
            PeerId::channel(u.channel_id)
        }
        tl::enums::Update::ChatParticipantAdmin(u) => PeerId::chat(u.chat_id),
        _ => return,
    };
    if let Some(chat) = chat.and_then(|id| id.bot_api_dialog_id()) {
        ctx.forget_admins(chat);
    }
}

const STALE_AFTER: i64 = 120;

fn age_seconds(message: &Message) -> i64 {
    let sent = message.date().as_second();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(sent);
    now - sent
}

pub const TITLE: &str = "title";

pub fn owner(ctx: &Ctx, chat: i64) -> Option<i64> {
    ctx.settings.value_parsed(chat, config::OWNER)
}

fn chat_id(message: &Message) -> Option<i64> {
    message.peer_id().bot_api_dialog_id()
}

pub fn is_bot_admin(ctx: &Ctx, chat: i64, user: i64) -> bool {
    ctx.settings.is_locked(chat, &bot_admin_key(user))
}

pub fn bot_admin_key(user: i64) -> String {
    format!("admin:{user}")
}

pub async fn is_exempt(ctx: &Ctx, message: &Message) -> bool {
    if let (Some(chat), Some(sender)) = (chat_id(message), message.sender_id().and_then(PeerId::bare_id))
        && vip::is_vip(ctx, chat, sender)
    {
        return true;
    }
    can_manage(ctx, message).await
}

pub async fn can_manage(ctx: &Ctx, message: &Message) -> bool {
    if is_owner(ctx, message) {
        return true;
    }
    let Some(chat) = chat_id(message) else {
        return false;
    };
    let Some(sender) = message.sender_id().and_then(PeerId::bare_id) else {
        return true;
    };
    if is_bot_admin(ctx, chat, sender) || ctx.is_cleaner(sender) {
        return true;
    }

    if let Some(is_admin) = ctx.cached_admin(chat, sender) {
        return is_admin;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };
    match chat_admins(ctx, chat_ref, chat).await {
        Some(admins) => admins.contains(&sender),

        None => permissions(ctx, message).await.is_some_and(|p| p.is_admin()),
    }
}

pub fn is_owner(ctx: &Ctx, message: &Message) -> bool {
    match (chat_id(message), message.sender_id().and_then(PeerId::bare_id)) {
        (Some(chat), Some(sender)) => owner(ctx, chat) == Some(sender),
        _ => false,
    }
}

pub async fn target(ctx: &Ctx, message: &Message, arg: Option<&str>) -> Option<(PeerRef, String)> {
    if let Some(arg) = arg {
        if let Some(username) = arg.strip_prefix('@').filter(|u| !u.is_empty()) {
            let peer = ctx.client.resolve_username(username).await.ok()??;
            let name = peer.name().unwrap_or(username).to_owned();
            return Some((peer.to_ref().await.ok()??, name));
        }
        if let Ok(id) = digits(arg).parse::<i64>() {
            return Some((PeerId::user(id)?.to_ambient_ref(), id.to_string()));
        }
        return None;
    }
    let replied = message.get_reply().await.ok()??;
    let name = name_of(&replied);
    Some((replied.sender_ref().await.ok()??, name))
}

pub fn digits(text: &str) -> std::borrow::Cow<'_, str> {
    if !text
        .chars()
        .any(|c| matches!(c, '۰'..='۹' | '٠'..='٩'))
    {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(
        text.chars()
            .map(|c| match c {
                '۰'..='۹' => char::from(b'0' + (c as u32 - '۰' as u32) as u8),
                '٠'..='٩' => char::from(b'0' + (c as u32 - '٠' as u32) as u8),
                other => other,
            })
            .collect(),
    )
}

pub fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn sender_of(message: &Message) -> Option<(i64, String)> {
    let id = message.sender_id().and_then(PeerId::bare_id)?;
    Some((id, name_of(message)))
}

pub fn name_of(message: &Message) -> String {
    match message.sender().and_then(|peer| peer.name()) {
        Some(name) => name.to_owned(),
        None => match message.sender_id().and_then(PeerId::bare_id) {
            Some(id) => id.to_string(),
            None => "کاربر".to_owned(),
        },
    }
}

pub async fn warm_admin_cache(ctx: Arc<Ctx>) {
    const AT_ONCE: usize = 4;

    let chats = ctx.settings.chats();
    let mut warmed = 0;
    for batch in chats.chunks(AT_ONCE) {
        let mut tasks = tokio::task::JoinSet::new();
        for &chat in batch {
            let ctx = Arc::clone(&ctx);
            tasks.spawn(async move {
                let Some(chat_ref) = ctx.chat_ref(chat) else {
                    return false;
                };

                admins_of(&ctx, chat_ref, chat, false).await.is_some()
            });
        }
        while let Some(result) = tasks.join_next().await {
            if matches!(result, Ok(true)) {
                warmed += 1;
            }
        }
    }
    if warmed > 0 {
        println!("warmed admin cache for {warmed} chats");
    }
}

#[derive(Clone)]
pub struct Joined {
    pub peer: PeerRef,
    pub id: i64,
    pub name: String,
    pub is_bot: bool,
}

pub async fn joined_users(ctx: &Ctx, message: &Message) -> Vec<Joined> {
    use grammers_client::tl::enums::MessageAction;

    let memo = chat_id(message).map(|chat| (chat, message.id()));
    if let Some(cached) = memo.and_then(|key| ctx.joined_cached(key)) {
        return cached;
    }

    let ids: Vec<i64> = match message.action() {
        Some(MessageAction::ChatAddUser(action)) => action.users.clone(),
        Some(MessageAction::ChatJoinedByLink(_)) => {
            let (Ok(Some(peer)), Some(id)) = (
                message.sender_ref().await,
                message.sender_id().and_then(PeerId::bare_id),
            ) else {
                return Vec::new();
            };
            let joined = vec![Joined {
                peer,
                id,
                name: name_of(message),
                is_bot: message.sender().is_some_and(|peer| match peer {
                    grammers_client::peer::Peer::User(user) => user.is_bot(),
                    _ => false,
                }),
            }];
            if let Some(key) = memo {
                ctx.remember_joined(key, joined.clone());
            }
            return joined;
        }
        _ => return Vec::new(),
    };

    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return Vec::new();
    };

    let mut participants = ctx.client.iter_participants(chat_ref).filter(
        grammers_client::tl::enums::ChannelParticipantsFilter::ChannelParticipantsRecent,
    );
    let mut found = Vec::new();
    while let Ok(Some(participant)) = participants.next().await {
        let user = participant.user;
        let id = user.id().bare_id_unchecked();
        if !ids.contains(&id) {
            continue;
        }
        if let Ok(Some(peer)) = user.to_ref().await {
            found.push(Joined {
                peer,
                id,
                name: user.full_name(),
                is_bot: user.is_bot(),
            });
        }
        if found.len() == ids.len() {
            break;
        }
    }
    if let Some(key) = memo {
        ctx.remember_joined(key, found.clone());
    }
    found
}

pub async fn admin_ref(ctx: &Ctx, chat: PeerRef, user_id: i64) -> Option<(PeerRef, String)> {
    let mut participants = ctx.client.iter_participants(chat).filter(
        grammers_client::tl::enums::ChannelParticipantsFilter::ChannelParticipantsAdmins,
    );
    while let Ok(Some(participant)) = participants.next().await {
        if participant.user.id().bare_id_unchecked() != user_id {
            continue;
        }
        let name = participant.user.full_name();
        return participant
            .user
            .to_ref()
            .await
            .ok()
            .flatten()
            .map(|peer| (peer, name));
    }
    None
}

pub async fn admins(ctx: &Ctx, chat: PeerRef) -> (Option<(i64, String)>, Vec<String>) {
    let mut participants = ctx.client.iter_participants(chat).filter(
        grammers_client::tl::enums::ChannelParticipantsFilter::ChannelParticipantsAdmins,
    );
    let (mut creator, mut names) = (None, Vec::new());
    while let Ok(Some(participant)) = participants.next().await {
        if participant.user.is_bot() {
            continue;
        }
        let name = esc(&participant.user.full_name());
        if matches!(participant.role, grammers_client::peer::Role::Creator(_)) {
            creator = Some((participant.user.id().bare_id_unchecked(), name.clone()));
            names.push(format!("★ {name}"));
        } else {
            names.push(format!("‹ {name}"));
        }
    }
    (creator, names)
}

pub async fn sender_is_creator(ctx: &Ctx, message: &Message) -> bool {
    permissions(ctx, message)
        .await
        .is_some_and(|p| p.is_creator())
}

async fn permissions(
    ctx: &Ctx,
    message: &Message,
) -> Option<grammers_client::client::ParticipantPermissions> {
    let (Ok(Some(chat)), Ok(Some(sender))) = (message.peer_ref().await, message.sender_ref().await)
    else {
        return None;
    };
    ctx.client.get_permissions(chat, sender).await.ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn sources_have_no_zero_width_non_joiner() {
        for file in [
            include_str!("mod.rs"),
            include_str!("betrayal.rs"),
            include_str!("bots.rs"),
            include_str!("callbacks.rs"),
            include_str!("captcha.rs"),
            include_str!("config.rs"),
            include_str!("extras.rs"),
            include_str!("filters.rs"),
            include_str!("flood.rs"),
            include_str!("lists.rs"),
            include_str!("locks.rs"),
            include_str!("notice.rs"),
            include_str!("packs.rs"),
            include_str!("panel.rs"),
            include_str!("ping.rs"),
            include_str!("promote.rs"),
            include_str!("purge.rs"),
            include_str!("report.rs"),
            include_str!("restrict.rs"),
            include_str!("stats.rs"),
            include_str!("strict.rs"),
            include_str!("style.rs"),
            include_str!("vip.rs"),
            include_str!("warns.rs"),
            include_str!("welcome.rs"),
            include_str!("toggles.rs"),
            include_str!("tune.rs"),
            include_str!("answers.rs"),
            include_str!("autoconfig.rs"),
        ] {
            assert!(!file.contains('\u{200c}'), "found U+200C in a handler");
        }
    }
}
