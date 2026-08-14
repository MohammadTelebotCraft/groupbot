use std::collections::HashMap;
use std::sync::RwLock;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

type Result<T> = std::result::Result<T, sqlx::Error>;

pub const INDEXED: &[&str] = &["filter:", "pack:", "answer:"];

type ChatIndex = [Vec<Box<str>>; INDEXED.len()];

pub struct Settings {
    pool: PgPool,

    cache: RwLock<HashMap<i64, HashMap<String, String>>>,

    index: RwLock<HashMap<i64, ChatIndex>>,
}

fn indexed_slot(key: &str) -> Option<(usize, &str)> {
    INDEXED
        .iter()
        .enumerate()
        .find_map(|(slot, prefix)| key.strip_prefix(prefix).map(|rest| (slot, rest)))
}

impl Settings {
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .min_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect(url)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS settings (
                chat_id BIGINT NOT NULL,
                key     TEXT   NOT NULL,
                value   TEXT   NOT NULL DEFAULT '',
                PRIMARY KEY (chat_id, key)
            )",
        )
        .execute(&pool)
        .await?;

        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT chat_id, key, value FROM settings")
                .fetch_all(&pool)
                .await?;

        let mut cache: HashMap<i64, HashMap<String, String>> = HashMap::new();
        let mut index: HashMap<i64, ChatIndex> = HashMap::new();
        for (chat, key, value) in rows {
            if let Some((slot, rest)) = indexed_slot(&key) {
                index.entry(chat).or_default()[slot].push(rest.into());
            }
            cache.entry(chat).or_default().insert(key, value);
        }

        Ok(Self {
            pool,
            cache: RwLock::new(cache),
            index: RwLock::new(index),
        })
    }

    pub fn is_locked(&self, chat: i64, lock: &str) -> bool {
        self.cache
            .read()
            .unwrap()
            .get(&chat)
            .and_then(|map| map.get(lock))
            .is_some_and(|value| value.is_empty())
    }

    pub async fn set(&self, chat: i64, lock: &str, on: bool) -> bool {
        {
            let mut cache = self.cache.write().unwrap();
            let map = cache.entry(chat).or_default();
            let changed = if on {
                let already = map.get(lock).is_some_and(|value| value.is_empty());
                if !already {
                    map.insert(lock.to_owned(), String::new());
                }
                !already
            } else {
                map.remove(lock).is_some()
            };
            if !changed {
                return false;
            }
        }
        self.reindex(chat, lock, on);
        let result = if on {
            sqlx::query(
                "INSERT INTO settings (chat_id, key) VALUES ($1, $2)
                 ON CONFLICT (chat_id, key) DO NOTHING",
            )
            .bind(chat)
            .bind(lock)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query("DELETE FROM settings WHERE chat_id = $1 AND key = $2")
                .bind(chat)
                .bind(lock)
                .execute(&self.pool)
                .await
        };
        if let Err(e) = result {
            eprintln!("settings write failed for {chat}/{lock}: {e}");
        }
        true
    }

    pub fn indexed_empty(&self, chat: i64, prefix: &str) -> bool {
        let Some((slot, _)) = indexed_slot(prefix) else {
            return true;
        };
        self.index
            .read()
            .unwrap()
            .get(&chat)
            .is_none_or(|index| index[slot].is_empty())
    }

    pub fn indexed_any(&self, chat: i64, prefix: &str, predicate: impl Fn(&str) -> bool) -> bool {
        let Some((slot, _)) = indexed_slot(prefix) else {
            return false;
        };
        let index = self.index.read().unwrap();
        index
            .get(&chat)
            .is_some_and(|index| index[slot].iter().any(|key| predicate(key)))
    }

    fn reindex(&self, chat: i64, key: &str, present: bool) {
        let Some((slot, rest)) = indexed_slot(key) else {
            return;
        };
        let mut index = self.index.write().unwrap();
        let entry = index.entry(chat).or_default();
        let at = entry[slot].iter().position(|known| &**known == rest);
        match (present, at) {
            (true, None) => entry[slot].push(rest.into()),
            (false, Some(at)) => {
                entry[slot].swap_remove(at);
            }
            _ => {}
        }
    }

    pub fn flags_with_prefix(&self, chat: i64, prefix: &str) -> Vec<String> {
        let cache = self.cache.read().unwrap();
        let Some(map) = cache.get(&chat) else {
            return Vec::new();
        };
        let mut found: Vec<String> = map
            .iter()
            .filter(|(_, value)| value.is_empty())
            .filter_map(|(key, _)| key.strip_prefix(prefix).map(str::to_owned))
            .collect();
        found.sort_unstable();
        found
    }

    pub async fn set_values(&self, rows: Vec<(i64, String, String)>) {
        const CHUNK: usize = 1_000;
        for batch in rows.chunks(CHUNK) {
            {
                let mut cache = self.cache.write().unwrap();
                for (chat, key, value) in batch {
                    cache
                        .entry(*chat)
                        .or_default()
                        .insert(key.clone(), value.clone());
                }
            }
            for (chat, key, _) in batch {
                self.reindex(*chat, key, true);
            }

            let (mut chats, mut keys, mut values) = (Vec::new(), Vec::new(), Vec::new());
            for (chat, key, value) in batch {
                chats.push(*chat);
                keys.push(key.clone());
                values.push(value.clone());
            }
            let result = sqlx::query(
                "INSERT INTO settings (chat_id, key, value)
                 SELECT * FROM UNNEST($1::bigint[], $2::text[], $3::text[])
                 ON CONFLICT (chat_id, key) DO UPDATE SET value = EXCLUDED.value",
            )
            .bind(&chats)
            .bind(&keys)
            .bind(&values)
            .execute(&self.pool)
            .await;
            if let Err(e) = result {
                eprintln!("settings batch write failed ({} rows): {e}", batch.len());
            }
        }
    }

    pub async fn prune_stale(&self, prefix: &str, keep: &str) -> usize {
        let stale: Vec<(i64, String)> = {
            let cache = self.cache.read().unwrap();
            cache
                .iter()
                .flat_map(|(chat, map)| {
                    map.iter()
                        .filter(|(key, value)| {
                            key.starts_with(prefix) && !value.starts_with(keep)
                        })
                        .map(move |(key, _)| (*chat, key.clone()))
                })
                .collect()
        };
        if stale.is_empty() {
            return 0;
        }
        {
            let mut cache = self.cache.write().unwrap();
            for (chat, key) in &stale {
                if let Some(map) = cache.get_mut(chat) {
                    map.remove(key);
                }
            }
        }
        let result = sqlx::query(
            "DELETE FROM settings WHERE (chat_id, key) IN (
                 SELECT * FROM UNNEST($1::bigint[], $2::text[])
             )",
        )
        .bind(stale.iter().map(|(chat, _)| *chat).collect::<Vec<_>>())
        .bind(stale.iter().map(|(_, key)| key.clone()).collect::<Vec<_>>())
        .execute(&self.pool)
        .await;
        if let Err(e) = result {
            eprintln!("settings prune failed: {e}");
        }
        stale.len()
    }

    pub async fn prune_users(&self, prefix: &str, drop: impl Fn(i64, i64) -> bool) -> usize {
        let stale: Vec<(i64, String)> = {
            let cache = self.cache.read().unwrap();
            let mut stale = Vec::new();
            for (chat, map) in cache.iter() {
                for key in map.keys() {
                    let Some(user) = key.strip_prefix(prefix).and_then(|u| u.parse::<i64>().ok())
                    else {
                        continue;
                    };
                    if drop(*chat, user) {
                        stale.push((*chat, key.clone()));
                    }
                }
            }
            stale
        };
        if stale.is_empty() {
            return 0;
        }
        {
            let mut cache = self.cache.write().unwrap();
            for (chat, key) in &stale {
                if let Some(map) = cache.get_mut(chat) {
                    map.remove(key);
                }
            }
        }
        let result = sqlx::query(
            "DELETE FROM settings WHERE (chat_id, key) IN (
                 SELECT * FROM UNNEST($1::bigint[], $2::text[])
             )",
        )
        .bind(stale.iter().map(|(chat, _)| *chat).collect::<Vec<_>>())
        .bind(stale.iter().map(|(_, key)| key.clone()).collect::<Vec<_>>())
        .execute(&self.pool)
        .await;
        if let Err(e) = result {
            eprintln!("settings user prune failed: {e}");
        }
        stale.len()
    }

    pub async fn ping(&self) -> Option<std::time::Duration> {
        let started = std::time::Instant::now();
        sqlx::query("SELECT 1").execute(&self.pool).await.ok()?;
        Some(started.elapsed())
    }

    pub fn chats(&self) -> Vec<i64> {
        self.cache.read().unwrap().keys().copied().collect()
    }

    pub fn pick_values<T>(
        &self,
        chat: i64,
        prefix: &str,
        mut pick: impl FnMut(&str, &str) -> Option<T>,
    ) -> Vec<T> {
        let cache = self.cache.read().unwrap();
        let Some(map) = cache.get(&chat) else {
            return Vec::new();
        };
        map.iter()
            .filter(|(_, value)| !value.is_empty())
            .filter_map(|(key, value)| pick(key.strip_prefix(prefix)?, value))
            .collect()
    }

    pub fn values_with_prefix(&self, chat: i64, prefix: &str) -> Vec<(String, String)> {
        let cache = self.cache.read().unwrap();
        let Some(map) = cache.get(&chat) else {
            return Vec::new();
        };
        map.iter()
            .filter(|(_, value)| !value.is_empty())
            .filter_map(|(key, value)| {
                Some((key.strip_prefix(prefix)?.to_owned(), value.clone()))
            })
            .collect()
    }

    pub fn value(&self, chat: i64, key: &str) -> Option<String> {
        self.cache
            .read()
            .unwrap()
            .get(&chat)?
            .get(key)
            .filter(|value| !value.is_empty())
            .cloned()
    }

    pub fn value_parsed<T: std::str::FromStr>(&self, chat: i64, key: &str) -> Option<T> {
        self.cache.read().unwrap().get(&chat)?.get(key)?.parse().ok()
    }

    pub async fn set_value(&self, chat: i64, key: &str, value: &str) {
        {
            let mut cache = self.cache.write().unwrap();
            let map = cache.entry(chat).or_default();

            if map.get(key).is_some_and(|stored| stored == value) {
                return;
            }
            map.insert(key.to_owned(), value.to_owned());
        }
        self.reindex(chat, key, true);
        let result = sqlx::query(
            "INSERT INTO settings (chat_id, key, value) VALUES ($1, $2, $3)
             ON CONFLICT (chat_id, key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(chat)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await;
        if let Err(e) = result {
            eprintln!("settings write failed for {chat}/{key}: {e}");
        }
    }

    pub async fn import_file(&self, path: &str) -> usize {
        let Ok(text) = std::fs::read_to_string(path) else {
            return 0;
        };
        let mut imported = 0;
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let Some(Ok(chat)) = parts.next().map(str::parse::<i64>) else {
                continue;
            };
            for part in parts {
                match part.split_once('=') {
                    Some((key, value)) => self.set_value(chat, key, value).await,
                    None => {
                        self.set(chat, part, true).await;
                    }
                }
                imported += 1;
            }
        }
        if imported > 0 {
            let _ = std::fs::rename(path, format!("{path}.imported"));
        }
        imported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_and_values_share_the_map() {
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("links".to_owned(), String::new());
        map.insert("owner".to_owned(), "42".to_owned());
        assert!(map.get("links").is_some_and(|v| v.is_empty()));
        assert!(!map.get("owner").is_some_and(|v| v.is_empty()));
        assert_eq!(map.get("owner").map(String::as_str), Some("42"));
    }

    #[tokio::test]
    #[ignore]
    async fn roundtrip() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let chat = -999_999_999_999;

        let settings = Settings::connect(&url).await.unwrap();
        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
        let settings = Settings::connect(&url).await.unwrap();

        settings.set_value(chat, "owner", "42").await;
        settings.set_value(chat, "owner", "7").await;
        assert_eq!(settings.value(chat, "owner").as_deref(), Some("7"));
        assert!(settings.set(chat, "links", true).await);
        assert!(!settings.set(chat, "links", true).await);

        assert!(settings.indexed_empty(chat, "filter:"));
        assert!(settings.set(chat, "filter:بد", true).await);
        settings.set_value(chat, "answer:سلام", "درود").await;
        assert!(!settings.indexed_empty(chat, "filter:"));
        assert!(settings.indexed_any(chat, "filter:", |word| word == "بد"));
        assert!(settings.indexed_any(chat, "answer:", |trigger| trigger == "سلام"));
        assert!(!settings.indexed_any(chat, "filter:", |word| word == "خوب"));

        let reloaded = Settings::connect(&url).await.unwrap();
        assert_eq!(reloaded.value(chat, "owner").as_deref(), Some("7"));
        assert!(reloaded.is_locked(chat, "links"));
        assert!(reloaded.indexed_any(chat, "filter:", |word| word == "بد"));
        assert!(reloaded.indexed_any(chat, "answer:", |trigger| trigger == "سلام"));

        assert!(settings.set(chat, "filter:بد", false).await);
        assert!(settings.indexed_empty(chat, "filter:"));

        settings.set_value(chat, "today:1", "5|3|Ali").await;
        settings.set_value(chat, "today:2", "9|1|Sara").await;
        assert_eq!(settings.prune_stale("today:", "9|").await, 1);
        assert!(settings.value(chat, "today:1").is_none());
        assert!(settings.value(chat, "today:2").is_some());

        settings.set_value(chat, "total:1", "5|Ali").await;
        settings.set_value(chat, "total:2", "9|Sara").await;
        assert_eq!(settings.prune_users("total:", |_, user| user == 1).await, 1);
        assert!(settings.value(chat, "total:1").is_none());
        assert!(settings.value(chat, "total:2").is_some());

        sqlx::query("DELETE FROM settings WHERE chat_id = $1")
            .bind(chat)
            .execute(&settings.pool)
            .await
            .unwrap();
    }
}
