use std::collections::HashMap;
use std::sync::RwLock;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

type Result<T> = std::result::Result<T, sqlx::Error>;

pub const INDEXED: &[&str] = &["filter:", "pack:", "answer:", "strict:"];

const COUNTER_PREFIXES: &[&str] =
    &["total:", "today:", "week:", "month:", "seen:", "adds:", "rank:"];

type ChatIndex = [Vec<Box<str>>; INDEXED.len()];

const INDEXES: &[&str] = &[

    "CREATE INDEX IF NOT EXISTS pending_due ON pending_deletes (due_at)",

    "CREATE INDEX IF NOT EXISTS settings_bot_admin ON settings (key) WHERE key LIKE 'admin:%'",
    "CREATE INDEX IF NOT EXISTS settings_owner ON settings (value) WHERE key = 'owner'",

    "CREATE INDEX IF NOT EXISTS settings_night ON settings (chat_id) WHERE key = 'night'",
    "CREATE INDEX IF NOT EXISTS settings_report_at ON settings (chat_id) WHERE key = 'report_at'",
    "CREATE INDEX IF NOT EXISTS settings_purge_at ON settings (chat_id) \
     WHERE key = 'auto_purge_at'",
    "CREATE INDEX IF NOT EXISTS settings_join_channel ON settings (chat_id) \
     WHERE key = 'join_channel'",
    "CREATE INDEX IF NOT EXISTS settings_add_required ON settings (chat_id) \
     WHERE key = 'add_required'",
    "CREATE INDEX IF NOT EXISTS settings_gate_on ON settings (chat_id) WHERE key = 'gate_on'",
];

pub type Idle = (Vec<(i64, String, u64)>, u64);

type BumpedRow = (i64, i64, String, i64, i64);

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

    cache: RwLock<HashMap<i64, HashMap<String, String>>>,

    index: RwLock<HashMap<i64, ChatIndex>>,
}

fn indexed_slot(key: &str) -> Option<(usize, &str)> {
    INDEXED
        .iter()
        .enumerate()
        .find_map(|(slot, prefix)| key.strip_prefix(prefix).map(|rest| (slot, rest)))
}

impl Settings {
    pub async fn connect(url: &str) -> Result<Self> {
        let size = std::env::var("DB_POOL")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(10)
            .clamp(2, 64);
        let pool = PgPoolOptions::new()
            .max_connections(size)
            .min_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .idle_timeout(std::time::Duration::from_secs(600))
            .connect(url)
            .await?;

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
        Self::migrate_counters(&mut conn).await?;
        Self::migrate_per_user(&mut conn).await?;
        Self::migrate_blank_notes(&mut conn).await?;
        Self::migrate_tallies(&mut conn).await?;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(SCHEMA_LOCK)
            .execute(&mut *conn)
            .await?;
        drop(conn);

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
            cache: RwLock::new(cache),
            index: RwLock::new(index),
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
                println!("migrated {} {prefix} rows into counters", moved.rows_affected());
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
            println!("migrated {} strike rows into counters", moved.rows_affected());
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

    async fn migrate_blank_notes(conn: &mut sqlx::PgConnection) -> Result<()> {
        const MARK: &str = "notes_migrated";

        let done = sqlx::query("SELECT 1 FROM settings WHERE chat_id = 0 AND key = $1")
            .bind(MARK)
            .fetch_optional(&mut *conn)
            .await?;
        if done.is_some() {
            return Ok(());
        }

        let dropped = sqlx::query("DELETE FROM settings WHERE key LIKE 'note:%' AND value = ''")
            .execute(&mut *conn)
            .await?;
        if dropped.rows_affected() > 0 {
            println!("dropped {} cleared note rows", dropped.rows_affected());
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

    pub async fn idle(
        &self,
        chat: i64,
        day: u64,
        days: u64,
        limit: i64,
    ) -> Idle {
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
        const FOLD: &str = "INSERT INTO counters
                 (chat_id, user_id, name, total, today, day, week, week_at, month, month_at, seen)
             SELECT chat, member, who, added, added, $5, added, $6, added, $7, $5
             FROM UNNEST($1::bigint[], $2::bigint[], $3::text[], $4::bigint[])
                  AS batch(chat, member, who, added)
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
        for (chats, users, names, added) in batches {
            let permit = std::sync::Arc::clone(&permits)
                .acquire_owned()
                .await
                .expect("the bump semaphore is never closed");
            let pool = self.pool.clone();
            tasks.spawn(async move {
                let _permit = permit;
                let rows: Result<Vec<BumpedRow>> = sqlx::query_as(FOLD)
                    .bind(&chats)
                    .bind(&users)
                    .bind(&names)
                    .bind(&added)
                    .bind(day as i64)
                    .bind(week as i64)
                    .bind(month as i64)
                    .fetch_all(&pool)
                    .await;
                rows.unwrap_or_else(|e| {
                    eprintln!("counters: bump of {} rows failed: {e}", chats.len());
                    Vec::new()
                })
            });
        }

        let mut folded = Vec::new();
        while let Some(done) = tasks.join_next().await {
            match done {
                Ok(rows) => folded.extend(rows.into_iter().map(
                    |(chat, user, name, total, awarded)| Bumped {
                        chat,
                        user,
                        name,
                        total: total.max(0) as u64,
                        awarded: awarded.max(0) as u64,
                    },
                )),
                Err(e) => eprintln!("counters: a bump chunk failed to run: {e}"),
            }
        }
        folded
    }

    pub async fn credit_add(&self, chat: i64, user: i64, name: &str, added: u64) -> u64 {
        let row: std::result::Result<Option<(i64,)>, _> = sqlx::query_as(
            "INSERT INTO counters (chat_id, user_id, name, adds) VALUES ($1, $2, $3, $4)
             ON CONFLICT (chat_id, user_id) DO UPDATE SET
                 adds = counters.adds + EXCLUDED.adds,
                 name = COALESCE(NULLIF(EXCLUDED.name, ''), counters.name)
             RETURNING adds",
        )
        .bind(chat)
        .bind(user)
        .bind(name)
        .bind(added as i64)
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
        sqlx::query_as::<_, (i64,)>("SELECT warns FROM counters WHERE chat_id = $1 AND user_id = $2")
            .bind(chat)
            .bind(user)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map_or(0, |(warns,)| warns.clamp(0, i64::from(u32::MAX)) as u32)
    }

    pub async fn set_warns(&self, chat: i64, user: i64, warns: u32) {
        if let Err(e) = sqlx::query(
            "INSERT INTO counters (chat_id, user_id, warns) VALUES ($1, $2, $3)
             ON CONFLICT (chat_id, user_id) DO UPDATE SET warns = EXCLUDED.warns",
        )
        .bind(chat)
        .bind(user)
        .bind(i64::from(warns))
        .execute(&self.pool)
        .await
        {
            eprintln!("counters: warns for {chat}/{user} failed: {e}");
        }
    }

    pub async fn add_strike(&self, chat: i64, user: i64, day: u64, days: u64) -> u32 {
        let row: std::result::Result<Option<(i64,)>, _> = sqlx::query_as(
            "INSERT INTO counters (chat_id, user_id, strikes, struck) VALUES ($1, $2, 1, $3)
             ON CONFLICT (chat_id, user_id) DO UPDATE SET
                 strikes = CASE WHEN $3 - counters.struck < $4 THEN counters.strikes ELSE 0 END + 1,
                 struck  = $3
             RETURNING strikes",
        )
        .bind(chat)
        .bind(user)
        .bind(day as i64)
        .bind(days as i64)
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
                eprintln!("pending deletes: batch write failed ({} rows): {e}", batch.len());
            }
        }
    }

    pub async fn drop_pending(&self, chat: i64, ids: &[i32]) {
        let result = sqlx::query("DELETE FROM pending_deletes WHERE chat_id = $1 AND message_id = ANY($2)")
            .bind(chat)
            .bind(ids)
            .execute(&self.pool)
            .await;
        if let Err(e) = result {
            eprintln!("pending deletes: {chat}: clear failed: {e}");
        }
    }

    pub async fn load_pending(&self, now: i64) -> Vec<(i64, i32, i64)> {
        const STALE_PENDING: i64 = 2 * 86_400;
        const MOST: i64 = 200_000;

        let floor = now - STALE_PENDING;
        match sqlx::query("DELETE FROM pending_deletes WHERE due_at < $1")
            .bind(floor)
            .execute(&self.pool)
            .await
        {
            Ok(done) if done.rows_affected() > 0 => {
                println!("pending deletes: dropped {} long overdue", done.rows_affected());
            }
            Ok(_) => {}
            Err(e) => eprintln!("pending deletes: could not drop the overdue: {e}"),
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
        match sqlx::query("DELETE FROM counters WHERE chat_id = ANY($1) AND seen > 0 AND seen <= $2")
            .bind(chats)
            .bind(before)
            .execute(&self.pool)
            .await
        {
            Ok(done) => done.rows_affected(),
            Err(e) => {
                eprintln!("counters: forget failed: {e}");
                0
            }
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
        {
            let mut cache = self.cache.write().unwrap();
            let map = cache.entry(chat).or_default();
            let changed = if on {
                let already = map.get(lock).is_some_and(|value| value.is_empty());
                if !already {
                    map.insert(lock.to_owned(), String::new());
                }
                !already
            } else {
                map.remove(lock).is_some()
            };
            if !changed {
                return false;
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
        }
        true
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
        for batch in rows.chunks(CHUNK) {
            let (mut chats, mut counters, mut added) = (Vec::new(), Vec::new(), Vec::new());
            for (chat, counter, count) in batch {
                chats.push(*chat);
                counters.push(*counter);
                added.push(*count as i64);
            }
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
            .execute(&self.pool)
            .await;
            if let Err(e) = result {
                eprintln!("tallies: batch of {} rows failed: {e}", batch.len());
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

    pub fn chats(&self) -> Vec<i64> {
        self.cache.read().unwrap().keys().copied().collect()
    }

    pub fn values_with_prefix(&self, chat: i64, prefix: &str) -> Vec<(String, String)> {
        let cache = self.cache.read().unwrap();
        let Some(map) = cache.get(&chat) else {
            return Vec::new();
        };
        map.iter()
            .filter(|(_, value)| !value.is_empty())
            .filter_map(|(key, value)| {
                Some((key.strip_prefix(prefix)?.to_owned(), value.clone()))
            })
            .collect()
    }

    pub fn value(&self, chat: i64, key: &str) -> Option<String> {
        self.with_chat(chat, |settings| settings.value(key).map(str::to_owned))
    }

    pub fn value_parsed<T: std::str::FromStr>(&self, chat: i64, key: &str) -> Option<T> {
        self.cache.read().unwrap().get(&chat)?.get(key)?.parse().ok()
    }

    pub async fn set_value(&self, chat: i64, key: &str, value: &str) {
        {
            let mut cache = self.cache.write().unwrap();
            let map = cache.entry(chat).or_default();

            if map.get(key).is_some_and(|stored| stored == value) {
                return;
            }
            map.insert(key.to_owned(), value.to_owned());
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
        }
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
                    Some((key, value)) => self.set_value(chat, key, value).await,
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
        sqlx::query(wipe).bind(&both).execute(&settings.pool).await.unwrap();

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

        sqlx::query(wipe).bind(&both).execute(&settings.pool).await.unwrap();
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
        sqlx::query(wipe).bind(&all).execute(&settings.pool).await.unwrap();

        settings.set(mine_by_flag, &format!("admin:{me}"), true).await;
        settings.set_value(mine_by_owner, "owner", &me.to_string()).await;
        settings.set_value(theirs, "owner", &them.to_string()).await;
        settings.set(theirs, &format!("admin:{them}"), true).await;

        let mut found = settings.panels_for(me, 21).await;
        found.retain(|chat| all.contains(chat));
        found.sort_unstable();
        let mut mine = vec![mine_by_flag, mine_by_owner];
        mine.sort_unstable();
        assert_eq!(found, mine, "someone else's group must not be listed");

        settings.set(mine_by_flag, &format!("admin:{me}"), false).await;
        let mut left = settings.panels_for(me, 21).await;
        left.retain(|chat| all.contains(chat));
        assert_eq!(left, vec![mine_by_owner]);

        sqlx::query(wipe).bind(&all).execute(&settings.pool).await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn every_chunk_of_a_large_flush_lands() {
        const ROWS: i64 = 12_001;

        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_987;

        let settings = Settings::connect(&url).await.unwrap();
        let wipe = "DELETE FROM counters WHERE chat_id = $1";
        sqlx::query(wipe).bind(chat).execute(&settings.pool).await.unwrap();

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
        assert_eq!(settings.board_totals(chat, Period::Total, 0).await.0 as i64, ROWS * 3);
        assert_eq!(settings.name_of(chat, 1).await.as_deref(), Some("member 1"));

        sqlx::query(wipe).bind(chat).execute(&settings.pool).await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn forget_idle_only_touches_the_chats_it_is_given() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let (a, b) = (-999_999_999_994, -999_999_999_993);

        let settings = Settings::connect(&url).await.unwrap();
        let wipe = "DELETE FROM counters WHERE chat_id = ANY($1)";
        let both = vec![a, b];
        sqlx::query(wipe).bind(&both).execute(&settings.pool).await.unwrap();

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
        assert_eq!(left(b).await, 1, "a chat that was not named must be untouched");

        assert_eq!(settings.forget_idle(&[b], 150).await, 1);
        assert_eq!(left(b).await, 0);

        sqlx::query(wipe).bind(&both).execute(&settings.pool).await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn tallies_roll_over_in_the_write() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_995;
        let (day, next) = (2_000, 2_001);

        let settings = Settings::connect(&url).await.unwrap();
        let wipe = "DELETE FROM tallies WHERE chat_id = $1";
        sqlx::query(wipe).bind(chat).execute(&settings.pool).await.unwrap();

        settings.add_tallies(&[(chat, "k_text", 3), (chat, "h9", 3)], day).await;
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

        sqlx::query(wipe).bind(chat).execute(&settings.pool).await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn clearing_a_valued_setting_removes_the_row() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_996;
        let note = "note:7";

        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        settings.set_value(chat, note, "برای بعد").await;
        assert_eq!(settings.value(chat, note).as_deref(), Some("برای بعد"));

        assert!(settings.set(chat, note, false).await);
        assert!(settings.value(chat, note).is_none());
        assert!(!settings.is_locked(chat, note));

        let left: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM settings WHERE chat_id = $1 AND key LIKE 'note:%'",
        )
        .bind(chat)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(left.0, 0);

        let reloaded = Settings::connect(&url).await.unwrap();
        assert!(reloaded.value(chat, note).is_none());
        assert!(!reloaded.is_locked(chat, note));

        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn counters_roll_over() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_998;
        let (day, week, month) = (1_000, 1_000 / 7, 1_000 / 30);

        let settings = Settings::connect(&url).await.unwrap();
        let wipe = "DELETE FROM counters WHERE chat_id = $1";
        sqlx::query(wipe).bind(chat).execute(&settings.pool).await.unwrap();

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
        assert_eq!(settings.board_totals(chat, Period::Today, day).await, (18, 2));
        assert_eq!(settings.board_totals(chat, Period::Total, 0).await, (18, 2));

        settings.bump(ali(2), day + 1, week, month).await;

        assert_eq!(settings.board_totals(chat, Period::Today, day + 1).await, (2, 1));
        let today = settings.board(chat, Period::Today, day + 1, 10).await;
        assert_eq!(today.len(), 1);
        assert_eq!((today[0].user, today[0].count), (1, 2));
        assert_eq!(settings.board_totals(chat, Period::Week, week).await, (20, 2));
        assert_eq!(settings.board_totals(chat, Period::Total, 0).await, (20, 2));

        settings.bump(ali(4), day + 8, week + 1, month).await;
        assert_eq!(settings.board_totals(chat, Period::Week, week + 1).await, (4, 1));
        assert_eq!(settings.board_totals(chat, Period::Month, month).await, (24, 2));

        let card = settings.card(chat, 1, day + 8).await;
        assert_eq!((card.total, card.today, card.place), (14, 4, Some(1)));
        assert_eq!(settings.card(chat, 2, day + 8).await.place, None);
        assert_eq!(settings.name_of(chat, 2).await.as_deref(), Some("Sara"));

        let (idle, total) = settings.idle(chat, day + 8, 5, 10).await;
        assert_eq!(total, 1);
        assert_eq!(idle.first().map(|(user, _, quiet)| (*user, *quiet)), Some((2, 8)));

        assert_eq!(settings.credit_add(chat, 1, "Ali", 3).await, 3);
        assert_eq!(settings.credit_add(chat, 1, "Ali", 2).await, 5);
        assert_eq!(settings.adds_of(chat, 1).await, 5);

        settings.clear_seen(chat, 2).await;
        assert_eq!(settings.idle(chat, day + 8, 5, 10).await.1, 0);

        sqlx::query(wipe).bind(chat).execute(&settings.pool).await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn strikes_expire_in_the_write() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_997;
        let (user, days) = (7, 7);

        let settings = Settings::connect(&url).await.unwrap();
        let wipe = "DELETE FROM counters WHERE chat_id = $1";
        sqlx::query(wipe).bind(chat).execute(&settings.pool).await.unwrap();

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

        sqlx::query(wipe).bind(chat).execute(&settings.pool).await.unwrap();
    }
}
