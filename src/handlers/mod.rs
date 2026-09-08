pub mod answers;
pub mod autoconfig;
pub mod betrayal;
pub mod biolink;
pub mod bots;
pub mod callbacks;
pub mod captcha;
pub mod cases;
pub mod cleaner;
pub mod cleaner_setup;
pub mod comment;
pub mod concept_vectors;
pub mod concepts;
pub mod config;
pub mod currency;
pub mod emoji_image;
pub mod ephemeral;
pub mod extras;
pub mod filters;
pub mod flood;
pub mod help;
pub mod imgfilter;
pub mod imgtext;
pub mod install;
pub mod intent;
pub mod invite;
pub mod join;
pub mod leftback;
pub mod limits;
pub mod lists;
pub mod locks;
pub mod log;
pub mod notice;
pub mod nsfw;
pub mod nsfw_head;
pub mod nsfw_head_vectors;
pub mod ocr;
pub mod packs;
pub mod panel;
pub mod ping;
pub mod pinlock;
pub mod premium;
pub mod promote;
pub mod purge;
pub mod raid;
pub mod report;
pub mod restrict;
pub mod rights;
pub mod setting;
pub mod stats;
pub mod strict;
pub mod style;
pub mod sudo;
pub mod tempmedia;
pub mod toggles;
pub mod trade;
pub mod tune;
pub mod vip;
pub mod vision;
pub mod voicemonitor;
pub mod warns;
pub mod welcome;

use std::cmp::Ordering as CmpOrdering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::num::NonZeroI64;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use grammers_client::Client;
use grammers_client::message::Message;
use grammers_client::peer::Peer;
use grammers_client::session::types::{PeerAuth, PeerId, PeerKind, PeerRef};
use grammers_client::update::Update;

use crate::state::{Settings, SettingsWriteError};

#[derive(Debug)]
pub enum ChatAdmissionError {
    RouteRejected,
    RuntimeCapacityReached,
    DurableCapacityReached(&'static str),
    Persistence(SettingsWriteError),
}

impl std::fmt::Display for ChatAdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RouteRejected => formatter.write_str("chat is not assigned to this shard"),
            Self::RuntimeCapacityReached => formatter.write_str("runtime chat capacity reached"),
            Self::DurableCapacityReached(scope) => {
                write!(formatter, "durable {scope} capacity reached")
            }
            Self::Persistence(error) => write!(formatter, "durable chat admission failed: {error}"),
        }
    }
}

#[derive(Default)]
pub struct ChatState {
    chat: i64,
    dirty: Arc<Dirty>,

    last_seen: AtomicU64,

    peer: RwLock<Option<PeerRef>>,

    admins: RwLock<Option<(Instant, HashSet<i64>)>>,
    admin_fetch: tokio::sync::Mutex<()>,
    configure_lock: Arc<tokio::sync::Mutex<()>>,
    setup_next_check: AtomicU64,

    messages: std::sync::Mutex<HashMap<i64, VecDeque<Instant>>>,
    removals: std::sync::Mutex<HashMap<i64, VecDeque<Instant>>>,
    notices: std::sync::Mutex<HashMap<(u8, i64), (Instant, Duration)>>,
    members: std::sync::Mutex<HashMap<i64, Instant>>,
    adds: std::sync::Mutex<HashMap<i64, (Instant, u64)>>,
    counts: std::sync::Mutex<HashMap<i64, (u64, String)>>,
    tallies: std::sync::Mutex<HashMap<&'static str, u64>>,
    logs: std::sync::Mutex<Vec<String>>,
    temp_media: std::sync::Mutex<VecDeque<(Instant, i32)>>,
    said: std::sync::Mutex<VecDeque<(i64, i32)>>,
    roots: std::sync::Mutex<HashMap<i32, (Instant, Root)>>,
    swept_bots: AtomicBool,
    tag_run: AtomicU64,
    joined: std::sync::Mutex<HashMap<i32, Vec<Joined>>>,

    inflight: AtomicUsize,
    inflight_notify: tokio::sync::Notify,
}

pub struct Queued {
    pub message: i32,
    pub sender: Option<i64>,
    pub name: String,
}

pub enum Root {
    Post,
    NotPost,
    Pending(Vec<Queued>),
}

pub enum RootClaim {
    Known(bool),
    Mine,
    Waiting,
}

const ROOTS_MAX: usize = 1_024;

const ROOT_QUEUE_MAX: usize = 32;

pub struct ChatPermit<'a> {
    state: &'a ChatState,
}

pub(super) struct GroupRightsGuard<'a> {
    chat: i64,
    _guard: tokio::sync::MutexGuard<'a, ()>,
}

impl GroupRightsGuard<'_> {
    pub(super) fn chat(&self) -> i64 {
        self.chat
    }
}

impl Drop for ChatPermit<'_> {
    fn drop(&mut self) {
        let previous = self.state.inflight.fetch_sub(1, Ordering::Release);
        debug_assert!(previous > 0);
        self.state.inflight_notify.notify_one();
    }
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

    pub fn remember_said(&self, user: i64, id: i32) {
        let mut said = self.said.lock().unwrap();
        while said.len() >= SAID_MAX {
            said.pop_front();
        }
        said.push_back((user, id));
    }

    pub fn take_said(&self, user: i64) -> Vec<i32> {
        let mut said = self.said.lock().unwrap();
        let mut mine = Vec::new();
        said.retain(|(who, id)| {
            if *who == user {
                mine.push(*id);
                return false;
            }
            true
        });
        mine
    }

    pub fn count(&self, user: i64, name: impl FnOnce() -> String, tallies: [&'static str; 2]) {
        {
            let mut counts = self.counts.lock().unwrap();
            let was_empty = counts.is_empty();
            if counts.contains_key(&user) || counts.len() < PER_CHAT_MAX {
                counts.entry(user).or_insert_with(|| (0, name())).0 += 1;
            }
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

    pub fn claim_tagging(&self, command: i32) -> u64 {
        let token = u64::from(command.max(0) as u32);
        self.tag_run.fetch_max(token, Ordering::AcqRel);
        token
    }

    pub fn tagging(&self, token: u64) -> bool {
        self.tag_run.load(Ordering::Acquire) == token
    }

    pub fn tagging_now(&self) -> bool {
        self.tag_run.load(Ordering::Acquire) != 0
    }

    pub fn stop_tagging(&self) {
        self.tag_run.store(0, Ordering::Release);
    }

    #[must_use]
    pub fn finish_tagging(&self, token: u64) -> bool {
        self.tag_run
            .compare_exchange(token, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub async fn slot(&self) -> ChatPermit<'_> {
        loop {
            let notified = self.inflight_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let acquired = self
                .inflight
                .fetch_update(Ordering::Acquire, Ordering::Relaxed, |current| {
                    (current < PER_CHAT_UPDATES).then_some(current + 1)
                })
                .is_ok();
            if acquired {
                return ChatPermit { state: self };
            }
            notified.await;
        }
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
            && self.counts.lock().unwrap().is_empty()
            && self.tallies.lock().unwrap().is_empty()
            && self.configure_lock.try_lock().is_ok()
    }

    pub fn remember_post(&self, id: i32) {
        let mut roots = self.roots.lock().unwrap();
        if !roots.contains_key(&id) && roots.len() >= ROOTS_MAX {
            make_room(&mut roots, ROOTS_MAX, |(at, _)| *at);
        }
        roots.insert(id, (Instant::now(), Root::Post));
    }

    pub fn root_known(&self, root: i32) -> Option<bool> {
        match self.roots.lock().unwrap().get(&root) {
            Some((_, Root::Post)) => Some(true),
            Some((_, Root::NotPost)) => Some(false),
            Some((_, Root::Pending(_))) | None => None,
        }
    }

    pub fn claim_root(&self, root: i32, waiting: Queued) -> RootClaim {
        let mut roots = self.roots.lock().unwrap();
        match roots.get_mut(&root) {
            Some((_, Root::Post)) => return RootClaim::Known(true),
            Some((_, Root::NotPost)) => return RootClaim::Known(false),
            Some((_, Root::Pending(queue))) => {
                if queue.len() < ROOT_QUEUE_MAX {
                    queue.push(waiting);
                }
                return RootClaim::Waiting;
            }
            None => {}
        }
        if roots.len() >= ROOTS_MAX {
            make_room(&mut roots, ROOTS_MAX, |(at, _)| *at);
        }
        roots.insert(root, (Instant::now(), Root::Pending(vec![waiting])));
        RootClaim::Mine
    }

    pub fn settle_root(&self, root: i32, post: bool) -> Vec<Queued> {
        let mut roots = self.roots.lock().unwrap();
        if !roots.contains_key(&root) && roots.len() >= ROOTS_MAX {
            make_room(&mut roots, ROOTS_MAX, |(at, _)| *at);
        }
        let settled = if post { Root::Post } else { Root::NotPost };
        match roots.insert(root, (Instant::now(), settled)) {
            Some((_, Root::Pending(queue))) => queue,
            _ => Vec::new(),
        }
    }

    pub fn forget_root(&self, root: i32) {
        self.roots.lock().unwrap().remove(&root);
    }
}

#[derive(Default)]
struct DirtyList(std::sync::Mutex<DirtyQueue>);

#[derive(Default)]
struct DirtyQueue {
    members: HashSet<i64>,
    order: VecDeque<i64>,
}

impl DirtyQueue {
    fn len(&self) -> usize {
        self.members.len()
    }
    fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

#[derive(Default)]
struct Dirty {
    logs: DirtyList,
    media: DirtyList,
    stats: DirtyList,
}

impl Dirty {
    fn mark(list: &DirtyList, chat: i64) {
        let mut queue = list.0.lock().unwrap();
        if queue.members.insert(chat) {
            queue.order.push_back(chat);
        }
    }

    fn take(list: &DirtyList, limit: usize) -> Vec<i64> {
        if limit == 0 {
            return Vec::new();
        }
        let mut dirty = list.0.lock().unwrap();
        let mut selected = Vec::with_capacity(limit.min(dirty.len()));
        for _ in 0..limit {
            let Some(chat) = dirty.order.pop_front() else {
                break;
            };
            dirty.members.remove(&chat);
            selected.push(chat);
        }
        selected
    }

    #[cfg(test)]
    fn drain(list: &DirtyList) -> Vec<i64> {
        let count = list.0.lock().unwrap().len();
        Self::take(list, count)
    }
}

type LoadedFilters = RwLock<HashMap<i64, (Instant, Arc<Vec<imgfilter::Filter>>)>>;
type VoiceVerdict = (Instant, bool, Option<Arc<str>>);
type FilteredVoice = (Instant, i64, Option<i64>, String);
type CachedVerdict = (Instant, nsfw::Judgement, bool);
type CachedMargins = (Instant, [f32; CONCEPT_SLOTS], bool);
type CachedCustom = (Instant, f32, bool);

#[derive(Clone, Copy)]
pub struct CapacitySnapshot {
    pub runtime_chats: usize,
    pub settings_chats: usize,
    pub settings_rows: usize,
    pub settings_bytes: usize,
    pub user_chats: usize,
    pub dirty_logs: usize,
    pub dirty_media: usize,
    pub dirty_stats: usize,
    pub pending_writes: usize,
    pub pending_drops: usize,
    pub deferred: usize,
    pub pending_admins: usize,
    pub deleted: usize,
    pub verdicts: usize,
    pub voice_verdicts: usize,
    pub filtered_voices: usize,
    pub outbound_active: usize,
    pub outbound_waiting: usize,
    pub outbound_critical_waiting: usize,
}

struct PendingPassword {
    id: u64,
    armed: Instant,
    deliver: tokio::sync::oneshot::Sender<String>,
}

pub(super) struct PasswordWait {
    id: u64,
    receive: tokio::sync::oneshot::Receiver<String>,
}

pub struct BotIdentity {
    id: NonZeroI64,
    username: Option<String>,
}

impl BotIdentity {
    pub fn new(id: NonZeroI64, username: Option<String>) -> Self {
        Self { id, username }
    }
}

#[derive(Default)]
struct PasswordMailbox {
    next_id: AtomicU64,
    pending: Mutex<Option<PendingPassword>>,
}

impl PasswordMailbox {
    fn arm(&self) -> PasswordWait {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (deliver, receive) = tokio::sync::oneshot::channel();
        *self.pending.lock().unwrap() = Some(PendingPassword {
            id,
            armed: Instant::now(),
            deliver,
        });
        PasswordWait { id, receive }
    }

    fn give(&self, password: String) -> bool {
        let Some(request) = self.pending.lock().unwrap().take() else {
            return false;
        };
        request.armed.elapsed() < PENDING_PASSWORD_TTL && request.deliver.send(password).is_ok()
    }

    fn cancel(&self, id: u64) {
        let mut pending = self.pending.lock().unwrap();
        if pending.as_ref().is_some_and(|request| request.id == id) {
            pending.take();
        }
    }

    async fn wait(&self, request: PasswordWait, timeout: Duration) -> Option<String> {
        let id = request.id;
        match tokio::time::timeout(timeout, request.receive).await {
            Ok(Ok(password)) => Some(password),
            Ok(Err(_)) | Err(_) => {
                self.cancel(id);
                None
            }
        }
    }
}

pub struct BackgroundEpochGuard<'a> {
    _epoch: tokio::sync::RwLockReadGuard<'a, ()>,
    panicked: &'a AtomicBool,
}

#[derive(Default)]
struct PersistenceBarrier {
    poisoned: AtomicBool,
    wake_supervisor: tokio::sync::Notify,
}

impl PersistenceBarrier {
    fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
        self.wake_supervisor.notify_one();
    }

    fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    async fn wait(&self) {
        loop {
            let notified = self.wake_supervisor.notified();
            if self.is_poisoned() {
                return;
            }
            notified.await;
        }
    }
}

impl Drop for BackgroundEpochGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.panicked.store(true, Ordering::Release);
        }
    }
}

pub struct Ctx {
    pub client: Client,
    pub settings: Arc<Settings>,
    bot_session: Arc<grammers_session::storages::ErasedSession>,
    max_runtime_chats: usize,
    allowed_chats: Option<Arc<HashSet<i64>>>,

    chats: RwLock<HashMap<i64, Arc<ChatState>>>,

    owned_tasks: std::sync::Mutex<OwnedTasks>,

    background_epoch: tokio::sync::RwLock<()>,

    background_panicked: AtomicBool,

    dirty: Arc<Dirty>,

    deleted: RwLock<HashMap<u64, (Instant, String)>>,
    next_deleted_key: AtomicU64,

    pub started: Instant,

    pending_admins: RwLock<HashMap<u64, promote::Pending>>,

    user: RwLock<Option<Client>>,

    cleaner_id: AtomicI64,

    bot_identity: BotIdentity,
    runtime_config: RuntimeConfig,

    user_chats: RwLock<HashMap<i64, PeerRef>>,

    pending_password: PasswordMailbox,

    cleaner_login: tokio::sync::Mutex<()>,

    restriction_writes: Box<[tokio::sync::Mutex<()>]>,

    group_rights: Box<[tokio::sync::Mutex<()>]>,

    join_refs: RwLock<HashMap<String, PeerRef>>,

    pending_writes: std::sync::Mutex<Vec<(i64, i32, i64)>>,

    pending_drops: std::sync::Mutex<Vec<(i64, i32)>>,

    persistence_barrier: PersistenceBarrier,

    deferred_deletes: std::sync::Mutex<BinaryHeap<DeferredEntry>>,
    next_deferred: AtomicU64,

    last_armed: AtomicU64,

    pending_numbers: std::sync::Mutex<PendingNumbers>,

    bios: RwLock<HashMap<i64, (Instant, bool)>>,

    bio_fetch: OnceLock<Arc<tokio::sync::Semaphore>>,

    bot_sweeps: OnceLock<Arc<tokio::sync::Semaphore>>,

    comment_lookups: OnceLock<Arc<tokio::sync::Semaphore>>,

    cleaner_joins: OnceLock<Arc<tokio::sync::Semaphore>>,

    tag_runs: OnceLock<Arc<tokio::sync::Semaphore>>,

    verdicts: RwLock<HashMap<i64, CachedVerdict>>,

    voice_verdicts: RwLock<HashMap<(i64, u64), VoiceVerdict>>,

    filtered_voices: RwLock<HashMap<u64, FilteredVoice>>,

    voice_pool: OnceLock<Arc<voicemonitor::VoicePool>>,

    voice_jobs: OnceLock<Arc<tokio::sync::Semaphore>>,

    nsfw_slots: OnceLock<Arc<tokio::sync::Semaphore>>,

    nsfw_tasks: OnceLock<Arc<tokio::sync::Semaphore>>,

    nsfw_fetches: OnceLock<tokio::sync::Semaphore>,

    margins: RwLock<HashMap<i64, CachedMargins>>,

    adverts: RwLock<HashMap<i64, (Instant, Option<&'static str>)>>,

    image_filters: LoadedFilters,
    stats_pending: tokio::sync::Mutex<Option<crate::state::StatsBatch>>,
    media_pending: tokio::sync::Mutex<Vec<(i64, i32, i64)>>,
    pub(super) log_flush: tokio::sync::Mutex<()>,
    sample_flush: tokio::sync::Mutex<()>,
    filter_versions: [AtomicU64; 256],
    filter_loads: [tokio::sync::Mutex<()>; 256],

    custom: RwLock<HashMap<(i64, u64), CachedCustom>>,

    intents: RwLock<HashMap<u64, (Instant, f32)>>,

    intent_tasks: OnceLock<Arc<tokio::sync::Semaphore>>,
    trade_history: std::sync::Mutex<trade::context::History>,

    samples: RwLock<Vec<Box<[f32]>>>,
    sample_at: AtomicUsize,
    samples_dirty: std::sync::atomic::AtomicBool,
}

struct OwnedTasks {
    accepting: bool,
    tasks: tokio::task::JoinSet<()>,
    retained_epochs: Vec<tokio::task::JoinSet<()>>,
    failures: usize,
}

struct SampleFlushGuard<'a> {
    dirty: &'a AtomicBool,
    complete: bool,
}

impl Drop for SampleFlushGuard<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.dirty.store(true, Ordering::Release);
        }
    }
}

impl Default for OwnedTasks {
    fn default() -> Self {
        Self {
            accepting: true,
            tasks: tokio::task::JoinSet::new(),
            retained_epochs: Vec::new(),
            failures: 0,
        }
    }
}

impl OwnedTasks {
    fn spawn(&mut self, future: impl std::future::Future<Output = ()> + Send + 'static) -> bool {
        if !self.accepting {
            return false;
        }
        while let Some(done) = self.tasks.try_join_next() {
            if let Err(error) = done {
                ::log::error!("owned handler task failed: {error}");
                self.failures += 1;
            }
        }
        self.tasks.spawn(future);
        true
    }

    fn begin_drain(&mut self) -> OwnedTaskDrain {
        self.accepting = false;
        let mut tasks = std::mem::take(&mut self.retained_epochs);
        tasks.push(std::mem::take(&mut self.tasks));
        OwnedTaskDrain {
            tasks,
            failures: std::mem::take(&mut self.failures),
        }
    }

    fn take_epoch(&mut self) -> OwnedTaskDrain {
        OwnedTaskDrain {
            tasks: vec![std::mem::take(&mut self.tasks)],
            failures: std::mem::take(&mut self.failures),
        }
    }

    fn retain_epoch(&mut self, mut epoch: OwnedTaskDrain) {
        self.retained_epochs.append(&mut epoch.tasks);
        self.failures += epoch.failures;
    }
}

pub struct OwnedTaskDrain {
    tasks: Vec<tokio::task::JoinSet<()>>,
    failures: usize,
}

impl OwnedTaskDrain {
    fn is_empty(&self) -> bool {
        self.tasks.iter().all(tokio::task::JoinSet::is_empty)
    }

    pub async fn join(&mut self) -> usize {
        for tasks in &mut self.tasks {
            while let Some(result) = tasks.join_next().await {
                if let Err(error) = result {
                    ::log::error!("owned handler task failed while draining: {error}");
                    self.failures += 1;
                }
            }
        }
        self.failures
    }

    pub async fn abort(&mut self) -> usize {
        for tasks in &mut self.tasks {
            tasks.abort_all();
        }
        self.join().await;
        self.failures
    }
}

pub struct OwnedTaskCheckpoint {
    epoch: OwnedTaskDrain,
    failures: usize,
}

impl OwnedTaskCheckpoint {
    pub fn retain_for_shutdown(self, ctx: &Ctx) {
        self.retain_epoch(&ctx.owned_tasks);
    }

    fn retain_epoch(self, owned: &std::sync::Mutex<OwnedTasks>) {
        let Self {
            mut epoch,
            failures,
        } = self;
        epoch.failures += failures;
        owned.lock().unwrap().retain_epoch(epoch);
    }

    pub async fn join(&mut self, ctx: &Ctx) -> usize {
        self.join_epochs(&ctx.owned_tasks).await
    }

    async fn join_epochs(&mut self, owned: &std::sync::Mutex<OwnedTasks>) -> usize {
        loop {
            self.failures += self.epoch.join().await;
            let next = owned.lock().unwrap().take_epoch();
            let empty = next.is_empty();
            self.epoch = next;
            if empty {
                self.failures += self.epoch.join().await;
                return self.failures;
            }
        }
    }

    pub async fn abort(&mut self, ctx: &Ctx) -> usize {
        self.abort_epochs(&ctx.owned_tasks).await
    }

    async fn abort_epochs(&mut self, owned: &std::sync::Mutex<OwnedTasks>) -> usize {
        loop {
            self.failures += self.epoch.abort().await;
            let next = owned.lock().unwrap().take_epoch();
            let empty = next.is_empty();
            self.epoch = next;
            if empty {
                self.failures += self.epoch.join().await;
                return self.failures;
            }
        }
    }
}

pub struct RuntimeConfig {
    sudo_id: Option<i64>,
    api_hash: String,
    start_links: config::ConfiguredLinks,
    miniapp_link: Option<String>,
    voice_admission: usize,
    nsfw_slots: usize,
    intent_tasks: usize,
    voice: voicemonitor::VoiceConfig,
}

impl RuntimeConfig {
    pub fn from_environment(
        sudo_id: Option<i64>,
        api_hash: String,
        miniapp_link: Option<String>,
    ) -> Result<Self, String> {
        Ok(Self {
            sudo_id,
            api_hash,
            start_links: config::ConfiguredLinks::from_environment()?,
            miniapp_link,
            voice_admission: configured_usize("VOICE_ADMISSION", 32, 1, 1_024)?,
            nsfw_slots: configured_usize("NSFW_SLOTS", DEFAULT_NSFW_SLOTS, 1, 256)?,
            intent_tasks: configured_usize("INTENT_TASKS", INTENT_TASKS, 1, 1_024)?,
            voice: voicemonitor::VoiceConfig::from_environment()?,
        })
    }

    pub async fn validate(&self) -> Result<(), String> {
        self.voice.validate().await
    }

    #[cfg(test)]
    fn for_test() -> Self {
        Self {
            sudo_id: None,
            api_hash: String::new(),
            start_links: config::ConfiguredLinks::default(),
            miniapp_link: None,
            voice_admission: 32,
            nsfw_slots: DEFAULT_NSFW_SLOTS,
            intent_tasks: INTENT_TASKS,
            voice: voicemonitor::VoiceConfig::for_test(),
        }
    }
}

fn configured_usize(
    name: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, String> {
    let value = match std::env::var(name) {
        Ok(value) => value.parse::<usize>().map_err(|_| {
            format!("{name} must be an integer in {minimum}..={maximum}, got {value:?}")
        })?,
        Err(std::env::VarError::NotPresent) => default,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{name} is not valid Unicode"));
        }
    };
    if !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "{name} must be in {minimum}..={maximum}, got {value}"
        ));
    }
    Ok(value)
}

pub const CONCEPT_SLOTS: usize = 8;

fn cache_source_matches(animated: bool, from_animation: bool) -> bool {
    !animated || from_animation
}

pub const NOTICE_EVERY: Duration = Duration::from_secs(120);

mod kind {
    pub const FILTER_NOTICE: u8 = 0;
    pub const REPORT: u8 = 1;
    pub const LOCK_NOTICE: u8 = 2;
    pub const GATE_NOTICE: u8 = 3;
    pub const SIGHTING: u8 = 4;
    pub const MODERATION: u8 = 5;
    pub const FLOOD_NOTICE: u8 = 6;
    pub const BOT_REMOVAL: u8 = 7;
    pub const INSTALL_NOTICE: u8 = 9;
    pub const CLEANER_INSTALL: u8 = 10;
    pub const COMMENT_SIGN: u8 = 11;
    pub const MINIAPP_FILTER_CREATE: u8 = 12;
    pub const MINIAPP_LIST_READ: u8 = 13;
    pub const MINIAPP_LIST_REMOVE: u8 = 14;
    pub const MINIAPP_LIST_CLEAR: u8 = 15;

    #[cfg(test)]
    pub const ALL: &[(&str, u8)] = &[
        ("FILTER_NOTICE", FILTER_NOTICE),
        ("REPORT", REPORT),
        ("LOCK_NOTICE", LOCK_NOTICE),
        ("GATE_NOTICE", GATE_NOTICE),
        ("SIGHTING", SIGHTING),
        ("MODERATION", MODERATION),
        ("FLOOD_NOTICE", FLOOD_NOTICE),
        ("BOT_REMOVAL", BOT_REMOVAL),
        ("INSTALL_NOTICE", INSTALL_NOTICE),
        ("CLEANER_INSTALL", CLEANER_INSTALL),
        ("COMMENT_SIGN", COMMENT_SIGN),
        ("MINIAPP_FILTER_CREATE", MINIAPP_FILTER_CREATE),
        ("MINIAPP_LIST_READ", MINIAPP_LIST_READ),
        ("MINIAPP_LIST_REMOVE", MINIAPP_LIST_REMOVE),
        ("MINIAPP_LIST_CLEAR", MINIAPP_LIST_CLEAR),
    ];
}

const DELETED_TTL: Duration = Duration::from_secs(3600);
const DELETED_MAX: usize = 5_000;
const DEFERRED_DELETE_MAX: usize = 100_000;
const PENDING_WRITE_MAX: usize = 200_000;
const PENDING_ADMIN_MAX: usize = 10_000;

pub(super) enum DeferredAction {
    Delete { chat: i64, message: i32 },
}

struct DeferredEntry {
    due: Instant,
    sequence: u64,
    action: DeferredAction,
}

impl PartialEq for DeferredEntry {
    fn eq(&self, other: &Self) -> bool {
        self.due == other.due && self.sequence == other.sequence
    }
}

impl Eq for DeferredEntry {}

impl Ord for DeferredEntry {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        other
            .due
            .cmp(&self.due)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl PartialOrd for DeferredEntry {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

struct PendingNumber {
    armed: Instant,
    target_chat: i64,
    setting: &'static str,
}

#[derive(Default)]
struct PendingNumbers {
    entries: HashMap<(i64, i64), PendingNumber>,
}

impl PendingNumbers {
    fn arm(&mut self, input_chat: i64, user: i64, target_chat: i64, setting: &'static str) {
        self.entries
            .retain(|_, pending| pending.armed.elapsed() < PENDING_NUMBER_TTL);
        make_room(&mut self.entries, PENDING_NUMBERS_MAX, |pending| {
            pending.armed
        });
        self.entries.insert(
            (input_chat, user),
            PendingNumber {
                armed: Instant::now(),
                target_chat,
                setting,
            },
        );
    }

    fn expected(&self, input_chat: i64, user: i64) -> Option<(i64, &'static str)> {
        let pending = self.entries.get(&(input_chat, user))?;
        (pending.armed.elapsed() < PENDING_NUMBER_TTL)
            .then_some((pending.target_chat, pending.setting))
    }

    fn take(
        &mut self,
        input_chat: i64,
        user: i64,
        target_chat: i64,
        setting: &'static str,
    ) -> bool {
        let Some(pending) = self.entries.get(&(input_chat, user)) else {
            return false;
        };
        if pending.target_chat != target_chat || pending.setting != setting {
            return false;
        }
        let pending = self
            .entries
            .remove(&(input_chat, user))
            .expect("pending number was checked above");
        pending.armed.elapsed() < PENDING_NUMBER_TTL
    }
}

type Tallies = HashMap<(i64, &'static str), u64>;

type Counts = HashMap<(i64, i64), (u64, String)>;

const MEMBER_TRUST: Duration = Duration::from_secs(60);

const PENDING_PASSWORD_TTL: Duration = Duration::from_secs(300);

const PENDING_NUMBER_TTL: Duration = Duration::from_secs(120);

const PENDING_NUMBERS_MAX: usize = 20_000;

const ARMED_WINDOW: u64 = 130_000;

const ADMIN_CACHE_TTL: Duration = Duration::from_secs(1800);
const ADMIN_CACHE_MAX: usize = 20_000;

pub const PER_CHAT_UPDATES: usize = 8;

const ADDS_TTL: Duration = Duration::from_secs(300);

const PER_CHAT_MAX: usize = 20_000;

const RESTRICTION_WRITE_STRIPES: usize = 4_096;
const GROUP_RIGHTS_STRIPES: usize = 2_048;

fn restriction_write_stripe(chat: i64, user: i64) -> usize {
    let mixed = (chat as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (user as u64)
            .rotate_left(29)
            .wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed as usize % RESTRICTION_WRITE_STRIPES
}

fn group_rights_stripe(chat: i64) -> usize {
    let mixed = (chat as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    mixed as usize % GROUP_RIGHTS_STRIPES
}

const FLUSH_CHAT_BATCH: usize = 512;
const STATS_ROWS_PER_FLUSH: usize = 50_000;
const LOG_ENTRIES_PER_FLUSH: usize = 50_000;
const MEDIA_IDS_PER_FLUSH: usize = 50_000;

fn make_room<K, V, F>(map: &mut HashMap<K, V>, limit: usize, at: F)
where
    K: Clone + Eq + Hash,
    F: Fn(&V) -> Instant,
{
    if limit == 0 {
        map.clear();
        return;
    }
    while map.len() >= limit {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, value)| at(value))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
}

const EVENTS_PER_SUBJECT_MAX: usize = raid::LIMIT_RANGE.1 as usize + 1;
const FLOOD_EVENTS_MAX: usize = flood::LIMIT_RANGE.1 as usize + 1;
const REMOVAL_EVENTS_MAX: usize = betrayal::LIMIT_RANGE.1 as usize + 1;

#[cfg(test)]
fn record_event(times: &mut VecDeque<Instant>, window: Duration) -> usize {
    record_event_bounded(times, window, EVENTS_PER_SUBJECT_MAX)
}

fn record_event_bounded(times: &mut VecDeque<Instant>, window: Duration, capacity: usize) -> usize {
    record_event_at(times, window, capacity, Instant::now())
}

fn record_event_at(
    times: &mut VecDeque<Instant>,
    window: Duration,
    capacity: usize,
    now: Instant,
) -> usize {
    while times
        .front()
        .is_some_and(|time| now.duration_since(*time) >= window)
    {
        times.pop_front();
    }
    while times.len() >= capacity {
        times.pop_front();
    }
    times.push_back(now);
    times.len()
}

const SAID_MAX: usize = 5_000;

const BIO_TTL: Duration = Duration::from_secs(600);

const BIO_MAX: usize = 10_000;

const BIO_FETCHES: usize = 4;

const COMMENT_LOOKUPS: usize = 4;

const VERDICT_TTL: Duration = Duration::from_secs(86_400);

const VERDICT_MAX: usize = 50_000;
const VOICE_CACHE_TTL: Duration = Duration::from_secs(3_600);
const VOICE_CACHE_MAX: usize = 4_096;
const VOICE_TEXT_TTL: Duration = Duration::from_secs(900);
const VOICE_TEXT_CACHE_MAX: usize = 4_096;

const FILTERS_TTL: Duration = Duration::from_secs(1_800);

const SAMPLE_CAP: usize = 1_024;

const DEFAULT_NSFW_SLOTS: usize = 2;

const NSFW_FETCHES: usize = 4;

const NSFW_TASKS: usize = 32;

const INTENT_TASKS: usize = 16;

pub const FLEET_CONCURRENCY: usize = 8;

pub const FLEET_CAMPAIGNS: usize = 4;

pub fn recent_minutes(now: u32) -> [String; 3] {
    let now = now % 1_440;
    [now, (now + 1_439) % 1_440, (now + 1_438) % 1_440].map(|minute| minute.to_string())
}

pub async fn bounded<T, F>(items: Vec<T>, cap: usize, run: impl Fn(T) -> F + Send + 'static)
where
    T: Send + 'static,
    F: std::future::Future<Output = ()> + Send + 'static,
{
    assert!(cap > 0, "fleet concurrency must be nonzero");
    let mut items = items.into_iter();
    let mut active: Vec<std::pin::Pin<Box<F>>> = Vec::with_capacity(cap);
    loop {
        while active.len() < cap {
            let Some(item) = items.next() else { break };
            active.push(Box::pin(run(item)));
        }
        if active.is_empty() {
            break;
        }
        std::future::poll_fn(|context| {
            for index in 0..active.len() {
                if active[index].as_mut().poll(context).is_ready() {
                    drop(active.swap_remove(index));
                    return std::task::Poll::Ready(());
                }
            }
            std::task::Poll::Pending
        })
        .await;
    }
}

impl Ctx {
    pub fn new_with_allowed_chats(
        client: Client,
        settings: Arc<Settings>,
        bot_session: Arc<grammers_session::storages::ErasedSession>,
        bot_identity: BotIdentity,
        runtime_config: RuntimeConfig,
        max_runtime_chats: usize,
        allowed_chats: Option<Arc<HashSet<i64>>>,
    ) -> Self {
        Self {
            client,
            settings,
            bot_session,
            max_runtime_chats,
            allowed_chats,
            chats: RwLock::new(HashMap::new()),
            owned_tasks: std::sync::Mutex::new(OwnedTasks::default()),
            background_epoch: tokio::sync::RwLock::new(()),
            background_panicked: AtomicBool::new(false),
            dirty: Arc::default(),
            deleted: RwLock::new(HashMap::new()),
            next_deleted_key: AtomicU64::new(1),
            started: Instant::now(),
            pending_admins: RwLock::new(HashMap::new()),
            user: RwLock::new(None),
            cleaner_id: AtomicI64::new(0),
            bot_identity,
            runtime_config,
            user_chats: RwLock::new(HashMap::new()),
            pending_password: PasswordMailbox::default(),
            cleaner_login: tokio::sync::Mutex::new(()),
            restriction_writes: (0..RESTRICTION_WRITE_STRIPES)
                .map(|_| tokio::sync::Mutex::new(()))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            group_rights: (0..GROUP_RIGHTS_STRIPES)
                .map(|_| tokio::sync::Mutex::new(()))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            join_refs: RwLock::new(HashMap::new()),
            pending_writes: std::sync::Mutex::new(Vec::new()),
            pending_drops: std::sync::Mutex::new(Vec::new()),
            persistence_barrier: PersistenceBarrier::default(),
            deferred_deletes: std::sync::Mutex::new(BinaryHeap::new()),
            next_deferred: AtomicU64::new(0),
            last_armed: AtomicU64::new(0),
            pending_numbers: std::sync::Mutex::new(PendingNumbers::default()),
            bios: RwLock::new(HashMap::new()),
            bio_fetch: OnceLock::new(),
            bot_sweeps: OnceLock::new(),
            comment_lookups: OnceLock::new(),
            cleaner_joins: OnceLock::new(),
            tag_runs: OnceLock::new(),
            verdicts: RwLock::new(HashMap::new()),
            voice_verdicts: RwLock::new(HashMap::new()),
            filtered_voices: RwLock::new(HashMap::new()),
            voice_pool: OnceLock::new(),
            voice_jobs: OnceLock::new(),
            nsfw_slots: OnceLock::new(),
            nsfw_tasks: OnceLock::new(),
            nsfw_fetches: OnceLock::new(),
            margins: RwLock::new(HashMap::new()),
            adverts: RwLock::new(HashMap::new()),
            image_filters: RwLock::new(HashMap::new()),
            stats_pending: tokio::sync::Mutex::new(None),
            media_pending: tokio::sync::Mutex::new(Vec::new()),
            log_flush: tokio::sync::Mutex::new(()),
            sample_flush: tokio::sync::Mutex::new(()),
            filter_versions: std::array::from_fn(|_| AtomicU64::new(0)),
            filter_loads: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
            custom: RwLock::new(HashMap::new()),
            intents: RwLock::new(HashMap::new()),
            intent_tasks: OnceLock::new(),
            trade_history: std::sync::Mutex::new(trade::context::History::default()),
            samples: RwLock::new(Vec::new()),
            sample_at: AtomicUsize::new(0),
            samples_dirty: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn owns_chat(&self, chat: i64) -> bool {
        self.allowed_chats
            .as_ref()
            .is_none_or(|allowed| allowed.contains(&chat))
    }

    pub fn sudo_id(&self) -> Option<i64> {
        self.runtime_config.sudo_id
    }

    pub fn api_hash(&self) -> &str {
        &self.runtime_config.api_hash
    }

    fn start_links(&self) -> &[config::ConfiguredLink] {
        self.runtime_config.start_links.as_slice()
    }

    pub fn miniapp_link(&self) -> Option<&str> {
        self.runtime_config.miniapp_link.as_deref()
    }

    pub(super) async fn restriction_write(
        &self,
        chat: i64,
        user: i64,
    ) -> tokio::sync::MutexGuard<'_, ()> {
        self.restriction_writes[restriction_write_stripe(chat, user)]
            .lock()
            .await
    }

    pub(super) async fn group_rights(&self, chat: i64) -> GroupRightsGuard<'_> {
        GroupRightsGuard {
            chat,
            _guard: self.group_rights[group_rights_stripe(chat)].lock().await,
        }
    }

    pub(super) fn try_group_rights(&self, chat: i64) -> Option<GroupRightsGuard<'_>> {
        Some(GroupRightsGuard {
            chat,
            _guard: self.group_rights[group_rights_stripe(chat)]
                .try_lock()
                .ok()?,
        })
    }

    pub fn spawn_owned(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        let mut owned = self.owned_tasks.lock().unwrap();
        if !owned.spawn(future) {
            ::log::error!("owned handler work was submitted after shutdown closed task admission");
        }
    }

    pub fn begin_owned_checkpoint(&self) -> OwnedTaskCheckpoint {
        OwnedTaskCheckpoint {
            epoch: self.owned_tasks.lock().unwrap().take_epoch(),
            failures: 0,
        }
    }

    pub async fn background_epoch(&self) -> BackgroundEpochGuard<'_> {
        BackgroundEpochGuard {
            _epoch: self.background_epoch.read().await,
            panicked: &self.background_panicked,
        }
    }

    pub async fn checkpoint_epoch(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.background_epoch.write().await
    }

    pub fn background_task_panicked(&self) -> bool {
        self.background_panicked.load(Ordering::Acquire)
    }

    pub fn begin_owned_drain(&self) -> OwnedTaskDrain {
        let mut owned = self.owned_tasks.lock().unwrap();
        owned.begin_drain()
    }

    pub async fn shutdown_voice(&self) {
        if let Some(voice_pool) = self.voice_pool.get() {
            voice_pool.shutdown().await;
        }
    }

    #[cfg(test)]
    pub fn state(&self, chat: i64) -> Arc<ChatState> {
        if let Some(state) = self.chats.read().unwrap().get(&chat) {
            return Arc::clone(state);
        }
        let dirty = Arc::clone(&self.dirty);
        Arc::clone(self.chats.write().unwrap().entry(chat).or_insert_with(|| {
            Arc::new(ChatState {
                chat,
                dirty,
                ..Default::default()
            })
        }))
    }

    pub fn try_state(&self, chat: i64) -> Option<Arc<ChatState>> {
        if !self.owns_chat(chat) {
            return None;
        }
        if let Some(state) = self.chats.read().unwrap().get(&chat) {
            return Some(Arc::clone(state));
        }
        let dirty = Arc::clone(&self.dirty);
        let mut chats = self.chats.write().unwrap();
        if let Some(state) = chats.get(&chat) {
            return Some(Arc::clone(state));
        }
        if chats.len() >= self.max_runtime_chats {
            return None;
        }
        Some(Arc::clone(chats.entry(chat).or_insert_with(|| {
            Arc::new(ChatState {
                chat,
                dirty,
                ..Default::default()
            })
        })))
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

    pub fn capacity_snapshot(&self) -> CapacitySnapshot {
        let (outbound_active, outbound_waiting) = self.client.outbound_snapshot();
        CapacitySnapshot {
            runtime_chats: self.chats.read().unwrap().len(),
            settings_chats: self.settings.chat_count(),
            settings_rows: self.settings.setting_count(),
            settings_bytes: self.settings.setting_bytes(),
            user_chats: self.user_chats.read().unwrap().len(),
            dirty_logs: self.dirty.logs.0.lock().unwrap().len(),
            dirty_media: self.dirty.media.0.lock().unwrap().len(),
            dirty_stats: self.dirty.stats.0.lock().unwrap().len(),
            pending_writes: self.pending_writes.lock().unwrap().len(),
            pending_drops: self.pending_drops.lock().unwrap().len(),
            deferred: self.deferred_deletes.lock().unwrap().len(),
            pending_admins: self.pending_admins.read().unwrap().len(),
            deleted: self.deleted.read().unwrap().len(),
            verdicts: self.verdicts.read().unwrap().len(),
            voice_verdicts: self.voice_verdicts.read().unwrap().len(),
            filtered_voices: self.filtered_voices.read().unwrap().len(),
            outbound_active,
            outbound_waiting,
            outbound_critical_waiting: self.client.outbound_critical_waiting(),
        }
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
        let Some(state) = self.try_state(chat) else {
            return;
        };
        let mut members = state.members.lock().unwrap();
        if members.len() >= PER_CHAT_MAX {
            members.retain(|_, seen| seen.elapsed() < MEMBER_TRUST);
            make_room(&mut members, PER_CHAT_MAX, |seen| *seen);
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
            make_room(&mut bios, BIO_MAX, |(at, _)| *at);
        }
        bios.insert(user, (Instant::now(), false));
        None
    }

    pub fn remember_bio(&self, user: i64, has_link: bool) {
        let mut bios = self.bios.write().unwrap();
        if !bios.contains_key(&user) && bios.len() >= BIO_MAX {
            bios.retain(|_, (at, _)| at.elapsed() < BIO_TTL);
            make_room(&mut bios, BIO_MAX, |(at, _)| *at);
        }
        bios.insert(user, (Instant::now(), has_link));
    }

    pub fn forget_bio(&self, user: i64) {
        self.bios.write().unwrap().remove(&user);
    }

    pub async fn bio_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(
            self.bio_fetch
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(BIO_FETCHES))),
        )
        .acquire_owned()
        .await
        .expect("the bio semaphore is never closed")
    }

    pub async fn comment_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(
            self.comment_lookups
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(COMMENT_LOOKUPS))),
        )
        .acquire_owned()
        .await
        .expect("the comment semaphore is never closed")
    }

    pub fn known_verdict(&self, file: i64, animated: bool) -> Option<nsfw::Judgement> {
        let verdicts = self.verdicts.read().unwrap();
        verdicts
            .get(&file)
            .filter(|(at, _, from_animation)| {
                at.elapsed() < VERDICT_TTL && cache_source_matches(animated, *from_animation)
            })
            .map(|(_, judged, _)| *judged)
    }

    pub fn remember_verdict(&self, file: i64, judged: nsfw::Judgement, from_animation: bool) {
        let mut verdicts = self.verdicts.write().unwrap();
        if !verdicts.contains_key(&file) && verdicts.len() >= VERDICT_MAX {
            verdicts.retain(|_, (at, ..)| at.elapsed() < VERDICT_TTL);
            make_room(&mut verdicts, VERDICT_MAX, |(at, ..)| *at);
        }
        verdicts.insert(file, (Instant::now(), judged, from_animation));
    }

    pub fn known_voice(&self, file: i64, profile: u64) -> Option<bool> {
        self.voice_verdicts
            .read()
            .unwrap()
            .get(&(file, profile))
            .filter(|(at, _, _)| at.elapsed() < VOICE_CACHE_TTL)
            .map(|(_, bad, _)| *bad)
    }

    pub fn known_voice_text(&self, file: i64, profile: u64) -> Option<String> {
        self.voice_verdicts
            .read()
            .unwrap()
            .get(&(file, profile))
            .filter(|(at, _, _)| at.elapsed() < VOICE_CACHE_TTL)
            .and_then(|(_, _, text)| text.as_ref().map(|text| text.to_string()))
    }

    pub fn remember_voice(&self, file: i64, profile: u64, bad: bool, text: Option<String>) {
        let mut verdicts = self.voice_verdicts.write().unwrap();
        if !verdicts.contains_key(&(file, profile)) && verdicts.len() >= VOICE_CACHE_MAX {
            verdicts.retain(|_, (at, _, _)| at.elapsed() < VOICE_CACHE_TTL);
            make_room(&mut verdicts, VOICE_CACHE_MAX, |(at, _, _)| *at);
        }
        verdicts.insert(
            (file, profile),
            (Instant::now(), bad, text.map(Arc::<str>::from)),
        );
    }

    pub fn remember_filtered_voice(&self, chat: i64, speaker: Option<i64>, text: String) -> u64 {
        let key = self.next_deleted_key.fetch_add(1, Ordering::Relaxed);
        let mut voices = self.filtered_voices.write().unwrap();
        if voices.len() >= VOICE_TEXT_CACHE_MAX {
            voices.retain(|_, (at, ..)| at.elapsed() < VOICE_TEXT_TTL);
            make_room(&mut voices, VOICE_TEXT_CACHE_MAX, |(at, ..)| *at);
        }
        voices.insert(key, (Instant::now(), chat, speaker, text));
        key
    }

    pub fn filtered_voice(&self, key: u64) -> Option<(i64, Option<i64>, String)> {
        let voices = self.filtered_voices.read().unwrap();
        voices
            .get(&key)
            .filter(|(at, ..)| at.elapsed() < VOICE_TEXT_TTL)
            .map(|(_, chat, speaker, text)| (*chat, *speaker, text.clone()))
    }

    pub fn voice_pool(&self) -> Arc<voicemonitor::VoicePool> {
        Arc::clone(self.voice_pool.get_or_init(|| {
            Arc::new(voicemonitor::VoicePool::new(
                self.runtime_config.voice.clone(),
            ))
        }))
    }

    pub async fn voice_job_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(self.voice_jobs.get_or_init(|| {
            Arc::new(tokio::sync::Semaphore::new(
                self.runtime_config.voice_admission,
            ))
        }))
        .acquire_owned()
        .await
        .expect("the voice admission semaphore is never closed")
    }

    pub async fn nsfw_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(
            self.nsfw_slots.get_or_init(|| {
                Arc::new(tokio::sync::Semaphore::new(self.runtime_config.nsfw_slots))
            }),
        )
        .acquire_owned()
        .await
        .expect("the nsfw semaphore is never closed")
    }

    pub async fn nsfw_task_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(
            self.nsfw_tasks
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(NSFW_TASKS))),
        )
        .acquire_owned()
        .await
        .expect("the nsfw task semaphore is never closed")
    }

    pub fn known_margins(&self, file: i64, animated: bool) -> Option<[f32; CONCEPT_SLOTS]> {
        let margins = self.margins.read().unwrap();
        margins
            .get(&file)
            .filter(|(at, _, from_animation)| {
                at.elapsed() < VERDICT_TTL && cache_source_matches(animated, *from_animation)
            })
            .map(|(_, all, _)| *all)
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
        if !adverts.contains_key(&file) && adverts.len() >= VERDICT_MAX {
            adverts.retain(|_, (at, _)| at.elapsed() < VERDICT_TTL);
            make_room(&mut adverts, VERDICT_MAX, |(at, _)| *at);
        }
        adverts.insert(file, (Instant::now(), why));
    }

    pub async fn image_filters(
        &self,
        chat: i64,
    ) -> Result<Arc<Vec<imgfilter::Filter>>, sqlx::Error> {
        let slot = (chat as u64 % 256) as usize;
        let _loading = self.filter_loads[slot].lock().await;
        loop {
            let generation = self.filter_versions[slot].load(Ordering::Acquire);
            {
                let cache = self.image_filters.read().unwrap();
                if let Some((at, filters)) = cache.get(&chat)
                    && at.elapsed() < FILTERS_TTL
                {
                    return Ok(Arc::clone(filters));
                }
            }
            let filters: Vec<imgfilter::Filter> = self
                .settings
                .image_filters(chat)
                .await?
                .into_iter()
                .filter(|row| row.vector.len() == imgfilter::DIM)
                .map(|row| imgfilter::Filter {
                    print: imgfilter::fingerprint(&row.vector, row.scale),
                    vector: vision::unit(&imgfilter::dequantize(&row.vector, row.scale)),
                    name: row.name,
                    cut: if row.samples == 0 {
                        imgfilter::FIXED_MODEL_CUT
                    } else {
                        imgfilter::FIXED_EXAMPLE_CUT
                    },
                    live: true,
                })
                .collect();
            let filters = Arc::new(filters);
            {
                let mut cache = self.image_filters.write().unwrap();
                if self.filter_versions[slot].load(Ordering::Acquire) != generation {
                    continue;
                }
                if cache.len() >= PER_CHAT_MAX {
                    cache.retain(|_, (at, _)| at.elapsed() < FILTERS_TTL);
                    make_room(&mut cache, PER_CHAT_MAX, |(at, _)| *at);
                }
                cache.insert(chat, (Instant::now(), Arc::clone(&filters)));
            }
            return Ok(filters);
        }
    }

    pub fn forget_image_filters(&self, chat: i64) {
        let mut cache = self.image_filters.write().unwrap();
        self.filter_versions[(chat as u64 % 256) as usize].fetch_add(1, Ordering::Release);
        cache.remove(&chat);
    }

    pub fn known_intent(&self, key: u64) -> Option<f32> {
        let intents = self.intents.read().unwrap();
        intents
            .get(&key)
            .filter(|(at, _)| at.elapsed() < VERDICT_TTL)
            .map(|(_, margin)| *margin)
    }

    pub fn remember_intent(&self, key: u64, margin: f32) {
        let mut intents = self.intents.write().unwrap();
        if !intents.contains_key(&key) && intents.len() >= VERDICT_MAX {
            intents.retain(|_, (at, _)| at.elapsed() < VERDICT_TTL);
            make_room(&mut intents, VERDICT_MAX, |(at, _)| *at);
        }
        intents.insert(key, (Instant::now(), margin));
    }

    pub async fn intent_task_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(self.intent_tasks.get_or_init(|| {
            Arc::new(tokio::sync::Semaphore::new(
                self.runtime_config.intent_tasks,
            ))
        }))
        .acquire_owned()
        .await
        .expect("the intent task semaphore is never closed")
    }

    pub fn known_custom(&self, file: i64, print: u64, animated: bool) -> Option<f32> {
        let custom = self.custom.read().unwrap();
        custom
            .get(&(file, print))
            .filter(|(at, _, from_animation)| {
                at.elapsed() < VERDICT_TTL && cache_source_matches(animated, *from_animation)
            })
            .map(|(_, margin, _)| *margin)
    }

    pub fn remember_custom(&self, file: i64, print: u64, margin: f32, from_animation: bool) {
        let mut custom = self.custom.write().unwrap();
        if !custom.contains_key(&(file, print)) && custom.len() >= VERDICT_MAX {
            custom.retain(|_, (at, ..)| at.elapsed() < VERDICT_TTL);
            make_room(&mut custom, VERDICT_MAX, |(at, ..)| *at);
        }
        custom.insert((file, print), (Instant::now(), margin, from_animation));
    }

    pub fn remember_sample(&self, embedding: &[f32]) {
        if embedding.len() != imgfilter::DIM {
            return;
        }
        let at = self.sample_at.fetch_add(1, Ordering::Relaxed) % SAMPLE_CAP;
        let mut samples = self.samples.write().unwrap();
        match samples.len() < SAMPLE_CAP {
            true => samples.push(embedding.to_vec().into_boxed_slice()),
            false => samples[at] = embedding.to_vec().into_boxed_slice(),
        }
        self.samples_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub async fn flush_samples(&self) -> bool {
        let _flush = self.sample_flush.lock().await;
        if !self
            .samples_dirty
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            return true;
        }
        let mut flush = SampleFlushGuard {
            dirty: &self.samples_dirty,
            complete: false,
        };
        let samples = self.samples();
        if samples.is_empty() {
            flush.complete = true;
            return true;
        }
        let flat: Vec<f32> = samples.iter().flat_map(|s| s.iter().copied()).collect();
        let (bytes, scale) = imgfilter::quantize(&flat);
        let count = match i32::try_from(samples.len()) {
            Ok(count) => count,
            Err(error) => {
                ::log::error!("calibration: sample count exceeds PostgreSQL integer: {error}");
                self.samples_dirty.store(true, Ordering::Relaxed);
                return false;
            }
        };
        if let Err(error) = self.settings.save_samples(&bytes, scale, count).await {
            ::log::warn!(
                "calibration: could not save the reservoir; retaining it for retry: {error}"
            );
            self.samples_dirty.store(true, Ordering::Relaxed);
            return false;
        }
        flush.complete = true;
        true
    }

    pub async fn load_samples(&self) -> Result<(), sqlx::Error> {
        let Some((bytes, scale, count)) = self.settings.load_samples().await? else {
            return Ok(());
        };
        let count = match usize::try_from(count) {
            Ok(count) => count.min(SAMPLE_CAP),
            Err(error) => {
                ::log::error!("calibration: stored sample count is invalid: {error}");
                return Ok(());
            }
        };
        if count == 0 || bytes.len() != count * imgfilter::DIM {
            if !bytes.is_empty() {
                eprintln!(
                    "calibration: stored reservoir is {} bytes for {count} samples, ignoring it",
                    bytes.len()
                );
            }
            return Ok(());
        }
        let flat = imgfilter::dequantize(&bytes, scale);
        let mut samples = self.samples.write().unwrap();
        *samples = flat
            .as_chunks::<{ imgfilter::DIM }>()
            .0
            .iter()
            .map(|chunk| chunk.to_vec().into_boxed_slice())
            .collect();
        self.sample_at
            .store(samples.len() % SAMPLE_CAP, Ordering::Relaxed);
        println!("calibration: {} samples restored", samples.len());
        Ok(())
    }

    pub fn samples(&self) -> Vec<Box<[f32]>> {
        self.samples.read().unwrap().clone()
    }

    pub fn remember_margins(&self, file: i64, all: [f32; CONCEPT_SLOTS], from_animation: bool) {
        let mut margins = self.margins.write().unwrap();
        if !margins.contains_key(&file) && margins.len() >= VERDICT_MAX {
            margins.retain(|_, (at, ..)| at.elapsed() < VERDICT_TTL);
            make_room(&mut margins, VERDICT_MAX, |(at, ..)| *at);
        }
        margins.insert(file, (Instant::now(), all, from_animation));
    }

    pub async fn nsfw_fetch(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.nsfw_fetches
            .get_or_init(|| tokio::sync::Semaphore::new(NSFW_FETCHES))
            .acquire()
            .await
            .expect("the nsfw fetch semaphore is never closed")
    }

    pub fn tag_slot(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(
            self.tag_runs
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(FLEET_CONCURRENCY))),
        )
        .try_acquire_owned()
        .ok()
    }

    pub async fn sweep_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(
            self.bot_sweeps
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(FLEET_CAMPAIGNS))),
        )
        .acquire_owned()
        .await
        .expect("the sweep semaphore is never closed")
    }

    pub async fn cleaner_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(
            self.cleaner_joins
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1))),
        )
        .acquire_owned()
        .await
        .expect("the cleaner semaphore is never closed")
    }

    pub async fn resolve_group(&self, chat: i64) -> Option<PeerRef> {
        if !self.owns_chat(chat) {
            return None;
        }
        let id = PeerId::from_bot_api_dialog_id(chat)?;
        autoconfig::resolve_peer(Some(self.bot_session.as_ref()), id, self.chat_ref(chat)).await
    }

    pub fn try_cleaner_slot(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(
            self.cleaner_joins
                .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1))),
        )
        .try_acquire_owned()
        .ok()
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
        let Some(state) = self.try_state(chat) else {
            return;
        };
        let mut adds = state.adds.lock().unwrap();
        if adds.len() >= PER_CHAT_MAX {
            adds.retain(|_, (at, _)| at.elapsed() < ADDS_TTL);
            make_room(&mut adds, PER_CHAT_MAX, |(at, _)| *at);
        }
        adds.insert(user, (Instant::now(), added));
    }

    pub fn queue_log(&self, chat: i64, entry: String) {
        const MAX_PER_CHAT: usize = 200;
        let Some(state) = self.try_state(chat) else {
            return;
        };
        let mut entries = state.logs.lock().unwrap();
        let was_empty = entries.is_empty();
        if entries.len() >= MAX_PER_CHAT {
            entries.remove(0);
            self.poison_cursor_barrier();
            ::log::error!(
                "log: queue for {chat} exceeded {MAX_PER_CHAT}; cursor barrier poisoned after dropping the oldest entry"
            );
        }
        entries.push(entry);
        if was_empty {
            Dirty::mark(&self.dirty.logs, chat);
        }
    }

    pub fn retry_logs(&self, chat: i64, mut retry: Vec<String>) {
        const MAX_PER_CHAT: usize = 200;
        let Some(state) = self.try_state(chat) else {
            return;
        };
        let mut entries = state.logs.lock().unwrap();
        retry.append(&mut entries);
        if retry.len() > MAX_PER_CHAT {
            let excess = retry.len() - MAX_PER_CHAT;
            retry.drain(..excess);
            self.poison_cursor_barrier();
            ::log::error!(
                "log: retry queue for {chat} exceeded {MAX_PER_CHAT}; dropped {excess} oldest entries"
            );
        }
        *entries = retry;
        Dirty::mark(&self.dirty.logs, chat);
    }

    pub fn remember_said(&self, chat: i64, user: i64, id: i32) {
        if let Some(state) = self.try_state(chat) {
            state.remember_said(user, id);
        }
    }

    pub fn take_said(&self, chat: i64, user: i64) -> Vec<i32> {
        self.peek(chat)
            .map(|state| state.take_said(user))
            .unwrap_or_default()
    }

    pub fn queue_temp_media(&self, chat: i64, id: i32, due: Instant, due_at: i64) {
        let Some(state) = self.try_state(chat) else {
            return;
        };
        {
            let mut queue = state.temp_media.lock().unwrap();
            let was_empty = queue.is_empty();
            let can_persist = tempmedia::queue(&mut queue, id, due)
                .is_none_or(|dropped| self.remember_pending_drop(chat, dropped));
            if was_empty {
                Dirty::mark(&self.dirty.media, chat);
            }
            if !can_persist {
                self.poison_cursor_barrier();
                return;
            }
        }
        self.remember_pending_write(chat, id, due_at);
    }

    fn remember_pending_write(&self, chat: i64, id: i32, due_at: i64) -> bool {
        let mut writes = self.pending_writes.lock().unwrap();
        if writes.len() < PENDING_WRITE_MAX {
            writes.push((chat, id, due_at));
            true
        } else {
            self.poison_cursor_barrier();
            false
        }
    }

    fn poison_cursor_barrier(&self) {
        self.persistence_barrier.poison();
    }

    pub(super) fn persistence_failed(&self, reason: &str) {
        ::log::error!("persistence barrier poisoned: {reason}");
        self.poison_cursor_barrier();
    }

    pub(super) fn admission_failed(&self, chat: i64, error: ChatAdmissionError) {
        match error {
            ChatAdmissionError::Persistence(error) => {
                self.persistence_failed(&format!("could not durably admit chat {chat}: {error}"))
            }
            ChatAdmissionError::RouteRejected
            | ChatAdmissionError::RuntimeCapacityReached
            | ChatAdmissionError::DurableCapacityReached(_) => {
                ::log::warn!("chat admission rejected for {chat}: {error}");
            }
        }
    }

    pub fn cursor_barrier_poisoned(&self) -> bool {
        self.persistence_barrier.is_poisoned()
    }

    pub async fn persistence_barrier_failed(&self) {
        self.persistence_barrier.wait().await;
    }

    pub fn restore_temp_media(&self, chat: i64, id: i32, due: Instant) {
        let Some(state) = self.try_state(chat) else {
            return;
        };
        let mut queue = state.temp_media.lock().unwrap();
        let was_empty = queue.is_empty();
        if let Some(dropped) = tempmedia::queue(&mut queue, id, due) {
            self.remember_pending_drop(chat, dropped);
        }
        if was_empty {
            Dirty::mark(&self.dirty.media, chat);
        }
    }

    fn remember_pending_drop(&self, chat: i64, id: i32) -> bool {
        let mut drops = self.pending_drops.lock().unwrap();
        if drops.len() < PENDING_WRITE_MAX {
            drops.push((chat, id));
            true
        } else {
            false
        }
    }

    pub fn retry_pending_drops(&self, rows: &[(i64, i32)]) {
        let mut drops = self.pending_drops.lock().unwrap();
        let room = PENDING_WRITE_MAX.saturating_sub(drops.len());
        drops.extend(rows.iter().copied().take(room));
    }

    pub fn take_pending_writes(&self) -> Vec<(i64, i32, i64)> {
        std::mem::take(&mut *self.pending_writes.lock().unwrap())
    }

    pub fn take_pending_drops(&self) -> Vec<(i64, i32)> {
        std::mem::take(&mut *self.pending_drops.lock().unwrap())
    }

    pub fn schedule_delete(&self, chat: i64, message: i32, due: Instant) {
        let mut queue = self.deferred_deletes.lock().unwrap();
        if queue.len() >= DEFERRED_DELETE_MAX {
            self.poison_cursor_barrier();
            return;
        }
        let sequence = self.next_deferred.fetch_add(1, Ordering::Relaxed);
        queue.push(DeferredEntry {
            due,
            sequence,
            action: DeferredAction::Delete { chat, message },
        });
        drop(queue);
        self.remember_pending_write(chat, message, tempmedia::due_at_unix(due));
    }

    pub(super) fn take_due_actions(&self, limit: usize) -> Vec<DeferredAction> {
        let now = Instant::now();
        let mut queue = self.deferred_deletes.lock().unwrap();
        let mut due = Vec::with_capacity(limit.min(queue.len()));
        while due.len() < limit && queue.peek().is_some_and(|entry| entry.due <= now) {
            due.push(queue.pop().expect("peeked deferred action").action);
        }
        due
    }

    pub fn take_due_media(&self) -> Vec<(i64, Vec<i32>)> {
        let now = Instant::now();
        let mut ready = Vec::new();
        let mut remaining = MEDIA_IDS_PER_FLUSH;
        let chats = Dirty::take(&self.dirty.media, FLUSH_CHAT_BATCH);
        for chat in chats {
            if remaining == 0 {
                Dirty::mark(&self.dirty.media, chat);
                continue;
            }
            let Some(state) = self.peek(chat) else {
                continue;
            };
            if self.chat_ref(chat).is_none() {
                Dirty::mark(&self.dirty.media, chat);
                continue;
            }
            let due = {
                let mut queue = state.temp_media.lock().unwrap();
                let due = tempmedia::drain_due_up_to(&mut queue, now, remaining);
                if !queue.is_empty() {
                    Dirty::mark(&self.dirty.media, chat);
                } else {
                    queue.shrink_to_fit();
                }
                due
            };
            if !due.is_empty() {
                remaining = remaining.saturating_sub(due.len());
                ready.push((chat, due));
            }
        }
        ready
    }

    pub fn take_logs(&self) -> Vec<(i64, Vec<String>)> {
        let mut remaining = LOG_ENTRIES_PER_FLUSH;
        let chats = Dirty::take(&self.dirty.logs, FLUSH_CHAT_BATCH);
        let mut ready = Vec::new();
        for chat in chats {
            if remaining == 0 {
                Dirty::mark(&self.dirty.logs, chat);
                continue;
            }
            let Some(state) = self.peek(chat) else {
                continue;
            };
            let mut logs = state.logs.lock().unwrap();
            if logs.is_empty() {
                continue;
            }
            let count = logs.len().min(remaining);
            let queued: Vec<String> = if count == logs.len() {
                std::mem::take(&mut *logs)
            } else {
                logs.drain(..count).collect()
            };
            remaining -= queued.len();
            if !logs.is_empty() {
                Dirty::mark(&self.dirty.logs, chat);
            }
            ready.push((chat, queued));
        }
        ready
    }

    pub fn has_pending_logs(&self) -> bool {
        !self.dirty.logs.0.lock().unwrap().is_empty()
    }

    fn joined_cached(&self, key: (i64, i32)) -> Option<Vec<Joined>> {
        self.peek(key.0)?
            .joined
            .lock()
            .unwrap()
            .get(&key.1)
            .cloned()
    }

    fn remember_joined(&self, key: (i64, i32), joined: Vec<Joined>) {
        const MAX: usize = 200;
        let Some(state) = self.try_state(key.0) else {
            return;
        };
        let mut cache = state.joined.lock().unwrap();
        if cache.len() >= MAX {
            let mut ids: Vec<i32> = cache.keys().copied().collect();
            ids.sort_unstable();
            let cutoff = ids[ids.len() / 2];
            cache.retain(|id, _| *id >= cutoff);
        }
        cache.insert(key.1, joined);
    }

    pub fn me_id(&self) -> i64 {
        self.bot_identity.id.get()
    }

    pub fn bot_username(&self) -> Option<&str> {
        self.bot_identity.username.as_deref()
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

    pub(super) fn try_cleaner_login(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.cleaner_login.try_lock().ok()
    }

    pub(super) fn expect_password(&self) -> PasswordWait {
        self.pending_password.arm()
    }

    pub(super) fn give_password(&self, password: String) -> bool {
        self.pending_password.give(password)
    }

    pub(super) async fn await_password(
        &self,
        request: PasswordWait,
        timeout: Duration,
    ) -> Option<String> {
        self.pending_password.wait(request, timeout).await
    }

    pub fn expect_number(
        &self,
        input_chat: i64,
        user: i64,
        target_chat: i64,
        setting: &'static str,
    ) {
        self.last_armed
            .store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
        self.pending_numbers
            .lock()
            .unwrap()
            .arm(input_chat, user, target_chat, setting);
    }

    pub fn expected_number(&self, input_chat: i64, user: i64) -> Option<(i64, &'static str)> {
        self.pending_numbers
            .lock()
            .unwrap()
            .expected(input_chat, user)
    }

    pub fn take_expected_number(
        &self,
        input_chat: i64,
        user: i64,
        target_chat: i64,
        setting: &'static str,
    ) -> bool {
        self.pending_numbers
            .lock()
            .unwrap()
            .take(input_chat, user, target_chat, setting)
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
        if pendings.len() >= PENDING_ADMIN_MAX {
            pendings.retain(|_, p| p.started.elapsed() < promote::PENDING_TTL);
            make_room(&mut pendings, PENDING_ADMIN_MAX, |pending| pending.started);
        }
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
        self.throttle(kind::FILTER_NOTICE, chat, user, NOTICE_EVERY)
    }

    pub fn may_notify_every(&self, chat: i64, user: i64, every: Duration) -> bool {
        every.is_zero() || self.throttle(kind::GATE_NOTICE, chat, user, every)
    }

    pub fn may_notify_lock(&self, chat: i64, user: i64) -> bool {
        self.throttle(kind::LOCK_NOTICE, chat, user, Duration::from_secs(20))
    }

    pub fn may_notify_flood(&self, chat: i64, user: i64) -> bool {
        self.throttle(kind::FLOOD_NOTICE, chat, user, NOTICE_EVERY)
    }

    pub fn try_autoconfig(&self, chat: i64) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        Arc::clone(&self.try_state(chat)?.configure_lock)
            .try_lock_owned()
            .ok()
    }

    pub fn claim_setup_probe(&self, state: &ChatState, explicit: bool) -> bool {
        let now = self.uptime_millis();
        state
            .setup_next_check
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                (explicit || now >= next).then_some(now + 60_000)
            })
            .is_ok()
    }

    pub fn claim_install_notice(&self, chat: i64, missing: i64) -> bool {
        self.throttle(kind::INSTALL_NOTICE, chat, missing, Duration::from_secs(2))
    }

    pub fn claim_cleaner_install(&self, chat: i64) -> bool {
        self.throttle(kind::CLEANER_INSTALL, chat, 0, Duration::from_secs(300))
    }

    pub fn claim_bot_removal(&self, chat: i64, user: i64) -> bool {
        self.throttle(kind::BOT_REMOVAL, chat, user, Duration::from_secs(60))
    }

    pub fn claim_comment_sign(&self, chat: i64, post: i32) -> bool {
        self.throttle(
            kind::COMMENT_SIGN,
            chat,
            i64::from(post),
            Duration::from_secs(300),
        )
    }

    pub fn claim_moderation(&self, chat: i64, message: i32) -> bool {
        self.throttle(
            kind::MODERATION,
            chat,
            i64::from(message),
            Duration::from_secs(120),
        )
    }

    pub fn first_sighting(&self, chat: i64, user: i64, window: Duration) -> bool {
        self.throttle(kind::SIGHTING, chat, user, window)
    }

    pub fn may_report(&self, chat: i64, user: i64) -> bool {
        self.throttle(kind::REPORT, chat, user, report::EVERY)
    }

    pub fn claim_miniapp_filter_create(&self, chat: i64, every: Duration) -> bool {
        self.throttle(kind::MINIAPP_FILTER_CREATE, chat, 0, every)
    }

    pub fn claim_miniapp_list_read(&self, chat: i64, list: lists::Kind, every: Duration) -> bool {
        self.throttle(kind::MINIAPP_LIST_READ, chat, list.throttle_id(), every)
    }

    pub fn claim_miniapp_list_remove(&self, chat: i64, list: lists::Kind, every: Duration) -> bool {
        self.throttle(kind::MINIAPP_LIST_REMOVE, chat, list.throttle_id(), every)
    }

    pub fn claim_miniapp_list_clear(&self, chat: i64, list: lists::Kind, every: Duration) -> bool {
        self.throttle(kind::MINIAPP_LIST_CLEAR, chat, list.throttle_id(), every)
    }

    fn throttle(&self, kind: u8, chat: i64, user: i64, every: Duration) -> bool {
        let Some(state) = self.try_state(chat) else {
            return false;
        };
        let mut notices = state.notices.lock().unwrap();
        if let Some((last, _)) = notices.get(&(kind, user))
            && last.elapsed() < every
        {
            return false;
        }
        if notices.len() >= PER_CHAT_MAX {
            notices.retain(|_, (last, window)| last.elapsed() < *window);
            make_room(&mut notices, PER_CHAT_MAX, |(last, _)| *last);
        }
        notices.insert((kind, user), (Instant::now(), every));
        true
    }

    pub fn keep_deleted(&self, text: String) -> u64 {
        let key = self.next_deleted_key.fetch_add(1, Ordering::Relaxed);
        let mut deleted = self.deleted.write().unwrap();
        if deleted.len() >= DELETED_MAX {
            deleted.retain(|_, (kept, _)| kept.elapsed() < DELETED_TTL);
            make_room(&mut deleted, DELETED_MAX, |(kept, _)| *kept);
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
        let Some(state) = self.try_state(chat) else {
            return 0;
        };
        let mut messages = state.messages.lock().unwrap();
        if messages.len() >= PER_CHAT_MAX && !messages.contains_key(&user) {
            messages.retain(|_, times| times.iter().any(|t| t.elapsed() < window));
            make_room(&mut messages, PER_CHAT_MAX, |times| {
                times.front().copied().unwrap_or_else(Instant::now)
            });
        }
        let times = messages.entry(user).or_default();
        record_event_bounded(
            times,
            window,
            if user == 0 {
                EVENTS_PER_SUBJECT_MAX
            } else {
                FLOOD_EVENTS_MAX
            },
        )
    }

    pub fn record_removal(&self, chat: i64, actor: i64, window: Duration) -> usize {
        let Some(state) = self.try_state(chat) else {
            return 0;
        };
        let mut removals = state.removals.lock().unwrap();
        if removals.len() >= PER_CHAT_MAX && !removals.contains_key(&actor) {
            removals.retain(|_, times| times.iter().any(|t| t.elapsed() < window));
            make_room(&mut removals, PER_CHAT_MAX, |times| {
                times.front().copied().unwrap_or_else(Instant::now)
            });
        }
        let times = removals.entry(actor).or_default();
        record_event_bounded(times, window, REMOVAL_EVENTS_MAX)
    }

    pub async fn admit_chat(
        &self,
        chat: i64,
        peer: PeerRef,
    ) -> Result<Arc<ChatState>, ChatAdmissionError> {
        if !self.owns_chat(chat) {
            return Err(ChatAdmissionError::RouteRejected);
        }
        let hash = peer.auth.hash();
        if self.settings.value_parsed::<i64>(chat, HASH) != Some(hash)
            && let Err(error) = self
                .settings
                .try_set_value(chat, HASH, &hash.to_string())
                .await
        {
            return Err(match error {
                SettingsWriteError::CapacityReached(scope) => {
                    ChatAdmissionError::DurableCapacityReached(scope)
                }
                other => ChatAdmissionError::Persistence(other),
            });
        }
        let state = self
            .try_state(chat)
            .ok_or(ChatAdmissionError::RuntimeCapacityReached)?;
        *state.peer.write().unwrap() = Some(peer);
        self.touch(&state);
        Ok(state)
    }

    pub fn bump(&self, chat: i64, counter: &'static str) {
        if let Some(state) = self.try_state(chat) {
            state.bump(counter);
        }
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
        state
            .last_seen
            .store(self.uptime_millis(), Ordering::Relaxed);
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
            if chats.get(&chat).is_some_and(|state| {
                Arc::strong_count(state) == 1
                    && !self.settings.is_locked(chat, bots::LOCK)
                    && state.evictable(idle, now)
            }) {
                chats.remove(&chat);
            }
        }
        before - chats.len()
    }

    pub fn take_stats(&self) -> (Tallies, Counts) {
        let mut tallies = HashMap::new();
        let mut counts = HashMap::new();
        let mut remaining = STATS_ROWS_PER_FLUSH;
        for chat in Dirty::take(&self.dirty.stats, FLUSH_CHAT_BATCH) {
            let Some(state) = self.peek(chat) else {
                continue;
            };
            for (counter, count) in std::mem::take(&mut *state.tallies.lock().unwrap()) {
                tallies.insert((chat, counter), count);
            }
            if remaining == 0 {
                Dirty::mark(&self.dirty.stats, chat);
                continue;
            }
            let mut per_user = state.counts.lock().unwrap();
            let room = per_user.len();
            let taken = room.min(remaining);
            let mut left = HashMap::with_capacity(room.saturating_sub(taken));
            for (index, (user, count)) in
                std::mem::replace(&mut *per_user, HashMap::with_capacity(room))
                    .into_iter()
                    .enumerate()
            {
                if index < taken {
                    counts.insert((chat, user), count);
                } else {
                    left.insert(user, count);
                }
            }
            remaining -= taken;
            *per_user = left;
            if !per_user.is_empty() {
                Dirty::mark(&self.dirty.stats, chat);
            }
        }
        (tallies, counts)
    }

    pub fn stats_flush_batches(&self) -> usize {
        self.dirty
            .stats
            .0
            .lock()
            .unwrap()
            .len()
            .div_ceil(FLUSH_CHAT_BATCH)
    }

    fn live_ref(&self, chat: i64) -> Option<PeerRef> {
        let state = self.live_state(chat)?;
        *state.peer.read().unwrap()
    }

    fn live_state(&self, chat: i64) -> Option<Arc<ChatState>> {
        let state = self.peek(chat)?;
        let has_peer = state.peer.read().unwrap().is_some();
        has_peer.then_some(state)
    }

    pub fn chat_ref(&self, chat: i64) -> Option<PeerRef> {
        if !self.owns_chat(chat) {
            return None;
        }
        if let Some(peer) = self.live_ref(chat) {
            return Some(peer);
        }
        let id = PeerId::from_bot_api_dialog_id(chat)?;
        let hash = self
            .settings
            .value_parsed::<i64>(chat, HASH)
            .or_else(|| self.settings.durable_chat_hash(chat))?;
        if id.kind() == PeerKind::Channel && hash == 0 {
            return None;
        }
        Some(PeerRef {
            id,
            auth: PeerAuth::from_hash(hash),
        })
    }

    pub fn forget_admins(&self, chat: i64) {
        if let Some(state) = self.peek(chat) {
            *state.admins.write().unwrap() = None;
        }
    }

    fn cache_admins(&self, chat: i64, admins: HashSet<i64>) {
        if let Some(state) = self.try_state(chat) {
            *state.admins.write().unwrap() = Some((Instant::now(), admins));
        }
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

fn is_badge_only(permissions: &grammers_client::peer::Permissions) -> bool {
    let grammers_client::tl::types::ChatAdminRights {
        anonymous,
        change_info,
        post_messages,
        edit_messages,
        delete_messages,
        ban_users,
        invite_users,
        pin_messages,
        add_admins,
        manage_call,
        other,
        manage_topics,
        post_stories,
        edit_stories,
        delete_stories,
        manage_direct_messages,
        manage_ranks,
        manage_linked_peers,
    } = permissions.raw;
    invite_users
        && !(anonymous
            || change_info
            || post_messages
            || edit_messages
            || delete_messages
            || ban_users
            || pin_messages
            || add_admins
            || manage_call
            || other
            || manage_topics
            || post_stories
            || edit_stories
            || delete_stories
            || manage_direct_messages
            || manage_ranks
            || manage_linked_peers)
}

fn wears_only_a_badge(
    ctx: &Ctx,
    chat: i64,
    user: i64,
    participant: &grammers_client::peer::Participant,
) -> bool {
    let grammers_client::peer::Role::Admin(admin) = &participant.role else {
        return false;
    };
    ctx.settings.is_locked(chat, &stats::badge_key(user)) && is_badge_only(admin.permissions())
}

pub async fn chat_admins(ctx: &Ctx, chat_ref: PeerRef, chat: i64) -> Option<HashSet<i64>> {
    if let Some(admins) = ctx.cached_admins(chat) {
        return Some(admins);
    }

    let state = ctx.try_state(chat)?;
    let _fetching = state.admin_fetch.lock().await;
    if let Some(admins) = ctx.cached_admins(chat) {
        return Some(admins);
    }

    let mut participants = ctx
        .client
        .iter_participants(chat_ref)
        .filter(grammers_client::tl::enums::ChannelParticipantsFilter::ChannelParticipantsAdmins);
    let mut admins = HashSet::new();
    let mut failed = false;
    let mut truncated = false;
    loop {
        match participants.next().await {
            Ok(Some(participant)) => {
                let Some(id) = participant
                    .id()
                    .bare_id()
                    .filter(|_| participant.id().kind() == PeerKind::User)
                else {
                    continue;
                };
                if holds_the_group(&participant) && !wears_only_a_badge(ctx, chat, id, &participant)
                {
                    admins.insert(id);
                    if admins.len() >= ADMIN_CACHE_MAX {
                        truncated = true;
                        break;
                    }
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

    if failed || truncated || admins.is_empty() {
        return None;
    }
    ctx.cache_admins(chat, admins.clone());
    Some(admins)
}

pub async fn dispatch(ctx: &Arc<Ctx>, update: Update) {
    let message = match update {
        Update::NewMessage(message) if !message.outgoing() => message,

        Update::MessageEdited(message) if !message.outgoing() => {
            if is_group_message(ctx, &message).await {
                let Some(chat) = message.peer_id().bot_api_dialog_id() else {
                    return;
                };
                let Ok(Some(peer)) = message.peer_ref().await else {
                    return;
                };
                let _state = match ctx.admit_chat(chat, peer).await {
                    Ok(state) => state,
                    Err(error) => {
                        ctx.admission_failed(chat, error);
                        return;
                    }
                };
                let _slot = _state.slot().await;
                let view = locks::View::new(&message);
                locks::on_edit(ctx, &message, &view).await;
                trade::watch(ctx, &message, chat, &view).await;
            }
            return;
        }
        Update::CallbackQuery(query) => {
            callbacks::handle(ctx, &query).await;
            return;
        }

        Update::Raw(raw) => {
            let raw_chat = raw_chat_id(&raw);
            let admitted = if let Some(chat) = raw_chat {
                let Some(peer) = ctx.resolve_group(chat).await else {
                    return;
                };
                let state = match ctx.admit_chat(chat, peer).await {
                    Ok(state) => state,
                    Err(error) => {
                        ctx.admission_failed(chat, error);
                        return;
                    }
                };
                Some(state)
            } else {
                None
            };
            invalidate_admins(ctx, &raw);
            let state = admitted;
            let _slot = match state.as_deref() {
                Some(state) => Some(state.slot().await),
                None => None,
            };
            autoconfig::on_raw(ctx, &raw).await;
            if let grammers_client::tl::enums::Update::ChannelParticipant(update) = &raw.raw {
                betrayal::on_participant_update(ctx, update).await;
                bots::on_participant_update(ctx, update).await;
                raid::on_participant_update(ctx, update).await;
                log::on_participant(ctx, update).await;
                leftback::on_participant_update(ctx, update).await;
            }
            pinlock::on_raw(ctx, &raw).await;
            return;
        }
        _ => return,
    };
    let message = &message;

    if age_seconds(message) > STALE_AFTER {
        if let Some(chat) = message
            .peer_id()
            .bot_api_dialog_id()
            .filter(|chat| *chat < 0)
        {
            match message.peer_ref().await {
                Ok(Some(peer)) => {
                    if let Err(error) = ctx.admit_chat(chat, peer).await {
                        ctx.admission_failed(chat, error);
                    }
                }
                Ok(None) => ctx.persistence_failed(&format!(
                    "stale group update {chat} had no peer reference for durable admission"
                )),
                Err(error) => ctx.persistence_failed(&format!(
                    "stale group update {chat} could not resolve peer reference: {error}"
                )),
            }
            if let Err(error) = captcha::resume_durable_rejoin(ctx, message).await {
                ctx.persistence_failed(&format!(
                    "stale join persistence for {chat} could not resume moderation: {error}"
                ));
            }
            if let Err(error) = captcha::resume_interrupted_setup(ctx, message).await {
                ctx.persistence_failed(&format!(
                    "stale join persistence for {chat} could not reconstruct captcha setup: {error}"
                ));
            }
        }
        STALE_UPDATES.fetch_add(1, Ordering::Relaxed);
        return;
    }

    if message.peer_id().kind() == PeerKind::User {
        let handled = cleaner::take_password(ctx, message).await
            || config::start(ctx, message).await
            || config::help(ctx, message).await
            || currency::handle(ctx, message).await
            || cleaner::handle(ctx, message).await
            || sudo::handle(ctx, message).await
            || panel::handle_private(ctx, message).await;
        if handled {
            return;
        }
        if ctx.maybe_expecting_number() {
            let view = locks::View::new(message);
            if panel::typed_number(ctx, message, &view).await {
                return;
            }
        }
        return;
    }

    if !is_group_message(ctx, message).await {
        return;
    }

    let Some(chat) = chat_id(message) else {
        return;
    };

    let state = if let Some(state) = ctx.live_state(chat) {
        ctx.touch(&state);
        state
    } else {
        let Ok(Some(peer)) = message.peer_ref().await else {
            return;
        };

        let state = match ctx.admit_chat(chat, peer).await {
            Ok(state) => state,
            Err(error) => {
                ctx.admission_failed(chat, error);
                return;
            }
        };

        if let Some(title) = message.peer().and_then(|peer| peer.name())
            && ctx.settings.value(chat, TITLE).as_deref() != Some(title)
            && let Err(error) = ctx.settings.try_set_value(chat, TITLE, title).await
        {
            ::log::warn!("chat admission: could not store title for {chat}: {error}");
        }
        state
    };

    let _slot = state.slot().await;

    autoconfig::on_message(ctx, message, &state).await;

    if !state.swept_bots.load(Ordering::Relaxed)
        && ctx.settings.is_locked(chat, bots::LOCK)
        && !state.swept_bots.swap(true, Ordering::Relaxed)
    {
        let permit = ctx.sweep_slot().await;
        let ctx = Arc::clone(ctx);
        Arc::clone(&ctx).spawn_owned(async move {
            let _permit = permit;
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
    if flood::check(ctx, message).await {
        return;
    }

    tempmedia::watch(ctx, message, &view).await;

    restrict::remember(ctx, chat, message);

    comment::on_post(ctx, &state, message, chat).await;

    nsfw::watch(ctx, message, chat, &view).await;

    voicemonitor::watch(ctx, message, chat, &view).await;

    trade::watch(ctx, message, chat, &view).await;

    if panel::typed_number(ctx, message, &view).await {
        return;
    }

    if join::enforce(ctx, message).await {
        return;
    }

    let bot_authored = bot_authored(message);

    let _ = locks::handle(ctx, message, &view).await
        || cleaner::on_join(ctx, message).await
        || bots::handle(ctx, message).await
        || captcha::on_join(ctx, message).await
        || welcome::on_join(ctx, message).await
        || (!bot_authored
            && (welcome::handle(ctx, message).await
                || config::handle(ctx, message).await
                || ephemeral::handle(ctx, message).await
                || panel::handle(ctx, message).await
                || restrict::handle(ctx, message, &view).await
                || restrict::handle_custom_setup(ctx, message).await
                || lists::command(ctx, message).await
                || install::handle(ctx, message).await
                 || voicemonitor::handle(ctx, message).await
                 || ping::handle(ctx, message).await
                 || currency::handle(ctx, message).await
                 || sudo::handle(ctx, message).await
                || promote::handle(ctx, message, &view).await
                || stats::handle(ctx, message).await
                || cases::handle(ctx, message).await
                || report::handle(ctx, message).await
                || packs::handle(ctx, message).await
                || extras::handle(ctx, message, &view).await
                || captcha::handle(ctx, message).await
                || tune::handle(ctx, message, &view).await
                || warns::handle(ctx, message, &view).await
                || imgfilter::handle(ctx, message).await
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
                || invite::handle(ctx, message).await
                || vip::handle(ctx, message).await
                || bots::allow(ctx, message).await
                || restrict::handle_custom(ctx, message, &view).await))
        || (!bot_authored && answers::handle(ctx, message, &view).await);

    locks::service(ctx, message).await;
}

pub async fn respond(
    ctx: &Ctx,
    message: &Message,
    kind: crate::response::ResponseKind,
    content: impl Into<grammers_client::message::InputMessage>,
) {
    if let Err(error) = crate::response::send(
        &ctx.client,
        &ctx.settings,
        message,
        kind,
        crate::response::IntendedAudience::RequesterOnly,
        content.into(),
    )
    .await
    {
        ::log::warn!(
            "response_delivery kind={} delivery=failed error={error}",
            kind.as_str()
        );
    }
}

pub async fn announce(
    ctx: &Ctx,
    message: &Message,
    kind: crate::response::ResponseKind,
    content: impl Into<grammers_client::message::InputMessage>,
) {
    if let Err(error) = crate::response::send(
        &ctx.client,
        &ctx.settings,
        message,
        kind,
        crate::response::IntendedAudience::WholeGroup,
        content.into(),
    )
    .await
    {
        ::log::warn!(
            "response_delivery kind={} delivery=failed error={error}",
            kind.as_str()
        );
    }
}

pub async fn respond_shared(
    ctx: &Ctx,
    message: &Message,
    kind: crate::response::ResponseKind,
    content: impl Into<grammers_client::message::InputMessage>,
) {
    if let Err(error) = crate::response::send(
        &ctx.client,
        &ctx.settings,
        message,
        kind,
        crate::response::IntendedAudience::SharedWorkflow,
        content.into(),
    )
    .await
    {
        ::log::warn!(
            "response_delivery kind={} delivery=failed error={error}",
            kind.as_str()
        );
    }
}

pub async fn respond_if_private(
    ctx: &Ctx,
    message: &Message,
    kind: crate::response::ResponseKind,
    content: impl Into<grammers_client::message::InputMessage>,
) {
    if let Err(error) =
        crate::response::send_if_private(&ctx.client, &ctx.settings, message, kind, content.into())
            .await
    {
        ::log::warn!(
            "response_delivery kind={} delivery=failed error={error}",
            kind.as_str()
        );
    }
}

pub fn dispatch_key(update: &Update) -> i64 {
    match update {
        Update::NewMessage(message) | Update::MessageEdited(message) => {
            message.peer_id().bot_api_dialog_id().unwrap_or(0)
        }
        Update::CallbackQuery(query) => {
            let here = query.peer_id().bot_api_dialog_id().unwrap_or(0);
            callbacks::dispatch_chat(query.data(), here)
        }
        Update::Raw(raw) => raw_chat_id(raw).unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod scalability_tests;

fn bot_authored(message: &Message) -> bool {
    matches!(message.sender(), Some(Peer::User(user)) if user.is_bot())
}

fn invalidate_admins(ctx: &Ctx, raw: &grammers_client::update::Raw) {
    use grammers_client::tl;
    use tl::enums::ChannelParticipant as P;

    let admin_change =
        |participant: &Option<P>| matches!(participant, Some(P::Admin(_) | P::Creator(_)));
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

fn raw_chat_id(raw: &grammers_client::update::Raw) -> Option<i64> {
    use grammers_client::tl;

    let peer = match &raw.raw {
        tl::enums::Update::Channel(update) => PeerId::channel(update.channel_id),
        tl::enums::Update::Chat(update) => PeerId::chat(update.chat_id),
        tl::enums::Update::ChatParticipantAdd(update) => PeerId::chat(update.chat_id),
        tl::enums::Update::ChannelParticipant(update) => PeerId::channel(update.channel_id),
        tl::enums::Update::ChatParticipantAdmin(update) => PeerId::chat(update.chat_id),
        tl::enums::Update::PinnedChannelMessages(update) => PeerId::channel(update.channel_id),
        tl::enums::Update::PinnedMessages(update) => PeerId::try_from(&update.peer).ok(),
        _ => return None,
    }?;
    peer.bot_api_dialog_id()
}

const STALE_AFTER: i64 = 120;
pub static STALE_UPDATES: AtomicU64 = AtomicU64::new(0);

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

fn is_group_dialog(id: PeerId, peer: Option<&grammers_client::peer::Peer>) -> bool {
    match peer {
        Some(grammers_client::peer::Peer::Group(group)) if group.id() == id => {
            use grammers_client::tl::enums::Chat;
            match &group.raw {
                Chat::Chat(_) | Chat::Forbidden(_) => true,
                Chat::Channel(channel) => {
                    !channel.broadcast && (channel.megagroup || channel.gigagroup)
                }
                Chat::ChannelForbidden(channel) => !channel.broadcast && channel.megagroup,
                _ => false,
            }
        }
        Some(_) => false,
        None => id.kind() == PeerKind::Chat,
    }
}

async fn is_group_message(ctx: &Ctx, message: &Message) -> bool {
    if message.peer().is_some() || message.peer_id().kind() != PeerKind::Channel {
        return is_group_dialog(message.peer_id(), message.peer());
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    ctx.resolve_group(chat).await.is_some()
}

pub fn is_bot_admin(ctx: &Ctx, chat: i64, user: i64) -> bool {
    ctx.settings.is_locked(chat, &bot_admin_key(user))
}

pub fn bot_admin_key(user: i64) -> String {
    format!("admin:{user}")
}

fn saved_from_itself(sender: Option<PeerId>, saved_from: Option<PeerId>) -> bool {
    match (sender, saved_from) {
        (Some(sender), Some(saved_from)) => {
            sender.kind() == PeerKind::Channel && sender == saved_from
        }
        _ => false,
    }
}

pub fn is_linked_post(message: &Message) -> bool {
    let grammers_client::tl::enums::Message::Message(raw) = &message.raw else {
        return false;
    };
    let Some(grammers_client::tl::enums::MessageFwdHeader::Header(header)) = raw.fwd_from.as_ref()
    else {
        return false;
    };
    saved_from_itself(
        message.sender_id(),
        header
            .saved_from_peer
            .as_ref()
            .and_then(|peer| PeerId::try_from(peer).ok()),
    )
}

pub async fn is_exempt(ctx: &Ctx, message: &Message) -> bool {
    if is_linked_post(message) {
        return true;
    }
    if let (Some(chat), Some(sender)) = (
        chat_id(message),
        message.sender_id().and_then(PeerId::bare_id),
    ) && vip::is_vip(ctx, chat, sender)
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

        None => {
            !ctx.settings.is_locked(chat, &stats::badge_key(sender))
                && permissions(ctx, message)
                    .await
                    .is_some_and(|p| p.is_admin())
        }
    }
}

pub fn is_owner(ctx: &Ctx, message: &Message) -> bool {
    match (
        chat_id(message),
        message.sender_id().and_then(PeerId::bare_id),
    ) {
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

pub async fn resolve(ctx: &Ctx, message: &Message, named: Named<'_>) -> Option<(PeerRef, String)> {
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
        .any(|c| matches!(c, '۰'..='۹' | '٠'..='٩' | 'ي' | 'ى' | 'ك'))
    {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(
        text.chars()
            .map(|c| match c {
                '۰'..='۹' => char::from(b'0' + (c as u32 - '۰' as u32) as u8),
                '٠'..='٩' => char::from(b'0' + (c as u32 - '٠' as u32) as u8),
                'ي' | 'ى' => 'ی',
                'ك' => 'ک',
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

    let mut participants = ctx
        .client
        .iter_participants(chat_ref)
        .filter(grammers_client::tl::enums::ChannelParticipantsFilter::ChannelParticipantsRecent);
    let mut found = Vec::new();
    while let Ok(Some(participant)) = participants.next().await {
        let Some(user) = participant.user() else {
            continue;
        };
        let Some(id) = user.id().bare_id() else {
            continue;
        };
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
    let mut participants = ctx
        .client
        .iter_participants(chat)
        .filter(grammers_client::tl::enums::ChannelParticipantsFilter::ChannelParticipantsAdmins);
    let mut seen = 0;
    while let Ok(Some(participant)) = participants.next().await {
        seen += 1;
        if seen > ADMIN_CACHE_MAX {
            break;
        }
        if participant.id().bare_id() != Some(user_id) || !holds_the_group(&participant) {
            continue;
        }
        let user = participant.user()?;
        let name = user.full_name();
        return user.to_ref().await.ok().flatten().map(|peer| (peer, name));
    }
    None
}

pub struct AdminEntry {
    pub id: i64,
    pub name: String,
    pub is_creator: bool,
    pub is_bot: bool,
}

pub async fn admin_entries(ctx: &Ctx, chat: PeerRef) -> Vec<AdminEntry> {
    let mut participants = ctx
        .client
        .iter_participants(chat)
        .filter(grammers_client::tl::enums::ChannelParticipantsFilter::ChannelParticipantsAdmins);
    let mut found = Vec::new();
    while let Ok(Some(participant)) = participants.next().await {
        if !holds_the_group(&participant) {
            continue;
        }
        if found.len() >= ADMIN_CACHE_MAX {
            break;
        }
        let Some(id) = participant
            .id()
            .bare_id()
            .filter(|_| participant.id().kind() == PeerKind::User)
        else {
            continue;
        };
        let user = participant.user();
        found.push(AdminEntry {
            id,
            name: user
                .map(|user| esc(&user.full_name()))
                .unwrap_or_else(|| id.to_string()),
            is_creator: matches!(&participant.role, grammers_client::peer::Role::Creator(_)),
            is_bot: user.is_some_and(|user| user.is_bot()),
        });
    }
    found
}

pub async fn admins(ctx: &Ctx, chat: PeerRef) -> (Option<(i64, String)>, Vec<String>) {
    let entries = admin_entries(ctx, chat).await;
    let creator = entries
        .iter()
        .find(|entry| entry.is_creator)
        .map(|entry| (entry.id, entry.name.clone()));
    let names = entries
        .into_iter()
        .map(|entry| {
            if entry.is_bot {
                format!("‹ {} · ربات", entry.name)
            } else if entry.is_creator {
                format!("★ {}", entry.name)
            } else {
                format!("‹ {}", entry.name)
            }
        })
        .collect();
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
    use super::{
        BackgroundEpochGuard, Ctx, OwnedTaskCheckpoint, OwnedTasks, PasswordMailbox, PeerId,
        PersistenceBarrier, SampleFlushGuard, saved_from_itself,
    };

    #[test]
    fn context_inline_layout_stays_below_the_test_thread_stack() {
        let bytes = std::mem::size_of::<Ctx>();
        println!("Ctx inline layout: {bytes} bytes");
        assert!(bytes < 64 * 1024, "Ctx grew to {bytes} inline bytes");
    }

    #[tokio::test]
    async fn persistence_poison_is_sticky_and_wakes_the_supervisor_immediately() {
        let barrier = std::sync::Arc::new(PersistenceBarrier::default());
        let waiting = std::sync::Arc::clone(&barrier);
        let waiter = tokio::spawn(async move { waiting.wait().await });
        tokio::task::yield_now().await;
        barrier.poison();
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("supervisor was not woken")
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(10), barrier.wait())
            .await
            .expect("a supervisor that starts after poison missed the sticky failure");
    }

    #[tokio::test]
    async fn background_panic_is_published_before_the_epoch_unlocks() {
        let epoch = std::sync::Arc::new(tokio::sync::RwLock::new(()));
        let panicked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_epoch = std::sync::Arc::clone(&epoch);
        let task_panicked = std::sync::Arc::clone(&panicked);
        let (acquired, acquired_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = BackgroundEpochGuard {
                _epoch: task_epoch.read().await,
                panicked: &task_panicked,
            };
            acquired.send(()).unwrap();
            panic!("injected background failure");
        });

        acquired_rx.await.unwrap();
        let writer = epoch.write().await;
        assert!(panicked.load(std::sync::atomic::Ordering::Acquire));
        drop(writer);
        assert!(task.await.unwrap_err().is_panic());
    }

    #[tokio::test]
    async fn owned_task_drain_closes_admission_and_joins_the_exact_accepted_set() {
        let completed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut owned = OwnedTasks::default();
        let accepted = std::sync::Arc::clone(&completed);
        assert!(owned.spawn(async move {
            accepted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));

        let mut drain = owned.begin_drain();
        let rejected = std::sync::Arc::clone(&completed);
        assert!(!owned.spawn(async move {
            rejected.fetch_add(100, std::sync::atomic::Ordering::SeqCst);
        }));
        assert_eq!(drain.join().await, 0);

        assert_eq!(completed.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_owned_join_retains_the_same_tasks_for_abort() {
        struct Dropped(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = Dropped(std::sync::Arc::clone(&dropped));
        let mut owned = OwnedTasks::default();
        assert!(owned.spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        }));
        let mut drain = owned.begin_drain();

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), drain.join())
                .await
                .is_err()
        );
        assert_eq!(drain.abort().await, 1);
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn owned_task_panic_is_retained_until_shutdown_decides_cursor_safety() {
        let mut owned = OwnedTasks::default();
        assert!(owned.spawn(async { panic!("injected owned-task failure") }));
        tokio::task::yield_now().await;

        let mut drain = owned.begin_drain();
        assert_eq!(drain.join().await, 1);
    }

    #[tokio::test]
    async fn cancelled_checkpoint_join_retains_handles_until_abort_is_observed() {
        struct Dropped(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = Dropped(std::sync::Arc::clone(&dropped));
        let owned = std::sync::Arc::new(std::sync::Mutex::new(OwnedTasks::default()));
        assert!(owned.lock().unwrap().spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        }));
        let epoch = owned.lock().unwrap().take_epoch();
        let mut checkpoint = OwnedTaskCheckpoint { epoch, failures: 0 };

        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(5),
                checkpoint.join_epochs(&owned),
            )
            .await
            .is_err()
        );
        assert!(!dropped.load(std::sync::atomic::Ordering::SeqCst));
        checkpoint.retain_epoch(&owned);
        let mut shutdown = owned.lock().unwrap().begin_drain();
        assert_eq!(shutdown.abort().await, 1);
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn checkpoint_joins_children_admitted_by_the_captured_epoch() {
        let completed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let owned = std::sync::Arc::new(std::sync::Mutex::new(OwnedTasks::default()));
        let child_owner = std::sync::Arc::clone(&owned);
        let child_completed = std::sync::Arc::clone(&completed);
        assert!(owned.lock().unwrap().spawn(async move {
            assert!(child_owner.lock().unwrap().spawn(async move {
                child_completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }));
        }));
        let epoch = owned.lock().unwrap().take_epoch();
        let mut checkpoint = OwnedTaskCheckpoint { epoch, failures: 0 };

        assert_eq!(checkpoint.join_epochs(&owned).await, 0);
        assert_eq!(completed.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn cancelled_sample_flush_rearms_the_dirty_bit() {
        let dirty = std::sync::atomic::AtomicBool::new(false);
        {
            let _flush = SampleFlushGuard {
                dirty: &dirty,
                complete: false,
            };
        }
        assert!(dirty.load(std::sync::atomic::Ordering::Acquire));
        dirty.store(false, std::sync::atomic::Ordering::Release);
        {
            let _flush = SampleFlushGuard {
                dirty: &dirty,
                complete: true,
            };
        }
        assert!(!dirty.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn restriction_coordinators_have_stable_bounded_stripes() {
        assert_eq!(super::RESTRICTION_WRITE_STRIPES, 4_096);
        assert_eq!(super::GROUP_RIGHTS_STRIPES, 2_048);
        assert_eq!(
            super::restriction_write_stripe(-1_001, 42),
            super::restriction_write_stripe(-1_001, 42)
        );
        assert_eq!(
            super::group_rights_stripe(-1_001),
            super::group_rights_stripe(-1_001)
        );
        assert!(super::restriction_write_stripe(i64::MIN, i64::MAX) < 4_096);
        assert!(super::group_rights_stripe(i64::MIN) < 2_048);
    }

    #[tokio::test]
    async fn password_handoff_wakes_exactly_one_waiter_without_polling() {
        let mailbox = PasswordMailbox::default();
        let waiting = mailbox.arm();

        assert!(mailbox.give("secret".to_owned()));
        assert_eq!(
            mailbox
                .wait(waiting, std::time::Duration::from_millis(10))
                .await
                .as_deref(),
            Some("secret")
        );
        assert!(!mailbox.give("second".to_owned()));
    }

    #[tokio::test]
    async fn stale_password_waiter_cannot_cancel_a_new_request() {
        let mailbox = PasswordMailbox::default();
        let stale = mailbox.arm();
        let current = mailbox.arm();

        assert!(
            mailbox
                .wait(stale, std::time::Duration::from_millis(10))
                .await
                .is_none()
        );
        assert!(mailbox.give("current".to_owned()));
        assert_eq!(
            mailbox
                .wait(current, std::time::Duration::from_millis(10))
                .await
                .as_deref(),
            Some("current")
        );
    }

    #[tokio::test]
    async fn timed_out_password_request_stops_accepting_secrets() {
        let mailbox = PasswordMailbox::default();
        let waiting = mailbox.arm();

        assert!(
            mailbox
                .wait(waiting, std::time::Duration::from_millis(1))
                .await
                .is_none()
        );
        assert!(!mailbox.give("late".to_owned()));
    }

    #[tokio::test]
    async fn group_routing_distinguishes_broadcasts_from_supergroups_with_the_same_id_format() {
        use grammers_client::{Client, SenderPool, peer::Peer, tl};
        use grammers_session::storages::MemorySession;
        use std::sync::Arc;

        let pool = SenderPool::new(Arc::new(MemorySession::default()), 1);
        let client = Client::new(pool.handle.clone());
        let id = PeerId::channel(123).unwrap();
        for (broadcast, megagroup, allowed) in [
            (true, false, false),
            (false, true, true),
            (false, false, false),
        ] {
            let peer = Peer::try_from_raw(
                &client,
                tl::types::ChannelForbidden {
                    broadcast,
                    megagroup,
                    monoforum: false,
                    id: 123,
                    access_hash: 42,
                    title: "dialog".into(),
                    until_date: None,
                }
                .into(),
            )
            .expect("fixture uses a valid Telegram channel identifier");
            assert_eq!(super::is_group_dialog(id, Some(&peer)), allowed);
            assert!(!super::is_group_dialog(
                PeerId::channel(456).unwrap(),
                Some(&peer)
            ));
        }
        assert!(!super::is_group_dialog(id, None));
        assert!(!super::is_group_dialog(PeerId::user(123).unwrap(), None));
        let basic = Peer::try_from_raw(
            &client,
            tl::types::ChatForbidden {
                id: 123,
                title: "group".into(),
            }
            .into(),
        )
        .expect("fixture uses a valid Telegram chat identifier");
        assert!(super::is_group_dialog(basic.id(), Some(&basic)));
        assert!(super::is_group_dialog(basic.id(), None));
    }

    #[test]
    fn pending_numbers_keep_input_and_target_dialogs_separate() {
        let mut pending = super::PendingNumbers::default();
        pending.arm(101, 7, -1_001, "fl_lim");
        pending.arm(202, 7, -2_002, "bt_lim");

        assert_eq!(pending.expected(101, 7), Some((-1_001, "fl_lim")));
        assert_eq!(pending.expected(202, 7), Some((-2_002, "bt_lim")));
        assert_eq!(pending.expected(303, 7), None);

        assert!(!pending.take(101, 7, -9_009, "fl_lim"));
        assert_eq!(pending.expected(101, 7), Some((-1_001, "fl_lim")));
        assert!(pending.take(101, 7, -1_001, "fl_lim"));
        assert!(!pending.take(101, 7, -1_001, "fl_lim"));
        assert_eq!(pending.expected(202, 7), Some((-2_002, "bt_lim")));
    }

    #[test]
    fn pending_number_for_one_dialog_is_replaced_by_the_latest_prompt() {
        let mut pending = super::PendingNumbers::default();
        pending.arm(101, 7, -1_001, "fl_lim");
        pending.arm(101, 7, -2_002, "bt_lim");

        assert_eq!(pending.expected(101, 7), Some((-2_002, "bt_lim")));
    }

    #[test]
    fn expired_pending_numbers_are_not_read_or_taken() {
        use std::time::{Duration, Instant};

        let mut pending = super::PendingNumbers::default();
        pending.entries.insert(
            (101, 7),
            super::PendingNumber {
                armed: Instant::now() - super::PENDING_NUMBER_TTL - Duration::from_secs(1),
                target_chat: -1_001,
                setting: "fl_lim",
            },
        );

        assert_eq!(pending.expected(101, 7), None);
        assert!(!pending.take(101, 7, -1_001, "fl_lim"));
    }

    #[test]
    fn animated_media_rejects_a_thumbnail_cache_but_accepts_full_animation() {
        assert!(
            !super::cache_source_matches(true, false),
            "a GIF must not reuse a result computed from its cover frame"
        );
        assert!(
            super::cache_source_matches(true, true),
            "a GIF may reuse a result computed after full animation sampling"
        );
        assert!(
            super::cache_source_matches(false, false),
            "ordinary still media keeps the existing cache path"
        );
    }

    #[test]
    fn deferred_actions_are_taken_in_deadline_order() {
        use std::collections::BinaryHeap;
        use std::time::{Duration, Instant};

        let now = Instant::now();
        let mut queue = BinaryHeap::new();
        queue.push(super::DeferredEntry {
            due: now + Duration::from_secs(3),
            sequence: 0,
            action: super::DeferredAction::Delete {
                chat: 1,
                message: 3,
            },
        });
        queue.push(super::DeferredEntry {
            due: now + Duration::from_secs(1),
            sequence: 1,
            action: super::DeferredAction::Delete {
                chat: 1,
                message: 1,
            },
        });
        queue.push(super::DeferredEntry {
            due: now + Duration::from_secs(2),
            sequence: 2,
            action: super::DeferredAction::Delete {
                chat: 1,
                message: 2,
            },
        });

        assert_eq!(
            queue.pop().map(|entry| entry.due),
            Some(now + Duration::from_secs(1))
        );
        assert_eq!(
            queue.pop().map(|entry| entry.due),
            Some(now + Duration::from_secs(2))
        );
        assert_eq!(
            queue.pop().map(|entry| entry.due),
            Some(now + Duration::from_secs(3))
        );
    }

    #[test]
    fn recent_minutes_wrap_at_midnight() {
        assert_eq!(
            super::recent_minutes(0),
            ["0".to_owned(), "1439".to_owned(), "1438".to_owned()]
        );
        assert_eq!(
            super::recent_minutes(1_440),
            ["0".to_owned(), "1439".to_owned(), "1438".to_owned()]
        );
        assert_eq!(
            super::recent_minutes(732),
            ["732".to_owned(), "731".to_owned(), "730".to_owned()]
        );
    }

    #[tokio::test]
    async fn per_chat_slots_release_without_a_per_chat_semaphore_allocation() {
        use std::sync::Arc;
        use tokio::sync::oneshot;

        let state = Arc::new(super::ChatState::default());
        let mut permits = Vec::new();
        for _ in 0..super::PER_CHAT_UPDATES {
            permits.push(state.slot().await);
        }

        let (ready, mut finished) = oneshot::channel();
        let waiter_state = Arc::clone(&state);
        let waiter = tokio::spawn(async move {
            let _permit = waiter_state.slot().await;
            let _ = ready.send(());
        });
        tokio::task::yield_now().await;
        assert!(
            finished.try_recv().is_err(),
            "the per-chat ceiling must hold"
        );

        permits.pop();
        finished
            .await
            .expect("a released slot must wake the waiter");
        waiter.await.expect("the waiter task must finish");
    }

    #[test]
    fn no_two_throttles_share_a_discriminant() {
        for (name, value) in super::kind::ALL {
            let same: Vec<&str> = super::kind::ALL
                .iter()
                .filter(|(other, v)| v == value && other != name)
                .map(|(other, _)| *other)
                .collect();
            assert!(same.is_empty(), "{name} shares {value} with {same:?}");
        }
    }

    #[test]
    fn only_the_channels_own_auto_forward_is_immune() {
        let channel = PeerId::channel_unchecked(100);
        let other = PeerId::channel_unchecked(200);
        let member = PeerId::user_unchecked(300);

        assert!(saved_from_itself(Some(channel), Some(channel)));

        assert!(!saved_from_itself(Some(member), Some(channel)));

        assert!(!saved_from_itself(Some(other), Some(channel)));

        assert!(!saved_from_itself(Some(channel), None));
        assert!(!saved_from_itself(None, Some(channel)));
        assert!(!saved_from_itself(None, None));
    }
    #[test]
    fn a_queue_that_refills_during_a_drain_is_not_lost() {
        use std::sync::Mutex;

        const CHAT: i64 = 7;
        let queue: Mutex<Vec<&str>> = Mutex::new(Vec::new());
        let dirty = super::DirtyList::default();

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
            dirty.0.lock().unwrap().len(),
            1,
            "a busy chat marks once per window, not once per entry"
        );
        assert_eq!(drain(), vec![vec!["one", "two"]]);

        push("three");
        assert_eq!(drain(), vec![vec!["three"]]);

        assert!(
            drain().is_empty(),
            "a chat with nothing queued is never visited"
        );
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
        super::bounded((0..50_000).collect(), CAP, move |_| {
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

        assert_eq!(ran.load(Ordering::SeqCst), 50_000, "every item has to run");
        assert!(peak.load(Ordering::SeqCst) <= CAP, "the cap has to hold");
        assert_eq!(
            live.load(Ordering::SeqCst),
            0,
            "it has to wait for all of them"
        );
    }

    #[tokio::test]
    async fn cancelling_a_bounded_campaign_leaves_no_detached_children() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let entered = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Semaphore::new(0));
        let task = tokio::spawn(super::bounded((0..16).collect(), 4, {
            let entered = Arc::clone(&entered);
            let completed = Arc::clone(&completed);
            let barrier = Arc::clone(&barrier);
            move |_| {
                let entered = Arc::clone(&entered);
                let completed = Arc::clone(&completed);
                let barrier = Arc::clone(&barrier);
                async move {
                    entered.fetch_add(1, Ordering::SeqCst);
                    barrier.acquire().await.unwrap().forget();
                    completed.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while entered.load(Ordering::SeqCst) < 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        barrier.add_permits(16);
        tokio::task::yield_now().await;
        assert_eq!(completed.load(Ordering::SeqCst), 0);
        assert_eq!(entered.load(Ordering::SeqCst), 4);
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

        assert_eq!(digits("قفل عكس"), "قفل عکس");
        assert_eq!(digits("پاكسازي ۵۰"), "پاکسازی 50");
        assert_eq!(digits("لينك"), "لینک");
        assert_eq!(digits("علي"), "علی");
        assert_eq!(digits("عﻟى"), "عﻟی");
        assert!(matches!(
            digits("كانفيگ"),
            std::borrow::Cow::Owned(ref folded) if folded == "کانفیگ"
        ));
        for text in ["قفل عكس", "پاكسازي ۵۰", "۱۲۳"] {
            assert_eq!(digits(text).chars().count(), text.chars().count());
        }
        assert!(digits("۱۲۳").len() < "۱۲۳".len());

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
        state.count(
            7,
            || panic!("the name is read once, not per message"),
            ["k_photo", "h9"],
        );

        let counts = state.counts.lock().unwrap();
        assert_eq!(counts.get(&7), Some(&(2, "Ali".to_owned())));

        let tallies = state.tallies.lock().unwrap();
        assert_eq!(tallies.get("k_text"), Some(&1));
        assert_eq!(tallies.get("k_photo"), Some(&1));
        assert_eq!(tallies.get("h9"), Some(&2));
        drop((counts, tallies));

        assert!(
            state.dirty.stats.0.lock().unwrap().len() <= 2,
            "the mark is a transition, not a per-message write"
        );
    }

    #[test]
    fn per_chat_counter_map_stays_bounded_under_unique_users() {
        use super::{ChatState, PER_CHAT_MAX};

        let state = ChatState::default();
        for user in 0..=(PER_CHAT_MAX as i64) {
            state.count(user, || "member".to_owned(), ["total", "today"]);
        }
        state.count(
            0,
            || panic!("an existing user does not need a name"),
            ["total", "today"],
        );

        let counts = state.counts.lock().unwrap();
        assert_eq!(counts.len(), PER_CHAT_MAX);
        assert_eq!(counts.get(&0).map(|(count, _)| *count), Some(2));
    }

    #[test]
    fn remembered_thread_roots_stay_bounded_and_lose_the_oldest() {
        use super::{ChatState, Queued, ROOTS_MAX, RootClaim};

        let state = ChatState::default();
        for post in 1..=(ROOTS_MAX as i32 + 10) {
            state.remember_post(post);
        }
        assert_eq!(state.roots.lock().unwrap().len(), ROOTS_MAX);
        assert_eq!(state.root_known(1), None, "the oldest went first");
        assert_eq!(state.root_known(ROOTS_MAX as i32 + 10), Some(true));

        let queued = || Queued {
            message: 5,
            sender: Some(9),
            name: "کاربر".to_owned(),
        };
        assert!(matches!(
            state.claim_root(90_001, queued()),
            RootClaim::Mine
        ));
        assert!(matches!(
            state.claim_root(90_001, queued()),
            RootClaim::Waiting
        ));
        assert_eq!(state.root_known(90_001), None, "a claim is not a verdict");
        assert_eq!(
            state.settle_root(90_001, true).len(),
            2,
            "both queued comments"
        );
        assert_eq!(state.root_known(90_001), Some(true));

        assert!(matches!(
            state.claim_root(90_002, queued()),
            RootClaim::Mine
        ));
        state.forget_root(90_002);
        assert_eq!(state.root_known(90_002), None);
    }

    #[test]
    fn flood_windows_have_a_hard_per_subject_cap() {
        use super::{EVENTS_PER_SUBJECT_MAX, record_event};
        use std::collections::VecDeque;
        use std::time::Duration;

        let mut times = VecDeque::new();
        for _ in 0..(EVENTS_PER_SUBJECT_MAX + 100) {
            record_event(&mut times, Duration::from_secs(60));
        }

        assert_eq!(times.len(), EVENTS_PER_SUBJECT_MAX);
    }

    #[test]
    fn bounded_events_preserve_every_supported_threshold_decision() {
        use super::{
            EVENTS_PER_SUBJECT_MAX, FLOOD_EVENTS_MAX, REMOVAL_EVENTS_MAX, record_event_at,
        };
        let origin = std::time::Instant::now();
        for cap in [EVENTS_PER_SUBJECT_MAX, FLOOD_EVENTS_MAX, REMOVAL_EVENTS_MAX] {
            let mut bounded = std::collections::VecDeque::new();
            let mut reference = std::collections::VecDeque::new();
            let mut elapsed = 0u64;
            let mut seed = 42u64;
            for index in 0..20_000 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                elapsed += if index % 2000 < 1500 {
                    seed % 3
                } else {
                    seed % 500
                };
                let now = origin + std::time::Duration::from_millis(elapsed);
                let window = std::time::Duration::from_millis(50 + (index / 311 % 5) * 300);
                let count = record_event_at(&mut bounded, window, cap, now);
                let full = record_event_at(&mut reference, window, usize::MAX, now);
                for threshold in 1..cap {
                    assert_eq!(
                        count > threshold,
                        full > threshold,
                        "cap={cap} index={index} threshold={threshold}"
                    );
                }
                assert!(bounded.len() <= cap);
            }
        }
    }

    #[test]
    fn fresh_cache_entries_still_obey_the_hard_cap() {
        use super::make_room;
        use std::collections::HashMap;
        use std::time::Instant;

        let at = Instant::now();
        let mut entries: HashMap<u64, (Instant, u8)> = (0..8).map(|key| (key, (at, 0))).collect();
        make_room(&mut entries, 8, |(at, _)| *at);
        assert_eq!(entries.len(), 7);
        entries.insert(99, (Instant::now(), 0));
        assert_eq!(entries.len(), 8);
    }

    #[test]
    fn dirty_take_leaves_unselected_chats_marked() {
        use super::{Dirty, DirtyList};

        let list = DirtyList::default();
        for chat in 0..10 {
            Dirty::mark(&list, chat);
        }

        let first = Dirty::take(&list, 3);
        assert_eq!(first.len(), 3);
        assert_eq!(list.0.lock().unwrap().len(), 7);

        let second = Dirty::drain(&list);
        assert_eq!(second.len(), 7);
        assert!(list.0.lock().unwrap().is_empty());
    }

    #[test]
    fn the_wipe_ring_keeps_a_window_and_hands_back_one_member() {
        use super::{ChatState, SAID_MAX};

        let state = ChatState::default();
        for id in 1..=5 {
            state.remember_said(7, id);
            state.remember_said(9, id + 100);
        }

        assert_eq!(state.take_said(7), vec![1, 2, 3, 4, 5]);
        assert!(state.take_said(7).is_empty());
        assert_eq!(state.take_said(9), vec![101, 102, 103, 104, 105]);
        assert!(state.take_said(11).is_empty(), "a member who never spoke");

        for id in 1..=(SAID_MAX as i32 + 10) {
            state.remember_said(3, id);
        }
        let kept = state.take_said(3);
        assert_eq!(kept.len(), SAID_MAX);
        assert_eq!(kept.first(), Some(&11));
        assert_eq!(kept.last(), Some(&(SAID_MAX as i32 + 10)));
    }

    #[test]
    fn a_chat_with_queued_work_is_never_evicted() {
        use super::ChatState;
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        const IDLE: Duration = Duration::from_secs(3600);
        let now = (IDLE.as_millis() as u64) * 3 / 2;

        let state = ChatState::default();
        assert!(state.evictable(IDLE, now), "quiet and empty");

        let configuring = state.configure_lock.try_lock().unwrap();
        assert!(
            !state.evictable(IDLE, now),
            "an activation must keep its single-flight guard"
        );
        drop(configuring);
        assert!(state.evictable(IDLE, now));

        state.logs.lock().unwrap().push("a line".to_owned());
        assert!(!state.evictable(IDLE, now));
        state.logs.lock().unwrap().clear();

        state
            .temp_media
            .lock()
            .unwrap()
            .push_back((Instant::now(), 1));
        assert!(!state.evictable(IDLE, now));
        state.temp_media.lock().unwrap().clear();

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
        use std::collections::VecDeque;
        use std::time::{Duration, Instant};

        const IDLE: Duration = Duration::from_secs(3600);
        let now = (IDLE.as_millis() as u64) * 3 / 2;

        let state = ChatState::default();

        state
            .messages
            .lock()
            .unwrap()
            .insert(7, VecDeque::from([Instant::now()]));
        state
            .removals
            .lock()
            .unwrap()
            .insert(7, VecDeque::from([Instant::now()]));
        state.members.lock().unwrap().insert(7, Instant::now());
        state.adds.lock().unwrap().insert(7, (Instant::now(), 1));
        state
            .notices
            .lock()
            .unwrap()
            .insert((2, 7), (Instant::now(), Duration::from_secs(20)));
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
            include_str!("currency.rs"),
            include_str!("extras.rs"),
            include_str!("filters.rs"),
            include_str!("flood.rs"),
            include_str!("help.rs"),
            include_str!("install.rs"),
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
            include_str!("intent.rs"),
            include_str!("invite.rs"),
            include_str!("trade.rs"),
            include_str!("voicemonitor.rs"),
        ] {
            assert!(!file.contains('\u{200c}'), "found U+200C in a handler");
        }
    }
}
