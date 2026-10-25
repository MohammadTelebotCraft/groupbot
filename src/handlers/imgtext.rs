use std::collections::HashMap;

use super::imgfilter::DIM;
use super::vision::beside_the_binary;

type MergeTable = HashMap<(u32, u32), (u32, u32)>;

const MODEL_FILE: &str = "vision_text.onnx";
const VOCAB_FILE: &str = "vision_text_vocab.txt";
const MERGES_FILE: &str = "vision_text_merges.txt";

const MODEL_SHA256: &str = "3ef95a696a9503dfb1fbd2121910d4bdcae2e1458b069f5d99d3c1167fd9cf6f";
const VOCAB_SHA256: &str = "8a6f76ca6767969cb17e1c356167105fd6f585294d6da0c5fe54663708254c0c";
const MERGES_SHA256: &str = "626454b548d013402acc84f01c56716e506b36d01395f1bd1252720c5edecd28";

pub(crate) fn initialize_once_retry<'a, T, E>(
    cell: &'a std::sync::OnceLock<T>,
    gate: &std::sync::Mutex<()>,
    build: impl FnOnce() -> Result<Option<T>, E>,
) -> Result<Option<&'a T>, E> {
    if let Some(value) = cell.get() {
        return Ok(Some(value));
    }
    let _guard = gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(value) = cell.get() {
        return Ok(Some(value));
    }
    let Some(value) = build()? else {
        return Ok(None);
    };
    Ok(Some(cell.get_or_init(|| value)))
}

fn validate_artifact(
    path: &std::path::Path,
    expected: &str,
) -> Result<(), super::nsfw::ModelError> {
    let actual = super::vision::sha256_file(path)?;
    if actual == expected {
        return Ok(());
    }
    Err(super::nsfw::ModelError::artifact(format!(
        "{} SHA-256 is {actual}, expected reviewed {expected}; refusing an unpaired vision text bundle",
        path.display()
    )))
}

const TOKENS: usize = 64;

const SPACE_MARK: char = '\u{2581}';

const TEMPLATES: &[&str] = &[
    "{}",
    "عکس {}",
    "تصویری از {}",
    "a photo of {}",
    "a photograph of {}",
    "a picture of {}",
    "an image of {}",
    "a close-up photo of {}",
];

const IDLE: std::time::Duration = std::time::Duration::from_secs(600);

static LOADED: std::sync::Mutex<Option<(std::sync::Arc<super::nsfw::Session>, std::time::Instant)>> =
    std::sync::Mutex::new(None);
static REAPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn model() -> Result<Option<std::sync::Arc<super::nsfw::Session>>, super::nsfw::ModelError> {
    static MISSING_WARNING: std::sync::Once = std::sync::Once::new();
    let mut loaded = LOADED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((session, used)) = loaded.as_mut() {
        *used = std::time::Instant::now();
        return Ok(Some(std::sync::Arc::clone(session)));
    }
    let Some(path) = beside_the_binary(MODEL_FILE)? else {
        MISSING_WARNING.call_once(|| {
            eprintln!("imgtext: {MODEL_FILE} is not beside the binary, so text filters are inert");
        });
        return Ok(None);
    };
    validate_artifact(&path, MODEL_SHA256)?;
    let session = super::nsfw::open_path(&path, "text tower").ok_or_else(|| {
        super::nsfw::ModelError::Runtime("vision text tower would not load".to_owned())
    })?;
    let session = std::sync::Arc::new(session);
    *loaded = Some((std::sync::Arc::clone(&session), std::time::Instant::now()));
    if !REAPING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        std::thread::spawn(reap);
    }
    Ok(Some(session))
}

fn reap() {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
        let mut loaded = LOADED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if loaded.as_ref().is_none_or(|(_, used)| used.elapsed() >= IDLE) {
            let dropped = loaded.take();
            REAPING.store(false, std::sync::atomic::Ordering::Release);
            drop(loaded);
            if let Some(dropped) = dropped {
                drop(dropped);
                super::nsfw::trim::release_freed();
                log::info!("imgtext: text tower dropped after {}s without a phrase", IDLE.as_secs());
            }
            return;
        }
    }
}

pub struct Bpe {
    ids: HashMap<String, u32>,
    merges: MergeTable,
    bytes: [u32; 256],
    eos: u32,
}

pub(super) fn unescape(line: &str) -> String {
    if !line.contains('\\') {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('s') => out.push(' '),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn read_vocab(text: &str) -> Option<(HashMap<String, u32>, [u32; 256], u32)> {
    let mut ids: HashMap<String, u32> = HashMap::with_capacity(text.len() / 8);
    for (at, line) in text.lines().enumerate() {
        let piece = unescape(line.trim_end_matches('\r'));
        let id = u32::try_from(at).ok()?;
        if piece.is_empty() || ids.insert(piece, id).is_some() {
            return None;
        }
    }
    let (eos, unk) = (*ids.get("<eos>")?, *ids.get("<unk>")?);
    let mut bytes = [unk; 256];
    for (value, slot) in bytes.iter_mut().enumerate() {
        if let Some(id) = ids.get(&format!("<0x{value:02X}>")) {
            *slot = *id;
        }
    }
    Some((ids, bytes, eos))
}

fn read_merges(
    text: &str,
    ids: &HashMap<String, u32>,
) -> Result<MergeTable, super::nsfw::ModelError> {
    let mut merges = HashMap::with_capacity(text.len() / 16);
    for (rank, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some((left, right)) = line.split_once(' ') else {
            return Err(super::nsfw::ModelError::artifact(format!(
                "vision text merges line {} has no separator",
                rank + 1
            )));
        };
        if left.is_empty() || right.is_empty() || right.contains(' ') {
            return Err(super::nsfw::ModelError::artifact(format!(
                "vision text merges line {} does not contain exactly one ASCII-space separator",
                rank + 1
            )));
        }
        let (left, right) = (unescape(left), unescape(right));
        let joined = format!("{left}{right}");
        let (Some(&left), Some(&right), Some(&merged)) =
            (ids.get(&left), ids.get(&right), ids.get(&joined))
        else {
            continue;
        };
        let rank = u32::try_from(rank).map_err(|_| {
            super::nsfw::ModelError::artifact("vision text merge table exceeds u32 ranks")
        })?;
        merges.entry((left, right)).or_insert((rank, merged));
    }
    Ok(merges)
}

fn load(vocab: &std::path::Path, merges: &std::path::Path) -> Result<Bpe, super::nsfw::ModelError> {
    let vocab_text = std::fs::read_to_string(vocab).map_err(|error| {
        super::nsfw::ModelError::artifact(format!("could not read {}: {error}", vocab.display()))
    })?;
    let (ids, bytes, eos) = read_vocab(&vocab_text).ok_or_else(|| {
        super::nsfw::ModelError::artifact(format!(
            "{} is not a valid vision text vocabulary",
            vocab.display()
        ))
    })?;
    let merges_text = std::fs::read_to_string(merges).map_err(|error| {
        super::nsfw::ModelError::artifact(format!("could not read {}: {error}", merges.display()))
    })?;
    let merges = read_merges(&merges_text, &ids)?;
    Ok(Bpe {
        ids,
        merges,
        bytes,
        eos,
    })
}

fn bpe() -> Result<Option<&'static Bpe>, super::nsfw::ModelError> {
    static CELL: std::sync::OnceLock<Bpe> = std::sync::OnceLock::new();
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static MISSING_WARNING: std::sync::Once = std::sync::Once::new();
    initialize_once_retry(&CELL, &GATE, || {
        let (Some(vocab), Some(merges)) = (
            beside_the_binary(VOCAB_FILE)?,
            beside_the_binary(MERGES_FILE)?,
        ) else {
            MISSING_WARNING.call_once(|| {
                eprintln!(
                    "imgtext: {VOCAB_FILE} or {MERGES_FILE} is not beside the binary, so text filters are inert"
                );
            });
            return Ok(None);
        };
        validate_artifact(&vocab, VOCAB_SHA256)?;
        validate_artifact(&merges, MERGES_SHA256)?;
        let bpe = load(&vocab, &merges)?;
        eprintln!(
            "imgtext: {} vocabulary pieces, {} merges",
            bpe.ids.len(),
            bpe.merges.len()
        );
        Ok(Some(bpe))
    })
}

impl Bpe {
    fn symbols(&self, text: &str) -> Vec<u32> {
        let mut out = Vec::with_capacity(text.len());
        let mut buffer = [0u8; 4];
        for c in text.chars() {
            let c = if c == ' ' { SPACE_MARK } else { c };
            if let Some(&id) = self.ids.get(c.encode_utf8(&mut buffer) as &str) {
                out.push(id);
                continue;
            }
            for byte in c.encode_utf8(&mut buffer).as_bytes() {
                out.push(self.bytes[*byte as usize]);
            }
        }
        out
    }

    fn merge(&self, mut symbols: Vec<u32>) -> Vec<u32> {
        while symbols.len() > 1 {
            let mut best: Option<(u32, usize, u32)> = None;
            for at in 0..symbols.len() - 1 {
                let Some(&(rank, merged)) = self.merges.get(&(symbols[at], symbols[at + 1])) else {
                    continue;
                };
                if best.is_none_or(|(seen, _, _)| rank < seen) {
                    best = Some((rank, at, merged));
                }
            }
            let Some((_, at, merged)) = best else {
                break;
            };
            let (left, right) = (symbols[at], symbols[at + 1]);
            let mut next = Vec::with_capacity(symbols.len() - 1);
            let mut index = 0;
            while index < symbols.len() {
                if index + 1 < symbols.len()
                    && symbols[index] == left
                    && symbols[index + 1] == right
                {
                    next.push(merged);
                    index += 2;
                    continue;
                }
                next.push(symbols[index]);
                index += 1;
            }
            symbols = next;
        }
        symbols
    }

    pub fn encode(&self, text: &str) -> Vec<i64> {
        let mut ids = self.merge(self.symbols(text));
        ids.truncate(TOKENS - 1);
        ids.push(self.eos);
        let mut out: Vec<i64> = ids.into_iter().map(i64::from).collect();
        out.resize(TOKENS, 0);
        out
    }
}

fn run(session: &super::nsfw::Session, ids: Vec<i64>) -> Result<Vec<f32>, super::nsfw::ModelError> {
    let value =
        ort::value::Value::from_array((vec![1i64, TOKENS as i64], ids)).map_err(|error| {
            super::nsfw::ModelError::contract(format!("vision text input: {error}"))
        })?;
    let mut session = session
        .lock()
        .map_err(|_| super::nsfw::ModelError::Poisoned("vision text tower"))?;
    let output = session
        .run(ort::inputs![value])
        .map_err(|error| super::nsfw::ModelError::Runtime(format!("vision text pass: {error}")))?;
    let (_, output) = output
        .into_iter()
        .next()
        .ok_or_else(|| super::nsfw::ModelError::contract("vision text graph has no output"))?;
    let (shape, values) = output.try_extract_tensor::<f32>().map_err(|error| {
        super::nsfw::ModelError::contract(format!("vision text output is not f32: {error}"))
    })?;
    if shape[..] != [DIM as i64] && shape[..] != [1, DIM as i64] {
        return Err(super::nsfw::ModelError::contract(format!(
            "vision text output shape {shape:?}, expected [{DIM}] or [1, {DIM}]"
        )));
    }
    super::vision::validated_unit(values, "vision text embedding")
}

#[derive(Debug, PartialEq)]
pub enum EmbedOutcome {
    Unavailable,
    Empty,
    Embedding(Vec<f32>),
}

const ENGLISH_TEMPLATES: &[&str] = &[
    "{}",
    "a photo of {}",
    "a photograph of {}",
    "a picture of {}",
    "an image of {}",
    "a close-up photo of {}",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frames {
    Typed,
    English,
}

fn ensemble(
    phrase: &str,
    bpe: &Bpe,
    templates: &[&str],
    mut infer: impl FnMut(Vec<i64>) -> Result<Vec<f32>, super::nsfw::ModelError>,
) -> Result<Vec<f32>, super::nsfw::ModelError> {
    let mut sum = vec![0f32; DIM];
    for template in templates {
        let framed = template.replace("{}", phrase);
        let out = infer(bpe.encode(&framed))?;
        for (at, value) in out.into_iter().enumerate() {
            sum[at] += value;
        }
    }
    super::vision::validated_unit(&sum, "vision text template ensemble")
}

fn canonical_phrase(phrase: &str) -> &str {
    match phrase {
        "english springer" => "english springer spaniel",
        _ => phrase,
    }
}

pub fn try_embed(phrase: &str) -> Result<EmbedOutcome, super::nsfw::ModelError> {
    try_embed_framed(phrase, Frames::Typed)
}

pub fn try_embed_framed(phrase: &str, frames: Frames) -> Result<EmbedOutcome, super::nsfw::ModelError> {
    let phrase = phrase.trim().to_lowercase();
    if phrase.is_empty() {
        return Ok(EmbedOutcome::Empty);
    }
    let (Some(session), Some(bpe)) = (model()?, bpe()?) else {
        return Ok(EmbedOutcome::Unavailable);
    };
    let templates = match frames {
        Frames::Typed => TEMPLATES,
        Frames::English => ENGLISH_TEMPLATES,
    };
    ensemble(canonical_phrase(&phrase), bpe, templates, |ids| run(&session, ids))
        .map(EmbedOutcome::Embedding)
}

pub fn embed_framed(phrase: &str, frames: Frames) -> Option<Vec<f32>> {
    match try_embed_framed(phrase, frames) {
        Ok(EmbedOutcome::Embedding(vector)) => Some(vector),
        Ok(EmbedOutcome::Unavailable | EmbedOutcome::Empty) => None,
        Err(error) => {
            log::error!("imgtext: embedding failed: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_or_missing_initialization_retries_then_caches_success() {
        let cell = std::sync::OnceLock::new();
        let gate = std::sync::Mutex::new(());
        let mut calls = 0;
        let mut build = || {
            calls += 1;
            match calls {
                1 => Ok(None),
                2 => Err("transient load error"),
                3 => Ok(Some(42)),
                _ => panic!("successful initialization should be cached"),
            }
        };
        assert_eq!(initialize_once_retry(&cell, &gate, &mut build), Ok(None));
        assert_eq!(
            initialize_once_retry(&cell, &gate, &mut build),
            Err("transient load error")
        );
        assert_eq!(
            initialize_once_retry(&cell, &gate, &mut build),
            Ok(Some(&42))
        );
        assert_eq!(
            initialize_once_retry(&cell, &gate, &mut build),
            Ok(Some(&42))
        );
        assert_eq!(calls, 3);
    }

    fn tiny() -> Bpe {
        let mut pieces: Vec<String> = vec![
            "<pad>".to_owned(),
            "<eos>".to_owned(),
            "<unk>".to_owned(),
            "a".to_owned(),
            "b".to_owned(),
            "c".to_owned(),
            "ab".to_owned(),
            "bc".to_owned(),
            "abc".to_owned(),
            "\u{2581}".to_owned(),
            "\u{2581}a".to_owned(),
        ];
        for value in 0..256u32 {
            pieces.push(format!("<0x{value:02X}>"));
        }
        let text = pieces.join("\n");
        let (ids, bytes, eos) = read_vocab(&text).expect("the tiny vocabulary is complete");
        let merges = read_merges("a b\nb c\nab c\n", &ids).expect("valid merge table");
        Bpe {
            ids,
            merges,
            bytes,
            eos,
        }
    }

    #[test]
    fn merges_lowest_rank_first() {
        let bpe = tiny();
        let ids = bpe.encode("abc");
        assert_eq!(ids[0], i64::from(bpe.ids["abc"]));
        assert_eq!(ids[1], i64::from(bpe.eos));
    }

    #[test]
    fn a_phrase_in_the_vocabulary_is_still_built_by_merging() {
        let mut bpe = tiny();
        bpe.merges = read_merges("b c\na b\nab c\n", &bpe.ids).expect("valid merge table");
        let ids = bpe.encode("abc");
        assert_ne!(
            ids[0],
            i64::from(bpe.ids["abc"]),
            "the shortcut would have produced the whole piece"
        );
    }

    #[test]
    fn a_space_becomes_the_mark_and_an_unknown_character_becomes_its_bytes() {
        let bpe = tiny();
        assert_eq!(bpe.symbols(" a"), vec![bpe.ids["\u{2581}"], bpe.ids["a"]]);
        assert_eq!(bpe.symbols("خ").len(), 2, "خ is two bytes in UTF-8");
        assert!(bpe.symbols("خ").iter().all(|id| *id >= bpe.bytes[0]));
    }

    #[test]
    fn malformed_merge_syntax_is_an_artifact_error() {
        let bpe = tiny();
        assert!(matches!(
            read_merges("a b\ncorrupt\nab c\n", &bpe.ids),
            Err(super::super::nsfw::ModelError::Artifact(_))
        ));
        assert!(read_merges("a  b\n", &bpe.ids).is_err());
    }

    #[test]
    fn non_ascii_whitespace_is_valid_merge_data() {
        let ids = HashMap::from([
            ("\u{202f}".to_owned(), 1),
            (":".to_owned(), 2),
            ("\u{202f}:".to_owned(), 3),
            ("\u{202f}\u{202f}".to_owned(), 4),
        ]);
        let merges = read_merges("\u{202f} :\n\u{202f} \u{202f}\n", &ids)
            .expect("U+202F is data, not the separator");
        assert_eq!(merges.get(&(1, 2)), Some(&(0, 3)));
        assert_eq!(merges.get(&(1, 1)), Some(&(1, 4)));
    }

    #[test]
    fn one_failed_template_rejects_the_whole_ensemble() {
        let bpe = tiny();
        let mut calls = 0usize;
        let result = ensemble("abc", &bpe, TEMPLATES, |_| {
            calls += 1;
            if calls == 2 {
                return Err(super::super::nsfw::ModelError::Runtime(
                    "injected template failure".to_owned(),
                ));
            }
            let mut vector = vec![0.0; DIM];
            vector[0] = 1.0;
            Ok(vector)
        });
        assert!(matches!(
            result,
            Err(super::super::nsfw::ModelError::Runtime(_))
        ));
        assert_eq!(calls, 2, "the ensemble must stop at the first failed pass");
    }

    #[test]
    fn an_encoding_is_padded_and_bounded() {
        let bpe = tiny();
        let ids = bpe.encode("a");
        assert_eq!(ids.len(), TOKENS);
        assert_eq!(ids[1], i64::from(bpe.eos));
        assert!(ids[2..].iter().all(|id| *id == 0), "the tail is padding");

        let long = "abc".repeat(200);
        let ids = bpe.encode(&long);
        assert_eq!(ids.len(), TOKENS);
        assert_eq!(
            ids[TOKENS - 1],
            i64::from(bpe.eos),
            "the separator survives truncation"
        );
    }

    #[test]
    fn escapes_round_trip() {
        assert_eq!(unescape("a\\sb"), "a b");
        assert_eq!(unescape("a\\nb"), "a\nb");
        assert_eq!(unescape("a\\\\b"), "a\\b");
        assert_eq!(unescape("plain"), "plain");
    }

    #[test]
    #[ignore = "needs vision_text_vocab.txt and vision_text_merges.txt from an export"]
    fn agrees_with_the_reference_tokenizer() {
        let dir = std::path::PathBuf::from(
            std::env::var("VISION_FILES").unwrap_or_else(|_| "target/release".to_owned()),
        );
        let bpe = load(&dir.join(VOCAB_FILE), &dir.join(MERGES_FILE))
            .expect("the exported vocabulary and merges");
        for (phrase, want) in [
            ("a photo of a cat", &[194661, 2567, 544, 444, 4192, 1][..]),
            ("خودرو لوکس", &[111832, 3615, 42433, 18431, 1][..]),
            ("سیگار", &[9395, 103211, 1][..]),
            ("cigarette", &[177150, 1][..]),
            ("Hello World!", &[4307, 3675, 194743, 1][..]),
            ("عکس سگ", &[40040, 149326, 1][..]),
            ("中文", &[413, 369, 358, 415, 335, 320, 1][..]),
        ] {
            let got = bpe.encode(phrase);
            assert_eq!(&got[..want.len()], want, "«{phrase}»");
            assert!(
                got[want.len()..].iter().all(|id| *id == 0),
                "«{phrase}» padding"
            );
        }
    }

    #[test]
    #[ignore = "needs vision_text.onnx and its two files from an export"]
    fn a_persian_phrase_lands_next_to_its_own_direction() {
        super::super::nsfw::configure_model_tests()
            .expect("configure the test's VISION_FILES bundle before loading the text tower");
        use super::super::concept_vectors as vectors;
        use super::super::vision::dot;
        for (phrase, near, far, what) in [
            ("عکس خانوادگی", &vectors::SAFE, &vectors::EXPLICIT, "ordinary"),
            ("پورن", &vectors::EXPLICIT, &vectors::SAFE, "explicit"),
        ] {
            let vector = embed_framed(phrase, Frames::Typed).expect("the text tower beside the test binary");
            assert_eq!(vector.len(), DIM);
            let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "not unit length: {norm}");
            let (near, far) = (dot(&vector, near), dot(&vector, far));
            assert!(
                near > far,
                "«{phrase}» is not nearer the {what} direction ({near:.4} against {far:.4})"
            );
        }
    }

    #[test]
    #[ignore = "needs vision_text.onnx and its two files from an export"]
    fn dropping_the_tower_returns_its_memory() {
        super::super::nsfw::configure_model_tests()
            .expect("configure the test's VISION_FILES bundle before loading the text tower");
        let anonymous_mb = || {
            let status = std::fs::read_to_string("/proc/self/status").expect("status");
            status
                .lines()
                .find_map(|line| line.strip_prefix("RssAnon:"))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|kb| kb.parse::<u64>().ok())
                .expect("RssAnon")
                / 1024
        };
        embed_framed("کتاب", Frames::Typed).expect("the text tower beside the test binary");
        let held = anonymous_mb();
        drop(LOADED.lock().unwrap().take());
        super::super::nsfw::trim::release_freed();
        let freed = held.saturating_sub(anonymous_mb());
        eprintln!("text tower: {held} MB anonymous while held, {freed} MB returned on drop");
        assert!(freed >= 400, "only {freed} MB came back when the tower was dropped");
        embed_framed("کتاب", Frames::Typed).expect("the tower reloads after a drop");
    }

    #[test]
    fn every_template_carries_the_phrase() {
        for template in TEMPLATES {
            assert!(template.contains("{}"), "«{template}» drops the phrase");
            assert!(!template.contains('\u{200c}'));
            assert_ne!(template.replace("{}", "خودرو"), *template);
        }
    }

    #[test]
    fn only_the_abbreviated_breed_name_is_expanded() {
        assert_eq!(
            canonical_phrase("english springer"),
            "english springer spaniel"
        );
        assert_eq!(
            canonical_phrase("english springer dog"),
            "english springer dog"
        );
        assert_eq!(canonical_phrase("سیگار"), "سیگار");
    }

    #[test]
    fn every_english_frame_carries_the_phrase_and_is_lowercase() {
        for template in ENGLISH_TEMPLATES {
            assert!(template.contains("{}"), "«{template}» drops the phrase");
            assert_eq!(*template, template.to_lowercase());
        }
    }
}
