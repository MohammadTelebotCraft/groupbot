use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct Job {
    key: i64,
    received: Instant,
    delay: Duration,
    _payload: Vec<u8>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "open-loop synthetic queue benchmark"]
async fn queue_workloads() {
    for scenario in ["hot_burst", "many_active", "overload", "downstream_stall"] {
        for fair in [false, true] {
            let (sender, mut receiver) = tokio::sync::mpsc::channel::<Job>(4096);
            let dropped = Arc::new(AtomicUsize::new(0));
            let raw_peak = Arc::new(AtomicUsize::new(0));
            let delivered = Arc::new(Mutex::new(Vec::<(i64, u64)>::new()));
            let states: Arc<Vec<_>> = Arc::new(
                (0..1001)
                    .map(|_| Arc::new(crate::handlers::ChatState::default()))
                    .collect(),
            );
            let started = Instant::now();
            let lost = dropped.clone();
            let raw_max = raw_peak.clone();
            let producer = tokio::spawn(async move {
                let count = match scenario {
                    "hot_burst" | "downstream_stall" => 768,
                    _ => 20000,
                };
                for index in 0..count {
                    if scenario == "many_active" && index % 100 == 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    let key = if matches!(scenario, "hot_burst" | "downstream_stall") && index < 512
                    {
                        0
                    } else {
                        index % 1000 + 1
                    };
                    let delay_ms = match scenario {
                        "hot_burst" if key == 0 => 20,
                        "downstream_stall" if key == 0 => 50,
                        "hot_burst" | "downstream_stall" => 0,
                        "overload" => 5,
                        _ => 1,
                    };
                    let result = sender.try_send(Job {
                        key: key as i64,
                        received: Instant::now(),
                        delay: Duration::from_millis(delay_ms),
                        _payload: vec![0; 2048],
                    });
                    if result.is_err() {
                        lost.fetch_add(1, Ordering::Relaxed);
                    }
                    raw_max.fetch_max(4096 - sender.capacity(), Ordering::Relaxed);
                }
                count
            });
            let run = |job: Job| {
                let delivered = delivered.clone();
                let state = states[job.key as usize].clone();
                async move {
                    let _slot = state.slot().await;
                    if !job.delay.is_zero() {
                        tokio::time::sleep(job.delay).await;
                    }
                    delivered
                        .lock()
                        .unwrap()
                        .push((job.key, job.received.elapsed().as_micros() as u64));
                }
            };
            let mut queue_peak = 0;
            let mut active_peak = 0;
            if fair {
                let mut dispatcher = Dispatcher::new(512, 8, 4096);
                let mut open = true;
                while open || !dispatcher.is_empty() {
                    while let Some(completion) = dispatcher.try_join_next() {
                        completion.result.unwrap();
                    }
                    dispatcher.start(run);
                    active_peak = active_peak.max(dispatcher.active());
                    queue_peak = queue_peak.max(dispatcher.pending());
                    if !open && dispatcher.is_empty() {
                        break;
                    }
                    tokio::select! {
                        job = receiver.recv(), if open && dispatcher.has_room() => match job {
                            Some(job) => dispatcher.push(job.key, job), None => open = false,
                        },
                        done = dispatcher.join_next(), if dispatcher.active() > 0 => { done.unwrap().result.unwrap(); }
                    }
                }
                assert!(dispatcher.lanes.is_empty());
            } else {
                let permits = Arc::new(tokio::sync::Semaphore::new(512));
                let mut tasks = JoinSet::new();
                loop {
                    while let Some(result) = tasks.try_join_next() {
                        result.unwrap();
                    }
                    let permit = permits.clone().acquire_owned().await.unwrap();
                    let Some(job) = receiver.recv().await else {
                        break;
                    };
                    let future = run(job);
                    tasks.spawn(async move {
                        let _permit = permit;
                        future.await;
                    });
                    active_peak = active_peak.max(512 - permits.available_permits());
                }
                while let Some(result) = tasks.join_next().await {
                    result.unwrap();
                }
            }
            let offered = producer.await.unwrap();
            let elapsed = started.elapsed().as_secs_f64();
            let results = delivered.lock().unwrap();
            let mut all: Vec<_> = results.iter().map(|(_, time)| *time).collect();
            all.sort_unstable();
            let mut cold: Vec<_> = results
                .iter()
                .filter(|(key, _)| *key != 0)
                .map(|(_, time)| *time)
                .collect();
            cold.sort_unstable();
            let percentiles = |values: &[u64]| {
                [50, 95, 99].map(|p| values[(values.len() * p / 100).min(values.len() - 1)])
            };
            assert_eq!(results.len() + dropped.load(Ordering::Relaxed), offered);
            assert!(active_peak <= 512 && queue_peak <= 4096);
            println!(
                "PERF {}",
                serde_json::json!({"kind":"queue_simulation","scenario":scenario,"dispatcher":if fair {"fair"} else {"legacy"},
                "offered":offered,"completed":results.len(),"dropped_at_raw_channel":dropped.load(Ordering::Relaxed),
                "seconds":elapsed,"updates_per_second":results.len() as f64/elapsed,"p50_p95_p99_us":percentiles(&all),
                "unrelated_p50_p95_p99_us":percentiles(&cold),"worker_peak":active_peak,"application_queue_peak":queue_peak,
                "raw_queue_peak":raw_peak.load(Ordering::Relaxed),"payload_bytes":2048})
            );
        }
    }
}
