use super::*;

pub const MAX_PENDING_STRICT_ACTIONS_PER_CHAT: i64 = 256;
const MAX_ERROR_BYTES: usize = 1_024;
const MAX_TARGET_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrictAction {
    Ban,
    Mute,
}

impl StrictAction {
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
                "invalid pending strict action {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrictIntentState {
    None,
    Active,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StrictIncrement {
    pub count: u32,
    pub intent: StrictIntentState,
}

#[derive(Clone, Copy, Debug)]
pub struct StrictViolation<'a> {
    pub chat: i64,
    pub user: i64,
    pub day: u64,
    pub expiry_days: u64,
    pub limit: u32,
    pub action: StrictAction,
    pub duration_seconds: Option<u64>,
    pub until_date: i32,
    pub target_name: &'a str,
    pub wipe_history: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingStrictAction {
    pub chat: i64,
    pub user: i64,
    pub action: StrictAction,
    pub duration_seconds: Option<u64>,
    pub until_date: i32,
    pub threshold: u32,
    pub target_name: String,
    pub wipe_history: bool,
    pub awaiting_rejoin: bool,
    pub attempts: u32,
    generation: i64,
    version: i64,
    lease_token: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrictQueueClass {
    Fresh,
    Retry,
}

type PendingStrictTuple = (
    i64,
    i64,
    String,
    Option<i64>,
    i32,
    i32,
    String,
    bool,
    bool,
    i32,
    i64,
    i64,
    i64,
);

fn decode_pending_strict(row: PendingStrictTuple) -> Result<PendingStrictAction> {
    let duration_seconds = row
        .3
        .map(|seconds| {
            u64::try_from(seconds)
                .map_err(|_| sqlx::Error::Protocol("negative pending strict duration".to_owned()))
        })
        .transpose()?;
    let threshold = u32::try_from(row.5)
        .map_err(|_| sqlx::Error::Protocol("invalid pending strict threshold".to_owned()))?;
    if threshold == 0 {
        return Err(sqlx::Error::Protocol(
            "zero pending strict threshold".to_owned(),
        ));
    }
    Ok(PendingStrictAction {
        chat: row.0,
        user: row.1,
        action: StrictAction::from_db(&row.2)?,
        duration_seconds,
        until_date: row.4,
        threshold,
        target_name: row.6,
        wipe_history: row.7,
        awaiting_rejoin: row.8,
        attempts: u32::try_from(row.9)
            .map_err(|_| sqlx::Error::Protocol("negative strict attempt count".to_owned()))?,
        generation: row.10,
        version: row.11,
        lease_token: row.12,
    })
}

fn bounded_error(reason: &str) -> String {
    bounded_text(reason, MAX_ERROR_BYTES)
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    let mut kept = String::with_capacity(value.len().min(max_bytes));
    for character in value.chars() {
        if kept.len() + character.len_utf8() > max_bytes {
            break;
        }
        kept.push(character);
    }
    kept
}

impl Settings {
    pub(super) async fn init_strict(connection: &mut sqlx::PgConnection) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_strict_actions (
                chat_id BIGINT NOT NULL,
                user_id BIGINT NOT NULL,
                action TEXT NOT NULL CHECK (action IN ('ban', 'mute')),
                duration_seconds BIGINT CHECK (duration_seconds IS NULL OR duration_seconds > 0),
                until_date INT NOT NULL DEFAULT 0,
                threshold INT NOT NULL CHECK (threshold > 0),
                target_name TEXT NOT NULL,
                wipe_history BOOLEAN NOT NULL,
                created_at BIGINT NOT NULL,
                available_at BIGINT NOT NULL,
                attempts INT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
                generation BIGINT NOT NULL DEFAULT nextval('durable_work_token_seq'),
                version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
                lease_token BIGINT,
                awaiting_rejoin BOOLEAN NOT NULL DEFAULT FALSE,
                terminal_at BIGINT,
                last_error TEXT,
                PRIMARY KEY (chat_id, user_id)
            )",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "ALTER TABLE pending_strict_actions
             ADD COLUMN IF NOT EXISTS until_date INT NOT NULL DEFAULT 0,
             ADD COLUMN IF NOT EXISTS awaiting_rejoin BOOLEAN NOT NULL DEFAULT FALSE",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "UPDATE pending_strict_actions
             SET until_date = LEAST(2147483647::BIGINT,
                                    GREATEST(1, created_at + duration_seconds))::INT
             WHERE until_date = 0 AND duration_seconds IS NOT NULL",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query("DROP INDEX IF EXISTS pending_strict_fresh_due")
            .execute(&mut *connection)
            .await?;
        sqlx::query("DROP INDEX IF EXISTS pending_strict_retry_due")
            .execute(&mut *connection)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS pending_strict_fresh_due
             ON pending_strict_actions (available_at, created_at, chat_id, user_id)
             WHERE attempts = 0 AND terminal_at IS NULL AND NOT awaiting_rejoin",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS pending_strict_retry_due
             ON pending_strict_actions (available_at, attempts, created_at, chat_id, user_id)
             WHERE attempts > 0 AND terminal_at IS NULL AND NOT awaiting_rejoin",
        )
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS pending_strict_terminal_age
             ON pending_strict_actions (terminal_at, chat_id, user_id)
             WHERE terminal_at IS NOT NULL",
        )
        .execute(connection)
        .await?;
        Ok(())
    }

    pub async fn increment_strict(
        &self,
        violation: StrictViolation<'_>,
    ) -> Result<Option<StrictIncrement>> {
        let StrictViolation {
            chat,
            user,
            day,
            expiry_days,
            limit,
            action,
            duration_seconds,
            until_date,
            target_name,
            wipe_history,
        } = violation;
        if limit == 0 {
            return Err(sqlx::Error::Protocol(
                "strict threshold must be positive".to_owned(),
            ));
        }
        let duration_seconds = duration_seconds
            .map(|seconds| {
                i64::try_from(seconds).map_err(|_| {
                    sqlx::Error::Protocol("strict duration exceeds PostgreSQL bigint".to_owned())
                })
            })
            .transpose()?;
        let threshold = i32::try_from(limit).map_err(|_| {
            sqlx::Error::Protocol("strict threshold exceeds PostgreSQL integer".to_owned())
        })?;
        let target_name = bounded_text(target_name, MAX_TARGET_BYTES);
        let now = unix_now();
        let slot = self.write_slot(chat);
        let _writing = slot.lock().await;
        let mut tx = self.pool.begin().await?;

        let existing: Option<(bool, bool, i64)> = sqlx::query_as(
            "SELECT pending.terminal_at IS NOT NULL, pending.awaiting_rejoin,
                    COALESCE(counters.strikes, 0)
             FROM pending_strict_actions AS pending
             LEFT JOIN counters USING (chat_id, user_id)
             WHERE pending.chat_id = $1 AND pending.user_id = $2
             FOR UPDATE OF pending",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((terminal, awaiting_rejoin, count)) = existing {
            if terminal {
                sqlx::query(
                    "UPDATE pending_strict_actions
                     SET action = $3, duration_seconds = $4, until_date = $5, threshold = $6,
                         target_name = $7, wipe_history = $8, created_at = $9,
                         available_at = $9, attempts = 0,
                         generation = nextval('durable_work_token_seq'),
                         version = version + 1, lease_token = NULL,
                         awaiting_rejoin = FALSE, terminal_at = NULL, last_error = NULL
                     WHERE chat_id = $1 AND user_id = $2 AND terminal_at IS NOT NULL",
                )
                .bind(chat)
                .bind(user)
                .bind(action.as_db())
                .bind(duration_seconds)
                .bind(until_date)
                .bind(threshold)
                .bind(target_name)
                .bind(wipe_history)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            } else if awaiting_rejoin {
                sqlx::query(
                    "UPDATE pending_strict_actions
                     SET awaiting_rejoin = FALSE, available_at = $3, last_error = NULL
                     WHERE chat_id = $1 AND user_id = $2 AND awaiting_rejoin
                       AND lease_token IS NULL AND terminal_at IS NULL",
                )
                .bind(chat)
                .bind(user)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            return Ok(Some(StrictIncrement {
                count: warning_count(count)?,
                intent: StrictIntentState::Active,
            }));
        }

        let count = if limit == 1 {
            1
        } else {
            let max_counter_rows = self.max_counter_rows.unwrap_or(i64::MAX / 2);
            let count: Option<i64> = sqlx::query_scalar(
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
                 INSERT INTO counters (chat_id, user_id, strikes, struck)
                 SELECT $1, $2, 1, $3
                 WHERE EXISTS (SELECT 1 FROM existing) OR EXISTS (SELECT 1 FROM reserved)
                 ON CONFLICT (chat_id, user_id) DO UPDATE SET
                     strikes = CASE WHEN $3 - counters.struck < $4
                                    THEN counters.strikes ELSE 0 END + 1,
                     struck = $3
                 RETURNING strikes",
            )
            .bind(chat)
            .bind(user)
            .bind(saturating_postgres_i64(day))
            .bind(saturating_postgres_i64(expiry_days))
            .bind(max_counter_rows)
            .bind(MAX_COUNTER_ROWS_PER_CHAT)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(count) = count else {
                tx.rollback().await?;
                return Ok(None);
            };
            warning_count(count)?
        };

        if count < limit {
            tx.commit().await?;
            return Ok(Some(StrictIncrement {
                count,
                intent: StrictIntentState::None,
            }));
        }

        let admitted: bool = sqlx::query_scalar(
            "SELECT count(*) < $2 FROM pending_strict_actions WHERE chat_id = $1",
        )
        .bind(chat)
        .bind(MAX_PENDING_STRICT_ACTIONS_PER_CHAT)
        .fetch_one(&mut *tx)
        .await?;
        if !admitted {
            tx.rollback().await?;
            return Ok(None);
        }
        sqlx::query(
            "INSERT INTO pending_strict_actions
             (chat_id, user_id, action, duration_seconds, until_date, threshold, target_name, wipe_history,
              created_at, available_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)",
        )
        .bind(chat)
        .bind(user)
        .bind(action.as_db())
        .bind(duration_seconds)
        .bind(until_date)
        .bind(threshold)
        .bind(target_name)
        .bind(wipe_history)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(StrictIncrement {
            count,
            intent: StrictIntentState::Active,
        }))
    }

    pub async fn claim_strict_action(
        &self,
        chat: i64,
        user: i64,
        now: i64,
        lease_until: i64,
    ) -> Result<Option<PendingStrictAction>> {
        let row: Option<PendingStrictTuple> = sqlx::query_as(
            "UPDATE pending_strict_actions
             SET available_at = $4,
                 attempts = attempts + 1,
                 version = version + 1,
                 lease_token = nextval('durable_work_token_seq')
             WHERE chat_id = $1 AND user_id = $2
               AND terminal_at IS NULL AND NOT awaiting_rejoin AND available_at <= $3
             RETURNING chat_id, user_id, action, duration_seconds, until_date, threshold, target_name,
                       wipe_history, awaiting_rejoin, attempts,
                       generation, version, lease_token",
        )
        .bind(chat)
        .bind(user)
        .bind(now)
        .bind(lease_until)
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_pending_strict).transpose()
    }

    pub async fn claim_pending_strict_actions(
        &self,
        class: StrictQueueClass,
        now: i64,
        lease_until: i64,
        limit: i64,
    ) -> Result<Vec<PendingStrictAction>> {
        let predicate = match class {
            StrictQueueClass::Fresh => {
                "attempts = 0 AND terminal_at IS NULL AND NOT awaiting_rejoin AND available_at <= $1"
            }
            StrictQueueClass::Retry => {
                "attempts > 0 AND terminal_at IS NULL AND NOT awaiting_rejoin AND available_at <= $1"
            }
        };
        let statement = format!(
            "WITH due AS (
                 SELECT chat_id, user_id FROM pending_strict_actions
                 WHERE {predicate}
                 ORDER BY available_at, attempts, created_at, chat_id, user_id
                 FOR UPDATE SKIP LOCKED LIMIT $3
             )
             UPDATE pending_strict_actions AS pending
             SET available_at = $2,
                 attempts = pending.attempts + 1,
                 version = pending.version + 1,
                 lease_token = nextval('durable_work_token_seq')
             FROM due
             WHERE pending.chat_id = due.chat_id AND pending.user_id = due.user_id
             RETURNING pending.chat_id, pending.user_id, pending.action,
                       pending.duration_seconds, pending.until_date, pending.threshold, pending.target_name,
                       pending.wipe_history, pending.awaiting_rejoin, pending.attempts,
                       pending.generation, pending.version, pending.lease_token"
        );
        let rows: Vec<PendingStrictTuple> = sqlx::query_as(&statement)
            .bind(now)
            .bind(lease_until)
            .bind(limit.clamp(1, 64))
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(decode_pending_strict).collect()
    }

    pub async fn strict_action_is_current(&self, pending: &PendingStrictAction) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM pending_strict_actions
                 WHERE chat_id = $1 AND user_id = $2 AND action = $3
                   AND generation = $4 AND version = $5 AND lease_token = $6
                   AND attempts = $7 AND terminal_at IS NULL AND NOT awaiting_rejoin
             )",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.action.as_db())
        .bind(pending.generation)
        .bind(pending.version)
        .bind(pending.lease_token)
        .bind(i64::from(pending.attempts))
        .fetch_one(&self.pool)
        .await
    }

    pub async fn defer_strict_action(
        &self,
        pending: &PendingStrictAction,
        retry_at: i64,
        reason: &str,
    ) -> Result<bool> {
        let reason = bounded_error(reason);
        let updated = sqlx::query(
            "UPDATE pending_strict_actions
             SET available_at = $8, lease_token = NULL, awaiting_rejoin = FALSE, last_error = $9
             WHERE chat_id = $1 AND user_id = $2 AND action = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7 AND terminal_at IS NULL",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.action.as_db())
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

    pub async fn await_strict_rejoin(
        &self,
        pending: &PendingStrictAction,
        reason: &str,
    ) -> Result<bool> {
        let reason = bounded_error(reason);
        let mut tx = self.pool.begin().await?;
        super::lock_durable_member_rejoin(&mut tx, pending.chat, pending.user).await?;
        let updated = sqlx::query(
            "UPDATE pending_strict_actions
             SET awaiting_rejoin = TRUE, lease_token = NULL, last_error = $8
             WHERE chat_id = $1 AND user_id = $2 AND action = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7 AND terminal_at IS NULL",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.action.as_db())
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

    pub async fn resume_strict_on_rejoin(&self, chat: i64, user: i64, now: i64) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        super::lock_durable_member_rejoin(&mut tx, chat, user).await?;
        let updated = sqlx::query(
            "UPDATE pending_strict_actions
             SET awaiting_rejoin = FALSE, available_at = $3, last_error = NULL
             WHERE chat_id = $1 AND user_id = $2 AND awaiting_rejoin
               AND lease_token IS NULL AND terminal_at IS NULL",
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

    pub async fn complete_strict_action(&self, pending: &PendingStrictAction) -> Result<bool> {
        let slot = self.write_slot(pending.chat);
        let _writing = slot.lock().await;
        let mut tx = self.pool.begin().await?;
        let removed = sqlx::query(
            "DELETE FROM pending_strict_actions
             WHERE chat_id = $1 AND user_id = $2 AND action = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7 AND terminal_at IS NULL AND NOT awaiting_rejoin",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.action.as_db())
        .bind(pending.generation)
        .bind(pending.version)
        .bind(pending.lease_token)
        .bind(i64::from(pending.attempts))
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if removed {
            sqlx::query(
                "UPDATE counters SET strikes = 0, struck = 0 WHERE chat_id = $1 AND user_id = $2",
            )
            .bind(pending.chat)
            .bind(pending.user)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(removed)
    }

    pub async fn dead_letter_strict_action(
        &self,
        pending: &PendingStrictAction,
        terminal_at: i64,
        reason: &str,
    ) -> Result<bool> {
        let reason = bounded_error(reason);
        let updated = sqlx::query(
            "UPDATE pending_strict_actions
             SET terminal_at = $8, available_at = $8, lease_token = NULL,
                 awaiting_rejoin = FALSE, last_error = $9
             WHERE chat_id = $1 AND user_id = $2 AND action = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7 AND terminal_at IS NULL",
        )
        .bind(pending.chat)
        .bind(pending.user)
        .bind(pending.action.as_db())
        .bind(pending.generation)
        .bind(pending.version)
        .bind(pending.lease_token)
        .bind(i64::from(pending.attempts))
        .bind(terminal_at)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn purge_strict_dead_letters(&self, before: i64, limit: i64) -> Result<u64> {
        let removed = sqlx::query(
            "DELETE FROM pending_strict_actions
             WHERE terminal_at IS NOT NULL AND terminal_at <= $1
               AND ctid IN (
                 SELECT ctid FROM pending_strict_actions
                 WHERE terminal_at IS NOT NULL AND terminal_at <= $1
                 ORDER BY terminal_at, chat_id, user_id
                 FOR UPDATE SKIP LOCKED LIMIT $2
             )",
        )
        .bind(before)
        .bind(limit.clamp(1, 256))
        .execute(&self.pool)
        .await?;
        Ok(removed.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn violation(
        chat: i64,
        user: i64,
        day: u64,
        limit: u32,
        action: StrictAction,
        duration_seconds: Option<u64>,
    ) -> StrictViolation<'static> {
        StrictViolation {
            chat,
            user,
            day,
            expiry_days: 7,
            limit,
            action,
            duration_seconds,
            until_date: duration_seconds.map_or(0, |seconds| {
                i32::try_from(1_700_000_000_u64.saturating_add(seconds)).unwrap_or(i32::MAX)
            }),
            target_name: "member",
            wipe_history: false,
        }
    }

    async fn clean(settings: &Settings, chat: i64) {
        sqlx::query("DELETE FROM pending_strict_actions WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query(
            "WITH removed AS (
                 DELETE FROM counters WHERE chat_id = $1 RETURNING 1
             )
             UPDATE durable_counts
             SET counter_rows = GREATEST(0, counter_rows - (SELECT count(*) FROM removed))
             WHERE id = 0",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 1, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL strict action transaction test"]
    async fn threshold_intent_and_strike_reset_are_atomic() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let (chat, user) = (-999_999_998_601_i64, 601_i64);
        clean(&settings, chat).await;

        for expected in 1..=2 {
            let increment = settings
                .increment_strict(violation(chat, user, 100, 3, StrictAction::Mute, Some(300)))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(increment.count, expected);
            assert_eq!(increment.intent, StrictIntentState::None);
        }
        let crossed = settings
            .increment_strict(StrictViolation {
                wipe_history: true,
                ..violation(chat, user, 100, 3, StrictAction::Mute, Some(300))
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(crossed.count, 3);
        assert_eq!(crossed.intent, StrictIntentState::Active);
        let durable: (i64, bool, String, bool, i32) = sqlx::query_as(
            "SELECT counters.strikes,
                    EXISTS(SELECT 1 FROM pending_strict_actions
                           WHERE chat_id = $1 AND user_id = $2),
                    pending.target_name, pending.wipe_history, pending.until_date
             FROM counters
             JOIN pending_strict_actions AS pending USING (chat_id, user_id)
             WHERE counters.chat_id = $1 AND counters.user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(durable, (3, true, "member".to_owned(), true, 1_700_000_300));

        let now = unix_now();
        let claimed = settings
            .claim_strict_action(chat, user, now, now + 60)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.until_date, 1_700_000_300);
        assert!(settings.complete_strict_action(&claimed).await.unwrap());
        let settled: (i64, bool) = sqlx::query_as(
            "SELECT counters.strikes,
                    EXISTS(SELECT 1 FROM pending_strict_actions
                           WHERE chat_id = $1 AND user_id = $2)
             FROM counters WHERE chat_id = $1 AND user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(settled, (0, false));
        clean(&settings, chat).await;
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL strict lease crash-point test"]
    async fn expired_lease_recovery_fences_a_pre_crash_acknowledgement() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let (chat, user) = (-999_999_998_602_i64, 602_i64);
        clean(&settings, chat).await;
        settings
            .increment_strict(violation(chat, user, 100, 1, StrictAction::Ban, None))
            .await
            .unwrap()
            .unwrap();
        let now = unix_now();
        let (first, second) = tokio::join!(
            settings.claim_strict_action(chat, user, now, now + 60),
            settings.claim_strict_action(chat, user, now, now + 60)
        );
        let claims: Vec<_> = [first.unwrap(), second.unwrap()]
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(claims.len(), 1);
        let pre_crash = claims.into_iter().next().unwrap();
        assert!(
            settings
                .claim_strict_action(chat, user, now + 59, now + 119)
                .await
                .unwrap()
                .is_none()
        );
        let recovered = settings
            .claim_strict_action(chat, user, now + 61, now + 121)
            .await
            .unwrap()
            .unwrap();
        assert!(!settings.complete_strict_action(&pre_crash).await.unwrap());
        assert!(settings.complete_strict_action(&recovered).await.unwrap());
        clean(&settings, chat).await;
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL strict retry and dead-letter test"]
    async fn retry_and_dead_letter_keep_one_bounded_lifecycle() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let (chat, user) = (-999_999_998_603_i64, 603_i64);
        clean(&settings, chat).await;
        let below = settings
            .increment_strict(violation(chat, user, 100, 2, StrictAction::Mute, None))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(below.intent, StrictIntentState::None);
        settings
            .increment_strict(violation(chat, user, 100, 2, StrictAction::Mute, None))
            .await
            .unwrap()
            .unwrap();
        let now = unix_now();
        let first = settings
            .claim_strict_action(chat, user, now, now + 60)
            .await
            .unwrap()
            .unwrap();
        assert!(
            settings
                .await_strict_rejoin(&first, "USER_NOT_PARTICIPANT")
                .await
                .unwrap()
        );
        assert!(
            settings
                .claim_strict_action(chat, user, i64::MAX, i64::MAX)
                .await
                .unwrap()
                .is_none(),
            "an absent member must not consume retry leases"
        );
        let retained: (i64, bool) = sqlx::query_as(
            "SELECT counters.strikes, pending.awaiting_rejoin
             FROM counters JOIN pending_strict_actions AS pending USING (chat_id, user_id)
             WHERE counters.chat_id = $1 AND counters.user_id = $2",
        )
        .bind(chat)
        .bind(user)
        .fetch_one(&settings.pool)
        .await
        .unwrap();
        assert_eq!(retained, (2, true));
        assert!(
            settings
                .resume_strict_on_rejoin(chat, user, now)
                .await
                .unwrap()
        );
        let first = settings
            .claim_strict_action(chat, user, now, now + 60)
            .await
            .unwrap()
            .unwrap();
        assert!(
            settings
                .defer_strict_action(&first, now + 100, "temporary failure")
                .await
                .unwrap()
        );
        assert!(
            settings
                .claim_strict_action(chat, user, now + 99, now + 159)
                .await
                .unwrap()
                .is_none()
        );
        let retry = settings
            .claim_strict_action(chat, user, now + 100, now + 160)
            .await
            .unwrap()
            .unwrap();
        assert!(
            settings
                .dead_letter_strict_action(&retry, now + 101, "permanent failure")
                .await
                .unwrap()
        );
        assert!(
            settings
                .claim_strict_action(chat, user, i64::MAX, i64::MAX)
                .await
                .unwrap()
                .is_none()
        );
        let reactivated = settings
            .increment_strict(violation(chat, user, 101, 1, StrictAction::Mute, None))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reactivated.intent, StrictIntentState::Active);
        assert!(!settings.complete_strict_action(&retry).await.unwrap());
        let reactivated = settings
            .claim_strict_action(chat, user, unix_now(), unix_now() + 60)
            .await
            .unwrap()
            .unwrap();
        assert!(
            settings
                .dead_letter_strict_action(&reactivated, now + 102, "still permanent")
                .await
                .unwrap()
        );
        assert_eq!(
            settings
                .purge_strict_dead_letters(now + 101, 10)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            settings
                .purge_strict_dead_letters(now + 102, 10)
                .await
                .unwrap(),
            1
        );
        clean(&settings, chat).await;
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL strict admission rollback test"]
    async fn full_action_capacity_rolls_back_the_threshold_strike() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let (chat, user) = (-999_999_998_604_i64, 604_i64);
        clean(&settings, chat).await;
        sqlx::query(
            "INSERT INTO pending_strict_actions
             (chat_id, user_id, action, threshold, target_name, wipe_history,
              created_at, available_at)
             SELECT $1, member, 'mute', 1, 'fixture', false, $2, $2
             FROM generate_series(10000, 10255) AS member",
        )
        .bind(chat)
        .bind(unix_now())
        .execute(&settings.pool)
        .await
        .unwrap();

        let first = settings
            .increment_strict(violation(chat, user, 100, 2, StrictAction::Mute, None))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.count, 1);
        assert_eq!(first.intent, StrictIntentState::None);
        assert!(
            settings
                .increment_strict(violation(chat, user, 100, 2, StrictAction::Mute, None))
                .await
                .unwrap()
                .is_none()
        );
        let strikes: i64 =
            sqlx::query_scalar("SELECT strikes FROM counters WHERE chat_id = $1 AND user_id = $2")
                .bind(chat)
                .bind(user)
                .fetch_one(&settings.pool)
                .await
                .unwrap();
        assert_eq!(strikes, 1);
        clean(&settings, chat).await;
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL strict queue-class test"]
    async fn fresh_and_retry_claims_are_separate_and_bounded() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chat = -999_999_998_605_i64;
        clean(&settings, chat).await;
        for user in [605_i64, 606_i64] {
            settings
                .increment_strict(violation(chat, user, 100, 1, StrictAction::Mute, None))
                .await
                .unwrap()
                .unwrap();
        }
        let now = unix_now().saturating_add(1);
        let retry = settings
            .claim_strict_action(chat, 605, now, now + 60)
            .await
            .unwrap()
            .unwrap();
        assert!(
            settings
                .defer_strict_action(&retry, now + 1, "retry fixture")
                .await
                .unwrap()
        );
        let fresh = settings
            .claim_pending_strict_actions(StrictQueueClass::Fresh, now + 1, now + 61, 64)
            .await
            .unwrap();
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].user, 606);
        let retried = settings
            .claim_pending_strict_actions(StrictQueueClass::Retry, now + 1, now + 61, 64)
            .await
            .unwrap();
        assert_eq!(retried.len(), 1);
        assert_eq!(retried[0].user, 605);
        clean(&settings, chat).await;
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL strict state failure test"]
    async fn closed_pool_does_not_look_like_an_unpunished_violation() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        settings.pool.close().await;
        assert!(
            settings
                .increment_strict(violation(-1, 1, 100, 2, StrictAction::Mute, None))
                .await
                .is_err()
        );
    }
}
