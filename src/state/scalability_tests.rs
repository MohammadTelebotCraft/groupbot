use super::*;
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "isolated PostgreSQL probe; set DB_STATEMENT_TIMEOUT_MS=100 and DB_LOCK_TIMEOUT_MS=50"]
async fn database_delays() {
    const URL: &str = "postgresql://moh@127.0.0.1:55432/groupbot_audit";
    let settings = std::sync::Arc::new(Settings::connect(URL).await.unwrap());
    settings
        .try_set_value(-999_001, "audit_probe", "fixture_reset")
        .await
        .unwrap();
    let statement: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&settings.pool)
        .await
        .unwrap();
    let lock: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&settings.pool)
        .await
        .unwrap();
    let start = Instant::now();
    let sleep = sqlx::query("SELECT pg_sleep(0.3)")
        .execute(&settings.pool)
        .await;
    let sleep_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert!(
        settings
            .try_set_value(-999_001, "audit_probe", "before")
            .await
            .unwrap()
    );
    let blocker = PgPool::connect(URL).await.unwrap();
    let mut tx = blocker.begin().await.unwrap();
    sqlx::query("LOCK TABLE settings IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(350)).await;
        tx.rollback().await.unwrap();
    });
    let start = Instant::now();
    let written = settings
        .try_set_value(-999_001, "audit_probe", "after")
        .await
        .unwrap_or(false);
    let lock_ms = start.elapsed().as_secs_f64() * 1000.0;
    release.await.unwrap();
    let mirror = settings.value(-999_001, "audit_probe").unwrap();
    let durable: String = sqlx::query_scalar(
        "SELECT value FROM settings WHERE chat_id=-999001 AND key='audit_probe'",
    )
    .fetch_one(&settings.pool)
    .await
    .unwrap();
    assert_eq!(
        mirror, durable,
        "error rollback must restore the settings mirror"
    );
    assert_eq!(mirror, if written { "after" } else { "before" });
    if statement != "0" {
        assert!(sleep.is_err());
        assert!(sleep_ms < 300.0);
        assert!(!written && lock_ms < 350.0);
    }
    assert!(
        settings
            .try_set_value(-999_001, "audit_probe", "recovered")
            .await
            .unwrap()
    );
    println!(
        "PERF {}",
        serde_json::json!({"kind":"database_delays","statement_timeout":statement,"lock_timeout":lock,
        "sleep_ms":sleep_ms,"sleep_failed":sleep.is_err(),"lock_ms":lock_ms,"blocked_write_succeeded":written,"mirror_matches_database":mirror==durable,"recovery_write":true})
    );
}

#[tokio::test]
#[ignore = "isolated PostgreSQL probe; set DB_STATEMENT_TIMEOUT_MS=100 and DB_LOCK_TIMEOUT_MS=50"]
async fn database_recovery() {
    const URL: &str = "postgresql://moh@127.0.0.1:55432/groupbot_audit";
    let settings = Settings::connect(URL).await.unwrap();
    settings
        .try_set_value(-999_002, "audit_restart", "fixture_reset")
        .await
        .unwrap();
    assert!(
        settings
            .try_set_value(-999_002, "audit_restart", "durable")
            .await
            .unwrap()
    );
    let control = PgPool::connect(URL).await.unwrap();
    let mut held = Vec::new();
    for _ in 0..settings.pool.options().get_max_connections() {
        held.push(settings.pool.acquire().await.unwrap());
    }
    let mut victim = held.pop().unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *victim)
        .await
        .unwrap();
    let killed: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .fetch_one(&control)
        .await
        .unwrap();
    assert!(killed);
    assert!(sqlx::query("SELECT 1").execute(&mut *victim).await.is_err());
    let _ = victim.close().await;
    let started = Instant::now();
    let mut replacement = settings.pool.acquire().await.unwrap();
    let replacement_ms = started.elapsed().as_secs_f64() * 1000.0;
    let limits: (String, String) = sqlx::query_as(
        "SELECT current_setting('statement_timeout'), current_setting('lock_timeout')",
    )
    .fetch_one(&mut *replacement)
    .await
    .unwrap();
    assert_eq!(limits, ("100ms".into(), "50ms".into()));
    drop(replacement);
    drop(held);
    settings.pool.close().await;
    drop(settings);
    let reloaded = Settings::connect(URL).await.unwrap();
    assert_eq!(
        reloaded.value(-999_002, "audit_restart").as_deref(),
        Some("durable")
    );
    reloaded.pool.close().await;
    drop(reloaded);

    let owner = Settings::connect_with_chat_limit_and_process_lock(URL, Some(50_000))
        .await
        .unwrap();
    let duplicate_rejected = Settings::connect_with_chat_limit_and_process_lock(URL, Some(50_000))
        .await
        .is_err();
    assert!(duplicate_rejected);
    let owner_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **owner._process_lock.lock().await.as_mut().unwrap())
        .await
        .unwrap();
    sqlx::query("SELECT pg_terminate_backend($1)")
        .bind(owner_pid)
        .execute(&control)
        .await
        .unwrap();
    let successor = Settings::connect_with_chat_limit_and_process_lock(URL, Some(50_000))
        .await
        .unwrap();
    let unfenced_old_owner_write = owner
        .try_set_value(-999_002, "audit_restart", "unfenced")
        .await
        .is_ok();
    assert!(!unfenced_old_owner_write);
    assert_eq!(
        successor.value(-999_002, "audit_restart").as_deref(),
        Some("durable")
    );
    println!(
        "PERF {}",
        serde_json::json!({"kind":"database_recovery",
        "replacement_ms":replacement_ms,"replacement_timeouts":limits,
        "committed_setting_survives_reload":true,"duplicate_owner_rejected":duplicate_rejected,
        "old_owner_can_write_after_lock_loss":unfenced_old_owner_write,
        "successor_mirror_is_stale":false})
    );
}

#[tokio::test]
#[ignore = "isolated PostgreSQL probe; PERF_DB_POOL/PERF_DB_CONCURRENCY/PERF_DB_OPERATIONS optional"]
async fn durable_counts_pool_saturation() {
    const URL: &str = "postgresql://moh@127.0.0.1:55432/groupbot_audit";
    let pool_size = std::env::var("PERF_DB_POOL")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(8)
        .clamp(2, 64);
    let concurrency = std::env::var("PERF_DB_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(32)
        .clamp(1, 256);
    let operations = std::env::var("PERF_DB_OPERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, 1_000);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(pool_size)
        .min_connections(pool_size.min(2))
        .acquire_timeout(Duration::from_secs(2))
        .connect(URL)
        .await
        .unwrap();
    let baseline: i64 = sqlx::query_scalar("SELECT counter_rows FROM durable_counts WHERE id = 0")
        .fetch_one(&pool)
        .await
        .unwrap();

    let started = Instant::now();
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..concurrency {
        let pool = pool.clone();
        workers.spawn(async move {
            let mut latencies = Vec::with_capacity(operations);
            let mut failures = 0_u64;
            for _ in 0..operations {
                let operation_started = Instant::now();
                let result = async {
                    let mut tx = pool.begin().await?;
                    sqlx::query(
                        "UPDATE durable_counts SET counter_rows = counter_rows + 1 WHERE id = 0",
                    )
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await
                }
                .await;
                latencies.push(operation_started.elapsed().as_secs_f64() * 1000.0);
                if result.is_err() {
                    failures += 1;
                }
            }
            (latencies, failures)
        });
    }
    let mut latencies = Vec::with_capacity(concurrency * operations);
    let mut failures = 0_u64;
    while let Some(result) = workers.join_next().await {
        let (mut worker_latencies, worker_failures) = result.unwrap();
        latencies.append(&mut worker_latencies);
        failures += worker_failures;
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    let final_count: i64 =
        sqlx::query_scalar("SELECT counter_rows FROM durable_counts WHERE id = 0")
            .fetch_one(&pool)
            .await
            .unwrap();
    let applied = final_count.saturating_sub(baseline);
    sqlx::query("UPDATE durable_counts SET counter_rows = $1 WHERE id = 0")
        .bind(baseline)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(applied, i64::try_from(latencies.len()).unwrap());

    latencies.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| -> f64 {
        if latencies.is_empty() {
            return 0.0;
        }
        let index = ((latencies.len() - 1) as f64 * fraction).round() as usize;
        latencies[index]
    };
    let (observed_size, idle) = {
        let size = pool.size();
        (size, pool.num_idle())
    };
    println!(
        "PERF {}",
        serde_json::json!({
            "kind": "durable_counts_pool_saturation",
            "pool_configured": pool_size,
            "pool_observed": observed_size,
            "pool_idle_after": idle,
            "concurrency": concurrency,
            "operations_per_worker": operations,
            "operations": latencies.len(),
            "elapsed_ms": elapsed_ms,
            "throughput_ops_s": (latencies.len() as f64) / (elapsed_ms / 1000.0).max(f64::MIN_POSITIVE),
            "p50_ms": percentile(0.50),
            "p95_ms": percentile(0.95),
            "p99_ms": percentile(0.99),
            "failures": failures,
            "counter_rows_restored": applied == i64::try_from(latencies.len()).unwrap() && baseline == sqlx::query_scalar::<_, i64>("SELECT counter_rows FROM durable_counts WHERE id = 0").fetch_one(&pool).await.unwrap(),
        })
    );
    pool.close().await;
}
