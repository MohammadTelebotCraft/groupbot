mod handlers;
mod miniapp;
mod state;

use std::collections::HashSet;
use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use grammers_client::sender::{ConnectionParams, UpdatesConfiguration};
use grammers_client::session::storages::SqliteSession;
use grammers_client::{Client, SenderPool};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use handlers::Ctx;
use state::Settings;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Default)]
struct UpdateTelemetry {
    received: AtomicU64,
    completed: AtomicU64,
}

const SESSION_FILE: &str = "groupbot.session";

const USER_SESSION_FILE: &str = "cleaner.session";
const SETTINGS_FILE: &str = "groupbot.data";

const DEFAULT_UPDATE_CONCURRENCY: usize = 512;

const DEFAULT_MAX_SHARD_CHATS: usize = 50_000;

const DEFAULT_CLEANER_CONCURRENCY: usize = 128;

const NIGHT_CHECK: std::time::Duration = std::time::Duration::from_secs(60);

const LOG_FLUSH: std::time::Duration = std::time::Duration::from_secs(3);

const TEMP_MEDIA_SWEEP: std::time::Duration = std::time::Duration::from_secs(30);
pub const DEFERRED_SWEEP_SECS: u32 = 5;
const DEFERRED_SWEEP: std::time::Duration =
    std::time::Duration::from_secs(DEFERRED_SWEEP_SECS as u64);

const STATS_FLUSH: std::time::Duration = std::time::Duration::from_secs(60);
const SAMPLE_FLUSH: std::time::Duration = std::time::Duration::from_secs(60);

const CHAT_SWEEP: std::time::Duration = std::time::Duration::from_secs(600);

const CHAT_IDLE: std::time::Duration = std::time::Duration::from_secs(3600);

const UPDATES_CHANNEL_CAPACITY: std::num::NonZeroUsize = std::num::NonZeroUsize::new(4096).unwrap();

fn configured_cap(name: &str, default: usize, minimum: usize, maximum: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
        .clamp(minimum, maximum)
}

fn load_owned_chats(path: &str, max_chats: usize) -> Result<Arc<HashSet<i64>>> {
    let contents = std::fs::read_to_string(path)?;
    let mut chats = HashSet::new();
    for (line_number, line) in contents.lines().enumerate() {
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        if value.split_whitespace().count() != 1 {
            return Err(format!(
                "{path}:{}: expected one chat id per line; filter shard_route TSV output first",
                line_number + 1
            )
            .into());
        }
        let chat: i64 = value
            .parse()
            .map_err(|_| format!("{path}:{}: invalid BIGINT chat id", line_number + 1))?;
        if chat == 0 {
            return Err(format!("{path}:{}: chat id 0 is global state", line_number + 1).into());
        }
        if !chats.insert(chat) {
            return Err(format!("{path}:{}: duplicate chat id {chat}", line_number + 1).into());
        }
        if chats.len() > max_chats {
            return Err(format!("{path}: more than MAX_SHARD_CHATS={max_chats} chat ids").into());
        }
    }
    if chats.is_empty() {
        return Err(format!("{path}: route file is empty").into());
    }
    Ok(Arc::new(chats))
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn capacity_probe() -> Result {
    let database_url = env::var("DATABASE_URL")?;
    let started = std::time::Instant::now();
    let settings = Settings::connect(&database_url).await?;
    let load_ms = started.elapsed().as_millis();
    let chats = settings.chats();
    let passes = env::var("CAPACITY_PROBE_PASSES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3)
        .clamp(1, 100);
    let scan_started = std::time::Instant::now();
    let mut checked = 0usize;
    let mut present = 0usize;
    for _ in 0..passes {
        for chat in chats.iter().copied() {
            checked += 1;
            present += settings.with_chat(chat, |values| {
                ["owner", "flood", "night", "report_at", "auto_purge_at"]
                    .into_iter()
                    .filter(|key| values.value(key).is_some() || values.is_locked(key))
                    .count()
            });
        }
    }
    println!(
        "capacity-probe: chats={} settings_rows={} settings_bytes={} passes={} load_ms={} scan_ms={} checked={} present={}",
        settings.chat_count(),
        settings.setting_count(),
        settings.setting_bytes(),
        passes,
        load_ms,
        scan_started.elapsed().as_millis(),
        checked,
        present,
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result {
    simple_logger::SimpleLogger::new()
        .with_level(match env::var("LOG").as_deref() {
            Ok("debug") => log::LevelFilter::Debug,
            Ok("trace") => log::LevelFilter::Trace,
            Ok("info") => log::LevelFilter::Info,
            _ => log::LevelFilter::Warn,
        })
        .init()?;

    if env::args().skip(1).any(|arg| arg == "--capacity-probe") {
        return capacity_probe().await;
    }
    if env::args().skip(1).any(|arg| arg == "--model-probe") {
        let classifier = handlers::nsfw::capacity_probe();
        let ocr = handlers::ocr::capacity_probe();
        let vision = handlers::vision::present();
        println!(
            "model-probe: nsfw_pools={} ocr_pools={} vision_pool={}",
            classifier,
            ocr,
            usize::from(vision),
        );
        return Ok(());
    }

    let update_concurrency =
        configured_cap("UPDATE_CONCURRENCY", DEFAULT_UPDATE_CONCURRENCY, 32, 4096);
    let cleaner_concurrency =
        configured_cap("CLEANER_CONCURRENCY", DEFAULT_CLEANER_CONCURRENCY, 16, 1024);
    log::info!(
        "runtime admission: updates={update_concurrency}, cleaner={cleaner_concurrency}, update_queue={}",
        UPDATES_CHANNEL_CAPACITY
    );

    let api_id = env::var("TG_ID")?.parse()?;
    let api_hash = env::var("TG_HASH")?;
    let token = env::var("TG_BOT_TOKEN")?;

    let session = Arc::new(SqliteSession::open(SESSION_FILE).await?);

    let params = ConnectionParams {
        updates_channel_capacity: UPDATES_CHANNEL_CAPACITY,
        ..Default::default()
    };
    let SenderPool {
        runner,
        updates,
        handle,
    } = SenderPool::with_configuration(Arc::clone(&session), api_id, params);
    let client = Client::new(handle.clone());
    let pool_task = tokio::spawn(runner.run());

    if !client.is_authorized().await? {
        client.bot_sign_in(&token, &api_hash).await?;
        println!("signed in");
    }

    handlers::premium::initialize(&client).await;

    let database_url = env::var("DATABASE_URL")?;
    let max_shard_chats =
        configured_cap("MAX_SHARD_CHATS", DEFAULT_MAX_SHARD_CHATS, 1_000, 500_000);
    let settings =
        Settings::connect_with_chat_limit_and_process_lock(&database_url, Some(max_shard_chats))
            .await?;
    match settings.import_file(SETTINGS_FILE).await {
        0 => {}
        n => println!("imported {n} settings from {SETTINGS_FILE}"),
    }
    let normalized = settings
        .normalize_fixed_image_filters(handlers::imgfilter::FIXED_MODEL_CUT)
        .await;
    if normalized > 0 {
        println!(
            "image filters: normalized {normalized} fixed cut(s) to {:.3}",
            handlers::imgfilter::FIXED_MODEL_CUT
        );
    }
    let space_flag = format!("vision:{}", handlers::vision::CHECKPOINT);
    let migrating = !settings.is_locked(0, &space_flag);
    if migrating {
        println!(
            "vision: the database's vectors are not from {}; migrating",
            handlers::vision::CHECKPOINT
        );
        let examples = settings.example_image_filter_keys().await;
        for (chat, name) in &examples {
            settings.delete_image_filter(*chat, name).await;
            settings
                .set(
                    *chat,
                    &format!("{}{name}", handlers::imgfilter::PREFIX),
                    false,
                )
                .await;
            println!(
                "vision: retired example filter «{name}» in {chat}; its pictures are from the old space"
            );
        }
        settings.clear_samples().await;
    }
    let fixed_filters = if migrating {
        settings.phrase_image_filter_keys().await
    } else {
        settings.fixed_image_filter_keys().await
    };
    let mut refreshed = 0usize;
    for (chat, name) in fixed_filters {
        let Some(row) = settings.image_filter(chat, &name).await else {
            continue;
        };
        let phrase = name.clone();
        let Some(vector) = tokio::task::spawn_blocking(move || handlers::imgtext::embed(&phrase))
            .await
            .ok()
            .flatten()
        else {
            continue;
        };
        let (bytes, scale) = handlers::imgfilter::quantize(&vector);
        if settings
            .save_image_filter(
                chat,
                &name,
                &bytes,
                scale,
                row.cut,
                row.rate,
                row.live,
                row.samples,
                row.calibrated,
            )
            .await
        {
            refreshed += 1;
        }
    }
    if refreshed > 0 {
        println!("image filters: refreshed {refreshed} phrase vector(s)");
    }
    if migrating && settings.set(0, &space_flag, true).await {
        println!(
            "vision: database vectors are now from {}",
            handlers::vision::CHECKPOINT
        );
    }
    let shard_chats = settings.chat_count();
    if shard_chats > max_shard_chats {
        return Err(format!(
            "shard has {shard_chats} configured chats, above MAX_SHARD_CHATS={max_shard_chats}; split the database across fleet shards"
        )
        .into());
    }
    let shard_name = env::var("SHARD_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty());
    let allowed_chats = env::var("SHARD_CHAT_IDS_FILE")
        .ok()
        .map(|path| load_owned_chats(&path, max_shard_chats))
        .transpose()?;
    if shard_name.is_some() && allowed_chats.is_none() {
        return Err(
            "SHARD_NAME is set, but SHARD_CHAT_IDS_FILE is missing; a templated fleet shard must have an ownership file"
                .into(),
        );
    }
    if let Some(allowed) = &allowed_chats
        && let Some(chat) = settings
            .chats()
            .into_iter()
            .find(|chat| !allowed.contains(chat))
    {
        return Err(format!(
            "database contains chat {chat}, but SHARD_CHAT_IDS_FILE does not assign it to this shard"
        )
        .into());
    }
    let ctx = Arc::new(Ctx::new_with_allowed_chats(
        client.clone(),
        Arc::new(settings),
        max_shard_chats,
        allowed_chats,
    ));
    ctx.set_bot_session(grammers_session::storages::erase(Arc::clone(&session)));
    match client.get_me().await {
        Ok(me) => ctx.set_me_id(me.id().bare_id_unchecked()),
        Err(e) => eprintln!("could not read the bot's own id: {e}"),
    }

    let user_session = Arc::new(SqliteSession::open(USER_SESSION_FILE).await?);
    let SenderPool {
        runner: user_runner,
        updates: user_updates,
        handle: user_handle,
    } = SenderPool::with_configuration(
        Arc::clone(&user_session),
        api_id,
        ConnectionParams {
            updates_channel_capacity: UPDATES_CHANNEL_CAPACITY,
            ..Default::default()
        },
    );

    let user_task = tokio::spawn(user_runner.run());
    let user_client = Client::new(user_handle);
    let mut cleaner_ready = false;
    let cleaner_permits = Arc::new(Semaphore::new(cleaner_concurrency));
    match user_client.is_authorized().await {
        Ok(true) => match user_client.get_me().await {
            Ok(me) => {
                ctx.set_cleaner_id(me.id().bare_id_unchecked());
                cleaner_ready = true;
                println!("cleaner signed in as {}", me.full_name());
            }
            Err(e) => eprintln!("cleaner: signed in but unreachable: {e}"),
        },
        Ok(false) => println!("cleaner not signed in — send «ورود کلینر» to the bot"),
        Err(e) => eprintln!("cleaner: {e}"),
    }
    ctx.set_user_client(user_client.clone());

    if cleaner_ready {
        let cleaner_ctx = Arc::clone(&ctx);
        let cleaner_permits = Arc::clone(&cleaner_permits);
        tokio::spawn(async move {
            let mut updates = match user_client
                .stream_updates(
                    user_updates,
                    UpdatesConfiguration {
                        catch_up: false,
                        drop_idle_channels: true,
                    },
                )
                .await
            {
                Ok(updates) => updates,
                Err(e) => {
                    eprintln!("cleaner updates: {e}");
                    return;
                }
            };
            let mut tasks = JoinSet::new();
            loop {
                while let Some(finished) = tasks.try_join_next() {
                    if let Err(e) = finished {
                        eprintln!("cleaner update handler failed: {e}");
                    }
                }
                match updates.next().await {
                    Ok(grammers_client::update::Update::NewMessage(message)) => {
                        if !handlers::bots::cleaner_candidate(&cleaner_ctx, &message) {
                            continue;
                        }
                        let permit = cleaner_permits
                            .clone()
                            .acquire_owned()
                            .await
                            .expect("the cleaner update semaphore is never closed");
                        let ctx = Arc::clone(&cleaner_ctx);
                        tasks.spawn(async move {
                            let _permit = permit;
                            handlers::bots::on_cleaner_message(&ctx, &message).await;
                        });
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("cleaner updates: {e}");
                        break;
                    }
                }
            }
            while let Some(finished) = tasks.join_next().await {
                if let Err(e) = finished {
                    eprintln!("cleaner update handler failed: {e}");
                }
            }
        });
    } else {
        drop(user_updates);
    }

    handlers::join::prime(&ctx).await;
    handlers::tempmedia::restore(&ctx).await;

    tokio::spawn(handlers::cases::run_retention(Arc::clone(&ctx)));
    tokio::spawn(handlers::autoconfig::recover_startup(Arc::clone(&ctx)));

    let cleaner_recommend_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::cleaner_setup::run_recommendations(&cleaner_recommend_ctx).await;
        }
    });

    let night_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::extras::run_night(&night_ctx).await;
        }
    });

    let timed_lock_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::locks::run_timed(&timed_lock_ctx).await;
        }
    });

    let report_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::stats::run_daily(&report_ctx).await;
        }
    });

    let purge_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::purge::run_auto(&purge_ctx).await;
        }
    });

    let media_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TEMP_MEDIA_SWEEP);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::tempmedia::sweep(&media_ctx).await;
        }
    });

    let deferred_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(DEFERRED_SWEEP);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::tempmedia::sweep_deferred(&deferred_ctx).await;
        }
    });

    let log_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(LOG_FLUSH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::log::flush(&log_ctx).await;
        }
    });

    let stats_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut flushes: u32 = 1;
        let mut tick = tokio::time::interval(STATS_FLUSH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::stats::flush(&stats_ctx).await;

            if flushes.is_multiple_of(1440) {
                handlers::stats::prune(&stats_ctx).await;
            }
            flushes += 1;
        }
    });

    let sweep_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(CHAT_SWEEP);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            match sweep_ctx.evict_idle(CHAT_IDLE) {
                0 => {}
                dropped => log::debug!("forgot the state of {dropped} quiet chats"),
            }
        }
    });

    ctx.load_samples().await;

    let permits = Arc::new(Semaphore::new(update_concurrency));
    let capacity_ctx = Arc::clone(&ctx);
    let update_capacity = Arc::clone(&permits);
    let cleaner_capacity = Arc::clone(&cleaner_permits);
    let update_telemetry = Arc::new(UpdateTelemetry::default());
    let capacity_telemetry = Arc::clone(&update_telemetry);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let snapshot = capacity_ctx.capacity_snapshot();
            let updates_received = capacity_telemetry.received.swap(0, Ordering::Relaxed);
            let updates_completed = capacity_telemetry.completed.swap(0, Ordering::Relaxed);
            let update_available = update_capacity.available_permits();
            let cleaner_available = cleaner_capacity.available_permits();
            let (db_connections, db_idle) = capacity_ctx.settings.pool_stats();
            let (counter_rows, note_rows) = capacity_ctx.settings.durable_counts().await;
            log::info!(
                "capacity: runtime_chats={} settings_chats={}/{} settings_rows={} settings_bytes={} counter_rows={} note_rows={} user_chats={} dirty={}/{}/{} pending_writes={} pending_drops={} deferred={} pending_admins={} deleted={} verdicts={} voice={}/{} outbound={}/{} update_active={}/{} updates_received={} updates_completed={} cleaner_active={}/{} db_connections={} db_idle={}",
                snapshot.runtime_chats,
                snapshot.settings_chats,
                max_shard_chats,
                snapshot.settings_rows,
                snapshot.settings_bytes,
                counter_rows,
                note_rows,
                snapshot.user_chats,
                snapshot.dirty_logs,
                snapshot.dirty_media,
                snapshot.dirty_stats,
                snapshot.pending_writes,
                snapshot.pending_drops,
                snapshot.deferred,
                snapshot.pending_admins,
                snapshot.deleted,
                snapshot.verdicts,
                snapshot.voice_verdicts,
                snapshot.filtered_voices,
                snapshot.outbound_active,
                snapshot.outbound_waiting,
                update_concurrency.saturating_sub(update_available),
                update_concurrency,
                updates_received,
                updates_completed,
                cleaner_concurrency.saturating_sub(cleaner_available),
                cleaner_concurrency,
                db_connections,
                db_idle,
            );
        }
    });

    let samples_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SAMPLE_FLUSH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            samples_ctx.flush_samples().await;
        }
    });

    let badges_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        handlers::stats::sweep_badges(&badges_ctx).await;
    });

    let miniapp_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        miniapp::spawn(miniapp_ctx).await;
    });

    println!("running");
    let mut tasks = JoinSet::new();
    let mut updates = client
        .stream_updates(
            updates,
            UpdatesConfiguration {
                catch_up: false,
                drop_idle_channels: true,
            },
        )
        .await?;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let update_telemetry = Arc::clone(&update_telemetry);
    loop {
        while let Some(finished) = tasks.try_join_next() {
            if let Err(e) = finished {
                eprintln!("update handler failed: {e}");
            }
        }
        let permit = tokio::select! {
            _ = &mut shutdown => break,
            permit = Arc::clone(&permits).acquire_owned() => {
                permit.expect("the update semaphore is never closed")
            }
        };
        tokio::select! {
            _ = &mut shutdown => break,
            update = updates.next() => {
                let update = update?;
                update_telemetry.received.fetch_add(1, Ordering::Relaxed);
                let ctx = Arc::clone(&ctx);
                let telemetry = Arc::clone(&update_telemetry);
                tasks.spawn(async move {
                    let _permit = permit;
                    handlers::dispatch(&ctx, update).await;
                    telemetry.completed.fetch_add(1, Ordering::Relaxed);
                });
            },
        }
    }

    updates.sync_update_state().await?;
    handle.quit();
    user_task.abort();
    let _ = pool_task.await;
    while let Some(finished) = tasks.join_next().await {
        if let Err(e) = finished {
            eprintln!("update handler failed: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::load_owned_chats;
    use std::fs;

    #[test]
    fn route_file_is_strict_and_bounded() {
        let path = std::env::temp_dir().join(format!(
            "groupbot-route-{}-{}.txt",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, "# shard\n-100\n-200\n").unwrap();
        let loaded = load_owned_chats(path.to_str().unwrap(), 2).unwrap();
        assert!(loaded.contains(&-100));
        assert!(loaded.contains(&-200));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn route_file_rejects_tsv_and_duplicates() {
        let path = std::env::temp_dir().join(format!(
            "groupbot-route-invalid-{}-{}.txt",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, "-100\talpha\n-100\n").unwrap();
        assert!(load_owned_chats(path.to_str().unwrap(), 10).is_err());
        fs::remove_file(path).unwrap();
    }
}
