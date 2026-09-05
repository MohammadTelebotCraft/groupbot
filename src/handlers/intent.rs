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

fn model() -> Option<&'static super::nsfw::SessionPool> {
    static CELL: std::sync::OnceLock<Option<super::nsfw::SessionPool>> =
        std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        let Some(path) = beside_the_binary(MODEL_FILE) else {
            eprintln!("intent: {MODEL_FILE} is not beside the binary, so «قفل خرید و فروش» is inert");
            return None;
        };
        super::nsfw::open_path_pool(&path, "intent model", super::nsfw::infer_sessions())
    })
    .as_ref()
}

fn big_model() -> Option<&'static super::nsfw::SessionPool> {
    static CELL: std::sync::OnceLock<Option<super::nsfw::SessionPool>> =
        std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        let Some(path) = beside_the_binary(BIG_MODEL_FILE) else {
            eprintln!("intent: {BIG_MODEL_FILE} is not beside the binary, so undecided scores are not escalated");
            return None;
        };
        super::nsfw::open_path_pool(&path, "intent escalation model", 1)
    })
    .as_ref()
}

pub fn available() -> bool {
    vocab().is_some() && model().is_some()
}

pub fn big_available() -> bool {
    vocab().is_some() && big_model().is_some()
}

fn frames() -> &'static [Box<str>] {
    static CELL: std::sync::OnceLock<Vec<Box<str>>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        beside_the_binary(FRAMES_FILE)
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| text.split_whitespace().map(Box::from).collect())
            .unwrap_or_default()
    })
}

pub fn frame_name(frame: u8) -> &'static str {
    frames().get(usize::from(frame)).map_or("?", |name| name)
}

#[derive(Clone, Copy, Debug)]
pub struct Scored {
    pub margin: f32,
    pub frame: u8,
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
        longest = longest.max(piece.chars().count());
        ids.insert(piece.into_boxed_str(), at as u32);
        scores.push(score.parse::<f32>().ok()?);
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

fn vocab() -> Option<&'static Unigram> {
    static CELL: std::sync::OnceLock<Option<Unigram>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        let text = std::fs::read_to_string(beside_the_binary(VOCAB_FILE)?).ok()?;
        let vocab = read_vocab(&text)?;
        println!("intent: {} vocabulary pieces", vocab.scores.len());
        Some(vocab)
    })
    .as_ref()
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

    pub fn encode(&self, text: &str) -> Option<(Vec<i64>, Vec<i64>, f32)> {
        let (mut ids, unknown) = self.body(text);
        if ids.is_empty() {
            return None;
        }
        let share = unknown as f32 / ids.len() as f32;
        ids.truncate(TOKENS - 2);
        let mut framed = Vec::with_capacity(TOKENS);
        framed.push(i64::from(BOS));
        framed.extend(ids.into_iter().map(i64::from));
        framed.push(i64::from(EOS));
        let mut mask = vec![1i64; framed.len()];
        framed.resize(TOKENS, i64::from(PAD));
        mask.resize(TOKENS, 0);
        Some((framed, mask, share))
    }
}

fn arabic(c: char) -> bool {
    matches!(c,
        '\u{600}'..='\u{6ff}'
            | '\u{750}'..='\u{77f}'
            | '\u{8a0}'..='\u{8ff}'
            | '\u{fb50}'..='\u{fdff}'
            | '\u{fe70}'..='\u{feff}')
        && !matches!(c, '\u{660}'..='\u{669}' | '\u{6f0}'..='\u{6f9}')
}

pub fn descramble(text: &str) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut tampered = false;

    let separator = |c: char| ".·-_*`'٬".contains(c);
    for (at, &c) in chars.iter().enumerate() {
        let c = match c {
            '\u{64a}' | '\u{649}' => 'ی',
            '\u{643}' => 'ک',
            other => other,
        };
        if c == '\u{640}' {
            tampered = true;
            continue;
        }
        if separator(c)
            && at > 0
            && chars[..at].iter().rev().find(|c| !separator(**c)).copied().is_some_and(arabic)
            && chars[at + 1..].iter().find(|c| !separator(**c)).copied().is_some_and(arabic)
        {
            tampered = true;
            continue;
        }
        out.push(c);
    }

    let runs: Vec<char> = out.chars().collect();
    let mut squeezed = String::with_capacity(out.len());
    let mut at = 0;
    while at < runs.len() {
        let mut end = at;
        while end < runs.len() && runs[end] == runs[at] {
            end += 1;
        }
        for _ in 0..if end - at >= 3 { 1 } else { end - at } {
            squeezed.push(runs[at]);
        }
        at = end;
    }
    let out = squeezed;

    let words: Vec<&str> = out.split(' ').collect();
    let single = |word: &str| word.chars().count() == 1 && word.chars().all(arabic);
    if words.iter().filter(|word| single(word)).count() >= 3 {
        let mut joined: Vec<String> = Vec::with_capacity(words.len());
        let mut run: Vec<&str> = Vec::new();
        for word in words.iter().chain(std::iter::once(&"")) {
            if single(word) {
                run.push(word);
                continue;
            }
            if run.len() >= 3 {
                tampered = true;
                joined.push(run.concat());
            } else {
                joined.extend(run.iter().map(|w| (*w).to_owned()));
            }
            run.clear();
            if !word.is_empty() {
                joined.push((*word).to_owned());
            }
        }
        return (joined.join(" "), tampered);
    }
    (out, tampered)
}

fn canonical(text: &str) -> String {
    let (clean, _) = descramble(text.trim());
    match clean.char_indices().nth(MAX_CHARS) {
        Some((at, _)) => clean[..at].to_owned(),
        None => clean,
    }
}

pub fn text_key(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical(text).bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn run(pool: &super::nsfw::SessionPool, ids: Vec<i64>, mask: Vec<i64>) -> Option<Scored> {
    let shape = vec![1i64, TOKENS as i64];
    let ids = ort::value::Value::from_array((shape.clone(), ids)).ok()?;
    let mask = ort::value::Value::from_array((shape, mask)).ok()?;
    pool.with(|session| {
        let mut session = session.lock().ok()?;
        let output = session.run(ort::inputs![ids, mask]).ok()?;

        let mut outputs = output.into_iter();
        let (_, score) = outputs.next()?;
        let (_, logits) = outputs.next()?;
        let (_, score) = score.try_extract_tensor::<f32>().ok()?;
        let (_, logits) = logits.try_extract_tensor::<f32>().ok()?;
        let margin = *score.first()?;
        let frame = logits
            .iter()
            .enumerate()
            .fold(None, |best: Option<(usize, f32)>, (at, &v)| match best {
                Some((_, b)) if b >= v => best,
                _ => Some((at, v)),
            })?
            .0;
        margin.is_finite().then_some(Scored {
            margin,
            frame: u8::try_from(frame).unwrap_or(u8::MAX),
        })
    })
}

fn score_with(pool: &super::nsfw::SessionPool, text: &str) -> Option<Scored> {
    let vocab = vocab()?;
    let (ids, mask, unknown) = vocab.encode(&format!("{PREFIX}{}", canonical(text)))?;
    if unknown > UNK_LIMIT {
        return None;
    }
    run(pool, ids, mask)
}

pub fn score(text: &str) -> Option<Scored> {
    score_with(model()?, text)
}

pub fn score_big(text: &str) -> Option<Scored> {
    score_with(big_model()?, text)
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
        let (ids, mask, _) = bpe.encode(&long).expect("a long message encodes");
        assert_eq!(ids.len(), TOKENS);
        assert_eq!(
            ids[TOKENS - 1],
            i64::from(EOS),
            "the closer survives truncation"
        );
        assert!(mask.iter().all(|m| *m == 1));

        assert!(bpe.encode("   ").is_none(), "nothing to encode");
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
    fn the_text_key_is_stable_and_truncates_where_the_model_does() {
        assert_eq!(text_key("سلام"), text_key("  سلام  "));

        let long = "xy".repeat(MAX_CHARS);
        let longer = format!("{long}zx");
        assert_eq!(text_key(&long), text_key(&longer));
        assert_ne!(text_key("a"), text_key("b"));

        assert_eq!(text_key("سکه م.یو"), text_key("سکه میو"));
    }

    #[test]
    fn descrambling_undoes_the_tricks_and_flags_them() {
        let clean = |t: &str| descramble(t).0;
        let tampered = |t: &str| descramble(t).1;

        assert_eq!(clean("م.یو خربدارم"), "میو خربدارم");
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
        assert!(!tampered("یه دو تا چیز میخوام"), "short words are not spaced spelling");
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
            &[0, 28633, 1095, 6671, 9, 3215, 1302, 9, 102641, 6320, 61069, 2],
        ),
        ("قیمت چنده؟", &[0, 5897, 4391, 156, 932, 2]),
        ("for sale, only $25!", &[0, 88, 6969, 4, 3025, 2386, 1756, 34, 2]),

        ("\u{feb3}\u{fef4}\u{feb3}", &[0, 6, 87299, 2]),
        ("Hello World!", &[0, 20470, 4180, 34, 2]),
        ("😀 چه روز خوبی", &[0, 12409, 3969, 2207, 20397, 2]),

        ("中文测试", &[0, 6, 3, 2]),
    ];

    #[test]
    #[ignore = "needs intent.onnx, intent_vocab.txt and intent_frames.txt from an export"]
    fn a_selling_message_scores_above_ordinary_chat() {
        let selling = score("گوشی فروشی، ۲۰ میلیون، پیام بدید").expect("the model beside the binary");
        let chat = score("سلام بچه ها، خوبید؟").expect("ordinary chat scores");
        assert!(
            selling.margin > 0.0 && chat.margin < selling.margin,
            "selling scored {:.4}, ordinary chat {:.4}",
            selling.margin,
            chat.margin
        );
        assert_eq!(frame_name(selling.frame), "offer", "the frame the ad was read as");
        assert_ne!(frame_name(chat.frame), "?", "the frame file is beside the binary");
    }
}
