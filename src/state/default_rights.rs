use std::sync::atomic::Ordering;

use sqlx::{PgConnection, Postgres, Row, Transaction};

use super::{MirrorCommitGuard, Settings};

pub const RIGHT_COUNT: u8 = 14;
pub const ALL_RIGHTS: RightsMask = RightsMask((1_u16 << RIGHT_COUNT) - 1);

const FORCE_FINGERPRINT: i32 = 1 << RIGHT_COUNT;
const LEASE_SECONDS: i64 = 120;
const RETRY_MIN_SECONDS: i64 = 5;
const RETRY_MAX_SECONDS: i64 = 3_600;
const LAST_ERROR_BYTES: usize = 1_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RightsMask(u16);

impl RightsMask {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn from_storage(value: i32) -> Result<Self, DefaultRightsError> {
        let value = u16::try_from(value).map_err(|_| {
            DefaultRightsError::InvalidState(format!(
                "rights mask {value} is negative or too large"
            ))
        })?;
        if value & !ALL_RIGHTS.0 != 0 {
            return Err(DefaultRightsError::InvalidState(format!(
                "rights mask {value} contains unknown bits"
            )));
        }
        Ok(Self(value))
    }

    pub const fn storage(self) -> i32 {
        self.0 as i32
    }

    pub fn contains(self, bit: u8) -> bool {
        bit < RIGHT_COUNT && self.0 & (1_u16 << bit) != 0
    }

    fn set(&mut self, bit: u8, closed: bool) -> Result<(), DefaultRightsError> {
        if bit >= RIGHT_COUNT {
            return Err(DefaultRightsError::UnknownRight(bit));
        }
        if closed {
            self.0 |= 1_u16 << bit;
        } else {
            self.0 &= !(1_u16 << bit);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NightWindow {
    pub from: u16,
    pub to: u16,
}

impl NightWindow {
    pub fn new(from: u32, to: u32) -> Option<Self> {
        if from >= 1_440 || to >= 1_440 {
            return None;
        }
        let from = u16::try_from(from).ok()?;
        let to = u16::try_from(to).ok()?;
        (from != to).then_some(Self { from, to })
    }

    pub fn contains(self, minute: u16) -> bool {
        if self.from < self.to {
            (self.from..self.to).contains(&minute)
        } else {
            minute >= self.from || minute < self.to
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectiveRights {
    pub base: RightsMask,
    pub force_all: bool,
}

impl EffectiveRights {
    pub(crate) fn fingerprint(self) -> i32 {
        self.base.storage() | if self.force_all { FORCE_FINGERPRINT } else { 0 }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RightsSnapshot {
    pub chat: i64,
    pub base: RightsMask,
    pub seeded: bool,
    pub manual_lock: bool,
    pub timed_until: Option<i64>,
    pub night: Option<NightWindow>,
    pub intent_revision: i64,
    pub applied_revision: i64,
    pub applied_fingerprint: Option<i32>,
    pub remote_unknown: bool,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub retry_at: i64,
    pub next_transition_at: Option<i64>,
}

impl RightsSnapshot {
    pub fn effective_at(&self, now: i64) -> EffectiveRights {
        let minute = u16::try_from(now.rem_euclid(86_400) / 60).unwrap_or(0);
        EffectiveRights {
            base: self.base,
            force_all: self.manual_lock
                || self.timed_until.is_some_and(|until| until > now)
                || self.night.is_some_and(|window| window.contains(minute)),
        }
    }

    pub fn pending(&self, now: i64) -> bool {
        self.seeded
            && (self.intent_revision != self.applied_revision
                || self.applied_fingerprint != Some(self.effective_at(now).fingerprint()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedRightsMutation {
    pub snapshot: RightsSnapshot,
    pub delivery_required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RightsDeliveryClaim {
    pub chat: i64,
    pub revision: i64,
    pub token: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RightsNoticeKind {
    TimedOpened,
    NightLocked,
    NightOpened,
}

impl RightsNoticeKind {
    fn as_storage(self) -> &'static str {
        match self {
            Self::TimedOpened => "timed_opened",
            Self::NightLocked => "night_locked",
            Self::NightOpened => "night_opened",
        }
    }

    fn from_storage(value: &str) -> Result<Self, DefaultRightsError> {
        match value {
            "timed_opened" => Ok(Self::TimedOpened),
            "night_locked" => Ok(Self::NightLocked),
            "night_opened" => Ok(Self::NightOpened),
            other => Err(DefaultRightsError::InvalidState(format!(
                "unknown rights notice kind {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RightsNoticeClaim {
    pub chat: i64,
    pub token: i64,
    pub revision: i64,
    pub kind: RightsNoticeKind,
}

#[derive(Debug)]
pub enum DefaultRightsError {
    Database(sqlx::Error),
    CommitUncertain(sqlx::Error),
    SeedRequired,
    UnknownRight(u8),
    InvalidState(String),
    CapacityReached,
    UncertainState,
}

impl std::fmt::Display for DefaultRightsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "default-rights database error: {error}"),
            Self::CommitUncertain(error) => {
                write!(
                    formatter,
                    "default-rights commit outcome is uncertain: {error}"
                )
            }
            Self::SeedRequired => write!(formatter, "group default rights have not been seeded"),
            Self::UnknownRight(bit) => write!(formatter, "unknown default-right bit {bit}"),
            Self::InvalidState(reason) => {
                write!(formatter, "invalid default-rights state: {reason}")
            }
            Self::CapacityReached => write!(formatter, "configured chat capacity reached"),
            Self::UncertainState => write!(formatter, "settings ownership is no longer valid"),
        }
    }
}

impl std::error::Error for DefaultRightsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) | Self::CommitUncertain(error) => Some(error),
            Self::SeedRequired
            | Self::UnknownRight(_)
            | Self::InvalidState(_)
            | Self::CapacityReached
            | Self::UncertainState => None,
        }
    }
}

impl From<sqlx::Error> for DefaultRightsError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl DefaultRightsError {
    pub fn acceptance_unknown(&self) -> bool {
        matches!(self, Self::CommitUncertain(_))
    }
}

#[derive(Debug)]
struct StoredRights {
    snapshot: RightsSnapshot,
    intent_fingerprint: i32,
    delivery_state: String,
    lease_token: Option<i64>,
    lease_revision: Option<i64>,
    lease_until: Option<i64>,
    intent_notice: Option<RightsNoticeKind>,
    notice_kind: Option<RightsNoticeKind>,
    notice_revision: Option<i64>,
    notice_token: Option<i64>,
    notice_lease_until: Option<i64>,
    notice_retry_at: i64,
    notice_attempts: i32,
    notice_error: Option<String>,
}

impl Settings {
    pub(super) async fn init_default_rights(conn: &mut PgConnection) -> sqlx::Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS default_rights_state (
                chat_id BIGINT PRIMARY KEY,
                base_mask INT NOT NULL DEFAULT 0 CHECK (base_mask BETWEEN 0 AND 16383),
                seeded BOOLEAN NOT NULL DEFAULT FALSE,
                manual_lock BOOLEAN NOT NULL DEFAULT FALSE,
                timed_until BIGINT,
                night_from INT CHECK (night_from BETWEEN 0 AND 1439),
                night_to INT CHECK (night_to BETWEEN 0 AND 1439),
                intent_revision BIGINT NOT NULL DEFAULT 0 CHECK (intent_revision >= 0),
                intent_fingerprint INT NOT NULL DEFAULT 0,
                applied_revision BIGINT NOT NULL DEFAULT 0 CHECK (applied_revision >= 0),
                applied_fingerprint INT,
                remote_unknown BOOLEAN NOT NULL DEFAULT FALSE,
                delivery_state TEXT NOT NULL DEFAULT 'applied'
                    CHECK (delivery_state IN ('applied', 'pending', 'leased')),
                lease_token BIGINT,
                lease_revision BIGINT,
                lease_until BIGINT,
                retry_at BIGINT NOT NULL DEFAULT 0,
                attempts INT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
                last_error TEXT,
                next_transition_at BIGINT,
                intent_notice TEXT CHECK (intent_notice IN
                    ('timed_opened', 'night_locked', 'night_opened')),
                notice_kind TEXT CHECK (notice_kind IN
                    ('timed_opened', 'night_locked', 'night_opened')),
                notice_revision BIGINT,
                notice_token BIGINT,
                notice_lease_until BIGINT,
                notice_retry_at BIGINT NOT NULL DEFAULT 0,
                notice_attempts INT NOT NULL DEFAULT 0 CHECK (notice_attempts >= 0),
                notice_error TEXT,
                CHECK ((night_from IS NULL) = (night_to IS NULL)),
                CHECK ((lease_token IS NULL) = (lease_revision IS NULL)),
                CHECK ((lease_token IS NULL) = (lease_until IS NULL)),
                CHECK ((notice_kind IS NULL) = (notice_revision IS NULL)),
                CHECK ((notice_token IS NULL) = (notice_lease_until IS NULL))
            )",
        )
        .execute(&mut *conn)
        .await?;
        for statement in [
            "ALTER TABLE default_rights_state ADD COLUMN IF NOT EXISTS remote_unknown BOOLEAN NOT NULL DEFAULT FALSE",
            "ALTER TABLE default_rights_state ADD COLUMN IF NOT EXISTS intent_notice TEXT",
            "ALTER TABLE default_rights_state ADD COLUMN IF NOT EXISTS notice_kind TEXT",
            "ALTER TABLE default_rights_state ADD COLUMN IF NOT EXISTS notice_revision BIGINT",
            "ALTER TABLE default_rights_state ADD COLUMN IF NOT EXISTS notice_token BIGINT",
            "ALTER TABLE default_rights_state ADD COLUMN IF NOT EXISTS notice_lease_until BIGINT",
            "ALTER TABLE default_rights_state ADD COLUMN IF NOT EXISTS notice_retry_at BIGINT NOT NULL DEFAULT 0",
            "ALTER TABLE default_rights_state ADD COLUMN IF NOT EXISTS notice_attempts INT NOT NULL DEFAULT 0",
            "ALTER TABLE default_rights_state ADD COLUMN IF NOT EXISTS notice_error TEXT",
        ] {
            sqlx::query(statement).execute(&mut *conn).await?;
        }
        if let Some((chat, base, intent, applied, from, to)) =
            sqlx::query_as::<_, (i64, i32, i32, Option<i32>, Option<i32>, Option<i32>)>(
                "SELECT chat_id, base_mask, intent_fingerprint, applied_fingerprint,
                    night_from, night_to
             FROM default_rights_state
             WHERE base_mask NOT BETWEEN 0 AND 16383
                OR intent_fingerprint NOT BETWEEN 0 AND 32767
                OR (applied_fingerprint IS NOT NULL
                    AND applied_fingerprint NOT BETWEEN 0 AND 32767)
                OR (night_from IS NULL) <> (night_to IS NULL)
                OR night_from NOT BETWEEN 0 AND 1439
                OR night_to NOT BETWEEN 0 AND 1439
                OR night_from = night_to
             ORDER BY chat_id LIMIT 1",
            )
            .fetch_optional(&mut *conn)
            .await?
        {
            return Err(sqlx::Error::Protocol(format!(
                "corrupt default-rights state for chat {chat}: base={base}, intent={intent}, applied={applied:?}, night={from:?}|{to:?}"
            )));
        }
        sqlx::query(
            "DO $constraint$
             BEGIN
               IF NOT EXISTS (SELECT 1 FROM pg_constraint
                              WHERE conrelid = 'default_rights_state'::regclass
                                AND conname = 'default_rights_masks_valid') THEN
                 ALTER TABLE default_rights_state ADD CONSTRAINT default_rights_masks_valid
                   CHECK (base_mask BETWEEN 0 AND 16383
                      AND intent_fingerprint BETWEEN 0 AND 32767
                      AND (applied_fingerprint IS NULL
                           OR applied_fingerprint BETWEEN 0 AND 32767));
               END IF;
               IF NOT EXISTS (SELECT 1 FROM pg_constraint
                              WHERE conrelid = 'default_rights_state'::regclass
                                AND conname = 'default_rights_night_valid') THEN
                 ALTER TABLE default_rights_state ADD CONSTRAINT default_rights_night_valid
                   CHECK ((night_from IS NULL AND night_to IS NULL)
                      OR (night_from BETWEEN 0 AND 1439
                          AND night_to BETWEEN 0 AND 1439
                          AND night_from <> night_to));
               END IF;
             END
             $constraint$",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS default_rights_retry_due
             ON default_rights_state (retry_at, chat_id)
             WHERE seeded AND (
                 intent_revision <> applied_revision
                 OR applied_fingerprint IS DISTINCT FROM intent_fingerprint
             )",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS default_rights_transition_due
             ON default_rights_state (next_transition_at, chat_id)
             WHERE seeded AND next_transition_at IS NOT NULL",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS default_rights_lease_due
             ON default_rights_state (lease_until, chat_id)
             WHERE lease_token IS NOT NULL",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS default_rights_notice_due
             ON default_rights_state (notice_retry_at, chat_id)
             WHERE notice_kind IS NOT NULL",
        )
        .execute(&mut *conn)
        .await?;

        sqlx::query(
            "WITH clock AS (
                SELECT floor(extract(epoch FROM clock_timestamp()))::BIGINT + 12600 AS now,
                       ((floor(extract(epoch FROM clock_timestamp()))::BIGINT + 12600)
                           % 86400) / 60 AS minute
             ), legacy AS (
                SELECT chat_id,
                       bit_or(CASE key
                           WHEN 'perm:plain' THEN 1
                           WHEN 'perm:photos' THEN 2
                           WHEN 'perm:videos' THEN 4
                           WHEN 'perm:rounds' THEN 8
                           WHEN 'perm:audios' THEN 16
                           WHEN 'perm:voices' THEN 32
                           WHEN 'perm:docs' THEN 64
                           WHEN 'perm:stickers' THEN 128
                           WHEN 'perm:polls' THEN 256
                           WHEN 'perm:links' THEN 512
                           WHEN 'perm:reactions' THEN 1024
                           WHEN 'perm:info' THEN 2048
                           WHEN 'perm:invite' THEN 4096
                           WHEN 'perm:pin' THEN 8192
                           ELSE 0 END)::INT AS base_mask,
                       bool_or(key = 'perm_seeded') AS perm_seeded,
                       max(CASE WHEN key = 'glock_until' AND btrim(value) ~ '^[0-9]+$'
                                THEN btrim(value)::BIGINT END) AS timed_until,
                       bool_or(key = 'glock_until' AND btrim(value) ~ '^[0-9]+$')
                           AS legacy_timed_locked,
                       max(CASE WHEN key = 'night' AND value ~ '^[0-9]+\\|[0-9]+$'
                                THEN split_part(value, '|', 1)::INT END) AS night_from,
                       max(CASE WHEN key = 'night' AND value ~ '^[0-9]+\\|[0-9]+$'
                                THEN split_part(value, '|', 2)::INT END) AS night_to,
                       bool_or(key = 'night_state'
                               AND value IN ('on', 'pending_on')) AS legacy_night_on,
                       bool_or(key = 'night_state'
                               AND value IN ('on', 'off', 'pending_on', 'pending_off'))
                           AS legacy_night_known
                FROM settings
                WHERE key LIKE 'perm:%' OR key IN
                    ('perm_seeded', 'glock_until', 'night', 'night_state')
                GROUP BY chat_id
             ), prepared AS (
                SELECT legacy.*, clock.now, clock.minute,
                       CASE WHEN night_from BETWEEN 0 AND 1439
                                  AND night_to BETWEEN 0 AND 1439
                                  AND night_from <> night_to THEN night_from END AS valid_from,
                       CASE WHEN night_from BETWEEN 0 AND 1439
                                  AND night_to BETWEEN 0 AND 1439
                                  AND night_from <> night_to THEN night_to END AS valid_to
                FROM legacy CROSS JOIN clock
             ), effective AS (
                SELECT prepared.*,
                       (perm_seeded OR timed_until IS NOT NULL OR valid_from IS NOT NULL)
                           AS authoritative,
                       (timed_until IS NOT NULL AND timed_until > now) AS timed_on,
                       CASE WHEN valid_from IS NULL THEN FALSE
                            WHEN valid_from < valid_to
                              THEN minute >= valid_from AND minute < valid_to
                            ELSE minute >= valid_from OR minute < valid_to END AS desired_night,
                       (SELECT min(at) FROM (VALUES
                           (CASE WHEN timed_until > now THEN timed_until END),
                           (CASE WHEN valid_from IS NOT NULL THEN
                               now - (now % 86400) + valid_from * 60
                               + CASE WHEN now - (now % 86400) + valid_from * 60 <= now
                                      THEN 86400 ELSE 0 END END),
                           (CASE WHEN valid_to IS NOT NULL THEN
                               now - (now % 86400) + valid_to * 60
                               + CASE WHEN now - (now % 86400) + valid_to * 60 <= now
                                      THEN 86400 ELSE 0 END END)
                       ) AS transitions(at) WHERE at IS NOT NULL) AS next_transition
                FROM prepared
             )
             INSERT INTO default_rights_state (
                 chat_id, base_mask, seeded, timed_until, night_from, night_to,
                 intent_revision, intent_fingerprint, applied_revision,
                 applied_fingerprint, remote_unknown, delivery_state, retry_at,
                 next_transition_at, intent_notice
             )
             SELECT chat_id, base_mask, authoritative, timed_until, valid_from, valid_to,
                    CASE WHEN authoritative THEN 1 ELSE 0 END,
                    base_mask | CASE WHEN timed_on OR desired_night THEN 16384 ELSE 0 END,
                    CASE WHEN authoritative
                                   AND (valid_from IS NULL OR legacy_night_known)
                                   AND (timed_on OR desired_night) =
                                       (legacy_timed_locked OR legacy_night_on)
                         THEN 1 ELSE 0 END,
                    CASE WHEN authoritative
                                   AND (valid_from IS NULL OR legacy_night_known) THEN
                         base_mask | CASE WHEN legacy_timed_locked OR legacy_night_on
                                          THEN 16384 ELSE 0 END
                         END,
                    (perm_seeded AND timed_until IS NULL AND valid_from IS NULL),
                    CASE WHEN authoritative
                                   AND (valid_from IS NULL OR legacy_night_known)
                                   AND (timed_on OR desired_night) =
                                       (legacy_timed_locked OR legacy_night_on)
                         THEN 'applied' ELSE 'pending' END,
                    CASE WHEN authoritative AND (
                                   valid_from IS NOT NULL AND NOT legacy_night_known
                                   OR (timed_on OR desired_night) <>
                                      (legacy_timed_locked OR legacy_night_on))
                         THEN now ELSE 0 END,
                    next_transition,
                    CASE
                      WHEN timed_until IS NOT NULL AND timed_until <= now
                           AND NOT (timed_on OR desired_night) THEN 'timed_opened'
                      WHEN valid_from IS NOT NULL AND legacy_night_known
                           AND desired_night <> legacy_night_on
                        THEN CASE WHEN desired_night THEN 'night_locked' ELSE 'night_opened' END
                    END
             FROM effective
             ON CONFLICT (chat_id) DO NOTHING",
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "DELETE FROM settings
             WHERE key LIKE 'perm:%' OR key IN
                 ('perm_seeded', 'glock_until', 'night', 'night_state')",
        )
        .execute(&mut *conn)
        .await?;
        for index in [
            "settings_night",
            "settings_group_lock_due",
            "settings_night_boundary",
        ] {
            sqlx::query(&format!("DROP INDEX IF EXISTS {index}"))
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }

    pub async fn default_rights(
        &self,
        chat: i64,
    ) -> Result<Option<RightsSnapshot>, DefaultRightsError> {
        let row = sqlx::query(SELECT_RIGHTS)
            .bind(chat)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row
            .map(stored_from_row)
            .transpose()?
            .map(|row| row.snapshot))
    }

    pub async fn seed_default_rights(
        &self,
        chat: i64,
        base: RightsMask,
        now: i64,
    ) -> Result<AcceptedRightsMutation, DefaultRightsError> {
        self.mutate_default_rights(chat, now, move |row| {
            if !row.snapshot.seeded {
                row.snapshot.seeded = true;
                row.snapshot.base = base;
                row.intent_fingerprint = -1;
            }
            Ok(())
        })
        .await
    }

    pub async fn set_default_right(
        &self,
        chat: i64,
        bit: u8,
        closed: bool,
        now: i64,
    ) -> Result<AcceptedRightsMutation, DefaultRightsError> {
        self.mutate_default_rights(chat, now, move |row| {
            if !row.snapshot.seeded {
                return Err(DefaultRightsError::SeedRequired);
            }
            row.snapshot.base.set(bit, closed)
        })
        .await
    }

    pub async fn set_default_rights_manual_lock(
        &self,
        chat: i64,
        locked: bool,
        now: i64,
    ) -> Result<AcceptedRightsMutation, DefaultRightsError> {
        self.mutate_default_rights(chat, now, move |row| {
            if !row.snapshot.seeded {
                return Err(DefaultRightsError::SeedRequired);
            }
            row.snapshot.manual_lock = locked;
            row.snapshot.timed_until = None;
            Ok(())
        })
        .await
    }

    pub async fn set_default_rights_timed_lock(
        &self,
        chat: i64,
        until: i64,
        now: i64,
    ) -> Result<AcceptedRightsMutation, DefaultRightsError> {
        self.mutate_default_rights(chat, now, move |row| {
            if !row.snapshot.seeded {
                return Err(DefaultRightsError::SeedRequired);
            }
            row.snapshot.manual_lock = false;
            row.snapshot.timed_until = (until > now).then_some(until);
            Ok(())
        })
        .await
    }

    pub async fn set_default_rights_night(
        &self,
        chat: i64,
        night: Option<NightWindow>,
        now: i64,
    ) -> Result<AcceptedRightsMutation, DefaultRightsError> {
        self.mutate_default_rights(chat, now, move |row| {
            if !row.snapshot.seeded {
                return Err(DefaultRightsError::SeedRequired);
            }
            row.snapshot.night = night;
            Ok(())
        })
        .await
    }

    async fn mutate_default_rights(
        &self,
        chat: i64,
        now: i64,
        change: impl FnOnce(&mut StoredRights) -> Result<(), DefaultRightsError>,
    ) -> Result<AcceptedRightsMutation, DefaultRightsError> {
        if !self.ownership.alive.load(Ordering::Acquire) {
            return Err(DefaultRightsError::UncertainState);
        }
        let _writing = self.write_slot(chat).lock().await;
        let _capacity = self.capacity_write.lock().await;
        let new_chat = {
            let cache = self.cache.read().unwrap();
            !self.chat_is_configured(&cache, chat)
        };
        if new_chat && self.max_chats.is_some_and(|max| self.chat_count() >= max) {
            return Err(DefaultRightsError::CapacityReached);
        }
        let mut tx = self.pool.begin().await?;
        let admitted_hash: i64 = sqlx::query_scalar(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             VALUES ($1,
                     COALESCE((SELECT value::BIGINT FROM settings
                               WHERE chat_id = $1 AND key = 'hash'), 0),
                     $2)
             ON CONFLICT (chat_id) DO UPDATE SET
               access_hash = CASE WHEN EXCLUDED.access_hash <> 0
                                  THEN EXCLUDED.access_hash
                                  ELSE durable_chats.access_hash END
             RETURNING access_hash",
        )
        .bind(chat)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        let mut row = locked_row(&mut tx, chat).await?;
        normalize(&mut row, now);
        change(&mut row)?;
        if row.snapshot.remote_unknown
            && row.snapshot.intent_revision == row.snapshot.applied_revision
        {
            row.snapshot.intent_revision = row.snapshot.intent_revision.saturating_add(1);
            row.snapshot.retry_at = now;
            row.delivery_state = "pending".to_owned();
        }
        normalize(&mut row, now);
        let delivery_required = row.snapshot.pending(now);
        write_row(&mut tx, &row).await?;
        let mut commit = MirrorCommitGuard::new(&self.ownership);
        tx.commit()
            .await
            .map_err(DefaultRightsError::CommitUncertain)?;
        if chat != 0 {
            self.durable_chats
                .write()
                .unwrap()
                .insert(chat, admitted_hash);
        }
        commit.disarm();
        Ok(AcceptedRightsMutation {
            snapshot: row.snapshot,
            delivery_required,
        })
    }

    pub async fn default_rights_due(
        &self,
        now: i64,
        limit: i64,
    ) -> Result<Vec<i64>, DefaultRightsError> {
        self.default_rights_due_page(now, limit, 0).await
    }

    pub(crate) async fn default_rights_due_page(
        &self,
        now: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<i64>, DefaultRightsError> {
        let limit = limit.clamp(1, 256);
        let offset = offset.clamp(0, 16_384);
        let candidate_limit = limit.saturating_add(offset);
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT chat_id
             FROM (
                 SELECT chat_id, MIN(due_at) AS due_at
                 FROM (
                     (SELECT chat_id, retry_at AS due_at
                      FROM default_rights_state
                      WHERE seeded
                        AND (intent_revision <> applied_revision
                             OR applied_fingerprint IS DISTINCT FROM intent_fingerprint)
                        AND retry_at <= $1
                        AND (lease_until IS NULL OR lease_until <= $1)
                      ORDER BY retry_at, chat_id LIMIT $3)
                     UNION ALL
                     (SELECT chat_id, next_transition_at AS due_at
                      FROM default_rights_state
                      WHERE seeded AND next_transition_at IS NOT NULL
                        AND next_transition_at <= $1
                        AND (lease_until IS NULL OR lease_until <= $1)
                      ORDER BY next_transition_at, chat_id LIMIT $3)
                     UNION ALL
                     (SELECT chat_id, notice_retry_at AS due_at
                      FROM default_rights_state
                      WHERE notice_kind IS NOT NULL
                        AND notice_retry_at <= $1
                        AND (notice_lease_until IS NULL OR notice_lease_until <= $1)
                      ORDER BY notice_retry_at, chat_id LIMIT $3)
                 ) AS candidates
                 GROUP BY chat_id
             ) AS due
             ORDER BY due_at, chat_id
             LIMIT $2 OFFSET $4",
        )
        .bind(now)
        .bind(limit)
        .bind(candidate_limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(chat,)| chat).collect())
    }

    pub async fn claim_default_rights(
        &self,
        chat: i64,
        now: i64,
    ) -> Result<Option<RightsDeliveryClaim>, DefaultRightsError> {
        let mut tx = self.pool.begin().await?;
        let Some(mut row) = existing_locked_row(&mut tx, chat).await? else {
            tx.rollback().await?;
            return Ok(None);
        };
        normalize(&mut row, now);
        if !row.snapshot.seeded || !row.snapshot.pending(now) {
            write_row(&mut tx, &row).await?;
            tx.commit().await?;
            return Ok(None);
        }
        if row.lease_until.is_some_and(|until| until > now) {
            tx.rollback().await?;
            return Ok(None);
        }
        let token: i64 = sqlx::query_scalar("SELECT nextval('durable_work_token_seq')")
            .fetch_one(&mut *tx)
            .await?;
        row.delivery_state = "leased".to_owned();
        row.lease_token = Some(token);
        row.lease_revision = Some(row.snapshot.intent_revision);
        row.lease_until = Some(now.saturating_add(LEASE_SECONDS));
        let revision = row.snapshot.intent_revision;
        write_row(&mut tx, &row).await?;
        tx.commit().await?;
        Ok(Some(RightsDeliveryClaim {
            chat,
            revision,
            token,
        }))
    }

    pub async fn claimed_default_rights(
        &self,
        claim: &RightsDeliveryClaim,
        now: i64,
    ) -> Result<Option<EffectiveRights>, DefaultRightsError> {
        let Some(row) = sqlx::query(SELECT_RIGHTS)
            .bind(claim.chat)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        let row = stored_from_row(row)?;
        Ok((row.lease_token == Some(claim.token)
            && row.lease_revision == Some(claim.revision)
            && row.snapshot.intent_revision == claim.revision)
            .then(|| row.snapshot.effective_at(now)))
    }

    pub async fn ack_default_rights(
        &self,
        claim: &RightsDeliveryClaim,
        fingerprint: i32,
    ) -> Result<bool, DefaultRightsError> {
        let changed = sqlx::query(
            "UPDATE default_rights_state
             SET applied_revision = $3, applied_fingerprint = $4,
                 remote_unknown = FALSE,
                 delivery_state = CASE WHEN intent_revision = $3
                                            AND intent_fingerprint = $4
                                       THEN 'applied' ELSE 'pending' END,
                 lease_token = NULL, lease_revision = NULL, lease_until = NULL,
                 retry_at = 0, attempts = 0, last_error = NULL,
                 notice_kind = CASE WHEN intent_notice IS NOT NULL
                                    THEN intent_notice ELSE notice_kind END,
                 notice_revision = CASE WHEN intent_notice IS NOT NULL
                                        THEN $3 ELSE notice_revision END,
                 notice_token = CASE WHEN intent_notice IS NOT NULL
                                     THEN NULL ELSE notice_token END,
                 notice_lease_until = CASE WHEN intent_notice IS NOT NULL
                                           THEN NULL ELSE notice_lease_until END,
                 notice_retry_at = CASE WHEN intent_notice IS NOT NULL
                                        THEN 0 ELSE notice_retry_at END,
                 notice_attempts = CASE WHEN intent_notice IS NOT NULL
                                        THEN 0 ELSE notice_attempts END,
                 notice_error = CASE WHEN intent_notice IS NOT NULL
                                     THEN NULL ELSE notice_error END,
                 intent_notice = NULL
             WHERE chat_id = $1 AND lease_token = $2 AND lease_revision = $3
               AND intent_revision = $3 AND intent_fingerprint = $4",
        )
        .bind(claim.chat)
        .bind(claim.token)
        .bind(claim.revision)
        .bind(fingerprint)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed == 1 {
            return Ok(true);
        }
        sqlx::query(
            "UPDATE default_rights_state
             SET delivery_state = 'pending', lease_token = NULL, lease_revision = NULL,
                 lease_until = NULL, retry_at = 0
             WHERE chat_id = $1 AND lease_token = $2 AND lease_revision = $3",
        )
        .bind(claim.chat)
        .bind(claim.token)
        .bind(claim.revision)
        .execute(&self.pool)
        .await?;
        Ok(false)
    }

    pub async fn retry_default_rights(
        &self,
        claim: &RightsDeliveryClaim,
        now: i64,
        error: &str,
    ) -> Result<Option<i64>, DefaultRightsError> {
        let current: Option<(i32,)> = sqlx::query_as(
            "SELECT attempts FROM default_rights_state
             WHERE chat_id = $1 AND lease_token = $2 AND lease_revision = $3",
        )
        .bind(claim.chat)
        .bind(claim.token)
        .bind(claim.revision)
        .fetch_optional(&self.pool)
        .await?;
        let Some((attempts,)) = current else {
            return Ok(None);
        };
        let attempts = attempts.saturating_add(1);
        let retry_at = now.saturating_add(retry_delay(attempts));
        let error = truncate_utf8(error, LAST_ERROR_BYTES);
        let changed = sqlx::query(
            "UPDATE default_rights_state
             SET delivery_state = 'pending', lease_token = NULL, lease_revision = NULL,
                 lease_until = NULL, retry_at = $4, attempts = $5, last_error = $6
             WHERE chat_id = $1 AND lease_token = $2 AND lease_revision = $3",
        )
        .bind(claim.chat)
        .bind(claim.token)
        .bind(claim.revision)
        .bind(retry_at)
        .bind(attempts)
        .bind(error)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok((changed == 1).then_some(retry_at))
    }

    pub async fn claim_default_rights_notice(
        &self,
        chat: i64,
        now: i64,
    ) -> Result<Option<RightsNoticeClaim>, DefaultRightsError> {
        let token: i64 = sqlx::query_scalar("SELECT nextval('durable_work_token_seq')")
            .fetch_one(&self.pool)
            .await?;
        let row: Option<(String, i64)> = sqlx::query_as(
            "UPDATE default_rights_state
             SET notice_token=$2, notice_lease_until=$3
             WHERE chat_id=$1 AND notice_kind IS NOT NULL
               AND notice_retry_at <= $4
               AND (notice_lease_until IS NULL OR notice_lease_until <= $4)
             RETURNING notice_kind, notice_revision",
        )
        .bind(chat)
        .bind(token)
        .bind(now.saturating_add(LEASE_SECONDS))
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(kind, revision)| {
            Ok(RightsNoticeClaim {
                chat,
                token,
                revision,
                kind: RightsNoticeKind::from_storage(&kind)?,
            })
        })
        .transpose()
    }

    pub async fn ack_default_rights_notice(
        &self,
        claim: &RightsNoticeClaim,
    ) -> Result<bool, DefaultRightsError> {
        Ok(sqlx::query(
            "UPDATE default_rights_state
             SET notice_kind=NULL, notice_revision=NULL, notice_token=NULL,
                 notice_lease_until=NULL, notice_retry_at=0, notice_attempts=0,
                 notice_error=NULL
             WHERE chat_id=$1 AND notice_token=$2 AND notice_revision=$3",
        )
        .bind(claim.chat)
        .bind(claim.token)
        .bind(claim.revision)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    pub async fn retry_default_rights_notice(
        &self,
        claim: &RightsNoticeClaim,
        now: i64,
        error: &str,
    ) -> Result<Option<i64>, DefaultRightsError> {
        let attempts: Option<i32> = sqlx::query_scalar(
            "SELECT notice_attempts FROM default_rights_state
             WHERE chat_id=$1 AND notice_token=$2 AND notice_revision=$3",
        )
        .bind(claim.chat)
        .bind(claim.token)
        .bind(claim.revision)
        .fetch_optional(&self.pool)
        .await?;
        let Some(attempts) = attempts else {
            return Ok(None);
        };
        let attempts = attempts.saturating_add(1);
        let retry_at = now.saturating_add(retry_delay(attempts));
        let changed = sqlx::query(
            "UPDATE default_rights_state
             SET notice_token=NULL, notice_lease_until=NULL, notice_retry_at=$4,
                 notice_attempts=$5, notice_error=$6
             WHERE chat_id=$1 AND notice_token=$2 AND notice_revision=$3",
        )
        .bind(claim.chat)
        .bind(claim.token)
        .bind(claim.revision)
        .bind(retry_at)
        .bind(attempts)
        .bind(truncate_utf8(error, LAST_ERROR_BYTES))
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok((changed == 1).then_some(retry_at))
    }
}

const SELECT_RIGHTS: &str =
    "SELECT chat_id, base_mask, seeded, manual_lock, timed_until, night_from, night_to,
            intent_revision, intent_fingerprint, applied_revision, applied_fingerprint,
            remote_unknown, delivery_state, lease_token, lease_revision, lease_until,
            retry_at, attempts, last_error, next_transition_at, intent_notice,
            notice_kind, notice_revision, notice_token, notice_lease_until,
            notice_retry_at, notice_attempts, notice_error
     FROM default_rights_state WHERE chat_id = $1";

async fn locked_row(
    tx: &mut Transaction<'_, Postgres>,
    chat: i64,
) -> Result<StoredRights, DefaultRightsError> {
    sqlx::query("INSERT INTO default_rights_state (chat_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(chat)
        .execute(&mut **tx)
        .await?;
    let row = sqlx::query(&format!("{SELECT_RIGHTS} FOR UPDATE"))
        .bind(chat)
        .fetch_one(&mut **tx)
        .await?;
    stored_from_row(row)
}

async fn existing_locked_row(
    tx: &mut Transaction<'_, Postgres>,
    chat: i64,
) -> Result<Option<StoredRights>, DefaultRightsError> {
    let row = sqlx::query(&format!("{SELECT_RIGHTS} FOR UPDATE"))
        .bind(chat)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(stored_from_row).transpose()
}

fn stored_from_row(row: sqlx::postgres::PgRow) -> Result<StoredRights, DefaultRightsError> {
    let night_from: Option<i32> = row.try_get("night_from")?;
    let night_to: Option<i32> = row.try_get("night_to")?;
    let night = match (night_from, night_to) {
        (None, None) => None,
        (Some(from), Some(to)) => {
            let from = u32::try_from(from).map_err(|_| {
                DefaultRightsError::InvalidState(format!("negative night start {from}"))
            })?;
            let to = u32::try_from(to).map_err(|_| {
                DefaultRightsError::InvalidState(format!("negative night end {to}"))
            })?;
            Some(NightWindow::new(from, to).ok_or_else(|| {
                DefaultRightsError::InvalidState(format!("invalid night endpoints {from}|{to}"))
            })?)
        }
        _ => {
            return Err(DefaultRightsError::InvalidState(
                "night endpoints must both be present or absent".to_owned(),
            ));
        }
    };
    Ok(StoredRights {
        snapshot: RightsSnapshot {
            chat: row.try_get("chat_id")?,
            base: RightsMask::from_storage(row.try_get("base_mask")?)?,
            seeded: row.try_get("seeded")?,
            manual_lock: row.try_get("manual_lock")?,
            timed_until: row.try_get("timed_until")?,
            night,
            intent_revision: row.try_get("intent_revision")?,
            applied_revision: row.try_get("applied_revision")?,
            applied_fingerprint: row.try_get("applied_fingerprint")?,
            remote_unknown: row.try_get("remote_unknown")?,
            attempts: row.try_get("attempts")?,
            last_error: row.try_get("last_error")?,
            retry_at: row.try_get("retry_at")?,
            next_transition_at: row.try_get("next_transition_at")?,
        },
        intent_fingerprint: row.try_get("intent_fingerprint")?,
        delivery_state: row.try_get("delivery_state")?,
        lease_token: row.try_get("lease_token")?,
        lease_revision: row.try_get("lease_revision")?,
        lease_until: row.try_get("lease_until")?,
        intent_notice: row
            .try_get::<Option<String>, _>("intent_notice")?
            .as_deref()
            .map(RightsNoticeKind::from_storage)
            .transpose()?,
        notice_kind: row
            .try_get::<Option<String>, _>("notice_kind")?
            .as_deref()
            .map(RightsNoticeKind::from_storage)
            .transpose()?,
        notice_revision: row.try_get("notice_revision")?,
        notice_token: row.try_get("notice_token")?,
        notice_lease_until: row.try_get("notice_lease_until")?,
        notice_retry_at: row.try_get("notice_retry_at")?,
        notice_attempts: row.try_get("notice_attempts")?,
        notice_error: row.try_get("notice_error")?,
    })
}

async fn write_row(
    tx: &mut Transaction<'_, Postgres>,
    row: &StoredRights,
) -> Result<(), DefaultRightsError> {
    let (night_from, night_to) = row.snapshot.night.map_or((None, None), |night| {
        (Some(i32::from(night.from)), Some(i32::from(night.to)))
    });
    sqlx::query(
        "UPDATE default_rights_state SET
             base_mask=$2, seeded=$3, manual_lock=$4, timed_until=$5,
             night_from=$6, night_to=$7, intent_revision=$8, intent_fingerprint=$9,
             applied_revision=$10, applied_fingerprint=$11, remote_unknown=$12,
             delivery_state=$13, lease_token=$14, lease_revision=$15, lease_until=$16,
             retry_at=$17, attempts=$18, last_error=$19, next_transition_at=$20,
             intent_notice=$21, notice_kind=$22, notice_revision=$23,
             notice_token=$24, notice_lease_until=$25, notice_retry_at=$26,
             notice_attempts=$27, notice_error=$28
         WHERE chat_id=$1",
    )
    .bind(row.snapshot.chat)
    .bind(row.snapshot.base.storage())
    .bind(row.snapshot.seeded)
    .bind(row.snapshot.manual_lock)
    .bind(row.snapshot.timed_until)
    .bind(night_from)
    .bind(night_to)
    .bind(row.snapshot.intent_revision)
    .bind(row.intent_fingerprint)
    .bind(row.snapshot.applied_revision)
    .bind(row.snapshot.applied_fingerprint)
    .bind(row.snapshot.remote_unknown)
    .bind(&row.delivery_state)
    .bind(row.lease_token)
    .bind(row.lease_revision)
    .bind(row.lease_until)
    .bind(row.snapshot.retry_at)
    .bind(row.snapshot.attempts)
    .bind(&row.snapshot.last_error)
    .bind(row.snapshot.next_transition_at)
    .bind(row.intent_notice.map(RightsNoticeKind::as_storage))
    .bind(row.notice_kind.map(RightsNoticeKind::as_storage))
    .bind(row.notice_revision)
    .bind(row.notice_token)
    .bind(row.notice_lease_until)
    .bind(row.notice_retry_at)
    .bind(row.notice_attempts)
    .bind(&row.notice_error)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn normalize(row: &mut StoredRights, now: i64) {
    let timed_expired = row.snapshot.timed_until.is_some_and(|until| until <= now);
    let night_boundary_due = row
        .snapshot
        .next_transition_at
        .is_some_and(|transition| transition <= now)
        && row.snapshot.night.is_some();
    if timed_expired {
        row.snapshot.timed_until = None;
    }
    row.snapshot.next_transition_at =
        next_transition(now, row.snapshot.timed_until, row.snapshot.night);
    if !row.snapshot.seeded {
        return;
    }
    let fingerprint = row.snapshot.effective_at(now).fingerprint();
    if row.intent_fingerprint != fingerprint {
        row.intent_fingerprint = fingerprint;
        row.snapshot.intent_revision = row.snapshot.intent_revision.saturating_add(1);
        let applied_force = row
            .snapshot
            .applied_fingerprint
            .map(|applied| applied & FORCE_FINGERPRINT != 0);
        let target_force = fingerprint & FORCE_FINGERPRINT != 0;
        row.intent_notice = if applied_force.is_some_and(|applied| applied != target_force) {
            if timed_expired && !target_force {
                Some(RightsNoticeKind::TimedOpened)
            } else if night_boundary_due {
                Some(if target_force {
                    RightsNoticeKind::NightLocked
                } else {
                    RightsNoticeKind::NightOpened
                })
            } else {
                None
            }
        } else {
            None
        };
        row.snapshot.retry_at = now;
        row.snapshot.attempts = 0;
        row.snapshot.last_error = None;
        if row.lease_token.is_none() {
            row.delivery_state = "pending".to_owned();
        }
    }
}

fn next_transition(now: i64, timed_until: Option<i64>, night: Option<NightWindow>) -> Option<i64> {
    let mut next = timed_until.filter(|until| *until > now);
    if let Some(night) = night {
        let day = now - now.rem_euclid(86_400);
        for minute in [night.from, night.to] {
            let mut boundary = day.saturating_add(i64::from(minute) * 60);
            if boundary <= now {
                boundary = boundary.saturating_add(86_400);
            }
            next = Some(next.map_or(boundary, |current| current.min(boundary)));
        }
    }
    next
}

fn retry_delay(attempts: i32) -> i64 {
    let shift = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(u32::MAX)
        .min(20);
    RETRY_MIN_SECONDS
        .saturating_mul(1_i64.checked_shl(shift).unwrap_or(i64::MAX))
        .min(RETRY_MAX_SECONDS)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn admit_fixture_chats(
        executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
        chats: &[i64],
    ) {
        sqlx::query(
            "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
             SELECT chat_id, 1, 0 FROM UNNEST($1::BIGINT[]) AS input(chat_id)
             ON CONFLICT DO NOTHING",
        )
        .bind(chats)
        .execute(executor)
        .await
        .unwrap();
    }

    fn snapshot(base: RightsMask) -> RightsSnapshot {
        RightsSnapshot {
            chat: -1,
            base,
            seeded: true,
            manual_lock: false,
            timed_until: None,
            night: None,
            intent_revision: 1,
            applied_revision: 0,
            applied_fingerprint: None,
            remote_unknown: false,
            attempts: 0,
            last_error: None,
            retry_at: 0,
            next_transition_at: None,
        }
    }

    #[test]
    fn overlays_are_or_composed_and_preserve_granular_base() {
        let mut base = RightsMask::empty();
        base.set(3, true).unwrap();
        let mut row = snapshot(base);
        row.manual_lock = true;
        row.timed_until = Some(20_000);
        row.night = NightWindow::new(23 * 60, 7 * 60);
        assert!(row.effective_at(10_000).force_all);
        row.manual_lock = false;
        row.timed_until = None;
        assert!(row.effective_at(86_400 + 60 * 30).force_all);
        assert!(!row.effective_at(86_400 + 12 * 3_600).force_all);
        assert!(row.effective_at(86_400 + 12 * 3_600).base.contains(3));
    }

    #[test]
    fn next_transition_chooses_timer_before_night_and_rolls_boundaries() {
        let now = 86_400 + 12 * 3_600;
        let night = NightWindow::new(23 * 60, 7 * 60);
        assert_eq!(next_transition(now, Some(now + 10), night), Some(now + 10));
        assert_eq!(next_transition(now, None, night), Some(86_400 + 23 * 3_600));
        let after_start = 86_400 + 23 * 3_600 + 1;
        assert_eq!(
            next_transition(after_start, None, night),
            Some(2 * 86_400 + 7 * 3_600)
        );
    }

    #[test]
    fn retry_backoff_and_error_storage_are_bounded() {
        assert_eq!(retry_delay(1), 5);
        assert_eq!(retry_delay(2), 10);
        assert_eq!(retry_delay(100), RETRY_MAX_SECONDS);
        let long = "€".repeat(1_000);
        let cut = truncate_utf8(&long, LAST_ERROR_BYTES);
        assert!(cut.len() <= LAST_ERROR_BYTES);
        assert!(std::str::from_utf8(cut.as_bytes()).is_ok());
    }

    #[test]
    fn equal_night_ends_are_rejected_in_the_type() {
        assert!(NightWindow::new(60, 60).is_none());
        assert!(NightWindow::new(1_500, 120).is_none());
        assert!(NightWindow::new(60, 1_500).is_none());
    }

    #[test]
    fn stored_rights_masks_reject_negative_and_unknown_bits() {
        assert_eq!(RightsMask::from_storage(0).unwrap(), RightsMask::empty());
        assert_eq!(RightsMask::from_storage(16_383).unwrap(), ALL_RIGHTS);
        assert!(RightsMask::from_storage(-1).is_err());
        assert!(RightsMask::from_storage(16_384).is_err());
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL default-rights storage constraint test"]
    async fn database_rejects_invalid_masks_and_night_windows() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chats = [-999_999_997_711_i64, -999_999_997_712, -999_999_997_713];
        admit_fixture_chats(&settings.pool, &chats).await;
        assert!(
            sqlx::query("INSERT INTO default_rights_state (chat_id, base_mask) VALUES ($1, 16384)")
                .bind(chats[0])
                .execute(&settings.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query(
                "INSERT INTO default_rights_state (chat_id, intent_fingerprint) VALUES ($1, 32768)"
            )
            .bind(chats[1])
            .execute(&settings.pool)
            .await
            .is_err()
        );
        assert!(
            sqlx::query(
                "INSERT INTO default_rights_state (chat_id, night_from, night_to) VALUES ($1, 60, 60)"
            )
            .bind(chats[2])
            .execute(&settings.pool)
            .await
            .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL default-rights queue test"]
    async fn applied_static_rows_cannot_starve_pending_work_and_page_is_bounded() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chats: Vec<i64> = (-999_880_100..=-999_880_000).collect();
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chats(&settings.pool, &chats).await;
        for &chat in &chats[..70] {
            sqlx::query(
                "INSERT INTO default_rights_state
                 (chat_id,seeded,intent_revision,intent_fingerprint,
                  applied_revision,applied_fingerprint,delivery_state)
                 VALUES ($1,TRUE,1,0,1,0,'applied')",
            )
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        }
        let pending = chats[70];
        sqlx::query(
            "INSERT INTO default_rights_state
             (chat_id,seeded,intent_revision,intent_fingerprint,
              applied_revision,applied_fingerprint,delivery_state,retry_at)
             VALUES ($1,TRUE,2,1,1,0,'pending',0)",
        )
        .bind(pending)
        .execute(&settings.pool)
        .await
        .unwrap();
        assert_eq!(
            settings.default_rights_due(10_000, 64).await.unwrap(),
            vec![pending]
        );
        for &chat in &chats[71..] {
            sqlx::query(
                "INSERT INTO default_rights_state
                 (chat_id,seeded,intent_revision,intent_fingerprint,
                  applied_revision,applied_fingerprint,delivery_state,retry_at)
                 VALUES ($1,TRUE,2,1,1,0,'pending',0)",
            )
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        }
        assert!(settings.default_rights_due(10_000, 16).await.unwrap().len() <= 16);
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL 50k-row due-query EXPLAIN test"]
    async fn due_indexes_avoid_scanning_static_fleet_rows() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let mut tx = settings.pool.begin().await.unwrap();
        sqlx::query("SET LOCAL statement_timeout = '10s'")
            .execute(&mut *tx)
            .await
            .unwrap();
        let first = -999_990_000_i64;
        let last = first + 49_999;
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id BETWEEN $1 AND $2")
            .bind(first)
            .bind(last + 2)
            .execute(&mut *tx)
            .await
            .unwrap();
        let fixture_chats: Vec<i64> = (first..=last + 2).collect();
        admit_fixture_chats(&mut *tx, &fixture_chats).await;
        sqlx::query(
            "INSERT INTO default_rights_state
             (chat_id,seeded,intent_revision,intent_fingerprint,
              applied_revision,applied_fingerprint,delivery_state)
             SELECT chat,TRUE,1,0,1,0,'applied'
             FROM generate_series($1::bigint,$2::bigint) AS chat",
        )
        .bind(first)
        .bind(last)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO default_rights_state
             (chat_id,seeded,intent_revision,intent_fingerprint,
              applied_revision,applied_fingerprint,delivery_state,retry_at)
             VALUES ($1,TRUE,2,1,1,0,'pending',0)",
        )
        .bind(last + 1)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO default_rights_state
             (chat_id,seeded,intent_revision,intent_fingerprint,
              applied_revision,applied_fingerprint,delivery_state,next_transition_at)
             VALUES ($1,TRUE,1,0,1,0,'applied',0)",
        )
        .bind(last + 2)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("SET LOCAL enable_seqscan=off")
            .execute(&mut *tx)
            .await
            .unwrap();
        let plan: Vec<(String,)> = sqlx::query_as(
            "EXPLAIN (ANALYZE, BUFFERS)
             SELECT chat_id FROM default_rights_state
             WHERE seeded
               AND (intent_revision <> applied_revision
                    OR applied_fingerprint IS DISTINCT FROM intent_fingerprint)
               AND retry_at <= 10000
               AND (lease_until IS NULL OR lease_until <= 10000)
             ORDER BY retry_at, chat_id LIMIT 64",
        )
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        let plan = plan
            .into_iter()
            .map(|(line,)| line)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            plan.contains("default_rights_retry_due"),
            "unexpected plan:\n{plan}"
        );
        let transition_plan: Vec<(String,)> = sqlx::query_as(
            "EXPLAIN (ANALYZE, BUFFERS)
             SELECT chat_id FROM default_rights_state
             WHERE seeded AND next_transition_at IS NOT NULL
               AND next_transition_at <= 10000
               AND (lease_until IS NULL OR lease_until <= 10000)
             ORDER BY next_transition_at, chat_id LIMIT 64",
        )
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        let transition_plan = transition_plan
            .into_iter()
            .map(|(line,)| line)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            transition_plan.contains("default_rights_transition_due"),
            "unexpected transition plan:\n{transition_plan}"
        );
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL stale-ack and crash-lease test"]
    async fn stale_ack_cannot_certify_superseded_intent_and_expired_lease_recovers() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chat = -999_880_200;
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chats(&settings.pool, &[chat]).await;
        settings
            .seed_default_rights(chat, RightsMask::empty(), 1_000)
            .await
            .unwrap();
        let first = settings
            .claim_default_rights(chat, 1_000)
            .await
            .unwrap()
            .unwrap();
        settings
            .set_default_right(chat, 1, true, 1_001)
            .await
            .unwrap();
        assert!(!settings.ack_default_rights(&first, 0).await.unwrap());
        assert!(
            settings
                .default_rights(chat)
                .await
                .unwrap()
                .unwrap()
                .pending(1_001)
        );
        let second = settings
            .claim_default_rights(chat, 1_001)
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE default_rights_state SET lease_until=0 WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        assert!(
            settings
                .default_rights_due(1_002, 64)
                .await
                .unwrap()
                .contains(&chat)
        );
        assert!(
            settings
                .retry_default_rights(&second, 1_002, "transport unavailable")
                .await
                .unwrap()
                .is_some()
        );
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL post-ack notice outbox test"]
    async fn transition_notice_is_not_claimable_until_rights_ack() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chat = -999_880_305;
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chats(&settings.pool, &[chat]).await;
        sqlx::query(
            "INSERT INTO default_rights_state
             (chat_id,seeded,timed_until,intent_revision,intent_fingerprint,
              applied_revision,applied_fingerprint,delivery_state,next_transition_at)
             VALUES ($1,TRUE,999,1,16384,1,16384,'applied',999)",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        let claim = settings
            .claim_default_rights(chat, 1_000)
            .await
            .unwrap()
            .unwrap();
        assert!(
            settings
                .claim_default_rights_notice(chat, 1_000)
                .await
                .unwrap()
                .is_none()
        );
        assert!(settings.ack_default_rights(&claim, 0).await.unwrap());
        let notice = settings
            .claim_default_rights_notice(chat, 1_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(notice.kind, RightsNoticeKind::TimedOpened);
        assert!(settings.ack_default_rights_notice(&notice).await.unwrap());
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL legacy-rights migration test"]
    async fn legacy_static_rights_migrate_as_applied_without_fleet_rpc() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chat = -999_880_300;
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chats(&settings.pool, &[chat]).await;
        sqlx::query(
            "INSERT INTO settings (chat_id,key,value) VALUES
             ($1,'perm_seeded',''),($1,'perm:photos','')",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        let mut connection = settings.pool.acquire().await.unwrap();
        Settings::init_default_rights(&mut connection)
            .await
            .unwrap();
        let snapshot = settings.default_rights(chat).await.unwrap().unwrap();
        assert!(snapshot.base.contains(1));
        assert!(snapshot.remote_unknown);
        assert!(!snapshot.pending(1_000_000));
        assert!(
            !settings
                .default_rights_due(i64::MAX / 2, 256)
                .await
                .unwrap()
                .contains(&chat)
        );
        let accepted = settings
            .set_default_right(chat, 0, false, 1_000_000)
            .await
            .unwrap();
        assert!(accepted.delivery_required);
        assert!(
            settings
                .default_rights_due(1_000_000, 256)
                .await
                .unwrap()
                .contains(&chat)
        );
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL legacy overlay-only migration test"]
    async fn legacy_night_and_expired_timer_only_rows_are_seeded_and_due() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let timer_chat = -999_880_303;
        let night_chat = -999_880_304;
        let chats = [timer_chat, night_chat];
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chats(&settings.pool, &chats).await;
        sqlx::query(
            "INSERT INTO settings (chat_id,key,value) VALUES
             ($1,'glock_until','1'),
             ($2,'night','60|120')",
        )
        .bind(timer_chat)
        .bind(night_chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        let mut connection = settings.pool.acquire().await.unwrap();
        Settings::init_default_rights(&mut connection)
            .await
            .unwrap();

        let timer = settings.default_rights(timer_chat).await.unwrap().unwrap();
        assert!(timer.seeded);
        assert_eq!(timer.base, RightsMask::empty());
        assert!(timer.pending(i64::MAX / 4));
        let night = settings.default_rights(night_chat).await.unwrap().unwrap();
        assert!(night.seeded);
        assert_eq!(night.night, NightWindow::new(60, 120));
        assert!(night.pending(i64::MAX / 4));
        let due = settings
            .default_rights_due(i64::MAX / 4, 256)
            .await
            .unwrap();
        assert!(due.contains(&timer_chat));
        assert!(due.contains(&night_chat));

        sqlx::query("DELETE FROM default_rights_state WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL no-op mutation contract"]
    async fn repeated_same_value_is_applied_noop_not_pending() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chat = -999_880_301;
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chats(&settings.pool, &[chat]).await;
        sqlx::query(
            "INSERT INTO default_rights_state
             (chat_id,seeded,intent_revision,intent_fingerprint,
              applied_revision,applied_fingerprint,delivery_state)
             VALUES ($1,TRUE,1,0,1,0,'applied')",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        let accepted = settings
            .set_default_right(chat, 0, false, 1_000)
            .await
            .unwrap();
        assert!(!accepted.delivery_required);
        assert_eq!(accepted.snapshot.intent_revision, 1);
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL concurrent rights mutation test"]
    async fn concurrent_granular_edits_serialize_without_lost_bits() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let settings = Settings::connect(&url).await.unwrap();
        let chat = -999_880_302;
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        admit_fixture_chats(&settings.pool, &[chat]).await;
        sqlx::query(
            "INSERT INTO default_rights_state
             (chat_id,seeded,intent_revision,intent_fingerprint,
              applied_revision,applied_fingerprint,delivery_state)
             VALUES ($1,TRUE,1,0,1,0,'applied')",
        )
        .bind(chat)
        .execute(&settings.pool)
        .await
        .unwrap();
        let (first, second) = tokio::join!(
            settings.set_default_right(chat, 0, true, 1_000),
            settings.set_default_right(chat, 1, true, 1_000),
        );
        first.unwrap();
        second.unwrap();
        let snapshot = settings.default_rights(chat).await.unwrap().unwrap();
        assert!(snapshot.base.contains(0) && snapshot.base.contains(1));
        assert!(snapshot.intent_revision >= 3);
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id=$1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "isolated PostgreSQL rights/settings shared capacity test"]
    async fn rights_only_chat_remains_counted_and_blocks_next_chat_at_capacity() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let first = -999_880_306;
        let second = -999_880_307;
        let chats = [first, second];
        let initial = Settings::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&initial.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&initial.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_chats WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&initial.pool)
            .await
            .unwrap();
        let baseline: i64 = sqlx::query_scalar("SELECT count(*) FROM durable_chats")
            .fetch_one(&initial.pool)
            .await
            .unwrap();
        drop(initial);
        let limit = usize::try_from(baseline).unwrap().saturating_add(1);
        let settings = Settings::connect_with_chat_limit(&url, Some(limit))
            .await
            .unwrap();
        settings
            .seed_default_rights(first, RightsMask::empty(), 1_000)
            .await
            .unwrap();
        assert!(settings.chats().contains(&first));
        assert_eq!(settings.chat_count(), limit);
        settings
            .try_set(first, "capacity_fixture", true)
            .await
            .unwrap();
        settings
            .try_set(first, "capacity_fixture", false)
            .await
            .unwrap();
        assert!(settings.chats().contains(&first));
        assert_eq!(settings.chat_count(), limit);
        assert!(matches!(
            settings
                .seed_default_rights(second, RightsMask::empty(), 1_000)
                .await,
            Err(DefaultRightsError::CapacityReached)
        ));
        sqlx::query("DELETE FROM default_rights_state WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM durable_chats WHERE chat_id = ANY($1)")
            .bind(&chats[..])
            .execute(&settings.pool)
            .await
            .unwrap();
    }
}
