
use super::{Result, Settings};

pub const MAX_PENDING_CAPTCHAS_PER_CHAT: i64 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaFailureAction {
    Kick,
    Mute,
}

impl CaptchaFailureAction {
    fn as_db(self) -> &'static str {
        match self {
            Self::Kick => "kick",
            Self::Mute => "mute",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "kick" => Ok(Self::Kick),
            "mute" => Ok(Self::Mute),
            value => Err(sqlx::Error::Protocol(format!(
                "invalid pending captcha failure action {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaPhase {
    Arming,
    Passing,
    Expiring,
}

impl CaptchaPhase {
    fn as_db(self) -> &'static str {
        match self {
            Self::Arming => "arming",
            Self::Passing => "passing",
            Self::Expiring => "expiring",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "arming" => Ok(Self::Arming),
            "passing" => Ok(Self::Passing),
            "expiring" => Ok(Self::Expiring),
            value => Err(sqlx::Error::Protocol(format!(
                "invalid leased pending captcha state {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptchaWork {
    pub chat: i64,
    pub user: i64,
    pub message_id: Option<i32>,
    pub due_at: i64,
    pub failure_action: CaptchaFailureAction,
    pub phase: CaptchaPhase,
    pub attempts: u32,
    pub restriction_until: Option<i32>,
    pub quarantined_at: Option<i64>,
    pub terminal_reason: Option<String>,
    pub source_message_id: i32,
    pub kick_until: Option<i32>,
    generation: i64,
    version: i64,
    lease_token: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptchaReservation {
    pub chat: i64,
    pub user: i64,
    pub restriction_until: i32,
    pub superseded_message: Option<i32>,
    generation: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptchaReservationOutcome {
    Reserved(CaptchaReservation),
    Resume(CaptchaReservation),
    Duplicate,
    CapacityReached,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptchaAnswerClaim {
    Missing,
    NotReady,
    Incorrect,
    Expired,
    AlreadyClaimed,
    Claimed(CaptchaWork),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptchaQueueClass {
    Fresh,
    Retry,
    Quarantine,
}

type CaptchaTuple = (
    i64,
    i64,
    Option<i32>,
    i64,
    String,
    String,
    i32,
    i64,
    i64,
    i64,
    Option<i32>,
    Option<i64>,
    Option<String>,
    i32,
    Option<i32>,
);

fn decode(row: CaptchaTuple) -> Result<CaptchaWork> {
    Ok(CaptchaWork {
        chat: row.0,
        user: row.1,
        message_id: row.2,
        due_at: row.3,
        failure_action: CaptchaFailureAction::from_db(&row.4)?,
        phase: CaptchaPhase::from_db(&row.5)?,
        attempts: u32::try_from(row.6)
            .map_err(|_| sqlx::Error::Protocol("negative captcha attempt count".to_owned()))?,
        generation: row.7,
        version: row.8,
        lease_token: row.9,
        restriction_until: row.10,
        quarantined_at: row.11,
        terminal_reason: row.12,
        source_message_id: row.13,
        kick_until: row.14,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptchaReservationInput {
    pub chat: i64,
    pub user: i64,
    pub answer: usize,
    pub source_message_id: i32,
    pub due_at: i64,
    pub retry_at: i64,
    pub restriction_until: i32,
    pub failure_action: CaptchaFailureAction,
}

impl Settings {
    pub async fn reserve_captcha(
        &self,
        input: CaptchaReservationInput,
    ) -> Result<CaptchaReservationOutcome> {
        let CaptchaReservationInput {
            chat,
            user,
            answer,
            source_message_id,
            due_at,
            retry_at,
            restriction_until,
            failure_action,
        } = input;
        let answer = i32::try_from(answer)
            .map_err(|_| sqlx::Error::Protocol("captcha answer index exceeds i32".to_owned()))?;
        let mut tx = self.pool.begin().await?;
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO pending_captchas
             (chat_id, user_id, answer, source_message_id, due_at, retry_at, restriction_until,
              failure_action, state)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'arming')
             ON CONFLICT (chat_id, user_id) DO NOTHING
             RETURNING generation",
        )
        .bind(chat)
        .bind(user)
        .bind(answer)
        .bind(source_message_id)
        .bind(due_at)
        .bind(retry_at)
        .bind(restriction_until)
        .bind(failure_action.as_db())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(generation) = inserted {
            let shard_rows: i64 = sqlx::query_scalar(
                "SELECT captcha_rows FROM durable_counts WHERE id = 0 FOR UPDATE",
            )
            .fetch_one(&mut *tx)
            .await?;
            let chat_rows: i64 =
                sqlx::query_scalar("SELECT count(*) FROM pending_captchas WHERE chat_id = $1")
                    .bind(chat)
                    .fetch_one(&mut *tx)
                    .await?;
            let max_shard = self.max_pending_captcha_rows.unwrap_or(i64::MAX / 2);
            if chat_rows > MAX_PENDING_CAPTCHAS_PER_CHAT || shard_rows >= max_shard {
                tx.rollback().await?;
                return Ok(CaptchaReservationOutcome::CapacityReached);
            }
            sqlx::query("UPDATE durable_counts SET captcha_rows = captcha_rows + 1 WHERE id = 0")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(CaptchaReservationOutcome::Reserved(CaptchaReservation {
                chat,
                user,
                restriction_until,
                superseded_message: None,
                generation,
            }));
        }

        let previous: (i32, Option<i32>, String) = sqlx::query_as(
            "SELECT source_message_id, message_id, state FROM pending_captchas
             WHERE chat_id = $1 AND user_id = $2 FOR UPDATE",
        )
        .bind(chat)
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
        if previous.0 > source_message_id
            || (previous.0 == source_message_id && previous.2 != "arming")
        {
            tx.commit().await?;
            return Ok(CaptchaReservationOutcome::Duplicate);
        }
        let resumes_interrupted_setup = previous.0 == source_message_id;
        let generation: Option<i64> = sqlx::query_scalar(
            "UPDATE pending_captchas SET
                 answer = $3,
                 source_message_id = $4,
                 message_id = NULL,
                 due_at = $5,
                 retry_at = $6,
                 restriction_until = $7,
                 failure_action = $8,
                 state = 'arming',
                 attempts = 0,
                 generation = nextval('durable_work_token_seq'),
                 version = version + 1,
                 lease_token = NULL,
                 quarantined_at = NULL,
                 terminal_reason = NULL,
                 kick_until = NULL
             WHERE chat_id = $1 AND user_id = $2
               AND (source_message_id < $4 OR (source_message_id = $4 AND state = 'arming'))
             RETURNING generation",
        )
        .bind(chat)
        .bind(user)
        .bind(answer)
        .bind(source_message_id)
        .bind(due_at)
        .bind(retry_at)
        .bind(restriction_until)
        .bind(failure_action.as_db())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        let generation = generation.ok_or_else(|| {
            sqlx::Error::Protocol(
                "newer captcha lifecycle disappeared while holding its row lock".to_owned(),
            )
        })?;
        let reservation = CaptchaReservation {
            chat,
            user,
            restriction_until,
            superseded_message: previous.1,
            generation,
        };
        Ok(if resumes_interrupted_setup {
            CaptchaReservationOutcome::Resume(reservation)
        } else {
            CaptchaReservationOutcome::Reserved(reservation)
        })
    }

    pub async fn interrupted_captcha_users(
        &self,
        chat: i64,
        source_message_id: i32,
    ) -> Result<Vec<i64>> {
        sqlx::query_scalar(
            "SELECT user_id FROM pending_captchas
             WHERE chat_id = $1 AND source_message_id = $2 AND state = 'arming'
             ORDER BY user_id",
        )
        .bind(chat)
        .bind(source_message_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn attach_captcha_message(
        &self,
        reservation: &CaptchaReservation,
        message: i32,
        due_at: i64,
    ) -> Result<bool> {
        let updated = sqlx::query(
            "UPDATE pending_captchas SET message_id = $4, due_at = $5
             WHERE chat_id = $1 AND user_id = $2 AND generation = $3
               AND state = 'arming' AND lease_token IS NULL",
        )
        .bind(reservation.chat)
        .bind(reservation.user)
        .bind(reservation.generation)
        .bind(message)
        .bind(due_at)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn activate_captcha(
        &self,
        reservation: &CaptchaReservation,
        message: i32,
    ) -> Result<bool> {
        let updated = sqlx::query(
            "UPDATE pending_captchas SET state = 'pending', retry_at = due_at
             WHERE chat_id = $1 AND user_id = $2 AND generation = $3
               AND state = 'arming' AND message_id = $4 AND lease_token IS NULL",
        )
        .bind(reservation.chat)
        .bind(reservation.user)
        .bind(reservation.generation)
        .bind(message)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn abandon_captcha(&self, reservation: &CaptchaReservation) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let deleted = sqlx::query(
            "DELETE FROM pending_captchas
             WHERE chat_id = $1 AND user_id = $2 AND generation = $3
               AND state IN ('arming', 'pending') AND lease_token IS NULL",
        )
        .bind(reservation.chat)
        .bind(reservation.user)
        .bind(reservation.generation)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if deleted {
            sqlx::query(
                "UPDATE durable_counts SET captcha_rows = GREATEST(captcha_rows - 1, 0)
                 WHERE id = 0",
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(deleted)
    }

    pub async fn captcha_reservation_is_current(
        &self,
        reservation: &CaptchaReservation,
    ) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM pending_captchas
                 WHERE chat_id = $1 AND user_id = $2 AND generation = $3
                   AND state IN ('arming', 'pending') AND lease_token IS NULL
                   AND restriction_until = $4
             )",
        )
        .bind(reservation.chat)
        .bind(reservation.user)
        .bind(reservation.generation)
        .bind(reservation.restriction_until)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn captcha_work_is_current(&self, work: &CaptchaWork) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM pending_captchas
                 WHERE chat_id = $1 AND user_id = $2 AND state = $3
                   AND generation = $4 AND version = $5 AND lease_token = $6
                   AND attempts = $7
             )",
        )
        .bind(work.chat)
        .bind(work.user)
        .bind(work.phase.as_db())
        .bind(work.generation)
        .bind(work.version)
        .bind(work.lease_token)
        .bind(i64::from(work.attempts))
        .fetch_one(&self.pool)
        .await
    }

    pub async fn cancel_captcha_for_member_override(
        &self,
        chat: i64,
        user: i64,
    ) -> Result<Option<i32>> {
        let mut tx = self.pool.begin().await?;
        let removed: Option<Option<i32>> = sqlx::query_scalar(
            "DELETE FROM pending_captchas
             WHERE chat_id = $1 AND user_id = $2
             RETURNING message_id",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
        if removed.is_some() {
            sqlx::query(
                "UPDATE durable_counts SET captcha_rows = GREATEST(captcha_rows - 1, 0)
                 WHERE id = 0",
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(removed.flatten())
    }

    pub async fn claim_captcha_answer(
        &self,
        chat: i64,
        user: i64,
        answer: usize,
        expected_generation: Option<i64>,
        now: i64,
        lease_until: i64,
    ) -> Result<CaptchaAnswerClaim> {
        let answer = i32::try_from(answer)
            .map_err(|_| sqlx::Error::Protocol("captcha answer index exceeds i32".to_owned()))?;
        let mut tx = self.pool.begin().await?;
        let row: Option<(i32, i64, String, i64, i32)> = sqlx::query_as(
            "SELECT answer, due_at, state, generation, source_message_id FROM pending_captchas
             WHERE chat_id = $1 AND user_id = $2 FOR UPDATE",
        )
        .bind(chat)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
        let claim = match row {
            None => CaptchaAnswerClaim::Missing,
            Some((_, _, _, generation, _))
                if expected_generation.is_some_and(|expected| expected != generation) =>
            {
                CaptchaAnswerClaim::Missing
            }
            Some((_, _, _, _, source_message_id))
                if expected_generation.is_none() && source_message_id != 0 =>
            {
                CaptchaAnswerClaim::Missing
            }
            Some((_, _, state, _, _)) if state == "arming" => CaptchaAnswerClaim::NotReady,
            Some((_, _, state, _, _)) if state == "passing" => CaptchaAnswerClaim::AlreadyClaimed,
            Some((_, _, state, _, _)) if state == "expiring" => CaptchaAnswerClaim::Expired,
            Some((_, due_at, _, _, _)) if due_at <= now => CaptchaAnswerClaim::Expired,
            Some((expected, _, _, _, _)) if expected != answer => CaptchaAnswerClaim::Incorrect,
            Some(_) => {
                let row: CaptchaTuple = sqlx::query_as(
                    "UPDATE pending_captchas
                     SET state = 'passing', attempts = 1, retry_at = $3,
                         version = version + 1,
                         lease_token = nextval('durable_work_token_seq')
                     WHERE chat_id = $1 AND user_id = $2 AND state = 'pending'
                     RETURNING chat_id, user_id, message_id, due_at,
                               failure_action, state, attempts,
                               generation, version, lease_token, restriction_until,
                               quarantined_at, terminal_reason, source_message_id, kick_until",
                )
                .bind(chat)
                .bind(user)
                .bind(lease_until)
                .fetch_one(&mut *tx)
                .await?;
                CaptchaAnswerClaim::Claimed(decode(row)?)
            }
        };
        tx.commit().await?;
        Ok(claim)
    }

    pub async fn claim_due_captchas(
        &self,
        class: CaptchaQueueClass,
        now: i64,
        lease_until: i64,
        limit: i64,
    ) -> Result<Vec<CaptchaWork>> {
        let (predicate, order) = match class {
            CaptchaQueueClass::Fresh => (
                "state = 'pending' AND due_at <= $1",
                "due_at, chat_id, user_id",
            ),
            CaptchaQueueClass::Retry => (
                "state <> 'pending' AND quarantined_at IS NULL AND retry_at <= $1",
                "retry_at, attempts, due_at, chat_id, user_id",
            ),
            CaptchaQueueClass::Quarantine => (
                "quarantined_at IS NOT NULL AND retry_at <= $1",
                "retry_at, attempts, due_at, chat_id, user_id",
            ),
        };
        let statement = format!(
            "WITH due AS (
                 SELECT chat_id, user_id FROM pending_captchas
                 WHERE {predicate}
                 ORDER BY {order}
                 FOR UPDATE SKIP LOCKED
                 LIMIT $3
             )
             UPDATE pending_captchas AS captcha
             SET state = CASE WHEN captcha.state = 'pending' THEN 'expiring' ELSE captcha.state END,
                 attempts = captcha.attempts + 1,
                 retry_at = $2,
                 version = captcha.version + 1,
                 lease_token = nextval('durable_work_token_seq')
             FROM due
             WHERE captcha.chat_id = due.chat_id AND captcha.user_id = due.user_id
             RETURNING captcha.chat_id, captcha.user_id, captcha.message_id, captcha.due_at,
                       captcha.failure_action, captcha.state, captcha.attempts,
                       captcha.generation, captcha.version, captcha.lease_token,
                       captcha.restriction_until, captcha.quarantined_at,
                       captcha.terminal_reason, captcha.source_message_id, captcha.kick_until"
        );
        let rows: Vec<CaptchaTuple> = sqlx::query_as(&statement)
            .bind(now)
            .bind(lease_until)
            .bind(limit.clamp(1, 64))
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(decode).collect()
    }

    pub async fn prepare_captcha_kick(&self, work: &CaptchaWork, kick_until: i32) -> Result<bool> {
        let updated = sqlx::query(
            "UPDATE pending_captchas SET kick_until = $8
             WHERE chat_id = $1 AND user_id = $2 AND state = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7",
        )
        .bind(work.chat)
        .bind(work.user)
        .bind(work.phase.as_db())
        .bind(work.generation)
        .bind(work.version)
        .bind(work.lease_token)
        .bind(i64::from(work.attempts))
        .bind(kick_until)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn defer_captcha(&self, work: &CaptchaWork, retry_at: i64) -> Result<bool> {
        let updated = sqlx::query(
            "UPDATE pending_captchas SET retry_at = $8, lease_token = NULL
             WHERE chat_id = $1 AND user_id = $2 AND state = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7",
        )
        .bind(work.chat)
        .bind(work.user)
        .bind(work.phase.as_db())
        .bind(work.generation)
        .bind(work.version)
        .bind(work.lease_token)
        .bind(i64::from(work.attempts))
        .bind(retry_at)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn quarantine_captcha(
        &self,
        work: &CaptchaWork,
        retry_at: i64,
        now: i64,
        reason: &str,
    ) -> Result<bool> {
        let updated = sqlx::query(
            "UPDATE pending_captchas
             SET retry_at = $8, lease_token = NULL,
                 quarantined_at = COALESCE(quarantined_at, $9), terminal_reason = $10
             WHERE chat_id = $1 AND user_id = $2 AND state = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7",
        )
        .bind(work.chat)
        .bind(work.user)
        .bind(work.phase.as_db())
        .bind(work.generation)
        .bind(work.version)
        .bind(work.lease_token)
        .bind(i64::from(work.attempts))
        .bind(retry_at)
        .bind(now)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn finish_captcha(&self, work: &CaptchaWork) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let deleted = sqlx::query(
            "DELETE FROM pending_captchas
             WHERE chat_id = $1 AND user_id = $2 AND state = $3
               AND generation = $4 AND version = $5 AND lease_token = $6
               AND attempts = $7",
        )
        .bind(work.chat)
        .bind(work.user)
        .bind(work.phase.as_db())
        .bind(work.generation)
        .bind(work.version)
        .bind(work.lease_token)
        .bind(i64::from(work.attempts))
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if deleted {
            sqlx::query(
                "UPDATE durable_counts SET captcha_rows = GREATEST(captcha_rows - 1, 0)
                 WHERE id = 0",
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(deleted)
    }
}

impl CaptchaWork {
    pub fn workflow_key(&self) -> String {
        format!("captcha-failure:{}:{}", self.user, self.generation)
    }
}

impl CaptchaReservation {
    pub fn callback_generation(&self) -> i64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn admit_fixture_chat(settings: &Settings, chat: i64) {
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1, 1, 0) ON CONFLICT DO NOTHING",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
    }

    async fn clear_fixture_captchas(settings: &Settings, chat: i64) {
        let mut tx = settings.pool.begin().await.unwrap();
        let removed = sqlx::query("DELETE FROM pending_captchas WHERE chat_id = $1")
            .bind(chat)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        let removed = i64::try_from(removed).unwrap();
        sqlx::query(
            "UPDATE durable_counts
             SET captcha_rows = GREATEST(captcha_rows - $1, 0) WHERE id = 0",
        )
        .bind(removed)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    fn reservation(chat: i64, user: i64, source_message_id: i32) -> CaptchaReservationInput {
        CaptchaReservationInput {
            chat,
            user,
            answer: 2,
            source_message_id,
            due_at: 200,
            retry_at: 100,
            restriction_until: 500,
            failure_action: CaptchaFailureAction::Kick,
        }
    }

    fn expect_reserved(outcome: CaptchaReservationOutcome, context: &str) -> CaptchaReservation {
        match outcome {
            CaptchaReservationOutcome::Reserved(reservation) => reservation,
            other => panic!("{context}: got {other:?}"),
        }
    }

    fn expect_resumed(outcome: CaptchaReservationOutcome, context: &str) -> CaptchaReservation {
        match outcome {
            CaptchaReservationOutcome::Resume(reservation) => reservation,
            other => panic!("{context}: got {other:?}"),
        }
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL captcha capacity/concurrency probe"]
    async fn concurrent_first_reservations_charge_once_and_per_chat_capacity_is_exact() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let race_chat = -1_900_000_000_779_i64;
        let cap_chat = -1_900_000_000_780_i64;
        for chat in [race_chat, cap_chat] {
            admit_fixture_chat(&settings, chat).await;
            clear_fixture_captchas(&settings, chat).await;
        }
        let before_meta: i64 =
            sqlx::query_scalar("SELECT captcha_rows FROM durable_counts WHERE id = 0")
                .fetch_one(&settings.pool)
                .await
                .unwrap();
        let before_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM pending_captchas")
            .fetch_one(&settings.pool)
            .await
            .unwrap();

        let (first, second) = tokio::join!(
            settings.reserve_captcha(reservation(race_chat, 700_001, 1)),
            settings.reserve_captcha(reservation(race_chat, 700_001, 1)),
        );
        let outcomes = [first.unwrap(), second.unwrap()];
        let admitted = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, CaptchaReservationOutcome::Reserved(_)))
            .count();
        let resumed = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, CaptchaReservationOutcome::Resume(_)))
            .count();
        assert_eq!(admitted, 1, "the primary key admits one first lifecycle");
        assert_eq!(
            resumed, 1,
            "the concurrent exact replay renews the one non-actionable lifecycle"
        );
        let after_meta: i64 =
            sqlx::query_scalar("SELECT captcha_rows FROM durable_counts WHERE id = 0")
                .fetch_one(&settings.pool)
                .await
                .unwrap();
        let after_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM pending_captchas")
            .fetch_one(&settings.pool)
            .await
            .unwrap();
        assert_eq!(after_meta - before_meta, 1);
        assert_eq!(after_rows - before_rows, 1);
        assert_eq!(after_meta - before_meta, after_rows - before_rows);

        let before_replacement = after_meta;
        assert!(matches!(
            settings
                .reserve_captcha(reservation(race_chat, 700_001, 2))
                .await
                .unwrap(),
            CaptchaReservationOutcome::Reserved(_)
        ));
        let after_replacement: i64 =
            sqlx::query_scalar("SELECT captcha_rows FROM durable_counts WHERE id = 0")
                .fetch_one(&settings.pool)
                .await
                .unwrap();
        assert_eq!(after_replacement, before_replacement);

        for offset in 0..MAX_PENDING_CAPTCHAS_PER_CHAT {
            assert!(matches!(
                settings
                    .reserve_captcha(reservation(cap_chat, 800_000 + offset, 1))
                    .await
                    .unwrap(),
                CaptchaReservationOutcome::Reserved(_)
            ));
        }
        assert_eq!(
            settings
                .reserve_captcha(reservation(cap_chat, 900_000, 1))
                .await
                .unwrap(),
            CaptchaReservationOutcome::CapacityReached,
            "the first row beyond the per-chat ceiling must not remain inserted"
        );
        let cap_rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pending_captchas WHERE chat_id = $1")
                .bind(cap_chat)
                .fetch_one(&settings.pool)
                .await
                .unwrap();
        assert_eq!(cap_rows, MAX_PENDING_CAPTCHAS_PER_CHAT);

        clear_fixture_captchas(&settings, race_chat).await;
        clear_fixture_captchas(&settings, cap_chat).await;
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL captcha lifecycle probe"]
    async fn answer_and_expiry_are_mutually_exclusive_and_recoverable() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chat = -1_900_000_000_777_i64;
        let users = [
            900_000_001_i64,
            900_000_002,
            900_000_003,
            900_000_004,
            900_000_005,
            900_000_006,
        ];
        sqlx::query("DELETE FROM pending_captchas WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chat(&settings, chat).await;

        let first_reservation = expect_reserved(
            settings
                .reserve_captcha(CaptchaReservationInput {
                    chat,
                    user: users[0],
                    answer: 2,
                    source_message_id: 10,
                    due_at: 200,
                    retry_at: 100,
                    restriction_until: 500,
                    failure_action: CaptchaFailureAction::Kick,
                })
                .await
                .unwrap(),
            "first lifecycle is reserved",
        );
        assert_eq!(
            settings
                .claim_captcha_answer(
                    chat,
                    users[0],
                    2,
                    Some(first_reservation.callback_generation()),
                    10,
                    20,
                )
                .await
                .unwrap(),
            CaptchaAnswerClaim::NotReady
        );
        assert!(
            settings
                .attach_captcha_message(&first_reservation, 55, 200)
                .await
                .unwrap()
        );
        assert!(
            settings
                .activate_captcha(&first_reservation, 55)
                .await
                .unwrap()
        );
        assert_eq!(
            settings
                .reserve_captcha(CaptchaReservationInput {
                    chat,
                    user: users[0],
                    answer: 4,
                    source_message_id: 10,
                    due_at: 300,
                    retry_at: 100,
                    restriction_until: 600,
                    failure_action: CaptchaFailureAction::Mute,
                })
                .await
                .unwrap(),
            CaptchaReservationOutcome::Duplicate,
            "an exact replay must not replace an activated challenge's answer"
        );
        assert_eq!(
            settings
                .claim_captcha_answer(
                    chat,
                    users[0],
                    1,
                    Some(first_reservation.callback_generation()),
                    10,
                    20,
                )
                .await
                .unwrap(),
            CaptchaAnswerClaim::Incorrect
        );
        let (first, second) = tokio::join!(
            settings.claim_captcha_answer(
                chat,
                users[0],
                2,
                Some(first_reservation.callback_generation()),
                10,
                20,
            ),
            settings.claim_captcha_answer(
                chat,
                users[0],
                2,
                Some(first_reservation.callback_generation()),
                10,
                20,
            ),
        );
        let (first, second) = (first.unwrap(), second.unwrap());
        let (CaptchaAnswerClaim::Claimed(passing), CaptchaAnswerClaim::AlreadyClaimed) =
            (if matches!(&first, CaptchaAnswerClaim::Claimed(_)) {
                (first, second)
            } else {
                (second, first)
            })
        else {
            panic!("concurrent correct answers were not serialized");
        };
        assert_eq!(passing.phase, CaptchaPhase::Passing);
        assert_eq!(passing.message_id, Some(55));
        assert_eq!(passing.restriction_until, Some(500));
        assert!(settings.captcha_work_is_current(&passing).await.unwrap());
        let mut reclaimed = settings
            .claim_due_captchas(CaptchaQueueClass::Retry, 21, 40, 1)
            .await
            .unwrap();
        let current = reclaimed
            .pop()
            .expect("expired passing lease is recoverable");
        assert_eq!(current.user, users[0]);
        assert_eq!(current.phase, CaptchaPhase::Passing);
        assert!(
            !settings.finish_captcha(&passing).await.unwrap(),
            "a stale answer worker cannot delete a reclaimed lease"
        );
        assert!(
            !settings.defer_captcha(&passing, 500).await.unwrap(),
            "a stale answer worker cannot postpone a reclaimed lease"
        );

        let replacement_reservation = expect_reserved(
            settings
                .reserve_captcha(CaptchaReservationInput {
                    chat,
                    user: users[0],
                    answer: 1,
                    source_message_id: 11,
                    due_at: 500,
                    retry_at: 100,
                    restriction_until: 700,
                    failure_action: CaptchaFailureAction::Kick,
                })
                .await
                .unwrap(),
            "replacement lifecycle is reserved",
        );
        assert_eq!(replacement_reservation.superseded_message, Some(55));
        assert!(
            !settings.captcha_work_is_current(&current).await.unwrap(),
            "the old crash-recovery lease must not own the rejoined membership"
        );
        assert_eq!(
            settings
                .claim_captcha_answer(
                    chat,
                    users[0],
                    2,
                    Some(first_reservation.callback_generation()),
                    30,
                    60,
                )
                .await
                .unwrap(),
            CaptchaAnswerClaim::Missing,
            "an old challenge button cannot answer a replacement lifecycle"
        );
        assert!(
            !settings
                .attach_captcha_message(&first_reservation, 99, 999)
                .await
                .unwrap(),
            "a stale setup cannot attach its message to a replacement lifecycle"
        );
        assert!(
            !settings.abandon_captcha(&first_reservation).await.unwrap(),
            "a stale setup cannot delete a replacement lifecycle"
        );
        settings
            .attach_captcha_message(&replacement_reservation, 57, 500)
            .await
            .unwrap();
        settings
            .activate_captcha(&replacement_reservation, 57)
            .await
            .unwrap();
        let CaptchaAnswerClaim::Claimed(replacement) = settings
            .claim_captcha_answer(
                chat,
                users[0],
                1,
                Some(replacement_reservation.callback_generation()),
                30,
                60,
            )
            .await
            .unwrap()
        else {
            panic!("replacement lifecycle was not claimable");
        };
        assert!(
            !settings.finish_captcha(&current).await.unwrap(),
            "an old lifecycle cannot delete a replacement row in the same phase"
        );
        assert!(settings.finish_captcha(&replacement).await.unwrap());

        let override_reservation = expect_reserved(
            settings
                .reserve_captcha(CaptchaReservationInput {
                    chat,
                    user: users[0],
                    answer: 3,
                    source_message_id: 12,
                    due_at: 600,
                    retry_at: 500,
                    restriction_until: 900,
                    failure_action: CaptchaFailureAction::Kick,
                })
                .await
                .unwrap(),
            "bot override lifecycle is reserved",
        );
        settings
            .attach_captcha_message(&override_reservation, 59, 600)
            .await
            .unwrap();
        settings
            .activate_captcha(&override_reservation, 59)
            .await
            .unwrap();
        assert_eq!(
            settings
                .cancel_captcha_for_member_override(chat, users[0])
                .await
                .unwrap(),
            Some(59)
        );
        assert_eq!(
            settings
                .claim_captcha_answer(
                    chat,
                    users[0],
                    3,
                    Some(override_reservation.callback_generation()),
                    40,
                    60,
                )
                .await
                .unwrap(),
            CaptchaAnswerClaim::Missing,
            "a committed bot override must fence every late callback and worker"
        );

        let interrupted = expect_reserved(
            settings
                .reserve_captcha(reservation(chat, users[4], 50))
                .await
                .unwrap(),
            "interrupted sibling is reserved",
        );
        let active_sibling = expect_reserved(
            settings
                .reserve_captcha(reservation(chat, users[5], 50))
                .await
                .unwrap(),
            "active sibling is reserved",
        );
        assert!(
            settings
                .attach_captcha_message(&active_sibling, 65, 200)
                .await
                .unwrap()
        );
        assert!(
            settings
                .activate_captcha(&active_sibling, 65)
                .await
                .unwrap()
        );
        assert_eq!(
            settings.interrupted_captcha_users(chat, 50).await.unwrap(),
            vec![users[4]],
            "an active sibling from the same service message is not replay work"
        );
        assert!(
            settings
                .interrupted_captcha_users(chat, 49)
                .await
                .unwrap()
                .is_empty()
        );
        let resumed = expect_resumed(
            settings
                .reserve_captcha(CaptchaReservationInput {
                    answer: 4,
                    ..reservation(chat, users[4], 50)
                })
                .await
                .unwrap(),
            "exact arming replay is renewed",
        );
        assert_ne!(
            interrupted.callback_generation(),
            resumed.callback_generation(),
            "recovery fences a setup attempt that survived the process crash"
        );
        assert!(
            !settings
                .attach_captcha_message(&interrupted, 66, 200)
                .await
                .unwrap(),
            "the pre-crash generation cannot attach after recovery"
        );
        assert!(
            settings
                .attach_captcha_message(&resumed, 67, 200)
                .await
                .unwrap()
        );
        assert!(settings.activate_captcha(&resumed, 67).await.unwrap());
        assert!(
            settings
                .interrupted_captcha_users(chat, 50)
                .await
                .unwrap()
                .is_empty(),
            "activation closes exact stale-replay intent for every sibling"
        );
        assert_eq!(
            settings
                .reserve_captcha(reservation(chat, users[4], 50))
                .await
                .unwrap(),
            CaptchaReservationOutcome::Duplicate,
            "an activated recovery is idempotent under another exact replay"
        );
        assert_eq!(
            settings
                .cancel_captcha_for_member_override(chat, users[4])
                .await
                .unwrap(),
            Some(67)
        );
        assert_eq!(
            settings
                .cancel_captcha_for_member_override(chat, users[5])
                .await
                .unwrap(),
            Some(65)
        );

        let expiry_reservation = expect_reserved(
            settings
                .reserve_captcha(CaptchaReservationInput {
                    chat,
                    user: users[1],
                    answer: 0,
                    source_message_id: 20,
                    due_at: 5,
                    retry_at: 100,
                    restriction_until: 800,
                    failure_action: CaptchaFailureAction::Kick,
                })
                .await
                .unwrap(),
            "expiry lifecycle is reserved",
        );
        settings
            .attach_captcha_message(&expiry_reservation, 56, 5)
            .await
            .unwrap();
        settings
            .activate_captcha(&expiry_reservation, 56)
            .await
            .unwrap();
        expect_reserved(
            settings
                .reserve_captcha(CaptchaReservationInput {
                    chat,
                    user: users[2],
                    answer: 0,
                    source_message_id: 30,
                    due_at: 500,
                    retry_at: 5,
                    restriction_until: 900,
                    failure_action: CaptchaFailureAction::Mute,
                })
                .await
                .unwrap(),
            "arming lifecycle is reserved",
        );

        let mut due = settings
            .claim_due_captchas(CaptchaQueueClass::Fresh, 10, 40, 10)
            .await
            .unwrap();
        due.extend(
            settings
                .claim_due_captchas(CaptchaQueueClass::Retry, 10, 40, 10)
                .await
                .unwrap(),
        );
        due.sort_by_key(|work| work.user);
        assert_eq!(due.len(), 2);
        assert_eq!(due[0].user, users[1]);
        assert_eq!(due[0].phase, CaptchaPhase::Expiring);
        assert_eq!(due[1].user, users[2]);
        assert_eq!(due[1].phase, CaptchaPhase::Arming);
        assert!(due.iter().all(|work| work.attempts == 1));

        let expiry = due.remove(0);
        let arming = due.remove(0);
        assert!(
            settings.prepare_captcha_kick(&expiry, 71).await.unwrap(),
            "the kick intermediate must be durable before its first Telegram request"
        );
        assert!(
            settings
                .quarantine_captcha(&expiry, 1_000, 10, "transport outcome unknown")
                .await
                .unwrap()
        );
        assert!(
            !settings.finish_captcha(&expiry).await.unwrap(),
            "quarantine must release, not acknowledge, the exact lease"
        );
        assert!(settings.finish_captcha(&arming).await.unwrap());
        assert!(
            settings
                .claim_due_captchas(CaptchaQueueClass::Quarantine, 999, 2_000, 10)
                .await
                .unwrap()
                .is_empty()
        );

        let fresh_reservation = expect_reserved(
            settings
                .reserve_captcha(CaptchaReservationInput {
                    chat,
                    user: users[3],
                    answer: 0,
                    source_message_id: 40,
                    due_at: 1_000,
                    retry_at: 2_000,
                    restriction_until: 1_500,
                    failure_action: CaptchaFailureAction::Kick,
                })
                .await
                .unwrap(),
            "fresh lifecycle is reserved",
        );
        settings
            .attach_captcha_message(&fresh_reservation, 58, 1_000)
            .await
            .unwrap();
        settings
            .activate_captcha(&fresh_reservation, 58)
            .await
            .unwrap();
        let fresh = settings
            .claim_due_captchas(CaptchaQueueClass::Fresh, 1_000, 2_000, 1)
            .await
            .unwrap()
            .pop()
            .expect("fresh due work is claimable");
        assert_eq!(
            fresh.user, users[3],
            "quarantined retries must not starve fresh captcha work"
        );
        assert!(settings.finish_captcha(&fresh).await.unwrap());

        let quarantined = settings
            .claim_due_captchas(CaptchaQueueClass::Quarantine, 1_000, 2_000, 1)
            .await
            .unwrap()
            .pop()
            .expect("quarantined work remains recoverable");
        assert_eq!(quarantined.user, users[1]);
        assert_eq!(quarantined.kick_until, Some(71));
        assert_eq!(quarantined.quarantined_at, Some(10));
        assert_eq!(
            quarantined.terminal_reason.as_deref(),
            Some("transport outcome unknown")
        );
        assert!(settings.finish_captcha(&quarantined).await.unwrap());

        sqlx::query("DELETE FROM pending_captchas WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chat(&settings, chat).await;
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL captcha queue fairness probe"]
    async fn queue_classes_progress_beyond_one_page_under_continuous_fresh_work() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chat = -1_900_000_000_778_i64;
        let now = 10_000_i64;
        sqlx::query("DELETE FROM pending_captchas WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chat(&settings, chat).await;

        for offset in 0..12_i64 {
            sqlx::query(
                "INSERT INTO pending_captchas
                 (chat_id, user_id, answer, source_message_id, message_id, due_at, retry_at,
                  restriction_until, failure_action, state, attempts, quarantined_at)
                 VALUES ($1, $2, 0, $3, $3, $4, $4, $5, 'kick', 'pending', 0, NULL)",
            )
            .bind(chat)
            .bind(10_000 + offset)
            .bind(i32::try_from(100 + offset).unwrap())
            .bind(now - 100 + offset)
            .bind(20_000_i32)
            .execute(&settings.pool)
            .await
            .unwrap();
        }
        for offset in 0..8_i64 {
            sqlx::query(
                "INSERT INTO pending_captchas
                 (chat_id, user_id, answer, source_message_id, message_id, due_at, retry_at,
                  restriction_until, failure_action, state, attempts, quarantined_at)
                 VALUES ($1, $2, 0, $3, NULL, $4, $4, $5, 'kick', 'arming', 1, NULL)",
            )
            .bind(chat)
            .bind(20_000 + offset)
            .bind(i32::try_from(200 + offset).unwrap())
            .bind(now - 200 + offset)
            .bind(20_000_i32)
            .execute(&settings.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO pending_captchas
                 (chat_id, user_id, answer, source_message_id, message_id, due_at, retry_at,
                  restriction_until, failure_action, state, attempts, quarantined_at)
                 VALUES ($1, $2, 0, $3, $3, $4, $4, $5, 'kick', 'expiring', 9, $6)",
            )
            .bind(chat)
            .bind(30_000 + offset)
            .bind(i32::try_from(300 + offset).unwrap())
            .bind(now - 300 + offset)
            .bind(20_000_i32)
            .bind(now - 3_600)
            .execute(&settings.pool)
            .await
            .unwrap();
        }

        let fresh = settings
            .claim_due_captchas(CaptchaQueueClass::Fresh, now, now + 60, 4)
            .await
            .unwrap();
        assert_eq!(fresh.len(), 4);
        let retry = settings
            .claim_due_captchas(CaptchaQueueClass::Retry, now, now + 60, 4)
            .await
            .unwrap();
        let quarantine = settings
            .claim_due_captchas(CaptchaQueueClass::Quarantine, now, now + 60, 4)
            .await
            .unwrap();
        assert_eq!(retry.len(), 4);
        assert_eq!(quarantine.len(), 4);
        assert!(retry.iter().all(|work| work.quarantined_at.is_none()));
        assert!(quarantine.iter().all(|work| work.quarantined_at.is_some()));

        sqlx::query("DELETE FROM pending_captchas WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }
}
