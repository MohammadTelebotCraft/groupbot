use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

type Result<T> = std::result::Result<T, sqlx::Error>;

fn nonnegative_counter(field: &str, value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        sqlx::Error::Protocol(format!(
            "corrupt counter field {field}={value}; expected a non-negative PostgreSQL bigint"
        ))
    })
}
mod captcha;
mod default_rights;
mod durable;
mod ownership;
mod strict;

pub use captcha::{
    CaptchaAnswerClaim, CaptchaFailureAction, CaptchaPhase, CaptchaQueueClass, CaptchaReservation,
    CaptchaReservationInput, CaptchaReservationOutcome, CaptchaWork,
};
pub use default_rights::{
    DefaultRightsError, EffectiveRights, NightWindow, RightsMask, RightsNoticeKind, RightsSnapshot,
};
pub use durable::StatsBatch;
pub use strict::{
    PendingStrictAction, StrictAction, StrictIncrement, StrictIntentState, StrictQueueClass,
    StrictViolation,
};

#[cfg(test)]
mod scalability_tests;

pub const CASE_RETENTION_SECS: i64 = 90 * 24 * 60 * 60;

#[derive(Clone, Debug)]
pub struct NewModerationCase {
    pub chat: i64,
    pub subject: Option<i64>,
    pub subject_name: String,
    pub source: String,
    pub rule: String,
    pub reason: String,
    pub message: Option<i32>,
    pub media_kind: Option<String>,
    pub evidence: Option<String>,
    pub evidence_hash: Option<Vec<u8>>,
    pub action: String,
    pub action_until: Option<i64>,
    pub status: String,
    pub actor: Option<i64>,
    pub actor_name: String,
    pub event_kind: String,
    pub event_note: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkflowModerationCase {
    pub id: i64,
    pub inserted: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ModerationCase {
    pub id: i64,
    pub chat: i64,
    pub subject: Option<i64>,
    pub subject_name: String,
    pub source: String,
    pub rule: String,
    pub reason: String,
    pub message: Option<i32>,
    pub media_kind: Option<String>,
    pub evidence: Option<String>,
    pub action: String,
    pub action_until: Option<i64>,
    pub status: String,
    pub actor: Option<i64>,
    pub actor_name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ModerationCaseEvent {
    pub id: i64,
    pub case_id: i64,
    pub kind: String,
    pub actor: Option<i64>,
    pub actor_name: String,
    pub action: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ModerationCaseDetail {
    #[serde(flatten)]
    pub case: ModerationCase,
    pub events: Vec<ModerationCaseEvent>,
}

pub struct WarningCaseReversal<'a> {
    pub chat: i64,
    pub case_id: i64,
    pub user: i64,
    pub actor: i64,
    pub actor_name: &'a str,
    pub note: Option<&'a str>,
}

pub struct ModerationCaseTransition<'a> {
    pub chat: i64,
    pub case_id: i64,
    pub from: &'a str,
    pub to: &'a str,
    pub event_kind: &'a str,
    pub actor: Option<(i64, &'a str)>,
    pub action: Option<&'a str>,
    pub note: Option<&'a str>,
}

pub struct NewModerationCaseEvent<'a> {
    pub chat: i64,
    pub case_id: i64,
    pub kind: &'a str,
    pub actor: Option<(i64, &'a str)>,
    pub action: Option<&'a str>,
    pub note: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarningCaseReversalResult {
    Reversed,
    CaseChanged,
    NoWarning,
}

pub const INDEXED: &[&str] = &["filter:", "pack:", "answer:", "strict:", "imgf:", "cmd:"];

pub const MAX_COUNTER_ROWS_PER_CHAT: i64 = 20_000;
pub const MAX_NOTE_ROWS_PER_CHAT: i64 = 20_000;
pub const MAX_IMAGE_FILTERS_PER_CHAT: i64 = 8;
pub const MAX_TALLY_ROWS_PER_CHAT: i64 = 64;
pub const MAX_TALLY_COUNTER_BYTES: usize = 64;

const COUNTER_PREFIXES: &[&str] = &[
    "total:", "today:", "week:", "month:", "seen:", "adds:", "rank:",
];

type ChatIndex = [Vec<Box<str>>; INDEXED.len()];

const INDEXES: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS pending_due ON pending_deletes (due_at)",
    "DROP INDEX IF EXISTS pending_captcha_due",
    "CREATE INDEX IF NOT EXISTS pending_captcha_fresh_due
     ON pending_captchas (due_at, chat_id, user_id) WHERE state = 'pending'",
    "CREATE INDEX IF NOT EXISTS pending_captcha_retry_due
     ON pending_captchas (retry_at, attempts, due_at, chat_id, user_id)
     WHERE state <> 'pending' AND quarantined_at IS NULL",
    "CREATE INDEX IF NOT EXISTS pending_captcha_quarantine_due
     ON pending_captchas (retry_at, attempts, due_at, chat_id, user_id)
     WHERE quarantined_at IS NOT NULL",
    "CREATE INDEX IF NOT EXISTS started_users_recent ON started_users (last_started_at, user_id)",
    "CREATE INDEX IF NOT EXISTS settings_bot_admin ON settings (key) WHERE key LIKE 'admin:%'",
    "CREATE INDEX IF NOT EXISTS settings_owner ON settings (value) WHERE key = 'owner'",
    "CREATE INDEX IF NOT EXISTS settings_key_chat ON settings (key, chat_id)",
    "CREATE INDEX IF NOT EXISTS settings_cleaner_recommend_due ON settings ((-(chat_id % 360)), chat_id) \
     WHERE key = 'hash' AND chat_id < 0 AND value <> ''",
    "CREATE INDEX IF NOT EXISTS settings_badge_rows ON settings (chat_id, key) \
     WHERE key LIKE 'badge:%'",
    "CREATE INDEX IF NOT EXISTS settings_report_at ON settings (chat_id) WHERE key = 'report_at'",
    "CREATE INDEX IF NOT EXISTS settings_purge_at ON settings (chat_id) \
     WHERE key = 'auto_purge_at'",
    "CREATE INDEX IF NOT EXISTS settings_report_due ON settings (value, chat_id) \
     WHERE key = 'report_at' AND value <> ''",
    "CREATE INDEX IF NOT EXISTS settings_purge_due ON settings (value, chat_id) \
     WHERE key = 'auto_purge_at' AND value <> ''",
    "CREATE INDEX IF NOT EXISTS settings_join_channel ON settings (chat_id) \
     WHERE key = 'join_channel'",
    "CREATE INDEX IF NOT EXISTS settings_add_required ON settings (chat_id) \
     WHERE key = 'add_required'",
    "CREATE INDEX IF NOT EXISTS settings_gate_on ON settings (chat_id) WHERE key = 'gate_on'",
    "CREATE INDEX IF NOT EXISTS image_filters_uncalibrated ON image_filters (chat_id) \
     WHERE calibrated = FALSE",
    "CREATE INDEX IF NOT EXISTS counters_day_chat ON counters (day, chat_id) INCLUDE (today)",
    "CREATE INDEX IF NOT EXISTS moderation_cases_chat_recent ON moderation_cases (chat_id, id DESC)",
    "CREATE INDEX IF NOT EXISTS moderation_cases_chat_status_recent ON moderation_cases (chat_id, status, id DESC)",
    "CREATE INDEX IF NOT EXISTS moderation_cases_chat_subject_recent ON moderation_cases (chat_id, subject_user_id, id DESC) WHERE subject_user_id IS NOT NULL",
    "CREATE INDEX IF NOT EXISTS moderation_cases_expiry ON moderation_cases (created_at, id)",
    "CREATE UNIQUE INDEX IF NOT EXISTS moderation_cases_workflow_key
     ON moderation_cases (chat_id, workflow_key) WHERE workflow_key IS NOT NULL",
    "CREATE INDEX IF NOT EXISTS moderation_case_events_chat_case ON moderation_case_events (chat_id, case_id, id)",
    "DROP INDEX IF EXISTS pending_warn_actions_due",
    "DROP INDEX IF EXISTS pending_warn_actions_fresh_due",
    "DROP INDEX IF EXISTS pending_warn_actions_retry_due",
    "CREATE INDEX IF NOT EXISTS pending_warn_actions_fresh_due
     ON pending_warn_actions (claimed_until, created_at, chat_id, user_id)
     WHERE attempts = 0 AND NOT awaiting_rejoin",
    "CREATE INDEX IF NOT EXISTS pending_warn_actions_retry_due
     ON pending_warn_actions (claimed_until, attempts, created_at, chat_id, user_id)
     WHERE attempts > 0 AND NOT awaiting_rejoin",
];

pub type Idle = (Vec<(i64, String, u64)>, u64);

type BumpedRow = (i64, i64, String, i64, i64);

type ImageFilterTuple = (String, Vec<u8>, f32, f32, i32, bool, i32, bool);

pub struct ImageFilterRow {
    pub name: String,
    pub vector: Vec<u8>,
    pub scale: f32,
    pub cut: f32,
    pub rate: u32,
    pub live: bool,
    pub samples: u32,
    pub calibrated: bool,
}

pub struct ImageFilterWrite<'a> {
    pub name: &'a str,
    pub vector: &'a [u8],
    pub scale: f32,
    pub cut: f32,
    pub rate: u32,
    pub live: bool,
    pub samples: u32,
    pub calibrated: bool,
}

pub struct Counter {
    pub user: i64,
    pub name: String,
    pub count: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Bump {
    pub chat: i64,
    pub user: i64,
    pub name: String,
    pub added: u64,
}

pub struct Bumped {
    pub chat: i64,
    pub user: i64,
    pub name: String,
    pub total: u64,
    pub awarded: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarningPenalty {
    Ban,
    Mute,
}

impl WarningPenalty {
    fn as_db(self) -> &'static str {
        match self {
            Self::Ban => "ban",
            Self::Mute => "mute",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "ban" => Ok(Self::Ban),
            "mute" => Ok(Self::Mute),
            _ => Err(sqlx::Error::Protocol(format!(
                "invalid pending warning penalty {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WarningIncrement {
    pub count: u32,
    pub pending: Option<WarningPenalty>,
    pub added: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingWarningAction {
    pub chat: i64,
    pub user: i64,
    pub penalty: WarningPenalty,
    pub attempts: u32,
    pub awaiting_rejoin: bool,
    generation: i64,
    version: i64,
    lease_token: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarningQueueClass {
    Fresh,
    Retry,
}

type PendingWarningTuple = (i64, i64, String, i32, bool, i64, i64, i64);

fn decode_pending_warning(row: PendingWarningTuple) -> Result<PendingWarningAction> {
    Ok(PendingWarningAction {
        chat: row.0,
        user: row.1,
        penalty: WarningPenalty::from_db(&row.2)?,
        attempts: u32::try_from(row.3)
            .map_err(|_| sqlx::Error::Protocol("negative warning attempt count".to_owned()))?,
        awaiting_rejoin: row.4,
        generation: row.5,
        version: row.6,
        lease_token: row.7,
    })
}

fn bounded_warning_error(reason: &str) -> String {
    const MAX_BYTES: usize = 1_024;
    let mut kept = String::with_capacity(reason.len().min(MAX_BYTES));
    for character in reason.chars() {
        if kept.len() + character.len_utf8() > MAX_BYTES {
            break;
        }
        kept.push(character);
    }
    kept
}

pub(super) fn durable_member_rejoin_key(chat: i64, user: i64) -> i64 {
    let mixed = u64::from_ne_bytes(chat.to_ne_bytes()).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ u64::from_ne_bytes(user.to_ne_bytes())
            .rotate_left(29)
            .wrapping_mul(0xbf58_476d_1ce4_e5b9);
    i64::from_ne_bytes(mixed.to_ne_bytes())
}

async fn lock_durable_member_rejoin(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    chat: i64,
    user: i64,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(durable_member_rejoin_key(chat, user))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

impl PendingWarningAction {
    pub fn workflow_key(&self) -> String {
        format!("warning-action:{}:{}", self.user, self.generation)
    }
}

#[derive(Default)]
pub struct Fleet {
    pub chats: u64,
    pub configured: u64,
    pub members: u64,
    pub active_today: u64,
    pub messages_today: u64,
    pub messages_total: u64,
}

#[derive(Default)]
pub struct Card {
    pub today: u64,
    pub total: u64,
    pub adds: u64,
    pub place: Option<u64>,
}

#[derive(Clone, Copy)]
pub enum Period {
    Total,
    Today,
    Week,
    Month,
    Adds,
}

impl Period {
    fn count(self) -> &'static str {
        match self {
            Period::Total => "total",
            Period::Today => "today",
            Period::Week => "week",
            Period::Month => "month",
            Period::Adds => "adds",
        }
    }

    fn stamp(self) -> Option<&'static str> {
        match self {
            Period::Today => Some("day"),
            Period::Week => Some("week_at"),
            Period::Month => Some("month_at"),
            Period::Total | Period::Adds => None,
        }
    }
}

pub struct ChatSettings<'a>(pub(crate) Option<&'a HashMap<String, String>>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorruptSetting {
    key: String,
    value: String,
    expected: String,
}

impl CorruptSetting {
    fn new(key: &str, value: &str, expected: impl Into<String>) -> Self {
        Self {
            key: key.to_owned(),
            value: value.to_owned(),
            expected: expected.into(),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Display for CorruptSetting {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "setting {:?} contains {:?}; expected {}",
            self.key, self.value, self.expected
        )
    }
}

impl std::error::Error for CorruptSetting {}

impl ChatSettings<'_> {
    pub fn is_locked(&self, key: &str) -> bool {
        self.0
            .and_then(|map| map.get(key))
            .is_some_and(|value| value.is_empty())
    }

    pub fn value(&self, key: &str) -> Option<&str> {
        self.0
            .and_then(|map| map.get(key))
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    pub fn parsed<T: std::str::FromStr>(
        &self,
        key: &str,
    ) -> std::result::Result<Option<T>, CorruptSetting> {
        let Some(value) = self.0.and_then(|map| map.get(key)) else {
            return Ok(None);
        };
        if value.is_empty() {
            return Err(CorruptSetting::new(key, value, "a non-empty typed value"));
        }
        value
            .parse()
            .map(Some)
            .map_err(|_| CorruptSetting::new(key, value, "a value of the requested type"))
    }

    pub fn number_checked(
        &self,
        key: &str,
        default: u32,
        range: (u32, u32),
    ) -> std::result::Result<u32, CorruptSetting> {
        debug_assert!((range.0..=range.1).contains(&default));
        let Some(number) = self.parsed::<u32>(key)? else {
            return Ok(default);
        };
        if !(range.0..=range.1).contains(&number) {
            let value = self
                .0
                .and_then(|map| map.get(key))
                .map_or("", String::as_str);
            return Err(CorruptSetting::new(
                key,
                value,
                format!("an integer in {}..={}", range.0, range.1),
            ));
        }
        Ok(number)
    }

    pub fn number(&self, key: &str, default: u32, range: (u32, u32)) -> u32 {
        assert!(
            VALIDATED_SETTING_KEYS.contains(&key),
            "infallible numeric getter must enroll its key in persisted validation"
        );
        self.number_checked(key, default, range)
            .expect("settings mirror contains only validated numeric policy")
    }
}

pub struct Settings {
    pool: PgPool,

    _process_lock: tokio::sync::Mutex<Option<sqlx::pool::PoolConnection<sqlx::Postgres>>>,
    ownership: ownership::Ownership,
    stats_directory: Option<std::path::PathBuf>,

    cache: RwLock<HashMap<i64, HashMap<String, String>>>,

    index: RwLock<HashMap<i64, ChatIndex>>,

    durable_chats: RwLock<HashMap<i64, i64>>,

    max_chats: Option<usize>,
    max_rows: Option<usize>,
    max_bytes: Option<usize>,
    max_counter_rows: Option<i64>,
    max_tally_rows: Option<i64>,
    max_note_rows: Option<i64>,
    max_pending_captcha_rows: Option<i64>,
    setting_rows: AtomicUsize,
    setting_bytes: AtomicUsize,

    capacity_write: tokio::sync::Mutex<()>,

    write_slots: Box<[std::sync::Arc<tokio::sync::Mutex<()>>]>,
}

const WRITE_SLOTS: usize = 1024;
pub const MAX_SETTINGS_ROWS_PER_CHAT: usize = 512;
pub const MAX_SETTING_KEY_BYTES: usize = 512;
pub const MAX_SETTING_VALUE_BYTES: usize = 16 * 1024;

const VALIDATED_SETTING_KEYS: &[&str] = &[
    "strict_limit",
    "strict_time",
    "strict_action",
    "captcha_timeout",
    "captcha_choices",
    "captcha_action",
    "warn_limit",
    "warn_action",
    "add_required",
    "gate_every",
    "gate_ttl",
    "pin_kept",
    "owner",
    "hash",
    "night",
    "night_state",
    "glock_until",
    "report_at",
    "report_day",
    "auto_purge_at",
    "auto_purge_count",
    "auto_purge_day",
    "cln_checked_slot",
    "raid_limit",
    "raid_window",
    "raid_time",
    "flood_limit",
    "flood_window",
    "cq_lim",
    "tmed_min",
    "trade_lim",
];

fn parse_bounded_setting(
    key: &str,
    value: &str,
    minimum: u64,
    maximum: u64,
) -> std::result::Result<(), CorruptSetting> {
    let parsed = value.parse::<u64>().map_err(|_| {
        CorruptSetting::new(key, value, format!("an integer in {minimum}..={maximum}"))
    })?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(CorruptSetting::new(
            key,
            value,
            format!("an integer in {minimum}..={maximum}"),
        ));
    }
    Ok(())
}

fn validate_persisted_setting(key: &str, value: &str) -> std::result::Result<(), CorruptSetting> {
    if key.starts_with("response:") {
        return crate::response::validate_setting(key, value)
            .map_err(|expected| CorruptSetting::new(key, value, expected));
    }
    match key {
        "strict_limit" => parse_bounded_setting(key, value, 1, 20),
        "strict_time" => parse_bounded_setting(key, value, 0, 10_080),
        "strict_action" if matches!(value, "ban" | "mute") => Ok(()),
        "strict_action" => Err(CorruptSetting::new(key, value, "ban or mute")),
        "captcha_timeout" => parse_bounded_setting(key, value, 30, 900),
        "captcha_choices" => parse_bounded_setting(key, value, 2, 6),
        "captcha_action" if matches!(value, "kick" | "mute") => Ok(()),
        "captcha_action" => Err(CorruptSetting::new(key, value, "kick or mute")),
        "warn_limit" => parse_bounded_setting(key, value, 1, 100),
        "warn_action" if matches!(value, "ban" | "mute") => Ok(()),
        "warn_action" => Err(CorruptSetting::new(key, value, "ban or mute")),
        "add_required" => parse_bounded_setting(key, value, 0, 1_000),
        "gate_every" | "gate_ttl" => parse_bounded_setting(key, value, 0, 3_600),
        "report_at" | "auto_purge_at" => parse_bounded_setting(key, value, 0, 1_439),
        "auto_purge_count" => parse_bounded_setting(key, value, 0, 100_000),
        "report_day" | "auto_purge_day" => value
            .parse::<u64>()
            .map(|_| ())
            .map_err(|_| CorruptSetting::new(key, value, "a non-negative day number")),
        "cln_checked_slot" => value
            .parse::<u64>()
            .map(|_| ())
            .map_err(|_| CorruptSetting::new(key, value, "a non-negative schedule slot")),
        "raid_limit" => parse_bounded_setting(key, value, 2, 200),
        "raid_window" => parse_bounded_setting(key, value, 5, 600),
        "raid_time" => parse_bounded_setting(key, value, 1, 10_080),
        "flood_limit" => parse_bounded_setting(key, value, 2, 50),
        "flood_window" => parse_bounded_setting(key, value, 2, 120),
        "cq_lim" => parse_bounded_setting(key, value, 5, 60),
        "tmed_min" => parse_bounded_setting(key, value, 1, 1_440),
        "trade_lim" => parse_bounded_setting(key, value, 30, 60),
        "pin_kept" => value
            .parse::<i32>()
            .ok()
            .filter(|message| *message > 0)
            .map(|_| ())
            .ok_or_else(|| CorruptSetting::new(key, value, "a positive Telegram message id")),
        "owner" => value
            .parse::<i64>()
            .ok()
            .filter(|user| *user > 0)
            .map(|_| ())
            .ok_or_else(|| CorruptSetting::new(key, value, "a positive Telegram user id")),
        "hash" => value
            .parse::<i64>()
            .map(|_| ())
            .map_err(|_| CorruptSetting::new(key, value, "a signed Telegram access hash")),
        "glock_until" => value
            .parse::<i64>()
            .ok()
            .filter(|until| *until > 0)
            .map(|_| ())
            .ok_or_else(|| CorruptSetting::new(key, value, "a positive Unix deadline")),
        "night_state" if matches!(value, "on" | "off" | "pending_on" | "pending_off") => Ok(()),
        "night_state" => Err(CorruptSetting::new(
            key,
            value,
            "on, off, pending_on, or pending_off",
        )),
        "night" => {
            let (from, to) = value.split_once('|').ok_or_else(|| {
                CorruptSetting::new(key, value, "two distinct minutes as FROM|TO")
            })?;
            let from = from
                .parse::<u16>()
                .map_err(|_| CorruptSetting::new(key, value, "two distinct minutes as FROM|TO"))?;
            let to = to
                .parse::<u16>()
                .map_err(|_| CorruptSetting::new(key, value, "two distinct minutes as FROM|TO"))?;
            if from > 1_439 || to > 1_439 || from == to {
                return Err(CorruptSetting::new(
                    key,
                    value,
                    "two distinct minutes as FROM|TO",
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingMutation<'a> {
    Put {
        key: &'a str,
        value: &'a str,
    },
    PutIfEmpty {
        key: &'a str,
        value: &'a str,
        condition_key: &'a str,
    },
    Delete {
        key: &'a str,
    },
}

impl<'a> SettingMutation<'a> {
    fn key(self) -> &'a str {
        match self {
            Self::Put { key, .. } | Self::PutIfEmpty { key, .. } | Self::Delete { key } => key,
        }
    }
}

struct SettingChange {
    key: String,
    next: Option<String>,
}

#[derive(Debug)]
pub enum SettingsWriteError {
    UncertainState,
    KeyTooLarge,
    ValueTooLarge,
    DuplicateKey,
    InvalidValue(CorruptSetting),
    CapacityReached(&'static str),
    Database(sqlx::Error),
    CommitUncertain(sqlx::Error),
}

impl SettingsWriteError {
    pub fn commit_outcome_unknown(&self) -> bool {
        matches!(self, Self::CommitUncertain(_))
    }
}

#[derive(Debug)]
pub enum NoteWriteError {
    ValueTooLarge,
    CapacityReached(&'static str),
    Database(sqlx::Error),
}

impl std::fmt::Display for NoteWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ValueTooLarge => write!(
                formatter,
                "note value exceeds {MAX_SETTING_VALUE_BYTES} bytes"
            ),
            Self::CapacityReached(scope) => write!(formatter, "{scope} note capacity reached"),
            Self::Database(error) => write!(formatter, "note database write failed: {error}"),
        }
    }
}

impl std::error::Error for NoteWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::ValueTooLarge | Self::CapacityReached(_) => None,
        }
    }
}

impl std::fmt::Display for SettingsWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UncertainState => write!(
                formatter,
                "settings mirror is no longer authoritative; process restart required"
            ),
            Self::KeyTooLarge => write!(
                formatter,
                "setting key exceeds {MAX_SETTING_KEY_BYTES} bytes"
            ),
            Self::ValueTooLarge => {
                write!(
                    formatter,
                    "setting value exceeds {MAX_SETTING_VALUE_BYTES} bytes"
                )
            }
            Self::DuplicateKey => write!(formatter, "settings batch contains a duplicate key"),
            Self::InvalidValue(error) => write!(formatter, "{error}"),
            Self::CapacityReached(limit) => write!(formatter, "settings {limit} capacity reached"),
            Self::Database(error) => write!(formatter, "settings database write failed: {error}"),
            Self::CommitUncertain(error) => {
                write!(formatter, "settings commit outcome is uncertain: {error}")
            }
        }
    }
}

impl std::error::Error for SettingsWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidValue(error) => Some(error),
            Self::Database(error) | Self::CommitUncertain(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum SettingsImportError {
    Read {
        path: String,
        source: std::io::Error,
    },
    Malformed {
        line: usize,
        reason: &'static str,
    },
    InvalidValue {
        line: usize,
        source: CorruptSetting,
    },
    Write {
        line: usize,
        source: SettingsWriteError,
    },
    Rename {
        from: String,
        to: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for SettingsImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => write!(formatter, "could not read {path}: {source}"),
            Self::Malformed { line, reason } => {
                write!(formatter, "malformed settings import line {line}: {reason}")
            }
            Self::InvalidValue { line, source } => {
                write!(formatter, "invalid setting on import line {line}: {source}")
            }
            Self::Write { line, source } => {
                write!(
                    formatter,
                    "could not import setting on line {line}: {source}"
                )
            }
            Self::Rename { from, to, source } => {
                write!(
                    formatter,
                    "could not rename imported file {from} to {to}: {source}"
                )
            }
        }
    }
}

impl std::error::Error for SettingsImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Rename { source, .. } => Some(source),
            Self::Write { source, .. } => Some(source),
            Self::InvalidValue { source, .. } => Some(source),
            Self::Malformed { .. } => None,
        }
    }
}

struct MirrorCommitGuard {
    alive: std::sync::Arc<AtomicBool>,
    lost: std::sync::Arc<tokio::sync::Notify>,
    armed: bool,
}

impl MirrorCommitGuard {
    fn new(ownership: &ownership::Ownership) -> Self {
        Self {
            alive: std::sync::Arc::clone(&ownership.alive),
            lost: std::sync::Arc::clone(&ownership.lost),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for MirrorCommitGuard {
    fn drop(&mut self) {
        if self.armed {
            self.alive.store(false, Ordering::Release);
            self.lost.notify_waiters();
            log::error!(
                "settings write ended with an unknowable commit outcome; closing the request gate"
            );
        }
    }
}
const MAX_STARTED_USERS: i64 = 5_000_000;
const DEFAULT_MAX_SETTINGS_ROWS: usize = 5_000_000;
const MIN_MAX_SETTINGS_ROWS: usize = 100_000;
const ABSOLUTE_MAX_SETTINGS_ROWS: usize = 50_000_000;
const DEFAULT_MAX_SETTINGS_BYTES: usize = 512 * 1024 * 1024;
const MIN_MAX_SETTINGS_BYTES: usize = 16 * 1024 * 1024;
const ABSOLUTE_MAX_SETTINGS_BYTES: usize = 8 * 1024 * 1024 * 1024;
const DEFAULT_MAX_COUNTER_ROWS: i64 = 5_000_000;
const MIN_MAX_COUNTER_ROWS: i64 = 100_000;
const ABSOLUTE_MAX_COUNTER_ROWS: i64 = 50_000_000;
const DEFAULT_MAX_TALLY_ROWS: i64 = 1_000_000;
const MIN_MAX_TALLY_ROWS: i64 = 10_000;
const ABSOLUTE_MAX_TALLY_ROWS: i64 = 10_000_000;
const DEFAULT_MAX_NOTE_ROWS: i64 = 5_000_000;
const MIN_MAX_NOTE_ROWS: i64 = 100_000;
const ABSOLUTE_MAX_NOTE_ROWS: i64 = 50_000_000;
const DEFAULT_MAX_PENDING_CAPTCHA_ROWS: i64 = 500_000;
const MIN_MAX_PENDING_CAPTCHA_ROWS: i64 = 1_000;
const ABSOLUTE_MAX_PENDING_CAPTCHA_ROWS: i64 = 5_000_000;

fn indexed_slot(key: &str) -> Option<(usize, &str)> {
    INDEXED
        .iter()
        .enumerate()
        .find_map(|(slot, prefix)| key.strip_prefix(prefix).map(|rest| (slot, rest)))
}

fn setting_size(key: &str, value: &str) -> usize {
    key.len().saturating_add(value.len())
}

fn saturating_postgres_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn configured_usize(name: &str, default: usize, minimum: usize, maximum: usize) -> Result<usize> {
    match std::env::var(name) {
        Ok(value) => parse_bounded_usize(name, Some(&value), default, minimum, maximum),
        Err(std::env::VarError::NotPresent) => {
            parse_bounded_usize(name, None, default, minimum, maximum)
        }
        Err(std::env::VarError::NotUnicode(_)) => Err(sqlx::Error::Protocol(format!(
            "{name} must contain valid Unicode"
        ))),
    }
}

fn parse_bounded_usize(
    name: &str,
    configured: Option<&str>,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize> {
    let value = match configured {
        Some(value) => value.parse::<usize>().map_err(|_| {
            sqlx::Error::Protocol(format!(
                "{name} must be an integer from {minimum} to {maximum}"
            ))
        })?,
        None => default,
    };
    if !(minimum..=maximum).contains(&value) {
        return Err(sqlx::Error::Protocol(format!(
            "{name}={value} is outside {minimum}..={maximum}"
        )));
    }
    Ok(value)
}

fn configured_i64(name: &str, default: i64, minimum: i64, maximum: i64) -> Result<i64> {
    match std::env::var(name) {
        Ok(value) => parse_bounded_i64(name, Some(&value), default, minimum, maximum),
        Err(std::env::VarError::NotPresent) => {
            parse_bounded_i64(name, None, default, minimum, maximum)
        }
        Err(std::env::VarError::NotUnicode(_)) => Err(sqlx::Error::Protocol(format!(
            "{name} must contain valid Unicode"
        ))),
    }
}

fn parse_bounded_i64(
    name: &str,
    configured: Option<&str>,
    default: i64,
    minimum: i64,
    maximum: i64,
) -> Result<i64> {
    let value = match configured {
        Some(value) => value.parse::<i64>().map_err(|_| {
            sqlx::Error::Protocol(format!(
                "{name} must be an integer from {minimum} to {maximum}"
            ))
        })?,
        None => default,
    };
    if !(minimum..=maximum).contains(&value) {
        return Err(sqlx::Error::Protocol(format!(
            "{name}={value} is outside {minimum}..={maximum}"
        )));
    }
    Ok(value)
}

impl Settings {
    async fn validate_persisted_settings(connection: &mut sqlx::PgConnection) -> Result<()> {
        const PAGE: i64 = 10_000;
        let (mut at_chat, mut at_key) = (i64::MIN, String::new());
        loop {
            let rows: Vec<(i64, String, String)> = sqlx::query_as(
                "SELECT chat_id, key, value FROM settings
                 WHERE (key = ANY($1::TEXT[]) OR key LIKE 'response:%')
                   AND (chat_id, key) > ($2, $3)
                 ORDER BY chat_id, key LIMIT $4",
            )
            .bind(VALIDATED_SETTING_KEYS)
            .bind(at_chat)
            .bind(&at_key)
            .bind(PAGE)
            .fetch_all(&mut *connection)
            .await?;
            if rows.is_empty() {
                break;
            }
            for (chat, key, value) in &rows {
                validate_persisted_setting(key, value).map_err(|error| {
                    sqlx::Error::Protocol(format!(
                        "refusing to start with corrupt setting for chat {chat}: {error}"
                    ))
                })?;
            }
            let Some((chat, key, _)) = rows.last() else {
                break;
            };
            at_chat = *chat;
            at_key.clone_from(key);
        }
        Ok(())
    }

    pub async fn connect(url: &str) -> Result<Self> {
        Self::connect_with_chat_limit(url, None).await
    }

    pub async fn connect_with_chat_limit(url: &str, max_chats: Option<usize>) -> Result<Self> {
        Self::connect_inner(url, max_chats, false).await
    }

    pub async fn connect_with_chat_limit_and_process_lock(
        url: &str,
        max_chats: Option<usize>,
    ) -> Result<Self> {
        Self::connect_inner(url, max_chats, true).await
    }

    async fn connect_inner(
        url: &str,
        max_chats: Option<usize>,
        lock_process: bool,
    ) -> Result<Self> {
        let max_rows = max_chats
            .map(|_| {
                configured_usize(
                    "MAX_SHARD_SETTINGS_ROWS",
                    DEFAULT_MAX_SETTINGS_ROWS,
                    MIN_MAX_SETTINGS_ROWS,
                    ABSOLUTE_MAX_SETTINGS_ROWS,
                )
            })
            .transpose()?;
        let max_bytes = max_chats
            .map(|_| {
                configured_usize(
                    "MAX_SHARD_SETTINGS_BYTES",
                    DEFAULT_MAX_SETTINGS_BYTES,
                    MIN_MAX_SETTINGS_BYTES,
                    ABSOLUTE_MAX_SETTINGS_BYTES,
                )
            })
            .transpose()?;
        let max_counter_rows = max_chats
            .map(|_| {
                configured_i64(
                    "MAX_SHARD_COUNTER_ROWS",
                    DEFAULT_MAX_COUNTER_ROWS,
                    MIN_MAX_COUNTER_ROWS,
                    ABSOLUTE_MAX_COUNTER_ROWS,
                )
            })
            .transpose()?;
        let max_tally_rows = max_chats
            .map(|_| {
                configured_i64(
                    "MAX_SHARD_TALLY_ROWS",
                    DEFAULT_MAX_TALLY_ROWS,
                    MIN_MAX_TALLY_ROWS,
                    ABSOLUTE_MAX_TALLY_ROWS,
                )
            })
            .transpose()?;
        let max_note_rows = max_chats
            .map(|_| {
                configured_i64(
                    "MAX_SHARD_NOTE_ROWS",
                    DEFAULT_MAX_NOTE_ROWS,
                    MIN_MAX_NOTE_ROWS,
                    ABSOLUTE_MAX_NOTE_ROWS,
                )
            })
            .transpose()?;
        let max_pending_captcha_rows = max_chats
            .map(|_| {
                configured_i64(
                    "MAX_SHARD_PENDING_CAPTCHAS",
                    DEFAULT_MAX_PENDING_CAPTCHA_ROWS,
                    MIN_MAX_PENDING_CAPTCHA_ROWS,
                    ABSOLUTE_MAX_PENDING_CAPTCHA_ROWS,
                )
            })
            .transpose()?;
        let size = u32::try_from(configured_usize("DB_POOL", 8, 2, 64)?).map_err(|_| {
            sqlx::Error::Protocol("DB_POOL does not fit the pool size type".to_owned())
        })?;
        let statement_ms = configured_usize("DB_STATEMENT_TIMEOUT_MS", 5_000, 100, 120_000)?;
        let lock_ms = configured_usize("DB_LOCK_TIMEOUT_MS", 1_000, 50, 30_000)?.min(statement_ms);
        let ownership = ownership::Ownership::default();
        let pool_epoch = ownership.epoch.clone();
        let pool_alive = ownership.alive.clone();
        let connect_epoch = ownership.epoch.clone();
        let connect_alive = ownership.alive.clone();
        let pool = PgPoolOptions::new()
            .max_connections(size)
            .min_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .idle_timeout(std::time::Duration::from_secs(600))
            .before_acquire(move |connection, _| {
                let epoch = pool_epoch.load(Ordering::Acquire);
                let alive = pool_alive.load(Ordering::Acquire);
                Box::pin(async move {
                    if !alive { return Err(sqlx::Error::Protocol("database ownership lost".into())); }
                    sqlx::query("SELECT set_config('groupbot.owner_epoch', $1, false)")
                        .bind(epoch.to_string()).execute(connection).await?;
                    Ok(true)
                })
            })
            .after_connect(move |connection, _| {
                let epoch = connect_epoch.load(Ordering::Acquire);
                let alive = connect_alive.load(Ordering::Acquire);
                Box::pin(async move {
                if !alive { return Err(sqlx::Error::Protocol("database ownership lost".into())); }
                sqlx::query("SELECT set_config('statement_timeout', $1, false), set_config('lock_timeout', $2, false), set_config('groupbot.owner_epoch', $3, false)")
                    .bind(format!("{statement_ms}ms"))
                    .bind(format!("{lock_ms}ms"))
                    .bind(epoch.to_string())
                    .execute(connection).await?;
                Ok(())
            })})
            .connect(url)
            .await?;
        let process_lock = if lock_process {
            let mut lock = pool.acquire().await?;
            let acquired: bool = sqlx::query_scalar(
                "SELECT pg_try_advisory_lock(hashtextextended('groupbot:process', 0))",
            )
            .fetch_one(&mut *lock)
            .await?;
            if !acquired {
                return Err(sqlx::Error::Protocol(
                    "another groupbot process already owns this database".to_owned(),
                ));
            }
            Some(lock)
        } else {
            None
        };
        const SCHEMA_LOCK: i64 = 0x67_72_6f_75_70;
        let mut conn = pool.acquire().await?;
        let migration_ms = configured_usize("DB_MIGRATION_TIMEOUT_MS", 120_000, 1_000, 3_600_000)?;
        let migration_lock_ms =
            configured_usize("DB_MIGRATION_LOCK_TIMEOUT_MS", 30_000, 100, 120_000)?
                .min(migration_ms);
        sqlx::query("SELECT set_config('statement_timeout', $1, false), set_config('lock_timeout', $2, false)")
            .bind(format!("{migration_ms}ms"))
            .bind(format!("{migration_lock_ms}ms"))
            .execute(&mut *conn).await?;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(SCHEMA_LOCK)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS settings (
                chat_id BIGINT NOT NULL,
                key     TEXT   NOT NULL,
                value   TEXT   NOT NULL DEFAULT '',
                PRIMARY KEY (chat_id, key)
            )",
        )
        .execute(&mut *conn)
        .await?;
        Self::validate_persisted_settings(&mut conn).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS durable_chats (
                chat_id BIGINT PRIMARY KEY CHECK (
                    chat_id BETWEEN -999999999999 AND -1
                    OR chat_id BETWEEN -1997852516352 AND -1000000000001
                    OR chat_id BETWEEN -4000000000000 AND -2002147483649
                ),
                access_hash BIGINT NOT NULL DEFAULT 0,
                admitted_at BIGINT NOT NULL DEFAULT 0
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS shard_migration_receipts (
                chat_id BIGINT PRIMARY KEY REFERENCES durable_chats(chat_id) ON DELETE CASCADE,
                completed_at BIGINT NOT NULL CHECK (completed_at >= 0),
                fingerprint TEXT NOT NULL CHECK (fingerprint <> '')
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS counters (
                chat_id  BIGINT NOT NULL,
                user_id  BIGINT NOT NULL,
                name     TEXT   NOT NULL DEFAULT '',
                total    BIGINT NOT NULL DEFAULT 0,
                today    BIGINT NOT NULL DEFAULT 0,
                day      BIGINT NOT NULL DEFAULT 0,
                week     BIGINT NOT NULL DEFAULT 0,
                week_at  BIGINT NOT NULL DEFAULT 0,
                month    BIGINT NOT NULL DEFAULT 0,
                month_at BIGINT NOT NULL DEFAULT 0,
                seen     BIGINT NOT NULL DEFAULT 0,
                adds     BIGINT NOT NULL DEFAULT 0,
                awarded  BIGINT NOT NULL DEFAULT 0,
                warns    BIGINT NOT NULL DEFAULT 0,
                strikes  BIGINT NOT NULL DEFAULT 0,
                struck   BIGINT NOT NULL DEFAULT 0,
                PRIMARY KEY (chat_id, user_id)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS durable_counts (
                id           SMALLINT PRIMARY KEY CHECK (id = 0),
                counter_rows BIGINT NOT NULL DEFAULT 0,
                tally_rows   BIGINT NOT NULL DEFAULT 0,
                note_rows    BIGINT NOT NULL DEFAULT 0,
                captcha_rows BIGINT NOT NULL DEFAULT 0
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "ALTER TABLE durable_counts
             ADD COLUMN IF NOT EXISTS captcha_rows BIGINT NOT NULL DEFAULT 0,
             ADD COLUMN IF NOT EXISTS tally_rows BIGINT NOT NULL DEFAULT 0",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS started_users (
                user_id         BIGINT PRIMARY KEY,
                access_hash     BIGINT NOT NULL DEFAULT 0,
                last_started_at BIGINT NOT NULL DEFAULT 0
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "ALTER TABLE started_users
             ADD COLUMN IF NOT EXISTS last_started_at BIGINT NOT NULL DEFAULT 0",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS started_users_meta (
                id    SMALLINT PRIMARY KEY CHECK (id = 0),
                total BIGINT NOT NULL DEFAULT 0
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO started_users_meta (id, total)
             SELECT 0, count(*) FROM started_users
             ON CONFLICT (id) DO NOTHING",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS notes (
                chat_id BIGINT NOT NULL,
                user_id BIGINT NOT NULL,
                value   TEXT   NOT NULL,
                PRIMARY KEY (chat_id, user_id)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tallies (
                chat_id BIGINT NOT NULL,
                counter TEXT   NOT NULL,
                day     BIGINT NOT NULL DEFAULT 0,
                count   BIGINT NOT NULL DEFAULT 0,
                PRIMARY KEY (chat_id, counter)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_deletes (
                chat_id    BIGINT NOT NULL,
                message_id INT    NOT NULL,
                due_at     BIGINT NOT NULL,
                PRIMARY KEY (chat_id, message_id)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("CREATE SEQUENCE IF NOT EXISTS durable_work_token_seq")
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_captchas (
                chat_id       BIGINT NOT NULL,
                user_id       BIGINT NOT NULL,
                answer        INT NOT NULL CHECK (answer >= 0),
                source_message_id INT NOT NULL DEFAULT 0,
                message_id    INT,
                due_at        BIGINT NOT NULL,
                retry_at      BIGINT NOT NULL,
                failure_action TEXT NOT NULL CHECK (failure_action IN ('kick', 'mute')),
                state         TEXT NOT NULL CHECK (state IN ('arming', 'pending', 'passing', 'expiring')),
                attempts      INT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
                generation    BIGINT NOT NULL DEFAULT nextval('durable_work_token_seq'),
                version       BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
                lease_token   BIGINT,
                restriction_until INT,
                kick_until INT,
                quarantined_at BIGINT,
                terminal_reason TEXT,
                PRIMARY KEY (chat_id, user_id),
                CHECK (state = 'arming' OR message_id IS NOT NULL)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "ALTER TABLE pending_captchas
             ADD COLUMN IF NOT EXISTS generation BIGINT NOT NULL
                 DEFAULT nextval('durable_work_token_seq'),
             ADD COLUMN IF NOT EXISTS version BIGINT NOT NULL DEFAULT 1,
             ADD COLUMN IF NOT EXISTS lease_token BIGINT,
             ADD COLUMN IF NOT EXISTS source_message_id INT NOT NULL DEFAULT 0,
             ADD COLUMN IF NOT EXISTS restriction_until INT,
             ADD COLUMN IF NOT EXISTS kick_until INT,
             ADD COLUMN IF NOT EXISTS quarantined_at BIGINT,
             ADD COLUMN IF NOT EXISTS terminal_reason TEXT",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS moderation_cases (
                id              BIGSERIAL NOT NULL,
                chat_id         BIGINT NOT NULL,
                subject_user_id BIGINT,
                subject_name    TEXT NOT NULL DEFAULT '',
                source          TEXT NOT NULL,
                rule_key        TEXT NOT NULL,
                reason          TEXT NOT NULL,
                message_id      INT,
                media_kind      TEXT,
                evidence_text   TEXT,
                evidence_hash   BYTEA,
                primary_action  TEXT NOT NULL,
                action_until    BIGINT,
                status          TEXT NOT NULL CHECK (status IN ('open', 'resolved', 'reversed')),
                actor_id        BIGINT,
                actor_name      TEXT NOT NULL DEFAULT '',
                created_at      BIGINT NOT NULL,
                updated_at      BIGINT NOT NULL,
                workflow_key    TEXT,
                PRIMARY KEY (chat_id, id)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("ALTER TABLE moderation_cases ADD COLUMN IF NOT EXISTS workflow_key TEXT")
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS moderation_case_events (
                id         BIGSERIAL NOT NULL,
                chat_id    BIGINT NOT NULL,
                case_id    BIGINT NOT NULL,
                kind       TEXT NOT NULL,
                actor_id   BIGINT,
                actor_name TEXT NOT NULL DEFAULT '',
                action     TEXT,
                note       TEXT,
                created_at BIGINT NOT NULL,
                PRIMARY KEY (chat_id, id),
                FOREIGN KEY (chat_id, case_id)
                    REFERENCES moderation_cases(chat_id, id) ON DELETE CASCADE
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "DO $migration$
             BEGIN
               IF EXISTS (
                 SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'moderation_cases'::regclass
                   AND conname = 'moderation_cases_pkey'
                   AND pg_get_constraintdef(oid) = 'PRIMARY KEY (id)'
               ) THEN
                 ALTER TABLE moderation_case_events
                   ADD COLUMN IF NOT EXISTS chat_id BIGINT;
                 UPDATE moderation_case_events AS event
                 SET chat_id = case_row.chat_id
                 FROM moderation_cases AS case_row
                 WHERE event.case_id = case_row.id AND event.chat_id IS NULL;
                 ALTER TABLE moderation_case_events ALTER COLUMN chat_id SET NOT NULL;
                 ALTER TABLE moderation_case_events
                   DROP CONSTRAINT IF EXISTS moderation_case_events_case_id_fkey;
                 ALTER TABLE moderation_case_events
                   DROP CONSTRAINT IF EXISTS moderation_case_events_pkey;
                 ALTER TABLE moderation_cases
                   DROP CONSTRAINT moderation_cases_pkey;
                 ALTER TABLE moderation_cases
                   ADD CONSTRAINT moderation_cases_pkey PRIMARY KEY (chat_id, id);
                 ALTER TABLE moderation_case_events
                   ADD CONSTRAINT moderation_case_events_pkey PRIMARY KEY (chat_id, id);
                 ALTER TABLE moderation_case_events
                   ADD CONSTRAINT moderation_case_events_case_fkey
                   FOREIGN KEY (chat_id, case_id)
                   REFERENCES moderation_cases(chat_id, id) ON DELETE CASCADE;
               END IF;
             END
             $migration$",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_warn_actions (
                chat_id   BIGINT NOT NULL,
                user_id   BIGINT NOT NULL,
                penalty   TEXT NOT NULL CHECK (penalty IN ('ban', 'mute')),
                created_at BIGINT NOT NULL,
                claimed_until BIGINT NOT NULL DEFAULT 0,
                attempts INT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
                generation BIGINT NOT NULL DEFAULT nextval('durable_work_token_seq'),
                version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
                lease_token BIGINT,
                awaiting_rejoin BOOLEAN NOT NULL DEFAULT FALSE,
                last_error TEXT,
                PRIMARY KEY (chat_id, user_id)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "ALTER TABLE pending_warn_actions
             ADD COLUMN IF NOT EXISTS claimed_until BIGINT NOT NULL DEFAULT 0,
             ADD COLUMN IF NOT EXISTS attempts INT NOT NULL DEFAULT 0,
             ADD COLUMN IF NOT EXISTS generation BIGINT NOT NULL
                 DEFAULT nextval('durable_work_token_seq'),
             ADD COLUMN IF NOT EXISTS version BIGINT NOT NULL DEFAULT 1,
             ADD COLUMN IF NOT EXISTS lease_token BIGINT,
             ADD COLUMN IF NOT EXISTS awaiting_rejoin BOOLEAN NOT NULL DEFAULT FALSE,
             ADD COLUMN IF NOT EXISTS last_error TEXT",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS image_filters (
                chat_id BIGINT  NOT NULL,
                name    TEXT    NOT NULL,
                vec     BYTEA   NOT NULL,
                scale   REAL    NOT NULL,
                cut     REAL    NOT NULL,
                rate    INT     NOT NULL DEFAULT 1000,
                live    BOOLEAN NOT NULL DEFAULT FALSE,
                samples INT     NOT NULL DEFAULT 0,
                PRIMARY KEY (chat_id, name)
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS calibration (
                id    INT   PRIMARY KEY,
                vecs  BYTEA NOT NULL,
                scale REAL  NOT NULL,
                count INT   NOT NULL
            )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "ALTER TABLE image_filters ADD COLUMN IF NOT EXISTS              calibrated BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(&mut *conn)
        .await?;
        for column in ["warns", "strikes", "struck"] {
            sqlx::query(&format!(
                "ALTER TABLE counters ADD COLUMN IF NOT EXISTS {column} BIGINT NOT NULL DEFAULT 0"
            ))
            .execute(&mut *conn)
            .await?;
        }
        for index in INDEXES {
            sqlx::query(index).execute(&mut *conn).await?;
        }
        let started_total: i64 =
            sqlx::query_scalar("SELECT total FROM started_users_meta WHERE id = 0")
                .fetch_one(&mut *conn)
                .await?;
        if started_total > MAX_STARTED_USERS {
            let excess = started_total - MAX_STARTED_USERS;
            let deleted = sqlx::query(
                "WITH evict AS (
                     SELECT user_id FROM started_users
                     ORDER BY last_started_at, user_id
                     LIMIT $1
                 )
                 DELETE FROM started_users
                 WHERE user_id IN (SELECT user_id FROM evict)",
            )
            .bind(excess)
            .execute(&mut *conn)
            .await?
            .rows_affected();
            sqlx::query(
                "UPDATE started_users_meta
                 SET total = GREATEST(total - $1, 0)
                 WHERE id = 0",
            )
            .bind(saturating_postgres_i64(deleted))
            .execute(&mut *conn)
            .await?;
        }
        Self::validate_counter_rows(&mut conn).await?;
        Self::validate_tally_rows(&mut conn).await?;
        Self::validate_projected_counter_capacity(&mut conn, max_counter_rows).await?;
        Self::validate_projected_tally_capacity(&mut conn, max_tally_rows).await?;
        Self::migrate_counters(&mut conn).await?;
        Self::migrate_per_user(&mut conn).await?;
        Self::migrate_notes(&mut conn).await?;
        Self::migrate_tallies(&mut conn).await?;
        Self::migrate_wipe_split(&mut conn).await?;
        Self::validate_counter_rows(&mut conn).await?;
        Self::validate_tally_rows(&mut conn).await?;
        let (counter_rows, tally_rows, note_rows, captcha_rows): (i64, i64, i64, i64) =
            sqlx::query_as(
                "SELECT (SELECT count(*) FROM counters),
                    (SELECT count(*) FROM tallies),
                    (SELECT count(*) FROM notes),
                    (SELECT count(*) FROM pending_captchas)",
            )
            .fetch_one(&mut *conn)
            .await?;
        if let Some(max_counter_rows) = max_counter_rows
            && counter_rows > max_counter_rows
        {
            return Err(sqlx::Error::Protocol(format!(
                "database has {counter_rows} counter rows, above shard limit {max_counter_rows}"
            )));
        }
        let largest_counter_chat: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(row_count), 0) FROM (
                 SELECT count(*) AS row_count FROM counters GROUP BY chat_id
             ) AS per_chat",
        )
        .fetch_one(&mut *conn)
        .await?;
        if largest_counter_chat > MAX_COUNTER_ROWS_PER_CHAT {
            return Err(sqlx::Error::Protocol(format!(
                "database has a chat with {largest_counter_chat} counter rows, above per-chat limit {MAX_COUNTER_ROWS_PER_CHAT}"
            )));
        }
        if let Some(max_tally_rows) = max_tally_rows
            && tally_rows > max_tally_rows
        {
            return Err(sqlx::Error::Protocol(format!(
                "database has {tally_rows} tally rows, above shard limit {max_tally_rows}"
            )));
        }
        let largest_tally_chat: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(row_count), 0) FROM (
                 SELECT count(*) AS row_count FROM tallies GROUP BY chat_id
             ) AS per_chat",
        )
        .fetch_one(&mut *conn)
        .await?;
        if largest_tally_chat > MAX_TALLY_ROWS_PER_CHAT {
            return Err(sqlx::Error::Protocol(format!(
                "database has a chat with {largest_tally_chat} tally rows, above per-chat limit {MAX_TALLY_ROWS_PER_CHAT}"
            )));
        }
        if let Some(max_note_rows) = max_note_rows
            && note_rows > max_note_rows
        {
            return Err(sqlx::Error::Protocol(format!(
                "database has {note_rows} note rows, above shard limit {max_note_rows}"
            )));
        }
        if let Some(max_pending_captcha_rows) = max_pending_captcha_rows
            && captcha_rows > max_pending_captcha_rows
        {
            return Err(sqlx::Error::Protocol(format!(
                "database has {captcha_rows} pending captcha rows, above shard limit {max_pending_captcha_rows}"
            )));
        }
        let largest_captcha_chat: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(row_count), 0) FROM (
                 SELECT count(*) AS row_count FROM pending_captchas GROUP BY chat_id
             ) AS per_chat",
        )
        .fetch_one(&mut *conn)
        .await?;
        if largest_captcha_chat > captcha::MAX_PENDING_CAPTCHAS_PER_CHAT {
            return Err(sqlx::Error::Protocol(format!(
                "database has a chat with {largest_captcha_chat} pending captchas, above per-chat limit {}",
                captcha::MAX_PENDING_CAPTCHAS_PER_CHAT
            )));
        }
        sqlx::query(
            "INSERT INTO durable_counts (id, counter_rows, tally_rows, note_rows, captcha_rows)
             VALUES (0, $1, $2, $3, $4)
             ON CONFLICT (id) DO UPDATE SET
                 counter_rows = EXCLUDED.counter_rows,
                 tally_rows = EXCLUDED.tally_rows,
                 note_rows = EXCLUDED.note_rows,
                 captcha_rows = EXCLUDED.captcha_rows",
        )
        .bind(counter_rows)
        .bind(tally_rows)
        .bind(note_rows)
        .bind(captcha_rows)
        .execute(&mut *conn)
        .await?;
        Self::init_durable(&mut conn).await?;
        Self::init_default_rights(&mut conn).await?;
        Self::init_strict(&mut conn).await?;
        Self::validate_durable_chat_ids(&mut conn).await?;
        sqlx::query(
            "DO $constraint$
             BEGIN
               IF NOT EXISTS (SELECT 1 FROM pg_constraint
                              WHERE conrelid = 'durable_chats'::regclass
                                AND conname = 'durable_chats_chat_id_valid') THEN
                 ALTER TABLE durable_chats ADD CONSTRAINT durable_chats_chat_id_valid CHECK (
                   chat_id BETWEEN -999999999999 AND -1
                   OR chat_id BETWEEN -1997852516352 AND -1000000000001
                   OR chat_id BETWEEN -4000000000000 AND -2002147483649
                 );
               END IF;
             END $constraint$",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             SELECT chats.chat_id, COALESCE(hash.value::BIGINT, 0), 0
             FROM (
                 SELECT chat_id FROM settings WHERE chat_id <> 0
                 UNION SELECT chat_id FROM default_rights_state WHERE chat_id <> 0
                 UNION SELECT chat_id FROM pending_captchas WHERE chat_id <> 0
                 UNION SELECT chat_id FROM pending_warn_actions WHERE chat_id <> 0
                 UNION SELECT chat_id FROM pending_strict_actions WHERE chat_id <> 0
                 UNION SELECT chat_id FROM counters WHERE chat_id <> 0
                 UNION SELECT chat_id FROM notes WHERE chat_id <> 0
                 UNION SELECT chat_id FROM tallies WHERE chat_id <> 0
                 UNION SELECT chat_id FROM pending_deletes WHERE chat_id <> 0
                 UNION SELECT chat_id FROM pending_rank_awards WHERE chat_id <> 0
                 UNION SELECT chat_id FROM moderation_cases WHERE chat_id <> 0
                 UNION SELECT chat_id FROM image_filters WHERE chat_id <> 0
             ) AS chats
             LEFT JOIN settings AS hash
               ON hash.chat_id = chats.chat_id AND hash.key = 'hash'
                  AND hash.value ~ '^[+-]?[0-9]+$'
             ON CONFLICT (chat_id) DO UPDATE SET
                 access_hash = CASE WHEN EXCLUDED.access_hash <> 0
                                    THEN EXCLUDED.access_hash
                                    ELSE durable_chats.access_hash END",
        )
        .execute(&mut *conn)
        .await?;
        if let Some(max_chats) = max_chats {
            let admitted: i64 = sqlx::query_scalar("SELECT count(*) FROM durable_chats")
                .fetch_one(&mut *conn)
                .await?;
            let max_chats = i64::try_from(max_chats).map_err(|_| {
                sqlx::Error::Protocol("maximum chat count exceeds PostgreSQL bigint".to_owned())
            })?;
            if admitted > max_chats {
                return Err(sqlx::Error::Protocol(format!(
                    "database has {admitted} durably admitted chats, above shard limit {max_chats}"
                )));
            }
        }
        sqlx::query(
            "DO $constraint$
             BEGIN
               IF NOT EXISTS (SELECT 1 FROM pg_constraint
                              WHERE conrelid = 'counters'::regclass
                                AND conname = 'counters_identity_valid') THEN
                 ALTER TABLE counters ADD CONSTRAINT counters_identity_valid CHECK (
                   (chat_id BETWEEN -999999999999 AND -1
                    OR chat_id BETWEEN -1997852516352 AND -1000000000001
                    OR chat_id BETWEEN -4000000000000 AND -2002147483649)
                   AND user_id BETWEEN 1 AND 1099511627775
                 );
               END IF;
               IF NOT EXISTS (SELECT 1 FROM pg_constraint
                              WHERE conrelid = 'counters'::regclass
                                AND conname = 'counters_values_valid') THEN
                 ALTER TABLE counters ADD CONSTRAINT counters_values_valid
                   CHECK (total >= 0 AND today >= 0 AND day >= 0
                      AND week >= 0 AND week_at >= 0
                      AND month >= 0 AND month_at >= 0
                      AND seen >= 0 AND adds >= 0 AND awarded >= 0);
               END IF;
               IF NOT EXISTS (SELECT 1 FROM pg_constraint
                              WHERE conrelid = 'counters'::regclass
                                AND conname = 'counters_warns_valid') THEN
                 ALTER TABLE counters ADD CONSTRAINT counters_warns_valid
                   CHECK (warns BETWEEN 0 AND 4294967295);
               END IF;
               IF NOT EXISTS (SELECT 1 FROM pg_constraint
                              WHERE conrelid = 'counters'::regclass
                                AND conname = 'counters_strikes_valid') THEN
                 ALTER TABLE counters ADD CONSTRAINT counters_strikes_valid
                   CHECK (strikes BETWEEN 0 AND 4294967295 AND struck >= 0);
               END IF;
             END
             $constraint$",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("ALTER TABLE tallies DROP CONSTRAINT IF EXISTS tallies_values_valid")
            .execute(&mut *conn)
            .await?;
        sqlx::query(&format!(
            "ALTER TABLE tallies ADD CONSTRAINT tallies_values_valid CHECK (
               (chat_id BETWEEN -999999999999 AND -1
                OR chat_id BETWEEN -1997852516352 AND -1000000000001
                OR chat_id BETWEEN -4000000000000 AND -2002147483649)
               AND octet_length(counter) BETWEEN 1 AND {MAX_TALLY_COUNTER_BYTES}
               AND position(':' in counter) = 0 AND day >= 0 AND count >= 0
             )"
        ))
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "ALTER TABLE settings ADD COLUMN IF NOT EXISTS owner_chat_id BIGINT
               GENERATED ALWAYS AS (NULLIF(chat_id, 0)) STORED",
        )
        .execute(&mut *conn)
        .await?;
        for (table, column, constraint) in [
            ("settings", "owner_chat_id", "settings_chat_owner"),
            ("counters", "chat_id", "counters_chat_owner"),
            ("notes", "chat_id", "notes_chat_owner"),
            ("tallies", "chat_id", "tallies_chat_owner"),
            ("pending_deletes", "chat_id", "pending_deletes_chat_owner"),
            (
                "default_rights_state",
                "chat_id",
                "default_rights_chat_owner",
            ),
            ("pending_captchas", "chat_id", "pending_captchas_chat_owner"),
            ("moderation_cases", "chat_id", "moderation_cases_chat_owner"),
            (
                "moderation_case_events",
                "chat_id",
                "moderation_case_events_chat_owner",
            ),
            (
                "pending_warn_actions",
                "chat_id",
                "pending_warn_actions_chat_owner",
            ),
            (
                "pending_strict_actions",
                "chat_id",
                "pending_strict_actions_chat_owner",
            ),
            (
                "pending_rank_awards",
                "chat_id",
                "pending_rank_awards_chat_owner",
            ),
            ("image_filters", "chat_id", "image_filters_chat_owner"),
        ] {
            sqlx::query(&format!(
                "DO $constraint$
                 BEGIN
                   IF NOT EXISTS (
                     SELECT 1 FROM pg_constraint
                     WHERE conrelid = '{table}'::regclass AND conname = '{constraint}'
                   ) THEN
                      ALTER TABLE {table} ADD CONSTRAINT {constraint}
                        FOREIGN KEY ({column}) REFERENCES durable_chats(chat_id);
                   END IF;
                 END
                 $constraint$"
            ))
            .execute(&mut *conn)
            .await?;
        }
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(SCHEMA_LOCK)
            .execute(&mut *conn)
            .await?;
        if lock_process {
            let epoch = Self::install_fencing(&mut conn).await?;
            ownership.epoch.store(epoch, Ordering::Release);
        }
        sqlx::query("SELECT set_config('statement_timeout', $1, false), set_config('lock_timeout', $2, false)")
            .bind(format!("{statement_ms}ms"))
            .bind(format!("{lock_ms}ms"))
            .execute(&mut *conn).await?;
        drop(conn);

        let (setting_rows, setting_bytes): (i64, i64) = sqlx::query_as(
            "SELECT count(*)::bigint,
                    COALESCE(sum(octet_length(key) + octet_length(value)), 0)::bigint
             FROM settings",
        )
        .fetch_one(&pool)
        .await?;
        let setting_rows = usize::try_from(setting_rows).unwrap_or(usize::MAX);
        let setting_bytes = usize::try_from(setting_bytes).unwrap_or(usize::MAX);
        let (largest_key, largest_value): (Option<i32>, Option<i32>) =
            sqlx::query_as("SELECT max(octet_length(key)), max(octet_length(value)) FROM settings")
                .fetch_one(&pool)
                .await?;
        if max_rows.is_some()
            && (largest_key.is_some_and(|size| {
                usize::try_from(size).map_or(true, |size| size > MAX_SETTING_KEY_BYTES)
            }) || largest_value.is_some_and(|size| {
                usize::try_from(size).map_or(true, |size| size > MAX_SETTING_VALUE_BYTES)
            }))
        {
            return Err(sqlx::Error::Protocol(format!(
                "database contains a settings key/value above {} / {} bytes",
                MAX_SETTING_KEY_BYTES, MAX_SETTING_VALUE_BYTES
            )));
        }
        if let Some(max_rows) = max_rows
            && setting_rows > max_rows
        {
            return Err(sqlx::Error::Protocol(format!(
                "database has {setting_rows} settings rows, above shard limit {max_rows}"
            )));
        }
        if let Some(max_bytes) = max_bytes
            && setting_bytes > max_bytes
        {
            return Err(sqlx::Error::Protocol(format!(
                "database has {setting_bytes} setting bytes, above shard limit {max_bytes}"
            )));
        }
        if let Some(max_chats) = max_chats {
            let max_chats = i64::try_from(max_chats).map_err(|_| {
                sqlx::Error::Protocol("maximum chat count exceeds PostgreSQL bigint".to_owned())
            })?;
            let (chat_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM durable_chats")
                .fetch_one(&pool)
                .await?;
            if chat_count > max_chats {
                return Err(sqlx::Error::Protocol(format!(
                    "database has {chat_count} configured chats, above shard limit {max_chats}"
                )));
            }
            let largest_chat_rows: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(row_count), 0) FROM (
                     SELECT chat_id, count(*) AS row_count
                     FROM settings
                     WHERE chat_id <> 0
                     GROUP BY chat_id
                 ) AS per_chat",
            )
            .fetch_one(&pool)
            .await?;
            if largest_chat_rows
                > i64::try_from(MAX_SETTINGS_ROWS_PER_CHAT)
                    .expect("per-chat setting limit fits PostgreSQL bigint")
            {
                return Err(sqlx::Error::Protocol(format!(
                    "database has a chat with {largest_chat_rows} settings rows, above per-chat limit {MAX_SETTINGS_ROWS_PER_CHAT}"
                )));
            }
        }

        const PAGE: i64 = 50_000;

        let mut cache: HashMap<i64, HashMap<String, String>> = HashMap::new();
        let mut index: HashMap<i64, ChatIndex> = HashMap::new();
        let (mut at_chat, mut at_key) = (i64::MIN, String::new());
        loop {
            let page: Vec<(i64, String, String)> = sqlx::query_as(
                "SELECT chat_id, key, value FROM settings
                 WHERE (chat_id, key) > ($1, $2)
                 ORDER BY chat_id, key LIMIT $3",
            )
            .bind(at_chat)
            .bind(&at_key)
            .bind(PAGE)
            .fetch_all(&pool)
            .await?;

            let Some((last_chat, last_key, _)) = page.last() else {
                break;
            };
            (at_chat, at_key) = (*last_chat, last_key.clone());

            for (chat, key, value) in page {
                if let Some((slot, rest)) = indexed_slot(&key) {
                    index.entry(chat).or_default()[slot].push(rest.into());
                }
                cache.entry(chat).or_default().insert(key, value);
            }
        }
        let durable_chats = sqlx::query_as::<_, (i64, i64)>(
            "SELECT chat_id, access_hash FROM durable_chats ORDER BY chat_id",
        )
        .fetch_all(&pool)
        .await?
        .into_iter()
        .collect();

        Ok(Self {
            pool,
            _process_lock: tokio::sync::Mutex::new(process_lock),
            ownership,
            stats_directory: lock_process.then(|| {
                std::env::var_os("DURABLE_WORK_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| "durable-work".into())
                    .join("stats")
            }),
            cache: RwLock::new(cache),
            index: RwLock::new(index),
            durable_chats: RwLock::new(durable_chats),
            max_chats,
            max_rows,
            max_bytes,
            max_counter_rows,
            max_tally_rows,
            max_note_rows,
            max_pending_captcha_rows,
            setting_rows: AtomicUsize::new(setting_rows),
            setting_bytes: AtomicUsize::new(setting_bytes),
            capacity_write: tokio::sync::Mutex::new(()),
            write_slots: (0..WRITE_SLOTS)
                .map(|_| std::sync::Arc::new(tokio::sync::Mutex::new(())))
                .collect(),
        })
    }

    async fn validate_durable_chat_ids(conn: &mut sqlx::PgConnection) -> Result<()> {
        let corrupt: Option<(String, i64)> = sqlx::query_as(
            "SELECT owner, chat_id
             FROM (
                 SELECT 'durable_chats' AS owner, chat_id FROM durable_chats
                 UNION ALL SELECT 'settings', chat_id FROM settings WHERE chat_id <> 0
                 UNION ALL SELECT 'default_rights_state', chat_id FROM default_rights_state
                 UNION ALL SELECT 'pending_captchas', chat_id FROM pending_captchas
                 UNION ALL SELECT 'pending_warn_actions', chat_id FROM pending_warn_actions
                 UNION ALL SELECT 'pending_strict_actions', chat_id FROM pending_strict_actions
                 UNION ALL SELECT 'counters', chat_id FROM counters
                 UNION ALL SELECT 'notes', chat_id FROM notes
                 UNION ALL SELECT 'tallies', chat_id FROM tallies
                 UNION ALL SELECT 'pending_deletes', chat_id FROM pending_deletes
                 UNION ALL SELECT 'pending_rank_awards', chat_id FROM pending_rank_awards
                 UNION ALL SELECT 'moderation_cases', chat_id FROM moderation_cases
                 UNION ALL SELECT 'moderation_case_events', chat_id FROM moderation_case_events
                 UNION ALL SELECT 'image_filters', chat_id FROM image_filters
             ) AS owners
             WHERE NOT (chat_id BETWEEN -999999999999 AND -1
                        OR chat_id BETWEEN -1997852516352 AND -1000000000001
                        OR chat_id BETWEEN -4000000000000 AND -2002147483649)
             ORDER BY owner, chat_id LIMIT 1",
        )
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((owner, chat)) = corrupt {
            return Err(sqlx::Error::Protocol(format!(
                "corrupt durable chat identity {chat} in {owner}"
            )));
        }
        Ok(())
    }

    async fn validate_counter_rows(conn: &mut sqlx::PgConnection) -> Result<()> {
        type CounterDomainRow = (
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        );
        let corrupt: Option<CounterDomainRow> = sqlx::query_as(
            "SELECT chat_id, user_id, total, today, day, week, week_at, month, month_at,
                    seen, adds, awarded, warns, strikes, struck
             FROM counters
             WHERE NOT (chat_id BETWEEN -999999999999 AND -1
                        OR chat_id BETWEEN -1997852516352 AND -1000000000001
                        OR chat_id BETWEEN -4000000000000 AND -2002147483649)
                OR user_id NOT BETWEEN 1 AND 1099511627775
                OR total < 0 OR today < 0 OR day < 0
                OR week < 0 OR week_at < 0 OR month < 0 OR month_at < 0
                OR seen < 0 OR adds < 0 OR awarded < 0
                OR warns < 0 OR warns > 4294967295
                OR strikes < 0 OR strikes > 4294967295 OR struck < 0
             ORDER BY chat_id, user_id LIMIT 1",
        )
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(row) = corrupt {
            return Err(sqlx::Error::Protocol(format!(
                "corrupt counter row {}/{}: total={}, today={}, day={}, week={}, week_at={}, month={}, month_at={}, seen={}, adds={}, awarded={}, warns={}, strikes={}, struck={}",
                row.0,
                row.1,
                row.2,
                row.3,
                row.4,
                row.5,
                row.6,
                row.7,
                row.8,
                row.9,
                row.10,
                row.11,
                row.12,
                row.13,
                row.14
            )));
        }
        Ok(())
    }

    async fn validate_tally_rows(conn: &mut sqlx::PgConnection) -> Result<()> {
        let corrupt: Option<(i64, String, i64, i64)> = sqlx::query_as(
            "SELECT chat_id, counter, day, count FROM tallies
             WHERE NOT (chat_id BETWEEN -999999999999 AND -1
                        OR chat_id BETWEEN -1997852516352 AND -1000000000001
                        OR chat_id BETWEEN -4000000000000 AND -2002147483649)
                OR octet_length(counter) NOT BETWEEN 1 AND $1::INTEGER
                OR position(':' in counter) <> 0 OR day < 0 OR count < 0
             ORDER BY chat_id, counter LIMIT 1",
        )
        .bind(MAX_TALLY_COUNTER_BYTES.to_string())
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((chat, counter, day, count)) = corrupt {
            return Err(sqlx::Error::Protocol(format!(
                "corrupt tally row {chat}/{counter:?}: day={day}, count={count}"
            )));
        }
        Ok(())
    }

    async fn validate_projected_counter_capacity(
        conn: &mut sqlx::PgConnection,
        max_counter_rows: Option<i64>,
    ) -> Result<()> {
        let (projected_counter_rows, projected_counter_chat): (i64, i64) = sqlx::query_as(
            "WITH identities AS (
                 SELECT chat_id, user_id::TEXT AS member FROM counters
                 UNION
                 SELECT chat_id, split_part(key, ':', 2) AS member FROM settings
                 WHERE key ~ '^(total|seen|adds|rank|warn|sv):-?[0-9]+$'
             ), per_chat AS (
                 SELECT chat_id, count(*) AS row_count FROM identities GROUP BY chat_id
             )
             SELECT (SELECT count(*) FROM identities), COALESCE(MAX(row_count), 0)
             FROM per_chat",
        )
        .fetch_one(&mut *conn)
        .await?;
        if projected_counter_chat > MAX_COUNTER_ROWS_PER_CHAT {
            return Err(sqlx::Error::Protocol(format!(
                "legacy migration would create {projected_counter_chat} counter rows in one chat, above {MAX_COUNTER_ROWS_PER_CHAT}"
            )));
        }
        if let Some(max_counter_rows) = max_counter_rows
            && projected_counter_rows > max_counter_rows
        {
            return Err(sqlx::Error::Protocol(format!(
                "legacy migration would create {projected_counter_rows} counter rows, above shard limit {max_counter_rows}"
            )));
        }
        Ok(())
    }

    async fn validate_projected_tally_capacity(
        conn: &mut sqlx::PgConnection,
        max_tally_rows: Option<i64>,
    ) -> Result<()> {
        let (projected_rows, projected_chat): (i64, i64) = sqlx::query_as(
            "WITH identities AS (
                 SELECT chat_id, counter FROM tallies
                 UNION
                 SELECT chat_id, substring(key from 7) AS counter FROM settings
                 WHERE key LIKE 'tally:%'
             ), per_chat AS (
                 SELECT chat_id, count(*) AS row_count FROM identities GROUP BY chat_id
             )
             SELECT (SELECT count(*) FROM identities), COALESCE(MAX(row_count), 0)
             FROM per_chat",
        )
        .fetch_one(&mut *conn)
        .await?;
        if projected_chat > MAX_TALLY_ROWS_PER_CHAT {
            return Err(sqlx::Error::Protocol(format!(
                "legacy migration would create {projected_chat} tally rows in one chat, above {MAX_TALLY_ROWS_PER_CHAT}"
            )));
        }
        if let Some(max_tally_rows) = max_tally_rows
            && projected_rows > max_tally_rows
        {
            return Err(sqlx::Error::Protocol(format!(
                "legacy migration would create {projected_rows} tally rows, above shard limit {max_tally_rows}"
            )));
        }
        Ok(())
    }

    async fn migrate_counters(conn: &mut sqlx::PgConnection) -> Result<()> {
        const MARK: &str = "counters_migrated";
        const MOVES: &[(&str, &str, i32, &str)] = &[
            ("total:", "total", 1, "split_part(value, '|', 2)"),
            ("seen:", "seen", 1, "split_part(value, '|', 2)"),
            ("adds:", "adds", 1, "split_part(value, '|', 2)"),
            ("rank:", "awarded", 1, "''"),
        ];

        let done = sqlx::query("SELECT 1 FROM settings WHERE chat_id = 0 AND key = $1")
            .bind(MARK)
            .fetch_optional(&mut *conn)
            .await?;
        if done.is_some() {
            return Ok(());
        }

        let corrupt: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT chat_id, key, value FROM settings
             WHERE (key LIKE 'total:%' OR key LIKE 'today:%' OR key LIKE 'week:%'
                    OR key LIKE 'month:%' OR key LIKE 'seen:%' OR key LIKE 'adds:%'
                    OR key LIKE 'rank:%')
               AND NOT (
                 (chat_id BETWEEN -999999999999 AND -1
                  OR chat_id BETWEEN -1997852516352 AND -1000000000001
                  OR chat_id BETWEEN -4000000000000 AND -2002147483649)
                 AND
                 CASE WHEN key ~ '^(total|today|week|month|seen|adds|rank):-?[0-9]+$'
                      THEN split_part(key, ':', 2)::NUMERIC
                           BETWEEN 1 AND 1099511627775
                      ELSE FALSE END
                 AND CASE
                   WHEN split_part(key, ':', 1) IN ('today', 'week', 'month') THEN
                     CASE WHEN value ~ '^[0-9]+[|][0-9]+[|].*$' THEN
                       split_part(value, '|', 1)::NUMERIC BETWEEN 0 AND 9223372036854775807
                       AND split_part(value, '|', 2)::NUMERIC BETWEEN 0 AND 9223372036854775807
                     ELSE FALSE END
                   WHEN split_part(key, ':', 1) IN ('total', 'seen', 'adds') THEN
                     CASE WHEN value ~ '^[0-9]+[|].*$' THEN
                       split_part(value, '|', 1)::NUMERIC BETWEEN 0 AND 9223372036854775807
                     ELSE FALSE END
                   WHEN split_part(key, ':', 1) = 'rank' THEN
                     CASE WHEN value ~ '^[0-9]+$' THEN
                       value::NUMERIC BETWEEN 0 AND 9223372036854775807
                     ELSE FALSE END
                   ELSE FALSE
                 END
               )
             ORDER BY chat_id, key LIMIT 1",
        )
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((chat, key, value)) = corrupt {
            return Err(sqlx::Error::Protocol(format!(
                "refusing to delete corrupt legacy counter {chat}/{key}={value:?}"
            )));
        }

        for (prefix, column, part, name) in MOVES {
            let moved = sqlx::query(&format!(
                "INSERT INTO counters (chat_id, user_id, name, {column})
                 SELECT chat_id,
                        split_part(key, ':', 2)::bigint,
                        {name},
                        split_part(value, '|', {part})::bigint
                 FROM settings
                 WHERE key ~ '^{prefix}-?[0-9]+$'
                 ON CONFLICT (chat_id, user_id) DO UPDATE
                    SET {column} = EXCLUDED.{column},
                        name = COALESCE(NULLIF(EXCLUDED.name, ''), counters.name)"
            ))
            .execute(&mut *conn)
            .await?;
            if moved.rows_affected() > 0 {
                println!(
                    "migrated {} {prefix} rows into counters",
                    moved.rows_affected()
                );
            }
        }

        for prefix in COUNTER_PREFIXES {
            sqlx::query(&format!("DELETE FROM settings WHERE key LIKE '{prefix}%'"))
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query("INSERT INTO settings (chat_id, key) VALUES (0, $1) ON CONFLICT DO NOTHING")
            .bind(MARK)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    async fn migrate_per_user(conn: &mut sqlx::PgConnection) -> Result<()> {
        const MARK: &str = "per_user_migrated";

        let done = sqlx::query("SELECT 1 FROM settings WHERE chat_id = 0 AND key = $1")
            .bind(MARK)
            .fetch_optional(&mut *conn)
            .await?;
        if done.is_some() {
            return Ok(());
        }

        let corrupt: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT chat_id, key, value FROM settings
             WHERE (key LIKE 'warn:%' AND NOT (
                       (chat_id BETWEEN -999999999999 AND -1
                        OR chat_id BETWEEN -1997852516352 AND -1000000000001
                        OR chat_id BETWEEN -4000000000000 AND -2002147483649)
                       AND
                       CASE WHEN key ~ '^warn:-?[0-9]+$' THEN
                         split_part(key, ':', 2)::NUMERIC BETWEEN 1 AND 1099511627775
                       ELSE FALSE END
                       AND CASE WHEN value ~ '^[0-9]+$' THEN
                         value::NUMERIC BETWEEN 0 AND 4294967295
                       ELSE FALSE END))
                OR (key LIKE 'sv:%' AND NOT (
                       (chat_id BETWEEN -999999999999 AND -1
                        OR chat_id BETWEEN -1997852516352 AND -1000000000001
                        OR chat_id BETWEEN -4000000000000 AND -2002147483649)
                       AND
                       CASE WHEN key ~ '^sv:-?[0-9]+$' THEN
                         split_part(key, ':', 2)::NUMERIC BETWEEN 1 AND 1099511627775
                       ELSE FALSE END
                       AND CASE WHEN value ~ '^[0-9]+:[0-9]+$' THEN
                         split_part(value, ':', 1)::NUMERIC BETWEEN 0 AND 4294967295
                         AND split_part(value, ':', 2)::NUMERIC BETWEEN 0 AND 9223372036854775807
                       ELSE FALSE END))
             ORDER BY chat_id, key LIMIT 1",
        )
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((chat, key, value)) = corrupt {
            return Err(sqlx::Error::Protocol(format!(
                "refusing to normalize corrupt legacy punishment counter {chat}/{key}={value:?}"
            )));
        }

        let moved = sqlx::query(
            "INSERT INTO counters (chat_id, user_id, warns)
             SELECT chat_id,
                    split_part(key, ':', 2)::bigint,
                    value::bigint
             FROM settings
             WHERE key ~ '^warn:-?[0-9]+$'
             ON CONFLICT (chat_id, user_id) DO UPDATE SET warns = EXCLUDED.warns",
        )
        .execute(&mut *conn)
        .await?;
        if moved.rows_affected() > 0 {
            println!("migrated {} warn rows into counters", moved.rows_affected());
        }

        let moved = sqlx::query(
            "INSERT INTO counters (chat_id, user_id, strikes, struck)
             SELECT chat_id,
                    split_part(key, ':', 2)::bigint,
                    split_part(value, ':', 1)::bigint,
                    split_part(value, ':', 2)::bigint
             FROM settings
             WHERE key ~ '^sv:-?[0-9]+$'
             ON CONFLICT (chat_id, user_id) DO UPDATE
                SET strikes = EXCLUDED.strikes, struck = EXCLUDED.struck",
        )
        .execute(&mut *conn)
        .await?;
        if moved.rows_affected() > 0 {
            println!(
                "migrated {} strike rows into counters",
                moved.rows_affected()
            );
        }

        sqlx::query(
            "DELETE FROM settings
             WHERE key LIKE 'warn:%' OR key LIKE 'sv:%' OR key = 'sv_day'",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query("INSERT INTO settings (chat_id, key) VALUES (0, $1) ON CONFLICT DO NOTHING")
            .bind(MARK)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    async fn migrate_wipe_split(conn: &mut sqlx::PgConnection) -> Result<()> {
        const MARK: &str = "wipe_split_migrated";

        let done = sqlx::query("SELECT 1 FROM settings WHERE chat_id = 0 AND key = $1")
            .bind(MARK)
            .fetch_optional(&mut *conn)
            .await?;
        if done.is_some() {
            return Ok(());
        }

        for new_key in ["wipe_ban_on", "wipe_mute_on"] {
            sqlx::query(
                "INSERT INTO settings (chat_id, key)
                 SELECT chat_id, $1 FROM settings WHERE key = 'wipe_on'
                 ON CONFLICT (chat_id, key) DO NOTHING",
            )
            .bind(new_key)
            .execute(&mut *conn)
            .await?;
        }
        let moved = sqlx::query("DELETE FROM settings WHERE key = 'wipe_on'")
            .execute(&mut *conn)
            .await?;
        if moved.rows_affected() > 0 {
            println!(
                "migrated {} wipe_on rows into wipe_ban_on + wipe_mute_on",
                moved.rows_affected()
            );
        }
        sqlx::query("INSERT INTO settings (chat_id, key) VALUES (0, $1) ON CONFLICT DO NOTHING")
            .bind(MARK)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    async fn migrate_tallies(conn: &mut sqlx::PgConnection) -> Result<()> {
        const MARK: &str = "tallies_migrated";

        let done = sqlx::query("SELECT 1 FROM settings WHERE chat_id = 0 AND key = $1")
            .bind(MARK)
            .fetch_optional(&mut *conn)
            .await?;
        if done.is_some() {
            return Ok(());
        }

        let corrupt: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT chat_id, key, value FROM settings
             WHERE key LIKE 'tally:%' AND NOT (
                 (chat_id BETWEEN -999999999999 AND -1
                  OR chat_id BETWEEN -1997852516352 AND -1000000000001
                  OR chat_id BETWEEN -4000000000000 AND -2002147483649)
                 AND octet_length(substring(key from 7)) BETWEEN 1 AND $1::INTEGER
                 AND position(':' in substring(key from 7)) = 0
                 AND CASE WHEN value ~ '^[0-9]+[|][0-9]+$' THEN
                       split_part(value, '|', 1)::NUMERIC BETWEEN 0 AND 9223372036854775807
                       AND split_part(value, '|', 2)::NUMERIC BETWEEN 0 AND 9223372036854775807
                     ELSE FALSE END
             )
             ORDER BY chat_id, key LIMIT 1",
        )
        .bind(MAX_TALLY_COUNTER_BYTES.to_string())
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((chat, key, value)) = corrupt {
            return Err(sqlx::Error::Protocol(format!(
                "refusing to delete corrupt legacy tally {chat}/{key}={value:?}"
            )));
        }

        let moved = sqlx::query(
            "INSERT INTO tallies (chat_id, counter, day, count)
             SELECT chat_id,
                    substring(key from 7),
                    split_part(value, '|', 1)::bigint,
                    split_part(value, '|', 2)::bigint
             FROM settings
             WHERE key LIKE 'tally:%'
             ON CONFLICT (chat_id, counter) DO NOTHING",
        )
        .execute(&mut *conn)
        .await?;
        if moved.rows_affected() > 0 {
            println!("migrated {} tally rows into tallies", moved.rows_affected());
        }

        sqlx::query("DELETE FROM settings WHERE key LIKE 'tally:%'")
            .execute(&mut *conn)
            .await?;
        sqlx::query("INSERT INTO settings (chat_id, key) VALUES (0, $1) ON CONFLICT DO NOTHING")
            .bind(MARK)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    async fn migrate_notes(conn: &mut sqlx::PgConnection) -> Result<()> {
        const MARK: &str = "notes_table_migrated";

        let done = sqlx::query("SELECT 1 FROM settings WHERE chat_id = 0 AND key = $1")
            .bind(MARK)
            .fetch_optional(&mut *conn)
            .await?;
        if done.is_some() {
            return Ok(());
        }

        let moved = sqlx::query(
            "INSERT INTO notes (chat_id, user_id, value)
             SELECT chat_id, split_part(key, ':', 2)::bigint, value
             FROM settings
             WHERE key ~ '^note:-?[0-9]+$' AND value <> ''
             ON CONFLICT (chat_id, user_id) DO UPDATE SET value = EXCLUDED.value",
        )
        .execute(&mut *conn)
        .await?;
        let removed = sqlx::query("DELETE FROM settings WHERE key ~ '^note:-?[0-9]+$'")
            .execute(&mut *conn)
            .await?;
        if moved.rows_affected() > 0 || removed.rows_affected() > 0 {
            println!(
                "migrated {} note(s) into notes and removed {} legacy row(s)",
                moved.rows_affected(),
                removed.rows_affected()
            );
        }

        sqlx::query("INSERT INTO settings (chat_id, key) VALUES (0, $1) ON CONFLICT DO NOTHING")
            .bind(MARK)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    pub async fn board(
        &self,
        chat: i64,
        period: Period,
        stamp: u64,
        limit: i64,
    ) -> Result<Vec<Counter>> {
        let count = period.count();
        let sql = match period.stamp() {
            Some(at) => format!(
                "SELECT user_id, name, {count} FROM counters
                 WHERE chat_id = $1 AND {at} = $2 AND {count} > 0
                 ORDER BY {count} DESC LIMIT $3"
            ),
            None => format!(
                "SELECT user_id, name, {count} FROM counters
                 WHERE chat_id = $1 AND {count} > 0
                 ORDER BY {count} DESC LIMIT $2"
            ),
        };
        let mut query = sqlx::query_as::<_, (i64, String, i64)>(&sql).bind(chat);
        if period.stamp().is_some() {
            query = query.bind(saturating_postgres_i64(stamp));
        }
        let rows = query.bind(limit).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|(user, name, value)| {
                Ok(Counter {
                    user,
                    name,
                    count: nonnegative_counter(count, value)?,
                })
            })
            .collect()
    }

    pub async fn board_totals(&self, chat: i64, period: Period, stamp: u64) -> Result<(u64, u64)> {
        let count = period.count();
        let sql = match period.stamp() {
            Some(at) => format!(
                "SELECT COALESCE(SUM({count}), 0)::bigint, COUNT(*) FROM counters
                 WHERE chat_id = $1 AND {at} = $2 AND {count} > 0"
            ),
            None => format!(
                "SELECT COALESCE(SUM({count}), 0)::bigint, COUNT(*) FROM counters
                 WHERE chat_id = $1 AND {count} > 0"
            ),
        };
        let mut query = sqlx::query_as::<_, (i64, i64)>(&sql).bind(chat);
        if period.stamp().is_some() {
            query = query.bind(saturating_postgres_i64(stamp));
        }
        let (sum, users) = query.fetch_one(&self.pool).await?;
        Ok((
            nonnegative_counter(count, sum)?,
            nonnegative_counter("board member count", users)?,
        ))
    }

    pub async fn idle(&self, chat: i64, day: u64, days: u64, limit: i64) -> Result<Idle> {
        let rows: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT user_id, name, $2 - seen FROM counters
             WHERE chat_id = $1 AND seen > 0 AND $2 - seen >= $3
             ORDER BY seen ASC LIMIT $4",
        )
        .bind(chat)
        .bind(saturating_postgres_i64(day))
        .bind(saturating_postgres_i64(days))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let total: i64 = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM counters
             WHERE chat_id = $1 AND seen > 0 AND $2 - seen >= $3",
        )
        .bind(chat)
        .bind(saturating_postgres_i64(day))
        .bind(saturating_postgres_i64(days))
        .fetch_one(&self.pool)
        .await?
        .0;

        let idle = rows
            .into_iter()
            .map(|(user, name, quiet)| Ok((user, name, nonnegative_counter("idle days", quiet)?)))
            .collect::<Result<Vec<_>>>()?;
        Ok((idle, nonnegative_counter("idle member count", total)?))
    }

    #[cfg(test)]
    pub async fn bump(
        &self,
        bumps: Vec<Bump>,
        day: u64,
        week: u64,
        month: u64,
    ) -> Result<Vec<Bumped>> {
        const CHUNK: usize = 5_000;
        const AT_ONCE: usize = 4;
        const FOLD: &str = include_str!("state/counter_fold.sql");

        type Batch = (Vec<i64>, Vec<i64>, Vec<String>, Vec<i64>);
        let max_counter_rows = self.max_counter_rows.unwrap_or(i64::MAX / 2);
        let mut batches: Vec<Batch> = Vec::new();
        let mut batch = Batch::default();
        for bump in bumps {
            batch.0.push(bump.chat);
            batch.1.push(bump.user);
            batch.2.push(bump.name);
            batch.3.push(saturating_postgres_i64(bump.added));
            if batch.0.len() == CHUNK {
                batches.push(std::mem::take(&mut batch));
            }
        }
        if !batch.0.is_empty() {
            batches.push(batch);
        }

        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
        let mut tasks = tokio::task::JoinSet::new();
        let mut folded = Vec::new();
        let mut failure = None;
        let mut reap = |done: std::result::Result<
            Result<Vec<BumpedRow>>,
            tokio::task::JoinError,
        >| {
            match done {
                Ok(Ok(rows)) => {
                    for (chat, user, name, total, awarded) in rows {
                        match (
                            nonnegative_counter("total", total),
                            nonnegative_counter("awarded", awarded),
                        ) {
                            (Ok(total), Ok(awarded)) => folded.push(Bumped {
                                chat,
                                user,
                                name,
                                total,
                                awarded,
                            }),
                            (Err(error), _) | (_, Err(error)) => {
                                if failure.is_none() {
                                    failure = Some(error);
                                }
                            }
                        }
                    }
                }
                Ok(Err(error)) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(sqlx::Error::Protocol(format!(
                            "counter chunk task failed: {error}"
                        )));
                    }
                }
            }
        };
        for (chats, users, names, added) in batches {
            while let Some(done) = tasks.try_join_next() {
                reap(done);
            }

            let permit = loop {
                let acquire = std::sync::Arc::clone(&permits).acquire_owned();
                if tasks.is_empty() {
                    break acquire.await.expect("the bump semaphore is never closed");
                }
                tokio::select! {
                    permit = acquire => {
                        break permit.expect("the bump semaphore is never closed");
                    }
                    done = tasks.join_next() => {
                        if let Some(done) = done {
                            reap(done);
                        }
                    }
                }
            };
            let mut stripe_ids: Vec<usize> = chats
                .iter()
                .map(|chat| self.write_slot_index(*chat))
                .collect();
            stripe_ids.sort_unstable();
            stripe_ids.dedup();
            let stripe_locks: Vec<_> = stripe_ids
                .into_iter()
                .map(|stripe| std::sync::Arc::clone(&self.write_slots[stripe]))
                .collect();
            let pool = self.pool.clone();
            tasks.spawn(async move {
                let _permit = permit;
                let mut _writing = Vec::with_capacity(stripe_locks.len());
                for slot in stripe_locks {
                    _writing.push(slot.lock_owned().await);
                }
                sqlx::query_as(FOLD)
                    .bind(&chats)
                    .bind(&users)
                    .bind(&names)
                    .bind(&added)
                    .bind(saturating_postgres_i64(day))
                    .bind(saturating_postgres_i64(week))
                    .bind(saturating_postgres_i64(month))
                    .bind(MAX_COUNTER_ROWS_PER_CHAT)
                    .bind(max_counter_rows)
                    .fetch_all(&pool)
                    .await
            });
        }

        while let Some(done) = tasks.join_next().await {
            reap(done);
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(folded),
        }
    }

    pub async fn credit_add(
        &self,
        chat: i64,
        user: i64,
        name: &str,
        added: u64,
    ) -> Result<Option<u64>> {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let max_counter_rows = self.max_counter_rows.unwrap_or(i64::MAX / 2);
        let row: Option<(i64,)> = sqlx::query_as(
            "WITH existing AS (
                 SELECT 1 FROM counters WHERE chat_id = $1 AND user_id = $2
             ), per_chat AS (
                 SELECT count(*) < $6 AS admitted FROM counters WHERE chat_id = $1
             ), reserved AS (
                 UPDATE durable_counts
                 SET counter_rows = counter_rows + 1
                 WHERE id = 0
                   AND NOT EXISTS (SELECT 1 FROM existing)
                   AND counter_rows < $5
                   AND (SELECT admitted FROM per_chat)
                 RETURNING id
             )
             INSERT INTO counters (chat_id, user_id, name, adds)
             SELECT $1, $2, $3, $4
             WHERE EXISTS (SELECT 1 FROM existing) OR EXISTS (SELECT 1 FROM reserved)
             ON CONFLICT (chat_id, user_id) DO UPDATE SET
                 adds = counters.adds + EXCLUDED.adds,
                 name = COALESCE(NULLIF(EXCLUDED.name, ''), counters.name)
             RETURNING adds",
        )
        .bind(chat)
        .bind(user)
        .bind(name)
        .bind(saturating_postgres_i64(added))
        .bind(max_counter_rows)
        .bind(MAX_COUNTER_ROWS_PER_CHAT)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(adds,)| nonnegative_counter("adds", adds))
            .transpose()
    }

    pub async fn warns_of(&self, chat: i64, user: i64) -> Result<u32> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT warns FROM counters WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(warns,)| warning_count(warns)).unwrap_or(Ok(0))
    }

    #[cfg(test)]
    pub async fn set_warns(&self, chat: i64, user: i64, warns: u32) -> Result<()> {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        if warns == 0 {
            sqlx::query("UPDATE counters SET warns = 0 WHERE chat_id = $1 AND user_id = $2")
                .bind(chat)
                .bind(user)
                .execute(&self.pool)
                .await?;
            return Ok(());
        }
        let max_counter_rows = self.max_counter_rows.unwrap_or(i64::MAX / 2);
        let result = sqlx::query(
            "WITH existing AS (
                 SELECT 1 FROM counters WHERE chat_id = $1 AND user_id = $2
             ), per_chat AS (
                 SELECT count(*) < $5 AS admitted FROM counters WHERE chat_id = $1
             ), reserved AS (
                 UPDATE durable_counts
                 SET counter_rows = counter_rows + 1
                 WHERE id = 0
                   AND NOT EXISTS (SELECT 1 FROM existing)
                   AND counter_rows < $4
                   AND (SELECT admitted FROM per_chat)
                 RETURNING id
             )
             INSERT INTO counters (chat_id, user_id, warns)
             SELECT $1, $2, $3
             WHERE EXISTS (SELECT 1 FROM existing) OR EXISTS (SELECT 1 FROM reserved)
             ON CONFLICT (chat_id, user_id) DO UPDATE SET warns = EXCLUDED.warns",
        )
        .bind(chat)
        .bind(user)
        .bind(i64::from(warns))
        .bind(max_counter_rows)
        .bind(MAX_COUNTER_ROWS_PER_CHAT)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(sqlx::Error::Protocol(
                "counter row capacity reached while storing warning".to_owned(),
            ));
        }
        Ok(())
    }

    pub async fn increment_warning(
        &self,
        chat: i64,
        user: i64,
        limit: u32,
        penalty: WarningPenalty,
    ) -> Result<WarningIncrement> {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let mut tx = self.pool.begin().await?;
        let pending: Option<(String, i64)> = sqlx::query_as(
            "SELECT pending.penalty, COALESCE(counters.warns, 0)
             FROM pending_warn_actions AS pending
             LEFT JOIN counters USING (chat_id, user_id)
             WHERE pending.chat_id = $1 AND pending.user_id = $2
             FOR UPDATE OF pending",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((stored, count)) = pending {
            tx.commit().await?;
            return Ok(WarningIncrement {
                count: warning_count(count)?,
                pending: Some(WarningPenalty::from_db(&stored)?),
                added: false,
            });
        }

        let max_counter_rows = self.max_counter_rows.unwrap_or(i64::MAX / 2);
        let count: Option<i64> = sqlx::query_scalar(
            "WITH existing AS (
                 SELECT 1 FROM counters WHERE chat_id = $1 AND user_id = $2
             ), per_chat AS (
                 SELECT count(*) < $4 AS admitted FROM counters WHERE chat_id = $1
             ), reserved AS (
                 UPDATE durable_counts
                 SET counter_rows = counter_rows + 1
                 WHERE id = 0
                   AND NOT EXISTS (SELECT 1 FROM existing)
                   AND counter_rows < $3
                   AND (SELECT admitted FROM per_chat)
                 RETURNING id
             )
             INSERT INTO counters (chat_id, user_id, warns)
             SELECT $1, $2, 1
             WHERE EXISTS (SELECT 1 FROM existing) OR EXISTS (SELECT 1 FROM reserved)
             ON CONFLICT (chat_id, user_id) DO UPDATE
             SET warns = counters.warns + 1
             RETURNING warns",
        )
        .bind(chat)
        .bind(user)
        .bind(max_counter_rows)
        .bind(MAX_COUNTER_ROWS_PER_CHAT)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(count) = count else {
            tx.rollback().await?;
            return Err(sqlx::Error::Protocol(
                "counter row capacity reached while adding warning".to_owned(),
            ));
        };
        let count = warning_count(count)?;
        let pending = (count >= limit).then_some(penalty);
        if pending.is_some() {
            sqlx::query(
                "INSERT INTO pending_warn_actions
                 (chat_id, user_id, penalty, created_at, claimed_until)
                 VALUES ($1, $2, $3, $4, $4)
                 ON CONFLICT (chat_id, user_id) DO NOTHING",
            )
            .bind(chat)
            .bind(user)
            .bind(penalty.as_db())
            .bind(unix_now())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(WarningIncrement {
            count,
            pending,
            added: true,
        })
    }

    pub async fn decrement_warning(&self, chat: i64, user: i64) -> Result<u32> {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM pending_warn_actions WHERE chat_id = $1 AND user_id = $2")
            .bind(chat)
            .bind(user)
            .execute(&mut *tx)
            .await?;
        let count: Option<i64> = sqlx::query_scalar(
            "UPDATE counters
             SET warns = GREATEST(warns - 1, 0)
             WHERE chat_id = $1 AND user_id = $2
             RETURNING warns",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        count.map(warning_count).unwrap_or(Ok(0))
    }

    pub async fn warning_action_is_current(&self, pending: &PendingWarningAction) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM pending_warn_actions
                 WHERE chat_id = $1 AND user_id = $2 AND penalty = $3
                   AND generation = $4 AND version = $5 AND lease_token = $6
                   AND attempts = $7
             )",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.penalty.as_db())
        .bind(pending.generation)
        .bind(pending.version)
        .bind(pending.lease_token)
        .bind(i64::from(pending.attempts))
        .fetch_one(&self.pool)
        .await
    }

    pub async fn claim_warning_action(
        &self,
        chat: i64,
        user: i64,
        now: i64,
        lease_until: i64,
    ) -> Result<Option<PendingWarningAction>> {
        let row: Option<PendingWarningTuple> = sqlx::query_as(
            "UPDATE pending_warn_actions
             SET claimed_until = $4,
                 attempts = attempts + 1,
                 version = version + 1,
                 lease_token = nextval('durable_work_token_seq')
             WHERE chat_id = $1 AND user_id = $2 AND claimed_until <= $3
               AND NOT awaiting_rejoin
             RETURNING chat_id, user_id, penalty, attempts,
                       awaiting_rejoin, generation, version, lease_token",
        )
        .bind(chat)
        .bind(user)
        .bind(now)
        .bind(lease_until)
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_pending_warning).transpose()
    }

    pub async fn claim_pending_warning_actions(
        &self,
        class: WarningQueueClass,
        now: i64,
        lease_until: i64,
        limit: i64,
    ) -> Result<Vec<PendingWarningAction>> {
        let predicate = match class {
            WarningQueueClass::Fresh => {
                "attempts = 0 AND NOT awaiting_rejoin AND claimed_until <= $1"
            }
            WarningQueueClass::Retry => {
                "attempts > 0 AND NOT awaiting_rejoin AND claimed_until <= $1"
            }
        };
        let statement = format!(
            "WITH due AS (
                 SELECT chat_id, user_id
                 FROM pending_warn_actions
                 WHERE {predicate}
                 ORDER BY claimed_until, attempts, created_at, chat_id, user_id
                 FOR UPDATE SKIP LOCKED
                 LIMIT $3
             )
             UPDATE pending_warn_actions AS pending
             SET claimed_until = $2,
                 attempts = pending.attempts + 1,
                 version = pending.version + 1,
                 lease_token = nextval('durable_work_token_seq')
             FROM due
             WHERE pending.chat_id = due.chat_id AND pending.user_id = due.user_id
             RETURNING pending.chat_id, pending.user_id, pending.penalty, pending.attempts,
                       pending.awaiting_rejoin, pending.generation, pending.version, pending.lease_token"
        );
        let rows: Vec<PendingWarningTuple> = sqlx::query_as(&statement)
            .bind(now)
            .bind(lease_until)
            .bind(limit.clamp(1, 64))
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(decode_pending_warning).collect()
    }

    pub async fn defer_warning_action(
        &self,
        pending: &PendingWarningAction,
        retry_at: i64,
        reason: &str,
    ) -> Result<bool> {
        let reason = bounded_warning_error(reason);
        let updated = sqlx::query(
            "UPDATE pending_warn_actions
             SET claimed_until = $8, lease_token = NULL, awaiting_rejoin = FALSE,
                 last_error = $9
             WHERE chat_id = $1 AND user_id = $2 AND penalty = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.penalty.as_db())
        .bind(pending.generation)
        .bind(pending.version)
        .bind(pending.lease_token)
        .bind(i64::from(pending.attempts))
        .bind(retry_at)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn await_warning_rejoin(
        &self,
        pending: &PendingWarningAction,
        reason: &str,
    ) -> Result<bool> {
        let reason = bounded_warning_error(reason);
        let mut tx = self.pool.begin().await?;
        lock_durable_member_rejoin(&mut tx, pending.chat, pending.user).await?;
        let updated = sqlx::query(
            "UPDATE pending_warn_actions
             SET awaiting_rejoin = TRUE, lease_token = NULL, last_error = $8
             WHERE chat_id = $1 AND user_id = $2 AND penalty = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.penalty.as_db())
        .bind(pending.generation)
        .bind(pending.version)
        .bind(pending.lease_token)
        .bind(i64::from(pending.attempts))
        .bind(reason)
        .execute(&mut *tx)
        .await?;
        let changed = updated.rows_affected() == 1;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn resume_warning_on_rejoin(&self, chat: i64, user: i64, now: i64) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        lock_durable_member_rejoin(&mut tx, chat, user).await?;
        let updated = sqlx::query(
            "UPDATE pending_warn_actions
             SET awaiting_rejoin = FALSE, claimed_until = $3, last_error = NULL
             WHERE chat_id = $1 AND user_id = $2 AND awaiting_rejoin AND lease_token IS NULL",
        )
        .bind(chat)
        .bind(user)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let changed = updated.rows_affected() == 1;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn complete_warning_action(&self, pending: &PendingWarningAction) -> Result<bool> {
        let slot = self.write_slot(pending.chat);
        let _writing = slot.lock().await;
        let mut tx = self.pool.begin().await?;
        let removed = sqlx::query(
            "DELETE FROM pending_warn_actions
             WHERE chat_id = $1 AND user_id = $2 AND penalty = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.penalty.as_db())
        .bind(pending.generation)
        .bind(pending.version)
        .bind(pending.lease_token)
        .bind(i64::from(pending.attempts))
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if removed {
            sqlx::query("UPDATE counters SET warns = 0 WHERE chat_id = $1 AND user_id = $2")
                .bind(pending.chat)
                .bind(pending.user)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(removed)
    }

    pub async fn terminate_warning_action(&self, pending: &PendingWarningAction) -> Result<bool> {
        let slot = self.write_slot(pending.chat);
        let _writing = slot.lock().await;
        let removed = sqlx::query(
            "DELETE FROM pending_warn_actions
             WHERE chat_id = $1 AND user_id = $2 AND penalty = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.penalty.as_db())
        .bind(pending.generation)
        .bind(pending.version)
        .bind(pending.lease_token)
        .bind(i64::from(pending.attempts))
        .execute(&self.pool)
        .await?;
        Ok(removed.rows_affected() == 1)
    }

    pub async fn adds_of(&self, chat: i64, user: i64) -> Result<Option<u64>> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT adds FROM counters WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(adds,)| nonnegative_counter("adds", adds))
            .transpose()
    }

    pub async fn card(&self, chat: i64, user: i64, day: u64) -> Result<Option<Card>> {
        let row: Option<(i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT CASE WHEN c.day = $3 THEN c.today ELSE 0 END,
                    c.total,
                    c.adds,
                    (SELECT COUNT(*) + 1 FROM counters p
                      WHERE p.chat_id = c.chat_id AND p.day = $3
                        AND p.today > CASE WHEN c.day = $3 THEN c.today ELSE 0 END)
             FROM counters c WHERE c.chat_id = $1 AND c.user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .bind(saturating_postgres_i64(day))
        .fetch_optional(&self.pool)
        .await?;
        let Some((today, total, adds, place)) = row else {
            return Ok(None);
        };
        Ok(Some(Card {
            today: nonnegative_counter("today", today)?,
            total: nonnegative_counter("total", total)?,
            adds: nonnegative_counter("adds", adds)?,
            place: (today > 0)
                .then(|| nonnegative_counter("board place", place))
                .transpose()?,
        }))
    }

    pub async fn name_of(&self, chat: i64, user: i64) -> Result<Option<String>> {
        let row = sqlx::query_as::<_, (String,)>(
            "SELECT name FROM counters WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(name,)| name).filter(|name| !name.is_empty()))
    }

    pub async fn clear_seen(&self, chat: i64, user: i64) -> Result<()> {
        sqlx::query("UPDATE counters SET seen = 0 WHERE chat_id = $1 AND user_id = $2")
            .bind(chat)
            .bind(user)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_pending(&self, rows: &[(i64, i32, i64)]) -> Result<()> {
        const CHUNK: usize = 1_000;
        for batch in rows.chunks(CHUNK) {
            let (mut chats, mut ids, mut dues) = (Vec::new(), Vec::new(), Vec::new());
            for (chat, id, due) in batch {
                chats.push(*chat);
                ids.push(*id);
                dues.push(*due);
            }
            let result = sqlx::query(
                "INSERT INTO pending_deletes (chat_id, message_id, due_at)
                 SELECT * FROM UNNEST($1::bigint[], $2::int[], $3::bigint[])
                 ON CONFLICT (chat_id, message_id) DO NOTHING",
            )
            .bind(&chats)
            .bind(&ids)
            .bind(&dues)
            .execute(&self.pool)
            .await;
            result?;
        }
        Ok(())
    }

    pub async fn drop_pending(&self, chat: i64, ids: &[i32]) -> Result<()> {
        sqlx::query("DELETE FROM pending_deletes WHERE chat_id = $1 AND message_id = ANY($2)")
            .bind(chat)
            .bind(ids)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn drop_pending_rows(&self, rows: &[(i64, i32)]) -> Result<()> {
        const CHUNK: usize = 5_000;
        for batch in rows.chunks(CHUNK) {
            let (mut chats, mut ids) = (
                Vec::with_capacity(batch.len()),
                Vec::with_capacity(batch.len()),
            );
            for (chat, id) in batch {
                chats.push(*chat);
                ids.push(*id);
            }
            sqlx::query(
                "DELETE FROM pending_deletes AS pending
                 USING UNNEST($1::bigint[], $2::int[]) AS dropped(chat_id, message_id)
                 WHERE pending.chat_id = dropped.chat_id
                   AND pending.message_id = dropped.message_id",
            )
            .bind(&chats)
            .bind(&ids)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    pub async fn load_pending(&self, now: i64) -> Result<Vec<(i64, i32, i64)>> {
        const STALE_PENDING: i64 = 2 * 86_400;
        const STALE_BATCH: i64 = 5_000;
        const MOST: i64 = 200_000;

        let floor = now - STALE_PENDING;
        let mut dropped = 0u64;
        loop {
            let done = sqlx::query(
                "DELETE FROM pending_deletes
                 WHERE ctid IN (
                     SELECT ctid FROM pending_deletes
                     WHERE due_at < $1 ORDER BY due_at LIMIT $2
                 )",
            )
            .bind(floor)
            .bind(STALE_BATCH)
            .execute(&self.pool)
            .await?;
            let count = done.rows_affected();
            dropped += count;
            if count < STALE_BATCH as u64 {
                break;
            }
            tokio::task::yield_now().await;
        }
        if dropped > 0 {
            println!("pending deletes: dropped {dropped} long overdue");
        }

        let rows = sqlx::query_as(
            "SELECT chat_id, message_id, due_at FROM pending_deletes
             WHERE due_at >= $1 ORDER BY due_at LIMIT $2",
        )
        .bind(floor)
        .bind(MOST)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn forget_idle(&self, chats: &[i64], before: i64) -> Result<u64> {
        let mut stripe_ids: Vec<usize> = chats
            .iter()
            .map(|chat| self.write_slot_index(*chat))
            .collect();
        stripe_ids.sort_unstable();
        stripe_ids.dedup();
        let mut _writing = Vec::with_capacity(stripe_ids.len());
        for stripe in stripe_ids {
            _writing.push(self.write_slots[stripe].lock().await);
        }
        let deleted = sqlx::query_scalar::<_, i64>(
            "WITH removed AS (
                 DELETE FROM counters
                 WHERE chat_id = ANY($1) AND seen > 0 AND seen <= $2
                 RETURNING 1
             ), adjusted AS (
                 UPDATE durable_counts
                 SET counter_rows = GREATEST(
                     counter_rows - (SELECT count(*) FROM removed), 0
                 )
                 WHERE id = 0
                 RETURNING (SELECT count(*) FROM removed)::bigint AS deleted
             )
             SELECT deleted FROM adjusted",
        )
        .bind(chats)
        .bind(before)
        .fetch_one(&self.pool)
        .await?;
        nonnegative_counter("deleted idle member count", deleted)
    }

    pub async fn image_filters(&self, chat: i64) -> Result<Vec<ImageFilterRow>> {
        let rows: Vec<ImageFilterTuple> = sqlx::query_as(
            "SELECT name, vec, scale, cut, rate, live, samples, calibrated FROM image_filters
             WHERE chat_id = $1 ORDER BY name LIMIT 8",
        )
        .bind(chat)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Self::image_filter_row).collect())
    }

    pub async fn try_image_filter(&self, chat: i64, name: &str) -> Result<Option<ImageFilterRow>> {
        let row: Option<ImageFilterTuple> = sqlx::query_as(
            "SELECT name, vec, scale, cut, rate, live, samples, calibrated FROM image_filters
             WHERE chat_id = $1 AND name = $2",
        )
        .bind(chat)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Self::image_filter_row))
    }

    fn image_filter_row(row: ImageFilterTuple) -> ImageFilterRow {
        let (name, vector, scale, cut, rate, live, samples, calibrated) = row;
        ImageFilterRow {
            name,
            vector,
            scale,
            cut,
            rate: u32::try_from(rate).unwrap_or(0),
            live,
            samples: u32::try_from(samples).unwrap_or(0),
            calibrated,
        }
    }

    pub async fn try_normalize_fixed_image_filters(&self, cut: f32) -> Result<u64> {
        Ok(sqlx::query(
            "UPDATE image_filters
             SET cut = $1
             WHERE calibrated = TRUE AND samples = 0 AND live = TRUE AND cut <> $1",
        )
        .bind(cut)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    pub async fn try_fixed_image_filter_keys(&self) -> Result<Vec<(i64, String)>> {
        sqlx::query_as(
            "SELECT chat_id, name FROM image_filters
             WHERE calibrated = TRUE AND samples = 0 AND live = TRUE
             ORDER BY chat_id, name",
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn try_phrase_image_filter_keys(&self) -> Result<Vec<(i64, String)>> {
        sqlx::query_as(
            "SELECT chat_id, name FROM image_filters WHERE samples = 0 ORDER BY chat_id, name",
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn try_example_image_filter_keys(&self) -> Result<Vec<(i64, String)>> {
        sqlx::query_as(
            "SELECT chat_id, name FROM image_filters WHERE samples > 0 ORDER BY chat_id, name",
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn try_clear_samples(&self) -> Result<()> {
        sqlx::query("DELETE FROM calibration WHERE id = 0")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_samples(&self, vectors: &[u8], scale: f32, count: i32) -> Result<()> {
        sqlx::query(
            "INSERT INTO calibration (id, vecs, scale, count) VALUES (0, $1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET
                 vecs = EXCLUDED.vecs, scale = EXCLUDED.scale, count = EXCLUDED.count",
        )
        .bind(vectors)
        .bind(scale)
        .bind(count)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_samples(&self) -> Result<Option<(Vec<u8>, f32, i32)>> {
        sqlx::query_as("SELECT vecs, scale, count FROM calibration WHERE id = 0")
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn try_save_image_filter(
        &self,
        chat: i64,
        filter: ImageFilterWrite<'_>,
    ) -> Result<bool> {
        let ImageFilterWrite {
            name,
            vector,
            scale,
            cut,
            rate,
            live,
            samples,
            calibrated,
        } = filter;
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let existing: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM image_filters WHERE chat_id = $1 AND name = $2
             )",
        )
        .bind(chat)
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        if !existing {
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM image_filters WHERE chat_id = $1")
                    .bind(chat)
                    .fetch_one(&self.pool)
                    .await?;
            if count >= MAX_IMAGE_FILTERS_PER_CHAT {
                eprintln!(
                    "image filters: refusing new filter {chat}/{name}; limit is {MAX_IMAGE_FILTERS_PER_CHAT}"
                );
                return Ok(false);
            }
        }
        sqlx::query(
            "INSERT INTO image_filters              (chat_id, name, vec, scale, cut, rate, live, samples, calibrated)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (chat_id, name) DO UPDATE SET
                 vec = EXCLUDED.vec, scale = EXCLUDED.scale, cut = EXCLUDED.cut,
                 rate = EXCLUDED.rate, live = EXCLUDED.live, samples = EXCLUDED.samples,
                 calibrated = EXCLUDED.calibrated",
        )
        .bind(chat)
        .bind(name)
        .bind(vector)
        .bind(scale)
        .bind(cut)
        .bind(i32::try_from(rate).unwrap_or(i32::MAX))
        .bind(live)
        .bind(i32::try_from(samples).unwrap_or(i32::MAX))
        .bind(calibrated)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    pub async fn try_delete_image_filter(&self, chat: i64, name: &str) -> Result<()> {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        sqlx::query("DELETE FROM image_filters WHERE chat_id = $1 AND name = $2")
            .bind(chat)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub fn with_chat<T>(&self, chat: i64, f: impl FnOnce(ChatSettings<'_>) -> T) -> T {
        let cache = self.cache.read().unwrap();
        f(ChatSettings(cache.get(&chat)))
    }

    pub fn is_locked(&self, chat: i64, lock: &str) -> bool {
        self.with_chat(chat, |settings| settings.is_locked(lock))
    }

    pub async fn try_apply_batch(
        &self,
        chat: i64,
        mutations: &[SettingMutation<'_>],
    ) -> std::result::Result<usize, SettingsWriteError> {
        if !self.ownership.alive.load(Ordering::Acquire) {
            return Err(SettingsWriteError::UncertainState);
        }
        if mutations.is_empty() {
            return Ok(0);
        }

        let mut unique = HashSet::with_capacity(mutations.len());
        for mutation in mutations.iter().copied() {
            if mutation.key().len() > MAX_SETTING_KEY_BYTES {
                return Err(SettingsWriteError::KeyTooLarge);
            }
            match mutation {
                SettingMutation::Put { key, value }
                | SettingMutation::PutIfEmpty { key, value, .. } => {
                    if value.len() > MAX_SETTING_VALUE_BYTES {
                        return Err(SettingsWriteError::ValueTooLarge);
                    }
                    validate_persisted_setting(key, value)
                        .map_err(SettingsWriteError::InvalidValue)?;
                }
                SettingMutation::Delete { .. } => {}
            }
            if let SettingMutation::PutIfEmpty { condition_key, .. } = mutation
                && condition_key.len() > MAX_SETTING_KEY_BYTES
            {
                return Err(SettingsWriteError::KeyTooLarge);
            }
            if !unique.insert(mutation.key()) {
                return Err(SettingsWriteError::DuplicateKey);
            }
        }

        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let _capacity = self.capacity_write.lock().await;
        if !self.ownership.alive.load(Ordering::Acquire) {
            return Err(SettingsWriteError::UncertainState);
        }

        let (changes, old_rows, new_rows, old_bytes, new_bytes, requested_hash) = {
            let cache = self.cache.read().unwrap();
            let current = cache.get(&chat);
            let old_rows = current.map_or(0, HashMap::len);
            let mut changes = Vec::with_capacity(mutations.len());
            let mut removed_rows = 0usize;
            let mut added_rows = 0usize;
            let mut removed_bytes = 0usize;
            let mut added_bytes = 0usize;

            for mutation in mutations.iter().copied() {
                let key = mutation.key();
                let previous = current.and_then(|map| map.get(key)).cloned();
                let next = match mutation {
                    SettingMutation::Put { value, .. } => Some(value.to_owned()),
                    SettingMutation::PutIfEmpty {
                        value,
                        condition_key,
                        ..
                    } if current
                        .and_then(|map| map.get(condition_key))
                        .is_none_or(String::is_empty) =>
                    {
                        Some(value.to_owned())
                    }
                    SettingMutation::PutIfEmpty { .. } => previous.clone(),
                    SettingMutation::Delete { .. } => None,
                };
                if previous == next {
                    continue;
                }
                if let Some(value) = previous.as_deref() {
                    removed_rows = removed_rows.saturating_add(1);
                    removed_bytes = removed_bytes.saturating_add(setting_size(key, value));
                }
                if let Some(value) = next.as_deref() {
                    added_rows = added_rows.saturating_add(1);
                    added_bytes = added_bytes.saturating_add(setting_size(key, value));
                }
                changes.push(SettingChange {
                    key: key.to_owned(),
                    next,
                });
            }
            if changes.is_empty() {
                return Ok(0);
            }

            let new_rows = old_rows
                .saturating_sub(removed_rows)
                .saturating_add(added_rows);
            let global_rows = self
                .setting_rows
                .load(Ordering::Relaxed)
                .saturating_sub(removed_rows)
                .saturating_add(added_rows);
            let old_bytes = self.setting_bytes.load(Ordering::Relaxed);
            let new_bytes = old_bytes
                .saturating_sub(removed_bytes)
                .saturating_add(added_bytes);

            if new_rows > MAX_SETTINGS_ROWS_PER_CHAT
                || self.max_rows.is_some_and(|max| global_rows > max)
            {
                return Err(SettingsWriteError::CapacityReached("row"));
            }
            if self.max_bytes.is_some_and(|max| new_bytes > max) {
                return Err(SettingsWriteError::CapacityReached("byte"));
            }
            if !self.chat_is_configured(&cache, chat)
                && new_rows > 0
                && self
                    .max_chats
                    .is_some_and(|max| self.configured_chat_count(&cache) >= max)
            {
                return Err(SettingsWriteError::CapacityReached("chat"));
            }

            let requested_hash = match changes.iter().find(|change| change.key == "hash") {
                Some(change) => change.next.clone(),
                None => current.and_then(|map| map.get("hash")).cloned(),
            };
            (
                changes,
                old_rows,
                new_rows,
                old_bytes,
                new_bytes,
                requested_hash,
            )
        };

        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(SettingsWriteError::Database)?;
        let admitted_hash = if chat == 0 {
            None
        } else {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
                     VALUES ($1, COALESCE($2::BIGINT, 0), $3)
                     ON CONFLICT (chat_id) DO UPDATE SET
                       access_hash = CASE WHEN EXCLUDED.access_hash <> 0
                                          THEN EXCLUDED.access_hash
                                          ELSE durable_chats.access_hash END
                     RETURNING access_hash",
                )
                .bind(chat)
                .bind(requested_hash.as_deref())
                .bind(unix_now())
                .fetch_one(&mut *transaction)
                .await
                .map_err(SettingsWriteError::Database)?,
            )
        };
        let put_keys: Vec<&str> = changes
            .iter()
            .filter_map(|change| change.next.as_ref().map(|_| change.key.as_str()))
            .collect();
        let put_values: Vec<&str> = changes
            .iter()
            .filter_map(|change| change.next.as_deref())
            .collect();
        if !put_keys.is_empty() {
            sqlx::query(
                "INSERT INTO settings (chat_id, key, value)
                 SELECT $1, input.key, input.value
                 FROM UNNEST($2::TEXT[], $3::TEXT[]) AS input(key, value)
                 ON CONFLICT (chat_id, key) DO UPDATE SET value = EXCLUDED.value",
            )
            .bind(chat)
            .bind(&put_keys)
            .bind(&put_values)
            .execute(&mut *transaction)
            .await
            .map_err(SettingsWriteError::Database)?;
        }
        let deleted_keys: Vec<&str> = changes
            .iter()
            .filter(|change| change.next.is_none())
            .map(|change| change.key.as_str())
            .collect();
        if !deleted_keys.is_empty() {
            sqlx::query("DELETE FROM settings WHERE chat_id = $1 AND key = ANY($2::TEXT[])")
                .bind(chat)
                .bind(&deleted_keys)
                .execute(&mut *transaction)
                .await
                .map_err(SettingsWriteError::Database)?;
        }

        let mut commit = MirrorCommitGuard::new(&self.ownership);
        transaction
            .commit()
            .await
            .map_err(SettingsWriteError::CommitUncertain)?;

        {
            let mut cache = self.cache.write().unwrap();
            for change in &changes {
                match change.next.as_ref() {
                    Some(value) => {
                        cache
                            .entry(chat)
                            .or_default()
                            .insert(change.key.clone(), value.clone());
                    }
                    None => {
                        if let Some(settings) = cache.get_mut(&chat) {
                            settings.remove(&change.key);
                        }
                    }
                }
            }
            if chat != 0 && cache.get(&chat).is_some_and(HashMap::is_empty) {
                cache.remove(&chat);
            }
        }
        if new_rows >= old_rows {
            self.setting_rows
                .fetch_add(new_rows - old_rows, Ordering::Relaxed);
        } else {
            self.setting_rows
                .fetch_sub(old_rows - new_rows, Ordering::Relaxed);
        }
        if new_bytes >= old_bytes {
            self.setting_bytes
                .fetch_add(new_bytes - old_bytes, Ordering::Relaxed);
        } else {
            self.setting_bytes
                .fetch_sub(old_bytes - new_bytes, Ordering::Relaxed);
        }
        for change in &changes {
            self.reindex(chat, &change.key, change.next.is_some());
        }
        if let Some(hash) = admitted_hash {
            self.durable_chats.write().unwrap().insert(chat, hash);
        }
        commit.disarm();
        Ok(changes.len())
    }

    pub async fn try_set(
        &self,
        chat: i64,
        lock: &str,
        on: bool,
    ) -> std::result::Result<bool, SettingsWriteError> {
        let mutation = if on {
            SettingMutation::Put {
                key: lock,
                value: "",
            }
        } else {
            SettingMutation::Delete { key: lock }
        };
        Ok(self.try_apply_batch(chat, &[mutation]).await? != 0)
    }

    pub async fn set_flags(
        &self,
        chats: &[i64],
        key: &str,
        on: bool,
    ) -> std::result::Result<usize, SettingsWriteError> {
        if !self.ownership.alive.load(Ordering::Acquire) {
            return Err(SettingsWriteError::UncertainState);
        }
        if key.len() > MAX_SETTING_KEY_BYTES {
            return Err(SettingsWriteError::KeyTooLarge);
        }
        if on {
            validate_persisted_setting(key, "").map_err(SettingsWriteError::InvalidValue)?;
        }
        if chats.is_empty() {
            return Ok(0);
        }
        let mut ids = chats.to_vec();
        ids.sort_unstable();
        ids.dedup();

        let mut stripe_ids: Vec<usize> = ids
            .iter()
            .map(|chat| self.write_slot_index(*chat))
            .collect();
        stripe_ids.sort_unstable();
        stripe_ids.dedup();
        let mut _writing = Vec::with_capacity(stripe_ids.len());
        for stripe in stripe_ids {
            _writing.push(self.write_slots[stripe].lock().await);
        }
        let _capacity = self.capacity_write.lock().await;
        if !self.ownership.alive.load(Ordering::Acquire) {
            return Err(SettingsWriteError::UncertainState);
        }

        let mut changes: Vec<(i64, Option<String>)> = Vec::new();
        let mut db_ids = Vec::new();
        if on {
            let mut projected_rows = self.setting_rows.load(Ordering::Relaxed);
            let mut projected_bytes = self.setting_bytes.load(Ordering::Relaxed);
            let mut projected_chats = self.configured_chat_count(&self.cache.read().unwrap());
            let cache = self.cache.read().unwrap();
            for chat in ids.iter().copied() {
                let existing = cache.get(&chat).and_then(|map| map.get(key));
                if existing.is_some_and(String::is_empty) {
                    db_ids.push(chat);
                    continue;
                }
                if existing.is_none() {
                    let map_len = cache.get(&chat).map_or(0, HashMap::len);
                    let chat_is_new = !self.chat_is_configured(&cache, chat);
                    let chat_limit_reached =
                        chat_is_new && self.max_chats.is_some_and(|max| projected_chats >= max);
                    if map_len >= MAX_SETTINGS_ROWS_PER_CHAT
                        || self.max_rows.is_some_and(|max| projected_rows >= max)
                    {
                        return Err(SettingsWriteError::CapacityReached("row"));
                    }
                    if chat_limit_reached {
                        return Err(SettingsWriteError::CapacityReached("chat"));
                    }
                    let bytes = setting_size(key, "");
                    if self
                        .max_bytes
                        .is_some_and(|max| projected_bytes.saturating_add(bytes) > max)
                    {
                        return Err(SettingsWriteError::CapacityReached("byte"));
                    }
                    projected_rows = projected_rows.saturating_add(1);
                    projected_bytes = projected_bytes.saturating_add(bytes);
                    projected_chats = projected_chats.saturating_add(usize::from(chat_is_new));
                }
                changes.push((chat, existing.cloned()));
                db_ids.push(chat);
            }
        } else {
            let cache = self.cache.read().unwrap();
            changes.extend(ids.iter().filter_map(|chat| {
                cache
                    .get(chat)
                    .and_then(|map| map.get(key).cloned())
                    .map(|previous| (*chat, Some(previous)))
            }));
        }

        if !on {
            db_ids = ids.clone();
        }
        if db_ids.is_empty() {
            return Ok(0);
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(SettingsWriteError::Database)?;
        let admitted: Vec<(i64, i64)> = if on {
            sqlx::query_as(
                "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
                 SELECT requested.chat_id,
                        COALESCE((SELECT value::BIGINT FROM settings
                                  WHERE chat_id = requested.chat_id AND key = 'hash'), 0),
                        $2
                 FROM UNNEST($1::BIGINT[]) AS requested(chat_id)
                 WHERE requested.chat_id <> 0
                 ON CONFLICT (chat_id) DO UPDATE SET
                   access_hash = CASE WHEN EXCLUDED.access_hash <> 0
                                      THEN EXCLUDED.access_hash
                                      ELSE durable_chats.access_hash END
                 RETURNING chat_id, access_hash",
            )
            .bind(&db_ids)
            .bind(unix_now())
            .fetch_all(&mut *tx)
            .await
            .map_err(SettingsWriteError::Database)?
        } else {
            Vec::new()
        };
        if on {
            sqlx::query(
                "INSERT INTO settings (chat_id, key, value)
                 SELECT chat_id, $2, ''
                 FROM UNNEST($1::bigint[]) AS requested(chat_id)
                 ON CONFLICT (chat_id, key) DO UPDATE SET value = EXCLUDED.value",
            )
            .bind(&db_ids)
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(SettingsWriteError::Database)?;
        } else {
            sqlx::query("DELETE FROM settings WHERE key = $1 AND chat_id = ANY($2::bigint[])")
                .bind(key)
                .bind(&db_ids)
                .execute(&mut *tx)
                .await
                .map_err(SettingsWriteError::Database)?;
        }
        let mut commit = MirrorCommitGuard::new(&self.ownership);
        tx.commit()
            .await
            .map_err(SettingsWriteError::CommitUncertain)?;

        let mut changed = 0;
        {
            let mut cache = self.cache.write().unwrap();
            for (chat, _) in &changes {
                let chat = *chat;
                let map = cache.entry(chat).or_default();
                if on {
                    let old = map.insert(key.to_owned(), String::new());
                    let old_bytes = old
                        .as_deref()
                        .map(|value| setting_size(key, value))
                        .unwrap_or(0);
                    let new_bytes = setting_size(key, "");
                    if old.is_none() {
                        self.setting_rows.fetch_add(1, Ordering::Relaxed);
                    }
                    if new_bytes >= old_bytes {
                        self.setting_bytes
                            .fetch_add(new_bytes - old_bytes, Ordering::Relaxed);
                    } else {
                        self.setting_bytes
                            .fetch_sub(old_bytes - new_bytes, Ordering::Relaxed);
                    }
                    changed += usize::from(old.as_deref() != Some(""));
                } else if let Some(old) = map.remove(key) {
                    self.setting_rows.fetch_sub(1, Ordering::Relaxed);
                    self.setting_bytes
                        .fetch_sub(setting_size(key, &old), Ordering::Relaxed);
                    changed += 1;
                    if map.is_empty() && chat != 0 {
                        cache.remove(&chat);
                    }
                }
            }
        }
        for (chat, _) in &changes {
            self.reindex(*chat, key, on);
        }
        {
            let mut durable = self.durable_chats.write().unwrap();
            durable.extend(admitted);
        }
        commit.disarm();
        Ok(changed)
    }

    pub async fn remember_started_user(&self, user: i64, access_hash: i64) -> Result<()> {
        let slot = self.write_slot(0);
        let _writing = slot.lock().await;
        let now = unix_now();
        async {
            let mut tx = self.pool.begin().await?;
            let inserted: Option<i64> = sqlx::query_scalar(
                "INSERT INTO started_users (user_id, access_hash, last_started_at)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (user_id) DO NOTHING
                 RETURNING user_id",
            )
            .bind(user)
            .bind(access_hash)
            .bind(now)
            .fetch_optional(&mut *tx)
            .await?;
            if inserted.is_none() {
                sqlx::query(
                    "UPDATE started_users
                     SET access_hash = $2, last_started_at = $3
                     WHERE user_id = $1",
                )
                .bind(user)
                .bind(access_hash)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            } else {
                let total: i64 = sqlx::query_scalar(
                    "UPDATE started_users_meta
                     SET total = total + 1
                     WHERE id = 0
                     RETURNING total",
                )
                .fetch_one(&mut *tx)
                .await?;
                if total > MAX_STARTED_USERS {
                    let deleted = sqlx::query(
                        "WITH evict AS (
                             SELECT user_id FROM started_users
                             ORDER BY last_started_at, user_id
                             LIMIT $1
                         )
                         DELETE FROM started_users
                         WHERE user_id IN (SELECT user_id FROM evict)",
                    )
                    .bind(total - MAX_STARTED_USERS)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected();
                    sqlx::query(
                        "UPDATE started_users_meta
                         SET total = GREATEST(total - $1, 0)
                         WHERE id = 0",
                    )
                    .bind(saturating_postgres_i64(deleted))
                    .execute(&mut *tx)
                    .await?;
                }
            }
            tx.commit().await
        }
        .await?;
        Ok(())
    }

    pub async fn started_user(&self, user: i64) -> Result<Option<i64>> {
        let row =
            sqlx::query_as::<_, (i64,)>("SELECT access_hash FROM started_users WHERE user_id = $1")
                .bind(user)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(access_hash,)| access_hash))
    }

    fn write_slot_index(&self, chat: i64) -> usize {
        let hash = (chat as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let stripe_mask = u64::try_from(WRITE_SLOTS).expect("stripe count fits u64") - 1;
        usize::try_from(hash & stripe_mask).expect("write-stripe index fits usize")
    }

    fn write_slot(&self, chat: i64) -> &tokio::sync::Mutex<()> {
        self.write_slots[self.write_slot_index(chat)].as_ref()
    }

    pub fn indexed_empty(&self, chat: i64, prefix: &str) -> bool {
        let Some((slot, _)) = indexed_slot(prefix) else {
            return true;
        };
        self.index
            .read()
            .unwrap()
            .get(&chat)
            .is_none_or(|index| index[slot].is_empty())
    }

    pub fn indexed_any(&self, chat: i64, prefix: &str, predicate: impl Fn(&str) -> bool) -> bool {
        let Some((slot, _)) = indexed_slot(prefix) else {
            return false;
        };
        let index = self.index.read().unwrap();
        index
            .get(&chat)
            .is_some_and(|index| index[slot].iter().any(|key| predicate(key)))
    }

    fn reindex(&self, chat: i64, key: &str, present: bool) {
        let Some((slot, rest)) = indexed_slot(key) else {
            return;
        };
        let mut index = self.index.write().unwrap();
        let entry = index.entry(chat).or_default();
        let at = entry[slot].iter().position(|known| &**known == rest);
        match (present, at) {
            (true, None) => entry[slot].push(rest.into()),
            (false, Some(at)) => {
                entry[slot].swap_remove(at);
            }
            _ => {}
        }
        if entry.iter().all(Vec::is_empty) {
            index.remove(&chat);
        }
    }

    pub fn flags_with_prefix(&self, chat: i64, prefix: &str) -> Vec<String> {
        let cache = self.cache.read().unwrap();
        let Some(map) = cache.get(&chat) else {
            return Vec::new();
        };
        let mut found: Vec<String> = map
            .iter()
            .filter(|(_, value)| value.is_empty())
            .filter_map(|(key, _)| key.strip_prefix(prefix).map(str::to_owned))
            .collect();
        found.sort_unstable();
        found
    }

    #[cfg(test)]
    pub async fn add_tallies(&self, rows: &[(i64, &'static str, u64)], day: u64) -> Result<()> {
        const CHUNK: usize = 1_000;
        const AT_ONCE: usize = 4;
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
        let mut tasks = tokio::task::JoinSet::new();
        let mut failure = None;
        let mut reap = |done: std::result::Result<Result<()>, tokio::task::JoinError>| match done {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
            Err(error) => {
                if failure.is_none() {
                    failure = Some(sqlx::Error::Protocol(format!(
                        "tally batch task failed: {error}"
                    )));
                }
            }
        };
        for batch in rows.chunks(CHUNK) {
            let (mut chats, mut counters, mut added) = (Vec::new(), Vec::new(), Vec::new());
            for (chat, counter, count) in batch {
                chats.push(*chat);
                counters.push((*counter).to_owned());
                added.push(saturating_postgres_i64(*count));
            }
            while let Some(done) = tasks.try_join_next() {
                reap(done);
            }

            let permit = loop {
                let acquire = std::sync::Arc::clone(&permits).acquire_owned();
                if tasks.is_empty() {
                    break acquire.await.expect("the tally semaphore is never closed");
                }
                tokio::select! {
                    permit = acquire => {
                        break permit.expect("the tally semaphore is never closed");
                    }
                    done = tasks.join_next() => {
                        if let Some(done) = done {
                            reap(done);
                        }
                    }
                }
            };
            let pool = self.pool.clone();
            tasks.spawn(async move {
                let _permit = permit;
                sqlx::query(
                    "INSERT INTO tallies (chat_id, counter, day, count)
                     SELECT chat, name, $4, added
                     FROM UNNEST($1::bigint[], $2::text[], $3::bigint[]) AS batch(chat, name, added)
                     ON CONFLICT (chat_id, counter) DO UPDATE SET
                         count = CASE WHEN tallies.day = EXCLUDED.day
                                      THEN tallies.count ELSE 0 END + EXCLUDED.count,
                         day   = EXCLUDED.day",
                )
                .bind(&chats)
                .bind(&counters)
                .bind(&added)
                .bind(saturating_postgres_i64(day))
                .execute(&pool)
                .await?;
                Ok(())
            });
        }
        while let Some(done) = tasks.join_next().await {
            reap(done);
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub async fn tallies(&self, chat: i64, day: u64) -> Result<HashMap<String, u64>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT counter, count FROM tallies WHERE chat_id = $1 AND day = $2 AND count > 0",
        )
        .bind(chat)
        .bind(saturating_postgres_i64(day))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(counter, count)| {
                let count = nonnegative_counter(&format!("tally {counter}"), count)?;
                Ok((counter, count))
            })
            .collect()
    }

    pub async fn chats_with(&self, key: &str) -> Result<Vec<i64>> {
        let rows: Vec<(i64,)> =
            sqlx::query_as("SELECT chat_id FROM settings WHERE key = $1 AND value <> ''")
                .bind(key)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(chat,)| chat).collect())
    }

    pub async fn chats_with_values(&self, key: &str, values: &[String]) -> Result<Vec<i64>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT chat_id FROM settings
             WHERE key = $1 AND value = ANY($2::text[]) AND value <> ''",
        )
        .bind(key)
        .bind(values)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(chat,)| chat).collect())
    }

    pub async fn cleaner_recommendations_due(&self, minutes: &[i64]) -> Result<Vec<i64>> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT chat_id FROM settings
             WHERE key = 'hash' AND chat_id < 0 AND value <> ''
               AND (-(chat_id % 360)) = ANY($1::bigint[])",
        )
        .bind(minutes)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(chat,)| chat).collect())
    }

    pub async fn incomplete_setups(&self, include_cleaner: bool) -> Result<Vec<i64>> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT h.chat_id FROM settings h
             WHERE h.key = 'hash' AND h.chat_id < 0 AND h.value <> ''
               AND (NOT EXISTS (SELECT 1 FROM settings o
                   WHERE o.chat_id = h.chat_id AND o.key = 'owner' AND o.value <> '')
                   OR ($1 AND NOT EXISTS (SELECT 1 FROM settings c
                       WHERE c.chat_id = h.chat_id AND c.key = 'cln_added')))",
        )
        .bind(include_cleaner)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(chat,)| chat).collect())
    }

    pub async fn fleet(&self, day: u64) -> Result<Fleet> {
        let (chats, configured, members, active, today, total): (i64, i64, i64, i64, i64, i64) =
            sqlx::query_as(
                "SELECT
               (SELECT count(*) FROM (SELECT chat_id FROM settings WHERE chat_id < 0
                                      UNION SELECT chat_id FROM counters) t),
               (SELECT count(*) FROM settings WHERE key = 'owner'),
               (SELECT count(*) FROM counters),
               (SELECT count(DISTINCT chat_id) FROM counters WHERE day = $1 AND today > 0),
               -- `sum` over a bigint answers `numeric`, which does not decode as an i64.
               -- The cast is the fix and it cannot overflow: both sides are bigints already.
               (SELECT coalesce(sum(today), 0)::bigint FROM counters WHERE day = $1),
               (SELECT coalesce(sum(total), 0)::bigint FROM counters)",
            )
            .bind(saturating_postgres_i64(day))
            .fetch_one(&self.pool)
            .await?;
        Ok(Fleet {
            chats: nonnegative_counter("fleet chat count", chats)?,
            configured: nonnegative_counter("configured chat count", configured)?,
            members: nonnegative_counter("fleet member count", members)?,
            active_today: nonnegative_counter("active chat count", active)?,
            messages_today: nonnegative_counter("fleet today sum", today)?,
            messages_total: nonnegative_counter("fleet total sum", total)?,
        })
    }

    pub async fn busiest(&self, day: u64, limit: i64) -> Result<Vec<(i64, u64)>> {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT chat_id, sum(today)::bigint FROM counters
              WHERE day = $1 GROUP BY chat_id HAVING sum(today) > 0
              ORDER BY 2 DESC LIMIT $2",
        )
        .bind(saturating_postgres_i64(day))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(chat, count)| Ok((chat, nonnegative_counter("busiest sum", count)?)))
            .collect()
    }

    pub async fn adoption(&self, keys: &[&str]) -> Result<HashMap<String, u64>> {
        let asked: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT key, count(*) FROM settings WHERE key = ANY($1) GROUP BY key")
                .bind(&asked)
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|(key, count)| Ok((key, nonnegative_counter("adoption count", count)?)))
            .collect()
    }

    pub async fn badge_rows(&self, limit: i64) -> Result<Vec<(i64, i64)>> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT chat_id, key FROM settings
             WHERE key LIKE 'badge:%' ORDER BY chat_id, key LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(chat, key)| {
                let user = key
                    .strip_prefix("badge:")
                    .ok_or_else(|| {
                        sqlx::Error::Protocol(format!("invalid badge setting key {key:?}"))
                    })?
                    .parse::<i64>()
                    .map_err(|error| {
                        sqlx::Error::Protocol(format!(
                            "invalid badge user id in setting key {key:?}: {error}"
                        ))
                    })?;
                Ok((chat, user))
            })
            .collect()
    }

    pub async fn flagged_with(&self, flag: &str) -> Result<Vec<i64>> {
        let rows: Vec<(i64,)> = sqlx::query_as("SELECT chat_id FROM settings WHERE key = $1")
            .bind(flag)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(chat,)| chat).collect())
    }

    pub async fn panels_for(&self, user: i64, limit: i64) -> Result<Vec<i64>> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT chat_id FROM settings WHERE key LIKE 'admin:%' AND key = $1
             UNION
             SELECT chat_id FROM settings WHERE key = 'owner' AND value = $2
             LIMIT $3",
        )
        .bind(format!("admin:{user}"))
        .bind(user.to_string())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(chat,)| chat).collect())
    }

    pub async fn ping(&self) -> Result<std::time::Duration> {
        let started = std::time::Instant::now();
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(started.elapsed())
    }

    pub async fn create_moderation_case(&self, draft: NewModerationCase) -> Result<i64> {
        let now = unix_now();
        let mut tx = self.pool.begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO moderation_cases
             (chat_id, subject_user_id, subject_name, source, rule_key, reason, message_id,
              media_kind, evidence_text, evidence_hash, primary_action, action_until, status,
              actor_id, actor_name, created_at, updated_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$16)
             RETURNING id",
        )
        .bind(draft.chat)
        .bind(draft.subject)
        .bind(&draft.subject_name)
        .bind(&draft.source)
        .bind(&draft.rule)
        .bind(&draft.reason)
        .bind(draft.message)
        .bind(&draft.media_kind)
        .bind(&draft.evidence)
        .bind(&draft.evidence_hash)
        .bind(&draft.action)
        .bind(draft.action_until)
        .bind(&draft.status)
        .bind(draft.actor)
        .bind(&draft.actor_name)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO moderation_case_events
             (chat_id, case_id, kind, actor_id, actor_name, action, note, created_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(draft.chat)
        .bind(id)
        .bind(&draft.event_kind)
        .bind(draft.actor)
        .bind(&draft.actor_name)
        .bind(&draft.action)
        .bind(&draft.event_note)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn create_workflow_moderation_case(
        &self,
        workflow_key: &str,
        draft: NewModerationCase,
    ) -> Result<WorkflowModerationCase> {
        let now = unix_now();
        let mut tx = self.pool.begin().await?;
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO moderation_cases
             (chat_id, subject_user_id, subject_name, source, rule_key, reason, message_id,
              media_kind, evidence_text, evidence_hash, primary_action, action_until, status,
              actor_id, actor_name, created_at, updated_at, workflow_key)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$16,$17)
             ON CONFLICT (chat_id, workflow_key) WHERE workflow_key IS NOT NULL DO NOTHING
             RETURNING id",
        )
        .bind(draft.chat)
        .bind(draft.subject)
        .bind(&draft.subject_name)
        .bind(&draft.source)
        .bind(&draft.rule)
        .bind(&draft.reason)
        .bind(draft.message)
        .bind(&draft.media_kind)
        .bind(&draft.evidence)
        .bind(&draft.evidence_hash)
        .bind(&draft.action)
        .bind(draft.action_until)
        .bind(&draft.status)
        .bind(draft.actor)
        .bind(&draft.actor_name)
        .bind(now)
        .bind(workflow_key)
        .fetch_optional(&mut *tx)
        .await?;
        let was_inserted = inserted.is_some();
        let id = match inserted {
            Some(id) => {
                sqlx::query(
                    "INSERT INTO moderation_case_events
                     (chat_id, case_id, kind, actor_id, actor_name, action, note, created_at)
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
                )
                .bind(draft.chat)
                .bind(id)
                .bind(&draft.event_kind)
                .bind(draft.actor)
                .bind(&draft.actor_name)
                .bind(&draft.action)
                .bind(&draft.event_note)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                id
            }
            None => {
                sqlx::query_scalar(
                    "SELECT id FROM moderation_cases WHERE chat_id = $1 AND workflow_key = $2",
                )
                .bind(draft.chat)
                .bind(workflow_key)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        tx.commit().await?;
        Ok(WorkflowModerationCase {
            id,
            inserted: was_inserted,
        })
    }

    pub async fn moderation_cases(
        &self,
        chat: i64,
        status: Option<&str>,
        subject: Option<i64>,
        before: Option<i64>,
        limit: i64,
    ) -> Result<Vec<ModerationCase>> {
        let rows = sqlx::query(
            "SELECT id, chat_id, subject_user_id, subject_name, source, rule_key, reason,
                    message_id, media_kind, evidence_text, primary_action,
                    action_until, status, actor_id, actor_name, created_at, updated_at
             FROM moderation_cases
             WHERE chat_id = $1
               AND ($2::TEXT IS NULL OR status = $2)
               AND ($3::BIGINT IS NULL OR subject_user_id = $3)
               AND ($4::BIGINT IS NULL OR id < $4)
             ORDER BY id DESC
             LIMIT $5",
        )
        .bind(chat)
        .bind(status)
        .bind(subject)
        .bind(before)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(moderation_case_from_row).collect())
    }

    pub async fn moderation_cases_open_first(
        &self,
        chat: i64,
        subject: Option<i64>,
        limit: i64,
    ) -> Result<Vec<ModerationCase>> {
        let rows = sqlx::query(
            "SELECT id, chat_id, subject_user_id, subject_name, source, rule_key, reason,
                    message_id, media_kind, evidence_text, primary_action,
                    action_until, status, actor_id, actor_name, created_at, updated_at
             FROM moderation_cases
             WHERE chat_id = $1
               AND ($2::BIGINT IS NULL OR subject_user_id = $2)
             ORDER BY CASE WHEN status = 'open' THEN 0 ELSE 1 END, id DESC
             LIMIT $3",
        )
        .bind(chat)
        .bind(subject)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(moderation_case_from_row).collect())
    }

    pub async fn moderation_case(
        &self,
        chat: i64,
        id: i64,
    ) -> Result<Option<ModerationCaseDetail>> {
        let Some(row) = sqlx::query(
            "SELECT id, chat_id, subject_user_id, subject_name, source, rule_key, reason,
                    message_id, media_kind, evidence_text, primary_action,
                    action_until, status, actor_id, actor_name, created_at, updated_at
             FROM moderation_cases WHERE chat_id = $1 AND id = $2",
        )
        .bind(chat)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let events = sqlx::query(
            "SELECT id, case_id, kind, actor_id, actor_name, action, note, created_at
             FROM moderation_case_events WHERE chat_id = $1 AND case_id = $2 ORDER BY id",
        )
        .bind(chat)
        .bind(id)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(moderation_event_from_row)
        .collect();
        Ok(Some(ModerationCaseDetail {
            case: moderation_case_from_row(&row),
            events,
        }))
    }

    pub async fn transition_moderation_case(
        &self,
        transition: ModerationCaseTransition<'_>,
    ) -> Result<bool> {
        let ModerationCaseTransition {
            chat,
            case_id,
            from,
            to,
            event_kind,
            actor,
            action,
            note,
        } = transition;
        let now = unix_now();
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query(
            "UPDATE moderation_cases SET status = $1, updated_at = $2
             WHERE chat_id = $3 AND id = $4 AND status = $5",
        )
        .bind(to)
        .bind(now)
        .bind(chat)
        .bind(case_id)
        .bind(from)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if changed {
            sqlx::query(
                "INSERT INTO moderation_case_events
                 (chat_id, case_id, kind, actor_id, actor_name, action, note, created_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
            )
            .bind(chat)
            .bind(case_id)
            .bind(event_kind)
            .bind(actor.map(|value| value.0))
            .bind(actor.map_or("", |value| value.1))
            .bind(action)
            .bind(note)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn reverse_warning_case(
        &self,
        reversal: WarningCaseReversal<'_>,
    ) -> Result<WarningCaseReversalResult> {
        let WarningCaseReversal {
            chat,
            case_id,
            user,
            actor,
            actor_name,
            note,
        } = reversal;
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let mut tx = self.pool.begin().await?;
        let count: Option<i64> = sqlx::query_scalar(
            "SELECT warns FROM counters
             WHERE chat_id = $1 AND user_id = $2
             FOR UPDATE",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
        if count.is_none_or(|count| count <= 0) {
            tx.rollback().await?;
            return Ok(WarningCaseReversalResult::NoWarning);
        }
        let now = unix_now();
        let changed = sqlx::query(
            "UPDATE moderation_cases SET status = 'reversed', updated_at = $1
             WHERE chat_id = $2 AND id = $3 AND status = 'resolved'
               AND primary_action = 'warn' AND subject_user_id = $4",
        )
        .bind(now)
        .bind(chat)
        .bind(case_id)
        .bind(user)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if !changed {
            tx.rollback().await?;
            return Ok(WarningCaseReversalResult::CaseChanged);
        }
        sqlx::query(
            "UPDATE counters SET warns = warns - 1
             WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO moderation_case_events
             (chat_id, case_id, kind, actor_id, actor_name, action, note, created_at)
             VALUES ($1, $2, 'reversed', $3, $4, 'reverse', $5, $6)",
        )
        .bind(chat)
        .bind(case_id)
        .bind(actor)
        .bind(actor_name)
        .bind(note)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(WarningCaseReversalResult::Reversed)
    }

    pub async fn add_moderation_case_note(
        &self,
        chat: i64,
        id: i64,
        actor: Option<i64>,
        actor_name: &str,
        note: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "INSERT INTO moderation_case_events
             (chat_id, case_id, kind, actor_id, actor_name, note, created_at)
             SELECT chat_id, id, 'note', $3, $4, $5, $6 FROM moderation_cases
             WHERE chat_id = $1 AND id = $2",
        )
        .bind(chat)
        .bind(id)
        .bind(actor)
        .bind(actor_name)
        .bind(note)
        .bind(unix_now())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn append_moderation_case_event(
        &self,
        event: NewModerationCaseEvent<'_>,
    ) -> Result<bool> {
        let NewModerationCaseEvent {
            chat,
            case_id,
            kind,
            actor,
            action,
            note,
        } = event;
        let result = sqlx::query(
            "INSERT INTO moderation_case_events
             (chat_id, case_id, kind, actor_id, actor_name, action, note, created_at)
             SELECT chat_id, id, $3, $4, $5, $6, $7, $8 FROM moderation_cases
             WHERE chat_id = $1 AND id = $2",
        )
        .bind(chat)
        .bind(case_id)
        .bind(kind)
        .bind(actor.map(|value| value.0))
        .bind(actor.map_or("", |value| value.1))
        .bind(action)
        .bind(note)
        .bind(unix_now())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn purge_expired_moderation_cases(&self) -> Result<u64> {
        let cutoff = unix_now().saturating_sub(CASE_RETENTION_SECS);
        let result = sqlx::query(
            "DELETE FROM moderation_cases WHERE (chat_id, id) IN
             (SELECT chat_id, id FROM moderation_cases
              WHERE created_at < $1 ORDER BY created_at, chat_id, id LIMIT 10000)",
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub fn pool_stats(&self) -> (u32, usize) {
        (self.pool.size(), self.pool.num_idle())
    }

    pub async fn durable_counts(&self) -> Result<(i64, i64)> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT counter_rows, note_rows FROM durable_counts WHERE id = 0",
        )
        .fetch_one(&self.pool)
        .await
    }

    pub async fn note(&self, chat: i64, user: i64) -> Result<Option<String>> {
        sqlx::query_as::<_, (String,)>(
            "SELECT value FROM notes WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(|(value,)| value))
    }

    pub async fn set_note(
        &self,
        chat: i64,
        user: i64,
        value: &str,
    ) -> std::result::Result<(), NoteWriteError> {
        if value.len() > MAX_SETTING_VALUE_BYTES {
            return Err(NoteWriteError::ValueTooLarge);
        }
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let max_note_rows = self.max_note_rows.unwrap_or(i64::MAX / 2);
        async {
            let mut tx = self.pool.begin().await.map_err(NoteWriteError::Database)?;
            if value.is_empty() {
                let deleted = sqlx::query("DELETE FROM notes WHERE chat_id = $1 AND user_id = $2")
                    .bind(chat)
                    .bind(user)
                    .execute(&mut *tx)
                    .await
                    .map_err(NoteWriteError::Database)?
                    .rows_affected();
                if deleted > 0 {
                    sqlx::query(
                        "UPDATE durable_counts
                         SET note_rows = GREATEST(note_rows - $1, 0)
                         WHERE id = 0",
                    )
                    .bind(saturating_postgres_i64(deleted))
                    .execute(&mut *tx)
                    .await
                    .map_err(NoteWriteError::Database)?;
                }
            } else {
                let existing: bool = sqlx::query_scalar(
                    "SELECT EXISTS (
                         SELECT 1 FROM notes WHERE chat_id = $1 AND user_id = $2
                     )",
                )
                .bind(chat)
                .bind(user)
                .fetch_one(&mut *tx)
                .await
                .map_err(NoteWriteError::Database)?;
                if !existing {
                    let count: i64 =
                        sqlx::query_scalar("SELECT count(*) FROM notes WHERE chat_id = $1")
                            .bind(chat)
                            .fetch_one(&mut *tx)
                            .await
                            .map_err(NoteWriteError::Database)?;
                    if count >= MAX_NOTE_ROWS_PER_CHAT {
                        return Err(NoteWriteError::CapacityReached("per-chat"));
                    }
                    let reserved: Option<(i16,)> = sqlx::query_as(
                        "UPDATE durable_counts
                         SET note_rows = note_rows + 1
                         WHERE id = 0 AND note_rows < $1
                         RETURNING id",
                    )
                    .bind(max_note_rows)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(NoteWriteError::Database)?;
                    if reserved.is_none() {
                        return Err(NoteWriteError::CapacityReached("shard"));
                    }
                }
                sqlx::query(
                    "INSERT INTO notes (chat_id, user_id, value) VALUES ($1, $2, $3)
                     ON CONFLICT (chat_id, user_id) DO UPDATE SET value = EXCLUDED.value",
                )
                .bind(chat)
                .bind(user)
                .bind(value)
                .execute(&mut *tx)
                .await
                .map_err(NoteWriteError::Database)?;
            }
            tx.commit().await.map_err(NoteWriteError::Database)
        }
        .await
    }

    pub fn chats(&self) -> Vec<i64> {
        self.durable_chats.read().unwrap().keys().copied().collect()
    }

    pub fn chat_count(&self) -> usize {
        self.durable_chats.read().unwrap().len()
    }

    fn configured_chat_count(&self, _cache: &HashMap<i64, HashMap<String, String>>) -> usize {
        self.durable_chats.read().unwrap().len()
    }

    fn chat_is_configured(
        &self,
        _cache: &HashMap<i64, HashMap<String, String>>,
        chat: i64,
    ) -> bool {
        chat == 0 || self.durable_chats.read().unwrap().contains_key(&chat)
    }

    pub fn durable_chat_hash(&self, chat: i64) -> Option<i64> {
        self.durable_chats.read().unwrap().get(&chat).copied()
    }

    pub fn setting_count(&self) -> usize {
        self.setting_rows.load(Ordering::Relaxed)
    }

    pub fn setting_bytes(&self) -> usize {
        self.setting_bytes.load(Ordering::Relaxed)
    }

    pub fn values_with_prefix(&self, chat: i64, prefix: &str) -> Vec<(String, String)> {
        let cache = self.cache.read().unwrap();
        let Some(map) = cache.get(&chat) else {
            return Vec::new();
        };
        map.iter()
            .filter(|(_, value)| !value.is_empty())
            .filter_map(|(key, value)| Some((key.strip_prefix(prefix)?.to_owned(), value.clone())))
            .collect()
    }

    pub fn value(&self, chat: i64, key: &str) -> Option<String> {
        self.with_chat(chat, |settings| settings.value(key).map(str::to_owned))
    }

    pub fn try_value_parsed<T: std::str::FromStr>(
        &self,
        chat: i64,
        key: &str,
    ) -> std::result::Result<Option<T>, CorruptSetting> {
        self.with_chat(chat, |settings| settings.parsed(key))
    }

    pub fn value_parsed<T: std::str::FromStr>(&self, chat: i64, key: &str) -> Option<T> {
        assert!(
            VALIDATED_SETTING_KEYS.contains(&key),
            "infallible typed getter must enroll its key in persisted validation"
        );
        self.try_value_parsed(chat, key)
            .expect("settings mirror contains only validated typed policy")
    }

    pub async fn try_set_value(
        &self,
        chat: i64,
        key: &str,
        value: &str,
    ) -> std::result::Result<bool, SettingsWriteError> {
        Ok(self
            .try_apply_batch(chat, &[SettingMutation::Put { key, value }])
            .await?
            != 0)
    }

    pub async fn import_file(&self, path: &str) -> std::result::Result<usize, SettingsImportError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(source) => {
                return Err(SettingsImportError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        for (line_index, line) in text.lines().enumerate() {
            let mut parts = line.split_whitespace();
            let Some(chat_text) = parts.next() else {
                continue;
            };
            chat_text
                .parse::<i64>()
                .map_err(|_| SettingsImportError::Malformed {
                    line: line_index + 1,
                    reason: "chat id is not a signed 64-bit integer",
                })?;
            for part in parts {
                if part.starts_with('=') {
                    return Err(SettingsImportError::Malformed {
                        line: line_index + 1,
                        reason: "setting key is empty",
                    });
                }
                let (key, value) = part.split_once('=').unwrap_or((part, ""));
                if key.len() > MAX_SETTING_KEY_BYTES {
                    return Err(SettingsImportError::Write {
                        line: line_index + 1,
                        source: SettingsWriteError::KeyTooLarge,
                    });
                }
                if value.len() > MAX_SETTING_VALUE_BYTES {
                    return Err(SettingsImportError::Write {
                        line: line_index + 1,
                        source: SettingsWriteError::ValueTooLarge,
                    });
                }
                validate_persisted_setting(key, value).map_err(|source| {
                    SettingsImportError::InvalidValue {
                        line: line_index + 1,
                        source,
                    }
                })?;
            }
        }
        let mut imported = 0;
        for (line_index, line) in text.lines().enumerate() {
            let mut parts = line.split_whitespace();
            let Some(chat_text) = parts.next() else {
                continue;
            };
            let chat = chat_text
                .parse::<i64>()
                .map_err(|_| SettingsImportError::Malformed {
                    line: line_index + 1,
                    reason: "chat id is not a signed 64-bit integer",
                })?;
            for part in parts {
                if part.starts_with('=') {
                    return Err(SettingsImportError::Malformed {
                        line: line_index + 1,
                        reason: "setting key is empty",
                    });
                }
                let result = match part.split_once('=') {
                    Some((key, value)) => self.try_set_value(chat, key, value).await,
                    None => self.try_set(chat, part, true).await,
                };
                result.map_err(|source| SettingsImportError::Write {
                    line: line_index + 1,
                    source,
                })?;
                imported += 1;
            }
        }
        if imported > 0 {
            let imported_path = format!("{path}.imported");
            std::fs::rename(path, &imported_path).map_err(|source| {
                SettingsImportError::Rename {
                    from: path.to_owned(),
                    to: imported_path,
                    source,
                }
            })?;
        }
        Ok(imported)
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn warning_count(value: i64) -> Result<u32> {
    u32::try_from(value).map_err(|_| {
        sqlx::Error::Protocol(format!(
            "corrupt punishment counter {value}; expected 0..={}",
            u32::MAX
        ))
    })
}

fn moderation_case_from_row(row: &sqlx::postgres::PgRow) -> ModerationCase {
    ModerationCase {
        id: row.get("id"),
        chat: row.get("chat_id"),
        subject: row.get("subject_user_id"),
        subject_name: row.get("subject_name"),
        source: row.get("source"),
        rule: row.get("rule_key"),
        reason: row.get("reason"),
        message: row.get("message_id"),
        media_kind: row.get("media_kind"),
        evidence: row.get("evidence_text"),
        action: row.get("primary_action"),
        action_until: row.get("action_until"),
        status: row.get("status"),
        actor: row.get("actor_id"),
        actor_name: row.get("actor_name"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn moderation_event_from_row(row: &sqlx::postgres::PgRow) -> ModerationCaseEvent {
    ModerationCaseEvent {
        id: row.get("id"),
        case_id: row.get("case_id"),
        kind: row.get("kind"),
        actor: row.get("actor_id"),
        actor_name: row.get("actor_name"),
        action: row.get("action"),
        note: row.get("note"),
        created_at: row.get("created_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn admit_test_chats(settings: &Settings, chats: &[i64]) {
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             SELECT chat_id, 0, 0 FROM UNNEST($1::bigint[]) AS admitted(chat_id)
             ON CONFLICT (chat_id) DO NOTHING",
        )
        .bind(chats)
        .execute(&settings.pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL cleaner-recommendation query test"]
    async fn cleaner_recommendations_select_only_due_known_groups() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chats = [-999_999_999_861i64, -999_999_999_860, -999_999_999_859];
        let wipe = "DELETE FROM settings WHERE chat_id = ANY($1)";
        sqlx::query(wipe)
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
        settings
            .try_set_value(chats[0], "hash", "123")
            .await
            .unwrap();
        settings
            .try_set_value(chats[1], "hash", "456")
            .await
            .unwrap();
        settings
            .try_set_value(chats[2], "owner", "42")
            .await
            .unwrap();
        settings.try_set(chats[0], "cln_added", true).await.unwrap();
        let pending = settings.incomplete_setups(false).await.unwrap();
        assert!(pending.contains(&chats[0]));
        assert!(pending.contains(&chats[1]));
        assert!(!pending.contains(&chats[2]));
        settings
            .try_set_value(chats[0], "owner", "42")
            .await
            .unwrap();
        assert!(
            !settings
                .incomplete_setups(false)
                .await
                .unwrap()
                .contains(&chats[0])
        );
        settings
            .try_set_value(chats[1], "owner", "42")
            .await
            .unwrap();
        assert!(
            !settings
                .incomplete_setups(false)
                .await
                .unwrap()
                .contains(&chats[1])
        );
        let pending = settings.incomplete_setups(true).await.unwrap();
        assert!(
            pending.contains(&chats[1]),
            "configured but cleaner never joined"
        );
        assert!(
            !pending.contains(&chats[0]),
            "do not undo intentional cleaner removal"
        );
        assert!(!pending.contains(&chats[2]));
        let bucket = -(chats[0] % 360);
        let due = settings
            .cleaner_recommendations_due(&[bucket])
            .await
            .unwrap();
        assert!(due.contains(&chats[0]));
        assert!(!due.contains(&chats[1]));
        let all = settings
            .cleaner_recommendations_due(&(0..360).collect::<Vec<_>>())
            .await
            .unwrap();
        assert!(!all.contains(&chats[2]));
        assert!(
            settings
                .cleaner_recommendations_due(&[])
                .await
                .unwrap()
                .is_empty()
        );
        settings.try_set(chats[0], "hash", false).await.unwrap();
        assert!(
            !settings
                .cleaner_recommendations_due(&[bucket])
                .await
                .unwrap()
                .contains(&chats[0])
        );
        sqlx::query(wipe)
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[test]
    fn typed_settings_distinguish_absence_from_corruption() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("low".to_owned(), "1".to_owned());
        map.insert("high".to_owned(), "999".to_owned());
        map.insert("fine".to_owned(), "7".to_owned());
        map.insert("empty".to_owned(), String::new());
        map.insert("words".to_owned(), "زیاد".to_owned());
        let settings = ChatSettings(Some(&map));

        assert_eq!(settings.number_checked("fine", 8, (2, 50)), Ok(7));
        assert_eq!(settings.number_checked("absent", 8, (2, 50)), Ok(8));
        assert!(settings.number_checked("low", 8, (2, 50)).is_err());
        assert!(settings.number_checked("high", 8, (2, 50)).is_err());
        assert!(settings.number_checked("empty", 8, (2, 50)).is_err());
        assert!(settings.number_checked("words", 8, (2, 50)).is_err());
        assert_eq!(ChatSettings(None).number_checked("fine", 8, (2, 50)), Ok(8));
    }

    #[test]
    fn destructive_and_identity_settings_have_domain_validation() {
        for (key, value) in [
            ("strict_time", "forever"),
            ("strict_limit", "0"),
            ("strict_action", "kick"),
            ("captcha_action", "ban"),
            ("warn_action", "kick"),
            ("add_required", "1001"),
            ("pin_kept", "-1"),
            ("owner", "0"),
            ("hash", "not-an-integer"),
            ("report_at", "1440"),
            ("auto_purge_at", "-1"),
            ("night", "300|300"),
            ("night_state", "maybe"),
            ("glock_until", "never"),
        ] {
            let error = validate_persisted_setting(key, value).unwrap_err();
            assert_eq!(error.key(), key);
            assert_eq!(error.value(), value);
        }

        for (key, value) in [
            ("strict_time", "0"),
            ("strict_action", "mute"),
            ("captcha_action", "kick"),
            ("warn_action", "ban"),
            ("add_required", "1000"),
            ("pin_kept", "1"),
            ("owner", "42"),
            ("hash", "0"),
            ("report_at", "1439"),
            ("night", "1380|420"),
            ("night_state", "pending_off"),
            ("glock_until", "1"),
        ] {
            assert_eq!(validate_persisted_setting(key, value), Ok(()));
        }
    }

    #[test]
    fn configured_storage_limits_default_only_when_absent() {
        assert_eq!(parse_bounded_usize("ROWS", None, 8, 2, 10).unwrap(), 8);
        assert_eq!(parse_bounded_usize("ROWS", Some("7"), 8, 2, 10).unwrap(), 7);
        assert!(parse_bounded_usize("ROWS", Some("many"), 8, 2, 10).is_err());
        assert!(parse_bounded_usize("ROWS", Some("1"), 8, 2, 10).is_err());
        assert!(parse_bounded_usize("ROWS", Some("11"), 8, 2, 10).is_err());

        assert_eq!(parse_bounded_i64("ROWS", None, 8, 2, 10).unwrap(), 8);
        assert_eq!(parse_bounded_i64("ROWS", Some("2"), 8, 2, 10).unwrap(), 2);
        assert!(parse_bounded_i64("ROWS", Some("-1"), 8, 2, 10).is_err());
    }

    #[test]
    fn flags_and_values_share_the_map() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("links".to_owned(), String::new());
        map.insert("owner".to_owned(), "42".to_owned());
        assert!(map.get("links").is_some_and(|v| v.is_empty()));
        assert!(!map.get("owner").is_some_and(|v| v.is_empty()));
        assert_eq!(map.get("owner").map(String::as_str), Some("42"));
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL response-policy persistence and group-isolation test"]
    async fn response_policy_round_trips_resets_and_stays_group_scoped() {
        use crate::response::{ResponseKind, VisibilityOverride, policy_view, set_kind};

        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chats = [-999_999_999_970i64, -999_999_999_969i64];
        drop(Settings::connect(&url).await.unwrap());
        let control = PgPool::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&control)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_chats WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&control)
            .await
            .unwrap();

        let settings = Settings::connect(&url).await.unwrap();
        set_kind(
            &settings,
            chats[0],
            ResponseKind::ContentRemovalNotice,
            VisibilityOverride::Private,
        )
        .await
        .unwrap();
        set_kind(
            &settings,
            chats[0],
            ResponseKind::WelcomeNotice,
            VisibilityOverride::Private,
        )
        .await
        .unwrap();

        let first = policy_view(&settings, chats[0]);
        let second = policy_view(&settings, chats[1]);
        assert_eq!(
            first
                .overrides
                .iter()
                .find(|entry| entry.id == "content_removal_notice")
                .unwrap()
                .visibility,
            VisibilityOverride::Private
        );
        assert_eq!(
            first
                .overrides
                .iter()
                .find(|entry| entry.id == "welcome_notice")
                .unwrap()
                .visibility,
            VisibilityOverride::Private
        );
        assert!(
            second
                .overrides
                .iter()
                .all(|entry| entry.visibility == VisibilityOverride::Public)
        );

        crate::response::reset(&settings, chats[0]).await.unwrap();
        assert!(
            policy_view(&settings, chats[0])
                .overrides
                .iter()
                .all(|entry| entry.visibility == VisibilityOverride::Public)
        );
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM settings WHERE chat_id = $1 AND key LIKE 'response:%'",
        )
        .bind(chats[0])
        .fetch_one(&control)
        .await
        .unwrap();
        assert_eq!(remaining, 0);

        drop(settings);
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&control)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_chats WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&control)
            .await
            .unwrap();
    }

    #[test]
    fn an_ambiguous_commit_guard_closes_the_settings_gate() {
        let ownership = ownership::Ownership::default();
        {
            let _commit_in_progress = MirrorCommitGuard::new(&ownership);
        }
        assert!(
            !ownership.alive.load(Ordering::Acquire),
            "a cancelled or failed commit cannot leave an unverified mirror serving requests"
        );
        assert!(
            SettingsWriteError::CommitUncertain(sqlx::Error::PoolClosed).commit_outcome_unknown()
        );
        assert!(
            !SettingsWriteError::Database(sqlx::Error::PoolClosed).commit_outcome_unknown(),
            "a pre-commit database rejection must not be reported as possibly accepted"
        );
        assert!(
            !SettingsWriteError::UncertainState.commit_outcome_unknown(),
            "a later write rejected by the closed gate never reached PostgreSQL"
        );
    }

    #[test]
    fn production_code_cannot_return_to_swallowing_settings_wrappers() {
        fn rust_files(path: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(path).expect("source directory is readable") {
                let entry = entry.expect("source entry is readable");
                let path = entry.path();
                if path.is_dir() {
                    rust_files(&path, found);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    found.push(path);
                }
            }
        }

        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&source, &mut files);
        let flag_wrapper = [".settings", ".set("].concat();
        let value_wrapper = [".settings", ".set_value("].concat();
        for path in files {
            let text = std::fs::read_to_string(&path).expect("Rust source is UTF-8");
            let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
            assert!(
                !compact.contains(&flag_wrapper) && !compact.contains(&value_wrapper),
                "{} bypasses explicit settings error handling",
                path.display()
            );
        }
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL atomic settings-batch and capacity test"]
    async fn settings_batch_commits_all_changes_or_none_and_publishes_the_mirror() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_976i64;
        let blocked_chat = -999_999_999_975i64;
        let control = PgPool::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&[chat, blocked_chat][..])
            .execute(&control)
            .await
            .unwrap();

        let mut settings = Settings::connect(&url).await.unwrap();
        assert_eq!(
            settings
                .try_apply_batch(
                    chat,
                    &[
                        SettingMutation::Put {
                            key: "batch_a",
                            value: "one",
                        },
                        SettingMutation::Put {
                            key: "batch_b",
                            value: "two",
                        },
                    ],
                )
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            settings
                .try_apply_batch(
                    chat,
                    &[SettingMutation::PutIfEmpty {
                        key: "batch_guarded",
                        value: "default",
                        condition_key: "batch_b",
                    }],
                )
                .await
                .unwrap(),
            0
        );
        assert_eq!(settings.value(chat, "batch_guarded"), None);

        let corrupt_policy = settings.try_set_value(chat, "strict_time", "forever").await;
        assert!(matches!(
            corrupt_policy,
            Err(SettingsWriteError::InvalidValue(_))
        ));
        assert_eq!(settings.value(chat, "strict_time"), None);
        let corrupt_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM settings WHERE chat_id=$1 AND key='strict_time'",
        )
        .bind(chat)
        .fetch_one(&control)
        .await
        .unwrap();
        assert_eq!(corrupt_rows, 0, "invalid policy is rejected before SQL");

        settings.max_rows = Some(settings.setting_count());
        settings.max_bytes = Some(settings.setting_bytes());
        settings.max_chats = Some(settings.chat_count());
        assert_eq!(
            settings
                .try_apply_batch(
                    chat,
                    &[
                        SettingMutation::Delete { key: "batch_a" },
                        SettingMutation::Put {
                            key: "batch_c",
                            value: "one",
                        },
                    ],
                )
                .await
                .unwrap(),
            2,
            "a same-size replacement must be admitted at the exact row and byte ceilings"
        );
        assert_eq!(settings.value(chat, "batch_a"), None);
        assert_eq!(settings.value(chat, "batch_c").as_deref(), Some("one"));

        let rejected = settings
            .try_apply_batch(
                chat,
                &[
                    SettingMutation::Put {
                        key: "batch_d",
                        value: "four",
                    },
                    SettingMutation::Put {
                        key: "batch_e",
                        value: "five",
                    },
                ],
            )
            .await;
        assert!(matches!(
            rejected,
            Err(SettingsWriteError::CapacityReached(_))
        ));
        assert_eq!(settings.value(chat, "batch_d"), None);
        assert_eq!(settings.value(chat, "batch_e"), None);
        let refused_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM settings WHERE chat_id=$1 AND key=ANY($2::TEXT[])",
        )
        .bind(chat)
        .bind(&["batch_d", "batch_e"][..])
        .fetch_one(&control)
        .await
        .unwrap();
        assert_eq!(
            refused_rows, 0,
            "capacity rejection cannot partially commit"
        );

        let durable: Vec<(String, String)> = sqlx::query_as(
            "SELECT key,value FROM settings WHERE chat_id=$1 AND key LIKE 'batch_%' ORDER BY key",
        )
        .bind(chat)
        .fetch_all(&control)
        .await
        .unwrap();
        assert_eq!(
            durable,
            vec![
                ("batch_b".to_owned(), "two".to_owned()),
                ("batch_c".to_owned(), "one".to_owned()),
            ],
            "the post-commit mirror assertions above describe the same durable transaction"
        );

        let duplicate = settings
            .try_apply_batch(
                chat,
                &[
                    SettingMutation::Delete { key: "batch_b" },
                    SettingMutation::Put {
                        key: "batch_b",
                        value: "ambiguous",
                    },
                ],
            )
            .await;
        assert!(matches!(duplicate, Err(SettingsWriteError::DuplicateKey)));
        assert_eq!(settings.value(chat, "batch_b").as_deref(), Some("two"));

        settings.max_rows = None;
        settings.max_bytes = None;
        let new_chat = settings
            .try_set_value(blocked_chat, "batch_new", "value")
            .await;
        assert!(matches!(
            new_chat,
            Err(SettingsWriteError::CapacityReached("chat"))
        ));

        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&[chat, blocked_chat][..])
            .execute(&control)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL pre-commit cancellation probe"]
    async fn cancelling_a_batch_before_commit_keeps_the_gate_and_mirror_authoritative() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_974i64;
        let control = PgPool::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id=$1")
            .bind(chat)
            .execute(&control)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_chats WHERE chat_id=$1")
            .bind(chat)
            .execute(&control)
            .await
            .unwrap();
        let settings = std::sync::Arc::new(Settings::connect(&url).await.unwrap());
        let blocker = PgPool::connect(&url).await.unwrap();
        let mut transaction = blocker.begin().await.unwrap();
        sqlx::query("LOCK TABLE settings IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *transaction)
            .await
            .unwrap();

        let writing = std::sync::Arc::clone(&settings);
        let task = tokio::spawn(async move {
            writing
                .try_apply_batch(
                    chat,
                    &[SettingMutation::Put {
                        key: "cancelled_batch",
                        value: "must_not_land",
                    }],
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        transaction.rollback().await.unwrap();

        assert!(settings.ownership.alive.load(Ordering::Acquire));
        assert_eq!(settings.value(chat, "cancelled_batch"), None);
        let durable: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM settings
                     WHERE chat_id=$1 AND key='cancelled_batch'),
                    (SELECT count(*) FROM durable_chats WHERE chat_id=$1)",
        )
        .bind(chat)
        .fetch_one(&control)
        .await
        .unwrap();
        assert_eq!(
            durable,
            (0, 0),
            "cancelling the child write must roll its newly reserved owner back too"
        );
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL settings round-trip test"]
    async fn roundtrip() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_999;

        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_chats WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        let settings = Settings::connect(&url).await.unwrap();

        settings
            .try_set_value(chat, "hash", "123456789")
            .await
            .unwrap();
        let first_admission: (i64, String) = sqlx::query_as(
            "SELECT durable_chats.access_hash, settings.value
             FROM durable_chats JOIN settings
               ON settings.owner_chat_id = durable_chats.chat_id
             WHERE durable_chats.chat_id = $1 AND settings.key = 'hash'",
        )
        .bind(chat)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(first_admission, (123_456_789, "123456789".to_owned()));
        settings.try_set_value(chat, "owner", "42").await.unwrap();
        settings.try_set_value(chat, "owner", "7").await.unwrap();
        assert_eq!(settings.value(chat, "owner").as_deref(), Some("7"));
        settings
            .try_set_value(chat, "value_to_flag", "old")
            .await
            .unwrap();
        assert!(settings.try_set(chat, "value_to_flag", true).await.unwrap());
        assert!(settings.is_locked(chat, "value_to_flag"));
        assert!(settings.try_set(chat, "links", true).await.unwrap());
        assert!(!settings.try_set(chat, "links", true).await.unwrap());

        assert!(settings.indexed_empty(chat, "filter:"));
        assert!(settings.try_set(chat, "filter:بد", true).await.unwrap());
        settings
            .try_set_value(chat, "answer:سلام", "درود")
            .await
            .unwrap();
        assert!(!settings.indexed_empty(chat, "filter:"));
        assert!(settings.indexed_any(chat, "filter:", |word| word == "بد"));
        assert!(settings.indexed_any(chat, "answer:", |trigger| trigger == "سلام"));
        assert!(!settings.indexed_any(chat, "filter:", |word| word == "خوب"));

        let reloaded = Settings::connect(&url).await.unwrap();
        assert_eq!(reloaded.value(chat, "owner").as_deref(), Some("7"));
        assert!(
            reloaded.is_locked(chat, "value_to_flag"),
            "turning a valued row into a flag must update PostgreSQL, not only the mirror"
        );
        assert!(reloaded.is_locked(chat, "links"));
        assert!(reloaded.indexed_any(chat, "filter:", |word| word == "بد"));
        assert!(reloaded.indexed_any(chat, "answer:", |trigger| trigger == "سلام"));

        assert!(settings.try_set(chat, "filter:بد", false).await.unwrap());
        assert!(settings.indexed_empty(chat, "filter:"));

        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL import failure and retry-path probe"]
    async fn failed_import_keeps_its_source_file() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_997i64;
        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        let path = std::env::temp_dir().join(format!(
            "groupbot-import-{}-{}.data",
            std::process::id(),
            unix_now()
        ));
        std::fs::write(
            &path,
            format!("{chat} durable=first\n{chat} strict_action=kick\n"),
        )
        .unwrap();
        let path_text = path.to_string_lossy();
        assert!(matches!(
            settings.import_file(&path_text).await,
            Err(SettingsImportError::InvalidValue { line: 2, .. })
        ));
        assert!(path.exists(), "a partial import must remain retryable");
        assert!(!path.with_extension("data.imported").exists());
        assert_eq!(
            settings.value(chat, "durable"),
            None,
            "preflight must reject the whole malformed artifact before its first write"
        );

        std::fs::write(&path, format!("{chat} durable=first\n")).unwrap();
        assert_eq!(settings.import_file(&path_text).await.unwrap(), 1);
        assert!(!path.exists());
        assert_eq!(
            settings.import_file(&path_text).await.unwrap(),
            0,
            "a restart after the source was renamed must treat absence as no import"
        );
        std::fs::remove_file(format!("{path_text}.imported")).unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL startup corruption validation test"]
    async fn startup_validation_rejects_a_malformed_punishment_policy() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_958i64;
        let settings = Settings::connect(&url).await.unwrap();
        admit_test_chats(&settings, &[chat]).await;
        sqlx::query(
            "INSERT INTO settings (chat_id,key,value) VALUES ($1,'captcha_action','ban')
             ON CONFLICT (chat_id,key) DO UPDATE SET value=EXCLUDED.value",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();

        let mut connection = settings.pool.acquire().await.unwrap();
        let error = Settings::validate_persisted_settings(&mut connection)
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains(&chat.to_string()));
        assert!(message.contains("captcha_action"));

        sqlx::query("DELETE FROM settings WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO settings (chat_id,key,value) VALUES ($1,'hash','9223372036854775808')",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        let error = Settings::validate_persisted_settings(&mut connection)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&chat.to_string()));
        assert!(error.contains("hash"));
        sqlx::query("DELETE FROM settings WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL configured-chat query test"]
    async fn chats_with_only_lists_configured_chats() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let (on, off) = (-999_999_999_989, -999_999_999_988);

        let settings = Settings::connect(&url).await.unwrap();
        let both = vec![on, off];
        let wipe = "DELETE FROM settings WHERE chat_id = ANY($1)";
        sqlx::query(wipe)
            .bind(&both)
            .execute(&settings.pool)
            .await
            .unwrap();

        settings
            .try_set_value(on, "night", "1380|420")
            .await
            .unwrap();
        settings.try_set_value(off, "night", "0|60").await.unwrap();

        let mut listed = settings.chats_with("night").await.unwrap();
        listed.retain(|chat| both.contains(chat));
        listed.sort_unstable();
        let mut want = both.clone();
        want.sort_unstable();
        assert_eq!(listed, want);

        assert!(settings.try_set(off, "night", false).await.unwrap());
        let mut left = settings.chats_with("night").await.unwrap();
        left.retain(|chat| both.contains(chat));
        assert_eq!(left, vec![on]);

        settings.try_set(on, "gate_on", true).await.unwrap();
        assert!(!settings.chats_with("gate_on").await.unwrap().contains(&on));
        assert!(
            settings
                .flagged_with("gate_on")
                .await
                .unwrap()
                .contains(&on)
        );

        sqlx::query(wipe)
            .bind(&both)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL batch flag and mirror test"]
    async fn batch_flags_reconcile_the_mirror_and_database() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let (missing, existing) = (-999_999_999_985, -999_999_999_984);
        let unknown = -999_999_999_983;
        let all = vec![missing, existing];
        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&all)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_chats WHERE chat_id = $1")
            .bind(unknown)
            .execute(&settings.pool)
            .await
            .unwrap();
        assert_eq!(
            settings
                .set_flags(&[unknown], "gate_on", false)
                .await
                .unwrap(),
            0
        );
        let admitted: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM durable_chats WHERE chat_id = $1)")
                .bind(unknown)
                .fetch_one(&settings.pool)
                .await
                .unwrap();
        assert!(!admitted, "a no-op delete must not consume chat capacity");

        assert!(
            settings
                .try_set_value(missing, "join_channel", "@example")
                .await
                .unwrap()
        );
        assert!(
            settings
                .try_set_value(missing, "hash", "424242")
                .await
                .unwrap()
        );
        assert!(settings.try_set(missing, "hash", false).await.unwrap());
        assert_eq!(
            settings.durable_chat_hash(missing),
            Some(424242),
            "durable recovery retains the last authenticated peer after settings are removed"
        );
        assert!(settings.try_set(existing, "gate_on", true).await.unwrap());
        assert_eq!(settings.set_flags(&all, "gate_on", true).await.unwrap(), 1);
        assert!(settings.is_locked(missing, "gate_on"));
        assert!(settings.is_locked(existing, "gate_on"));
        assert_eq!(settings.set_flags(&all, "gate_on", true).await.unwrap(), 0);

        let rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM settings WHERE key = 'gate_on' AND chat_id = ANY($1)",
        )
        .bind(&all)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(rows, 2);

        assert_eq!(settings.set_flags(&all, "gate_on", false).await.unwrap(), 2);
        assert!(!settings.is_locked(missing, "gate_on"));
        assert!(!settings.is_locked(existing, "gate_on"));
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&all)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL panel ownership query test"]
    async fn panels_for_finds_owned_and_admined_chats() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let (mine_by_flag, mine_by_owner, theirs) =
            (-999_999_999_992, -999_999_999_991, -999_999_999_990);
        let (me, them) = (7_i64, 9_i64);

        let settings = Settings::connect(&url).await.unwrap();
        let all = vec![mine_by_flag, mine_by_owner, theirs];
        let wipe = "DELETE FROM settings WHERE chat_id = ANY($1)";
        sqlx::query(wipe)
            .bind(&all)
            .execute(&settings.pool)
            .await
            .unwrap();

        settings
            .try_set(mine_by_flag, &format!("admin:{me}"), true)
            .await
            .unwrap();
        settings
            .try_set_value(mine_by_owner, "owner", &me.to_string())
            .await
            .unwrap();
        settings
            .try_set_value(theirs, "owner", &them.to_string())
            .await
            .unwrap();
        settings
            .try_set(theirs, &format!("admin:{them}"), true)
            .await
            .unwrap();

        let mut found = settings.panels_for(me, 21).await.unwrap();
        found.retain(|chat| all.contains(chat));
        found.sort_unstable();
        let mut mine = vec![mine_by_flag, mine_by_owner];
        mine.sort_unstable();
        assert_eq!(found, mine, "someone else's group must not be listed");

        settings
            .try_set(mine_by_flag, &format!("admin:{me}"), false)
            .await
            .unwrap();
        let mut left = settings.panels_for(me, 21).await.unwrap();
        left.retain(|chat| all.contains(chat));
        assert_eq!(left, vec![mine_by_owner]);

        sqlx::query(wipe)
            .bind(&all)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL multi-chunk counter flush test"]
    async fn every_chunk_of_a_large_flush_lands() {
        const ROWS: i64 = 12_001;

        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_987;

        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 1, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        let wipe = "DELETE FROM counters WHERE chat_id = $1";
        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        let bumps: Vec<Bump> = (1..=ROWS)
            .map(|user| Bump {
                chat,
                user,
                name: format!("member {user}"),
                added: 2,
            })
            .collect();
        let folded = settings.bump(bumps, 500, 71, 16).await.unwrap();
        assert_eq!(folded.len() as i64, ROWS, "every chunk has to come back");

        let (total, members) = settings.board_totals(chat, Period::Total, 0).await.unwrap();
        assert_eq!(members as i64, ROWS);
        assert_eq!(total as i64, ROWS * 2);

        let again: Vec<Bump> = (1..=ROWS)
            .map(|user| Bump {
                chat,
                user,
                name: String::new(),
                added: 1,
            })
            .collect();
        settings.bump(again, 500, 71, 16).await.unwrap();
        assert_eq!(
            settings
                .board_totals(chat, Period::Total, 0)
                .await
                .unwrap()
                .0 as i64,
            ROWS * 3
        );
        assert_eq!(
            settings.name_of(chat, 1).await.unwrap().as_deref(),
            Some("member 1")
        );

        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL idle-chat cleanup test"]
    async fn forget_idle_only_touches_the_chats_it_is_given() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let (a, b) = (-999_999_999_994, -999_999_999_993);

        let settings = Settings::connect(&url).await.unwrap();
        let wipe = "DELETE FROM counters WHERE chat_id = ANY($1)";
        let both = vec![a, b];
        admit_test_chats(&settings, &both).await;
        sqlx::query(wipe)
            .bind(&both)
            .execute(&settings.pool)
            .await
            .unwrap();

        let spoke = |chat, user| Bump {
            chat,
            user,
            name: String::new(),
            added: 1,
        };

        settings.bump(vec![spoke(a, 1)], 100, 14, 3).await.unwrap();
        settings.bump(vec![spoke(a, 2)], 200, 28, 6).await.unwrap();
        settings.bump(vec![spoke(b, 1)], 100, 14, 3).await.unwrap();

        let left = |chat: i64| {
            let pool = settings.pool.clone();
            async move {
                sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM counters WHERE chat_id = $1")
                    .bind(chat)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .0
            }
        };

        assert_eq!(settings.forget_idle(&[a], 150).await.unwrap(), 1);
        assert_eq!(left(a).await, 1, "the member still talking must stay");
        assert_eq!(
            left(b).await,
            1,
            "a chat that was not named must be untouched"
        );

        assert_eq!(settings.forget_idle(&[b], 150).await.unwrap(), 1);
        assert_eq!(left(b).await, 0);

        sqlx::query(wipe)
            .bind(&both)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL tally rollover test"]
    async fn tallies_roll_over_in_the_write() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_995;
        let (day, next) = (2_000, 2_001);

        let settings = Settings::connect(&url).await.unwrap();
        admit_test_chats(&settings, &[chat]).await;
        let wipe = "DELETE FROM tallies WHERE chat_id = $1";
        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        settings
            .add_tallies(&[(chat, "k_text", 3), (chat, "h9", 3)], day)
            .await
            .unwrap();
        settings
            .add_tallies(&[(chat, "k_text", 5)], day)
            .await
            .unwrap();

        let today = settings.tallies(chat, day).await.unwrap();
        assert_eq!(today.get("k_text").copied(), Some(8));
        assert_eq!(today.get("h9").copied(), Some(3));

        settings
            .add_tallies(&[(chat, "k_text", 4)], next)
            .await
            .unwrap();
        let tomorrow = settings.tallies(chat, next).await.unwrap();
        assert_eq!(tomorrow.get("k_text").copied(), Some(4));

        assert!(!tomorrow.contains_key("h9"));

        let yesterday = settings.tallies(chat, day).await.unwrap();
        assert!(!yesterday.contains_key("k_text"));
        assert_eq!(yesterday.get("h9").copied(), Some(3));

        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL valued-setting deletion test"]
    async fn clearing_a_valued_setting_removes_the_row() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_996;
        let settings = Settings::connect(&url).await.unwrap();
        admit_test_chats(&settings, &[chat]).await;
        sqlx::query("DELETE FROM notes WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        settings.set_note(chat, 7, "برای بعد").await.unwrap();
        assert_eq!(
            settings.note(chat, 7).await.unwrap().as_deref(),
            Some("برای بعد")
        );

        settings.set_note(chat, 7, "").await.unwrap();
        assert!(settings.note(chat, 7).await.unwrap().is_none());

        let left: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM notes WHERE chat_id = $1 AND user_id = 7")
                .bind(chat)
                .fetch_one(&settings.pool)
                .await
                .unwrap();
        assert_eq!(left.0, 0);

        let reloaded = Settings::connect(&url).await.unwrap();
        assert!(reloaded.note(chat, 7).await.unwrap().is_none());

        sqlx::query("DELETE FROM notes WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL fleet dashboard aggregation test"]
    async fn the_fleet_dashboard_counts_what_it_says() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_997;
        let day = 4_242;

        let settings = Settings::connect(&url).await.unwrap();
        for wipe in [
            "DELETE FROM counters WHERE chat_id = $1",
            "DELETE FROM settings WHERE chat_id = $1",
        ] {
            sqlx::query(wipe)
                .bind(chat)
                .execute(&settings.pool)
                .await
                .unwrap();
        }

        settings.try_set_value(chat, "owner", "42").await.unwrap();
        settings.try_set(chat, "wipe_on", true).await.unwrap();
        settings
            .bump(
                vec![
                    Bump {
                        chat,
                        user: 1,
                        name: "Ali".to_owned(),
                        added: 6,
                    },
                    Bump {
                        chat,
                        user: 2,
                        name: "Sara".to_owned(),
                        added: 4,
                    },
                ],
                day,
                day / 7,
                day / 30,
            )
            .await
            .unwrap();

        let fleet = settings.fleet(day).await.unwrap();
        assert!(fleet.chats >= 1);
        assert!(fleet.configured >= 1, "the owner row is a configured chat");
        assert!(fleet.members >= 2, "two members were just counted");
        assert!(fleet.active_today >= 1);
        assert!(fleet.messages_today >= 10, "six and four said today");
        assert!(
            fleet.messages_total >= fleet.messages_today,
            "today is part of ever"
        );

        let busiest = settings.busiest(day, 500).await.unwrap();
        assert!(
            busiest.contains(&(chat, 10)),
            "{busiest:?} is missing the test chat"
        );

        let counts = settings
            .adoption(&["wipe_on", "nothing_uses_this"])
            .await
            .unwrap();
        assert!(counts.get("wipe_on").is_some_and(|on| *on >= 1));
        assert_eq!(counts.get("nothing_uses_this"), None);

        for wipe in [
            "DELETE FROM counters WHERE chat_id = $1",
            "DELETE FROM settings WHERE chat_id = $1",
        ] {
            sqlx::query(wipe)
                .bind(chat)
                .execute(&settings.pool)
                .await
                .unwrap();
        }
        assert!(
            !settings
                .busiest(day, 500)
                .await
                .unwrap()
                .iter()
                .any(|(id, _)| *id == chat),
            "a chat with no rows is on no board"
        );
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL counter rollover test"]
    async fn counters_roll_over() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_998;
        let (day, week, month) = (1_000, 1_000 / 7, 1_000 / 30);

        let settings = Settings::connect(&url).await.unwrap();
        admit_test_chats(&settings, &[chat]).await;
        let wipe = "DELETE FROM counters WHERE chat_id = $1";
        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        let spoke = |who: &[(i64, &str, u64)]| {
            who.iter()
                .map(|(user, name, added)| Bump {
                    chat,
                    user: *user,
                    name: (*name).to_owned(),
                    added: *added,
                })
                .collect::<Vec<_>>()
        };
        let both = |ali, sara| spoke(&[(1, "Ali", ali), (2, "Sara", sara)]);
        let ali = |count| spoke(&[(1, "Ali", count)]);

        settings.bump(both(5, 9), day, week, month).await.unwrap();
        settings.bump(both(3, 1), day, week, month).await.unwrap();

        let top = settings.board(chat, Period::Today, day, 10).await.unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!((top[0].user, top[0].count), (2, 10));
        assert_eq!((top[1].user, top[1].count), (1, 8));
        assert_eq!(
            settings
                .board_totals(chat, Period::Today, day)
                .await
                .unwrap(),
            (18, 2)
        );
        assert_eq!(
            settings.board_totals(chat, Period::Total, 0).await.unwrap(),
            (18, 2)
        );

        settings.bump(ali(2), day + 1, week, month).await.unwrap();
        assert_eq!(
            settings
                .board_totals(chat, Period::Today, day + 1)
                .await
                .unwrap(),
            (2, 1)
        );
        let today = settings
            .board(chat, Period::Today, day + 1, 10)
            .await
            .unwrap();
        assert_eq!(today.len(), 1);
        assert_eq!((today[0].user, today[0].count), (1, 2));
        assert_eq!(
            settings
                .board_totals(chat, Period::Week, week)
                .await
                .unwrap(),
            (20, 2)
        );
        assert_eq!(
            settings.board_totals(chat, Period::Total, 0).await.unwrap(),
            (20, 2)
        );

        settings
            .bump(ali(4), day + 8, week + 1, month)
            .await
            .unwrap();
        assert_eq!(
            settings
                .board_totals(chat, Period::Week, week + 1)
                .await
                .unwrap(),
            (4, 1)
        );
        assert_eq!(
            settings
                .board_totals(chat, Period::Month, month)
                .await
                .unwrap(),
            (24, 2)
        );

        let card = settings.card(chat, 1, day + 8).await.unwrap().unwrap();
        assert_eq!((card.total, card.today, card.place), (14, 4, Some(1)));
        assert_eq!(
            settings
                .card(chat, 2, day + 8)
                .await
                .unwrap()
                .unwrap()
                .place,
            None
        );
        assert_eq!(
            settings.name_of(chat, 2).await.unwrap().as_deref(),
            Some("Sara")
        );

        let (idle, total) = settings.idle(chat, day + 8, 5, 10).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(
            idle.first().map(|(user, _, quiet)| (*user, *quiet)),
            Some((2, 8))
        );

        assert_eq!(
            settings.credit_add(chat, 1, "Ali", 3).await.unwrap(),
            Some(3)
        );
        assert_eq!(
            settings.credit_add(chat, 1, "Ali", 2).await.unwrap(),
            Some(5)
        );
        assert_eq!(settings.adds_of(chat, 1).await.unwrap(), Some(5));

        settings.clear_seen(chat, 2).await.unwrap();
        assert_eq!(settings.idle(chat, day + 8, 5, 10).await.unwrap().1, 0);

        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL strike expiry test"]
    async fn strikes_expire_in_the_write() {
        async fn strike(settings: &Settings, chat: i64, user: i64, day: u64, days: u64) -> u32 {
            settings
                .increment_strict(StrictViolation {
                    chat,
                    user,
                    day,
                    expiry_days: days,
                    limit: 20,
                    action: StrictAction::Mute,
                    duration_seconds: None,
                    until_date: 0,
                    target_name: "member",
                    wipe_history: false,
                })
                .await
                .unwrap()
                .unwrap()
                .count
        }

        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_997;
        let (user, days) = (7, 7);

        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 1, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        let wipe = "DELETE FROM counters WHERE chat_id = $1";
        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        assert_eq!(strike(&settings, chat, user, 100, days).await, 1);
        assert_eq!(strike(&settings, chat, user, 101, days).await, 2);
        assert_eq!(strike(&settings, chat, user, 106, days).await, 3);

        assert_eq!(strike(&settings, chat, user, 107, days).await, 4);

        assert_eq!(strike(&settings, chat, user, 114, days).await, 1);

        assert_eq!(settings.warns_of(chat, user).await.unwrap(), 0);
        settings.set_warns(chat, user, 3).await.unwrap();
        assert_eq!(settings.warns_of(chat, user).await.unwrap(), 3);
        settings.set_warns(chat, user, 0).await.unwrap();
        assert_eq!(settings.warns_of(chat, user).await.unwrap(), 0);

        sqlx::query("DELETE FROM pending_warn_actions WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        let (first, second) = tokio::join!(
            settings.increment_warning(chat, user, 2, WarningPenalty::Ban),
            settings.increment_warning(chat, user, 2, WarningPenalty::Ban)
        );
        let mut counts = [first.unwrap().count, second.unwrap().count];
        counts.sort_unstable();
        assert_eq!(
            counts,
            [1, 2],
            "concurrent warnings must not lose an increment"
        );
        let now = unix_now();
        let stale = settings
            .claim_warning_action(chat, user, now, now + 60)
            .await
            .unwrap()
            .expect("threshold action is claimable");
        assert!(
            settings
                .claim_warning_action(chat, user, now, now + 60)
                .await
                .unwrap()
                .is_none(),
            "an unexpired lease cannot be stolen"
        );
        let current = settings
            .claim_warning_action(chat, user, now + 61, now + 121)
            .await
            .unwrap()
            .expect("an expired lease is recoverable");
        assert!(
            !settings.complete_warning_action(&stale).await.unwrap(),
            "a stale worker cannot acknowledge a reclaimed lease"
        );
        assert!(
            !settings
                .defer_warning_action(&stale, now + 500, "stale fixture")
                .await
                .unwrap(),
            "a stale worker cannot postpone a reclaimed lease"
        );
        assert!(settings.complete_warning_action(&current).await.unwrap());
        assert_eq!(settings.warns_of(chat, user).await.unwrap(), 0);

        settings
            .increment_warning(chat, user, 1, WarningPenalty::Ban)
            .await
            .unwrap();
        let revoked = settings
            .claim_warning_action(chat, user, now + 122, now + 182)
            .await
            .unwrap()
            .expect("revocable lifecycle is claimable");
        assert!(settings.warning_action_is_current(&revoked).await.unwrap());
        assert_eq!(settings.decrement_warning(chat, user).await.unwrap(), 0);
        assert!(
            !settings.warning_action_is_current(&revoked).await.unwrap(),
            "unwarn must revoke the leased durable action in the same transaction"
        );
        assert!(
            !settings.complete_warning_action(&revoked).await.unwrap(),
            "a worker cannot acknowledge an action superseded by unwarn"
        );

        let replacement = settings
            .increment_warning(chat, user, 1, WarningPenalty::Ban)
            .await
            .unwrap();
        assert_eq!(replacement.pending, Some(WarningPenalty::Ban));
        let replacement = settings
            .claim_warning_action(chat, user, now + 122, now + 182)
            .await
            .unwrap()
            .expect("replacement lifecycle is claimable");
        assert!(
            !settings.complete_warning_action(&current).await.unwrap(),
            "an old lifecycle cannot delete a replacement row with the same key"
        );
        assert!(
            settings
                .complete_warning_action(&replacement)
                .await
                .unwrap()
        );

        let terminal = settings
            .increment_warning(chat, user, 1, WarningPenalty::Ban)
            .await
            .unwrap();
        assert_eq!(terminal.count, 1);
        let terminal = settings
            .claim_warning_action(chat, user, now + 183, now + 243)
            .await
            .unwrap()
            .expect("terminal lifecycle is claimable");
        assert!(settings.terminate_warning_action(&terminal).await.unwrap());
        assert_eq!(
            settings.warns_of(chat, user).await.unwrap(),
            1,
            "a terminal Telegram contract failure did not apply a punishment and must not erase warnings"
        );

        let rejoin_user = user + 10;
        settings
            .increment_warning(chat, rejoin_user, 1, WarningPenalty::Ban)
            .await
            .unwrap();
        let awaiting = settings
            .claim_warning_action(chat, rejoin_user, now + 250, now + 310)
            .await
            .unwrap()
            .expect("absent-member lifecycle is claimable");
        assert!(
            settings
                .await_warning_rejoin(&awaiting, "USER_NOT_PARTICIPANT")
                .await
                .unwrap()
        );
        assert!(
            settings
                .claim_warning_action(chat, rejoin_user, now + 311, now + 371)
                .await
                .unwrap()
                .is_none(),
            "an absent member must be retained without becoming a hot retry"
        );
        assert!(
            settings
                .resume_warning_on_rejoin(chat, rejoin_user, now + 400)
                .await
                .unwrap()
        );
        let resumed = settings
            .claim_warning_action(chat, rejoin_user, now + 400, now + 460)
            .await
            .unwrap()
            .expect("a live rejoin makes the retained lifecycle claimable again");
        assert!(settings.terminate_warning_action(&resumed).await.unwrap());

        let ambiguous_user = user + 11;
        settings
            .increment_warning(chat, ambiguous_user, 1, WarningPenalty::Mute)
            .await
            .unwrap();
        let ambiguous = settings
            .claim_warning_action(chat, ambiguous_user, now + 500, now + 560)
            .await
            .unwrap()
            .expect("ambiguous publication fixture is claimable");
        let mut publisher = settings.pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(super::durable_member_rejoin_key(chat, ambiguous_user))
            .execute(&mut *publisher)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE pending_warn_actions
             SET awaiting_rejoin = TRUE, lease_token = NULL
             WHERE chat_id = $1 AND user_id = $2
               AND generation = $3 AND version = $4 AND lease_token = $5",
        )
        .bind(chat)
        .bind(ambiguous_user)
        .bind(ambiguous.generation)
        .bind(ambiguous.version)
        .bind(ambiguous.lease_token)
        .execute(&mut *publisher)
        .await
        .unwrap();
        let mut late_resume =
            Box::pin(settings.resume_warning_on_rejoin(chat, ambiguous_user, now + 600));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut late_resume)
                .await
                .is_err(),
            "rejoin passed a publication transaction whose commit was still ambiguous"
        );
        publisher.commit().await.unwrap();
        assert!(
            late_resume.await.unwrap(),
            "rejoin did not consume the publication that committed after its Rust guard vanished"
        );
        let awaiting_rejoin: bool = sqlx::query_scalar(
            "SELECT awaiting_rejoin FROM pending_warn_actions
             WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(ambiguous_user)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert!(!awaiting_rejoin);
        let reclaimed = settings
            .claim_warning_action(chat, ambiguous_user, now + 600, now + 660)
            .await
            .unwrap()
            .expect("the late publication is immediately claimable after rejoin");
        assert!(settings.terminate_warning_action(&reclaimed).await.unwrap());

        let retry_user = user + 1;
        let fresh_user = user + 2;
        settings
            .increment_warning(chat, retry_user, 1, WarningPenalty::Mute)
            .await
            .unwrap();
        let retry = settings
            .claim_warning_action(chat, retry_user, now + 300, now + 360)
            .await
            .unwrap()
            .expect("retry lifecycle is claimable");
        assert!(
            settings
                .defer_warning_action(&retry, now + 600, "retry fixture")
                .await
                .unwrap()
        );
        settings
            .increment_warning(chat, fresh_user, 1, WarningPenalty::Mute)
            .await
            .unwrap();
        let first_due = settings
            .claim_pending_warning_actions(WarningQueueClass::Fresh, now + 600, now + 660, 1)
            .await
            .unwrap()
            .pop()
            .expect("one warning action is due");
        assert_eq!(
            first_due.user, fresh_user,
            "an old retry loop must not starve fresh warning work"
        );
        assert!(settings.terminate_warning_action(&first_due).await.unwrap());
        let old_retry = settings
            .claim_pending_warning_actions(WarningQueueClass::Retry, now + 600, now + 660, 1)
            .await
            .unwrap()
            .pop()
            .expect("the old retry remains claimable after fresh work");
        assert_eq!(old_retry.user, retry_user);
        assert!(settings.terminate_warning_action(&old_retry).await.unwrap());

        assert_eq!(settings.warns_of(chat, 12_345).await.unwrap(), 0);

        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL moderation-case lifecycle test"]
    async fn moderation_cases_are_isolated_audited_and_expire() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let (chat, other) = (-999_999_999_994, -999_999_999_993);
        admit_test_chats(&settings, &[chat, other]).await;
        sqlx::query("DELETE FROM moderation_cases WHERE chat_id = ANY($1)")
            .bind(vec![chat, other])
            .execute(&settings.pool)
            .await
            .unwrap();

        let id = settings
            .create_moderation_case(NewModerationCase {
                chat,
                subject: Some(42),
                subject_name: "کاربر".to_owned(),
                source: "report".to_owned(),
                rule: "member_report".to_owned(),
                reason: "گزارش عضو".to_owned(),
                message: Some(7),
                media_kind: None,
                evidence: Some("نمونه".to_owned()),
                evidence_hash: Some(vec![1, 2, 3]),
                action: "none".to_owned(),
                action_until: None,
                status: "open".to_owned(),
                actor: Some(9),
                actor_name: "مدیر".to_owned(),
                event_kind: "reported".to_owned(),
                event_note: None,
            })
            .await
            .unwrap();
        assert!(settings.moderation_case(other, id).await.unwrap().is_none());
        assert_eq!(
            settings
                .moderation_cases(chat, Some("open"), Some(42), None, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            settings
                .transition_moderation_case(ModerationCaseTransition {
                    chat,
                    case_id: id,
                    from: "open",
                    to: "resolved",
                    event_kind: "resolved",
                    actor: Some((9, "مدیر")),
                    action: Some("none"),
                    note: Some("بررسی شد"),
                })
                .await
                .unwrap()
        );
        assert!(
            !settings
                .transition_moderation_case(ModerationCaseTransition {
                    chat,
                    case_id: id,
                    from: "open",
                    to: "resolved",
                    event_kind: "resolved",
                    actor: Some((9, "مدیر")),
                    action: None,
                    note: None,
                })
                .await
                .unwrap()
        );
        let detail = settings.moderation_case(chat, id).await.unwrap().unwrap();
        assert_eq!(detail.case.status, "resolved");
        assert_eq!(detail.events.len(), 2);

        let workflow_draft = || NewModerationCase {
            chat,
            subject: Some(43),
            subject_name: "workflow user".to_owned(),
            source: "automatic".to_owned(),
            rule: "captcha".to_owned(),
            reason: "failed captcha".to_owned(),
            message: Some(8),
            media_kind: None,
            evidence: None,
            evidence_hash: None,
            action: "kick".to_owned(),
            action_until: None,
            status: "resolved".to_owned(),
            actor: None,
            actor_name: String::new(),
            event_kind: "action_succeeded".to_owned(),
            event_note: None,
        };
        let workflow_id = settings
            .create_workflow_moderation_case("captcha-failure:43:991", workflow_draft())
            .await
            .unwrap();
        let replay_id = settings
            .create_workflow_moderation_case("captcha-failure:43:991", workflow_draft())
            .await
            .unwrap();
        assert!(workflow_id.inserted);
        assert!(!replay_id.inserted);
        assert_eq!(workflow_id.id, replay_id.id);
        let workflow = settings
            .moderation_case(chat, workflow_id.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            workflow.events.len(),
            1,
            "a crash replay must not duplicate the workflow audit event"
        );

        sqlx::query(
            "INSERT INTO moderation_cases
             (id, chat_id, subject_user_id, subject_name, source, rule_key, reason, message_id,
              media_kind, evidence_text, evidence_hash, primary_action, action_until, status,
              actor_id, actor_name, created_at, updated_at)
             SELECT id, $2, subject_user_id, subject_name, source, rule_key, reason, message_id,
                    media_kind, evidence_text, evidence_hash, primary_action, action_until,
                    status, actor_id, actor_name, created_at, updated_at
             FROM moderation_cases WHERE chat_id = $1 AND id = $3",
        )
        .bind(chat)
        .bind(other)
        .bind(id)
        .execute(&settings.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO moderation_case_events
             (id, chat_id, case_id, kind, actor_id, actor_name, action, note, created_at)
             SELECT id, $2, case_id, kind, actor_id, actor_name, action, note, created_at
             FROM moderation_case_events WHERE chat_id = $1 AND case_id = $3",
        )
        .bind(chat)
        .bind(other)
        .bind(id)
        .execute(&settings.pool)
        .await
        .unwrap();
        assert_eq!(
            settings
                .moderation_case(other, id)
                .await
                .unwrap()
                .unwrap()
                .events
                .len(),
            2
        );

        sqlx::query("UPDATE moderation_cases SET created_at = $1 WHERE chat_id = $2 AND id = $3")
            .bind(unix_now() - CASE_RETENTION_SECS - 1)
            .bind(chat)
            .bind(id)
            .execute(&settings.pool)
            .await
            .unwrap();
        assert_eq!(settings.purge_expired_moderation_cases().await.unwrap(), 1);
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM moderation_case_events WHERE chat_id = $1 AND case_id = $2",
        )
        .bind(chat)
        .bind(id)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(events, 0);
        assert!(settings.moderation_case(other, id).await.unwrap().is_some());
        sqlx::query("DELETE FROM moderation_cases WHERE chat_id = $1")
            .bind(other)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL warning queue fairness probe"]
    async fn warning_retry_progresses_with_more_than_a_page_of_continuous_fresh_work() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chat = -999_999_999_989_i64;
        let now = unix_now();
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 1, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM pending_warn_actions WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        for offset in 0..12_i64 {
            sqlx::query(
                "INSERT INTO pending_warn_actions
                 (chat_id, user_id, penalty, created_at, claimed_until, attempts)
                 VALUES ($1, $2, 'mute', $3, $3, 0)",
            )
            .bind(chat)
            .bind(40_000 + offset)
            .bind(now - 100 + offset)
            .execute(&settings.pool)
            .await
            .unwrap();
        }
        for offset in 0..8_i64 {
            sqlx::query(
                "INSERT INTO pending_warn_actions
                 (chat_id, user_id, penalty, created_at, claimed_until, attempts)
                 VALUES ($1, $2, 'mute', $3, $3, 1)",
            )
            .bind(chat)
            .bind(50_000 + offset)
            .bind(now - 1_000 + offset)
            .execute(&settings.pool)
            .await
            .unwrap();
        }

        let fresh = settings
            .claim_pending_warning_actions(WarningQueueClass::Fresh, now, now + 60, 4)
            .await
            .unwrap();
        assert_eq!(fresh.len(), 4);
        let retry = settings
            .claim_pending_warning_actions(WarningQueueClass::Retry, now, now + 60, 4)
            .await
            .unwrap();
        assert_eq!(retry.len(), 4);
        assert!(fresh.iter().all(|work| work.attempts == 1));
        assert!(retry.iter().all(|work| work.attempts == 2));

        sqlx::query("DELETE FROM pending_warn_actions WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL durable-chat ownership constraint test"]
    async fn every_chat_scoped_table_requires_the_authoritative_durable_owner() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        for (table, column) in [
            ("settings", "owner_chat_id"),
            ("counters", "chat_id"),
            ("notes", "chat_id"),
            ("tallies", "chat_id"),
            ("pending_deletes", "chat_id"),
            ("default_rights_state", "chat_id"),
            ("pending_captchas", "chat_id"),
            ("moderation_cases", "chat_id"),
            ("moderation_case_events", "chat_id"),
            ("pending_warn_actions", "chat_id"),
            ("pending_strict_actions", "chat_id"),
            ("pending_rank_awards", "chat_id"),
            ("image_filters", "chat_id"),
        ] {
            let covered: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                   SELECT 1 FROM pg_constraint AS constraint_row
                   JOIN unnest(constraint_row.conkey) AS key(attnum) ON TRUE
                   JOIN pg_attribute AS attribute
                     ON attribute.attrelid = constraint_row.conrelid
                    AND attribute.attnum = key.attnum
                   WHERE constraint_row.contype = 'f'
                     AND constraint_row.conrelid = to_regclass($1)
                     AND constraint_row.confrelid = 'durable_chats'::regclass
                     AND attribute.attname = $2
                 )",
            )
            .bind(table)
            .bind(column)
            .fetch_one(&settings.pool)
            .await
            .unwrap();
            assert!(covered, "{table}.{column} has no durable owner foreign key");
        }

        let chat = -999_999_997_699_i64;
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM tallies WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_chats WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        assert!(
            sqlx::query("INSERT INTO settings (chat_id, key) VALUES ($1, 'audit-owner')")
                .bind(chat)
                .execute(&settings.pool)
                .await
                .is_err(),
            "a chat setting bypassed durable admission"
        );
        assert!(
            sqlx::query(
                "INSERT INTO tallies (chat_id, counter, day, count)
                 VALUES ($1, 'audit_owner', 1, 1)",
            )
            .bind(chat)
            .execute(&settings.pool)
            .await
            .is_err(),
            "a tally bypassed durable admission"
        );

        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at) VALUES ($1, 0, 0)",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO settings (chat_id, key) VALUES ($1, 'audit-owner')")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        assert!(
            sqlx::query("DELETE FROM durable_chats WHERE chat_id = $1")
                .bind(chat)
                .execute(&settings.pool)
                .await
                .is_err(),
            "an admitted owner was deleted while a settings child remained"
        );
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_chats WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL closed-pool error-contract test"]
    async fn state_reads_and_cleanup_writes_preserve_database_failure() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        settings.pool.close().await;
        let day = 1_234;

        assert!(settings.board(-1, Period::Today, day, 10).await.is_err());
        assert!(
            settings
                .bump(
                    vec![Bump {
                        chat: -1,
                        user: 1,
                        name: "member".to_owned(),
                        added: 1,
                    }],
                    day,
                    day / 7,
                    day / 30,
                )
                .await
                .is_err()
        );
        assert!(settings.board_totals(-1, Period::Today, day).await.is_err());
        assert!(settings.idle(-1, day, 7, 10).await.is_err());
        assert!(settings.tallies(-1, day).await.is_err());
        assert!(
            settings
                .add_tallies(&[(-1, "k_text", 1)], day)
                .await
                .is_err()
        );
        assert!(settings.card(-1, 1, day).await.is_err());
        assert!(settings.chats_with("night").await.is_err());
        assert!(
            settings
                .chats_with_values("report_at", &["60".to_owned()])
                .await
                .is_err()
        );
        assert!(settings.cleaner_recommendations_due(&[0]).await.is_err());
        assert!(settings.incomplete_setups(false).await.is_err());
        assert!(settings.fleet(day).await.is_err());
        assert!(settings.busiest(day, 10).await.is_err());
        assert!(settings.adoption(&["wipe_on"]).await.is_err());
        assert!(settings.badge_rows(10).await.is_err());
        assert!(settings.image_filters(-1).await.is_err());
        assert!(settings.started_user(1).await.is_err());
        assert!(settings.load_pending(0).await.is_err());
        assert!(settings.clear_seen(-1, 1).await.is_err());
        assert!(settings.drop_pending_rows(&[(-1, 1)]).await.is_err());
        assert!(settings.forget_idle(&[-1], 0).await.is_err());
        assert!(settings.durable_counts().await.is_err());
        assert!(settings.ping().await.is_err());
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL counter/tally domain constraint test"]
    async fn counter_and_tally_domains_reject_corrupt_storage() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let (chat, user) = (-999_999_997_701_i64, 7701_i64);
        admit_test_chats(&settings, &[chat]).await;
        sqlx::query("DELETE FROM counters WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM tallies WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO counters (chat_id, user_id) VALUES ($1, $2)")
            .bind(chat)
            .bind(user)
            .execute(&settings.pool)
            .await
            .unwrap();
        for column in [
            "total", "today", "day", "week", "week_at", "month", "month_at", "seen", "adds",
            "awarded", "warns", "strikes", "struck",
        ] {
            let result = sqlx::query(&format!(
                "UPDATE counters SET {column} = -1 WHERE chat_id = $1 AND user_id = $2"
            ))
            .bind(chat)
            .bind(user)
            .execute(&settings.pool)
            .await;
            assert!(result.is_err(), "{column} accepted a negative value");
        }
        assert!(
            sqlx::query("INSERT INTO counters (chat_id, user_id) VALUES ($1, 0)")
                .bind(chat)
                .execute(&settings.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("INSERT INTO counters (chat_id, user_id) VALUES (-1000000000000, $1)")
                .bind(user + 1)
                .execute(&settings.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query(
                "INSERT INTO durable_chats (chat_id, access_hash, admitted_at) VALUES (7, 0, 0)"
            )
            .execute(&settings.pool)
            .await
            .is_err(),
            "durable admission accepted a user dialog id"
        );
        assert!(
            sqlx::query(
                "INSERT INTO tallies (chat_id, counter, day, count) VALUES ($1, 'text', -1, 0)"
            )
            .bind(chat)
            .execute(&settings.pool)
            .await
            .is_err()
        );
        assert!(
            sqlx::query("INSERT INTO tallies (chat_id, counter, day, count) VALUES ($1, '', 0, 0)")
                .bind(chat)
                .execute(&settings.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("INSERT INTO tallies (chat_id, counter, day, count) VALUES ($1, $2, 0, 0)")
                .bind(chat)
                .bind("x".repeat(MAX_TALLY_COUNTER_BYTES + 1))
                .execute(&settings.pool)
                .await
                .is_err(),
            "tallies accepted an unbounded counter identity"
        );
        assert!(
            sqlx::query(
                "INSERT INTO tallies (chat_id, counter, day, count) VALUES ($1, 'bad:name', 0, 0)"
            )
            .bind(chat)
            .execute(&settings.pool)
            .await
            .is_err(),
            "typed tallies accepted the legacy separator inside a counter identity"
        );
        sqlx::query("DELETE FROM counters WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL pre-migration counter-cap regression"]
    async fn legacy_sv_rows_are_counted_before_per_user_migration_writes() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL statement_timeout = '10s'")
            .execute(&mut *tx)
            .await
            .unwrap();
        let chat = -999_999_997_701_i64;
        sqlx::query("DELETE FROM counters WHERE chat_id = $1")
            .bind(chat)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 0, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO settings (chat_id, key, value)
             SELECT $1, 'sv:' || member::TEXT, '1:1'
             FROM generate_series(1, $2::BIGINT) AS member",
        )
        .bind(chat)
        .bind(MAX_COUNTER_ROWS_PER_CHAT + 1)
        .execute(&mut *tx)
        .await
        .unwrap();

        let error = Settings::validate_projected_counter_capacity(&mut tx, None)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&(MAX_COUNTER_ROWS_PER_CHAT + 1).to_string())
        );
        let (legacy, migrated): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM settings WHERE chat_id = $1 AND key LIKE 'sv:%'),
                    (SELECT count(*) FROM counters WHERE chat_id = $1)",
        )
        .bind(chat)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(legacy, MAX_COUNTER_ROWS_PER_CHAT + 1);
        assert_eq!(
            migrated, 0,
            "capacity must fail before the first migration write"
        );
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL pre-migration tally-cap regression"]
    async fn legacy_tally_rows_are_counted_before_migration_writes() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL statement_timeout = '10s'")
            .execute(&mut *tx)
            .await
            .unwrap();
        let chat = -999_999_997_703_i64;
        sqlx::query("DELETE FROM tallies WHERE chat_id = $1")
            .bind(chat)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 0, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO settings (chat_id, key, value)
             SELECT $1, 'tally:legacy_' || counter::TEXT, '1|1'
             FROM generate_series(1, $2::BIGINT) AS counter",
        )
        .bind(chat)
        .bind(MAX_TALLY_ROWS_PER_CHAT + 1)
        .execute(&mut *tx)
        .await
        .unwrap();

        let error = Settings::validate_projected_tally_capacity(&mut tx, None)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&(MAX_TALLY_ROWS_PER_CHAT + 1).to_string())
        );
        let (legacy, migrated): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM settings WHERE chat_id = $1 AND key LIKE 'tally:%'),
                    (SELECT count(*) FROM tallies WHERE chat_id = $1)",
        )
        .bind(chat)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(legacy, MAX_TALLY_ROWS_PER_CHAT + 1);
        assert_eq!(
            migrated, 0,
            "tally capacity must fail before the first migration write"
        );
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL fail-closed legacy counter/tally migration test"]
    async fn malformed_legacy_counter_and_tally_rows_survive_rejected_migration() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let chat = -999_999_997_702_i64;
        sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
        sqlx::query(
            "DELETE FROM settings WHERE chat_id = 0 AND key IN ('counters_migrated', 'tallies_migrated')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 0, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO settings (chat_id, key, value) VALUES ($1, $2, $3)")
            .bind(chat)
            .bind("total:1099511627776")
            .bind("7|member")
            .execute(&mut *conn)
            .await
            .unwrap();
        let error = Settings::migrate_counters(&mut conn).await.unwrap_err();
        assert!(error.to_string().contains("total:1099511627776"));
        let retained: i64 =
            sqlx::query_scalar("SELECT count(*) FROM settings WHERE chat_id = $1 AND key = $2")
                .bind(chat)
                .bind("total:1099511627776")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(retained, 1);
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO settings (chat_id, key, value) VALUES ($1, 'today:7', '1|-2|member')",
        )
        .bind(chat)
        .execute(&mut *conn)
        .await
        .unwrap();
        let error = Settings::migrate_counters(&mut conn).await.unwrap_err();
        assert!(error.to_string().contains("today:7"));
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO settings (chat_id, key, value) VALUES ($1, 'tally:text', '1|-2')")
            .bind(chat)
            .execute(&mut *conn)
            .await
            .unwrap();
        let error = Settings::migrate_tallies(&mut conn).await.unwrap_err();
        assert!(error.to_string().contains("tally:text"));
        let retained: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM settings WHERE chat_id = $1 AND key = 'tally:text'",
        )
        .bind(chat)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(retained, 1);
        sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
    }
}
