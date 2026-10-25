use std::collections::HashMap;
use std::sync::Arc;

use grammers_client::message::Message;
use grammers_client::session::types::PeerRef;

use super::{Ctx, VisionSource};
use super::concept_head_vectors::{COUNT, HEADS};
use super::nsfw;
use super::vision::dot;

pub struct Concept {
    pub key: &'static str,
    pub names: &'static [&'static str],
    pub notice: &'static str,
}

pub const CONCEPTS: &[Concept] = &[
    Concept {
        key: "c_cig",
        names: &["سیگار", "دخانیات", "قلیان"],
        notice: "تصویر سیگار و دخانیات",
    },
    Concept {
        key: "c_alc",
        names: &["مشروب", "الکل", "مشروبات"],
        notice: "تصویر مشروبات الکلی",
    },
    Concept {
        key: "c_gun",
        names: &["اسلحه", "سلاح", "چاقو"],
        notice: "تصویر تفنگ یا چاقو",
    },
    Concept {
        key: "c_bet",
        names: &["قمار", "شرط بندی", "شرطبندی"],
        notice: "تصویر قمار و شرط بندی",
    },
    Concept {
        key: "c_drg",
        names: &["مواد", "مواد مخدر"],
        notice: "تصویر مواد مخدر",
    },
    Concept {
        key: "c_bld",
        names: &["خون", "خونریزی"],
        notice: "تصویر خون و خونریزی",
    },
];

const _: () = assert!(COUNT == CONCEPTS.len());

pub const SHADOW: &str = "cq_shadow";

pub const LIMIT: &str = "cq_lim";

pub const LIMIT_RANGE: (u32, u32) = (5, 60);
pub const LIMIT_PRESETS: &[u32] = &[10, 15, 20, 25, 30, 40];
pub const DEFAULT_LIMIT: u32 = 20;

pub(crate) const LOGITS_PER_THOUSANDTH: f32 = 0.1;

const DEFAULT_MARGIN: f32 = DEFAULT_LIMIT as f32 / 1000.0;

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings.with_chat(chat, |settings| {
        settings.number(LIMIT, DEFAULT_LIMIT, LIMIT_RANGE)
    })
}

pub(crate) fn logit(slot: usize, embedding: &[f32]) -> f32 {
    let head = &HEADS[slot];
    dot(embedding, &head.weights) + head.bias
}

fn margin_of(slot: usize, embedding: &[f32]) -> f32 {
    DEFAULT_MARGIN + (logit(slot, embedding) - HEADS[slot].cut) / LOGITS_PER_THOUSANDTH / 1000.0
}

pub fn margins_from(embedding: &[f32]) -> [f32; super::CONCEPT_SLOTS] {
    let mut out = [f32::MIN; super::CONCEPT_SLOTS];
    for (slot, margin) in out.iter_mut().enumerate().take(CONCEPTS.len()) {
        *margin = margin_of(slot, embedding);
    }
    out
}

pub(crate) fn over(margin: f32, limit: u32) -> bool {
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

pub(crate) fn worst(
    all: &[f32; super::CONCEPT_SLOTS],
    armed: &Armed,
) -> Option<(&'static Concept, f32)> {
    armed
        .concepts
        .iter()
        .filter_map(|concept| {
            let slot = CONCEPTS.iter().position(|c| c.key == concept.key)?;
            Some((*concept, all[slot]))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

fn over_limit(all: &[f32; super::CONCEPT_SLOTS], armed: &Armed) -> Vec<(&'static Concept, usize)> {
    let mut out: Vec<(&'static Concept, usize)> = armed
        .concepts
        .iter()
        .filter_map(|concept| Some((*concept, CONCEPTS.iter().position(|c| c.key == concept.key)?)))
        .filter(|(_, slot)| over(all[*slot], armed.limit))
        .collect();
    out.sort_by(|a, b| all[b.1].total_cmp(&all[a.1]));
    out
}

pub(crate) fn needs_look(
    ctx: &Ctx,
    id: i64,
    source: Option<&VisionSource>,
    all: &[f32; super::CONCEPT_SLOTS],
    armed: &Armed,
) -> bool {
    armed.live
        && super::verify::installed()
        && over_limit(all, armed).iter().any(|(concept, _)| {
            source.is_none_or(|source| ctx.known_custom(id, super::verify::print(concept.key), source).is_none())
        })
}

pub(crate) async fn second_look(
    ctx: &Ctx,
    image: &Arc<image::RgbImage>,
    (chat, id): (i64, i64),
    source: &VisionSource,
    cacheable: bool,
    all: &[f32; super::CONCEPT_SLOTS],
    armed: &Armed,
) -> HashMap<&'static str, f32> {
    let mut fresh = HashMap::new();
    if !armed.live || !super::verify::installed() {
        return fresh;
    }
    for (concept, _) in over_limit(all, armed) {
        let print = super::verify::print(concept.key);
        if ctx.known_custom(id, print, source).is_some() {
            continue;
        }
        let key = concept.key;
        match super::verify::judge(Arc::clone(image), key, CONCEPT_YES, move |ask: &mut super::verify::Asker<'_>| {
            decide(key, ask)
        })
        .await
        {
            Ok(Some(p)) => {
                log::info!("concept: chat {chat} file {id} {} second look P(yes) {p:.3}", concept.key);
                if cacheable {
                    ctx.remember_custom(id, print, p, source.clone());
                }
                fresh.insert(concept.key, p);
            }
            Ok(None) => {
                fresh.insert(concept.key, 1.0);
            }
            Err(error) => log::error!("concept: chat {chat} file {id} {} second look failed: {error}", concept.key),
        }
    }
    fresh
}

const CONCEPT_YES: f32 = 0.85;

const GUN: &str = "a gun";
const SWORD: &str = "a sword";
const CUTLERY: &str = "cutlery or a kitchen knife";
const KNIFE: &str = "a pocket knife, a combat knife or a dagger";
const GUN_YES: f32 = 0.5;
const SWORD_YES: f32 = 0.4;
const CUTLERY_YES: f32 = 0.9;
const KNIFE_YES: f32 = 0.3;
const GAMBLED_WITH: [&str; 3] = [
    "a casino, poker chips, playing cards, a roulette wheel, a slot machine or a betting website",
    "sports betting",
    "a poker game or a poker tournament",
];
const GAMBLED_WITH_YES: f32 = 0.05;
const NOT_TOBACCO: &str = "a no-smoking sign";
const SIGN: &str = "a sign or a poster";
const WOUND: &str = "a wound or an injured person";
const SIGN_YES: f32 = 0.9;
const WOUND_YES: f32 = 0.5;

pub(crate) fn decide(
    key: &'static str,
    ask: &mut super::verify::Asker<'_>,
) -> Result<Option<f32>, nsfw::ModelError> {
    let Some(main) = ask(key)? else {
        return Ok(None);
    };
    if main < CONCEPT_YES {
        return Ok(Some(main));
    }
    let logit = |p: f32| (p.max(1e-6) / (1.0 - p).max(1e-6)).ln();
    let kept = match key {
        "c_gun" => {
            ask(GUN)?.is_none_or(|gun| gun >= GUN_YES)
                || (ask(SWORD)?.is_none_or(|sword| sword < SWORD_YES)
                    && ask(CUTLERY)?.is_none_or(|cutlery| cutlery < CUTLERY_YES)
                    && ask(KNIFE)?.is_none_or(|knife| knife >= KNIFE_YES))
        }
        "c_bet" => {
            let mut seen = false;
            for question in GAMBLED_WITH {
                if ask(question)?.is_none_or(|p| p >= GAMBLED_WITH_YES) {
                    seen = true;
                    break;
                }
            }
            seen
        }
        "c_cig" => ask(NOT_TOBACCO)?.is_none_or(|other| logit(main) - logit(other) >= -1.0),
        "c_bld" => {
            ask(SIGN)?.is_none_or(|sign| sign < SIGN_YES) || ask(WOUND)?.is_none_or(|wound| wound >= WOUND_YES)
        }
        _ => true,
    };
    Ok(Some(if kept { main } else { 0.0 }))
}

pub(crate) fn with_verdicts(
    ctx: &Ctx,
    id: i64,
    source: Option<&VisionSource>,
    all: &[f32; super::CONCEPT_SLOTS],
    armed: &Armed,
    fresh: &HashMap<&'static str, f32>,
) -> [f32; super::CONCEPT_SLOTS] {
    let mut out = *all;
    if !armed.live || !super::verify::installed() {
        return out;
    }
    for (concept, slot) in over_limit(all, armed) {
        let answer = fresh.get(concept.key).copied().or_else(|| {
            source.and_then(|source| ctx.known_custom(id, super::verify::print(concept.key), source))
        });
        if answer.is_none_or(|p| p < CONCEPT_YES) {
            out[slot] = f32::MIN;
        }
    }
    out
}

fn report(chat: i64, id: i64, all: &[f32; super::CONCEPT_SLOTS], armed: &Armed, cached: bool) {
    let shown: Vec<String> = armed
        .concepts
        .iter()
        .filter_map(|concept| {
            let slot = CONCEPTS.iter().position(|c| c.key == concept.key)?;
            Some(match all[slot] {
                margin if margin <= f32::MIN => format!("{} second-look-no", concept.key),
                margin => format!("{} {margin:+.3}", concept.key),
            })
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
    match message.delete_critical_counted().await {
        Ok(0) => return,
        Ok(_) => {}
        Err(e) => {
            eprintln!("concept: could not delete in {chat}: {e}");
            return;
        }
    }
    if !ctx.claim_moderation(chat, message.id()) {
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

pub struct Detached<'a> {
    pub chat: i64,
    pub chat_ref: PeerRef,
    pub message_id: i32,
    pub media_id: i64,
    pub margins: &'a [f32; super::CONCEPT_SLOTS],
    pub armed: &'a Armed,
    pub sender: Option<i64>,
    pub name: &'a str,
}

pub async fn act_detached(ctx: &Arc<Ctx>, detached: Detached<'_>) {
    let Detached {
        chat,
        chat_ref,
        message_id,
        media_id,
        margins,
        armed,
        sender,
        name,
    } = detached;
    report(chat, media_id, margins, armed, false);
    let Some((concept, margin)) = worst(margins, armed) else {
        return;
    };
    if !over(margin, armed.limit) || !armed.live {
        return;
    }
    match ctx
        .client
        .delete_messages_critical(chat_ref, &[message_id])
        .await
    {
        Ok(0) => {
            eprintln!("concept: delete returned zero events in {chat} msg {message_id}");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("concept: could not delete in {chat} msg {message_id}: {e}");
            return;
        }
    }
    if !ctx.claim_moderation(chat, message_id) {
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    nsfw::punish_and_notify(
        ctx,
        nsfw::DetachedModeration {
            chat,
            chat_ref,
            message_id,
            sender,
            name,
            cause: concept.key,
            reason: concept.notice,
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::vision::{DIM, unit};

    #[test]
    #[ignore = "needs the second look's files under VISION_FILES, CONCEPT_LIST and CONCEPT_OUT"]
    fn concept_decisions_for_a_list() {
        nsfw::configure_model_tests().expect("model test configuration");
        let (Ok(list), Ok(out)) = (std::env::var("CONCEPT_LIST"), std::env::var("CONCEPT_OUT")) else {
            return;
        };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
        let mut lines = Vec::new();
        let mut partial = std::fs::File::create(format!("{out}.partial")).expect("CONCEPT_OUT.partial");
        for line in std::fs::read_to_string(list).expect("CONCEPT_LIST").lines() {
            let Some((key, path)) = line.split_once('\t') else {
                continue;
            };
            let Some(concept) = CONCEPTS.iter().find(|concept| concept.key == key) else {
                continue;
            };
            let image = Arc::new(image::open(path).expect("a picture").to_rgb8());
            let key = concept.key;
            let answer = runtime
                .block_on(super::super::verify::judge(image, key, CONCEPT_YES, move |ask: &mut super::super::verify::Asker<'_>| {
                    decide(key, ask)
                }))
                .expect("a look");
            lines.push(format!("{key}\t{path}\t{}", answer.map_or("none".to_owned(), |p| format!("{p:.4}"))));
            use std::io::Write as _;
            writeln!(partial, "{}", lines.last().expect("just pushed")).expect("CONCEPT_OUT.partial");
        }
        std::fs::write(out, lines.join("\n") + "\n").expect("CONCEPT_OUT");
        println!("{} decisions", lines.len());
    }

    #[test]
    fn a_concept_is_asked_what_it_must_leave_alone() {
        let decided = |key: &'static str, answers: &[(&str, f32)]| {
            let answers: HashMap<String, f32> = answers.iter().map(|(q, p)| ((*q).to_owned(), *p)).collect();
            let mut asked = Vec::new();
            let mut ask = |q: &str| -> Result<Option<f32>, nsfw::ModelError> {
                asked.push(q.to_owned());
                Ok(answers.get(q).copied())
            };
            let answer = decide(key, &mut ask).expect("a decision");
            (answer, asked)
        };
        let gun = [("c_gun", 0.99), ("a gun", 0.98), ("a sword", 0.9)];
        assert_eq!(decided("c_gun", &gun), (Some(0.99), vec!["c_gun".to_owned(), "a gun".to_owned()]));
        let sword = [("c_gun", 0.99), ("a gun", 0.01), ("a sword", 0.9), ("cutlery or a kitchen knife", 0.0)];
        assert_eq!(decided("c_gun", &sword).0, Some(0.0));
        let dagger = [("c_gun", 0.97), ("a gun", 0.02), ("a sword", 0.1), ("cutlery or a kitchen knife", 0.2), (KNIFE, 0.8)];
        assert_eq!(decided("c_gun", &dagger).0, Some(0.97));
        let chainsaw = [("c_gun", 0.9), ("a gun", 0.0), ("a sword", 0.0), ("cutlery or a kitchen knife", 0.0), (KNIFE, 0.001)];
        assert_eq!(decided("c_gun", &chainsaw).0, Some(0.0), "neither a gun nor a knife");
        let butcher = [("c_gun", 0.96), ("a gun", 0.0), ("a sword", 0.01), ("cutlery or a kitchen knife", 0.455), (KNIFE, 0.003)];
        assert_eq!(decided("c_gun", &butcher).0, Some(0.0));
        let steak = [("c_gun", 0.95), ("a gun", 0.0), ("a sword", 0.0), ("cutlery or a kitchen knife", 0.99), (KNIFE, 0.9)];
        assert_eq!(decided("c_gun", &steak).0, Some(0.0));
        assert!(!decided("c_gun", &steak).1.contains(&KNIFE.to_owned()), "surely cutlery is not asked again");
        let folder = [("c_gun", 0.99), ("a gun", 0.0), ("a sword", 0.0), ("cutlery or a kitchen knife", 0.7), (KNIFE, 0.95)];
        assert_eq!(decided("c_gun", &folder).0, Some(0.99));
        let no = [("c_gun", 0.2), ("a gun", 0.99)];
        assert_eq!(decided("c_gun", &no), (Some(0.2), vec!["c_gun".to_owned()]));
        let bowling = [("c_bet", 0.98), (GAMBLED_WITH[0], 0.01), (GAMBLED_WITH[1], 0.01), (GAMBLED_WITH[2], 0.0)];
        assert_eq!(decided("c_bet", &bowling).0, Some(0.0));
        let poker = [("c_bet", 0.99), (GAMBLED_WITH[0], 0.01), (GAMBLED_WITH[1], 0.02), (GAMBLED_WITH[2], 0.99)];
        assert_eq!(decided("c_bet", &poker).0, Some(0.99));
        let casino = [("c_bet", 0.99), (GAMBLED_WITH[0], 0.8)];
        assert_eq!(decided("c_bet", &casino), (Some(0.99), vec!["c_bet".to_owned(), GAMBLED_WITH[0].to_owned()]));
        let bookmaker = [("c_bet", 0.9), (GAMBLED_WITH[0], 0.02), (GAMBLED_WITH[1], 0.6)];
        assert_eq!(decided("c_bet", &bookmaker).0, Some(0.9));
        let sign = [("c_cig", 0.95), ("a no-smoking sign", 0.999)];
        assert_eq!(decided("c_cig", &sign).0, Some(0.0));
        let placard = [("c_bld", 0.92), (SIGN, 0.998), (WOUND, 0.047)];
        assert_eq!(decided("c_bld", &placard).0, Some(0.0));
        let stabbed = [("c_bld", 0.99), (SIGN, 0.95), (WOUND, 0.96)];
        assert_eq!(decided("c_bld", &stabbed).0, Some(0.99));
        let wound = [("c_bld", 0.99), (SIGN, 0.1)];
        assert_eq!(decided("c_bld", &wound), (Some(0.99), vec!["c_bld".to_owned(), SIGN.to_owned()]));
        let beer = [("c_alc", 0.98)];
        assert_eq!(decided("c_alc", &beer), (Some(0.98), vec!["c_alc".to_owned()]));
        assert_eq!(decided("c_drg", &[]).0, None, "a question the prompts do not know");
    }

    fn at_logit(slot: usize, logit: f32) -> Vec<f32> {
        let head = &HEADS[slot];
        let norm = dot(&head.weights, &head.weights).sqrt();
        let along = unit(&head.weights);
        let scale = (logit - head.bias) / norm;
        along.iter().map(|x| x * scale).collect()
    }

    #[test]
    fn a_head_at_its_cut_is_the_default_limit() {
        for (slot, concept) in CONCEPTS.iter().enumerate() {
            let cut = HEADS[slot].cut;
            let at = margins_from(&at_logit(slot, cut + 1e-3))[slot];
            assert!(over(at, DEFAULT_LIMIT), "{} at its cut must delete", concept.key);
            let under = margins_from(&at_logit(slot, cut - 0.05))[slot];
            assert!(!over(under, DEFAULT_LIMIT), "{} under its cut must not", concept.key);
        }
    }

    #[test]
    fn a_limit_step_is_a_tenth_of_a_logit() {
        let cut = HEADS[0].cut;
        let one_logit_up = margins_from(&at_logit(0, cut + 1.0 + 1e-3))[0];
        assert!(over(one_logit_up, DEFAULT_LIMIT + 10));
        assert!(!over(one_logit_up, DEFAULT_LIMIT + 11));
        let one_logit_down = margins_from(&at_logit(0, cut - 1.0 + 1e-3))[0];
        assert!(over(one_logit_down, DEFAULT_LIMIT - 10));
        assert!(!over(one_logit_down, DEFAULT_LIMIT - 9));
        let clear = margins_from(&at_logit(0, cut + 4.5))[0];
        assert!(over(clear, LIMIT_RANGE.1));
    }

    #[test]
    fn the_prior_of_every_head_is_ordinary() {
        let zero = vec![0.0; DIM];
        let all = margins_from(&zero);
        for (slot, concept) in CONCEPTS.iter().enumerate() {
            assert!(!over(all[slot], DEFAULT_LIMIT), "{} fires on nothing", concept.key);
            assert!(
                HEADS[slot].weights.iter().any(|w| *w != 0.0),
                "{} has no head",
                concept.key
            );
        }
        for margin in &all[CONCEPTS.len()..] {
            assert_eq!(*margin, f32::MIN);
        }
    }

    #[test]
    fn the_default_limit_includes_its_boundary() {
        assert!(over(0.020, DEFAULT_LIMIT));
        assert!(!over(0.019, DEFAULT_LIMIT));
        assert!(LIMIT_PRESETS.contains(&DEFAULT_LIMIT));
        assert!(DEFAULT_LIMIT >= LIMIT_RANGE.0 && DEFAULT_LIMIT <= LIMIT_RANGE.1);
    }

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
    fn the_heads_are_distinct() {
        for (i, a) in HEADS.iter().enumerate() {
            for b in HEADS.iter().skip(i + 1) {
                let cosine = dot(&unit(&a.weights), &unit(&b.weights));
                assert!(cosine < 0.95, "two heads point the same way: {cosine}");
            }
        }
    }
}
