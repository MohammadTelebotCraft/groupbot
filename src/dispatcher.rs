use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::task::{Id, JoinError, JoinSet};

#[cfg(test)]
mod load_tests;

struct Pending<T> {
    value: T,
    received: Instant,
}

struct Lane<T> {
    pending: VecDeque<Pending<T>>,
    running: usize,
}

pub struct Completion {
    pub latency: Duration,
    pub queue_wait: Duration,
    pub result: Result<(), JoinError>,
}

pub struct Dispatcher<T> {
    lanes: HashMap<i64, Lane<T>>,
    ready: VecDeque<i64>,
    pending: usize,
    capacity: usize,
    concurrency: usize,
    per_peer: usize,
    tasks: JoinSet<(Duration, Duration)>,
    active: HashMap<Id, i64>,
}

impl<T: Send + 'static> Dispatcher<T> {
    pub fn new(concurrency: usize, per_peer: usize, capacity: usize) -> Self {
        assert!(concurrency > 0 && per_peer > 0 && capacity > 0);
        Self {
            lanes: HashMap::new(),
            ready: VecDeque::new(),
            pending: 0,
            capacity,
            concurrency,
            per_peer,
            tasks: JoinSet::new(),
            active: HashMap::new(),
        }
    }

    pub fn has_room(&self) -> bool {
        self.pending < self.capacity
    }
    pub fn pending(&self) -> usize {
        self.pending
    }
    pub fn active(&self) -> usize {
        self.active.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pending == 0 && self.active.is_empty()
    }

    pub fn push(&mut self, key: i64, value: T) {
        assert!(self.has_room(), "dispatcher admission capacity exceeded");
        let lane = self.lanes.entry(key).or_insert_with(|| Lane {
            pending: VecDeque::new(),
            running: 0,
        });
        if lane.running < self.per_peer && lane.pending.is_empty() {
            self.ready.push_back(key);
        }
        lane.pending.push_back(Pending {
            value,
            received: Instant::now(),
        });
        self.pending += 1;
    }

    pub fn start<F: Future<Output = ()> + Send + 'static>(&mut self, mut run: impl FnMut(T) -> F) {
        while self.active.len() < self.concurrency {
            let Some(key) = self.ready.pop_front() else {
                break;
            };
            let lane = self.lanes.get_mut(&key).expect("ready lane exists");
            debug_assert!(lane.running < self.per_peer);
            let item = lane.pending.pop_front().expect("ready lane has work");
            lane.running += 1;
            if lane.running < self.per_peer && !lane.pending.is_empty() {
                self.ready.push_back(key);
            }
            self.pending -= 1;
            let future = Box::pin(run(item.value));
            let handle = self.tasks.spawn(async move {
                let queue_wait = item.received.elapsed();
                future.await;
                (item.received.elapsed(), queue_wait)
            });
            self.active.insert(handle.id(), key);
        }
    }

    fn complete(&mut self, result: Result<(Id, (Duration, Duration)), JoinError>) -> Completion {
        let id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        let key = self.active.remove(&id).expect("completed task is tracked");
        let lane = self.lanes.get_mut(&key).expect("active lane exists");
        lane.running -= 1;
        if lane.pending.is_empty() && lane.running == 0 {
            self.lanes.remove(&key);
        } else if !lane.pending.is_empty() && lane.running + 1 == self.per_peer {
            self.ready.push_back(key);
        }
        match result {
            Ok((_, (latency, queue_wait))) => Completion {
                latency,
                queue_wait,
                result: Ok(()),
            },
            Err(error) => Completion {
                latency: Duration::ZERO,
                queue_wait: Duration::ZERO,
                result: Err(error),
            },
        }
    }

    pub fn try_join_next(&mut self) -> Option<Completion> {
        let result = self.tasks.try_join_next_with_id()?;
        Some(self.complete(result))
    }

    pub async fn join_next(&mut self) -> Option<Completion> {
        let result = self.tasks.join_next_with_id().await?;
        Some(self.complete(result))
    }

    pub async fn shutdown(&mut self) -> usize {
        let unfinished = self.pending + self.active.len();
        self.tasks.abort_all();
        while let Some(result) = self.tasks.join_next_with_id().await {
            let _completion = self.complete(result);
        }
        self.pending = 0;
        self.ready.clear();
        self.lanes.clear();
        debug_assert!(self.active.is_empty());
        unfinished
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn a_slow_peer_does_not_take_other_peers_workers() {
        let mut dispatcher = Dispatcher::new(32, 1, 1024);
        let barrier = Arc::new(tokio::sync::Semaphore::new(0));
        let done = Arc::new(Mutex::new(Vec::new()));
        for sequence in 0..512 {
            dispatcher.push(1, (1, sequence));
        }
        for peer in 2..=33 {
            dispatcher.push(peer, (peer, 0));
        }
        let run = |(peer, sequence)| {
            let barrier = barrier.clone();
            let done = done.clone();
            async move {
                if peer == 1 {
                    let _ = barrier.acquire().await.unwrap();
                }
                done.lock().unwrap().push((peer, sequence));
            }
        };
        dispatcher.start(run);
        assert_eq!(dispatcher.active(), 32);
        tokio::time::timeout(Duration::from_secs(2), async {
            for _ in 0..32 {
                dispatcher.join_next().await.unwrap().result.unwrap();
                dispatcher.start(run);
            }
        })
        .await
        .unwrap();
        assert_eq!(dispatcher.active(), 1);
        barrier.add_permits(1);
        while !dispatcher.is_empty() {
            dispatcher.join_next().await.unwrap().result.unwrap();
            dispatcher.start(run);
        }
        let got: Vec<_> = done
            .lock()
            .unwrap()
            .iter()
            .filter(|(peer, _)| *peer == 1)
            .map(|(_, n)| *n)
            .collect();
        assert_eq!(got, (0..512).collect::<Vec<_>>());
        assert!(dispatcher.lanes.is_empty(), "idle peers retain no lane");
    }

    #[tokio::test]
    async fn panicking_task_releases_its_lane_and_does_not_strand_successors() {
        let mut dispatcher = Dispatcher::new(1, 1, 2);
        dispatcher.push(1, true);
        dispatcher.push(1, false);
        assert!(!dispatcher.has_room());
        let run = |panic_now: bool| async move {
            assert!(!panic_now, "injected panic");
        };
        dispatcher.start(run);
        assert!(dispatcher.join_next().await.unwrap().result.is_err());
        dispatcher.start(run);
        dispatcher.join_next().await.unwrap().result.unwrap();
        assert!(dispatcher.is_empty());
        assert!(dispatcher.lanes.is_empty());
    }

    #[tokio::test]
    async fn forced_shutdown_joins_running_tasks_and_discards_pending_work() {
        let mut dispatcher = Dispatcher::new(2, 1, 4);
        let barrier = Arc::new(tokio::sync::Semaphore::new(0));
        let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for peer in 0..4 {
            dispatcher.push(peer, peer);
        }
        dispatcher.start(|_| {
            let barrier = Arc::clone(&barrier);
            let completed = Arc::clone(&completed);
            async move {
                barrier.acquire().await.unwrap().forget();
                completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        assert_eq!(dispatcher.shutdown().await, 4);
        assert!(dispatcher.is_empty());
        barrier.add_permits(4);
        tokio::task::yield_now().await;
        assert_eq!(completed.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
