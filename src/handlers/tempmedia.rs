use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use grammers_client::message::Message;

use super::Ctx;
use super::locks::{self, View};

pub const MODE: &str = "tmed";

pub const MINUTES: &str = "tmed_min";

pub const AUDIENCE: &str = "tmed_who";

const DEFAULT_MINUTES: u32 = 20;
pub const MINUTES_RANGE: (u32, u32) = (1, 1440);
pub const MINUTES_PRESETS: &[u32] = &[5, 10, 20, 30, 60, 180, 360, 720, 1440];

const MAX_PENDING: usize = 5_000;

const CHUNK: usize = 100;

const RETRY_AFTER: Duration = Duration::from_secs(60);

const MAX_DEFERRED_PER_SWEEP: usize = 2_000;

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

pub fn due_from_unix(due_at: i64) -> Instant {
    let left = due_at.saturating_sub(unix_now()).max(0) as u64;
    Instant::now() + Duration::from_secs(left)
}

pub struct Kind {
    pub name: &'static str,
    pub key: &'static str,
    pub label: &'static str,
    pub matches: fn(&View) -> bool,
}

pub const KINDS: &[Kind] = &[
    Kind {
        name: "sticker",
        key: "tmed_keep_sticker",
        label: "استیکر",
        matches: locks::is_sticker,
    },
    Kind {
        name: "gif",
        key: "tmed_keep_gif",
        label: "گیف",
        matches: locks::is_gif,
    },
    Kind {
        name: "photo",
        key: "tmed_keep_photo",
        label: "عکس",
        matches: locks::is_photo,
    },
    Kind {
        name: "video",
        key: "tmed_keep_video",
        label: "فیلم",
        matches: locks::is_video,
    },
    Kind {
        name: "music",
        key: "tmed_keep_music",
        label: "اهنگ",
        matches: locks::is_music,
    },
    Kind {
        name: "file",
        key: "tmed_keep_file",
        label: "فایل",
        matches: locks::is_file,
    },
];

pub fn find(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|kind| kind.name == name)
}

pub fn minutes(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings.with_chat(chat, |settings| {
        settings.number(MINUTES, DEFAULT_MINUTES, MINUTES_RANGE)
    })
}

pub fn reaches_everyone(ctx: &Ctx, chat: i64) -> bool {
    ctx.settings.value(chat, AUDIENCE).as_deref() == Some("all")
}

pub fn temporary(ctx: &Ctx, chat: i64, kind: &Kind) -> bool {
    !ctx.settings.is_locked(chat, kind.key)
}

pub async fn watch(ctx: &Ctx, message: &Message, view: &View<'_>) {
    if view.media().is_none() {
        return;
    }
    let Some(chat) = super::chat_id(message) else {
        return;
    };
    let Some((minutes, everyone)) = ctx.settings.with_chat(chat, |settings| {
        if !settings.is_locked(MODE) {
            return None;
        }
        let kind = KINDS.iter().find(|kind| (kind.matches)(view))?;
        if settings.is_locked(kind.key) {
            return None;
        }
        Some((
            settings.number(MINUTES, DEFAULT_MINUTES, MINUTES_RANGE),
            settings.value(AUDIENCE) == Some("all"),
        ))
    }) else {
        return;
    };

    if super::is_linked_post(message) {
        return;
    }

    if !everyone && super::is_exempt(ctx, message).await {
        return;
    }

    let seconds = u64::from(minutes) * 60;
    ctx.queue_temp_media(
        chat,
        message.id(),
        Instant::now() + Duration::from_secs(seconds),
        unix_now() + seconds as i64,
    );
}

pub async fn sweep_deferred(ctx: &std::sync::Arc<Ctx>) {
    process_deferred(ctx).await;
}

async fn process_deferred(ctx: &std::sync::Arc<Ctx>) {
    let mut deletes: HashMap<i64, Vec<i32>> = HashMap::new();
    let mut captchas = Vec::new();
    for action in ctx.take_due_actions(MAX_DEFERRED_PER_SWEEP) {
        match action {
            super::DeferredAction::Delete { chat, message, .. } => {
                deletes.entry(chat).or_default().push(message);
            }
            super::DeferredAction::Captcha {
                chat, user, target, ..
            } => captchas.push((chat, user, target)),
        }
    }

    let owner = std::sync::Arc::clone(ctx);
    super::bounded(
        deletes.into_iter().collect(),
        super::FLEET_CONCURRENCY,
        move |(chat, ids)| {
            let ctx = std::sync::Arc::clone(&owner);
            async move {
                let Some(chat_ref) = ctx.chat_ref(chat) else {
                    return;
                };
                for chunk in ids.chunks(CHUNK) {
                    if let Err(e) = ctx.client.delete_messages(chat_ref, chunk).await {
                        eprintln!("delayed delete: could not delete in {chat}: {e}");
                        break;
                    }
                }
            }
        },
    )
    .await;

    let owner = std::sync::Arc::clone(ctx);
    super::bounded(
        captchas,
        super::FLEET_CAMPAIGNS,
        move |(chat, user, target)| {
            let ctx = std::sync::Arc::clone(&owner);
            async move {
                let Some(chat_ref) = ctx.chat_ref(chat) else {
                    ctx.captcha_done(chat, user);
                    return;
                };
                super::captcha::expire(&ctx, chat_ref, chat, user, target).await;
            }
        },
    )
    .await;
}

pub async fn sweep(ctx: &std::sync::Arc<Ctx>) {
    let writes = ctx.take_pending_writes();
    if !writes.is_empty() {
        ctx.settings.save_pending(writes).await;
    }
    let drops = ctx.take_pending_drops();
    if !drops.is_empty() {
        let failed = ctx.settings.drop_pending_rows(&drops).await;
        if !failed.is_empty() {
            ctx.retry_pending_drops(&failed);
        }
    }

    let owner = std::sync::Arc::clone(ctx);
    super::bounded(
        ctx.take_due_media(),
        super::FLEET_CONCURRENCY,
        move |(chat, ids)| {
            let ctx = std::sync::Arc::clone(&owner);
            async move {
                let Some(chat_ref) = ctx.chat_ref(chat) else {
                    return;
                };
                let mut attempted = 0;
                for chunk in ids.chunks(CHUNK) {
                    if let Err(e) = ctx.client.delete_messages(chat_ref, chunk).await {
                        eprintln!("temp media: could not delete in {chat}: {e}");
                        break;
                    }
                    attempted += chunk.len();
                }

                ctx.settings.drop_pending(chat, &ids[..attempted]).await;
                for id in &ids[attempted..] {
                    ctx.restore_temp_media(chat, *id, Instant::now() + RETRY_AFTER);
                }
            }
        },
    )
    .await;
}

pub async fn restore(ctx: &Ctx) {
    let rows = ctx.settings.load_pending(unix_now()).await;
    if rows.is_empty() {
        return;
    }
    let count = rows.len();
    for (chat, id, due_at) in rows {
        ctx.restore_temp_media(chat, id, due_from_unix(due_at));
    }
    let dropped = ctx.take_pending_drops();
    if !dropped.is_empty() {
        let failed = ctx.settings.drop_pending_rows(&dropped).await;
        if !failed.is_empty() {
            ctx.retry_pending_drops(&failed);
        }
        println!(
            "temp media: removed {} overflowed durable row(s)",
            dropped.len().saturating_sub(failed.len())
        );
    }
    println!("temp media: restored {count} pending delete(s)");
}

pub fn queue(pending: &mut VecDeque<(Instant, i32)>, id: i32, due: Instant) -> Option<i32> {
    let dropped = (pending.len() >= MAX_PENDING)
        .then(|| pending.pop_front().expect("a full queue has a front"))
        .map(|(_, id)| id);
    pending.push_back((due, id));
    dropped
}

#[cfg(test)]
pub fn drain_due(pending: &mut VecDeque<(Instant, i32)>, now: Instant) -> Vec<i32> {
    drain_due_up_to(pending, now, usize::MAX)
}

pub fn drain_due_up_to(
    pending: &mut VecDeque<(Instant, i32)>,
    now: Instant,
    limit: usize,
) -> Vec<i32> {
    let mut due = Vec::new();
    while due.len() < limit {
        let Some(&(at, id)) = pending.front() else {
            break;
        };
        if at > now {
            break;
        }
        pending.pop_front();
        due.push(id);
    }
    due
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_named_once() {
        let count = KINDS.len();
        let mut names: Vec<&str> = KINDS.iter().map(|kind| kind.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "two kinds answer to the same name");

        let mut keys: Vec<&str> = KINDS.iter().map(|kind| kind.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two kinds share a settings key");
    }

    #[test]
    fn payloads_fit_telegram() {
        for kind in KINDS {
            let longest = format!("p:{}:{}:{MODE}:{}", i64::MAX, i64::MIN, kind.name);
            assert!(longest.len() <= 64, "payload too long for {}", kind.name);
        }
    }

    #[test]
    fn the_default_delay_is_offered_as_a_preset() {
        assert!(MINUTES_PRESETS.contains(&DEFAULT_MINUTES));
        assert!(DEFAULT_MINUTES >= MINUTES_RANGE.0 && DEFAULT_MINUTES <= MINUTES_RANGE.1);
    }

    #[test]
    fn only_what_is_due_leaves_the_queue() {
        let now = Instant::now();
        let mut pending = VecDeque::new();
        assert!(queue(&mut pending, 1, now - Duration::from_secs(2)).is_none());
        assert!(queue(&mut pending, 2, now - Duration::from_secs(1)).is_none());
        assert!(queue(&mut pending, 3, now + Duration::from_secs(60)).is_none());

        assert_eq!(drain_due(&mut pending, now), vec![1, 2]);
        assert_eq!(pending.len(), 1);
        assert!(drain_due(&mut pending, now).is_empty());
        assert_eq!(
            drain_due(&mut pending, now + Duration::from_secs(61)),
            vec![3]
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn bounded_due_drain_leaves_due_work_for_the_next_batch() {
        let now = Instant::now();
        let mut pending = VecDeque::from([
            (now - Duration::from_secs(1), 1),
            (now - Duration::from_secs(1), 2),
            (now - Duration::from_secs(1), 3),
            (now + Duration::from_secs(60), 4),
        ]);

        assert_eq!(drain_due_up_to(&mut pending, now, 2), vec![1, 2]);
        assert_eq!(drain_due_up_to(&mut pending, now, 2), vec![3]);
        assert_eq!(
            pending,
            VecDeque::from([(now + Duration::from_secs(60), 4)])
        );
    }

    #[test]
    fn a_stored_deadline_comes_back_usable() {
        let now = unix_now();
        assert!(now > 1_700_000_000, "unix_now looks wrong: {now}");

        let overdue = due_from_unix(now - 600);
        assert!(
            overdue <= Instant::now(),
            "an overdue row must fire at once"
        );

        let later = due_from_unix(now + 600);
        assert!(later > Instant::now());
        assert!(later <= Instant::now() + Duration::from_secs(601));

        assert!(due_from_unix(0) <= Instant::now());
    }

    #[test]
    fn a_full_queue_drops_its_oldest() {
        let now = Instant::now();
        let mut pending = VecDeque::new();
        for id in 0..MAX_PENDING as i32 + 10 {
            let dropped = queue(&mut pending, id, now);
            if id < MAX_PENDING as i32 {
                assert!(dropped.is_none());
            } else {
                assert_eq!(dropped, Some(id - MAX_PENDING as i32));
            }
        }
        assert_eq!(pending.len(), MAX_PENDING);
        assert_eq!(pending.front().map(|&(_, id)| id), Some(10));
    }
}
