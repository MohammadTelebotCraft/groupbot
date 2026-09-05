use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

fn store() -> &'static Mutex<HashMap<(i64, String), Instant>> {
    static STORE: OnceLock<Mutex<HashMap<(i64, String), Instant>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn throttled(chat: i64, key: &str, cooldown: Duration) -> bool {
    let mut map = store().lock().unwrap();
    let now = Instant::now();
    match map.get(&(chat, key.to_owned())) {
        Some(&last) if now.duration_since(last) < cooldown => true,
        _ => {
            map.insert((chat, key.to_owned()), now);
            false
        }
    }
}
