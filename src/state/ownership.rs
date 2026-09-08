use super::*;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::time::Duration;

pub(super) struct Ownership {
    pub epoch: Arc<AtomicI64>,
    pub alive: Arc<AtomicBool>,
    pub lost: Arc<tokio::sync::Notify>,
}

impl Default for Ownership {
    fn default() -> Self {
        Self {
            epoch: Arc::new(AtomicI64::new(0)),
            alive: Arc::new(AtomicBool::new(true)),
            lost: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

impl Settings {
    pub fn liveness(&self) -> Arc<AtomicBool> {
        self.ownership.alive.clone()
    }

    pub async fn ownership_lost(&self) {
        loop {
            let notified = self.ownership.lost.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.ownership.alive.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    pub async fn monitor_ownership(self: Arc<Self>) {
        let mut guard = self._process_lock.lock().await;
        let Some(connection) = guard.as_mut() else {
            return;
        };
        let epoch = self.ownership.epoch.load(Ordering::Acquire);
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                sqlx::query_scalar::<_, i64>("SELECT epoch FROM runtime_owner WHERE id=0")
                    .fetch_one(&mut **connection),
            )
            .await;
            if !matches!(result, Ok(Ok(current)) if current == epoch) {
                self.ownership.alive.store(false, Ordering::Release);
                self.ownership.lost.notify_waiters();
                log::error!("database ownership lost; refusing new work and Telegram requests");
                return;
            }
        }
    }

    pub(super) async fn install_fencing(connection: &mut sqlx::PgConnection) -> Result<i64> {
        sqlx::query("CREATE TABLE IF NOT EXISTS runtime_owner (id integer PRIMARY KEY CHECK(id=0), epoch bigint NOT NULL)")
            .execute(&mut *connection).await?;
        let epoch: i64 = sqlx::query_scalar("INSERT INTO runtime_owner(id,epoch) VALUES(0,1) ON CONFLICT(id) DO UPDATE SET epoch=runtime_owner.epoch+1 RETURNING epoch")
            .fetch_one(&mut *connection).await?;
        sqlx::query("CREATE OR REPLACE FUNCTION groupbot_fence_write() RETURNS trigger LANGUAGE plpgsql AS $$
            DECLARE requested bigint; current_epoch bigint;
            BEGIN
                requested := coalesce(nullif(current_setting('groupbot.owner_epoch', true), ''), '0')::bigint;
                IF requested <> 0 THEN
                    SELECT epoch INTO current_epoch FROM runtime_owner WHERE id=0 FOR SHARE;
                    IF requested IS DISTINCT FROM current_epoch THEN
                        RAISE EXCEPTION 'stale groupbot ownership epoch' USING ERRCODE='55000';
                    END IF;
                END IF;
                RETURN NULL;
            END $$").execute(&mut *connection).await?;
        let tables: Vec<String> = sqlx::query_scalar("SELECT tablename FROM pg_tables WHERE schemaname=current_schema() AND tablename <> 'runtime_owner'")
            .fetch_all(&mut *connection).await?;
        for table in tables {
            let quoted = format!("\"{}\"", table.replace('"', "\"\""));
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid=$1::regclass AND tgname='groupbot_owner_fence')")
                .bind(&quoted).fetch_one(&mut *connection).await?;
            if !exists {
                sqlx::query(&format!("CREATE TRIGGER groupbot_owner_fence BEFORE INSERT OR UPDATE OR DELETE ON {quoted} FOR EACH STATEMENT EXECUTE FUNCTION groupbot_fence_write()"))
                    .execute(&mut *connection).await?;
            }
        }
        Ok(epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "isolated PostgreSQL ownership fence and request gate fault test"]
    async fn checked_out_connection_is_fenced_and_monitor_closes_gate() {
        const URL: &str = "postgresql://moh@127.0.0.1:55432/groupbot_audit";
        let owner = Arc::new(
            Settings::connect_with_chat_limit_and_process_lock(URL, Some(50_000))
                .await
                .unwrap(),
        );
        owner
            .try_set_value(-999_777_002, "audit_fence", "before")
            .await
            .unwrap();
        let control = PgPool::connect(URL).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut **owner._process_lock.lock().await.as_mut().unwrap())
            .await
            .unwrap();
        let mut checked_out = owner.pool.acquire().await.unwrap();
        let monitor = tokio::spawn(owner.clone().monitor_ownership());
        sqlx::query("SELECT pg_terminate_backend($1)")
            .bind(pid)
            .execute(&control)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), owner.ownership_lost())
            .await
            .unwrap();
        assert!(!owner.liveness().load(Ordering::Acquire));
        let successor = Settings::connect_with_chat_limit_and_process_lock(URL, Some(50_000))
            .await
            .unwrap();
        let error = sqlx::query("INSERT INTO settings(chat_id,key,value) VALUES(-999777002,'audit_fence','bad') ON CONFLICT(chat_id,key) DO UPDATE SET value=EXCLUDED.value")
            .execute(&mut *checked_out).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("55000")
        );
        assert!(
            successor
                .try_set_value(-999_777_002, "audit_fence", "good")
                .await
                .unwrap()
        );
        monitor.await.unwrap();
        println!(
            "PERF {}",
            serde_json::json!({"kind":"ownership_gate", "stale_checked_out_write_rejected":true,"request_gate_closed":true})
        );
    }
}
