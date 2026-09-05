use std::collections::HashMap;

use super::imgfilter::DIM;
use super::vision::{beside_the_binary, unit};

const MODEL_FILE: &str = "vision_text.onnx";
const VOCAB_FILE: &str = "vision_text_vocab.txt";
const MERGES_FILE: &str = "vision_text_merges.txt";

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

fn model() -> Option<&'static super::nsfw::Session> {
    static CELL: std::sync::OnceLock<Option<super::nsfw::Session>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        let Some(path) = beside_the_binary(MODEL_FILE) else {
            eprintln!("imgtext: {MODEL_FILE} is not beside the binary, so text filters are inert");
            return None;
        };
        super::nsfw::open_path(&path, "text tower")
    })
    .as_ref()
}

pub struct Bpe {
    ids: HashMap<String, u32>,

    merges: HashMap<(u32, u32), (u32, u32)>,

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
        ids.insert(unescape(line.trim_end_matches('\r')), at as u32);
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

fn read_merges(text: &str, ids: &HashMap<String, u32>) -> HashMap<(u32, u32), (u32, u32)> {
    let mut merges = HashMap::with_capacity(text.len() / 16);
    for (rank, line) in text.lines().enumerate() {
        let Some((left, right)) = line.trim_end_matches('\r').split_once(' ') else {
            continue;
        };
        let (left, right) = (unescape(left), unescape(right));
        let joined = format!("{left}{right}");

        let (Some(&left), Some(&right), Some(&merged)) =
            (ids.get(&left), ids.get(&right), ids.get(&joined))
        else {
            continue;
        };
        merges.entry((left, right)).or_insert((rank as u32, merged));
    }
    merges
}

fn load(vocab: &std::path::Path, merges: &std::path::Path) -> Option<Bpe> {
    let (ids, bytes, eos) = read_vocab(&std::fs::read_to_string(vocab).ok()?)?;
    let merges = read_merges(&std::fs::read_to_string(merges).ok()?, &ids);
    Some(Bpe {
        ids,
        merges,
        bytes,
        eos,
    })
}

fn bpe() -> Option<&'static Bpe> {
    static CELL: std::sync::OnceLock<Option<Bpe>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        let bpe = load(
            &beside_the_binary(VOCAB_FILE)?,
            &beside_the_binary(MERGES_FILE)?,
        )?;
        println!(
            "imgtext: {} vocabulary pieces, {} merges",
            bpe.ids.len(),
            bpe.merges.len()
        );
        Some(bpe)
    })
    .as_ref()
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

fn run(session: &super::nsfw::Session, ids: Vec<i64>) -> Option<Vec<f32>> {
    let value = ort::value::Value::from_array((vec![1i64, TOKENS as i64], ids)).ok()?;
    let mut session = session.lock().ok()?;
    let output = session.run(ort::inputs![value]).ok()?;
    let (_, values) = output[0].try_extract_tensor::<f32>().ok()?;
    (values.len() == DIM).then(|| values.to_vec())
}

pub fn embed(phrase: &str) -> Option<Vec<f32>> {
    let phrase = phrase.trim();
    if phrase.is_empty() {
        return None;
    }
    let (session, bpe) = (model()?, bpe()?);

    let mut sum = vec![0f32; DIM];
    let mut taken = 0usize;
    for template in TEMPLATES {
        let framed = template.replace("{}", phrase);
        let Some(out) = run(session, bpe.encode(&framed)) else {
            continue;
        };

        for (at, value) in unit(&out).into_iter().enumerate() {
            sum[at] += value;
        }
        taken += 1;
    }
    if taken == 0 {
        eprintln!("imgtext: the text tower produced nothing for «{phrase}»");
        return None;
    }
    Some(unit(&sum))
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let merges = read_merges("a b\nb c\nab c\n", &ids);
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

        bpe.merges = read_merges("b c\na b\nab c\n", &bpe.ids);
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
    fn a_persian_phrase_lands_next_to_its_own_concept() {
        let vector = embed("سیگار").expect("the text tower beside the test binary");
        assert_eq!(vector.len(), DIM);
        let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "not unit length: {norm}");

        use super::super::concept_vectors as vectors;
        use super::super::vision::dot;
        let cigarette = dot(&vector, &vectors::CIGARETTE);
        for (name, other) in [
            ("alcohol", &vectors::ALCOHOL),
            ("blood", &vectors::BLOOD),
            ("gambling", &vectors::GAMBLING),
        ] {
            let other = dot(&vector, other);
            assert!(
                cigarette > other,
                "«سیگار» is nearer {name} ({other:.4}) than cigarette ({cigarette:.4})"
            );
        }
    }

    #[test]
    fn every_template_carries_the_phrase() {
        for template in TEMPLATES {
            assert!(template.contains("{}"), "«{template}» drops the phrase");
            assert!(!template.contains('\u{200c}'));
            assert_ne!(template.replace("{}", "خودرو"), *template);
        }
    }
}
