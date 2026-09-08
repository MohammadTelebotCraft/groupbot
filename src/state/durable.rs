use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SPOOL_IO_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const MAX_BATCH_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SPOOL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SPOOL_FILES: usize = 4096;
const AWARD_LEASE_SECONDS: i64 = 120;
const MAX_RANK_AWARD_ATTEMPTS: i64 = 20;
const MIN_RANK_RETRY_SECONDS: u64 = 30;
const MAX_RANK_RETRY_SECONDS: u64 = 6 * 60 * 60;
type BoxError = Box<dyn std::error::Error + Send + Sync>;
type DurableResult<T> = std::result::Result<T, BoxError>;
type PendingRankAwardRow = (i64, i64, String, i64, i64, i64, i64, i64);

#[derive(Debug)]
pub(crate) enum StatsPersistenceError {
    Permanent(String),
    Retryable(BoxError),
}

impl StatsPersistenceError {
    fn permanent(error: impl std::fmt::Display) -> Self {
        Self::Permanent(error.to_string())
    }

    pub(crate) fn is_permanent(&self) -> bool {
        matches!(self, Self::Permanent(_))
    }
}

impl std::fmt::Display for StatsPersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Permanent(error) => formatter.write_str(error),
            Self::Retryable(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for StatsPersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Permanent(_) => None,
            Self::Retryable(error) => Some(error.as_ref()),
        }
    }
}

impl From<sqlx::Error> for StatsPersistenceError {
    fn from(error: sqlx::Error) -> Self {
        let permanent = match &error {
            sqlx::Error::Protocol(_) => true,
            sqlx::Error::Database(database) => matches!(
                database.code().as_deref(),
                Some("22001" | "22003" | "22P02" | "23502" | "23503" | "23514")
            ),
            _ => false,
        };
        if permanent {
            Self::permanent(error)
        } else {
            Self::Retryable(Box::new(error))
        }
    }
}

#[derive(Debug)]
struct PermanentSpoolError(String);

impl std::fmt::Display for PermanentSpoolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PermanentSpoolError {}

fn permanent_spool_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::other(PermanentSpoolError(message.into()))
}

fn is_permanent_spool_error(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.is::<PermanentSpoolError>())
}

fn classify_spool_error(error: std::io::Error) -> StatsPersistenceError {
    if is_permanent_spool_error(&error) {
        StatsPersistenceError::permanent(error)
    } else {
        StatsPersistenceError::Retryable(Box::new(error))
    }
}

fn postgres_i64(value: u64, field: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        sqlx::Error::Protocol(format!(
            "statistics {field} value {value} exceeds PostgreSQL bigint"
        ))
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct StatsBatch {
    pub id: String,
    pub day: u64,
    pub tallies: Vec<(i64, String, u64)>,
    pub counts: Vec<Bump>,
}

impl StatsBatch {
    fn validate_id(&self) -> DurableResult<()> {
        if self.id.is_empty()
            || self.id.len() > 128
            || !self.id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        {
            return Err("invalid statistics batch id".into());
        }
        Ok(())
    }

    fn encoded(&self) -> DurableResult<Vec<u8>> {
        self.validate_id()?;
        let bytes = serde_json::to_vec(self)?;
        if u64::try_from(bytes.len())? > MAX_BATCH_BYTES {
            return Err("statistics batch exceeds disk-spool limit".into());
        }
        let mut tallies = HashSet::with_capacity(self.tallies.len());
        if self.tallies.iter().any(|(chat, name, _)| {
            name.is_empty()
                || name.len() > MAX_TALLY_COUNTER_BYTES
                || name.contains(':')
                || !tallies.insert((*chat, name.as_str()))
        }) {
            return Err("statistics batch contains an invalid or duplicate tally key".into());
        }
        let mut counts = HashSet::with_capacity(self.counts.len());
        if self
            .counts
            .iter()
            .any(|bump| !counts.insert((bump.chat, bump.user)))
        {
            return Err("statistics batch contains a duplicate member key".into());
        }
        Ok(bytes)
    }

    fn digest(&self) -> DurableResult<Vec<u8>> {
        Ok(Sha256::digest(self.encoded()?).to_vec())
    }

    pub fn new(day: u64, tallies: Vec<(i64, String, u64)>, counts: Vec<Bump>) -> Self {
        Self {
            id: new_opaque_id("b"),
            day,
            tallies,
            counts,
        }
    }
}

pub struct PendingRankAward {
    pub bumped: Bumped,
    pub milestone: u64,
    attempts: i64,
    version: i64,
    lease_token: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RankRetryDisposition {
    Deferred { delay_seconds: u64 },
    Terminal,
    Superseded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicationStep {
    TemporarySynced,
    FinalLinked,
    DirectorySyncedBeforeUnlink,
    TemporaryUnlinked,
    DirectorySyncedAfterUnlink,
}

impl Settings {
    pub(super) async fn init_durable(connection: &mut sqlx::PgConnection) -> Result<()> {
        sqlx::query("CREATE TABLE IF NOT EXISTS applied_stats_batches (id text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now())")
            .execute(&mut *connection).await?;
        sqlx::query("ALTER TABLE applied_stats_batches ADD COLUMN IF NOT EXISTS digest bytea")
            .execute(&mut *connection)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS applied_stats_batches_age ON applied_stats_batches(applied_at)")
            .execute(&mut *connection).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_rank_awards (
                chat_id bigint NOT NULL,
                user_id bigint NOT NULL,
                name text NOT NULL,
                total bigint NOT NULL,
                awarded bigint NOT NULL,
                milestone bigint NOT NULL,
                version bigint NOT NULL DEFAULT 1,
                lease_token text,
                lease_until timestamptz NOT NULL DEFAULT '-infinity',
                attempts integer NOT NULL DEFAULT 0,
                terminal_reason text,
                terminal_at timestamptz,
                created_at timestamptz NOT NULL DEFAULT now(),
                PRIMARY KEY(chat_id, user_id)
            )",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "ALTER TABLE pending_rank_awards
                ADD COLUMN IF NOT EXISTS milestone bigint,
                ADD COLUMN IF NOT EXISTS terminal_reason text,
                ADD COLUMN IF NOT EXISTS terminal_at timestamptz",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query("UPDATE pending_rank_awards SET milestone=0 WHERE milestone IS NULL")
            .execute(&mut *connection)
            .await?;
        sqlx::query(
            "UPDATE pending_rank_awards SET lease_until='-infinity' WHERE lease_until IS NULL",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "ALTER TABLE pending_rank_awards
                ALTER COLUMN milestone SET NOT NULL,
                ALTER COLUMN lease_until SET DEFAULT '-infinity',
                ALTER COLUMN lease_until SET NOT NULL",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS pending_rank_awards_claim_v2
             ON pending_rank_awards(created_at, attempts, chat_id, user_id)
             WHERE terminal_reason IS NULL",
        )
        .execute(connection)
        .await?;
        Ok(())
    }

    pub(crate) async fn stage_stats(
        &self,
        batch: &StatsBatch,
    ) -> std::result::Result<(), StatsPersistenceError> {
        let bytes = batch.encoded().map_err(StatsPersistenceError::permanent)?;
        let Some(directory) = &self.stats_directory else {
            return Ok(());
        };
        let path = directory.join(format!("{}.json", batch.id));
        let directory = directory.clone();
        let staged = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            stage_spool_file(&directory, &path, &bytes)
        })
        .await
        .map_err(|error| {
            if error.is_panic() {
                StatsPersistenceError::permanent(error)
            } else {
                StatsPersistenceError::Retryable(Box::new(error))
            }
        })?;
        staged.map_err(classify_spool_error)?;
        Ok(())
    }

    pub async fn finish_stats(&self, batch: &StatsBatch) -> DurableResult<()> {
        batch.validate_id()?;
        if let Some(directory) = &self.stats_directory {
            let directory = directory.clone();
            let path = directory.join(format!("{}.json", batch.id));
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                match std::fs::remove_file(path) {
                    Ok(()) => sync_directory(&directory)?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                Ok(())
            })
            .await??;
        }
        Ok(())
    }

    pub async fn recover_stats<P>(&self, should_enqueue_rank: P) -> DurableResult<()>
    where
        P: Fn(i64, u64, u64, u64) -> Option<u64>,
    {
        let Some(directory) = &self.stats_directory else {
            return Ok(());
        };
        tokio::fs::create_dir_all(directory).await?;
        let inspected_directory = directory.clone();
        let inventory = tokio::task::spawn_blocking(move || {
            preflight_hard_links(&inspected_directory)?;
            inspect_spool(&inspected_directory, MAX_SPOOL_BYTES, MAX_SPOOL_FILES)
        })
        .await??;
        let mut paths = HashSet::new();
        paths.extend(inventory.batches);
        let mut temporary_paths = inventory.temporaries;
        temporary_paths.sort();
        for temporary in temporary_paths {
            let directory = directory.clone();
            if let Some(path) =
                tokio::task::spawn_blocking(move || recover_spool_temporary(&directory, &temporary))
                    .await??
            {
                paths.insert(path);
            }
        }
        let mut paths: Vec<_> = paths.into_iter().collect();
        paths.sort();
        for path in paths {
            if tokio::fs::metadata(&path).await?.len() > MAX_BATCH_BYTES {
                return Err("oversized statistics recovery file".into());
            }
            let batch: StatsBatch = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
            batch.encoded()?;
            if path.file_stem().and_then(|name| name.to_str()) != Some(batch.id.as_str()) {
                return Err("statistics recovery filename/id mismatch".into());
            }
            self.apply_stats(&batch, &should_enqueue_rank).await?;
            self.finish_stats(&batch).await?;
        }
        self.prune_stats_receipts().await?;
        Ok(())
    }

    pub async fn prune_stats_receipts(&self) -> DurableResult<()> {
        let Some(directory) = &self.stats_directory else {
            return Ok(());
        };
        let directory = directory.clone();
        let inventory = tokio::task::spawn_blocking(move || {
            inspect_spool(&directory, MAX_SPOOL_BYTES, MAX_SPOOL_FILES)
        })
        .await??;
        if !inventory.batches.is_empty() || !inventory.temporaries.is_empty() {
            return Ok(());
        }
        sqlx::query("DELETE FROM applied_stats_batches WHERE id IN (SELECT id FROM applied_stats_batches WHERE applied_at < now() - interval '90 days' ORDER BY applied_at LIMIT 5000)")
            .execute(&self.pool).await?;
        Ok(())
    }

    pub(crate) async fn apply_stats<P>(
        &self,
        batch: &StatsBatch,
        should_enqueue_rank: P,
    ) -> std::result::Result<bool, StatsPersistenceError>
    where
        P: Fn(i64, u64, u64, u64) -> Option<u64>,
    {
        let digest = batch.digest().map_err(StatsPersistenceError::permanent)?;
        let day = postgres_i64(batch.day, "day")?;
        let mut slots: Vec<_> = batch
            .counts
            .iter()
            .map(|b| self.write_slot_index(b.chat))
            .collect();
        slots.sort_unstable();
        slots.dedup();
        let mut guards = Vec::new();
        for slot in slots {
            guards.push(self.write_slots[slot].clone().lock_owned().await);
        }
        let mut tx = self.pool.begin().await?;
        let fresh = sqlx::query(
            "INSERT INTO applied_stats_batches(id, digest) VALUES($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(&batch.id)
        .bind(&digest)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            != 0;
        if !fresh {
            let stored: Option<Vec<u8>> =
                sqlx::query_scalar("SELECT digest FROM applied_stats_batches WHERE id=$1")
                    .bind(&batch.id)
                    .fetch_one(&mut *tx)
                    .await?;
            match stored {
                Some(stored) if stored == digest => {}
                Some(_) => {
                    return Err(StatsPersistenceError::permanent(format!(
                        "statistics batch id {} was already applied with different content",
                        batch.id
                    )));
                }
                None => {
                    return Err(StatsPersistenceError::permanent(format!(
                        "statistics batch id {} has a legacy receipt without a verifiable digest",
                        batch.id
                    )));
                }
            }
            tx.commit().await?;
            return Ok(false);
        }
        let mut tally_rows = if batch.tallies.is_empty() {
            None
        } else {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT tally_rows FROM durable_counts WHERE id = 0 FOR UPDATE",
                )
                .fetch_one(&mut *tx)
                .await?,
            )
        };
        for rows in batch.tallies.chunks(1000) {
            let chats: Vec<_> = rows.iter().map(|r| r.0).collect();
            let counters: Vec<_> = rows.iter().map(|r| &r.1).collect();
            let values: Result<Vec<_>> = rows
                .iter()
                .map(|row| postgres_i64(row.2, "tally"))
                .collect();
            let (new_rows, largest_chat): (i64, i64) = sqlx::query_as(
                "WITH input AS (
                     SELECT DISTINCT chat, name
                     FROM UNNEST($1::bigint[], $2::text[]) AS batch(chat, name)
                 ), additions AS (
                     SELECT input.chat, input.name FROM input
                     WHERE NOT EXISTS (
                         SELECT 1 FROM tallies
                         WHERE tallies.chat_id = input.chat AND tallies.counter = input.name
                     )
                 ), affected AS (
                     SELECT DISTINCT chat FROM input
                 ), projected AS (
                     SELECT affected.chat,
                            (SELECT count(*) FROM tallies WHERE chat_id = affected.chat)
                          + (SELECT count(*) FROM additions WHERE chat = affected.chat) AS rows
                     FROM affected
                 )
                 SELECT (SELECT count(*) FROM additions), COALESCE(MAX(rows), 0)
                 FROM projected",
            )
            .bind(&chats)
            .bind(&counters)
            .fetch_one(&mut *tx)
            .await?;
            if largest_chat > MAX_TALLY_ROWS_PER_CHAT {
                return Err(StatsPersistenceError::permanent(format!(
                    "statistics batch would create {largest_chat} tally rows in one chat, above {MAX_TALLY_ROWS_PER_CHAT}"
                )));
            }
            let current = tally_rows.ok_or_else(|| {
                sqlx::Error::Protocol("tally capacity metadata was not locked".to_owned())
            })?;
            let projected = current.checked_add(new_rows).ok_or_else(|| {
                sqlx::Error::Protocol("projected tally row count overflowed bigint".to_owned())
            })?;
            if let Some(max_tally_rows) = self.max_tally_rows
                && projected > max_tally_rows
            {
                return Err(StatsPersistenceError::permanent(format!(
                    "statistics batch would create {projected} tally rows, above shard limit {max_tally_rows}"
                )));
            }
            sqlx::query("INSERT INTO tallies(chat_id,counter,day,count)
                SELECT chat,name,$4,added FROM UNNEST($1::bigint[],$2::text[],$3::bigint[]) AS b(chat,name,added)
                ON CONFLICT(chat_id,counter) DO UPDATE SET
                    count=CASE WHEN tallies.day=EXCLUDED.day THEN tallies.count ELSE 0 END+EXCLUDED.count,
                    day=EXCLUDED.day")
                .bind(chats).bind(counters).bind(values?).bind(day).execute(&mut *tx).await?;
            tally_rows = Some(projected);
        }
        if let Some(tally_rows) = tally_rows {
            sqlx::query("UPDATE durable_counts SET tally_rows = $1 WHERE id = 0")
                .bind(tally_rows)
                .execute(&mut *tx)
                .await?;
        }
        for rows in batch.counts.chunks(5000) {
            let added_by_member: std::collections::HashMap<_, _> = rows
                .iter()
                .map(|row| ((row.chat, row.user), row.added))
                .collect();
            let chats: Vec<_> = rows.iter().map(|r| r.chat).collect();
            let users: Vec<_> = rows.iter().map(|r| r.user).collect();
            let names: Vec<_> = rows.iter().map(|r| &r.name).collect();
            let added: Result<Vec<_>> = rows
                .iter()
                .map(|row| postgres_i64(row.added, "message count"))
                .collect();
            let changed: Vec<BumpedRow> = sqlx::query_as(include_str!("counter_fold.sql"))
                .bind(chats)
                .bind(users)
                .bind(names)
                .bind(added?)
                .bind(day)
                .bind(postgres_i64(batch.day / 7, "week")?)
                .bind(postgres_i64(batch.day / 30, "month")?)
                .bind(MAX_COUNTER_ROWS_PER_CHAT)
                .bind(self.max_counter_rows.unwrap_or(i64::MAX / 2))
                .fetch_all(&mut *tx)
                .await?;
            let mut chats = Vec::new();
            let mut users = Vec::new();
            let mut names = Vec::new();
            let mut totals = Vec::new();
            let mut awarded = Vec::new();
            let mut milestones = Vec::new();
            for row in &changed {
                let total = nonnegative_counter("total", row.3)?;
                let already_awarded = nonnegative_counter("awarded", row.4)?;
                let added = added_by_member
                    .get(&(row.0, row.1))
                    .copied()
                    .ok_or_else(|| {
                        sqlx::Error::Protocol(
                            "counter fold returned a member outside its input batch".into(),
                        )
                    })?;
                let previous_total = total.checked_sub(added).ok_or_else(|| {
                    sqlx::Error::Protocol(format!(
                        "counter fold returned total {total} below its accepted delta {added}"
                    ))
                })?;
                if let Some(milestone) =
                    should_enqueue_rank(row.0, previous_total, total, already_awarded)
                {
                    chats.push(row.0);
                    users.push(row.1);
                    names.push(&row.2);
                    totals.push(row.3);
                    awarded.push(row.4);
                    milestones.push(postgres_i64(milestone, "rank milestone")?);
                }
            }
            if !chats.is_empty() {
                sqlx::query(
                    "INSERT INTO pending_rank_awards
                        (chat_id, user_id, name, total, awarded, milestone)
                     SELECT chat, member, who, new_total, old_awarded, new_milestone
                     FROM UNNEST($1::bigint[], $2::bigint[], $3::text[], $4::bigint[],
                                 $5::bigint[], $6::bigint[])
                          AS pending(chat, member, who, new_total, old_awarded, new_milestone)
                     ON CONFLICT(chat_id, user_id) DO UPDATE SET
                        name=EXCLUDED.name,
                        total=GREATEST(pending_rank_awards.total, EXCLUDED.total),
                        awarded=GREATEST(pending_rank_awards.awarded, EXCLUDED.awarded),
                        version=pending_rank_awards.version+1,
                        lease_token=CASE WHEN EXCLUDED.milestone > pending_rank_awards.milestone
                                         THEN NULL ELSE pending_rank_awards.lease_token END,
                        lease_until=CASE WHEN EXCLUDED.milestone > pending_rank_awards.milestone
                                         THEN '-infinity' ELSE pending_rank_awards.lease_until END,
                        attempts=CASE WHEN EXCLUDED.milestone > pending_rank_awards.milestone
                                      THEN 0 ELSE pending_rank_awards.attempts END,
                        terminal_reason=CASE WHEN EXCLUDED.milestone > pending_rank_awards.milestone
                                             THEN NULL ELSE pending_rank_awards.terminal_reason END,
                        terminal_at=CASE WHEN EXCLUDED.milestone > pending_rank_awards.milestone
                                         THEN NULL ELSE pending_rank_awards.terminal_at END,
                        milestone=GREATEST(pending_rank_awards.milestone, EXCLUDED.milestone)",
                )
                .bind(chats)
                .bind(users)
                .bind(names)
                .bind(totals)
                .bind(awarded)
                .bind(milestones)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn claim_rank_awards(&self, limit: usize) -> DurableResult<Vec<PendingRankAward>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit)?;
        let lease_token = new_opaque_id("l");
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "WITH exhausted AS (
                SELECT chat_id, user_id
                FROM pending_rank_awards
                WHERE terminal_reason IS NULL AND attempts >= $1 AND lease_until <= now()
                ORDER BY created_at, attempts, chat_id, user_id
                LIMIT $2
                FOR UPDATE SKIP LOCKED
             )
             UPDATE pending_rank_awards AS pending SET
                terminal_reason='retry_limit_after_crash', terminal_at=now(),
                lease_token=NULL, lease_until='infinity'
             FROM exhausted
             WHERE pending.chat_id=exhausted.chat_id AND pending.user_id=exhausted.user_id",
        )
        .bind(MAX_RANK_AWARD_ATTEMPTS)
        .bind(limit)
        .execute(&mut *tx)
        .await?;
        let rows: Vec<PendingRankAwardRow> = sqlx::query_as(
            "WITH selected AS (
                SELECT chat_id, user_id
                FROM pending_rank_awards
                WHERE terminal_reason IS NULL AND attempts < $4 AND lease_until <= now()
                ORDER BY created_at, attempts, chat_id, user_id
                LIMIT $1
                FOR UPDATE SKIP LOCKED
             )
             UPDATE pending_rank_awards AS pending SET
                lease_token=$2,
                lease_until=now()+($3::bigint * interval '1 second'),
                attempts=pending.attempts+1,
                version=pending.version+1
             FROM selected
             WHERE pending.chat_id=selected.chat_id AND pending.user_id=selected.user_id
             RETURNING pending.chat_id, pending.user_id, pending.name, pending.total,
                       pending.awarded, pending.milestone, pending.attempts::bigint,
                       pending.version",
        )
        .bind(limit)
        .bind(&lease_token)
        .bind(AWARD_LEASE_SECONDS)
        .bind(MAX_RANK_AWARD_ATTEMPTS)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.into_iter()
            .map(
                |(chat, user, name, total, awarded, milestone, attempts, version)| {
                    Ok(PendingRankAward {
                        bumped: Bumped {
                            chat,
                            user,
                            name,
                            total: nonnegative_counter("pending rank total", total)?,
                            awarded: nonnegative_counter("pending rank awarded", awarded)?,
                        },
                        milestone: nonnegative_counter("pending rank milestone", milestone)?,
                        attempts,
                        version,
                        lease_token: lease_token.clone(),
                    })
                },
            )
            .collect::<Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub async fn defer_rank_award(
        &self,
        delivery: &PendingRankAward,
        reason: &'static str,
        requested_delay_seconds: Option<u64>,
    ) -> DurableResult<RankRetryDisposition> {
        if reason.len() > 128 {
            return Err("rank retry reason exceeds storage limit".into());
        }
        if delivery.attempts >= MAX_RANK_AWARD_ATTEMPTS {
            let changed = sqlx::query(
                "UPDATE pending_rank_awards SET
                    terminal_reason=$5, terminal_at=now(), lease_token=NULL,
                    lease_until='infinity'
                 WHERE chat_id=$1 AND user_id=$2 AND version=$3 AND lease_token=$4",
            )
            .bind(delivery.bumped.chat)
            .bind(delivery.bumped.user)
            .bind(delivery.version)
            .bind(&delivery.lease_token)
            .bind(format!("retry_limit:{reason}"))
            .execute(&self.pool)
            .await?
            .rows_affected();
            return Ok(if changed == 0 {
                RankRetryDisposition::Superseded
            } else {
                RankRetryDisposition::Terminal
            });
        }
        let delay_seconds = rank_retry_delay(delivery.attempts, requested_delay_seconds);
        let changed = sqlx::query(
            "UPDATE pending_rank_awards SET
                lease_token=NULL, lease_until=now()+($5::bigint * interval '1 second')
             WHERE chat_id=$1 AND user_id=$2 AND version=$3 AND lease_token=$4
               AND terminal_reason IS NULL",
        )
        .bind(delivery.bumped.chat)
        .bind(delivery.bumped.user)
        .bind(delivery.version)
        .bind(&delivery.lease_token)
        .bind(i64::try_from(delay_seconds)?)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(if changed == 0 {
            RankRetryDisposition::Superseded
        } else {
            RankRetryDisposition::Deferred { delay_seconds }
        })
    }

    pub async fn ack_rank_award(
        &self,
        delivery: &PendingRankAward,
        milestone: Option<u64>,
    ) -> DurableResult<bool> {
        let mut tx = self.pool.begin().await?;
        let removed = sqlx::query(
            "DELETE FROM pending_rank_awards
             WHERE chat_id=$1 AND user_id=$2 AND version=$3 AND lease_token=$4",
        )
        .bind(delivery.bumped.chat)
        .bind(delivery.bumped.user)
        .bind(delivery.version)
        .bind(&delivery.lease_token)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            != 0;
        if !removed {
            tx.rollback().await?;
            return Ok(false);
        }
        if let Some(milestone) = milestone {
            sqlx::query(
                "UPDATE counters SET awarded=GREATEST(awarded, $3)
                 WHERE chat_id=$1 AND user_id=$2",
            )
            .bind(delivery.bumped.chat)
            .bind(delivery.bumped.user)
            .bind(postgres_i64(milestone, "rank milestone")?)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}

fn new_opaque_id(prefix: &str) -> String {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    let random = |domain: u64| RandomState::new().hash_one((domain, serial, std::process::id()));
    format!(
        "{prefix}-{serial:016x}-{:016x}{:016x}{:016x}{:016x}",
        random(0),
        random(1),
        random(2),
        random(3),
    )
}

fn rank_retry_delay(attempts: i64, requested: Option<u64>) -> u64 {
    let exponent = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(20);
    let exponential = MIN_RANK_RETRY_SECONDS
        .checked_shl(exponent)
        .unwrap_or(MAX_RANK_RETRY_SECONDS)
        .min(MAX_RANK_RETRY_SECONDS);
    exponential
        .max(requested.unwrap_or(0))
        .min(MAX_RANK_RETRY_SECONDS)
}

fn stage_spool_file(directory: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let _guard = SPOOL_IO_LOCK
        .lock()
        .map_err(|_| permanent_spool_error("statistics spool lock was poisoned"))?;
    std::fs::create_dir_all(directory)?;
    let temporary = path.with_extension("tmp");
    match std::fs::symlink_metadata(&temporary) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(permanent_spool_error(
                    "statistics temporary path is not a regular file",
                ));
            }
            let existing = std::fs::read(&temporary)?;
            if existing == bytes {
                let additional = if path.exists() {
                    (0, 0)
                } else {
                    (
                        u64::try_from(bytes.len()).map_err(|_| {
                            permanent_spool_error("statistics batch size does not fit u64")
                        })?,
                        1,
                    )
                };
                ensure_spool_capacity(
                    directory,
                    additional.0,
                    additional.1,
                    MAX_SPOOL_BYTES,
                    MAX_SPOOL_FILES,
                )?;
                return publish_temporary(directory, &temporary, path, bytes, |_| {});
            }
            if bytes.starts_with(&existing) {
                std::fs::remove_file(&temporary)?;
                sync_directory(directory)?;
            } else {
                return Err(permanent_spool_error(
                    "statistics temporary id already exists with different content",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            inspect_spool(directory, MAX_SPOOL_BYTES, MAX_SPOOL_FILES)?;
            verify_existing_batch(path, bytes)?;
            return sync_directory(directory);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let byte_count = u64::try_from(bytes.len())
        .map_err(|_| permanent_spool_error("statistics batch size does not fit u64"))?;
    let reserved_bytes = byte_count
        .checked_mul(2)
        .ok_or_else(|| permanent_spool_error("statistics spool size overflow"))?;
    ensure_spool_capacity(
        directory,
        reserved_bytes,
        2,
        MAX_SPOOL_BYTES,
        MAX_SPOOL_FILES,
    )?;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    if let Err(error) = file.write_all(bytes) {
        if let Err(cleanup_error) = std::fs::remove_file(&temporary) {
            log::warn!(
                "statistics: could not remove failed temporary {}: {cleanup_error}",
                temporary.display()
            );
        }
        let _ = sync_directory(directory);
        return Err(error);
    }
    drop(file);
    publish_temporary(directory, &temporary, path, bytes, |_| {})
}

fn recover_spool_temporary(directory: &Path, temporary: &Path) -> std::io::Result<Option<PathBuf>> {
    let metadata = std::fs::symlink_metadata(temporary)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_BATCH_BYTES
    {
        log::warn!(
            "statistics: removing invalid crash temporary {}",
            temporary.display()
        );
        std::fs::remove_file(temporary)?;
        return Ok(None);
    }
    let bytes = std::fs::read(temporary)?;
    let batch: StatsBatch = match serde_json::from_slice(&bytes) {
        Ok(batch) => batch,
        Err(error) => {
            log::warn!(
                "statistics: removing incomplete crash temporary {}: {error}",
                temporary.display()
            );
            std::fs::remove_file(temporary)?;
            return Ok(None);
        }
    };
    let canonical = match batch.encoded() {
        Ok(canonical) if canonical == bytes => canonical,
        Ok(_) => {
            log::warn!(
                "statistics: removing non-canonical crash temporary {}",
                temporary.display()
            );
            std::fs::remove_file(temporary)?;
            return Ok(None);
        }
        Err(error) => {
            log::warn!(
                "statistics: removing invalid crash temporary {}: {error}",
                temporary.display()
            );
            std::fs::remove_file(temporary)?;
            return Ok(None);
        }
    };
    if temporary.file_stem().and_then(|name| name.to_str()) != Some(batch.id.as_str()) {
        return Err(permanent_spool_error(
            "non-canonical complete statistics temporary cannot be safely replayed",
        ));
    }
    let path = directory.join(format!("{}.json", batch.id));
    if !path.exists() {
        ensure_spool_capacity(
            directory,
            u64::try_from(canonical.len())
                .map_err(|_| permanent_spool_error("statistics batch size does not fit u64"))?,
            1,
            MAX_SPOOL_BYTES,
            MAX_SPOOL_FILES,
        )?;
    }
    publish_temporary(directory, temporary, &path, &canonical, |_| {})?;
    Ok(Some(path))
}

fn publish_temporary(
    directory: &Path,
    temporary: &Path,
    final_path: &Path,
    expected: &[u8],
    mut observe: impl FnMut(PublicationStep),
) -> std::io::Result<()> {
    std::fs::File::open(temporary)?.sync_all()?;
    observe(PublicationStep::TemporarySynced);
    match std::fs::hard_link(temporary, final_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_existing_batch(final_path, expected)?;
        }
        Err(error) => return Err(error),
    }
    observe(PublicationStep::FinalLinked);
    sync_directory(directory)?;
    observe(PublicationStep::DirectorySyncedBeforeUnlink);
    std::fs::remove_file(temporary)?;
    observe(PublicationStep::TemporaryUnlinked);
    sync_directory(directory)?;
    observe(PublicationStep::DirectorySyncedAfterUnlink);
    Ok(())
}

struct SpoolInventory {
    batches: Vec<PathBuf>,
    temporaries: Vec<PathBuf>,
    bytes: u64,
    entries: usize,
}

fn inspect_spool(
    directory: &Path,
    max_bytes: u64,
    max_files: usize,
) -> std::io::Result<SpoolInventory> {
    let mut inventory = SpoolInventory {
        batches: Vec::new(),
        temporaries: Vec::new(),
        bytes: 0,
        entries: 0,
    };
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        inventory.entries = inventory
            .entries
            .checked_add(1)
            .ok_or_else(|| permanent_spool_error("statistics spool entry count overflow"))?;
        if inventory.entries > max_files {
            return Err(permanent_spool_error(
                "statistics spool has too many entries; refusing unbounded recovery",
            ));
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(permanent_spool_error(format!(
                "statistics spool contains unsupported entry {}",
                path.display()
            )));
        }
        inventory.bytes = inventory
            .bytes
            .checked_add(metadata.len())
            .ok_or_else(|| permanent_spool_error("statistics spool byte count overflow"))?;
        if inventory.bytes > max_bytes {
            return Err(permanent_spool_error(
                "statistics spool exceeds its byte limit; refusing unbounded recovery",
            ));
        }
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("json") => inventory.batches.push(path),
            Some("tmp") => inventory.temporaries.push(path),
            _ => {
                return Err(permanent_spool_error(format!(
                    "statistics spool contains unknown file {}",
                    path.display()
                )));
            }
        }
    }
    Ok(inventory)
}

fn ensure_spool_capacity(
    directory: &Path,
    additional_bytes: u64,
    additional_entries: usize,
    max_bytes: u64,
    max_files: usize,
) -> std::io::Result<()> {
    let inventory = inspect_spool(directory, max_bytes, max_files)?;
    let used = inventory
        .bytes
        .checked_add(additional_bytes)
        .ok_or_else(|| permanent_spool_error("statistics spool byte count overflow"))?;
    let files = inventory
        .entries
        .checked_add(additional_entries)
        .ok_or_else(|| permanent_spool_error("statistics spool entry count overflow"))?;
    if used > max_bytes || files > max_files {
        return Err(permanent_spool_error(
            "statistics spool is full; retaining batch in memory",
        ));
    }
    Ok(())
}

fn preflight_hard_links(directory: &Path) -> std::io::Result<()> {
    let source = directory.join(".hardlink-probe-source");
    let link = directory.join(".hardlink-probe-link");
    const PROBE: &[u8] = b"groupbot hard-link preflight";
    for path in [&link, &source] {
        match std::fs::symlink_metadata(path) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && !metadata.file_type().is_symlink()
                    && std::fs::read(path)? == PROBE =>
            {
                std::fs::remove_file(path)?;
            }
            Ok(_) => {
                return Err(std::io::Error::other(format!(
                    "statistics hard-link probe path is occupied: {}",
                    path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    sync_directory(directory)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&source)?;
    file.write_all(PROBE)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = std::fs::hard_link(&source, &link) {
        let _ = std::fs::remove_file(&source);
        let _ = sync_directory(directory);
        return Err(std::io::Error::new(
            error.kind(),
            format!("statistics spool does not support durable hard links: {error}"),
        ));
    }
    sync_directory(directory)?;
    std::fs::remove_file(&link)?;
    std::fs::remove_file(&source)?;
    sync_directory(directory)
}

fn verify_existing_batch(path: &Path, expected: &[u8]) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(permanent_spool_error(
            "statistics batch path is not a regular file",
        ));
    }
    if std::fs::read(path)? != expected {
        return Err(permanent_spool_error(
            "statistics batch id already exists with different content",
        ));
    }
    Ok(())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const URL: &str = "postgresql://moh@127.0.0.1:55432/groupbot_audit";

    fn audit_rank_due(_chat: i64, previous_total: u64, total: u64, awarded: u64) -> Option<u64> {
        (previous_total < total && total >= 1 && awarded < total).then_some(total)
    }

    #[test]
    fn recovery_rejects_unsafe_ids() {
        let mut batch = StatsBatch::new(1, vec![], vec![]);
        assert!(batch.validate_id().is_ok());
        for id in ["", "../abc", "/abc", "abc\\def", "A".repeat(129).as_str()] {
            batch.id = id.to_owned();
            assert!(batch.validate_id().is_err());
        }
    }

    #[test]
    fn batch_ids_do_not_depend_on_wall_clock_and_are_unique_in_process() {
        let mut ids = HashSet::new();
        for _ in 0..4096 {
            let batch = StatsBatch::new(1, Vec::new(), Vec::new());
            assert!(batch.validate_id().is_ok());
            assert!(ids.insert(batch.id));
        }
    }

    #[test]
    fn malformed_duplicate_keys_are_rejected_before_sql() {
        let duplicate_tally = StatsBatch::new(
            1,
            vec![(1, "messages".into(), 1), (1, "messages".into(), 2)],
            Vec::new(),
        );
        assert!(duplicate_tally.encoded().is_err());
        let duplicate_member = StatsBatch::new(
            1,
            Vec::new(),
            vec![
                Bump {
                    chat: 1,
                    user: 2,
                    name: "first".into(),
                    added: 1,
                },
                Bump {
                    chat: 1,
                    user: 2,
                    name: "second".into(),
                    added: 1,
                },
            ],
        );
        assert!(duplicate_member.encoded().is_err());
    }

    #[test]
    fn staging_never_accepts_different_content_for_an_existing_id() {
        let directory = std::env::temp_dir().join(new_opaque_id("groupbot-stage-test"));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("same-id.json");
        stage_spool_file(&directory, &path, b"first").unwrap();
        stage_spool_file(&directory, &path, b"first").unwrap();
        assert!(stage_spool_file(&directory, &path, b"second").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        std::fs::write(directory.join("crash.tmp"), vec![0; 1024]).unwrap();
        let before = std::fs::metadata(&path).unwrap().len();
        let full = ensure_spool_capacity(&directory, 2, 1, before + 2, 3).unwrap_err();
        assert!(is_permanent_spool_error(&full));
        assert!(classify_spool_error(full).is_permanent());
        let next = directory.join("next.json");
        stage_spool_file(&directory, &next, b"next").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn publication_persists_both_directory_transitions_in_order() {
        let directory = std::env::temp_dir().join(new_opaque_id("groupbot-publish-test"));
        std::fs::create_dir(&directory).unwrap();
        let temporary = directory.join("batch.tmp");
        let final_path = directory.join("batch.json");
        std::fs::write(&temporary, b"durable").unwrap();
        let mut steps = Vec::new();
        publish_temporary(&directory, &temporary, &final_path, b"durable", |step| {
            steps.push(step)
        })
        .unwrap();
        assert_eq!(
            steps,
            [
                PublicationStep::TemporarySynced,
                PublicationStep::FinalLinked,
                PublicationStep::DirectorySyncedBeforeUnlink,
                PublicationStep::TemporaryUnlinked,
                PublicationStep::DirectorySyncedAfterUnlink,
            ]
        );
        assert!(!temporary.exists());
        assert_eq!(std::fs::read(final_path).unwrap(), b"durable");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn complete_crash_temporary_is_promoted_but_partial_one_is_removed() {
        let directory = std::env::temp_dir().join(new_opaque_id("groupbot-recover-test"));
        std::fs::create_dir(&directory).unwrap();
        let batch = StatsBatch::new(3, vec![(4, "messages".into(), 5)], Vec::new());
        let bytes = batch.encoded().unwrap();
        let complete = directory.join(format!("{}.tmp", batch.id));
        std::fs::write(&complete, &bytes).unwrap();
        let recovered = recover_spool_temporary(&directory, &complete)
            .unwrap()
            .unwrap();
        assert_eq!(recovered, directory.join(format!("{}.json", batch.id)));
        assert_eq!(std::fs::read(recovered).unwrap(), bytes);
        assert!(!complete.exists());

        let partial = directory.join("partial.tmp");
        std::fs::write(&partial, b"{\"id\":").unwrap();
        assert!(
            recover_spool_temporary(&directory, &partial)
                .unwrap()
                .is_none()
        );
        assert!(!partial.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn complete_live_retry_reuses_its_canonical_temporary() {
        let directory = std::env::temp_dir().join(new_opaque_id("groupbot-retry-test"));
        std::fs::create_dir(&directory).unwrap();
        let batch = StatsBatch::new(3, Vec::new(), Vec::new());
        let bytes = batch.encoded().unwrap();
        let path = directory.join(format!("{}.json", batch.id));
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, &bytes).unwrap();
        stage_spool_file(&directory, &path, &bytes).unwrap();
        assert!(path.exists());
        assert!(!temporary.exists());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn noncanonical_complete_temporary_cannot_replay_after_receipt_gc() {
        let directory = std::env::temp_dir().join(new_opaque_id("groupbot-old-temp-test"));
        std::fs::create_dir(&directory).unwrap();
        let batch = StatsBatch::new(3, Vec::new(), Vec::new());
        let temporary = directory.join("old-stage-name.tmp");
        std::fs::write(&temporary, batch.encoded().unwrap()).unwrap();
        assert!(recover_spool_temporary(&directory, &temporary).is_err());
        assert!(
            temporary.exists(),
            "unsafe replay candidate must fail closed"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn hard_link_preflight_leaves_the_spool_empty() {
        let directory = std::env::temp_dir().join(new_opaque_id("groupbot-link-probe-test"));
        std::fs::create_dir(&directory).unwrap();
        preflight_hard_links(&directory).unwrap();
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn inventory_counts_temporaries_and_rejects_unknown_entries() {
        let directory = std::env::temp_dir().join(new_opaque_id("groupbot-cap-test"));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("one.json"), b"12").unwrap();
        std::fs::write(directory.join("two.tmp"), b"345").unwrap();
        let inventory = inspect_spool(&directory, 5, 2).unwrap();
        assert_eq!((inventory.bytes, inventory.entries), (5, 2));
        let full = ensure_spool_capacity(&directory, 1, 0, 5, 2).unwrap_err();
        assert!(is_permanent_spool_error(&full));
        assert!(classify_spool_error(full).is_permanent());
        std::fs::write(directory.join("unknown"), b"").unwrap();
        assert!(inspect_spool(&directory, 100, 10).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rank_retry_backoff_is_bounded() {
        assert_eq!(rank_retry_delay(1, None), MIN_RANK_RETRY_SECONDS);
        assert_eq!(rank_retry_delay(2, None), MIN_RANK_RETRY_SECONDS * 2);
        assert_eq!(rank_retry_delay(1, Some(600)), 600);
        assert_eq!(
            rank_retry_delay(i64::MAX, Some(u64::MAX)),
            MAX_RANK_RETRY_SECONDS
        );
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL transaction fault and disk-spool crash recovery"]
    async fn failed_and_uncertain_statistics_commits_replay_once() {
        let chat = -999_777_001;
        let mut settings = Settings::connect(URL).await.unwrap();
        let batch = StatsBatch::new(
            20_000,
            vec![(chat, "audit_durable".into(), 7)],
            vec![Bump {
                chat,
                user: 42,
                name: "audit".into(),
                added: 7,
            }],
        );
        let directory = std::env::temp_dir().join(format!("groupbot-stats-{}", batch.id));
        settings.stats_directory = Some(directory.clone());
        for table in ["counters", "tallies"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE chat_id=$1"))
                .bind(chat)
                .execute(&settings.pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM pending_rank_awards WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        settings.stage_stats(&batch).await.unwrap();
        let control = PgPool::connect(URL).await.unwrap();
        let mut blocker = control.begin().await.unwrap();
        sqlx::query("LOCK TABLE counters IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        assert!(settings.apply_stats(&batch, audit_rank_due).await.is_err());
        blocker.rollback().await.unwrap();
        let receipts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM applied_stats_batches WHERE id=$1")
                .bind(&batch.id)
                .fetch_one(&control)
                .await
                .unwrap();
        let tallies: i64 = sqlx::query_scalar("SELECT count(*) FROM tallies WHERE chat_id=$1")
            .bind(chat)
            .fetch_one(&control)
            .await
            .unwrap();
        assert_eq!(
            (receipts, tallies),
            (0, 0),
            "failed counter SQL must roll back both receipt and tally"
        );
        assert!(settings.apply_stats(&batch, audit_rank_due).await.unwrap());
        let mut colliding = StatsBatch::new(
            20_000,
            vec![(chat, "audit_durable".into(), 700)],
            Vec::new(),
        );
        colliding.id.clone_from(&batch.id);
        let collision = match settings.apply_stats(&colliding, audit_rank_due).await {
            Err(error) => error,
            Ok(_) => panic!("a reused id with another payload must fail closed"),
        };
        assert!(
            collision
                .to_string()
                .contains("already applied with different content")
        );
        drop(settings);
        let mut restarted = Settings::connect(URL).await.unwrap();
        restarted.stats_directory = Some(directory.clone());
        restarted.recover_stats(audit_rank_due).await.unwrap();
        assert!(!restarted.apply_stats(&batch, audit_rank_due).await.unwrap());
        let mut awards = restarted.claim_rank_awards(4096).await.unwrap();
        let delivery = awards
            .iter()
            .position(|delivery| delivery.bumped.chat == chat && delivery.bumped.user == 42)
            .map(|position| awards.swap_remove(position))
            .expect("the committed counter must leave a durable rank side effect");
        assert_eq!(delivery.bumped.total, 7);
        assert!(restarted.ack_rank_award(&delivery, Some(7)).await.unwrap());
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pending_rank_awards WHERE chat_id=$1 AND user_id=42)",
        )
        .bind(chat)
        .fetch_one(&control)
        .await
        .unwrap();
        assert!(!pending);
        let total: i64 =
            sqlx::query_scalar("SELECT total FROM counters WHERE chat_id=$1 AND user_id=42")
                .bind(chat)
                .fetch_one(&control)
                .await
                .unwrap();
        let tally: i64 = sqlx::query_scalar(
            "SELECT count FROM tallies WHERE chat_id=$1 AND counter='audit_durable'",
        )
        .bind(chat)
        .fetch_one(&control)
        .await
        .unwrap();
        assert_eq!((total, tally), (7, 7));
        let awarded: i64 =
            sqlx::query_scalar("SELECT awarded FROM counters WHERE chat_id=$1 AND user_id=42")
                .bind(chat)
                .fetch_one(&control)
                .await
                .unwrap();
        assert_eq!(awarded, 7);
        assert!(!directory.join(format!("{}.json", batch.id)).exists());
        restarted.stage_stats(&batch).await.unwrap();
        sqlx::query(
            "UPDATE applied_stats_batches SET applied_at=now()-interval '91 days' WHERE id=$1",
        )
        .bind(&batch.id)
        .execute(&control)
        .await
        .unwrap();
        restarted.prune_stats_receipts().await.unwrap();
        let retained: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM applied_stats_batches WHERE id=$1)")
                .bind(&batch.id)
                .fetch_one(&control)
                .await
                .unwrap();
        assert!(retained);
        restarted.recover_stats(audit_rank_due).await.unwrap();
        let total_after_gc: i64 =
            sqlx::query_scalar("SELECT total FROM counters WHERE chat_id=$1 AND user_id=42")
                .bind(chat)
                .fetch_one(&control)
                .await
                .unwrap();
        assert_eq!(total_after_gc, 7);
        std::fs::remove_dir(directory).unwrap();
        println!(
            "PERF {}",
            serde_json::json!({"kind":"durable_statistics", "sql_failure_rolled_back":true, "restart_replay_exactly_once":true, "counter":total,"tally":tally})
        );
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL runtime tally-cap regression"]
    async fn statistics_batch_cannot_exceed_one_chats_tally_domain() {
        let chat = -999_777_099;
        let settings = Settings::connect(URL).await.unwrap();
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 1, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM tallies WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        let batch = StatsBatch::new(
            1,
            (0..=MAX_TALLY_ROWS_PER_CHAT)
                .map(|counter| (chat, format!("audit_{counter}"), 1))
                .collect(),
            Vec::new(),
        );
        let error = settings
            .apply_stats(&batch, audit_rank_due)
            .await
            .unwrap_err();
        assert!(error.is_permanent());
        assert!(
            error
                .to_string()
                .contains(&(MAX_TALLY_ROWS_PER_CHAT + 1).to_string())
        );
        let (rows, receipts): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM tallies WHERE chat_id = $1),
                    (SELECT count(*) FROM applied_stats_batches WHERE id = $2)",
        )
        .bind(chat)
        .bind(&batch.id)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(
            (rows, receipts),
            (0, 0),
            "capacity refusal must roll back both tally rows and idempotency receipt"
        );
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL foreign statistics spool ownership test"]
    async fn foreign_statistics_spool_cannot_manufacture_a_durable_chat() {
        let chat = -999_777_098_i64;
        let settings = Settings::connect(URL).await.unwrap();
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
        let batch = StatsBatch::new(1, vec![(chat, "messages".to_owned(), 1)], Vec::new());
        let error = settings
            .apply_stats(&batch, audit_rank_due)
            .await
            .unwrap_err();
        assert!(error.is_permanent());
        let (owners, tallies, receipts): (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM durable_chats WHERE chat_id = $1),
                    (SELECT count(*) FROM tallies WHERE chat_id = $1),
                    (SELECT count(*) FROM applied_stats_batches WHERE id = $2)",
        )
        .bind(chat)
        .bind(&batch.id)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(
            (owners, tallies, receipts),
            (0, 0, 0),
            "foreign durable state and its receipt must roll back together"
        );
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL outbox lease/version race"]
    async fn stale_award_ack_cannot_erase_a_newer_counter_update() {
        let chat = -999_777_002;
        let settings = Settings::connect(URL).await.unwrap();
        sqlx::query("DELETE FROM pending_rank_awards WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM counters WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();

        let make_batch = || {
            StatsBatch::new(
                20_000,
                Vec::new(),
                vec![Bump {
                    chat,
                    user: 43,
                    name: "lease".into(),
                    added: 1,
                }],
            )
        };
        settings
            .apply_stats(&make_batch(), audit_rank_due)
            .await
            .unwrap();
        let first = settings
            .claim_rank_awards(4096)
            .await
            .unwrap()
            .into_iter()
            .find(|delivery| delivery.bumped.chat == chat)
            .expect("first delivery");

        settings
            .apply_stats(&make_batch(), audit_rank_due)
            .await
            .unwrap();
        assert!(!settings.ack_rank_award(&first, Some(1)).await.unwrap());
        let second = settings
            .claim_rank_awards(4096)
            .await
            .unwrap()
            .into_iter()
            .find(|delivery| delivery.bumped.chat == chat)
            .expect("newer delivery");
        assert_eq!(second.bumped.total, 2);
        assert!(settings.ack_rank_award(&second, Some(2)).await.unwrap());
        let state: (i64, bool) = sqlx::query_as(
            "SELECT awarded,
                    EXISTS(SELECT 1 FROM pending_rank_awards WHERE chat_id=$1 AND user_id=43)
             FROM counters WHERE chat_id=$1 AND user_id=43",
        )
        .bind(chat)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(state, (2, false));
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL rank-outbox retry retention"]
    async fn transient_awards_are_retained_and_retry_exhaustion_is_terminal() {
        let chat = -999_777_005;
        let settings = Settings::connect(URL).await.unwrap();
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 0, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        for table in ["pending_rank_awards", "counters"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE chat_id=$1"))
                .bind(chat)
                .execute(&settings.pool)
                .await
                .unwrap();
        }
        let batch = StatsBatch::new(
            20_000,
            Vec::new(),
            vec![Bump {
                chat,
                user: 45,
                name: "retry".into(),
                added: 1,
            }],
        );
        settings.apply_stats(&batch, audit_rank_due).await.unwrap();
        let delivery = settings.claim_rank_awards(1).await.unwrap().pop().unwrap();
        assert!(matches!(
            settings
                .defer_rank_award(&delivery, "telegram_io", None)
                .await
                .unwrap(),
            RankRetryDisposition::Deferred { .. }
        ));
        let retained: (i64, bool, bool) = sqlx::query_as(
            "SELECT attempts::bigint, lease_until > now(), terminal_reason IS NULL
             FROM pending_rank_awards WHERE chat_id=$1 AND user_id=45",
        )
        .bind(chat)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(retained, (1, true, true));

        sqlx::query(
            "UPDATE pending_rank_awards SET attempts=$2, lease_until='-infinity'
             WHERE chat_id=$1 AND user_id=45",
        )
        .bind(chat)
        .bind(MAX_RANK_AWARD_ATTEMPTS - 1)
        .execute(&settings.pool)
        .await
        .unwrap();
        let final_attempt = settings.claim_rank_awards(1).await.unwrap().pop().unwrap();
        assert_eq!(final_attempt.attempts, MAX_RANK_AWARD_ATTEMPTS);
        assert_eq!(
            settings
                .defer_rank_award(&final_attempt, "telegram_io", None)
                .await
                .unwrap(),
            RankRetryDisposition::Terminal
        );
        let terminal: (bool, bool) = sqlx::query_as(
            "SELECT terminal_reason = 'retry_limit:telegram_io', lease_until = 'infinity'
             FROM pending_rank_awards WHERE chat_id=$1 AND user_id=45",
        )
        .bind(chat)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(terminal, (true, true));
        sqlx::query("DELETE FROM pending_rank_awards WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM counters WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL rank-outbox eligibility"]
    async fn outbox_only_records_enabled_new_milestones() {
        let enabled_chat = -999_777_003;
        let disabled_chat = -999_777_004;
        let settings = Settings::connect(URL).await.unwrap();
        for chat in [enabled_chat, disabled_chat] {
            sqlx::query("DELETE FROM pending_rank_awards WHERE chat_id=$1")
                .bind(chat)
                .execute(&settings.pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM counters WHERE chat_id=$1")
                .bind(chat)
                .execute(&settings.pool)
                .await
                .unwrap();
        }
        let due = |chat: i64, previous_total: u64, total: u64, awarded: u64| {
            if chat == disabled_chat {
                None
            } else {
                [50, 100]
                    .into_iter()
                    .rev()
                    .find(|milestone| total >= *milestone)
                    .filter(|milestone| previous_total < *milestone && awarded < *milestone)
            }
        };
        let batch = |chat, added| {
            StatsBatch::new(
                20_000,
                Vec::new(),
                vec![Bump {
                    chat,
                    user: 44,
                    name: "eligibility".into(),
                    added,
                }],
            )
        };

        settings
            .apply_stats(&batch(enabled_chat, 49), due)
            .await
            .unwrap();
        settings
            .apply_stats(&batch(disabled_chat, 50), due)
            .await
            .unwrap();
        let before_crossing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pending_rank_awards WHERE chat_id=ANY($1)")
                .bind(vec![enabled_chat, disabled_chat])
                .fetch_one(&settings.pool)
                .await
                .unwrap();
        assert_eq!(before_crossing, 0);

        settings
            .apply_stats(&batch(enabled_chat, 1), due)
            .await
            .unwrap();
        settings
            .apply_stats(&batch(enabled_chat, 1), due)
            .await
            .unwrap();
        let after_crossing: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT count(*), COALESCE(max(total), 0), COALESCE(max(milestone), 0),
                    COALESCE(max(version), 0)
             FROM pending_rank_awards WHERE chat_id=$1",
        )
        .bind(enabled_chat)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(after_crossing, (1, 50, 50, 1));
    }
}
