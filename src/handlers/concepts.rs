use std::sync::Arc;

use grammers_client::message::Message;
use grammers_client::session::types::PeerRef;

use super::Ctx;
use super::concept_vectors as vectors;
use super::nsfw;
use super::vision::{DIM, dot};

pub struct Concept {
    pub key: &'static str,
    pub names: &'static [&'static str],

    pub notice: &'static str,
    pub vector: &'static [f32; DIM],
}

pub const CONCEPTS: &[Concept] = &[
    Concept {
        key: "c_cig",
        names: &["سیگار", "دخانیات", "قلیان"],
        notice: "تصویر سیگار و دخانیات",
        vector: &vectors::CIGARETTE,
    },
    Concept {
        key: "c_alc",
        names: &["مشروب", "الکل", "مشروبات"],
        notice: "تصویر مشروبات الکلی",
        vector: &vectors::ALCOHOL,
    },
    Concept {
        key: "c_gun",
        names: &["اسلحه", "سلاح"],
        notice: "تصویر اسلحه",
        vector: &vectors::WEAPON,
    },
    Concept {
        key: "c_bet",
        names: &["قمار", "شرط بندی", "شرطبندی"],
        notice: "تصویر قمار و شرط بندی",
        vector: &vectors::GAMBLING,
    },
    Concept {
        key: "c_drg",
        names: &["مواد", "مواد مخدر"],
        notice: "تصویر مواد مخدر",
        vector: &vectors::DRUGS,
    },
    Concept {
        key: "c_bld",
        names: &["خون", "خونریزی"],
        notice: "تصویر خون و خونریزی",
        vector: &vectors::BLOOD,
    },
];

pub const SHADOW: &str = "cq_shadow";

pub const LIMIT: &str = "cq_lim";

pub const LIMIT_RANGE: (u32, u32) = (5, 60);
pub const LIMIT_PRESETS: &[u32] = &[10, 15, 20, 25, 30, 40];
pub const DEFAULT_LIMIT: u32 = 20;

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings.with_chat(chat, |settings| {
        settings.number(LIMIT, DEFAULT_LIMIT, LIMIT_RANGE)
    })
}

pub fn margins_from(embedding: &[f32]) -> [f32; super::CONCEPT_SLOTS] {
    let baseline = dot(embedding, &vectors::BACKGROUND);
    let mut out = [f32::MIN; super::CONCEPT_SLOTS];
    for (slot, concept) in CONCEPTS.iter().enumerate() {
        out[slot] = dot(embedding, concept.vector) - baseline;
    }
    out
}

fn over(margin: f32, limit: u32) -> bool {
    margin * 1000.0 >= limit as f32
}

pub struct Armed {
    pub concepts: Vec<&'static Concept>,
    pub limit: u32,
    pub live: bool,
}

pub fn armed_under(settings: &super::super::state::ChatSettings<'_>) -> Option<Armed> {
    let concepts: Vec<&'static Concept> = CONCEPTS
        .iter()
        .filter(|concept| settings.is_locked(concept.key))
        .collect();
    (!concepts.is_empty()).then(|| Armed {
        concepts,
        limit: settings.number(LIMIT, DEFAULT_LIMIT, LIMIT_RANGE),
        live: !settings.is_locked(SHADOW),
    })
}

fn worst(all: &[f32; super::CONCEPT_SLOTS], armed: &Armed) -> Option<(&'static Concept, f32)> {
    armed
        .concepts
        .iter()
        .filter_map(|concept| {
            let slot = CONCEPTS.iter().position(|c| c.key == concept.key)?;
            Some((*concept, all[slot]))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

fn report(chat: i64, id: i64, all: &[f32; super::CONCEPT_SLOTS], armed: &Armed, cached: bool) {
    let shown: Vec<String> = armed
        .concepts
        .iter()
        .filter_map(|concept| {
            let slot = CONCEPTS.iter().position(|c| c.key == concept.key)?;
            Some(format!("{} {:+.3}", concept.key, all[slot]))
        })
        .collect();
    let hit = worst(all, armed).is_some_and(|(_, margin)| over(margin, armed.limit));
    log::info!(
        "concept[{}]: chat {chat} file {id} limit {} {} {}{}",
        if armed.live { "live" } else { "shadow" },
        armed.limit,
        shown.join(" "),
        if hit { "OVER" } else { "ok" },
        if cached { " cached" } else { "" }
    );
}

pub async fn act_known(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    id: i64,
    all: &[f32; super::CONCEPT_SLOTS],
    armed: &Armed,
    cached: bool,
) {
    report(chat, id, all, armed, cached);
    let Some((concept, margin)) = worst(all, armed) else {
        return;
    };
    if !over(margin, armed.limit) || !armed.live {
        return;
    }
    if let Err(e) = message.delete().await {
        eprintln!("concept: could not delete in {chat}: {e}");
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    let chances = match super::strict::punish(ctx, message, chat, concept.key).await {
        super::strict::Outcome::Announced => return,
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::notice::send(ctx, message, chat, concept.notice, chances).await;
}

#[allow(clippy::too_many_arguments)]
pub async fn act_detached(
    ctx: &Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    id: i64,
    all: &[f32; super::CONCEPT_SLOTS],
    armed: &Armed,
    sender: Option<i64>,
    name: &str,
) {
    report(chat, id, all, armed, false);
    let Some((concept, margin)) = worst(all, armed) else {
        return;
    };
    if !over(margin, armed.limit) || !armed.live {
        return;
    }
    match ctx.client.delete_messages(chat_ref, &[message_id]).await {
        Ok(0) => {
            eprintln!("concept: delete affected nothing in {chat} msg {message_id}");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("concept: could not delete in {chat} msg {message_id}: {e}");
            return;
        }
    }
    ctx.bump(chat, super::stats::DELETED);
    nsfw::punish_and_notify(ctx, chat, chat_ref, sender, name, concept.key, concept.notice).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_fits_the_cache_row() {
        assert!(
            CONCEPTS.len() <= super::super::CONCEPT_SLOTS,
            "{} concepts will not fit {} slots",
            CONCEPTS.len(),
            super::super::CONCEPT_SLOTS
        );
    }

    #[test]
    fn a_chat_reads_only_its_own_concepts() {
        let mut all = [f32::MIN; super::super::CONCEPT_SLOTS];
        all[0] = 0.061;
        all[1] = 0.004;
        all[2] = 0.088;

        let armed: Vec<&'static Concept> = CONCEPTS.iter().take(2).collect();
        let armed = Armed {
            concepts: armed,
            limit: 30,
            live: true,
        };
        let (concept, margin) = worst(&all, &armed).expect("a winner among the armed");

        assert_eq!(concept.key, CONCEPTS[0].key);
        assert!((margin - 0.061).abs() < 1e-6);
    }

    #[test]
    fn every_concept_is_named_once_and_is_a_real_lock() {
        let mut keys: Vec<&str> = CONCEPTS.iter().map(|c| c.key).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two concepts share a settings key");

        let mut names: Vec<&str> = CONCEPTS
            .iter()
            .flat_map(|c| c.names.iter().copied())
            .collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two concepts answer to the same word");

        for concept in CONCEPTS {
            assert!(
                super::super::locks::LOCKS
                    .iter()
                    .any(|lock| lock.key == concept.key),
                "«{}» is a concept but not a lock",
                concept.names[0]
            );
        }
    }

    #[test]
    fn the_vectors_are_unit_length_and_distinct() {
        let all: Vec<&[f32; DIM]> = CONCEPTS
            .iter()
            .map(|c| c.vector)
            .chain(std::iter::once(&vectors::BACKGROUND))
            .collect();
        for v in &all {
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "not unit length: {norm}");
        }
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert!(dot(*a, *b) < 0.99, "two concepts are the same direction");
            }
        }
    }

    #[test]
    fn the_default_limit_separates_what_was_measured() {
        let innocent = [0.015f32, -0.000, -0.016, -0.006, -0.018];
        let matches = [0.064f32, 0.053, 0.035, 0.026];

        for m in innocent {
            assert!(
                !over(m, DEFAULT_LIMIT),
                "an innocent margin of {m} would delete"
            );
        }
        for m in matches {
            assert!(m > 0.0);
        }

        assert!(over(0.064, DEFAULT_LIMIT));
        assert!(over(0.053, DEFAULT_LIMIT));
        assert!(over(0.035, DEFAULT_LIMIT));
        assert!(over(0.026, 25));

        assert!(LIMIT_PRESETS.contains(&DEFAULT_LIMIT));
        assert!(DEFAULT_LIMIT >= LIMIT_RANGE.0 && DEFAULT_LIMIT <= LIMIT_RANGE.1);
    }
}
