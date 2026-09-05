use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

type Result<T> = std::result::Result<T, sqlx::Error>;

pub const INDEXED: &[&str] = &["filter:", "pack:", "answer:", "strict:", "imgf:", "cmd:"];

pub const MAX_COUNTER_ROWS_PER_CHAT: i64 = 20_000;
pub const MAX_NOTE_ROWS_PER_CHAT: i64 = 20_000;
pub const MAX_IMAGE_FILTERS_PER_CHAT: i64 = 8;

const COUNTER_PREFIXES: &[&str] = &[
    "total:", "today:", "week:", "month:", "seen:", "adds:", "rank:",
];

type ChatIndex = [Vec<Box<str>>; INDEXED.len()];

const INDEXES: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS pending_due ON pending_deletes (due_at)",

    "CREATE INDEX IF NOT EXISTS started_users_recent ON started_users (last_started_at, user_id)",

    "CREATE INDEX IF NOT EXISTS settings_bot_admin ON settings (key) WHERE key LIKE 'admin:%'",
    "CREATE INDEX IF NOT EXISTS settings_owner ON settings (value) WHERE key = 'owner'",

    "CREATE INDEX IF NOT EXISTS settings_key_chat ON settings (key, chat_id)",

    "CREATE INDEX IF NOT EXISTS settings_badge_rows ON settings (chat_id, key) \
     WHERE key LIKE 'badge:%'",

    "CREATE INDEX IF NOT EXISTS settings_night ON settings (chat_id) WHERE key = 'night'",
    "CREATE INDEX IF NOT EXISTS settings_report_at ON settings (chat_id) WHERE key = 'report_at'",
    "CREATE INDEX IF NOT EXISTS settings_purge_at ON settings (chat_id) \
     WHERE key = 'auto_purge_at'",

    "CREATE INDEX IF NOT EXISTS settings_report_due ON settings (value, chat_id) \
     WHERE key = 'report_at' AND value <> ''",
    "CREATE INDEX IF NOT EXISTS settings_purge_due ON settings (value, chat_id) \
     WHERE key = 'auto_purge_at' AND value <> ''",

    "CREATE INDEX IF NOT EXISTS settings_group_lock_due ON settings (value, chat_id) \
     WHERE key = 'glock_until' AND value <> ''",

    "CREATE INDEX IF NOT EXISTS settings_night_boundary ON settings \
     ((split_part(value, '|', 1)), (split_part(value, '|', 2)), chat_id) \
     WHERE key = 'night' AND value <> ''",
    "CREATE INDEX IF NOT EXISTS settings_join_channel ON settings (chat_id) \
     WHERE key = 'join_channel'",
    "CREATE INDEX IF NOT EXISTS settings_add_required ON settings (chat_id) \
     WHERE key = 'add_required'",
    "CREATE INDEX IF NOT EXISTS settings_gate_on ON settings (chat_id) WHERE key = 'gate_on'",

    "CREATE INDEX IF NOT EXISTS image_filters_uncalibrated ON image_filters (chat_id) \
     WHERE calibrated = FALSE",

    "CREATE INDEX IF NOT EXISTS counters_day_chat ON counters (day, chat_id) INCLUDE (today)",
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

pub struct Counter {
    pub user: i64,
    pub name: String,
    pub count: u64,
}

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

pub struct ChatSettings<'a>(Option<&'a HashMap<String, String>>);

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

    pub fn number(&self, key: &str, default: u32, range: (u32, u32)) -> u32 {
        self.value(key)
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
            .clamp(range.0, range.1)
    }
}

pub struct Settings {
    pool: PgPool,

    _process_lock: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,

    cache: RwLock<HashMap<i64, HashMap<String, String>>>,

    index: RwLock<HashMap<i64, ChatIndex>>,

    max_chats: Option<usize>,
    max_rows: Option<usize>,
    max_bytes: Option<usize>,
    max_counter_rows: Option<i64>,
    max_note_rows: Option<i64>,
    setting_rows: AtomicUsize,
    setting_bytes: AtomicUsize,

    night_legacy_checked: AtomicBool,

    write_slots: Box<[std::sync::Arc<tokio::sync::Mutex<()>>]>,
}

const WRITE_SLOTS: usize = 1024;
pub const MAX_SETTINGS_ROWS_PER_CHAT: usize = 512;
pub const MAX_SETTING_KEY_BYTES: usize = 512;
pub const MAX_SETTING_VALUE_BYTES: usize = 16 * 1024;
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
const DEFAULT_MAX_NOTE_ROWS: i64 = 5_000_000;
const MIN_MAX_NOTE_ROWS: i64 = 100_000;
const ABSOLUTE_MAX_NOTE_ROWS: i64 = 50_000_000;

fn indexed_slot(key: &str) -> Option<(usize, &str)> {
    INDEXED
        .iter()
        .enumerate()
        .find_map(|(slot, prefix)| key.strip_prefix(prefix).map(|rest| (slot, rest)))
}

fn setting_size(key: &str, value: &str) -> usize {
    key.len().saturating_add(value.len())
}

impl Settings {
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
        let max_rows = max_chats.map(|_| {
            std::env::var("MAX_SHARD_SETTINGS_ROWS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_MAX_SETTINGS_ROWS)
                .clamp(MIN_MAX_SETTINGS_ROWS, ABSOLUTE_MAX_SETTINGS_ROWS)
        });
        let max_bytes = max_chats.map(|_| {
            std::env::var("MAX_SHARD_SETTINGS_BYTES")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_MAX_SETTINGS_BYTES)
                .clamp(MIN_MAX_SETTINGS_BYTES, ABSOLUTE_MAX_SETTINGS_BYTES)
        });
        let max_counter_rows = max_chats.map(|_| {
            std::env::var("MAX_SHARD_COUNTER_ROWS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_MAX_COUNTER_ROWS)
                .clamp(MIN_MAX_COUNTER_ROWS, ABSOLUTE_MAX_COUNTER_ROWS)
        });
        let max_note_rows = max_chats.map(|_| {
            std::env::var("MAX_SHARD_NOTE_ROWS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_MAX_NOTE_ROWS)
                .clamp(MIN_MAX_NOTE_ROWS, ABSOLUTE_MAX_NOTE_ROWS)
        });

        let size = std::env::var("DB_POOL")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(8)
            .clamp(2, 64);
        let pool = PgPoolOptions::new()
            .max_connections(size)
            .min_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .idle_timeout(std::time::Duration::from_secs(600))
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
                note_rows    BIGINT NOT NULL DEFAULT 0
            )",
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
            .rows_affected() as i64;
            sqlx::query(
                "UPDATE started_users_meta
                 SET total = GREATEST(total - $1, 0)
                 WHERE id = 0",
            )
            .bind(deleted)
            .execute(&mut *conn)
            .await?;
        }

        if let Some(max_chats) = max_chats {
            let chat_count: i64 = sqlx::query_scalar(
                "SELECT count(DISTINCT chat_id) FROM settings WHERE chat_id <> 0",
            )
            .fetch_one(&mut *conn)
            .await?;
            if chat_count > max_chats as i64 {
                return Err(sqlx::Error::Protocol(format!(
                    "database has {chat_count} configured chats before migration, above shard limit {max_chats}"
                )));
            }
        }
        Self::migrate_counters(&mut conn).await?;
        Self::migrate_per_user(&mut conn).await?;
        Self::migrate_notes(&mut conn).await?;
        Self::migrate_tallies(&mut conn).await?;
        Self::migrate_wipe_split(&mut conn).await?;
        let (counter_rows, note_rows): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM counters),
                    (SELECT count(*) FROM notes)",
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
        if let Some(max_note_rows) = max_note_rows
            && note_rows > max_note_rows
        {
            return Err(sqlx::Error::Protocol(format!(
                "database has {note_rows} note rows, above shard limit {max_note_rows}"
            )));
        }
        sqlx::query(
            "INSERT INTO durable_counts (id, counter_rows, note_rows)
             VALUES (0, $1, $2)
             ON CONFLICT (id) DO UPDATE SET
                 counter_rows = EXCLUDED.counter_rows,
                 note_rows = EXCLUDED.note_rows",
        )
        .bind(counter_rows)
        .bind(note_rows)
        .execute(&mut *conn)
        .await?;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(SCHEMA_LOCK)
            .execute(&mut *conn)
            .await?;
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
            && (largest_key.is_some_and(|size| size as usize > MAX_SETTING_KEY_BYTES)
                || largest_value.is_some_and(|size| size as usize > MAX_SETTING_VALUE_BYTES))
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
            let (chat_count,): (i64,) = sqlx::query_as(
                "SELECT count(*) FROM (
                     SELECT chat_id FROM settings WHERE chat_id <> 0 GROUP BY chat_id
                 ) AS chats",
            )
            .fetch_one(&pool)
            .await?;
            if chat_count > max_chats as i64 {
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
            if largest_chat_rows > MAX_SETTINGS_ROWS_PER_CHAT as i64 {
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

        Ok(Self {
            pool,
            _process_lock: process_lock,
            cache: RwLock::new(cache),
            index: RwLock::new(index),
            max_chats,
            max_rows,
            max_bytes,
            max_counter_rows,
            max_note_rows,
            setting_rows: AtomicUsize::new(setting_rows),
            setting_bytes: AtomicUsize::new(setting_bytes),
            night_legacy_checked: AtomicBool::new(false),
            write_slots: (0..WRITE_SLOTS)
                .map(|_| std::sync::Arc::new(tokio::sync::Mutex::new(())))
                .collect(),
        })
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

        for (prefix, column, part, name) in MOVES {
            let moved = sqlx::query(&format!(
                "INSERT INTO counters (chat_id, user_id, name, {column})
                 SELECT chat_id,
                        split_part(key, ':', 2)::bigint,
                        {name},
                        CASE WHEN split_part(value, '|', {part}) ~ '^[0-9]+$'
                             THEN split_part(value, '|', {part})::bigint ELSE 0 END
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

        let moved = sqlx::query(
            "INSERT INTO counters (chat_id, user_id, warns)
             SELECT chat_id,
                    split_part(key, ':', 2)::bigint,
                    CASE WHEN value ~ '^[0-9]+$' THEN value::bigint ELSE 0 END
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
                    CASE WHEN split_part(value, ':', 1) ~ '^[0-9]+$'
                         THEN split_part(value, ':', 1)::bigint ELSE 0 END,
                    CASE WHEN split_part(value, ':', 2) ~ '^[0-9]+$'
                         THEN split_part(value, ':', 2)::bigint ELSE 0 END
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

        let moved = sqlx::query(
            "INSERT INTO tallies (chat_id, counter, day, count)
             SELECT chat_id,
                    substring(key from 7),
                    CASE WHEN split_part(value, '|', 1) ~ '^[0-9]+$'
                         THEN split_part(value, '|', 1)::bigint ELSE 0 END,
                    CASE WHEN split_part(value, '|', 2) ~ '^[0-9]+$'
                         THEN split_part(value, '|', 2)::bigint ELSE 0 END
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

    pub async fn board(&self, chat: i64, period: Period, stamp: u64, limit: i64) -> Vec<Counter> {
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
            query = query.bind(stamp as i64);
        }
        match query.bind(limit).fetch_all(&self.pool).await {
            Ok(rows) => rows
                .into_iter()
                .map(|(user, name, count)| Counter {
                    user,
                    name,
                    count: count.max(0) as u64,
                })
                .collect(),
            Err(e) => {
                eprintln!("counters: board {count} for {chat} failed: {e}");
                Vec::new()
            }
        }
    }

    pub async fn board_totals(&self, chat: i64, period: Period, stamp: u64) -> (u64, u64) {
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
            query = query.bind(stamp as i64);
        }
        match query.fetch_optional(&self.pool).await {
            Ok(Some((sum, users))) => (sum.max(0) as u64, users.max(0) as u64),
            Ok(None) => (0, 0),
            Err(e) => {
                eprintln!("counters: totals {count} for {chat} failed: {e}");
                (0, 0)
            }
        }
    }

    pub async fn idle(&self, chat: i64, day: u64, days: u64, limit: i64) -> Idle {
        let rows: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT user_id, name, $2 - seen FROM counters
             WHERE chat_id = $1 AND seen > 0 AND $2 - seen >= $3
             ORDER BY seen ASC LIMIT $4",
        )
        .bind(chat)
        .bind(day as i64)
        .bind(days as i64)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("counters: idle for {chat} failed: {e}");
            Vec::new()
        });

        let total: i64 = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM counters
             WHERE chat_id = $1 AND seen > 0 AND $2 - seen >= $3",
        )
        .bind(chat)
        .bind(day as i64)
        .bind(days as i64)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map_or(0, |(count,)| count);

        let idle = rows
            .into_iter()
            .map(|(user, name, quiet)| (user, name, quiet.max(0) as u64))
            .collect();
        (idle, total.max(0) as u64)
    }

    pub async fn bump(&self, bumps: Vec<Bump>, day: u64, week: u64, month: u64) -> Vec<Bumped> {
        const CHUNK: usize = 5_000;

        const AT_ONCE: usize = 4;
        const FOLD: &str = "WITH batch AS (
                 SELECT chat, member, who, added
                 FROM UNNEST($1::bigint[], $2::bigint[], $3::text[], $4::bigint[])
                      AS incoming(chat, member, who, added)
             ), existing AS (
                 SELECT chat_id, count(*)::bigint AS members
                 FROM counters
                 WHERE chat_id = ANY($1::bigint[])
                 GROUP BY chat_id
             ), existing_batch AS (
                 SELECT batch.chat, batch.member, batch.who, batch.added
                 FROM batch
                 WHERE EXISTS (
                     SELECT 1 FROM counters
                     WHERE counters.chat_id = batch.chat
                       AND counters.user_id = batch.member
                 )
             ), new_batch AS (
                 SELECT batch.chat, batch.member, batch.who, batch.added,
                        row_number() OVER (
                            PARTITION BY batch.chat ORDER BY batch.member
                        ) AS new_rank
                 FROM batch
                 WHERE NOT EXISTS (
                     SELECT 1 FROM counters
                     WHERE counters.chat_id = batch.chat
                       AND counters.user_id = batch.member
                 )
             ), allowed_new AS (
                 SELECT new_batch.chat, new_batch.member, new_batch.who, new_batch.added
                 FROM new_batch
                 LEFT JOIN existing ON existing.chat_id = new_batch.chat
                 WHERE coalesce(existing.members, 0) + new_batch.new_rank <= $8
             ), reserved AS (
                 UPDATE durable_counts
                 SET counter_rows = counter_rows + (SELECT count(*) FROM allowed_new)
                 WHERE id = 0
                   AND counter_rows + (SELECT count(*) FROM allowed_new) <= $9
                 RETURNING id
             ), write_batch AS (
                 SELECT chat, member, who, added FROM existing_batch
                 UNION ALL
                 SELECT chat, member, who, added
                 FROM allowed_new
                 WHERE EXISTS (SELECT 1 FROM reserved)
             )
             INSERT INTO counters
                 (chat_id, user_id, name, total, today, day, week, week_at, month, month_at, seen)
             SELECT chat, member, who, added, added, $5, added, $6, added, $7, $5
             FROM write_batch
             ON CONFLICT (chat_id, user_id) DO UPDATE SET
                 name  = COALESCE(NULLIF(EXCLUDED.name, ''), counters.name),
                 total = counters.total + EXCLUDED.total,
                 today = CASE WHEN counters.day = EXCLUDED.day
                              THEN counters.today ELSE 0 END + EXCLUDED.today,
                 day   = EXCLUDED.day,
                 week  = CASE WHEN counters.week_at = EXCLUDED.week_at
                              THEN counters.week ELSE 0 END + EXCLUDED.week,
                 week_at = EXCLUDED.week_at,
                 month = CASE WHEN counters.month_at = EXCLUDED.month_at
                              THEN counters.month ELSE 0 END + EXCLUDED.month,
                 month_at = EXCLUDED.month_at,
                 seen  = EXCLUDED.seen
             RETURNING chat_id, user_id, name, total, awarded";

        type Batch = (Vec<i64>, Vec<i64>, Vec<String>, Vec<i64>);
        let max_counter_rows = self.max_counter_rows.unwrap_or(i64::MAX / 2);
        let mut batches: Vec<Batch> = Vec::new();
        let mut batch = Batch::default();
        for bump in bumps {
            batch.0.push(bump.chat);
            batch.1.push(bump.user);
            batch.2.push(bump.name);
            batch.3.push(bump.added as i64);
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
        let mut reap =
            |done: std::result::Result<Vec<BumpedRow>, tokio::task::JoinError>| match done {
                Ok(rows) => {
                    folded.extend(rows.into_iter().map(|(chat, user, name, total, awarded)| {
                        Bumped {
                            chat,
                            user,
                            name,
                            total: total.max(0) as u64,
                            awarded: awarded.max(0) as u64,
                        }
                    }))
                }
                Err(e) => eprintln!("counters: a bump chunk failed to run: {e}"),
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
                let rows: Result<Vec<BumpedRow>> = sqlx::query_as(FOLD)
                    .bind(&chats)
                    .bind(&users)
                    .bind(&names)
                    .bind(&added)
                    .bind(day as i64)
                    .bind(week as i64)
                    .bind(month as i64)
                    .bind(MAX_COUNTER_ROWS_PER_CHAT)
                    .bind(max_counter_rows)
                    .fetch_all(&pool)
                    .await;
                rows.unwrap_or_else(|e| {
                    eprintln!("counters: bump of {} rows failed: {e}", chats.len());
                    Vec::new()
                })
            });
        }

        while let Some(done) = tasks.join_next().await {
            reap(done);
        }
        folded
    }

    pub async fn credit_add(&self, chat: i64, user: i64, name: &str, added: u64) -> u64 {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let max_counter_rows = self.max_counter_rows.unwrap_or(i64::MAX / 2);
        let row: std::result::Result<Option<(i64,)>, _> = sqlx::query_as(
            "WITH existing AS (
                 SELECT 1 FROM counters WHERE chat_id = $1 AND user_id = $2
             ), reserved AS (
                 UPDATE durable_counts
                 SET counter_rows = counter_rows + 1
                 WHERE id = 0
                   AND NOT EXISTS (SELECT 1 FROM existing)
                   AND counter_rows < $5
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
        .bind(added as i64)
        .bind(max_counter_rows)
        .fetch_optional(&self.pool)
        .await;
        match row {
            Ok(Some((adds,))) => adds.max(0) as u64,
            Ok(None) => 0,
            Err(e) => {
                eprintln!("counters: credit for {chat}/{user} failed: {e}");
                0
            }
        }
    }

    pub async fn warns_of(&self, chat: i64, user: i64) -> u32 {
        sqlx::query_as::<_, (i64,)>(
            "SELECT warns FROM counters WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map_or(0, |(warns,)| warns.clamp(0, i64::from(u32::MAX)) as u32)
    }

    pub async fn set_warns(&self, chat: i64, user: i64, warns: u32) {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let max_counter_rows = self.max_counter_rows.unwrap_or(i64::MAX / 2);
        if let Err(e) = sqlx::query(
            "WITH existing AS (
                 SELECT 1 FROM counters WHERE chat_id = $1 AND user_id = $2
             ), reserved AS (
                 UPDATE durable_counts
                 SET counter_rows = counter_rows + 1
                 WHERE id = 0
                   AND NOT EXISTS (SELECT 1 FROM existing)
                   AND counter_rows < $4
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
        .execute(&self.pool)
        .await
        {
            eprintln!("counters: warns for {chat}/{user} failed: {e}");
        }
    }

    pub async fn add_strike(&self, chat: i64, user: i64, day: u64, days: u64) -> u32 {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let max_counter_rows = self.max_counter_rows.unwrap_or(i64::MAX / 2);
        let row: std::result::Result<Option<(i64,)>, _> = sqlx::query_as(
            "WITH existing AS (
                 SELECT 1 FROM counters WHERE chat_id = $1 AND user_id = $2
             ), reserved AS (
                 UPDATE durable_counts
                 SET counter_rows = counter_rows + 1
                 WHERE id = 0
                   AND NOT EXISTS (SELECT 1 FROM existing)
                   AND counter_rows < $5
                 RETURNING id
             )
             INSERT INTO counters (chat_id, user_id, strikes, struck)
             SELECT $1, $2, 1, $3
             WHERE EXISTS (SELECT 1 FROM existing) OR EXISTS (SELECT 1 FROM reserved)
             ON CONFLICT (chat_id, user_id) DO UPDATE SET
                 strikes = CASE WHEN $3 - counters.struck < $4 THEN counters.strikes ELSE 0 END + 1,
                 struck  = $3
             RETURNING strikes",
        )
        .bind(chat)
        .bind(user)
        .bind(day as i64)
        .bind(days as i64)
        .bind(max_counter_rows)
        .fetch_optional(&self.pool)
        .await;
        match row {
            Ok(Some((strikes,))) => strikes.clamp(0, i64::from(u32::MAX)) as u32,
            Ok(None) => 0,
            Err(e) => {
                eprintln!("counters: strike for {chat}/{user} failed: {e}");
                0
            }
        }
    }

    pub async fn clear_strikes(&self, chat: i64, user: i64) {
        if let Err(e) = sqlx::query(
            "UPDATE counters SET strikes = 0, struck = 0 WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .execute(&self.pool)
        .await
        {
            eprintln!("counters: clearing strikes for {chat}/{user} failed: {e}");
        }
    }

    pub async fn adds_of(&self, chat: i64, user: i64) -> u64 {
        sqlx::query_as::<_, (i64,)>("SELECT adds FROM counters WHERE chat_id = $1 AND user_id = $2")
            .bind(chat)
            .bind(user)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map_or(0, |(adds,)| adds.max(0) as u64)
    }

    pub async fn card(&self, chat: i64, user: i64, day: u64) -> Card {
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
        .bind(day as i64)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("counters: card for {chat}/{user} failed: {e}");
            None
        });
        let Some((today, total, adds, place)) = row else {
            return Card::default();
        };
        Card {
            today: today.max(0) as u64,
            total: total.max(0) as u64,
            adds: adds.max(0) as u64,
            place: (today > 0).then(|| place.max(1) as u64),
        }
    }

    pub async fn name_of(&self, chat: i64, user: i64) -> Option<String> {
        sqlx::query_as::<_, (String,)>(
            "SELECT name FROM counters WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(|(name,)| name)
        .filter(|name| !name.is_empty())
    }

    pub async fn set_awarded(&self, chat: i64, user: i64, milestone: u64) {
        if let Err(e) =
            sqlx::query("UPDATE counters SET awarded = $3 WHERE chat_id = $1 AND user_id = $2")
                .bind(chat)
                .bind(user)
                .bind(milestone as i64)
                .execute(&self.pool)
                .await
        {
            eprintln!("counters: award for {chat}/{user} failed: {e}");
        }
    }

    pub async fn clear_seen(&self, chat: i64, user: i64) {
        if let Err(e) =
            sqlx::query("UPDATE counters SET seen = 0 WHERE chat_id = $1 AND user_id = $2")
                .bind(chat)
                .bind(user)
                .execute(&self.pool)
                .await
        {
            eprintln!("counters: clearing seen for {chat}/{user} failed: {e}");
        }
    }

    pub async fn save_pending(&self, rows: Vec<(i64, i32, i64)>) {
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
            if let Err(e) = result {
                eprintln!(
                    "pending deletes: batch write failed ({} rows): {e}",
                    batch.len()
                );
            }
        }
    }

    pub async fn drop_pending(&self, chat: i64, ids: &[i32]) {
        let result =
            sqlx::query("DELETE FROM pending_deletes WHERE chat_id = $1 AND message_id = ANY($2)")
                .bind(chat)
                .bind(ids)
                .execute(&self.pool)
                .await;
        if let Err(e) = result {
            eprintln!("pending deletes: {chat}: clear failed: {e}");
        }
    }

    pub async fn drop_pending_rows(&self, rows: &[(i64, i32)]) -> Vec<(i64, i32)> {
        const CHUNK: usize = 5_000;
        let mut failed = Vec::new();
        for batch in rows.chunks(CHUNK) {
            let (mut chats, mut ids) = (
                Vec::with_capacity(batch.len()),
                Vec::with_capacity(batch.len()),
            );
            for (chat, id) in batch {
                chats.push(*chat);
                ids.push(*id);
            }
            let result = sqlx::query(
                "DELETE FROM pending_deletes AS pending
                 USING UNNEST($1::bigint[], $2::int[]) AS dropped(chat_id, message_id)
                 WHERE pending.chat_id = dropped.chat_id
                   AND pending.message_id = dropped.message_id",
            )
            .bind(&chats)
            .bind(&ids)
            .execute(&self.pool)
            .await;
            if let Err(e) = result {
                eprintln!(
                    "pending deletes: overflow cleanup of {} rows failed: {e}",
                    batch.len()
                );
                failed.extend_from_slice(batch);
            }
        }
        failed
    }

    pub async fn load_pending(&self, now: i64) -> Vec<(i64, i32, i64)> {
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
            .await;
            let count = match done {
                Ok(done) => done.rows_affected(),
                Err(e) => {
                    eprintln!("pending deletes: could not drop the overdue: {e}");
                    break;
                }
            };
            dropped += count;
            if count < STALE_BATCH as u64 {
                break;
            }

            tokio::task::yield_now().await;
        }
        if dropped > 0 {
            println!("pending deletes: dropped {dropped} long overdue");
        }

        match sqlx::query_as(
            "SELECT chat_id, message_id, due_at FROM pending_deletes
             WHERE due_at >= $1 ORDER BY due_at LIMIT $2",
        )
        .bind(floor)
        .bind(MOST)
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("pending deletes: load failed: {e}");
                Vec::new()
            }
        }
    }

    pub async fn forget_idle(&self, chats: &[i64], before: i64) -> u64 {
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
        match sqlx::query_scalar::<_, i64>(
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
        .await
        {
            Ok(deleted) => deleted.max(0) as u64,
            Err(e) => {
                eprintln!("counters: forget failed: {e}");
                0
            }
        }
    }

    pub async fn image_filters(&self, chat: i64) -> Vec<ImageFilterRow> {
        let rows: Vec<ImageFilterTuple> = match sqlx::query_as(
            "SELECT name, vec, scale, cut, rate, live, samples, calibrated FROM image_filters
             WHERE chat_id = $1 ORDER BY name LIMIT 8",
        )
        .bind(chat)
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("image filters: could not read {chat}: {e}");
                Vec::new()
            }
        };
        rows.into_iter().map(Self::image_filter_row).collect()
    }

    pub async fn image_filter(&self, chat: i64, name: &str) -> Option<ImageFilterRow> {
        let row: Option<ImageFilterTuple> = match sqlx::query_as(
            "SELECT name, vec, scale, cut, rate, live, samples, calibrated FROM image_filters
             WHERE chat_id = $1 AND name = $2",
        )
        .bind(chat)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(row) => row,
            Err(e) => {
                eprintln!("image filters: could not read {chat}/{name}: {e}");
                None
            }
        };
        row.map(Self::image_filter_row)
    }

    fn image_filter_row(row: ImageFilterTuple) -> ImageFilterRow {
        let (name, vector, scale, cut, rate, live, samples, calibrated) = row;
        ImageFilterRow {
            name,
            vector,
            scale,
            cut,
            rate: rate.max(0) as u32,
            live,
            samples: samples.max(0) as u32,
            calibrated,
        }
    }

    pub async fn normalize_fixed_image_filters(&self, cut: f32) -> u64 {
        match sqlx::query(
            "UPDATE image_filters
             SET cut = $1
             WHERE calibrated = TRUE AND samples = 0 AND live = TRUE AND cut <> $1",
        )
        .bind(cut)
        .execute(&self.pool)
        .await
        {
            Ok(result) => result.rows_affected(),
            Err(e) => {
                eprintln!("image filters: could not harden fixed cuts: {e}");
                0
            }
        }
    }

    pub async fn fixed_image_filter_keys(&self) -> Vec<(i64, String)> {
        match sqlx::query_as(
            "SELECT chat_id, name FROM image_filters
             WHERE calibrated = TRUE AND samples = 0 AND live = TRUE
             ORDER BY chat_id, name",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("image filters: could not list fixed filters: {e}");
                Vec::new()
            }
        }
    }

    pub async fn phrase_image_filter_keys(&self) -> Vec<(i64, String)> {
        match sqlx::query_as(
            "SELECT chat_id, name FROM image_filters WHERE samples = 0 ORDER BY chat_id, name",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("image filters: could not list phrase filters: {e}");
                Vec::new()
            }
        }
    }

    pub async fn example_image_filter_keys(&self) -> Vec<(i64, String)> {
        match sqlx::query_as(
            "SELECT chat_id, name FROM image_filters WHERE samples > 0 ORDER BY chat_id, name",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("image filters: could not list example filters: {e}");
                Vec::new()
            }
        }
    }

    pub async fn clear_samples(&self) {
        if let Err(e) = sqlx::query("DELETE FROM calibration WHERE id = 0")
            .execute(&self.pool)
            .await
        {
            eprintln!("calibration: could not clear the reservoir: {e}");
        }
    }

    pub async fn save_samples(&self, vectors: &[u8], scale: f32, count: u32) {
        if let Err(e) = sqlx::query(
            "INSERT INTO calibration (id, vecs, scale, count) VALUES (0, $1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET
                 vecs = EXCLUDED.vecs, scale = EXCLUDED.scale, count = EXCLUDED.count",
        )
        .bind(vectors)
        .bind(scale)
        .bind(i32::try_from(count).unwrap_or(i32::MAX))
        .execute(&self.pool)
        .await
        {
            eprintln!("calibration: could not save the reservoir: {e}");
        }
    }

    pub async fn load_samples(&self) -> Option<(Vec<u8>, f32, u32)> {
        let row: (Vec<u8>, f32, i32) =
            sqlx::query_as("SELECT vecs, scale, count FROM calibration WHERE id = 0")
                .fetch_optional(&self.pool)
                .await
                .ok()??;
        Some((row.0, row.1, row.2.max(0) as u32))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_image_filter(
        &self,
        chat: i64,
        name: &str,
        vector: &[u8],
        scale: f32,
        cut: f32,
        rate: u32,
        live: bool,
        samples: u32,
        calibrated: bool,
    ) -> bool {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let existing: bool = match sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM image_filters WHERE chat_id = $1 AND name = $2
             )",
        )
        .bind(chat)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        {
            Ok(existing) => existing,
            Err(e) => {
                eprintln!("image filters: could not check {chat}/{name}: {e}");
                return false;
            }
        };
        if !existing {
            let count: i64 =
                match sqlx::query_scalar("SELECT count(*) FROM image_filters WHERE chat_id = $1")
                    .bind(chat)
                    .fetch_one(&self.pool)
                    .await
                {
                    Ok(count) => count,
                    Err(e) => {
                        eprintln!("image filters: could not count {chat}: {e}");
                        return false;
                    }
                };
            if count >= MAX_IMAGE_FILTERS_PER_CHAT {
                eprintln!(
                    "image filters: refusing new filter {chat}/{name}; limit is {MAX_IMAGE_FILTERS_PER_CHAT}"
                );
                return false;
            }
        }
        if let Err(e) = sqlx::query(
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
        .await
        {
            eprintln!("image filters: could not write {chat}/{name}: {e}");
            return false;
        }
        true
    }

    pub async fn delete_image_filter(&self, chat: i64, name: &str) {
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        if let Err(e) = sqlx::query("DELETE FROM image_filters WHERE chat_id = $1 AND name = $2")
            .bind(chat)
            .bind(name)
            .execute(&self.pool)
            .await
        {
            eprintln!("image filters: could not delete {chat}/{name}: {e}");
        }
    }

    pub fn with_chat<T>(&self, chat: i64, f: impl FnOnce(ChatSettings<'_>) -> T) -> T {
        let cache = self.cache.read().unwrap();
        f(ChatSettings(cache.get(&chat)))
    }

    pub fn is_locked(&self, chat: i64, lock: &str) -> bool {
        self.with_chat(chat, |settings| settings.is_locked(lock))
    }

    pub async fn set(&self, chat: i64, lock: &str, on: bool) -> bool {
        if lock.len() > MAX_SETTING_KEY_BYTES {
            eprintln!("settings: refusing {chat}; key is too large");
            return false;
        }
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;

        let previous;
        let added;
        let removed;
        let byte_delta;
        {
            let mut cache = self.cache.write().unwrap();
            let chat_is_new = chat != 0 && !cache.contains_key(&chat);
            let chat_limit_reached = chat_is_new
                && self.max_chats.is_some_and(|max| {
                    cache
                        .len()
                        .saturating_sub(usize::from(cache.contains_key(&0)))
                        >= max
                });
            let existing = cache.get(&chat).and_then(|map| map.get(lock));
            if on && existing.is_none() {
                let map_len = cache.get(&chat).map_or(0, HashMap::len);
                if map_len >= MAX_SETTINGS_ROWS_PER_CHAT
                    || chat_limit_reached
                    || self
                        .max_rows
                        .is_some_and(|max| self.setting_rows.load(Ordering::Relaxed) >= max)
                {
                    eprintln!("settings: refusing {chat}/{lock}; settings capacity reached");
                    return false;
                }
                let new_bytes = setting_size(lock, "");
                if self.max_bytes.is_some_and(|max| {
                    self.setting_bytes
                        .load(Ordering::Relaxed)
                        .saturating_add(new_bytes)
                        > max
                }) {
                    eprintln!("settings: refusing {chat}/{lock}; setting byte capacity reached");
                    return false;
                }
            }
            let map = cache.entry(chat).or_default();
            let changed = if on {
                let already = map.get(lock).is_some_and(|value| value.is_empty());
                previous = map.insert(lock.to_owned(), String::new());
                let old_bytes = previous
                    .as_deref()
                    .map(|value| setting_size(lock, value))
                    .unwrap_or(0);
                byte_delta = setting_size(lock, "") as isize - old_bytes as isize;
                !already
            } else {
                previous = map.remove(lock);
                byte_delta = previous
                    .as_deref()
                    .map(|value| -(setting_size(lock, value) as isize))
                    .unwrap_or(0);
                previous.is_some()
            };
            if !changed {
                return false;
            }
            added = on && previous.is_none();
            removed = !on && previous.is_some();
            if added {
                self.setting_rows.fetch_add(1, Ordering::Relaxed);
            }
            if removed {
                self.setting_rows.fetch_sub(1, Ordering::Relaxed);
            }
            if byte_delta >= 0 {
                self.setting_bytes
                    .fetch_add(byte_delta as usize, Ordering::Relaxed);
            } else {
                self.setting_bytes
                    .fetch_sub(byte_delta.unsigned_abs(), Ordering::Relaxed);
            }
            if removed && map.is_empty() && chat != 0 {
                cache.remove(&chat);
            }
        }
        self.reindex(chat, lock, on);
        let result = if on {
            sqlx::query(
                "INSERT INTO settings (chat_id, key) VALUES ($1, $2)
                 ON CONFLICT (chat_id, key) DO NOTHING",
            )
            .bind(chat)
            .bind(lock)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query("DELETE FROM settings WHERE chat_id = $1 AND key = $2")
                .bind(chat)
                .bind(lock)
                .execute(&self.pool)
                .await
        };
        if let Err(e) = result {
            eprintln!("settings write failed for {chat}/{lock}: {e}");
            if added {
                self.setting_rows.fetch_sub(1, Ordering::Relaxed);
            }
            if removed {
                self.setting_rows.fetch_add(1, Ordering::Relaxed);
            }
            if byte_delta >= 0 {
                self.setting_bytes
                    .fetch_sub(byte_delta as usize, Ordering::Relaxed);
            } else {
                self.setting_bytes
                    .fetch_add(byte_delta.unsigned_abs(), Ordering::Relaxed);
            }

            self.revert(chat, lock, previous);
        }
        true
    }

    pub async fn set_flags(&self, chats: &[i64], key: &str, on: bool) -> usize {
        if key.len() > MAX_SETTING_KEY_BYTES || chats.is_empty() {
            return 0;
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

        let mut changes: Vec<(i64, Option<String>)> = Vec::new();
        let mut db_ids = Vec::new();
        if on {
            let mut projected_rows = self.setting_rows.load(Ordering::Relaxed);
            let mut projected_bytes = self.setting_bytes.load(Ordering::Relaxed);
            let cache = self.cache.read().unwrap();
            for chat in ids.iter().copied() {
                let existing = cache.get(&chat).and_then(|map| map.get(key));
                if existing.is_some_and(String::is_empty) {
                    db_ids.push(chat);
                    continue;
                }
                if existing.is_none() {
                    let map_len = cache.get(&chat).map_or(0, HashMap::len);
                    let chat_limit_reached = chat != 0
                        && !cache.contains_key(&chat)
                        && self.max_chats.is_some_and(|max| {
                            cache
                                .len()
                                .saturating_sub(usize::from(cache.contains_key(&0)))
                                >= max
                        });
                    if map_len >= MAX_SETTINGS_ROWS_PER_CHAT
                        || chat_limit_reached
                        || self.max_rows.is_some_and(|max| projected_rows >= max)
                    {
                        continue;
                    }
                    let bytes = setting_size(key, "");
                    if self
                        .max_bytes
                        .is_some_and(|max| projected_bytes.saturating_add(bytes) > max)
                    {
                        continue;
                    }
                    projected_rows = projected_rows.saturating_add(1);
                    projected_bytes = projected_bytes.saturating_add(bytes);
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
            return 0;
        }

        let result = async {
            let mut tx = self.pool.begin().await?;
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
                .await?;
            } else {
                sqlx::query("DELETE FROM settings WHERE key = $1 AND chat_id = ANY($2::bigint[])")
                    .bind(key)
                    .bind(&db_ids)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await
        }
        .await;
        if let Err(e) = result {
            eprintln!(
                "settings batch flag {key}: could not write {} rows: {e}",
                db_ids.len()
            );
            return 0;
        }

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
        changed
    }

    pub async fn remember_started_user(&self, user: i64, access_hash: i64) {
        let slot = self.write_slot(0);
        let _writing = slot.lock().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
            .unwrap_or(0);
        let result = async {
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
                    .rows_affected() as i64;
                    sqlx::query(
                        "UPDATE started_users_meta
                         SET total = GREATEST(total - $1, 0)
                         WHERE id = 0",
                    )
                    .bind(deleted)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            tx.commit().await
        }
        .await;
        if let Err(e) = result {
            eprintln!("started users: could not remember {user}: {e}");
        }
    }

    pub async fn started_user(&self, user: i64) -> Option<i64> {
        match sqlx::query_as::<_, (i64,)>(
            "SELECT access_hash FROM started_users WHERE user_id = $1",
        )
        .bind(user)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(row) => row.map(|(access_hash,)| access_hash),
            Err(e) => {
                eprintln!("started users: could not look up {user}: {e}");
                None
            }
        }
    }

    fn write_slot_index(&self, chat: i64) -> usize {
        let hash = (chat as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        (hash as usize) & (WRITE_SLOTS - 1)
    }

    fn write_slot(&self, chat: i64) -> &tokio::sync::Mutex<()> {
        self.write_slots[self.write_slot_index(chat)].as_ref()
    }

    fn revert(&self, chat: i64, key: &str, previous: Option<String>) {
        let present = previous.is_some();
        {
            let mut cache = self.cache.write().unwrap();
            match previous {
                Some(value) => {
                    cache.entry(chat).or_default().insert(key.to_owned(), value);
                }
                None => {
                    let remove_chat = cache.get_mut(&chat).is_some_and(|map| {
                        map.remove(key);
                        map.is_empty()
                    });
                    if remove_chat && chat != 0 {
                        cache.remove(&chat);
                    }
                }
            };
        }
        self.reindex(chat, key, present);
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

    pub async fn add_tallies(&self, rows: &[(i64, &'static str, u64)], day: u64) {
        const CHUNK: usize = 1_000;
        const AT_ONCE: usize = 4;
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
        let mut tasks = tokio::task::JoinSet::new();
        for batch in rows.chunks(CHUNK) {
            let (mut chats, mut counters, mut added) = (Vec::new(), Vec::new(), Vec::new());
            for (chat, counter, count) in batch {
                chats.push(*chat);
                counters.push((*counter).to_owned());
                added.push(*count as i64);
            }
            while let Some(done) = tasks.try_join_next() {
                if let Err(e) = done {
                    eprintln!("tallies: a batch task failed: {e}");
                }
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
                        if let Some(Err(e)) = done {
                            eprintln!("tallies: a batch task failed: {e}");
                        }
                    }
                }
            };
            let pool = self.pool.clone();
            tasks.spawn(async move {
                let _permit = permit;
                let result = sqlx::query(
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
                .bind(day as i64)
                .execute(&pool)
                .await;
                if let Err(e) = result {
                    eprintln!("tallies: batch of {} rows failed: {e}", chats.len());
                }
            });
        }
        while let Some(done) = tasks.join_next().await {
            if let Err(e) = done {
                eprintln!("tallies: a batch task failed: {e}");
            }
        }
    }

    pub async fn tallies(&self, chat: i64, day: u64) -> HashMap<String, u64> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT counter, count FROM tallies WHERE chat_id = $1 AND day = $2 AND count > 0",
        )
        .bind(chat)
        .bind(day as i64)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("tallies: read for {chat} failed: {e}");
            Vec::new()
        });
        rows.into_iter()
            .map(|(counter, count)| (counter, count.max(0) as u64))
            .collect()
    }

    pub async fn chats_with(&self, key: &str) -> Vec<i64> {
        let rows: Vec<(i64,)> =
            sqlx::query_as("SELECT chat_id FROM settings WHERE key = $1 AND value <> ''")
                .bind(key)
                .fetch_all(&self.pool)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("settings: chats with {key} failed: {e}");
                    Vec::new()
                });
        rows.into_iter().map(|(chat,)| chat).collect()
    }

    pub async fn chats_with_values(&self, key: &str, values: &[String]) -> Vec<i64> {
        if values.is_empty() {
            return Vec::new();
        }
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT chat_id FROM settings
             WHERE key = $1 AND value = ANY($2::text[]) AND value <> ''",
        )
        .bind(key)
        .bind(values)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("settings: due chats for {key} failed: {e}");
            Vec::new()
        });
        rows.into_iter().map(|(chat,)| chat).collect()
    }

    pub async fn night_due(&self, minutes: &[String]) -> Vec<i64> {
        if minutes.is_empty() {
            return Vec::new();
        }
        let mut rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT night.chat_id
             FROM settings AS night
             WHERE night.key = 'night'
               AND night.value <> ''
               AND night.value ~ '^[0-9]{1,4}\\|[0-9]{1,4}$'
               AND split_part(night.value, '|', 1) = ANY($1::text[])
             UNION
             SELECT night.chat_id
             FROM settings AS night
             WHERE night.key = 'night'
               AND night.value <> ''
               AND night.value ~ '^[0-9]{1,4}\\|[0-9]{1,4}$'
               AND split_part(night.value, '|', 2) = ANY($1::text[])",
        )
        .bind(minutes)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("settings: indexed night chats failed: {e}");
            Vec::new()
        });

        if !self.night_legacy_checked.load(Ordering::Acquire) {
            let missing: Vec<(i64,)> = sqlx::query_as(
                "SELECT night.chat_id
                 FROM settings AS night
                 WHERE night.key = 'night'
                   AND night.value <> ''
                   AND night.value ~ '^[0-9]{1,4}\\|[0-9]{1,4}$'
                   AND NOT EXISTS (
                       SELECT 1 FROM settings AS state
                       WHERE state.chat_id = night.chat_id AND state.key = 'night_state'
                   )",
            )
            .fetch_all(&self.pool)
            .await
            .unwrap_or_else(|e| {
                eprintln!("settings: unmarked night chats failed: {e}");
                Vec::new()
            });
            if missing.is_empty() {
                self.night_legacy_checked.store(true, Ordering::Release);
            }
            rows.extend(missing);
        }
        rows.sort_unstable();
        rows.dedup();
        rows.into_iter().map(|(chat,)| chat).collect()
    }

    pub async fn group_locks_due(&self, now: &str) -> Vec<i64> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT chat_id FROM settings
             WHERE key = 'glock_until' AND value <> '' AND value <= $1",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("settings: due group locks failed: {e}");
            Vec::new()
        });
        rows.into_iter().map(|(chat,)| chat).collect()
    }

    pub async fn fleet(&self, day: u64) -> Fleet {
        let row: Option<(i64, i64, i64, i64, i64, i64)> = sqlx::query_as(
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
        .bind(day as i64)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("settings: fleet failed: {e}");
            None
        });
        let Some((chats, configured, members, active, today, total)) = row else {
            return Fleet::default();
        };
        let at = |value: i64| value.max(0) as u64;
        Fleet {
            chats: at(chats),
            configured: at(configured),
            members: at(members),
            active_today: at(active),
            messages_today: at(today),
            messages_total: at(total),
        }
    }

    pub async fn busiest(&self, day: u64, limit: i64) -> Vec<(i64, u64)> {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT chat_id, sum(today)::bigint FROM counters
              WHERE day = $1 GROUP BY chat_id HAVING sum(today) > 0
              ORDER BY 2 DESC LIMIT $2",
        )
        .bind(day as i64)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("settings: busiest failed: {e}");
            Vec::new()
        });
        rows.into_iter()
            .map(|(chat, count)| (chat, count.max(0) as u64))
            .collect()
    }

    pub async fn adoption(&self, keys: &[&str]) -> HashMap<String, u64> {
        let asked: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT key, count(*) FROM settings WHERE key = ANY($1) GROUP BY key")
                .bind(&asked)
                .fetch_all(&self.pool)
                .await
                .unwrap_or_else(|e| {
                    eprintln!("settings: adoption failed: {e}");
                    Vec::new()
                });
        rows.into_iter()
            .map(|(key, count)| (key, count.max(0) as u64))
            .collect()
    }

    pub async fn badge_rows(&self, limit: i64) -> Vec<(i64, i64)> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT chat_id, key FROM settings
             WHERE key LIKE 'badge:%' ORDER BY chat_id, key LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("settings: badge rows failed: {e}");
            Vec::new()
        });
        rows.into_iter()
            .filter_map(|(chat, key)| Some((chat, key.strip_prefix("badge:")?.parse().ok()?)))
            .collect()
    }

    pub async fn flagged_with(&self, flag: &str) -> Vec<i64> {
        let rows: Vec<(i64,)> = sqlx::query_as("SELECT chat_id FROM settings WHERE key = $1")
            .bind(flag)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_else(|e| {
                eprintln!("settings: chats flagged {flag} failed: {e}");
                Vec::new()
            });
        rows.into_iter().map(|(chat,)| chat).collect()
    }

    pub async fn panels_for(&self, user: i64, limit: i64) -> Vec<i64> {
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
        .await
        .unwrap_or_else(|e| {
            eprintln!("settings: panels for {user} failed: {e}");
            Vec::new()
        });
        rows.into_iter().map(|(chat,)| chat).collect()
    }

    pub async fn ping(&self) -> Option<std::time::Duration> {
        let started = std::time::Instant::now();
        sqlx::query("SELECT 1").execute(&self.pool).await.ok()?;
        Some(started.elapsed())
    }

    pub fn pool_stats(&self) -> (u32, usize) {
        (self.pool.size(), self.pool.num_idle())
    }

    pub async fn durable_counts(&self) -> (i64, i64) {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT counter_rows, note_rows FROM durable_counts WHERE id = 0",
        )
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("capacity: durable row telemetry failed: {e}");
            None
        })
        .unwrap_or((0, 0))
    }

    pub async fn note(&self, chat: i64, user: i64) -> Option<String> {
        sqlx::query_as::<_, (String,)>(
            "SELECT value FROM notes WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            eprintln!("notes: read for {chat}/{user} failed: {e}");
            None
        })
        .map(|(value,)| value)
    }

    pub async fn set_note(&self, chat: i64, user: i64, value: &str) {
        if value.len() > MAX_SETTING_VALUE_BYTES {
            eprintln!("notes: refusing {chat}/{user}; note is too large");
            return;
        }
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let max_note_rows = self.max_note_rows.unwrap_or(i64::MAX / 2);
        let result: Result<()> = async {
            let mut tx = self.pool.begin().await?;
            if value.is_empty() {
                let deleted = sqlx::query("DELETE FROM notes WHERE chat_id = $1 AND user_id = $2")
                    .bind(chat)
                    .bind(user)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected();
                if deleted > 0 {
                    sqlx::query(
                        "UPDATE durable_counts
                         SET note_rows = GREATEST(note_rows - $1, 0)
                         WHERE id = 0",
                    )
                    .bind(deleted as i64)
                    .execute(&mut *tx)
                    .await?;
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
                .await?;
                if !existing {
                    let count: i64 =
                        sqlx::query_scalar("SELECT count(*) FROM notes WHERE chat_id = $1")
                            .bind(chat)
                            .fetch_one(&mut *tx)
                            .await?;
                    if count >= MAX_NOTE_ROWS_PER_CHAT {
                        eprintln!(
                            "notes: refusing {chat}/{user}; limit is {MAX_NOTE_ROWS_PER_CHAT}"
                        );
                        return Ok(());
                    }
                    let reserved: Option<(i16,)> = sqlx::query_as(
                        "UPDATE durable_counts
                         SET note_rows = note_rows + 1
                         WHERE id = 0 AND note_rows < $1
                         RETURNING id",
                    )
                    .bind(max_note_rows)
                    .fetch_optional(&mut *tx)
                    .await?;
                    if reserved.is_none() {
                        eprintln!("notes: refusing {chat}/{user}; shard limit is {max_note_rows}");
                        return Ok(());
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
                .await?;
            }
            tx.commit().await
        }
        .await;
        if let Err(e) = result {
            eprintln!("notes: write for {chat}/{user} failed: {e}");
        }
    }

    pub fn chats(&self) -> Vec<i64> {
        self.cache
            .read()
            .unwrap()
            .keys()
            .copied()
            .filter(|chat| *chat != 0)
            .collect()
    }

    pub fn chat_count(&self) -> usize {
        let cache = self.cache.read().unwrap();
        cache
            .len()
            .saturating_sub(usize::from(cache.contains_key(&0)))
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

    pub fn value_parsed<T: std::str::FromStr>(&self, chat: i64, key: &str) -> Option<T> {
        self.cache
            .read()
            .unwrap()
            .get(&chat)?
            .get(key)?
            .parse()
            .ok()
    }

    pub async fn set_value(&self, chat: i64, key: &str, value: &str) -> bool {
        if key.len() > MAX_SETTING_KEY_BYTES || value.len() > MAX_SETTING_VALUE_BYTES {
            eprintln!("settings: refusing {chat}/{key}; key or value is too large");
            return false;
        }
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let previous;
        let byte_delta;
        {
            let mut cache = self.cache.write().unwrap();
            let chat_is_new = chat != 0 && !cache.contains_key(&chat);
            let chat_limit_reached = chat_is_new
                && self.max_chats.is_some_and(|max| {
                    cache
                        .len()
                        .saturating_sub(usize::from(cache.contains_key(&0)))
                        >= max
                });
            let is_new = !cache.get(&chat).is_some_and(|map| map.contains_key(key));
            if is_new
                && (cache
                    .get(&chat)
                    .is_some_and(|map| map.len() >= MAX_SETTINGS_ROWS_PER_CHAT)
                    || chat_limit_reached
                    || self
                        .max_rows
                        .is_some_and(|max| self.setting_rows.load(Ordering::Relaxed) >= max))
            {
                eprintln!("settings: refusing {chat}/{key}; settings capacity reached");
                return false;
            }
            let map = cache.entry(chat).or_default();

            if map.get(key).is_some_and(|stored| stored == value) {
                return true;
            }
            let old_bytes = map
                .get(key)
                .map(|stored| setting_size(key, stored))
                .unwrap_or(0);
            let new_bytes = setting_size(key, value);
            if self.max_bytes.is_some_and(|max| {
                self.setting_bytes
                    .load(Ordering::Relaxed)
                    .saturating_sub(old_bytes)
                    .saturating_add(new_bytes)
                    > max
            }) {
                eprintln!("settings: refusing {chat}/{key}; setting byte capacity reached");
                return false;
            }
            previous = map.insert(key.to_owned(), value.to_owned());
            byte_delta = new_bytes as isize
                - previous
                    .as_deref()
                    .map(|stored| setting_size(key, stored))
                    .unwrap_or(0) as isize;
            if previous.is_none() {
                self.setting_rows.fetch_add(1, Ordering::Relaxed);
            }
            if byte_delta >= 0 {
                self.setting_bytes
                    .fetch_add(byte_delta as usize, Ordering::Relaxed);
            } else {
                self.setting_bytes
                    .fetch_sub(byte_delta.unsigned_abs(), Ordering::Relaxed);
            }
        }
        self.reindex(chat, key, true);
        let result = sqlx::query(
            "INSERT INTO settings (chat_id, key, value) VALUES ($1, $2, $3)
             ON CONFLICT (chat_id, key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(chat)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await;
        if let Err(e) = result {
            eprintln!("settings write failed for {chat}/{key}: {e}");
            if previous.is_none() {
                self.setting_rows.fetch_sub(1, Ordering::Relaxed);
            }
            if byte_delta >= 0 {
                self.setting_bytes
                    .fetch_sub(byte_delta as usize, Ordering::Relaxed);
            } else {
                self.setting_bytes
                    .fetch_add(byte_delta.unsigned_abs(), Ordering::Relaxed);
            }
            self.revert(chat, key, previous);
            return false;
        }
        true
    }

    pub async fn import_file(&self, path: &str) -> usize {
        let Ok(text) = std::fs::read_to_string(path) else {
            return 0;
        };
        let mut imported = 0;
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let Some(Ok(chat)) = parts.next().map(str::parse::<i64>) else {
                continue;
            };
            for part in parts {
                match part.split_once('=') {
                    Some((key, value)) => {
                        let _ = self.set_value(chat, key, value).await;
                    }
                    None => {
                        self.set(chat, part, true).await;
                    }
                }
                imported += 1;
            }
        }
        if imported > 0 {
            let _ = std::fs::rename(path, format!("{path}.imported"));
        }
        imported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_is_defaulted_and_clamped_the_same_way_everywhere() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("low".to_owned(), "1".to_owned());
        map.insert("high".to_owned(), "999".to_owned());
        map.insert("fine".to_owned(), "7".to_owned());
        map.insert("empty".to_owned(), String::new());
        map.insert("words".to_owned(), "زیاد".to_owned());
        let settings = ChatSettings(Some(&map));

        assert_eq!(settings.number("fine", 8, (2, 50)), 7);
        assert_eq!(settings.number("low", 8, (2, 50)), 2);
        assert_eq!(settings.number("high", 8, (2, 50)), 50);

        assert_eq!(settings.number("absent", 8, (2, 50)), 8);
        assert_eq!(settings.number("empty", 8, (2, 50)), 8);
        assert_eq!(settings.number("words", 8, (2, 50)), 8);
        assert_eq!(settings.number("absent", 1, (2, 50)), 2);

        assert_eq!(ChatSettings(None).number("fine", 8, (2, 50)), 8);
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
    #[ignore]
    async fn roundtrip() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_999;

        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        let settings = Settings::connect(&url).await.unwrap();

        settings.set_value(chat, "owner", "42").await;
        settings.set_value(chat, "owner", "7").await;
        assert_eq!(settings.value(chat, "owner").as_deref(), Some("7"));
        assert!(settings.set(chat, "links", true).await);
        assert!(!settings.set(chat, "links", true).await);

        assert!(settings.indexed_empty(chat, "filter:"));
        assert!(settings.set(chat, "filter:بد", true).await);
        settings.set_value(chat, "answer:سلام", "درود").await;
        assert!(!settings.indexed_empty(chat, "filter:"));
        assert!(settings.indexed_any(chat, "filter:", |word| word == "بد"));
        assert!(settings.indexed_any(chat, "answer:", |trigger| trigger == "سلام"));
        assert!(!settings.indexed_any(chat, "filter:", |word| word == "خوب"));

        let reloaded = Settings::connect(&url).await.unwrap();
        assert_eq!(reloaded.value(chat, "owner").as_deref(), Some("7"));
        assert!(reloaded.is_locked(chat, "links"));
        assert!(reloaded.indexed_any(chat, "filter:", |word| word == "بد"));
        assert!(reloaded.indexed_any(chat, "answer:", |trigger| trigger == "سلام"));

        assert!(settings.set(chat, "filter:بد", false).await);
        assert!(settings.indexed_empty(chat, "filter:"));

        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
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

        settings.set_value(on, "night", "1380|420").await;
        settings.set_value(off, "night", "0|60").await;

        let mut listed = settings.chats_with("night").await;
        listed.retain(|chat| both.contains(chat));
        listed.sort_unstable();
        let mut want = both.clone();
        want.sort_unstable();
        assert_eq!(listed, want);

        assert!(settings.set(off, "night", false).await);
        let mut left = settings.chats_with("night").await;
        left.retain(|chat| both.contains(chat));
        assert_eq!(left, vec![on]);

        settings.set(on, "gate_on", true).await;
        assert!(!settings.chats_with("gate_on").await.contains(&on));
        assert!(settings.flagged_with("gate_on").await.contains(&on));

        sqlx::query(wipe)
            .bind(&both)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn batch_flags_reconcile_the_mirror_and_database() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let (missing, existing) = (-999_999_999_985, -999_999_999_984);
        let all = vec![missing, existing];
        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&all)
            .execute(&settings.pool)
            .await
            .unwrap();

        assert!(
            settings
                .set_value(missing, "join_channel", "@example")
                .await
        );
        assert!(settings.set(existing, "gate_on", true).await);
        assert_eq!(settings.set_flags(&all, "gate_on", true).await, 1);
        assert!(settings.is_locked(missing, "gate_on"));
        assert!(settings.is_locked(existing, "gate_on"));
        assert_eq!(settings.set_flags(&all, "gate_on", true).await, 0);

        let rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM settings WHERE key = 'gate_on' AND chat_id = ANY($1)",
        )
        .bind(&all)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(rows, 2);

        assert_eq!(settings.set_flags(&all, "gate_on", false).await, 2);
        assert!(!settings.is_locked(missing, "gate_on"));
        assert!(!settings.is_locked(existing, "gate_on"));
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&all)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
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
            .set(mine_by_flag, &format!("admin:{me}"), true)
            .await;
        settings
            .set_value(mine_by_owner, "owner", &me.to_string())
            .await;
        settings.set_value(theirs, "owner", &them.to_string()).await;
        settings.set(theirs, &format!("admin:{them}"), true).await;

        let mut found = settings.panels_for(me, 21).await;
        found.retain(|chat| all.contains(chat));
        found.sort_unstable();
        let mut mine = vec![mine_by_flag, mine_by_owner];
        mine.sort_unstable();
        assert_eq!(found, mine, "someone else's group must not be listed");

        settings
            .set(mine_by_flag, &format!("admin:{me}"), false)
            .await;
        let mut left = settings.panels_for(me, 21).await;
        left.retain(|chat| all.contains(chat));
        assert_eq!(left, vec![mine_by_owner]);

        sqlx::query(wipe)
            .bind(&all)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn every_chunk_of_a_large_flush_lands() {
        const ROWS: i64 = 12_001;

        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_987;

        let settings = Settings::connect(&url).await.unwrap();
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
        let folded = settings.bump(bumps, 500, 71, 16).await;
        assert_eq!(folded.len() as i64, ROWS, "every chunk has to come back");

        let (total, members) = settings.board_totals(chat, Period::Total, 0).await;
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
        settings.bump(again, 500, 71, 16).await;
        assert_eq!(
            settings.board_totals(chat, Period::Total, 0).await.0 as i64,
            ROWS * 3
        );
        assert_eq!(settings.name_of(chat, 1).await.as_deref(), Some("member 1"));

        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn forget_idle_only_touches_the_chats_it_is_given() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let (a, b) = (-999_999_999_994, -999_999_999_993);

        let settings = Settings::connect(&url).await.unwrap();
        let wipe = "DELETE FROM counters WHERE chat_id = ANY($1)";
        let both = vec![a, b];
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

        settings.bump(vec![spoke(a, 1)], 100, 14, 3).await;
        settings.bump(vec![spoke(a, 2)], 200, 28, 6).await;
        settings.bump(vec![spoke(b, 1)], 100, 14, 3).await;

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

        assert_eq!(settings.forget_idle(&[a], 150).await, 1);
        assert_eq!(left(a).await, 1, "the member still talking must stay");
        assert_eq!(
            left(b).await,
            1,
            "a chat that was not named must be untouched"
        );

        assert_eq!(settings.forget_idle(&[b], 150).await, 1);
        assert_eq!(left(b).await, 0);

        sqlx::query(wipe)
            .bind(&both)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn tallies_roll_over_in_the_write() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_995;
        let (day, next) = (2_000, 2_001);

        let settings = Settings::connect(&url).await.unwrap();
        let wipe = "DELETE FROM tallies WHERE chat_id = $1";
        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        settings
            .add_tallies(&[(chat, "k_text", 3), (chat, "h9", 3)], day)
            .await;
        settings.add_tallies(&[(chat, "k_text", 5)], day).await;

        let today = settings.tallies(chat, day).await;
        assert_eq!(today.get("k_text").copied(), Some(8));
        assert_eq!(today.get("h9").copied(), Some(3));

        settings.add_tallies(&[(chat, "k_text", 4)], next).await;
        let tomorrow = settings.tallies(chat, next).await;
        assert_eq!(tomorrow.get("k_text").copied(), Some(4));

        assert!(!tomorrow.contains_key("h9"));

        let yesterday = settings.tallies(chat, day).await;
        assert!(!yesterday.contains_key("k_text"));
        assert_eq!(yesterday.get("h9").copied(), Some(3));

        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn clearing_a_valued_setting_removes_the_row() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_996;
        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM notes WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        settings.set_note(chat, 7, "برای بعد").await;
        assert_eq!(settings.note(chat, 7).await.as_deref(), Some("برای بعد"));

        settings.set_note(chat, 7, "").await;
        assert!(settings.note(chat, 7).await.is_none());

        let left: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM notes WHERE chat_id = $1 AND user_id = 7")
                .bind(chat)
                .fetch_one(&settings.pool)
                .await
                .unwrap();
        assert_eq!(left.0, 0);

        let reloaded = Settings::connect(&url).await.unwrap();
        assert!(reloaded.note(chat, 7).await.is_none());

        sqlx::query("DELETE FROM notes WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
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

        settings.set_value(chat, "owner", "42").await;
        settings.set(chat, "wipe_on", true).await;
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
            .await;

        let fleet = settings.fleet(day).await;
        assert!(fleet.chats >= 1);
        assert!(fleet.configured >= 1, "the owner row is a configured chat");
        assert!(fleet.members >= 2, "two members were just counted");
        assert!(fleet.active_today >= 1);
        assert!(fleet.messages_today >= 10, "six and four said today");
        assert!(
            fleet.messages_total >= fleet.messages_today,
            "today is part of ever"
        );

        let busiest = settings.busiest(day, 500).await;
        assert!(
            busiest.contains(&(chat, 10)),
            "{busiest:?} is missing the test chat"
        );

        let counts = settings.adoption(&["wipe_on", "nothing_uses_this"]).await;
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
                .iter()
                .any(|(id, _)| *id == chat),
            "a chat with no rows is on no board"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn counters_roll_over() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_998;
        let (day, week, month) = (1_000, 1_000 / 7, 1_000 / 30);

        let settings = Settings::connect(&url).await.unwrap();
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

        settings.bump(both(5, 9), day, week, month).await;
        settings.bump(both(3, 1), day, week, month).await;

        let top = settings.board(chat, Period::Today, day, 10).await;
        assert_eq!(top.len(), 2);
        assert_eq!((top[0].user, top[0].count), (2, 10));
        assert_eq!((top[1].user, top[1].count), (1, 8));
        assert_eq!(
            settings.board_totals(chat, Period::Today, day).await,
            (18, 2)
        );
        assert_eq!(settings.board_totals(chat, Period::Total, 0).await, (18, 2));

        settings.bump(ali(2), day + 1, week, month).await;

        assert_eq!(
            settings.board_totals(chat, Period::Today, day + 1).await,
            (2, 1)
        );
        let today = settings.board(chat, Period::Today, day + 1, 10).await;
        assert_eq!(today.len(), 1);
        assert_eq!((today[0].user, today[0].count), (1, 2));
        assert_eq!(
            settings.board_totals(chat, Period::Week, week).await,
            (20, 2)
        );
        assert_eq!(settings.board_totals(chat, Period::Total, 0).await, (20, 2));

        settings.bump(ali(4), day + 8, week + 1, month).await;
        assert_eq!(
            settings.board_totals(chat, Period::Week, week + 1).await,
            (4, 1)
        );
        assert_eq!(
            settings.board_totals(chat, Period::Month, month).await,
            (24, 2)
        );

        let card = settings.card(chat, 1, day + 8).await;
        assert_eq!((card.total, card.today, card.place), (14, 4, Some(1)));
        assert_eq!(settings.card(chat, 2, day + 8).await.place, None);
        assert_eq!(settings.name_of(chat, 2).await.as_deref(), Some("Sara"));

        let (idle, total) = settings.idle(chat, day + 8, 5, 10).await;
        assert_eq!(total, 1);
        assert_eq!(
            idle.first().map(|(user, _, quiet)| (*user, *quiet)),
            Some((2, 8))
        );

        assert_eq!(settings.credit_add(chat, 1, "Ali", 3).await, 3);
        assert_eq!(settings.credit_add(chat, 1, "Ali", 2).await, 5);
        assert_eq!(settings.adds_of(chat, 1).await, 5);

        settings.clear_seen(chat, 2).await;
        assert_eq!(settings.idle(chat, day + 8, 5, 10).await.1, 0);

        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn strikes_expire_in_the_write() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_997;
        let (user, days) = (7, 7);

        let settings = Settings::connect(&url).await.unwrap();
        let wipe = "DELETE FROM counters WHERE chat_id = $1";
        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        assert_eq!(settings.add_strike(chat, user, 100, days).await, 1);
        assert_eq!(settings.add_strike(chat, user, 101, days).await, 2);
        assert_eq!(settings.add_strike(chat, user, 106, days).await, 3);

        assert_eq!(settings.add_strike(chat, user, 107, days).await, 4);

        assert_eq!(settings.add_strike(chat, user, 114, days).await, 1);

        settings.clear_strikes(chat, user).await;
        assert_eq!(settings.add_strike(chat, user, 114, days).await, 1);

        assert_eq!(settings.warns_of(chat, user).await, 0);
        settings.set_warns(chat, user, 3).await;
        assert_eq!(settings.warns_of(chat, user).await, 3);
        settings.set_warns(chat, user, 0).await;
        assert_eq!(settings.warns_of(chat, user).await, 0);

        assert_eq!(settings.warns_of(chat, 12_345).await, 0);

        sqlx::query(wipe)
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn due_group_locks_compare_as_numbers() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let (early, late) = (-999_999_999_996, -999_999_999_995);

        let wipe = "DELETE FROM settings WHERE chat_id = ANY($1)";
        let chats = vec![early, late];
        sqlx::query(wipe)
            .bind(&chats)
            .execute(&settings.pool)
            .await
            .unwrap();

        let key = super::super::handlers::locks::GROUP_UNTIL;
        let stamp = super::super::handlers::locks::stamp;
        settings.set_value(early, key, &stamp(1_000)).await;
        settings.set_value(late, key, &stamp(9_000)).await;

        let due = settings.group_locks_due(&stamp(5_000)).await;
        assert!(due.contains(&early), "the expired lock is due");
        assert!(!due.contains(&late), "the running lock is not");

        assert_eq!(stamp(900).len(), stamp(1_000_000_000).len());
        assert!(stamp(900) < stamp(1_000_000_000));

        let due = settings.group_locks_due(&stamp(10_000)).await;
        assert!(due.contains(&early) && due.contains(&late));
        settings.set(early, key, false).await;
        settings.set(late, key, false).await;
        let due = settings.group_locks_due(&stamp(10_000)).await;
        assert!(!due.contains(&early) && !due.contains(&late));

        sqlx::query(wipe)
            .bind(&chats)
            .execute(&settings.pool)
            .await
            .unwrap();
    }
}
