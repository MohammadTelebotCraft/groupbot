
use std::collections::HashMap;

use super::vision::beside_the_binary;

const MODEL_FILE: &str = "intent.onnx";
const BIG_MODEL_FILE: &str = "intent_big.onnx";
const VOCAB_FILE: &str = "intent_vocab.txt";
const FRAMES_FILE: &str = "intent_frames.txt";

const TOKENS: usize = 128;

const BOS: u32 = 0;
const PAD: u32 = 1;
const EOS: u32 = 2;
const UNK: u32 = 3;

const PREFIX: &str = "query: ";

const UNK_PENALTY: f32 = 10.0;

const MAX_CHARS: usize = 512;

const UNK_LIMIT: f32 = 0.3;

const SPACE_MARK: char = '\u{2581}';

const TO_SPACE: &[char] = &[
    '\t', '\n', '\r', '\u{c}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{200e}', '\u{200f}',
    '\u{2028}', '\u{2029}', '\u{feff}',
];

const TO_NOTHING: &[char] = &['\u{b}', '\u{7f}'];

fn artifact_path(name: &str) -> Result<Option<std::path::PathBuf>, super::nsfw::ModelError> {
    match beside_the_binary(name) {
        Ok(path) => Ok(path),
        Err(error) => {
            log::error!("intent: could not locate {name}: {error}");
            Err(error)
        }
    }
}

fn validate_declared_outputs(
    pool: &super::nsfw::SessionPool,
    frame_count: usize,
) -> Result<(), super::nsfw::ModelError> {
    pool.with(|session| {
        let session = session
            .lock()
            .map_err(|_| super::nsfw::ModelError::Poisoned("intent metadata"))?;
        let [score, frames] = session.outputs() else {
            return Err(super::nsfw::ModelError::contract(format!(
                "intent graph declares {} outputs, expected 2",
                session.outputs().len()
            )));
        };
        for (name, output, width) in [
            ("score", score, 1_i64),
            ("frames", frames, frame_count as i64),
        ] {
            let ort::value::ValueType::Tensor { ty, shape, .. } = output.dtype() else {
                return Err(super::nsfw::ModelError::contract(format!(
                    "intent {name} output is not a tensor"
                )));
            };
            if *ty != ort::value::TensorElementType::Float32
                || shape.len() != 2
                || !matches!(shape[0], -1 | 1)
                || shape[1] != width
            {
                return Err(super::nsfw::ModelError::contract(format!(
                    "intent {name} output has type {ty:?} and shape {shape:?}; expected f32 [-1|1, {width}]"
                )));
            }
        }
        Ok(())
    })
}

fn model() -> Result<Option<&'static super::nsfw::SessionPool>, super::nsfw::ModelError> {
    static CELL: std::sync::OnceLock<
        Result<Option<super::nsfw::SessionPool>, super::nsfw::ModelError>,
    > = std::sync::OnceLock::new();
    match CELL.get_or_init(|| {
        let Some(path) = artifact_path(MODEL_FILE)? else {
            eprintln!(
                "intent: {MODEL_FILE} is not beside the binary, so «قفل خرید و فروش» is inert"
            );
            return Ok(None);
        };
        let pool =
            super::nsfw::open_path_pool(&path, "intent model", super::nsfw::infer_sessions())
                .ok_or_else(|| {
                    super::nsfw::ModelError::Runtime("intent model would not load".to_owned())
                })?;
        let frame_count = frames()?
            .ok_or_else(|| {
                super::nsfw::ModelError::artifact(format!(
                    "{FRAMES_FILE} is required by {MODEL_FILE}"
                ))
            })?
            .len();
        if let Err(error) = validate_declared_outputs(&pool, frame_count) {
            log::error!("intent: {MODEL_FILE} is incompatible: {error}");
            return Err(error);
        }
        Ok(Some(pool))
    }) {
        Ok(Some(model)) => Ok(Some(model)),
        Ok(None) => Ok(None),
        Err(error) => Err(error.clone()),
    }
}

fn big_model() -> Result<Option<&'static super::nsfw::SessionPool>, super::nsfw::ModelError> {
    static CELL: std::sync::OnceLock<
        Result<Option<super::nsfw::SessionPool>, super::nsfw::ModelError>,
    > = std::sync::OnceLock::new();
    match CELL.get_or_init(|| {
        let Some(path) = artifact_path(BIG_MODEL_FILE)? else {
            eprintln!("intent: {BIG_MODEL_FILE} is not beside the binary, so undecided scores are not escalated");
            return Ok(None);
        };
        let pool = super::nsfw::open_path_pool(&path, "intent escalation model", 1)
            .ok_or_else(|| {
                super::nsfw::ModelError::Runtime(
                    "intent escalation model would not load".to_owned(),
                )
            })?;
        let frame_count = frames()?
            .ok_or_else(|| {
                super::nsfw::ModelError::artifact(format!(
                    "{FRAMES_FILE} is required by {BIG_MODEL_FILE}"
                ))
            })?
            .len();
        if let Err(error) = validate_declared_outputs(&pool, frame_count) {
            log::error!("intent: {BIG_MODEL_FILE} is incompatible: {error}");
            return Err(error);
        }
        Ok(Some(pool))
    }) {
        Ok(Some(model)) => Ok(Some(model)),
        Ok(None) => Ok(None),
        Err(error) => Err(error.clone()),
    }
}

pub fn available() -> bool {
    matches!(vocab(), Ok(Some(_)))
        && matches!(frames(), Ok(Some(_)))
        && matches!(model(), Ok(Some(_)))
}

pub fn big_available() -> bool {
    matches!(vocab(), Ok(Some(_)))
        && matches!(frames(), Ok(Some(_)))
        && matches!(big_model(), Ok(Some(_)))
}

fn parse_frames(text: &str) -> Result<Vec<Box<str>>, super::nsfw::ModelError> {
    let mut frames: Vec<Box<str>> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let name = line.trim_end_matches('\r');
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(super::nsfw::ModelError::artifact(format!(
                "{FRAMES_FILE} line {} is empty or contains whitespace",
                index + 1
            )));
        }
        if frames.iter().any(|seen| seen.as_ref() == name) {
            return Err(super::nsfw::ModelError::artifact(format!(
                "{FRAMES_FILE} repeats frame {name:?}"
            )));
        }
        frames.push(Box::from(name));
        if frames.len() > usize::from(u8::MAX) + 1 {
            return Err(super::nsfw::ModelError::artifact(format!(
                "{FRAMES_FILE} has more frames than the stored u8 frame id can represent"
            )));
        }
    }
    if frames.is_empty() {
        return Err(super::nsfw::ModelError::artifact(format!(
            "{FRAMES_FILE} contains no frame names"
        )));
    }
    Ok(frames)
}

fn frames() -> Result<Option<&'static [Box<str>]>, super::nsfw::ModelError> {
    static CELL: std::sync::OnceLock<Result<Option<Vec<Box<str>>>, super::nsfw::ModelError>> =
        std::sync::OnceLock::new();
    match CELL.get_or_init(|| {
        let Some(path) = artifact_path(FRAMES_FILE)? else {
            eprintln!(
                "intent: {FRAMES_FILE} is not beside the binary, so the intent models are inert"
            );
            return Ok(None);
        };
        let loaded = std::fs::read_to_string(&path)
            .map_err(|error| {
                super::nsfw::ModelError::artifact(format!(
                    "could not read {}: {error}",
                    path.display()
                ))
            })
            .and_then(|text| parse_frames(&text));
        match loaded {
            Ok(frames) => Ok(Some(frames)),
            Err(error) => {
                log::error!("intent: frame metadata failed to load: {error}");
                Err(error)
            }
        }
    }) {
        Ok(Some(frames)) => Ok(Some(frames)),
        Ok(None) => Ok(None),
        Err(error) => Err(error.clone()),
    }
}

pub fn frame_name(frame: u8) -> &'static str {
    frames()
        .ok()
        .flatten()
        .and_then(|frames| frames.get(usize::from(frame)))
        .map_or("?", |name| name)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scored {
    pub margin: f32,
    pub frame: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbstainReason {
    Empty,
    TooLong,
    TokenWindow,
    UnknownShare,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScoreOutcome {
    Unavailable,
    Abstained(AbstainReason),
    Scored(Scored),
}


pub struct Unigram {
    ids: HashMap<Box<str>, u32>,
    scores: Vec<f32>,
    longest: usize,
    floor: f32,
}

fn read_vocab(text: &str) -> Option<Unigram> {
    let mut ids = HashMap::with_capacity(text.len() / 12);
    let mut scores = Vec::with_capacity(text.len() / 12);
    let mut longest = 1;
    for (at, line) in text.lines().enumerate() {
        let (piece, score) = line.trim_end_matches('\r').split_once(' ')?;
        let piece = super::imgtext::unescape(piece);
        let id = u32::try_from(at).ok()?;
        let score = score.parse::<f32>().ok()?;
        if piece.is_empty() || !score.is_finite() {
            return None;
        }
        longest = longest.max(piece.chars().count());
        if ids.insert(piece.into_boxed_str(), id).is_some() {
            return None;
        }
        scores.push(score);
    }
    for id in [BOS, PAD, EOS, UNK] {
        scores.get(id as usize)?;
    }
    let floor = scores.iter().copied().fold(f32::MAX, f32::min) - UNK_PENALTY;
    Some(Unigram {
        ids,
        scores,
        longest,
        floor,
    })
}

fn vocab() -> Result<Option<&'static Unigram>, super::nsfw::ModelError> {
    static CELL: std::sync::OnceLock<Result<Option<Unigram>, super::nsfw::ModelError>> =
        std::sync::OnceLock::new();
    match CELL.get_or_init(|| {
        let Some(path) = artifact_path(VOCAB_FILE)? else {
            eprintln!(
                "intent: {VOCAB_FILE} is not beside the binary, so the intent models are inert"
            );
            return Ok(None);
        };
        let loaded = std::fs::read_to_string(&path)
            .map_err(|error| {
                super::nsfw::ModelError::artifact(format!(
                    "could not read {}: {error}",
                    path.display()
                ))
            })
            .and_then(|text| {
                read_vocab(&text).ok_or_else(|| {
                    super::nsfw::ModelError::artifact(format!(
                        "{} is not a valid intent vocabulary",
                        path.display()
                    ))
                })
            });
        match loaded {
            Ok(vocab) => {
                println!("intent: {} vocabulary pieces", vocab.scores.len());
                Ok(Some(vocab))
            }
            Err(error) => {
                log::error!("intent: vocabulary failed to load: {error}");
                Err(error)
            }
        }
    }) {
        Ok(Some(vocab)) => Ok(Some(vocab)),
        Ok(None) => Ok(None),
        Err(error) => Err(error.clone()),
    }
}

fn normalize(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    text.nfkc()
        .filter(|c| !TO_NOTHING.contains(c))
        .map(|c| if TO_SPACE.contains(&c) { ' ' } else { c })
        .collect()
}

impl Unigram {
    fn word(&self, word: &str) -> Vec<u32> {
        let bounds: Vec<usize> = word
            .char_indices()
            .map(|(at, _)| at)
            .chain(std::iter::once(word.len()))
            .collect();
        let n = bounds.len() - 1;
        let mut best = vec![f32::NEG_INFINITY; n + 1];
        best[0] = 0.0;
        let mut back = vec![(0usize, UNK); n + 1];
        for end in 1..=n {
            for start in end.saturating_sub(self.longest)..end {
                if best[start] == f32::NEG_INFINITY {
                    continue;
                }
                let Some(&id) = self.ids.get(&word[bounds[start]..bounds[end]]) else {
                    continue;
                };
                let score = best[start] + self.scores[id as usize];
                if score > best[end] {
                    best[end] = score;
                    back[end] = (start, id);
                }
            }
            let fallback = best[end - 1] + self.floor;
            if best[end - 1] != f32::NEG_INFINITY && fallback > best[end] {
                best[end] = fallback;
                back[end] = (end - 1, UNK);
            }
        }
        let mut out = Vec::new();
        let mut end = n;
        while end > 0 {
            let (start, id) = back[end];
            out.push(id);
            end = start;
        }
        out.reverse();
        out.dedup_by(|a, b| *a == UNK && *b == UNK);
        out
    }

    fn body(&self, text: &str) -> (Vec<u32>, usize) {
        let mut out = Vec::new();
        let mut unknown = 0;
        let mut buffer = String::new();
        for word in normalize(text).split_whitespace() {
            buffer.clear();
            buffer.push(SPACE_MARK);
            buffer.push_str(word);
            let ids = self.word(&buffer);
            unknown += ids.iter().filter(|id| **id == UNK).count();
            out.extend(ids);
        }
        (out, unknown)
    }

    pub fn encode(&self, text: &str) -> Result<(Vec<i64>, Vec<i64>, f32), AbstainReason> {
        let (ids, unknown) = self.body(text);
        if ids.is_empty() {
            return Err(AbstainReason::Empty);
        }
        if ids.len() > TOKENS - 2 {
            return Err(AbstainReason::TokenWindow);
        }
        let share = unknown as f32 / ids.len() as f32;
        let mut framed = Vec::with_capacity(TOKENS);
        framed.push(i64::from(BOS));
        framed.extend(ids.into_iter().map(i64::from));
        framed.push(i64::from(EOS));
        let mut mask = vec![1i64; framed.len()];
        framed.resize(TOKENS, i64::from(PAD));
        mask.resize(TOKENS, 0);
        Ok((framed, mask, share))
    }
}


#[path = "intent_normalize.rs"]
mod normalization;
pub use normalization::descramble;

fn canonical(text: &str) -> String {
    descramble(text.trim()).0
}

pub fn text_key(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in b"trade-normalization-v2\0"
        .iter()
        .copied()
        .chain(canonical(text).bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn run(
    pool: &super::nsfw::SessionPool,
    ids: Vec<i64>,
    mask: Vec<i64>,
    frame_count: usize,
) -> Result<Scored, super::nsfw::ModelError> {
    let shape = vec![1i64, TOKENS as i64];
    let ids = ort::value::Value::from_array((shape.clone(), ids))
        .map_err(|error| super::nsfw::ModelError::contract(format!("intent ids input: {error}")))?;
    let mask = ort::value::Value::from_array((shape, mask)).map_err(|error| {
        super::nsfw::ModelError::contract(format!("intent mask input: {error}"))
    })?;
    pool.with(|session| {
        let mut session = session
            .lock()
            .map_err(|_| super::nsfw::ModelError::Poisoned("intent classifier"))?;
        let output = session.run(ort::inputs![ids, mask]).map_err(|error| {
            super::nsfw::ModelError::Runtime(format!("intent forward pass: {error}"))
        })?;
        let mut outputs = output.into_iter();
        let (_, score) = outputs
            .next()
            .ok_or_else(|| super::nsfw::ModelError::contract("intent graph has no score output"))?;
        let (_, logits) = outputs
            .next()
            .ok_or_else(|| super::nsfw::ModelError::contract("intent graph has no frame output"))?;
        if outputs.next().is_some() {
            return Err(super::nsfw::ModelError::contract(
                "intent graph exposes more than two outputs",
            ));
        }
        let (score_shape, score) = score.try_extract_tensor::<f32>().map_err(|error| {
            super::nsfw::ModelError::contract(format!("intent score output is not f32: {error}"))
        })?;
        let (frame_shape, logits) = logits.try_extract_tensor::<f32>().map_err(|error| {
            super::nsfw::ModelError::contract(format!("intent frame output is not f32: {error}"))
        })?;
        validate_outputs(score_shape, score, frame_shape, logits, frame_count)
    })
}

fn validate_outputs(
    score_shape: &[i64],
    score: &[f32],
    frame_shape: &[i64],
    logits: &[f32],
    frame_count: usize,
) -> Result<Scored, super::nsfw::ModelError> {
    if score_shape != [1, 1] || score.len() != 1 {
        return Err(super::nsfw::ModelError::contract(format!(
            "intent score shape is {score_shape:?}, expected [1, 1]"
        )));
    }
    if frame_shape != [1, frame_count as i64] || logits.len() != frame_count {
        return Err(super::nsfw::ModelError::contract(format!(
            "intent frame shape is {frame_shape:?} with {} values, expected [1, {frame_count}]",
            logits.len()
        )));
    }
    let margin = score[0];
    if !margin.is_finite() || logits.iter().any(|value| !value.is_finite()) {
        return Err(super::nsfw::ModelError::contract(
            "intent output contains a non-finite value",
        ));
    }
    let frame = logits
        .iter()
        .enumerate()
        .fold(
            None,
            |best: Option<(usize, f32)>, (at, &value)| match best {
                Some((_, current)) if current >= value => best,
                _ => Some((at, value)),
            },
        )
        .ok_or_else(|| super::nsfw::ModelError::contract("intent has no frame logits"))?
        .0;
    let frame = u8::try_from(frame).map_err(|_| {
        super::nsfw::ModelError::contract("intent frame index does not fit its stored type")
    })?;
    Ok(Scored { margin, frame })
}

fn score_with(
    pool: &super::nsfw::SessionPool,
    text: &str,
) -> Result<ScoreOutcome, super::nsfw::ModelError> {
    let Some(vocab) = vocab()? else {
        return Ok(ScoreOutcome::Unavailable);
    };
    let Some(frames) = frames()? else {
        return Ok(ScoreOutcome::Unavailable);
    };
    let clean = canonical(text);
    if clean.chars().count() > MAX_CHARS {
        return Ok(ScoreOutcome::Abstained(AbstainReason::TooLong));
    }
    let input = format!("{PREFIX}{clean}");
    let (ids, mask, unknown) = match vocab.encode(&input) {
        Ok(encoded) => encoded,
        Err(reason) => return Ok(ScoreOutcome::Abstained(reason)),
    };
    if unknown > UNK_LIMIT {
        return Ok(ScoreOutcome::Abstained(AbstainReason::UnknownShare));
    }
    run(pool, ids, mask, frames.len()).map(ScoreOutcome::Scored)
}

pub fn score(text: &str) -> Result<ScoreOutcome, super::nsfw::ModelError> {
    let Some(model) = model()? else {
        return Ok(ScoreOutcome::Unavailable);
    };
    score_with(model, text)
}

pub fn score_big(text: &str) -> Result<ScoreOutcome, super::nsfw::ModelError> {
    let Some(model) = big_model()? else {
        return Ok(ScoreOutcome::Unavailable);
    };
    score_with(model, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Unigram {
        let lines = [
            ("<s>", 0.0),
            ("<pad>", 0.0),
            ("</s>", 0.0),
            ("<unk>", 0.0),
            ("\u{2581}", -4.0),
            ("a", -3.0),
            ("b", -3.0),
            ("ab", -5.0),
            ("\u{2581}a", -2.0),
            ("\u{2581}ab", -8.0),
            ("c\\sd", -6.0),
        ];
        let text: String = lines
            .iter()
            .map(|(piece, score)| format!("{piece} {score}\n"))
            .collect();
        read_vocab(&text).expect("the tiny vocabulary is complete")
    }

    #[test]
    fn viterbi_picks_the_higher_scoring_split() {
        let bpe = tiny();
        assert_eq!(
            bpe.word("\u{2581}ab"),
            vec![8, 6],
            "the whole-word piece outbid a cheaper split"
        );
    }

    #[test]
    fn unknown_characters_fuse_into_one_token() {
        let bpe = tiny();
        assert_eq!(bpe.word("\u{2581}xy"), vec![4, UNK]);
        assert_eq!(bpe.word("\u{2581}axa"), vec![8, UNK, 5]);
    }

    #[test]
    fn every_word_gets_the_space_mark() {
        let bpe = tiny();
        let (ids, _) = bpe.body("a a");
        assert_eq!(ids, vec![8, 8]);
    }

    #[test]
    fn an_encoding_is_framed_padded_and_masked() {
        let bpe = tiny();
        let (ids, mask, share) = bpe.encode("a").expect("one word encodes");
        assert_eq!(ids.len(), TOKENS);
        assert_eq!(mask.len(), TOKENS);
        assert_eq!(&ids[..3], &[i64::from(BOS), 8, i64::from(EOS)]);
        assert!(ids[3..].iter().all(|id| *id == i64::from(PAD)));
        assert_eq!(&mask[..3], &[1, 1, 1]);
        assert!(mask[3..].iter().all(|m| *m == 0));
        assert_eq!(share, 0.0);

        let long = "a ".repeat(400);
        assert!(
            matches!(bpe.encode(&long), Err(AbstainReason::TokenWindow)),
            "a clipped message must abstain"
        );
        let exact = "a ".repeat(TOKENS - 2);
        let (ids, mask, _) = bpe.encode(&exact).expect("a complete message fits exactly");
        assert_eq!(ids.len(), TOKENS);
        assert_eq!(
            ids[TOKENS - 1],
            i64::from(EOS),
            "the closer fits without truncation"
        );
        assert!(mask.iter().all(|m| *m == 1));

        assert!(matches!(bpe.encode("   "), Err(AbstainReason::Empty)));
    }

    #[test]
    fn the_unknown_share_is_counted() {
        let bpe = tiny();
        let (_, _, share) = bpe.encode("a xyzq").expect("encodes with unknowns");
        assert!((share - 1.0 / 3.0).abs() < 1e-6, "share was {share}");
    }

    #[test]
    fn normalisation_matches_the_measured_charsmap() {
        assert_eq!(normalize("\u{645}\u{6cc}\u{200c}\u{641}"), "می ف");
        assert_eq!(normalize("\u{feb3}"), "\u{633}");
        assert_eq!(normalize("\u{2026}"), "...");
        assert_eq!(normalize("a\tb\nc"), "a b c");
        assert_eq!(normalize("a\u{b}b\u{7f}c"), "abc");
        assert_eq!(normalize("\u{6f1}\u{6f2}"), "\u{6f1}\u{6f2}");
    }

    #[test]
    fn escaped_pieces_round_trip_through_the_vocabulary() {
        let bpe = tiny();
        assert_eq!(bpe.ids.get("c d").copied(), Some(10));
    }

    #[test]
    fn corrupt_vocabulary_scores_and_duplicate_pieces_are_rejected() {
        assert!(read_vocab("<s> 0\n<pad> 0\n</s> 0\n<unk> NaN\n").is_none());
        assert!(read_vocab("<s> 0\n<pad> 0\n</s> 0\n<unk> 0\n<s> -1\n").is_none());
    }

    #[test]
    fn frame_metadata_is_required_and_canonical() {
        assert!(parse_frames("").is_err());
        assert!(parse_frames("offer\n\nquote\n").is_err());
        assert!(parse_frames("offer\noffer\n").is_err());
        assert!(parse_frames("two words\n").is_err());
        assert_eq!(
            parse_frames("offer\nquote\n").expect("valid frames"),
            vec![Box::<str>::from("offer"), Box::<str>::from("quote")]
        );
    }

    #[test]
    fn classifier_outputs_must_match_the_frame_artifact() {
        let scored = validate_outputs(&[1, 1], &[0.5], &[1, 2], &[0.1, 0.9], 2)
            .expect("matching finite outputs");
        assert_eq!(scored.frame, 1);
        assert!(validate_outputs(&[1], &[0.5], &[1, 2], &[0.1, 0.9], 2).is_err());
        assert!(validate_outputs(&[1, 1], &[0.5], &[1, 3], &[0.1, 0.9, 0.0], 2).is_err());
        assert!(validate_outputs(&[1, 1], &[f32::NAN], &[1, 2], &[0.1, 0.9], 2).is_err());
        assert!(validate_outputs(&[1, 1], &[0.5], &[1, 2], &[0.1, f32::INFINITY], 2).is_err());
    }

    #[test]
    fn the_text_key_is_stable_and_includes_the_entire_message() {
        assert_eq!(text_key("سلام"), text_key("  سلام  "));
        let long = "xy".repeat(MAX_CHARS);
        let longer = format!("{long}zx");
        assert_ne!(text_key(&long), text_key(&longer));
        assert_ne!(text_key("a"), text_key("b"));
        assert_eq!(text_key("سکه م.یو"), text_key("سکه میو"));
    }

    #[test]
    fn descrambling_undoes_the_tricks_and_flags_them() {
        let clean = |t: &str| descramble(t).0;
        let tampered = |t: &str| descramble(t).1;

        assert_eq!(clean("م.یو خربدارم"), "میو خریدارم");
        assert!(tampered("م.یو خربدارم"));
        assert_eq!(clean("می..فروشم تتر"), "میفروشم تتر");
        assert_eq!(clean("فـروشی ماشین"), "فروشی ماشین");
        assert!(tampered("فـروشی ماشین"), "tatweel is tampering");
        assert_eq!(clean("ف ر و ش ی گوشی"), "فروشی گوشی");
        assert!(tampered("ف ر و ش ی گوشی"));
        assert_eq!(clean("فرووووشی"), "فروشی");
        assert!(!tampered("سلامممم چطوری"));
        assert_eq!(clean("سلامممم چطوری"), "سلام چطوری");
        assert_eq!(clean("t.me/shop"), "t.me/shop");
        assert!(!tampered("t.me/shop"));
        assert_eq!(clean("قیمتش ۲.۵ میلیونه"), "قیمتش ۲.۵ میلیونه");
        assert_eq!(clean("سلام،خوبی؟"), "سلام،خوبی؟");
        assert!(
            !tampered("یه دو تا چیز میخوام"),
            "short words are not spaced spelling"
        );
    }

    #[test]
    #[ignore = "needs intent_vocab.txt from an export"]
    fn agrees_with_the_reference_tokenizer() {
        let dir = std::path::PathBuf::from(
            std::env::var("VISION_FILES").unwrap_or_else(|_| "target/release".to_owned()),
        );
        let text = std::fs::read_to_string(dir.join(VOCAB_FILE)).expect("the exported vocabulary");
        let vocab = read_vocab(&text).expect("the vocabulary parses");
        for (case, want) in PINNED {
            let (ids, _, _) = vocab.encode(case).expect("every pinned case encodes");
            assert_eq!(&ids[..want.len()], *want, "«{case}»");
            assert!(
                ids[want.len()..].iter().all(|id| *id == i64::from(PAD)),
                "«{case}» padding"
            );
        }
    }

    const PINNED: &[(&str, &[i64])] = &[
        (
            "query: گوشی فروشی، پیام بدید",
            &[0, 37, 966, 12, 22497, 9078, 121, 44, 18632, 5421, 1955, 2],
        ),
        ("سلام دنیا", &[0, 9252, 5350, 2]),
        (
            "میفروشم گوشی ۲۰۰ تومن",
            &[0, 320, 63559, 314, 22497, 65534, 929, 3644, 2],
        ),
        ("می\u{200c}فروشم", &[0, 320, 9078, 314, 2]),
        (
            "کارت 6037-9918-1234-5670",
            &[
                0, 28633, 1095, 6671, 9, 3215, 1302, 9, 102641, 6320, 61069, 2,
            ],
        ),
        ("قیمت چنده؟", &[0, 5897, 4391, 156, 932, 2]),
        (
            "for sale, only $25!",
            &[0, 88, 6969, 4, 3025, 2386, 1756, 34, 2],
        ),
        ("\u{feb3}\u{fef4}\u{feb3}", &[0, 6, 87299, 2]),
        ("Hello World!", &[0, 20470, 4180, 34, 2]),
        ("😀 چه روز خوبی", &[0, 12409, 3969, 2207, 20397, 2]),
        ("中文测试", &[0, 6, 3, 2]),
    ];

    #[test]
    #[ignore = "needs intent.onnx, intent_vocab.txt and intent_frames.txt from an export"]
    fn a_selling_message_scores_above_ordinary_chat() {
        let scored = |text| match score(text).expect("inference succeeds") {
            ScoreOutcome::Scored(scored) => scored,
            other => panic!("expected a score, got {other:?}"),
        };
        let selling = scored("گوشی فروشی، ۲۰ میلیون، پیام بدید");
        let chat = scored("سلام بچه ها، خوبید؟");
        assert!(
            selling.margin > 0.0 && chat.margin < selling.margin,
            "selling scored {:.4}, ordinary chat {:.4}",
            selling.margin,
            chat.margin
        );
        assert_eq!(
            frame_name(selling.frame),
            "offer",
            "the frame the ad was read as"
        );
        assert_ne!(
            frame_name(chat.frame),
            "?",
            "the frame file is beside the binary"
        );
    }

    #[test]
    #[ignore = "needs shipped intent vocabulary and model"]
    fn incomplete_model_inputs_abstain() {
        assert!(available(), "the test must have a real model");
        let long = format!("اکانت فروشی {} این فقط نقل قول بود", "سلام ".repeat(130));
        assert!(matches!(
            score(&long),
            Ok(ScoreOutcome::Abstained(AbstainReason::TooLong))
        ));
        let tokens = "xy ".repeat(150);
        assert!(matches!(
            score(&tokens),
            Ok(ScoreOutcome::Abstained(
                AbstainReason::TokenWindow | AbstainReason::UnknownShare
            ))
        ));
    }
}
