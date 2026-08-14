mod handlers;
mod state;

use std::env;
use std::sync::Arc;

use grammers_client::sender::{ConnectionParams, UpdatesConfiguration};
use grammers_client::session::storages::SqliteSession;
use grammers_client::{Client, SenderPool};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use handlers::Ctx;
use state::Settings;

type Result = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

const SESSION_FILE: &str = "groupbot.session";

const USER_SESSION_FILE: &str = "cleaner.session";
const SETTINGS_FILE: &str = "groupbot.data";

const MAX_CONCURRENT_UPDATES: usize = 512;

const NIGHT_CHECK: std::time::Duration = std::time::Duration::from_secs(60);

const LOG_FLUSH: std::time::Duration = std::time::Duration::from_secs(3);

const TEMP_MEDIA_SWEEP: std::time::Duration = std::time::Duration::from_secs(30);

const STATS_FLUSH: std::time::Duration = std::time::Duration::from_secs(60);

const UPDATES_CHANNEL_CAPACITY: std::num::NonZeroUsize =
    std::num::NonZeroUsize::new(4096).unwrap();

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

    let settings = Settings::connect(&env::var("DATABASE_URL")?).await?;
    match settings.import_file(SETTINGS_FILE).await {
        0 => {}
        n => println!("imported {n} settings from {SETTINGS_FILE}"),
    }
    let ctx = Arc::new(Ctx::new(client.clone(), Arc::new(settings)));
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
        tokio::spawn(async move {
            let mut updates = match user_client
                .stream_updates(user_updates, UpdatesConfiguration { catch_up: false })
                .await
            {
                Ok(updates) => updates,
                Err(e) => {
                    eprintln!("cleaner updates: {e}");
                    return;
                }
            };
            loop {
                match updates.next().await {
                    Ok(grammers_client::update::Update::NewMessage(message)) => {
                        if !handlers::bots::cleaner_candidate(&cleaner_ctx, &message) {
                            continue;
                        }
                        let ctx = Arc::clone(&cleaner_ctx);
                        tokio::spawn(async move {
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
        });
    } else {
        drop(user_updates);
    }

    handlers::join::prime(&ctx).await;
    handlers::tempmedia::restore(&ctx).await;

    let night_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(NIGHT_CHECK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            handlers::extras::run_night(&night_ctx).await;
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

    println!("running");
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_UPDATES));
    let mut tasks = JoinSet::new();
    let mut updates = client

        .stream_updates(updates, UpdatesConfiguration { catch_up: true })
        .await?;
    loop {
        while let Some(finished) = tasks.try_join_next() {
            if let Err(e) = finished {
                eprintln!("update handler failed: {e}");
            }
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            update = updates.next() => {
                let update = update?;
                let ctx = Arc::clone(&ctx);
                let permits = Arc::clone(&permits);
                tasks.spawn(async move {
                    let _permit = permits.acquire().await;
                    handlers::dispatch(&ctx, update).await;
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
