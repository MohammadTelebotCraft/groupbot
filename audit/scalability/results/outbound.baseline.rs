use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use grammers_session::types::PeerRef;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

const DEFAULT_RATE: u32 = 25;
const DEFAULT_CONCURRENCY: usize = 32;
const DEFAULT_GROUP_MESSAGES_PER_MINUTE: u32 = 20;
const DEFAULT_CHAT_CACHE_MAX: usize = 50_000;

struct Bucket {
    tokens: f64,
    updated: Instant,
}

pub(crate) struct OutboundBudget {
    bucket: Mutex<Bucket>,
    rate: f64,
    burst: f64,
    in_flight: Arc<Semaphore>,
    capacity: usize,
    waiting: AtomicUsize,
    chat_waiting: AtomicUsize,
    chat_next: Mutex<HashMap<i64, Instant>>,
    chat_interval: Duration,
    chat_cache_max: usize,
}

pub(crate) struct OutboundPermit {
    _in_flight: OwnedSemaphorePermit,
}

impl OutboundBudget {
    pub(crate) fn from_env() -> Self {
        let rate = env_u32("TELEGRAM_ACTIONS_PER_SECOND", DEFAULT_RATE, 1, 1_000);
        let burst = env_u32("TELEGRAM_ACTION_BURST", rate, 1, 10_000);
        let concurrency = env_usize(
            "TELEGRAM_OUTBOUND_CONCURRENCY",
            DEFAULT_CONCURRENCY,
            1,
            1_024,
        );
        let group_messages = env_u32(
            "TELEGRAM_GROUP_MESSAGES_PER_MINUTE",
            DEFAULT_GROUP_MESSAGES_PER_MINUTE,
            1,
            1_200,
        );
        let chat_cache_max = env_usize(
            "TELEGRAM_CHAT_RATE_CACHE_MAX",
            DEFAULT_CHAT_CACHE_MAX,
            1_000,
            500_000,
        );
        Self {
            bucket: Mutex::new(Bucket {
                tokens: burst as f64,
                updated: Instant::now(),
            }),
            rate: rate as f64,
            burst: burst as f64,
            in_flight: Arc::new(Semaphore::new(concurrency)),
            capacity: concurrency,
            waiting: AtomicUsize::new(0),
            chat_waiting: AtomicUsize::new(0),
            chat_next: Mutex::new(HashMap::new()),
            chat_interval: Duration::from_secs_f64(60.0 / group_messages as f64),
            chat_cache_max,
        }
    }

    pub(crate) async fn acquire_for_peer(&self, peer: PeerRef) -> OutboundPermit {
        if let Some(chat) = peer.id.bot_api_dialog_id().filter(|chat| *chat < 0) {
            let delay = {
                let now = Instant::now();
                let mut next = self.chat_next.lock().await;
                reserve_chat(
                    &mut next,
                    chat,
                    now,
                    self.chat_interval,
                    self.chat_cache_max,
                )
            };
            if !delay.is_zero() {
                self.chat_waiting.fetch_add(1, Ordering::Relaxed);
                let _waiting = ChatWaitingGuard(&self.chat_waiting);
                tokio::time::sleep(delay).await;
            }
        }
        self.acquire().await
    }

    pub(crate) async fn acquire(&self) -> OutboundPermit {
        self.waiting.fetch_add(1, Ordering::Relaxed);
        let in_flight = Arc::clone(&self.in_flight)
            .acquire_owned()
            .await
            .expect("the outbound semaphore is never closed");

        loop {
            let delay = {
                let mut bucket = self.bucket.lock().await;
                let elapsed = bucket.updated.elapsed().as_secs_f64();
                bucket.updated = Instant::now();
                bucket.tokens = (bucket.tokens + elapsed * self.rate).min(self.burst);
                if bucket.tokens >= 1.0 {
                    bucket.tokens -= 1.0;
                    None
                } else {
                    Some(Duration::from_secs_f64((1.0 - bucket.tokens) / self.rate))
                }
            };
            let Some(delay) = delay else { break };
            tokio::time::sleep(delay).await;
        }

        self.waiting.fetch_sub(1, Ordering::Relaxed);
        OutboundPermit {
            _in_flight: in_flight,
        }
    }

    pub(crate) fn snapshot(&self) -> (usize, usize) {
        let active = self
            .capacity
            .saturating_sub(self.in_flight.available_permits());
        let total =
            self.waiting.load(Ordering::Relaxed) + self.chat_waiting.load(Ordering::Relaxed);
        (active, total.saturating_sub(active))
    }
}

struct ChatWaitingGuard<'a>(&'a AtomicUsize);

impl Drop for ChatWaitingGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn reserve_chat(
    next: &mut HashMap<i64, Instant>,
    chat: i64,
    now: Instant,
    interval: Duration,
    capacity: usize,
) -> Duration {
    if !next.contains_key(&chat) && next.len() >= capacity {
        next.retain(|_, due| *due <= now);
        while next.len() >= capacity {
            let Some(oldest) = next
                .iter()
                .min_by_key(|(_, due)| **due)
                .map(|(chat, _)| *chat)
            else {
                break;
            };
            next.remove(&oldest);
        }
    }
    let due = next.entry(chat).or_insert(now);
    let start = (*due).max(now);
    *due = start + interval;
    start.saturating_duration_since(now)
}

fn env_u32(name: &str, default: u32, min: u32, max: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn env_usize(name: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::reserve_chat;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    #[test]
    fn per_chat_reservations_are_paced_and_capped() {
        let mut next = HashMap::new();
        let now = Instant::now();
        let interval = Duration::from_secs(3);
        assert!(reserve_chat(&mut next, -1, now, interval, 2).is_zero());
        assert!(reserve_chat(&mut next, -1, now, interval, 2) >= interval);
        assert!(reserve_chat(&mut next, -2, now, interval, 2).is_zero());
        assert!(reserve_chat(&mut next, -3, now, interval, 2).is_zero());
        assert!(next.len() <= 2);
    }
}
