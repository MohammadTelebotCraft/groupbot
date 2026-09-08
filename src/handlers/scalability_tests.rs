use super::*;
use grammers_client::{SenderPool, tl};
use grammers_session::{storages::MemorySession, updates::UpdatesLike};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;

struct Meter;
static METER_ON: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if METER_ON.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if METER_ON.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(size as u64, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;

#[test]
fn exact_dependency_retry_policy_bounds_flood_and_io_retries() {
    use grammers_client::sender::{
        AutoSleep, InvocationError, RetryContext, RetryPolicy, RpcError,
    };
    use std::ops::ControlFlow;
    let policy = AutoSleep::default();
    let flood = |seconds, failures| RetryContext {
        fail_count: std::num::NonZeroU32::new(failures).unwrap(),
        slept_so_far: Duration::ZERO,
        error: InvocationError::Rpc(RpcError {
            code: 420,
            name: "FLOOD_WAIT".into(),
            value: Some(seconds),
            caused_by: None,
        }),
    };
    for _ in 0..1000 {
        assert_eq!(
            policy.should_retry(&flood(60, 1)),
            ControlFlow::Continue(Duration::from_secs(60))
        );
        assert_eq!(policy.should_retry(&flood(61, 1)), ControlFlow::Break(()));
        assert_eq!(policy.should_retry(&flood(1, 2)), ControlFlow::Break(()));
    }
    let io = |failures| RetryContext {
        fail_count: std::num::NonZeroU32::new(failures).unwrap(),
        slept_so_far: Duration::ZERO,
        error: InvocationError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "offline fault",
        )),
    };
    assert_eq!(
        policy.should_retry(&io(1)),
        ControlFlow::Continue(Duration::from_secs(1))
    );
    assert_eq!(policy.should_retry(&io(2)), ControlFlow::Break(()));
}

fn rss_kib() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .unwrap()
}

fn cpu_seconds() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    (fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()) as f64
        / std::env::var("PERF_CLK_TCK")
            .unwrap()
            .parse::<f64>()
            .unwrap()
}

fn thread_count() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Threads:")?.trim().parse().ok())
        .unwrap()
}

fn sample_latency(samples: &mut Vec<u64>, seen: &mut usize, stride: usize, value: u64) {
    if (*seen).is_multiple_of(stride) {
        samples.push(value);
    }
    *seen += 1;
}

fn replay_message(
    template: &grammers_client::update::Message,
    fresh: bool,
    sequence: usize,
) -> grammers_client::update::Message {
    let mut message = template.clone();
    if fresh {
        let date = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i32;
        let update = |raw: &mut tl::enums::Message| {
            if let tl::enums::Message::Message(raw) = raw {
                raw.date = date;
                raw.id = sequence as i32 + 1;
            }
        };
        let inner: &mut grammers_client::message::Message = &mut message;
        update(&mut inner.raw);
        if let tl::enums::Update::NewMessage(raw) = &mut message.raw {
            update(&mut raw.message);
        }
    }
    message
}

fn raw_message(chat: i64, id: i32, text: &str) -> tl::enums::Message {
    tl::types::Message {
        out: false,
        mentioned: false,
        media_unread: false,
        silent: false,
        post: false,
        from_scheduled: false,
        legacy: false,
        edit_hide: false,
        pinned: false,
        noforwards: false,
        invert_media: false,
        offline: false,
        video_processing_pending: false,
        paid_suggested_post_stars: false,
        paid_suggested_post_ton: false,
        id,
        from_id: Some(tl::types::PeerUser { user_id: 99 }.into()),
        from_boosts_applied: None,
        from_rank: None,
        peer_id: tl::types::PeerChat { chat_id: -chat }.into(),
        saved_peer_id: None,
        fwd_from: None,
        via_bot_id: None,
        via_business_bot_id: None,
        guestchat_via_from: None,
        reply_to: None,
        date: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i32,
        message: text.into(),
        media: None,
        reply_markup: None,
        entities: None,
        views: None,
        forwards: None,
        replies: None,
        edit_date: None,
        post_author: None,
        grouped_id: None,
        reactions: None,
        restriction_reason: None,
        ttl_period: None,
        quick_reply_shortcut_id: None,
        effect: None,
        factcheck: None,
        report_delivery_until_date: None,
        paid_message_stars: None,
        suggested_post: None,
        schedule_repeat_period: None,
        summary_from_language: None,
        rich_message: None,
    }
    .into()
}

async fn fixture(
    groups: usize,
    active: usize,
) -> (
    Arc<Ctx>,
    Vec<grammers_client::update::Message>,
    sqlx::PgPool,
    usize,
    usize,
) {
    const URL: &str = "postgresql://moh@127.0.0.1:55432/groupbot_audit";
    let db = sqlx::PgPool::connect(URL).await.unwrap();
    drop(Settings::connect(URL).await.unwrap());
    sqlx::query("TRUNCATE durable_chats CASCADE")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
         SELECT -g, 0, 0 FROM generate_series(1,$1) g",
    )
    .bind(groups as i32)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO settings (chat_id,key,value) SELECT -g,k,v FROM generate_series(1,$1) g CROSS JOIN (VALUES ('hash','0'),('owner','42'),('links',''),('photo','')) s(k,v)")
        .bind(groups as i32).execute(&db).await.unwrap();
    let before = rss_kib();
    let settings = Arc::new(if std::env::var("PERF_OWNERSHIP").as_deref() == Ok("1") {
        Settings::connect_with_chat_limit_and_process_lock(URL, Some(groups + 1))
            .await
            .unwrap()
    } else {
        Settings::connect(URL).await.unwrap()
    });
    let mirror_rss = rss_kib().saturating_sub(before);
    let session = Arc::new(MemorySession::default());
    let pool = SenderPool::new(Arc::clone(&session), 1);
    let client = Client::new(pool.handle.clone());
    let ctx = Arc::new(Ctx::new_with_allowed_chats(
        client.clone(),
        settings,
        grammers_session::storages::erase(session),
        BotIdentity::new(std::num::NonZeroI64::MIN, None),
        RuntimeConfig::for_test(),
        groups + 1,
        None,
    ));
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let mut updates = client
        .stream_updates(rx, grammers_client::sender::UpdatesConfiguration::default())
        .await
        .unwrap();
    let mut messages = Vec::new();
    let before_active = rss_kib();
    for index in 1..=active {
        let chat = -(index as i64);
        let state = ctx.state(chat);
        *state.peer.write().unwrap() = Some(PeerId::chat(index as i64).unwrap().to_ambient_ref());
        ctx.cache_admins(chat, HashSet::from([42]));
        tx.send(UpdatesLike::Updates(
            tl::types::Updates {
                updates: vec![
                    tl::types::UpdateNewMessage {
                        message: raw_message(
                            chat,
                            index as i32,
                            "سلام دوستان امروز حال شما چطور است",
                        ),
                        pts: index as i32,
                        pts_count: 1,
                    }
                    .into(),
                ],
                users: vec![tl::types::UserEmpty { id: 99 }.into()],
                chats: vec![
                    tl::types::ChatForbidden {
                        id: index as i64,
                        title: "audit group".into(),
                    }
                    .into(),
                ],
                date: 1,
                seq: index as i32,
            }
            .into(),
        ))
        .await
        .unwrap();
        let Update::NewMessage(message) = updates.next().await.unwrap() else {
            panic!("fixture")
        };
        dispatch(&ctx, Update::NewMessage(message.clone())).await;
        messages.push(message);
    }
    let active_rss = rss_kib().saturating_sub(before_active);
    drop(pool);
    while !ctx.dirty.stats.0.lock().unwrap().is_empty() {
        ctx.take_stats();
    }
    (ctx, messages, db, mirror_rss, active_rss)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "offline load test; run tools/scalability_pg.sh first"]
async fn replay_dispatch() {
    let groups = std::env::var("PERF_GROUPS")
        .unwrap_or("10000".into())
        .parse::<usize>()
        .unwrap();
    let active = std::env::var("PERF_ACTIVE")
        .unwrap_or("1000".into())
        .parse::<usize>()
        .unwrap();
    let count = std::env::var("PERF_UPDATES")
        .unwrap_or("100000".into())
        .parse::<usize>()
        .unwrap();
    let (ctx, messages, db, mirror_rss, active_rss) = fixture(groups, active).await;
    sqlx::query("SELECT pg_stat_statements_reset()")
        .execute(&db)
        .await
        .unwrap();
    ALLOCS.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    METER_ON.store(
        std::env::var("PERF_ALLOC").as_deref() == Ok("1"),
        Ordering::Relaxed,
    );
    let start_cpu = cpu_seconds();
    let started = Instant::now();
    let permits = Arc::new(tokio::sync::Semaphore::new(512));
    let mut tasks = tokio::task::JoinSet::new();
    let mut latency = Vec::with_capacity(count.min(100000));
    let mut latency_seen = 0;
    let stride = count.div_ceil(100000).max(1);
    let mut max_tasks = 0;
    let fair = std::env::var("PERF_DISPATCHER").as_deref() == Ok("fair");
    let mut queue_peak = 0;
    if fair {
        let mut dispatcher = crate::dispatcher::Dispatcher::new(512, 8, 4096);
        let run = |message| {
            let ctx = ctx.clone();
            async move {
                dispatch(&ctx, Update::NewMessage(message)).await;
            }
        };
        for index in 0..count {
            while let Some(done) = dispatcher.try_join_next() {
                done.result.unwrap();
                sample_latency(
                    &mut latency,
                    &mut latency_seen,
                    stride,
                    done.latency.as_nanos() as u64,
                );
            }
            dispatcher.start(run);
            if !dispatcher.has_room() {
                let done = dispatcher.join_next().await.unwrap();
                done.result.unwrap();
                sample_latency(
                    &mut latency,
                    &mut latency_seen,
                    stride,
                    done.latency.as_nanos() as u64,
                );
                dispatcher.start(run);
            }
            dispatcher.push(
                -(index as i64 % active as i64 + 1),
                replay_message(&messages[index % active], count > 100000, index),
            );
            max_tasks = max_tasks.max(dispatcher.active());
            queue_peak = queue_peak.max(dispatcher.pending());
        }
        while !dispatcher.is_empty() {
            dispatcher.start(run);
            if let Some(done) = dispatcher.join_next().await {
                done.result.unwrap();
                sample_latency(
                    &mut latency,
                    &mut latency_seen,
                    stride,
                    done.latency.as_nanos() as u64,
                );
            }
        }
    } else {
        for index in 0..count {
            while let Some(result) = tasks.try_join_next() {
                sample_latency(&mut latency, &mut latency_seen, stride, result.unwrap());
            }
            let received = Instant::now();
            let permit = permits.clone().acquire_owned().await.unwrap();
            let ctx = ctx.clone();
            let message = replay_message(&messages[index % active], count > 100000, index);
            tasks.spawn(async move {
                let _permit = permit;
                dispatch(&ctx, Update::NewMessage(message)).await;
                received.elapsed().as_nanos() as u64
            });
            max_tasks = max_tasks.max(tasks.len());
        }
        while let Some(result) = tasks.join_next().await {
            sample_latency(&mut latency, &mut latency_seen, stride, result.unwrap());
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - start_cpu;
    METER_ON.store(false, Ordering::Relaxed);
    let allocations = ALLOCS.load(Ordering::Relaxed);
    let allocated_bytes = ALLOC_BYTES.load(Ordering::Relaxed);
    latency.sort_unstable();
    let queries: i64 = sqlx::query_scalar("SELECT coalesce(sum(calls),0)::bigint FROM pg_stat_statements WHERE query NOT LIKE '%pg_stat_statements%'")
        .fetch_one(&db).await.unwrap();
    let mut counted = 0u64;
    while !ctx.dirty.stats.0.lock().unwrap().is_empty() {
        let (_, counts) = ctx.take_stats();
        counted += counts.values().map(|(n, _)| *n).sum::<u64>();
    }
    assert_eq!(
        counted, count as u64,
        "every replayed message must reach statistics"
    );
    println!(
        "PERF {}",
        serde_json::json!({"kind":"dispatch", "groups":groups,"active":active,"updates":count,
        "seconds":elapsed,"updates_per_second":count as f64 / elapsed,"cpu_seconds":cpu,"cpu_percent":100.0*cpu/elapsed,
        "p50_us":latency[latency.len()/2] as f64/1000.0,"p95_us":latency[latency.len()*95/100] as f64/1000.0,"p99_us":latency[latency.len()*99/100] as f64/1000.0,
        "latency_samples":latency.len(),"latency_sample_stride":stride,
        "dispatcher":if fair {"fair"} else {"legacy"},"application_queue_peak":queue_peak,
        "allocation_meter":std::env::var("PERF_ALLOC").as_deref() == Ok("1"),
        "rss_kib":rss_kib(),"mirror_delta_kib":mirror_rss,"active_fixture_delta_kib":active_rss,
        "chat_state_bytes":std::mem::size_of::<ChatState>(),"allocations_per_update":allocations as f64/count as f64,
        "allocated_bytes_per_update":allocated_bytes as f64/count as f64,"db_queries":queries,"max_joinset":max_tasks,"counted":counted,
        "telegram_calls":0,"telegram":"disconnected; non-action workload"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "fresh-process idle registered-group cost; run tools/scalability_pg.sh first"]
async fn idle_registered_groups() {
    let groups = std::env::var("PERF_GROUPS")
        .unwrap_or("10000".into())
        .parse::<usize>()
        .unwrap();
    let before = rss_kib();
    let start_cpu = cpu_seconds();
    let started = Instant::now();
    let (ctx, messages, _db, mirror_rss, active_rss) = fixture(groups, 0).await;
    assert!(messages.is_empty());
    let built_ms = started.elapsed().as_secs_f64() * 1000.0;
    let settled_rss = rss_kib();
    let idle_cpu_start = cpu_seconds();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let idle_cpu = cpu_seconds() - idle_cpu_start;
    let snapshot = ctx.capacity_snapshot();
    assert_eq!(snapshot.runtime_chats, 0);
    assert_eq!(snapshot.settings_chats, groups);
    println!(
        "PERF {}",
        serde_json::json!({
            "kind": "idle_registered_groups",
            "groups": groups,
            "runtime_chats": snapshot.runtime_chats,
            "settings_chats": snapshot.settings_chats,
            "chat_state_bytes": std::mem::size_of::<ChatState>(),
            "process_rss_before_kib": before,
            "process_rss_settled_kib": settled_rss,
            "mirror_delta_kib": mirror_rss,
            "active_fixture_delta_kib": active_rss,
            "build_ms": built_ms,
            "build_cpu_seconds": cpu_seconds() - start_cpu,
            "idle_observation_seconds": 2,
            "idle_cpu_seconds": idle_cpu,
            "threads": thread_count(),
            "per_group_mirror_bytes": if groups == 0 { 0.0 } else { mirror_rss as f64 * 1024.0 / groups as f64 },
            "per_group_tasks": 0,
            "per_group_timers": 0,
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "offline microbenchmark; run tools/scalability_pg.sh first"]
async fn scan_allocations() {
    let (ctx, messages, _, _, _) = fixture(10000, 1).await;
    let view = locks::View::new(&messages[0]);
    black_box(locks::scan(&ctx, -1, &view));
    let count = 500_000;
    ALLOCS.store(0, Ordering::Relaxed);
    METER_ON.store(true, Ordering::Relaxed);
    let start = Instant::now();
    for _ in 0..count {
        black_box(locks::scan(&ctx, -1, black_box(&view)));
    }
    let seconds = start.elapsed().as_secs_f64();
    METER_ON.store(false, Ordering::Relaxed);
    println!(
        "PERF {}",
        serde_json::json!({"kind":"lock_scan", "iterations":count,"seconds":seconds,
        "ns_per_scan":seconds*1e9/count as f64,"allocations_per_scan":ALLOCS.load(Ordering::Relaxed) as f64/count as f64})
    );
}

#[test]
#[ignore = "CPU microbenchmark of the actual flood/raid window"]
fn flood_window_cost() {
    for size in [32, EVENTS_PER_SUBJECT_MAX] {
        let mut times = VecDeque::from(vec![Instant::now(); size]);
        let iterations = 20_000;
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(record_event(&mut times, Duration::from_secs(600)));
            while times.len() > size {
                times.pop_front();
            }
        }
        println!(
            "PERF {}",
            serde_json::json!({"kind":"flood_window", "occupancy":size,
            "iterations":iterations,"ns_per_update":start.elapsed().as_secs_f64()*1e9/iterations as f64})
        );
    }
}

#[test]
#[ignore = "fresh-process measured timestamp-ring storage at fixed subject population"]
fn event_ring_memory() {
    let legacy = std::env::var("PERF_EVENT_CAP").as_deref() == Ok("legacy");
    let capacities = if legacy {
        [4096, 4096]
    } else {
        [FLOOD_EVENTS_MAX, REMOVAL_EVENTS_MAX]
    };
    let before = rss_kib();
    let mut queues = Vec::new();
    let now = Instant::now();
    let start = Instant::now();
    for cap in capacities {
        for _ in 0..1000 {
            let mut queue = VecDeque::new();
            for _ in 0..5000 {
                record_event_at(&mut queue, Duration::from_secs(600), cap, now);
            }
            queues.push(queue);
        }
    }
    let retained: usize = queues
        .iter()
        .map(|q| q.capacity() * std::mem::size_of::<Instant>())
        .sum();
    println!(
        "PERF {}",
        serde_json::json!({"kind":"event_ring_memory","legacy_capacity":legacy,
        "subjects_per_map":1000,"maps":2,"events":10000000,"seconds":start.elapsed().as_secs_f64(),
        "timestamp_buffer_bytes":retained,"rss_delta_kib":rss_kib().saturating_sub(before),"capacities":capacities})
    );
    black_box(queues);
}

#[test]
#[ignore = "isolated mutex contention probe; no Telegram or database"]
fn counter_lock_contention() {
    for groups in [1, 1000] {
        let states: Vec<_> = (0..groups)
            .map(|_| Arc::new(ChatState::default()))
            .collect();
        let start = Instant::now();
        let mut waits = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|worker| {
                    let states = &states;
                    scope.spawn(move || {
                        let mut waits = Vec::with_capacity(20000);
                        for sequence in 0..20000 {
                            let state = &states[(sequence * 8 + worker) % groups];
                            let start = Instant::now();
                            let mut counts = state.counts.lock().unwrap();
                            waits.push(start.elapsed().as_nanos() as u64);
                            counts.entry(42).or_insert_with(|| (0, String::new())).0 += 1;
                        }
                        waits
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        let seconds = start.elapsed().as_secs_f64();
        waits.sort_unstable();
        let percentiles = [50, 95, 99].map(|p| waits[(waits.len() - 1) * p / 100]);
        let sum: u64 = states
            .iter()
            .map(|s| {
                s.counts
                    .lock()
                    .unwrap()
                    .values()
                    .map(|(n, _)| *n)
                    .sum::<u64>()
            })
            .sum();
        assert_eq!(sum, 160000);
        println!(
            "PERF {}",
            serde_json::json!({"kind":"counter_lock_contention",
            "groups":groups,"threads":8,"operations":sum,"seconds":seconds,
            "wait_ns_p50_p95_p99":percentiles,"total_thread_wait_ms":waits.iter().sum::<u64>() as f64/1e6,
            "instant_bytes":std::mem::size_of::<Instant>()})
        );
    }
}

#[test]
fn flood_expiration_keeps_the_live_suffix_and_supports_shorter_windows() {
    let now = Instant::now();
    let mut times = VecDeque::from([
        now - Duration::from_secs(30),
        now - Duration::from_secs(10),
        now,
    ]);
    assert_eq!(record_event(&mut times, Duration::from_secs(20)), 3);
    assert_eq!(times.front().copied(), Some(now - Duration::from_secs(10)));
    assert_eq!(record_event(&mut times, Duration::from_secs(5)), 3);
    assert_eq!(times.front().copied(), Some(now));
    assert!(
        times
            .iter()
            .zip(times.iter().skip(1))
            .all(|(left, right)| left <= right)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "set VOICE_MONITOR_SCRIPT=tools/scalability_voice_stub.py VOICE_WORKERS=4"]
async fn voice_worker_parallelism() {
    assert_eq!(
        std::env::var("VOICE_MONITOR_SCRIPT").unwrap(),
        "tools/scalability_voice_stub.py"
    );
    let pool = Arc::new(voicemonitor::VoicePool::new(
        voicemonitor::VoiceConfig::from_environment().unwrap(),
    ));
    let start = Instant::now();
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let pool = pool.clone();
        jobs.spawn(async move { pool.recognize("offline-fixture".into(), 1.0).await.unwrap() });
    }
    while let Some(result) = jobs.join_next().await {
        assert_eq!(result.unwrap().usable_windows, 1);
    }
    println!(
        "PERF {}",
        serde_json::json!({"kind":"voice_workers","jobs":16,
        "stub_delay_ms":100,"workers":4,"seconds":start.elapsed().as_secs_f64()})
    );
}

#[test]
fn redirtied_groups_do_not_starve_groups_waiting_for_their_first_flush() {
    let list = DirtyList::default();
    for chat in 0..10000 {
        Dirty::mark(&list, chat);
    }
    let mut seen = HashSet::new();
    for _ in 0..20 {
        let batch = Dirty::take(&list, 512);
        seen.extend(batch.iter().copied());
        for chat in batch {
            Dirty::mark(&list, chat);
        }
    }
    assert_eq!(seen.len(), 10000);
    assert_eq!(
        list.0.lock().unwrap().len(),
        10000,
        "marks remain deduplicated"
    );
}

#[tokio::test]
#[ignore = "chat admission/eviction ownership against isolated PostgreSQL"]
async fn admitted_work_owns_chat_state_until_it_finishes() {
    let (ctx, _, _, _, _) = fixture(1, 0).await;
    let peer = PeerId::chat(1).unwrap().to_ambient_ref();
    let admitted = ctx
        .admit_chat(-1, peer)
        .await
        .expect("fixture chat fits the admission bound");

    assert_eq!(ctx.capacity_snapshot().runtime_chats, 1);
    assert_eq!(
        ctx.evict_idle(Duration::ZERO),
        0,
        "the Arc returned by admission is an in-flight ownership claim"
    );

    drop(admitted);
    assert_eq!(ctx.evict_idle(Duration::ZERO), 1);
}

#[tokio::test]
#[ignore = "member-serialized rejoin wake-up against isolated PostgreSQL"]
async fn rejoin_resume_waits_until_absence_quarantine_is_published() {
    let (ctx, _, db, _, _) = fixture(1, 0).await;
    let chat = -1_i64;
    let user = 77_i64;
    sqlx::query("DELETE FROM pending_warn_actions WHERE chat_id = $1 AND user_id = $2")
        .bind(chat)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM pending_strict_actions WHERE chat_id = $1 AND user_id = $2")
        .bind(chat)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO durable_chats (chat_id, access_hash, admitted_at)
         VALUES ($1, 1, 0) ON CONFLICT (chat_id) DO NOTHING",
    )
    .bind(chat)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pending_warn_actions
         (chat_id, user_id, penalty, created_at, claimed_until, lease_token)
         VALUES ($1, $2, 'mute', 0, 60, 1)",
    )
    .bind(chat)
    .bind(user)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pending_strict_actions
         (chat_id, user_id, action, until_date, threshold, target_name, wipe_history,
          created_at, available_at, lease_token)
         VALUES ($1, $2, 'mute', 0, 1, 'fixture', FALSE, 0, 0, 2)",
    )
    .bind(chat)
    .bind(user)
    .execute(&db)
    .await
    .unwrap();

    let publisher = restrict::lock_member(&ctx, chat, user).await;
    let mut resume = Box::pin(captcha::resume_member_rejoin(&ctx, chat, user, 100));
    let first_poll = std::future::poll_fn(|context| {
        std::task::Poll::Ready(std::future::Future::poll(resume.as_mut(), context))
    })
    .await;
    assert!(matches!(first_poll, std::task::Poll::Pending));

    sqlx::query(
        "UPDATE pending_warn_actions
         SET awaiting_rejoin = TRUE, lease_token = NULL
         WHERE chat_id = $1 AND user_id = $2",
    )
    .bind(chat)
    .bind(user)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE pending_strict_actions
         SET awaiting_rejoin = TRUE, lease_token = NULL
         WHERE chat_id = $1 AND user_id = $2",
    )
    .bind(chat)
    .bind(user)
    .execute(&db)
    .await
    .unwrap();
    drop(publisher);
    resume.await.unwrap();

    let (warning_waiting, strict_waiting): (bool, bool) = sqlx::query_as(
        "SELECT
           (SELECT awaiting_rejoin FROM pending_warn_actions
            WHERE chat_id = $1 AND user_id = $2),
           (SELECT awaiting_rejoin FROM pending_strict_actions
            WHERE chat_id = $1 AND user_id = $2)",
    )
    .bind(chat)
    .bind(user)
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(!warning_waiting && !strict_waiting);
    sqlx::query("DELETE FROM pending_warn_actions WHERE chat_id = $1 AND user_id = $2")
        .bind(chat)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM pending_strict_actions WHERE chat_id = $1 AND user_id = $2")
        .bind(chat)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bounded stats flush against isolated PostgreSQL"]
async fn fleet_stats_flush() {
    let (ctx, _, db, _, _) = fixture(10000, 10000).await;
    for chat in 1..=10000 {
        ctx.state(-chat)
            .count(99, || "fixture".into(), ["k_text", "h0"]);
    }
    sqlx::query("SELECT pg_stat_statements_reset()")
        .execute(&db)
        .await
        .unwrap();
    let start = Instant::now();
    stats::flush(&ctx).await;
    let elapsed = start.elapsed().as_secs_f64();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM counters WHERE total=1")
        .fetch_one(&db)
        .await
        .unwrap();
    let calls: i64 = sqlx::query_scalar("SELECT coalesce(sum(calls),0)::bigint FROM pg_stat_statements WHERE query NOT LIKE '%pg_stat_statements%' AND query NOT LIKE 'SELECT count(*) FROM counters%'")
        .fetch_one(&db).await.unwrap();
    assert_eq!(count, 10000);
    let mut legacy: HashSet<i64> = (0..10000).collect();
    let mut seen = HashSet::new();
    for _ in 0..20 {
        let batch: Vec<_> = legacy.iter().copied().take(512).collect();
        for chat in &batch {
            legacy.remove(chat);
        }
        seen.extend(batch.iter().copied());
        for chat in batch {
            legacy.insert(chat);
        }
    }
    println!(
        "PERF {}",
        serde_json::json!({"kind":"fleet_stats_flush","groups":10000,"persisted_groups":count,
        "seconds":elapsed,"db_statements":calls,"remaining_dirty":ctx.dirty.stats.0.lock().unwrap().len(),
        "legacy_groups_seen_in_20_passes":seen.len(),"legacy_single_pass_chat_limit":512})
    );
}

#[tokio::test]
#[ignore = "compiler layout diagnostic, no handler is polled"]
async fn future_sizes() {
    let (ctx, messages, _, _, _) = fixture(10000, 1).await;
    let message = &messages[0];
    let view = locks::View::new(message);
    macro_rules! size {
        ($future:expr) => {
            std::mem::size_of_val(&$future)
        };
    }
    fn returned<A, R>(_: impl FnOnce(A) -> R) -> usize {
        std::mem::size_of::<R>()
    }
    println!(
        "PERF {}",
        serde_json::json!({"kind":"future_sizes",
        "dispatch":size!(dispatch(&ctx, Update::NewMessage(message.clone()))),
        "callbacks":returned(|query| callbacks::handle(&ctx, query)),
        "raw_autoconfig":returned(|raw| autoconfig::on_raw(&ctx, raw)),
        "private_cleaner":size!(cleaner::handle(&ctx, message)),
        "private_sudo":size!(sudo::handle(&ctx, message)),
        "captcha_join":size!(captcha::on_join(&ctx,message)),
        "welcome_join":size!(welcome::on_join(&ctx,message)),
        "cleaner_join":size!(cleaner::on_join(&ctx,message)),
        "cleaner_wipe":size!(cleaner::wipe(&ctx,message)),
        "cleaner_sweep":size!(cleaner::sweep(&ctx,message)),
        "purge_all":size!(purge::handle_all(&ctx,message)),
        "voice_watch":size!(voicemonitor::watch(&ctx,message,-1,&view)),
        "trade_watch":size!(trade::watch(&ctx,message,-1,&view)),
        "locks":size!(locks::handle(&ctx, message, &view)),
        "flood":size!(flood::check(&ctx, message)),
        "nsfw":size!(nsfw::watch(&ctx, message, -1, &view)),
        "autoconfig":size!(autoconfig::on_message(&ctx, message, &ctx.state(-1))),
        "config":size!(config::handle(&ctx, message)),
        "panel":size!(panel::handle(&ctx, message)),
        "restrict":size!(restrict::handle(&ctx, message, &view)),
        "promote":size!(promote::handle(&ctx, message, &view)),
        "stats":size!(stats::handle(&ctx, message)),
        "extras":size!(extras::handle(&ctx, message, &view)),
        "purge":size!(purge::handle(&ctx, message, &view)),
        "tune":size!(tune::handle(&ctx, message, &view)),
        "imgfilter":size!(imgfilter::handle(&ctx, message)),
        "join":size!(join::enforce(&ctx, message))})
    );
}
