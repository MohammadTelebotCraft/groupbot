pub mod answers;
pub mod autoconfig;
pub mod betrayal;
pub mod biolink;
pub mod bots;
pub mod callbacks;
pub mod captcha;
pub mod cleaner;
pub mod concept_vectors;
pub mod concepts;
pub mod emoji_image;
pub mod config;
pub mod extras;
pub mod filters;
pub mod flood;
pub mod help;
pub mod join;
pub mod limits;
pub mod lists;
pub mod log;
pub mod locks;
pub mod notice;
pub mod ocr;
pub mod nsfw;
pub mod packs;
pub mod panel;
pub mod ping;
pub mod promote;
pub mod purge;
pub mod report;
pub mod rights;
pub mod pinlock;
pub mod raid;
pub mod restrict;
pub mod setting;
pub mod stats;
pub mod strict;
pub mod style;
pub mod tempmedia;
pub mod vip;
pub mod warns;
pub mod welcome;
pub mod toggles;
pub mod tune;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use grammers_client::Client;
use grammers_client::message::Message;
use grammers_client::peer::Peer;
use grammers_client::update::Update;
use grammers_client::session::types::{PeerAuth, PeerId, PeerKind, PeerRef};

use crate::state::Settings;

#[derive(Default)]
pub struct ChatState {
    chat: i64,
    dirty: Arc<Dirty>,

    last_seen: AtomicU64,

    peer: RwLock<Option<PeerRef>>,

    admins: RwLock<Option<(Instant, HashSet<i64>)>>,

    admin_fetch: tokio::sync::Mutex<()>,

    messages: std::sync::Mutex<HashMap<i64, Vec<Instant>>>,
    removals: std::sync::Mutex<HashMap<i64, Vec<Instant>>>,
    notices: std::sync::Mutex<HashMap<(u8, i64), Instant>>,
    members: std::sync::Mutex<HashMap<i64, Instant>>,
    adds: std::sync::Mutex<HashMap<i64, (Instant, u64)>>,
    captchas: std::sync::Mutex<HashMap<i64, captcha::Pending>>,
    counts: std::sync::Mutex<HashMap<i64, (u64, String)>>,
    tallies: std::sync::Mutex<HashMap<&'static str, u64>>,
    logs: std::sync::Mutex<Vec<String>>,
    temp_media: std::sync::Mutex<VecDeque<(Instant, i32)>>,

    swept_bots: AtomicBool,
    joined: std::sync::Mutex<HashMap<i32, Vec<Joined>>>,
    pending_numbers: std::sync::Mutex<HashMap<i64, PendingNumber>>,

    inflight: OnceLock<tokio::sync::Semaphore>,
}

impl ChatState {
    pub fn bump(&self, counter: &'static str) {
        let mut counters = self.tallies.lock().unwrap();
        let was_empty = counters.is_empty();
        *counters.entry(counter).or_insert(0) += 1;
        if was_empty {
            Dirty::mark(&self.dirty.stats, self.chat);
        }
    }

    pub fn count(&self, user: i64, name: impl FnOnce() -> String, tallies: [&'static str; 2]) {
        {
            let mut counts = self.counts.lock().unwrap();
            let was_empty = counts.is_empty();
            counts.entry(user).or_insert_with(|| (0, name())).0 += 1;
            if was_empty {
                Dirty::mark(&self.dirty.stats, self.chat);
            }
        }
        let mut counters = self.tallies.lock().unwrap();
        let was_empty = counters.is_empty();
        for counter in tallies {
            *counters.entry(counter).or_insert(0) += 1;
        }
        if was_empty {
            Dirty::mark(&self.dirty.stats, self.chat);
        }
    }

    pub async fn slot(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.inflight
            .get_or_init(|| tokio::sync::Semaphore::new(PER_CHAT_UPDATES))
            .acquire()
            .await
            .expect("the per-chat semaphore is never closed")
    }

    fn is_quiet(&self, idle: Duration, now: u64) -> bool {
        now.saturating_sub(self.last_seen.load(Ordering::Relaxed)) >= idle.as_millis() as u64
    }

    fn evictable(&self, idle: Duration, now: u64) -> bool {
        if !self.is_quiet(idle, now) {
            return false;
        }
        self.logs.lock().unwrap().is_empty()
            && self.temp_media.lock().unwrap().is_empty()
            && self.captchas.lock().unwrap().is_empty()
            && self.pending_numbers.lock().unwrap().is_empty()
            && self.counts.lock().unwrap().is_empty()
            && self.tallies.lock().unwrap().is_empty()
    }
}

#[derive(Default)]
struct Dirty {
    logs: std::sync::Mutex<Vec<i64>>,
    media: std::sync::Mutex<Vec<i64>>,

    stats: std::sync::Mutex<Vec<i64>>,
}

impl Dirty {
    fn mark(list: &std::sync::Mutex<Vec<i64>>, chat: i64) {
        list.lock().unwrap().push(chat);
    }

    fn drain(list: &std::sync::Mutex<Vec<i64>>) -> Vec<i64> {
        std::mem::take(&mut *list.lock().unwrap())
    }
}

pub struct Ctx {
    pub client: Client,
    pub settings: Arc<Settings>,

    chats: RwLock<HashMap<i64, Arc<ChatState>>>,

    dirty: Arc<Dirty>,

    deleted: RwLock<HashMap<u64, (Instant, String)>>,
    next_deleted_key: AtomicU64,

    pub started: Instant,

    pending_admins: RwLock<HashMap<u64, promote::Pending>>,

    user: RwLock<Option<Client>>,

    cleaner_id: AtomicI64,

    me_id: AtomicI64,

    user_chats: RwLock<HashMap<i64, PeerRef>>,

    pending_password: RwLock<Option<(Instant, Option<String>)>>,

    join_refs: RwLock<HashMap<String, PeerRef>>,

    pending_writes: std::sync::Mutex<Vec<(i64, i32, i64)>>,

    last_armed: AtomicU64,

    bios: RwLock<HashMap<i64, (Instant, bool)>>,

    bio_fetch: OnceLock<tokio::sync::Semaphore>,

    bot_sweeps: OnceLock<tokio::sync::Semaphore>,

    verdicts: RwLock<HashMap<i64, (Instant, f32, bool, bool)>>,

    nsfw_slots: OnceLock<Arc<tokio::sync::Semaphore>>,

    nsfw_fetches: OnceLock<tokio::sync::Semaphore>,

    margins: RwLock<HashMap<i64, (Instant, [f32; CONCEPT_SLOTS])>>,

    adverts: RwLock<HashMap<i64, (Instant, Option<&'static str>)>>,
}

pub const CONCEPT_SLOTS: usize = 8;

pub const NOTICE_EVERY: Duration = Duration::from_secs(120);

const DELETED_TTL: Duration = Duration::from_secs(3600);
const DELETED_MAX: usize = 5_000;

type PendingNumber = (Instant, &'static str);

type Tallies = HashMap<(i64, &'static str), u64>;

type Counts = HashMap<(i64, i64), (u64, String)>;

const MEMBER_TRUST: Duration = Duration::from_secs(60);

const PENDING_PASSWORD_TTL: Duration = Duration::from_secs(300);

const PENDING_NUMBER_TTL: Duration = Duration::from_secs(120);

const ARMED_WINDOW: u64 = 130_000;

const ADMIN_CACHE_TTL: Duration = Duration::from_secs(1800);

const PER_CHAT_UPDATES: usize = 8;

const ADDS_TTL: Duration = Duration::from_secs(300);

const PER_CHAT_MAX: usize = 20_000;
const CAPTCHA_TTL: Duration = Duration::from_secs(900);

const BIO_TTL: Duration = Duration::from_secs(600);

const BIO_MAX: usize = 10_000;

const BIO_FETCHES: usize = 4;

const VERDICT_TTL: Duration = Duration::from_secs(86_400);

const VERDICT_MAX: usize = 50_000;

const NSFW_SLOTS: usize = 2;

const NSFW_FETCHES: usize = 4;

pub const FLEET_CONCURRENCY: usize = 8;

pub const FLEET_CAMPAIGNS: usize = 4;

pub async fn bounded<T, F>(items: Vec<T>, cap: usize, run: impl Fn(T) -> F)
where
    T: Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let permits = Arc::new(tokio::sync::Semaphore::new(cap));
    let mut tasks = tokio::task::JoinSet::new();
    for item in items {
        let permit = Arc::clone(&permits)
            .acquire_owned()
            .await
            .expect("the fleet semaphore is never closed");
        let work = run(item);
        tasks.spawn(async move {
            let _permit = permit;
            work.await;
        });
    }
    while let Some(done) = tasks.join_next().await {
        if let Err(e) = done {
            eprintln!("fleet job task failed: {e}");
        }
    }
}

impl Ctx {
    pub fn new(client: Client, settings: Arc<Settings>) -> Self {
        Self {
            client,
            settings,
            chats: RwLock::new(HashMap::new()),
            dirty: Arc::default(),
            deleted: RwLock::new(HashMap::new()),
            next_deleted_key: AtomicU64::new(1),
            started: Instant::now(),
            pending_admins: RwLock::new(HashMap::new()),
            user: RwLock::new(None),
            cleaner_id: AtomicI64::new(0),
            me_id: AtomicI64::new(0),
            user_chats: RwLock::new(HashMap::new()),
            pending_password: RwLock::new(None),
            join_refs: RwLock::new(HashMap::new()),
            pending_writes: std::sync::Mutex::new(Vec::new()),
            last_armed: AtomicU64::new(0),
            bios: RwLock::new(HashMap::new()),
            bio_fetch: OnceLock::new(),
            bot_sweeps: OnceLock::new(),
            verdicts: RwLock::new(HashMap::new()),
            nsfw_slots: OnceLock::new(),
            nsfw_fetches: OnceLock::new(),
            margins: RwLock::new(HashMap::new()),
            adverts: RwLock::new(HashMap::new()),
        }
    }

    pub fn state(&self, chat: i64) -> Arc<ChatState> {
        if let Some(state) = self.chats.read().unwrap().get(&chat) {
            return Arc::clone(state);
        }
        let dirty = Arc::clone(&self.dirty);
        Arc::clone(
            self.chats
                .write()
                .unwrap()
                .entry(chat)
                .or_insert_with(|| {
                    Arc::new(ChatState {
                        chat,
                        dirty,
                        ..Default::default()
                    })
                }),
        )
    }

    fn peek(&self, chat: i64) -> Option<Arc<ChatState>> {
        self.chats.read().unwrap().get(&chat).map(Arc::clone)
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
        self.peek(chat).is_some_and(|state| {
            state
                .members
                .lock()
                .unwrap()
                .get(&user)
                .is_some_and(|seen| seen.elapsed() < MEMBER_TRUST)
        })
    }

    pub fn remember_member(&self, chat: i64, user: i64) {
        let state = self.state(chat);
        let mut members = state.members.lock().unwrap();
        if members.len() >= PER_CHAT_MAX {
            members.retain(|_, seen| seen.elapsed() < MEMBER_TRUST);
        }
        members.insert(user, Instant::now());
    }

    pub fn forget_member(&self, chat: i64, user: i64) {
        if let Some(state) = self.peek(chat) {
            state.members.lock().unwrap().remove(&user);
        }
    }

    pub fn claim_bio(&self, user: i64) -> Option<bool> {
        let mut bios = self.bios.write().unwrap();
        if let Some((at, seen)) = bios.get(&user)
            && at.elapsed() < BIO_TTL
        {
            return Some(*seen);
        }
        if bios.len() >= BIO_MAX {
            bios.retain(|_, (at, _)| at.elapsed() < BIO_TTL);
        }
        bios.insert(user, (Instant::now(), false));
        None
    }

    pub fn remember_bio(&self, user: i64, has_link: bool) {
        self.bios
            .write()
            .unwrap()
            .insert(user, (Instant::now(), has_link));
    }

    pub fn forget_bio(&self, user: i64) {
        self.bios.write().unwrap().remove(&user);
    }

    pub async fn bio_slot(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.bio_fetch
            .get_or_init(|| tokio::sync::Semaphore::new(BIO_FETCHES))
            .acquire()
            .await
            .expect("the bio semaphore is never closed")
    }

    pub fn known_verdict(&self, file: i64) -> Option<(f32, bool, bool)> {
        let verdicts = self.verdicts.read().unwrap();
        verdicts
            .get(&file)
            .filter(|(at, ..)| at.elapsed() < VERDICT_TTL)
            .map(|(_, score, innocent, explicit)| (*score, *innocent, *explicit))
    }

    pub fn remember_verdict(&self, file: i64, score: f32, innocent: bool, explicit: bool) {
        let mut verdicts = self.verdicts.write().unwrap();
        if verdicts.len() >= VERDICT_MAX {
            verdicts.retain(|_, (at, ..)| at.elapsed() < VERDICT_TTL);
        }
        verdicts.insert(file, (Instant::now(), score, innocent, explicit));
    }

    pub async fn nsfw_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(
            self.nsfw_slots
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(NSFW_SLOTS))),
        )
        .acquire_owned()
        .await
        .expect("the nsfw semaphore is never closed")
    }

    pub fn known_margins(&self, file: i64) -> Option<[f32; CONCEPT_SLOTS]> {
        let margins = self.margins.read().unwrap();
        margins
            .get(&file)
            .filter(|(at, _)| at.elapsed() < VERDICT_TTL)
            .map(|(_, all)| *all)
    }

    pub fn known_advert(&self, file: i64) -> Option<Option<&'static str>> {
        let adverts = self.adverts.read().unwrap();
        adverts
            .get(&file)
            .filter(|(at, _)| at.elapsed() < VERDICT_TTL)
            .map(|(_, why)| *why)
    }

    pub fn remember_advert(&self, file: i64, why: Option<&'static str>) {
        let mut adverts = self.adverts.write().unwrap();
        if adverts.len() >= VERDICT_MAX {
            adverts.retain(|_, (at, _)| at.elapsed() < VERDICT_TTL);
        }
        adverts.insert(file, (Instant::now(), why));
    }

    pub fn remember_margins(&self, file: i64, all: [f32; CONCEPT_SLOTS]) {
        let mut margins = self.margins.write().unwrap();
        if margins.len() >= VERDICT_MAX {
            margins.retain(|_, (at, _)| at.elapsed() < VERDICT_TTL);
        }
        margins.insert(file, (Instant::now(), all));
    }

    pub async fn nsfw_fetch(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.nsfw_fetches
            .get_or_init(|| tokio::sync::Semaphore::new(NSFW_FETCHES))
            .acquire()
            .await
            .expect("the nsfw fetch semaphore is never closed")
    }

    pub async fn sweep_slot(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.bot_sweeps
            .get_or_init(|| tokio::sync::Semaphore::new(FLEET_CAMPAIGNS))
            .acquire()
            .await
            .expect("the sweep semaphore is never closed")
    }

    pub fn cached_adds(&self, chat: i64, user: i64) -> Option<u64> {
        self.peek(chat)?
            .adds
            .lock()
            .unwrap()
            .get(&user)
            .filter(|(at, _)| at.elapsed() < ADDS_TTL)
            .map(|(_, added)| *added)
    }

    pub fn remember_adds(&self, chat: i64, user: i64, added: u64) {
        let state = self.state(chat);
        let mut adds = state.adds.lock().unwrap();
        if adds.len() >= PER_CHAT_MAX {
            adds.retain(|_, (at, _)| at.elapsed() < ADDS_TTL);
        }
        adds.insert(user, (Instant::now(), added));
    }

    pub fn queue_log(&self, chat: i64, entry: String) {
        const MAX_PER_CHAT: usize = 200;
        let state = self.state(chat);
        let mut entries = state.logs.lock().unwrap();
        let was_empty = entries.is_empty();
        if entries.len() >= MAX_PER_CHAT {
            entries.remove(0);
        }
        entries.push(entry);
        if was_empty {
            Dirty::mark(&self.dirty.logs, chat);
        }
    }

    pub fn queue_temp_media(&self, chat: i64, id: i32, due: Instant, due_at: i64) {
        let state = self.state(chat);
        {
            let mut queue = state.temp_media.lock().unwrap();
            let was_empty = queue.is_empty();
            tempmedia::queue(&mut queue, id, due);
            if was_empty {
                Dirty::mark(&self.dirty.media, chat);
            }
        }
        self.pending_writes.lock().unwrap().push((chat, id, due_at));
    }

    pub fn restore_temp_media(&self, chat: i64, id: i32, due: Instant) {
        let state = self.state(chat);
        let mut queue = state.temp_media.lock().unwrap();
        let was_empty = queue.is_empty();
        tempmedia::queue(&mut queue, id, due);
        if was_empty {
            Dirty::mark(&self.dirty.media, chat);
        }
    }

    pub fn take_pending_writes(&self) -> Vec<(i64, i32, i64)> {
        std::mem::take(&mut *self.pending_writes.lock().unwrap())
    }

    pub fn take_due_media(&self) -> Vec<(i64, Vec<i32>)> {
        let now = Instant::now();
        let mut ready = Vec::new();
        for chat in Dirty::drain(&self.dirty.media) {
            let Some(state) = self.peek(chat) else {
                continue;
            };

            if self.chat_ref(chat).is_none() {
                Dirty::mark(&self.dirty.media, chat);
                continue;
            }
            let due = {
                let mut queue = state.temp_media.lock().unwrap();
                let due = tempmedia::drain_due(&mut queue, now);

                if !queue.is_empty() {
                    Dirty::mark(&self.dirty.media, chat);
                }
                due
            };
            if !due.is_empty() {
                ready.push((chat, due));
            }
        }
        ready
    }

    pub fn take_logs(&self) -> Vec<(i64, Vec<String>)> {
        Dirty::drain(&self.dirty.logs)
            .into_iter()
            .filter_map(|chat| {
                let state = self.peek(chat)?;
                let queued = std::mem::take(&mut *state.logs.lock().unwrap());
                (!queued.is_empty()).then_some((chat, queued))
            })
            .collect()
    }

    fn joined_cached(&self, key: (i64, i32)) -> Option<Vec<Joined>> {
        self.peek(key.0)?.joined.lock().unwrap().get(&key.1).cloned()
    }

    fn remember_joined(&self, key: (i64, i32), joined: Vec<Joined>) {
        const MAX: usize = 200;
        let state = self.state(key.0);
        let mut cache = state.joined.lock().unwrap();
        if cache.len() >= MAX {
            cache.clear();
        }
        cache.insert(key.1, joined);
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
        self.last_armed.store(
            self.started.elapsed().as_millis() as u64,
            Ordering::Relaxed,
        );
        let state = self.state(chat);
        let mut pending = state.pending_numbers.lock().unwrap();
        pending.retain(|_, (armed, _)| armed.elapsed() < PENDING_NUMBER_TTL);
        pending.insert(user, (Instant::now(), setting));
    }

    pub fn take_expected_number(&self, chat: i64, user: i64) -> Option<&'static str> {
        let state = self.peek(chat)?;
        let mut pending = state.pending_numbers.lock().unwrap();
        let (armed, setting) = pending.remove(&user)?;
        (armed.elapsed() < PENDING_NUMBER_TTL).then_some(setting)
    }

    pub fn maybe_expecting_number(&self) -> bool {
        let armed = self.last_armed.load(Ordering::Relaxed);
        armed != 0 && self.started.elapsed().as_millis() as u64 - armed < ARMED_WINDOW
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

    pub fn first_sighting(&self, chat: i64, user: i64, window: Duration) -> bool {
        self.throttle(4, chat, user, window)
    }

    pub fn may_report(&self, chat: i64, user: i64) -> bool {
        self.throttle(1, chat, user, report::EVERY)
    }

    fn throttle(&self, kind: u8, chat: i64, user: i64, every: Duration) -> bool {
        let state = self.state(chat);
        let mut notices = state.notices.lock().unwrap();
        if let Some(last) = notices.get(&(kind, user))
            && last.elapsed() < every
        {
            return false;
        }
        if notices.len() >= PER_CHAT_MAX {
            notices.retain(|_, last| last.elapsed() < NOTICE_EVERY);
        }
        notices.insert((kind, user), Instant::now());
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
        let state = self.state(chat);
        let mut messages = state.messages.lock().unwrap();
        if messages.len() >= PER_CHAT_MAX {
            messages.retain(|_, times| times.iter().any(|t| t.elapsed() < window));
        }
        let times = messages.entry(user).or_default();
        times.retain(|t| t.elapsed() < window);
        times.push(Instant::now());
        times.len()
    }

    pub fn record_removal(&self, chat: i64, actor: i64, window: Duration) -> usize {
        let state = self.state(chat);
        let mut removals = state.removals.lock().unwrap();
        if removals.len() >= PER_CHAT_MAX {
            removals.retain(|_, times| times.iter().any(|t| t.elapsed() < window));
        }
        let times = removals.entry(actor).or_default();
        times.retain(|t| t.elapsed() < window);
        times.push(Instant::now());
        times.len()
    }

    pub fn remember_chat(&self, chat: i64, peer: PeerRef) {
        *self.state(chat).peer.write().unwrap() = Some(peer);
    }

    pub fn bump(&self, chat: i64, counter: &'static str) {
        self.state(chat).bump(counter);
    }

    fn cached_admin(&self, chat: i64, user: i64) -> Option<bool> {
        let state = self.peek(chat)?;
        let admins = state.admins.read().unwrap();
        let (fetched, admins) = admins.as_ref()?;
        (fetched.elapsed() < ADMIN_CACHE_TTL).then(|| admins.contains(&user))
    }

    fn uptime_millis(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn touch(&self, state: &ChatState) {
        state.last_seen.store(self.uptime_millis(), Ordering::Relaxed);
    }

    pub fn evict_idle(&self, idle: Duration) -> usize {
        let now = self.uptime_millis();
        let quiet: Vec<(i64, Arc<ChatState>)> = {
            let chats = self.chats.read().unwrap();
            chats
                .iter()
                .filter(|(_, state)| state.is_quiet(idle, now))
                .map(|(chat, state)| (*chat, Arc::clone(state)))
                .collect()
        };

        let mut going: Vec<i64> = Vec::new();
        for (chat, state) in quiet {
            if !self.settings.is_locked(chat, bots::LOCK) && state.evictable(idle, now) {
                going.push(chat);
            }
        }
        if going.is_empty() {
            return 0;
        }

        let mut chats = self.chats.write().unwrap();
        let before = chats.len();
        for chat in going {
            if chats
                .get(&chat)
                .is_some_and(|state| Arc::strong_count(state) == 1 && state.is_quiet(idle, now))
            {
                chats.remove(&chat);
            }
        }
        before - chats.len()
    }

    pub fn take_stats(&self) -> (Tallies, Counts) {
        let mut tallies = HashMap::new();
        let mut counts = HashMap::new();
        for chat in Dirty::drain(&self.dirty.stats) {
            let Some(state) = self.peek(chat) else {
                continue;
            };
            for (counter, count) in std::mem::take(&mut *state.tallies.lock().unwrap()) {
                tallies.insert((chat, counter), count);
            }
            let mut per_user = state.counts.lock().unwrap();
            let room = per_user.len();
            for (user, count) in std::mem::replace(&mut *per_user, HashMap::with_capacity(room)) {
                counts.insert((chat, user), count);
            }
        }
        (tallies, counts)
    }

    pub fn captcha_start(&self, chat: i64, user: i64, pending: captcha::Pending) {
        let state = self.state(chat);
        let mut captchas = state.captchas.lock().unwrap();
        captchas.retain(|_, p| p.started.elapsed() < CAPTCHA_TTL);
        captchas.insert(user, pending);
    }

    pub fn captcha_pending(&self, chat: i64, user: i64) -> Option<captcha::Pending> {
        self.peek(chat)?.captchas.lock().unwrap().get(&user).cloned()
    }

    pub fn captcha_done(&self, chat: i64, user: i64) {
        if let Some(state) = self.peek(chat) {
            state.captchas.lock().unwrap().remove(&user);
        }
    }

    fn live_ref(&self, chat: i64) -> Option<PeerRef> {
        *self.peek(chat)?.peer.read().unwrap()
    }

    pub fn chat_ref(&self, chat: i64) -> Option<PeerRef> {
        if let Some(peer) = self.live_ref(chat) {
            return Some(peer);
        }
        let hash = self.settings.value_parsed::<i64>(chat, HASH)?;
        Some(PeerRef {
            id: PeerId::from_bot_api_dialog_id(chat)?,
            auth: PeerAuth::from_hash(hash),
        })
    }

    pub fn forget_admins(&self, chat: i64) {
        if let Some(state) = self.peek(chat) {
            *state.admins.write().unwrap() = None;
        }
    }

    fn cache_admins(&self, chat: i64, admins: HashSet<i64>) {
        *self.state(chat).admins.write().unwrap() = Some((Instant::now(), admins));
    }

    fn cached_admins(&self, chat: i64) -> Option<HashSet<i64>> {
        let state = self.peek(chat)?;
        let admins = state.admins.read().unwrap();
        let (fetched, admins) = admins.as_ref()?;
        (fetched.elapsed() < ADMIN_CACHE_TTL).then(|| admins.clone())
    }
}

pub async fn is_admin(ctx: &Ctx, chat_ref: PeerRef, chat: i64, user: i64) -> bool {
    owner(ctx, chat) == Some(user)
        || is_bot_admin(ctx, chat, user)
        || chat_admins(ctx, chat_ref, chat)
            .await
            .is_some_and(|admins| admins.contains(&user))
}

fn holds_the_group(participant: &grammers_client::peer::Participant) -> bool {
    matches!(
        participant.role,
        grammers_client::peer::Role::Admin(_) | grammers_client::peer::Role::Creator(_)
    )
}

pub async fn chat_admins(ctx: &Ctx, chat_ref: PeerRef, chat: i64) -> Option<HashSet<i64>> {
    if let Some(admins) = ctx.cached_admins(chat) {
        return Some(admins);
    }

    let state = ctx.state(chat);
    let _fetching = state.admin_fetch.lock().await;
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
                if holds_the_group(&participant) {
                    admins.insert(participant.user.id().bare_id_unchecked());
                }
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("could not list admins of {chat}: {e}");
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
                bots::on_participant_update(ctx, update).await;
                raid::on_participant_update(ctx, update).await;
                log::on_participant(ctx, update).await;
            }
            pinlock::on_raw(ctx, &raw).await;
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
            || config::help(ctx, message).await
            || cleaner::handle(ctx, message).await
            || panel::handle_private(ctx, message).await;
        return;
    }

    let Some(chat) = chat_id(message) else {
        return;
    };

    if ctx.live_ref(chat).is_none()
        && let Ok(Some(peer)) = message.peer_ref().await
    {
        ctx.remember_chat(chat, peer);

        if let Some(title) = message.peer().and_then(|peer| peer.name())
            && ctx.settings.value(chat, TITLE).as_deref() != Some(title)
        {
            ctx.settings.set_value(chat, TITLE, title).await;
        }

        let hash = peer.auth.hash();
        if ctx.settings.value_parsed::<i64>(chat, HASH) != Some(hash) {
            ctx.settings.set_value(chat, HASH, &hash.to_string()).await;
        }
    }

    let state = ctx.state(chat);
    ctx.touch(&state);
    let _slot = state.slot().await;

    if !state.swept_bots.load(Ordering::Relaxed)
        && ctx.settings.is_locked(chat, bots::LOCK)
        && !state.swept_bots.swap(true, Ordering::Relaxed)
    {
        let ctx = Arc::clone(ctx);
        tokio::spawn(async move {
            let _slot = ctx.sweep_slot().await;
            bots::sweep(&ctx, chat).await;
        });
    }

    let view = locks::View::new(message);

    stats::count(&state, message, &view);
    if matches!(
        message.action(),
        Some(grammers_client::tl::enums::MessageAction::ChatDeleteUser(_))
    ) {
        state.bump(stats::LEFT);
    }

    if matches!(
        message.action(),
        Some(
            grammers_client::tl::enums::MessageAction::ChatAddUser(_)
                | grammers_client::tl::enums::MessageAction::ChatJoinedByLink(_)
        )
    ) {
        state.bump(stats::JOINED);

        let _ = biolink::tripped(ctx, chat, message).await;
        if let Some(grammers_client::tl::enums::MessageAction::ChatAddUser(action)) =
            message.action()
        {
            stats::count_add(ctx, message, action.users.len()).await;
        }
        raid::check(ctx, message, chat).await;
    }
    flood::check(ctx, message).await;

    tempmedia::watch(ctx, message, &view).await;

    nsfw::watch(ctx, message, chat, &view).await;

    if panel::typed_number(ctx, message, &view).await {
        return;
    }

    if join::enforce(ctx, message).await {
        return;
    }

    let bot_authored = bot_authored(message);

    let _ = locks::handle(ctx, message, &view).await
        || cleaner::on_join(ctx, message).await
        || autoconfig::on_message(ctx, message).await
        || bots::handle(ctx, message).await
        || captcha::on_join(ctx, message).await
        || welcome::on_join(ctx, message).await
        || (!bot_authored
            && (welcome::handle(ctx, message).await
                || config::handle(ctx, message).await
                || panel::handle(ctx, message).await
                || restrict::handle(ctx, message, &view).await
                || lists::command(ctx, message).await
                || ping::handle(ctx, message).await
                || nsfw::test(ctx, message).await
                || promote::handle(ctx, message, &view).await
                || stats::handle(ctx, message).await
                || report::handle(ctx, message).await
                || packs::handle(ctx, message).await
                || extras::handle(ctx, message, &view).await
                || tune::handle(ctx, message, &view).await
                || warns::handle(ctx, message, &view).await
                || filters::handle(ctx, message).await
                || purge::handle(ctx, message, &view).await
                || purge::handle_all(ctx, message).await
                || flood::handle(ctx, message, &view).await
                || join::handle(ctx, message).await
                || cleaner::add(ctx, message).await
                || cleaner::wipe(ctx, message).await
                || cleaner::sweep(ctx, message).await
                || log::handle(ctx, message).await
                || rights::handle(ctx, message).await
                || vip::handle(ctx, message).await
                || bots::allow(ctx, message).await))
        || (!bot_authored && answers::handle(ctx, message, &view).await);

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

pub const HASH: &str = "hash";

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

pub fn is_linked_post(message: &Message) -> bool {
    let grammers_client::tl::enums::Message::Message(raw) = &message.raw else {
        return false;
    };
    let Some(grammers_client::tl::enums::MessageFwdHeader::Header(header)) = raw.fwd_from.as_ref()
    else {
        return false;
    };
    let Some(origin) = header.saved_from_peer.clone() else {
        return false;
    };

    message.sender_id() == Some(PeerId::from(origin))
}

pub async fn is_exempt(ctx: &Ctx, message: &Message) -> bool {
    if is_linked_post(message) {
        return true;
    }
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

    if let Some(sender) = message.sender_id()
        && sender.kind() == PeerKind::Channel
    {
        return sender == message.peer_id();
    }
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

pub struct Named<'a> {
    arg: Option<&'a str>,
}

pub(crate) fn arg_names_a_user(arg: &str) -> bool {
    arg.strip_prefix('@').is_some_and(|name| !name.is_empty())
        || digits(arg).trim().parse::<i64>().is_ok()
}

pub fn named<'a>(message: &Message, arg: Option<&'a str>) -> Option<Named<'a>> {
    let names_somebody = match arg {
        Some(arg) => arg_names_a_user(arg),
        None => message.reply_to_message_id().is_some(),
    };
    names_somebody.then_some(Named { arg })
}

pub async fn resolve(
    ctx: &Ctx,
    message: &Message,
    named: Named<'_>,
) -> Option<(PeerRef, String)> {
    if let Some(arg) = named.arg {
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

pub fn phrase_carries_text(command: &str) -> bool {
    command.split_whitespace().count() > 1
}

pub fn numbers_in(tail: &str) -> Option<Vec<u32>> {
    digits(tail)
        .split_whitespace()
        .map(|word| word.parse().ok())
        .collect()
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
        if participant.user.id().bare_id_unchecked() != user_id || !holds_the_group(&participant) {
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
        if !holds_the_group(&participant) {
            continue;
        }
        let name = esc(&participant.user.full_name());

        if participant.user.is_bot() {
            names.push(format!("‹ {name} · ربات"));
            continue;
        }
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
    fn a_queue_that_refills_during_a_drain_is_not_lost() {
        use std::sync::Mutex;

        const CHAT: i64 = 7;
        let queue: Mutex<Vec<&str>> = Mutex::new(Vec::new());
        let dirty: Mutex<Vec<i64>> = Mutex::new(Vec::new());

        let push = |entry| {
            let mut queued = queue.lock().unwrap();
            let was_empty = queued.is_empty();
            queued.push(entry);
            if was_empty {
                super::Dirty::mark(&dirty, CHAT);
            }
        };
        let drain = || {
            super::Dirty::drain(&dirty)
                .into_iter()
                .map(|_| std::mem::take(&mut *queue.lock().unwrap()))
                .collect::<Vec<_>>()
        };

        push("one");
        push("two");
        assert_eq!(
            dirty.lock().unwrap().len(),
            1,
            "a busy chat marks once per window, not once per entry"
        );
        assert_eq!(drain(), vec![vec!["one", "two"]]);

        push("three");
        assert_eq!(drain(), vec![vec!["three"]]);

        assert!(drain().is_empty(), "a chat with nothing queued is never visited");
    }

    #[test]
    fn a_stored_hash_rebuilds_the_peer_a_message_would_have_given() {
        use grammers_client::session::types::{PeerAuth, PeerId, PeerRef};

        for (chat, hash) in [(-1_001_234_567_890_i64, 8_123_456_789_i64), (-4_242, 0)] {
            let stored = hash.to_string();
            let parsed: i64 = stored.parse().unwrap();
            let rebuilt = PeerRef {
                id: PeerId::from_bot_api_dialog_id(chat).unwrap(),
                auth: PeerAuth::from_hash(parsed),
            };
            assert_eq!(rebuilt.id.bot_api_dialog_id(), Some(chat));
            assert_eq!(rebuilt.auth.hash(), hash);
        }
    }

    #[tokio::test]
    async fn bounded_never_exceeds_its_cap() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const CAP: usize = 4;
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let ran = Arc::new(AtomicUsize::new(0));

        let (live_in, peak_in, ran_in) = (Arc::clone(&live), Arc::clone(&peak), Arc::clone(&ran));
        super::bounded((0..100).collect(), CAP, move |_| {
            let (live, peak, ran) = (
                Arc::clone(&live_in),
                Arc::clone(&peak_in),
                Arc::clone(&ran_in),
            );
            async move {
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                ran.fetch_add(1, Ordering::SeqCst);
                live.fetch_sub(1, Ordering::SeqCst);
            }
        })
        .await;

        assert_eq!(ran.load(Ordering::SeqCst), 100, "every item has to run");
        assert!(peak.load(Ordering::SeqCst) <= CAP, "the cap has to hold");
        assert_eq!(live.load(Ordering::SeqCst), 0, "it has to wait for all of them");
    }

    #[test]
    fn digits_normalise_both_digit_sets_and_borrow_otherwise() {
        use super::digits;
        use std::borrow::Cow;

        assert_eq!(digits("ضد رگبار ۱۰ ۵"), "ضد رگبار 10 5");
        assert_eq!(digits("حذف ٥"), "حذف 5");
        assert_eq!(digits("۰۱۲۳۴۵۶۷۸۹"), "0123456789");
        assert_eq!(digits("٠١٢٣٤٥٦٧٨٩"), "0123456789");
        assert_eq!(digits("سکوت ۳۰ دقیقه"), "سکوت 30 دقیقه");

        assert!(matches!(digits("سلام دوستان"), Cow::Borrowed(_)));
        assert!(matches!(digits("ban 10"), Cow::Borrowed(_)));
        assert!(matches!(digits(""), Cow::Borrowed(_)));
        assert!(matches!(digits("۱"), Cow::Owned(_)));
    }

    #[test]
    fn counting_a_message_keeps_the_first_name_and_loses_no_count() {
        use super::ChatState;

        let state = ChatState::default();
        state.count(7, || "Ali".to_owned(), ["k_text", "h9"]);
        state.count(7, || panic!("the name is read once, not per message"), ["k_photo", "h9"]);

        let counts = state.counts.lock().unwrap();
        assert_eq!(counts.get(&7), Some(&(2, "Ali".to_owned())));

        let tallies = state.tallies.lock().unwrap();
        assert_eq!(tallies.get("k_text"), Some(&1));
        assert_eq!(tallies.get("k_photo"), Some(&1));
        assert_eq!(tallies.get("h9"), Some(&2));
        drop((counts, tallies));

        assert!(
            state.dirty.stats.lock().unwrap().len() <= 2,
            "the mark is a transition, not a per-message write"
        );
    }

    #[test]
    fn a_chat_with_queued_work_is_never_evicted() {
        use super::{ChatState, captcha};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        const IDLE: Duration = Duration::from_secs(3600);

        let now = (IDLE.as_millis() as u64) * 3 / 2;

        let state = ChatState::default();
        assert!(state.evictable(IDLE, now), "quiet and empty");

        state.logs.lock().unwrap().push("a line".to_owned());
        assert!(!state.evictable(IDLE, now));
        state.logs.lock().unwrap().clear();

        state.temp_media.lock().unwrap().push_back((Instant::now(), 1));
        assert!(!state.evictable(IDLE, now));
        state.temp_media.lock().unwrap().clear();

        state.captchas.lock().unwrap().insert(
            7,
            captcha::Pending {
                answer: 0,
                message_id: 1,
                started: Instant::now(),
            },
        );
        assert!(!state.evictable(IDLE, now));
        state.captchas.lock().unwrap().clear();

        state.bump("joined");
        assert!(!state.evictable(IDLE, now));
        state.tallies.lock().unwrap().clear();

        state.count(7, || "Ali".to_owned(), ["k_text", "h9"]);
        assert!(!state.evictable(IDLE, now));
        state.counts.lock().unwrap().clear();
        state.tallies.lock().unwrap().clear();

        assert!(state.evictable(IDLE, now), "emptied again, so it may go");

        state.last_seen.store(now, Ordering::Relaxed);
        assert!(!state.evictable(IDLE, now));
    }

    #[test]
    fn stale_cache_entries_do_not_pin_an_idle_chat() {
        use super::ChatState;
        use std::time::{Duration, Instant};

        const IDLE: Duration = Duration::from_secs(3600);
        let now = (IDLE.as_millis() as u64) * 3 / 2;

        let state = ChatState::default();

        state.messages.lock().unwrap().insert(7, vec![Instant::now()]);
        state.removals.lock().unwrap().insert(7, vec![Instant::now()]);
        state.members.lock().unwrap().insert(7, Instant::now());
        state.adds.lock().unwrap().insert(7, (Instant::now(), 1));
        state.notices.lock().unwrap().insert((2, 7), Instant::now());
        state.joined.lock().unwrap().insert(1, Vec::new());

        assert!(
            state.evictable(IDLE, now),
            "stale windows are caches, not work — they must not pin the chat forever"
        );

        state.logs.lock().unwrap().push("unsent".to_owned());
        assert!(!state.evictable(IDLE, now));
        state.logs.lock().unwrap().clear();

        state.bump("joined");
        assert!(!state.evictable(IDLE, now), "unflushed counters are work");
        state.tallies.lock().unwrap().clear();

        assert!(state.evictable(IDLE, now));
    }

    #[test]
    fn a_flushed_chat_marks_itself_again_when_it_speaks() {
        use super::{ChatState, Dirty};

        let state = ChatState::default();
        state.bump("joined");
        assert_eq!(Dirty::drain(&state.dirty.stats), vec![state.chat]);

        state.tallies.lock().unwrap().clear();
        assert!(Dirty::drain(&state.dirty.stats).is_empty());

        state.bump("left");
        assert_eq!(Dirty::drain(&state.dirty.stats), vec![state.chat]);
    }

    #[test]
    fn a_closed_tail_is_all_or_nothing() {
        use super::numbers_in;
        let word = "چیه";

        assert_eq!(numbers_in(""), Some(vec![]));
        assert_eq!(numbers_in(" 50 "), Some(vec![50]));
        assert_eq!(numbers_in("10 5"), Some(vec![10, 5]));
        assert_eq!(numbers_in("۱۰ ۵"), Some(vec![10, 5]));

        assert_eq!(numbers_in(word), None);
        assert_eq!(numbers_in(&format!("{word} 50")), None);
        assert_eq!(numbers_in(&format!("50 {word}")), None);
    }

    #[test]
    fn sources_have_no_zero_width_non_joiner() {
        for file in [
            include_str!("mod.rs"),
            include_str!("betrayal.rs"),
            include_str!("biolink.rs"),
            include_str!("bots.rs"),
            include_str!("callbacks.rs"),
            include_str!("captcha.rs"),
            include_str!("config.rs"),
            include_str!("extras.rs"),
            include_str!("filters.rs"),
            include_str!("flood.rs"),
            include_str!("help.rs"),
            include_str!("limits.rs"),
            include_str!("lists.rs"),
            include_str!("locks.rs"),
            include_str!("notice.rs"),
            include_str!("nsfw.rs"),
            include_str!("packs.rs"),
            include_str!("panel.rs"),
            include_str!("ping.rs"),
            include_str!("promote.rs"),
            include_str!("purge.rs"),
            include_str!("report.rs"),
            include_str!("pinlock.rs"),
            include_str!("raid.rs"),
            include_str!("restrict.rs"),
            include_str!("setting.rs"),
            include_str!("stats.rs"),
            include_str!("strict.rs"),
            include_str!("style.rs"),
            include_str!("tempmedia.rs"),
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
