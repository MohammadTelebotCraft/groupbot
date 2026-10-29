use std::collections::HashMap;
use std::sync::Arc;

use grammers_client::media::{Document, Media};
use grammers_client::message::Message;
use grammers_client::session::types::PeerRef;

use crate::response::ResponseKind;

use super::concept_vectors as vectors;
use super::imgsubject::Route;
use super::vision::{dot, unit};
use super::{Ctx, VisionSource, esc};

pub const PREFIX: &str = "imgf:";

pub const CAUSE: &str = "imgf";

pub use super::vision::DIM;

const MAX_NAME: usize = 32;

pub const MAX_FILTERS: usize = 8;

const MAX_SAMPLES: u32 = 8;

pub const RATE_RANGE: (u32, u32) = (100, 20_000);
pub const DEFAULT_RATE: u32 = 1_000;

pub const FIXED_MODEL_CUT: f32 = 0.038;

pub const FIXED_EXAMPLE_CUT: f32 = 0.62;

pub const MIN_SAMPLES: usize = 32;

pub struct Filter {
    pub name: String,
    pub vector: Vec<f32>,
    pub cut: f32,
    pub live: bool,
    pub print: u64,
    pub faces: Option<Vec<Vec<f32>>>,
    pub person: Option<&'static super::facelock::Person>,
    pub gate: Option<Gate>,
    pub subject: Option<&'static str>,
}

impl Filter {
    pub fn is_face(&self) -> bool {
        self.faces.is_some()
    }
}

pub const TEXT_ADD: &[&str] = &[
    "فیلتر متنی",
    "/imagefilter",
    "/phrasefilter",
];
pub const FACE_ADD: &[&str] = &[
    "قفل چهره",
    "فیلتر چهره",
    "/lockface",
    "/facelock",
];
pub const PHOTO_ADD: &[&str] = &[
    "قفل تصویر",
    "فیلتر تصویر",
    "فیلتر عکس",
    "/lockimage",
    "/imagelock",
];
pub const SAMPLE_ADD: &[&str] = &[
    "فیلتر این",
    "فیلتر نمونه",
    "/filterimage",
    "/samplefilter",
];
pub const REMOVE: &[&str] = &[
    "حذف فیلتر تصویری",
    "حذف فیلتر تصویر",
    "حذف فیلتر عکس",
    "/unfilterimage",
    "/delimagefilter",
    "/removeimagefilter",
];
pub const CALIBRATE: &[&str] = &[];

pub const TUNE: &[&str] = &[
    "فیلتر تصویری",
    "/imagefiltermode",
];
const LIVE_WORD: &[&str] = &["روشن", "فعال"];
const SHADOW_WORD: &[&str] = &["خاموش"];

pub fn key(name: &str) -> String {
    format!("{PREFIX}{name}")
}

pub fn names(ctx: &Ctx, chat: i64) -> Vec<String> {
    ctx.settings.flags_with_prefix(chat, PREFIX)
}

pub fn any(ctx: &Ctx, chat: i64) -> bool {
    !ctx.settings.indexed_empty(chat, PREFIX)
}

pub fn margin(embedding: &[f32], vector: &[f32]) -> f32 {
    dot(embedding, vector) - dot(embedding, &vectors::BACKGROUND)
}

pub fn quantize(v: &[f32]) -> (Vec<u8>, f32) {
    let max = v.iter().fold(0f32, |m, x| m.max(x.abs())).max(1e-6);
    let scale = max / 127.0;
    let bytes = v
        .iter()
        .map(|x| ((x / scale).round().clamp(-127.0, 127.0) as i8) as u8)
        .collect();
    (bytes, scale)
}

pub fn dequantize(bytes: &[u8], scale: f32) -> Vec<f32> {
    bytes.iter().map(|b| f32::from(*b as i8) * scale).collect()
}

pub fn fingerprint(bytes: &[u8], scale: f32) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes.iter().chain(&scale.to_le_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn z_for(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239e0,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838e0,
        -2.549_732_539_343_734e0,
        4.374_664_141_464_968e0,
        2.938_163_982_698_783e0,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996e0,
        3.754_408_661_907_416e0,
    ];
    const LOW: f64 = 0.024_25;

    let p = p.clamp(1e-12, 1.0 - 1e-12);
    if p < LOW {
        let q = (-2.0 * p.ln()).sqrt();
        return (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0);
    }
    if p > 1.0 - LOW {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        return -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0);
    }
    let q = p - 0.5;
    let r = q * q;
    (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
        / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
}

pub struct Calibration {
    pub cut: f32,
    pub samples: usize,
    pub mean: f32,
    pub sd: f32,
}

pub fn calibrate(vector: &[f32], samples: &[Box<[f32]>], rate: u32) -> Option<Calibration> {
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let rate = rate.clamp(RATE_RANGE.0, RATE_RANGE.1);
    let margins: Vec<f32> = samples.iter().map(|s| margin(s, vector)).collect();
    let count = margins.len() as f32;
    let mean = margins.iter().sum::<f32>() / count;
    let variance = margins.iter().map(|m| (m - mean) * (m - mean)).sum::<f32>() / count;
    let sd = variance.sqrt();
    let z = z_for(1.0 - 1.0 / f64::from(rate));

    let n = margins.len() as f64;
    let t = z + (z * z * z + z) / (4.0 * (n - 1.0).max(1.0));
    let widened = (t * (1.0 + 1.0 / n).sqrt()) as f32;

    Some(Calibration {
        cut: (mean + widened * sd).max(mean + 1e-3),
        samples: samples.len(),
        mean,
        sd,
    })
}

const BANK: &[u8] = include_bytes!("../../assets/imgfilter_bank.bin");

const BANK_RATE: f64 = 0.001;
const PHRASE_RATE: f64 = 0.0005;

pub fn rate_for(samples: u32) -> f64 {
    if samples == 0 { PHRASE_RATE } else { BANK_RATE }
}
const BANK_PAD: f32 = 0.002;

struct Bank {
    scales: Vec<f32>,
    rows: &'static [u8],
    width: usize,
    background: Vec<f32>,
}

fn bank() -> &'static Bank {
    static CELL: std::sync::OnceLock<Bank> = std::sync::OnceLock::new();
    CELL.get_or_init(|| Bank::load(BANK, b"IFB1", "imgfilter_bank.bin"))
}

const POOL: &[u8] = include_bytes!("../../assets/imgfilter_pool.bin");

fn pool() -> &'static Bank {
    static CELL: std::sync::OnceLock<Bank> = std::sync::OnceLock::new();
    CELL.get_or_init(|| Bank::load(POOL, b"IFP1", "imgfilter_pool.bin"))
}

const REGION: &[u8] = include_bytes!("../../assets/imgfilter_region.bin");

fn region() -> &'static Bank {
    static CELL: std::sync::OnceLock<Bank> = std::sync::OnceLock::new();
    CELL.get_or_init(|| Bank::load(REGION, b"IFR1", "imgfilter_region.bin"))
}

const FITTED: &[u8] = include_bytes!("../../assets/imgfilter_concepts.bin");

struct Fitted {
    cut: f32,
    scale: f32,
    at: usize,
}

fn fitted() -> &'static std::collections::HashMap<&'static str, Fitted> {
    static CELL: std::sync::OnceLock<std::collections::HashMap<&'static str, Fitted>> =
        std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        let word = |at: usize| u32::from_le_bytes(FITTED[at..at + 4].try_into().expect("header"));
        assert!(&FITTED[..4] == b"IFC1" && word(8) as usize == DIM, "imgfilter_concepts.bin is not this space");
        let mut out = std::collections::HashMap::new();
        let mut at = 12;
        for _ in 0..word(4) {
            let length = u16::from_le_bytes(FITTED[at..at + 2].try_into().expect("key length")) as usize;
            let key = std::str::from_utf8(&FITTED[at + 2..at + 2 + length]).expect("key");
            at += 2 + length;
            let cut = f32::from_le_bytes(FITTED[at..at + 4].try_into().expect("cut"));
            let scale = f32::from_le_bytes(FITTED[at + 4..at + 8].try_into().expect("scale"));
            out.insert(key, Fitted { cut, scale, at: at + 8 });
            at += 8 + DIM;
        }
        assert_eq!(at, FITTED.len(), "imgfilter_concepts.bin is truncated");
        out
    })
}

const VOCAB: &[u8] = include_bytes!("../../assets/imgfilter_vocab.bin");
const VOCAB_ROWS: &str = include_str!("../../assets/imgfilter_vocab.tsv");

fn vocab() -> &'static Bank {
    static CELL: std::sync::OnceLock<Bank> = std::sync::OnceLock::new();
    CELL.get_or_init(|| Bank::load(VOCAB, b"IFV1", "imgfilter_vocab.bin"))
}

fn vocab_row(english: &str) -> Option<u32> {
    static CELL: std::sync::OnceLock<HashMap<&'static str, u32>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        VOCAB_ROWS
            .lines()
            .filter_map(|line| {
                let (english, row) = line.split_once('\t')?;
                Some((english, row.parse().ok()?))
            })
            .collect()
    })
    .get(english)
    .copied()
}

const GATE_SAME: f32 = 0.8;
const GATE_FITTED: u32 = 25;
const GATE_LEARNED: u32 = 100;
const GATED_BELOW: f32 = 1e-3;

struct VocabShape {
    mean_dots: Vec<f32>,
    lengths: Vec<f32>,
    mean_square: f32,
}

fn vocab_shape() -> &'static VocabShape {
    static CELL: std::sync::OnceLock<VocabShape> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        let vocab = vocab();
        let count = vocab.scales.len();
        let mut mean = vec![0.0f32; DIM];
        for at in 0..count {
            for (total, x) in mean.iter_mut().zip(vocab.decode(at)) {
                *total += x / count as f32;
            }
        }
        let mean_dots = vocab.dots(&mean);
        let mean_square = dot(&mean, &mean);
        let lengths = (0..count)
            .map(|at| {
                let row = vocab.decode(at);
                (dot(&row, &row) - 2.0 * mean_dots[at] + mean_square).max(1e-12).sqrt()
            })
            .collect();
        VocabShape {
            mean_dots,
            lengths,
            mean_square,
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    row: u32,
    top: u32,
}

impl Gate {
    pub fn for_english(english: &str) -> Option<Gate> {
        let row = vocab_row(english)?;
        let top = if fitted().contains_key(english) {
            GATE_FITTED
        } else {
            GATE_LEARNED
        };
        Some(Gate { row, top })
    }

    pub fn names(&self, embedding: &[f32]) -> bool {
        let (vocab, shape) = (vocab(), vocab_shape());
        let row = self.row as usize;
        let own = vocab.decode(row);
        let mine = dot(embedding, &own);
        let own_length = shape.lengths[row];
        let raw = vocab.dots(&own);
        let mut above = 0;
        for (at, pictured) in vocab.dots(embedding).into_iter().enumerate() {
            if pictured <= mine {
                continue;
            }
            let same = (raw[at] - shape.mean_dots[at] - shape.mean_dots[row] + shape.mean_square)
                / (shape.lengths[at] * own_length);
            if same < GATE_SAME {
                above += 1;
                if above >= self.top {
                    return false;
                }
            }
        }
        true
    }
}

fn gated(margin: f32, cut: f32, gate: Option<Gate>, embedding: &[f32]) -> f32 {
    match gate {
        Some(gate) if margin >= cut && !gate.names(embedding) => cut - GATED_BELOW,
        _ => margin,
    }
}

pub fn is_trained(english: &str) -> bool {
    fitted().contains_key(english)
}

pub fn fitted_filter(english: &str) -> Option<Learned> {
    let entry = fitted().get(english)?;
    Some(Learned {
        vector: dequantize(&FITTED[entry.at..entry.at + DIM], entry.scale),
        cut: entry.cut,
    })
}

const UNDERSTOOD_TAIL: f32 = 5.2;
const UNDERSTOOD_REGIONAL: f32 = 0.5;

#[derive(Clone, Copy, Debug)]
pub struct Understanding {
    pub tail: f32,
    pub regional: f32,
}

impl Understanding {
    pub fn ok(&self) -> bool {
        self.tail >= UNDERSTOOD_TAIL && self.regional <= UNDERSTOOD_REGIONAL
    }
}

pub fn understanding(text: &[f32]) -> Understanding {
    let (pool, region) = (pool(), region());
    let direction = unit(text);
    let margins: Vec<f32> = pool
        .dots(&direction)
        .into_iter()
        .zip(&pool.background)
        .map(|(dot, background)| dot - background)
        .collect();
    let mut sorted = margins.clone();
    sorted.sort_unstable_by(|a, b| b.total_cmp(a));
    let count = sorted.len() as f32;
    let mean = margins.iter().sum::<f32>() / count;
    let sd = (margins.iter().map(|m| (m - mean) * (m - mean)).sum::<f32>() / count)
        .sqrt()
        .max(1e-6);
    let median = sorted[sorted.len() / 2];
    let top = sorted.iter().take(20).sum::<f32>() / 20.0;
    let regional: Vec<f32> = region
        .dots(&direction)
        .into_iter()
        .zip(&region.background)
        .map(|(dot, background)| dot - background)
        .collect();
    let mut both: Vec<(f32, bool)> = margins
        .iter()
        .map(|m| (*m, false))
        .chain(regional.iter().map(|m| (*m, true)))
        .collect();
    both.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
    let from_region = both.iter().take(100).filter(|(_, ours)| *ours).count();
    Understanding {
        tail: (top - median) / sd,
        regional: from_region as f32 / 100.0,
    }
}

impl Bank {
    fn load(bytes: &'static [u8], magic: &[u8; 4], name: &str) -> Self {
        let count = u32::from_le_bytes(bytes[4..8].try_into().expect("bank header")) as usize;
        let width = u32::from_le_bytes(bytes[8..12].try_into().expect("bank header")) as usize;
        assert!(&bytes[..4] == magic && width == DIM, "{name} is not this space");
        let stride = 4 + width;
        let rows = &bytes[12..];
        assert_eq!(rows.len(), count * stride, "{name} is truncated");
        let scales: Vec<f32> = (0..count)
            .map(|at| {
                f32::from_le_bytes(rows[at * stride..at * stride + 4].try_into().expect("scale"))
            })
            .collect();
        let mut bank = Bank {
            scales,
            rows,
            width,
            background: Vec::new(),
        };
        bank.background = bank.dots(&vectors::BACKGROUND);
        bank
    }

    fn decode(&self, at: usize) -> Vec<f32> {
        let scale = self.scales[at];
        self.row(at).iter().map(|q| f32::from(*q as i8) * scale).collect()
    }

    fn row(&self, at: usize) -> &[u8] {
        let stride = 4 + self.width;
        &self.rows[at * stride + 4..(at + 1) * stride]
    }

    fn dots(&self, vector: &[f32]) -> Vec<f32> {
        let stride = 4 + self.width;
        self.scales
            .iter()
            .enumerate()
            .map(|(at, scale)| {
                let row = &self.rows[at * stride + 4..(at + 1) * stride];
                let sum: f32 = row
                    .iter()
                    .zip(vector)
                    .map(|(q, v)| f32::from(*q as i8) * v)
                    .sum();
                sum * scale
            })
            .collect()
    }
}

const LEARN_TOP: usize = 30;
const LEARN_STEPS: usize = 300;
const LEARN_RATE: f32 = 2.0;
const LEARN_L2: f32 = 1e-3;
const LEARN_START: f32 = 10.0;
const LEARN_EXCLUDE: usize = 1000;
const LEARN_POOL_NEGATIVES: usize = 8000;
const LEARN_TEXT_SHARE: f32 = 0.5;
const LEARNED_MARGIN: f32 = 1.25;

fn lanes_dot(a: &[f32], b: &[f32]) -> f32 {
    let (a_lanes, a_tail) = a.as_chunks::<8>();
    let (b_lanes, b_tail) = b.as_chunks::<8>();
    let mut lanes = [0.0f32; 8];
    for (x, y) in a_lanes.iter().zip(b_lanes) {
        for lane in 0..8 {
            lanes[lane] += x[lane] * y[lane];
        }
    }
    let tail: f32 = a_tail.iter().zip(b_tail).map(|(x, y)| x * y).sum();
    lanes.iter().sum::<f32>() + tail
}

pub const LEARNER: &str = "subject-5";

pub fn written_by_learn(vector: &[f32]) -> bool {
    dot(vector, vector) > 4.0
}

pub struct Learned {
    pub vector: Vec<f32>,
    pub cut: f32,
}

pub fn learn(text: &[f32]) -> Learned {
    let pool = pool();
    let start = unit(text);
    let ranked = ranked_pool(&start);
    let positives: Vec<Vec<f32>> = ranked
        .iter()
        .take(LEARN_TOP)
        .map(|at| pool.decode(*at))
        .collect();
    blend(&start, &positives, &ranked)
}

pub fn learn_examples(examples: &[Vec<f32>]) -> Learned {
    let mut sum = vec![0.0f32; DIM];
    for example in examples {
        for (total, x) in sum.iter_mut().zip(example) {
            *total += x;
        }
    }
    let start = unit(&sum);
    let ranked = ranked_pool(&start);
    blend(&start, examples, &ranked)
}

fn ranked_pool(direction: &[f32]) -> Vec<usize> {
    let pool = pool();
    let mut ranked: Vec<(f32, usize)> = pool
        .dots(direction)
        .into_iter()
        .zip(&pool.background)
        .map(|(dot, background)| dot - background)
        .enumerate()
        .map(|(at, margin)| (margin, at))
        .collect();
    ranked.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked.into_iter().map(|(_, at)| at).collect()
}

fn blend(start: &[f32], positives: &[Vec<f32>], ranked: &[usize]) -> Learned {
    let (pool, bank) = (pool(), bank());
    let rest = &ranked[LEARN_EXCLUDE.min(ranked.len())..];
    let negatives: Vec<usize> = if rest.len() <= LEARN_POOL_NEGATIVES {
        rest.to_vec()
    } else {
        (0..LEARN_POOL_NEGATIVES)
            .map(|at| rest[at * (rest.len() - 1) / (LEARN_POOL_NEGATIVES - 1)])
            .collect()
    };
    let head = fit(start, positives, &negatives);

    let margins_over = |direction: &[f32]| -> Vec<f32> {
        let bank_part = bank
            .dots(direction)
            .into_iter()
            .zip(&bank.background)
            .map(|(dot, background)| dot - background);
        let pool_dots = pool.dots(direction);
        let pool_part = rest.iter().map(|&at| pool_dots[at] - pool.background[at]);
        bank_part.chain(pool_part).collect()
    };
    let spread = |values: &[f32]| -> f32 {
        let n = values.len() as f32;
        let mean = values.iter().sum::<f32>() / n;
        (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n)
            .sqrt()
            .max(1e-6)
    };
    let text_weight = LEARN_TEXT_SHARE / spread(&margins_over(start));
    let head_weight = (1.0 - LEARN_TEXT_SHARE) / spread(&margins_over(&head));
    let background_weight = 1.0 - text_weight - head_weight;
    let blended: Vec<f32> = start
        .iter()
        .zip(&head)
        .zip(vectors::BACKGROUND.iter())
        .map(|((t, h), b)| text_weight * t + head_weight * h + background_weight * b)
        .collect();
    let (bytes, scale) = quantize(&blended);
    let vector = dequantize(&bytes, scale);
    let scores = margins_over(&vector);
    let highest = scores.iter().copied().fold(f32::MIN, f32::max);
    Learned {
        cut: highest + LEARNED_MARGIN * spread(&scores),
        vector,
    }
}

fn fit(start: &[f32], positives: &[Vec<f32>], pool_negatives: &[usize]) -> Vec<f32> {
    let (pool, bank) = (pool(), bank());
    let rows = positives.len() + bank.scales.len() + pool_negatives.len();
    let sources: Vec<(f32, f32)> = positives
        .iter()
        .map(|_| (1.0, 0.5 / positives.len() as f32))
        .chain((0..bank.scales.len()).map(|_| (0.0, 0.25 / bank.scales.len() as f32)))
        .chain(
            pool_negatives
                .iter()
                .map(|_| (0.0, 0.25 / pool_negatives.len().max(1) as f32)),
        )
        .collect();
    let mut matrix = vec![0.0f32; rows * DIM];
    let (matrix_rows, _) = matrix.as_chunks_mut::<DIM>();
    let (positive_rows, rest) = matrix_rows.split_at_mut(positives.len());
    let (bank_rows, pool_rows) = rest.split_at_mut(bank.scales.len());
    for (row, positive) in positive_rows.iter_mut().zip(positives) {
        row.copy_from_slice(&positive[..DIM]);
    }
    for (at, row) in bank_rows.iter_mut().enumerate() {
        row.copy_from_slice(&bank.decode(at));
    }
    for (row, &at) in pool_rows.iter_mut().zip(pool_negatives) {
        row.copy_from_slice(&pool.decode(at));
    }
    let mut weights: Vec<f32> = start.iter().map(|x| x * LEARN_START).collect();
    let mut bias = 0.0f32;
    let mut gradient = vec![0.0f32; DIM];
    for _ in 0..LEARN_STEPS {
        gradient.iter_mut().for_each(|g| *g = 0.0);
        let mut bias_gradient = 0.0f32;
        for (row, (label, weight)) in matrix.as_chunks::<DIM>().0.iter().zip(&sources) {
            let z = lanes_dot(row, &weights) + bias;
            let g = weight * (1.0 / (1.0 + (-z).exp()) - label);
            bias_gradient += g;
            for (total, x) in gradient.iter_mut().zip(row) {
                *total += g * x;
            }
        }
        for (w, g) in weights.iter_mut().zip(&gradient) {
            *w -= LEARN_RATE * (g + LEARN_L2 * *w);
        }
        bias -= LEARN_RATE * bias_gradient;
    }
    unit(&weights)
}

pub fn encode_examples(examples: &[Vec<f32>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(examples.len() * (4 + DIM));
    for example in examples {
        let (bytes, scale) = quantize(example);
        out.extend_from_slice(&scale.to_le_bytes());
        out.extend(bytes);
    }
    out
}

pub fn decode_examples(bytes: &[u8]) -> Vec<Vec<f32>> {
    bytes
        .as_chunks::<{ 4 + DIM }>()
        .0
        .iter()
        .map(|chunk| {
            let scale = f32::from_le_bytes(chunk[..4].try_into().expect("four bytes"));
            unit(&dequantize(&chunk[4..], scale))
        })
        .collect()
}

pub fn learned_examples(bytes: &[u8]) -> Option<(Vec<f32>, f32)> {
    type Memo = HashMap<u64, (Vec<f32>, f32)>;
    static MEMO: std::sync::Mutex<Option<Memo>> = std::sync::Mutex::new(None);
    let examples = decode_examples(bytes);
    if examples.is_empty() {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(bytes, &mut hasher);
    let key = std::hash::Hasher::finish(&hasher);
    if let Some(found) = MEMO
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|memo| memo.get(&key).cloned())
    {
        return Some(found);
    }
    let Learned { vector, cut } = learn_examples(&examples);
    let mut memo = MEMO.lock().unwrap();
    let memo = memo.get_or_insert_with(HashMap::new);
    if memo.len() >= 1024 {
        memo.clear();
    }
    memo.insert(key, (vector.clone(), cut));
    Some((vector, cut))
}

pub fn cut_for(print: u64, vector: &[f32], samples: u32) -> f32 {
    static MEMO: std::sync::Mutex<Option<HashMap<(u64, bool), f32>>> = std::sync::Mutex::new(None);
    let key = (print, samples == 0);
    if let Some(cut) = MEMO
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|memo| memo.get(&key).copied())
    {
        return cut;
    }
    let cut = bank_cut(vector, rate_for(samples));
    let mut memo = MEMO.lock().unwrap();
    let memo = memo.get_or_insert_with(HashMap::new);
    if memo.len() >= 4096 {
        memo.clear();
    }
    memo.insert(key, cut);
    cut
}

pub fn bank_cut(vector: &[f32], rate: f64) -> f32 {
    let bank = bank();
    let mut margins: Vec<f32> = bank
        .dots(vector)
        .into_iter()
        .zip(&bank.background)
        .map(|(dot, background)| dot - background)
        .collect();
    margins.sort_unstable_by(|a, b| b.total_cmp(a));
    let at = ((margins.len() as f64) * rate) as usize;
    margins[at.min(margins.len() - 1)] + BANK_PAD
}

fn uncalibrated_cut(samples: u32) -> f32 {
    match samples {
        0 => FIXED_MODEL_CUT,
        _ => FIXED_EXAMPLE_CUT,
    }
}

fn worst<'a>(filters: &'a [Filter], margins: &[f32], live_only: bool) -> Option<(&'a Filter, f32)> {
    filters
        .iter()
        .zip(margins)
        .filter(|(filter, m)| (!live_only || filter.live) && **m >= filter.cut)
        .max_by(|a, b| (a.1 - a.0.cut).total_cmp(&(b.1 - b.0.cut)))
        .map(|(filter, m)| (filter, *m))
}

fn report(chat: i64, id: i64, filters: &[Filter], margins: &[f32], cached: bool) {
    let decision = worst(filters, margins, true).map_or("none", |(filter, _)| filter.name.as_str());
    let matched = worst(filters, margins, false).map_or("none", |(filter, _)| filter.name.as_str());
    log::info!(
        "imgfilter: chat {chat} file {id} decision={} matched={}{}",
        decision,
        matched,
        if cached { " cached" } else { "" }
    );
}

pub fn cached_margins(
    ctx: &Ctx,
    id: i64,
    filters: &[Filter],
    source: &VisionSource,
) -> Option<Vec<f32>> {
    filters
        .iter()
        .map(|filter| ctx.known_custom(id, filter.print, source))
        .collect()
}

pub const UNSEEN: f32 = f32::NEG_INFINITY;

pub fn score(filter: &Filter, embedding: &[f32], prints: Option<&[Vec<f32>]>) -> f32 {
    match (&filter.faces, prints) {
        (Some(references), Some(prints)) => face_score(references, filter.person, prints, false),
        (Some(_), None) => UNSEEN,
        (None, _) => gated(
            margin(embedding, &filter.vector),
            filter.cut,
            filter.gate,
            embedding,
        ),
    }
}

fn face_score(
    references: &[Vec<f32>],
    person: Option<&super::facelock::Person>,
    prints: &[Vec<f32>],
    drawn: bool,
) -> f32 {
    let Some(person) = person else {
        return super::facelock::photographs_likeness(prints, references);
    };
    let again = if references.is_empty() {
        -1.0
    } else {
        super::facelock::likeness(prints, references)
            - (super::facelock::DUPLICATE - super::facelock::CUT)
    };
    let model = if drawn {
        super::facelock::drawn_likeness(prints, person)
    } else {
        super::facelock::person_likeness(prints, person)
    };
    again.max(model)
}

#[derive(Clone, Debug)]
pub enum Spec {
    Vector(Vec<f32>, f32, Option<Gate>),
    Face(Vec<Vec<f32>>, Option<&'static super::facelock::Person>, f32),
}

impl Spec {
    pub fn is_face(&self) -> bool {
        matches!(self, Self::Face(..))
    }

    pub fn cut(&self) -> f32 {
        match self {
            Self::Vector(_, cut, _) | Self::Face(_, _, cut) => *cut,
        }
    }
}

pub fn spec(filter: &Filter) -> Spec {
    match &filter.faces {
        Some(references) => Spec::Face(references.clone(), filter.person, filter.cut),
        None => Spec::Vector(filter.vector.clone(), filter.cut, filter.gate),
    }
}

pub fn score_spec(
    spec: &Spec,
    embedding: &[f32],
    prints: Option<&[Vec<f32>]>,
    drawn: bool,
) -> f32 {
    match spec {
        Spec::Face(references, person, _) => {
            let whole = match person {
                Some(person) if drawn => super::facelock::drawn_vision_likeness(embedding, person),
                _ => None,
            };
            let faces = prints.map(|prints| face_score(references, *person, prints, drawn));
            match (faces, whole) {
                (Some(_), Some(whole))
                    if whole < super::facelock::CUT - super::facelock::DRAWN_VETO =>
                {
                    whole
                }
                (Some(faces), Some(whole)) => faces.max(whole),
                (Some(only), None) | (None, Some(only)) => only,
                (None, None) => UNSEEN,
            }
        }
        Spec::Vector(vector, cut, gate) => gated(margin(embedding, vector), *cut, *gate, embedding),
    }
}

pub fn score_all(filters: &[Filter], embedding: &[f32], prints: Option<&[Vec<f32>]>) -> Vec<f32> {
    filters.iter().map(|filter| score(filter, embedding, prints)).collect()
}

pub fn remember_all(
    ctx: &Ctx,
    id: i64,
    filters: &[Filter],
    margins: &[f32],
    unasked: &[usize],
    source: &VisionSource,
) {
    for (at, (filter, margin)) in filters.iter().zip(margins).enumerate() {
        if *margin == UNSEEN || *margin <= f32::MIN || unasked.contains(&at) {
            continue;
        }
        ctx.remember_custom(id, filter.print, *margin, source.clone());
    }
}

const VETOED: f32 = 1.0;

pub fn wants_second_look(filters: &[Filter], margins: &[f32]) -> bool {
    super::verify::installed()
        && filters
            .iter()
            .zip(margins)
            .any(|(filter, margin)| filter.subject.is_some() && *margin >= filter.cut)
}

pub async fn second_look(
    image: &Arc<image::RgbImage>,
    (chat, id): (i64, i64),
    filters: &[Filter],
    margins: &mut [f32],
) -> Vec<usize> {
    let mut unasked = Vec::new();
    if !super::verify::installed() {
        return unasked;
    }
    for (at, filter) in filters.iter().enumerate() {
        let Some(subject) = filter.subject else {
            continue;
        };
        if margins[at] < filter.cut {
            continue;
        }
        let yes = if is_trained(subject) { super::verify::YES } else { super::verify::YES_UNTRAINED };
        match super::verify::look(Arc::clone(image), subject, yes).await {
            Ok(Some(p)) if p >= yes => {}
            Ok(Some(p)) => {
                log::info!("imgfilter: chat {chat} file {id} «{}» second look no ({p:.3})", filter.name);
                margins[at] = filter.cut - VETOED;
            }
            Ok(None) => {}
            Err(error) => {
                log::error!("imgfilter: chat {chat} file {id} «{}» second look failed: {error}", filter.name);
                margins[at] = filter.cut - VETOED;
                unasked.push(at);
            }
        }
    }
    unasked
}

pub async fn act_known(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    id: i64,
    filters: &[Filter],
    margins: &[f32],
) {
    report(chat, id, filters, margins, true);
    let Some((filter, _)) = worst(filters, margins, true) else {
        return;
    };
    match message.delete_critical_counted().await {
        Ok(0) => return,
        Ok(_) => {}
        Err(e) => {
            eprintln!("imgfilter: could not delete in {chat}: {e}");
            return;
        }
    }
    if !ctx.claim_moderation(chat, message.id()) {
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    let chances = match super::strict::punish(ctx, message, chat, CAUSE).await {
        super::strict::Outcome::Announced => return,
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::notice::send(
        ctx,
        message,
        chat,
        &format!("تصویر {}", filter.name),
        chances,
    )
    .await;
}

pub struct Detached<'a> {
    pub chat: i64,
    pub chat_ref: PeerRef,
    pub message_id: i32,
    pub media_id: i64,
    pub filters: &'a [Filter],
    pub margins: &'a [f32],
    pub sender: Option<i64>,
    pub name: &'a str,
}

pub async fn act_detached(ctx: &Arc<Ctx>, detached: Detached<'_>) {
    let Detached {
        chat,
        chat_ref,
        message_id,
        media_id,
        filters,
        margins,
        sender,
        name,
    } = detached;
    report(chat, media_id, filters, margins, false);
    let Some((filter, _)) = worst(filters, margins, true) else {
        return;
    };
    match ctx.delete_proven(chat, chat_ref, message_id).await {
        Ok(false) => {
            eprintln!("imgfilter: delete returned zero events in {chat} msg {message_id}");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("imgfilter: could not delete in {chat} msg {message_id}: {e}");
            return;
        }
    }
    if !ctx.claim_moderation(chat, message_id) {
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    super::nsfw::punish_and_notify(
        ctx,
        super::nsfw::DetachedModeration {
            chat,
            chat_ref,
            message_id,
            sender,
            name,
            cause: CAUSE,
            reason: &format!("تصویر {}", filter.name),
        },
    )
    .await;
}

#[derive(Debug, PartialEq)]
enum Ask<'a> {
    FromText(&'a str),
    FromPhoto(&'a str),
    FromExample(&'a str),
    FromFace(&'a str),
    Remove(&'a str),
    Recalibrate(&'a str),
    Live(&'a str, bool),
}

fn after<'a>(text: &'a str, aliases: &[&str]) -> Option<&'a str> {
    for alias in aliases {
        let Some(rest) = super::commands::strip_prefix(text, alias) else {
            continue;
        };
        return Some(rest);
    }
    None
}

fn parse(text: &str) -> Option<Ask<'_>> {
    if let Some(rest) = after(text, TEXT_ADD) {
        return (!rest.is_empty()).then_some(Ask::FromText(rest));
    }
    if let Some(rest) = after(text, PHOTO_ADD) {
        return (!rest.is_empty()).then_some(Ask::FromPhoto(rest));
    }
    if let Some(rest) = after(text, FACE_ADD) {
        return (!rest.is_empty()).then_some(Ask::FromFace(rest));
    }
    if let Some(rest) = after(text, SAMPLE_ADD) {
        return (!rest.is_empty()).then_some(Ask::FromExample(rest));
    }
    if let Some(rest) = after(text, REMOVE) {
        return (!rest.is_empty()).then_some(Ask::Remove(rest));
    }
    if let Some(rest) = after(text, CALIBRATE) {
        return (!rest.is_empty()).then_some(Ask::Recalibrate(rest));
    }

    parse_tune(after(text, TUNE)?)
}

fn parse_tune(rest: &str) -> Option<Ask<'_>> {
    let rest = rest.trim_end_matches(super::commands::separator);
    let (head, tail) = rest.rsplit_once(super::commands::separator)?;
    let (head, tail) = (
        head.trim_matches(super::commands::separator),
        tail.trim_matches(super::commands::separator),
    );
    if head.is_empty() {
        return None;
    }
    if super::commands::contains(LIVE_WORD, tail) {
        return Some(Ask::Live(head, true));
    }
    if super::commands::contains(SHADOW_WORD, tail) {
        return Some(Ask::Live(head, false));
    }
    None
}

fn acceptable(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_NAME
        && !name.contains(':')
        && !name.contains('=')
        && !name.contains('<')
        && !name.contains('&')
        && !name.chars().any(char::is_control)
        && !name.ends_with(" ~")
}

pub async fn handle(ctx: &Arc<Ctx>, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(ask) = parse(text) else {
        return false;
    };
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }

    match ask {
        Ask::FromText(phrase) | Ask::FromPhoto(phrase) => {
            from_text(ctx, message, chat, phrase).await
        }
        Ask::FromExample(name)
            if matches!(super::imgsubject::resolve(name), Some(Route::Person(_))) =>
        {
            from_face(ctx, message, chat, name).await
        }
        Ask::FromExample(name) => from_example(ctx, message, chat, name).await,
        Ask::FromFace(name) => from_face(ctx, message, chat, name).await,
        Ask::Remove(name) => remove(ctx, message, chat, name).await,
        Ask::Recalibrate(name) => recalibrate(ctx, message, chat, name, None).await,
        Ask::Live(name, live) => set_live(ctx, message, chat, name, live).await,
    }
    true
}

enum ExampleImage {
    Thumb(super::nsfw::Ladder),
    Original(Document),
    OriginalFrames(Document),
}

const ANIMATED_EXAMPLE_INSTRUCTION: &str = "برای ساخت فیلتر از فایل چندتصویری یا متحرک، تصویر یا فریم دلخواه را جداگانه به صورت عکس ثابت بفرستید و روی آن ریپلای کنید.";
const UNVERIFIED_EXAMPLE_INSTRUCTION: &str =
    "نوع فایل اصلی مشخص نشد؛ فیلتر ذخیره نشد. یک عکس ثابت بفرستید.";

fn thumbnail_raster_document(media: &Media) -> Option<&Document> {
    let document = match media {
        Media::Document(document) => document,
        Media::Sticker(sticker) => &sticker.document,
        _ => return None,
    };
    let declared = matches!(document.mime_type(), Some("image/png" | "image/webp"));
    let named = document.name().is_some_and(|name| {
        name.rsplit('.')
            .next()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png") || ext.eq_ignore_ascii_case("webp"))
    });
    (declared || named).then_some(document)
}

fn verify_example_raster(kind: Option<bool>) -> Result<(), &'static str> {
    match kind {
        Some(false) => Ok(()),
        Some(true) => Err(ANIMATED_EXAMPLE_INSTRUCTION),
        None => Err(UNVERIFIED_EXAMPLE_INSTRUCTION),
    }
}

async fn decode_original_example(bytes: Vec<u8>) -> Result<image::RgbImage, &'static str> {
    let Some(mut frames) = super::nsfw::decode_animated_frames(bytes).await else {
        return Err("فایل اصلی کامل باز نشد؛ فیلتر ذخیره نشد.");
    };
    if frames.len() != 1 {
        return Err(ANIMATED_EXAMPLE_INSTRUCTION);
    }
    Ok(frames.pop().unwrap())
}

fn example_image(media: &Media) -> Result<Option<ExampleImage>, &'static str> {
    let document = match media {
        Media::Document(document) => Some(document),
        Media::Sticker(sticker) => Some(&sticker.document),
        _ => None,
    };
    if document.is_some_and(|document| {
        document.is_animated()
            || document.mime_type().is_some_and(|mime| {
                mime.starts_with("video/")
                    || matches!(
                        mime,
                        "image/gif"
                            | "image/apng"
                            | "image/avif-sequence"
                            | "application/x-tgsticker"
                    )
            })
            || document.name().is_some_and(|name| {
                name.rsplit('.').next().is_some_and(|extension| {
                    ["gif", "apng", "mp4", "webm", "tgs"]
                        .iter()
                        .any(|animated| extension.eq_ignore_ascii_case(animated))
                })
            })
    }) {
        return Err(ANIMATED_EXAMPLE_INSTRUCTION);
    }
    if let Some(document) = super::nsfw::animated_document(media) {
        return Ok(Some(ExampleImage::OriginalFrames(document.clone())));
    }
    Ok(super::nsfw::ladder(media)
        .map(ExampleImage::Thumb)
        .or_else(|| {
            super::nsfw::still_image_without_thumb(media, false).map(ExampleImage::Original)
        }))
}

async fn embed_reply(
    ctx: &Arc<Ctx>,
    chat: i64,
    reply: &Message,
) -> Result<(Vec<f32>, bool), &'static str> {
    let (embedding, portrait) = with_reply_image(ctx, chat, reply, |image| {
        (super::vision::embed_of(&image), super::facelock::portrait(&image))
    })
    .await?;
    match embedding {
        Ok(Some(embedding)) => Ok((embedding, portrait)),
        Ok(None) => Err("مدل تصویر در دسترس نیست."),
        Err(error) => {
            log::error!("image filter: image embedding failed: {error}");
            Err("مدل تصویر خطا داد؛ نتیجه ذخیره نشد.")
        }
    }
}

async fn with_reply_image<T: Send + 'static>(
    ctx: &Arc<Ctx>,
    chat: i64,
    reply: &Message,
    work: impl FnOnce(image::RgbImage) -> T + Send + 'static,
) -> Result<T, &'static str> {
    let Some(media) = reply.media() else {
        return Err("روی یک عکس ثابت ریپلای کنید.");
    };
    let Some(id) = super::nsfw::file_id(&media) else {
        return Err("این رسانه تصویر قابل بررسی ندارد.");
    };
    let Some(source) = example_image(&media)? else {
        return Err("این رسانه تصویر قابل بررسی ندارد.");
    };
    let _buffers = ctx.media_buffer_slot().await;
    if matches!(source, ExampleImage::Thumb(_))
        && let Some(document) = thumbnail_raster_document(&media)
    {
        verify_example_raster(
            super::nsfw::probe_example_raster(ctx, chat, id, document).await,
        )?;
    }
    let bytes = match &source {
        ExampleImage::Thumb(ladder) => {
            super::nsfw::fetch(ctx, chat, id, ladder, ladder.start).await
        }
        ExampleImage::Original(document) | ExampleImage::OriginalFrames(document) => {
            super::nsfw::fetch_still_example(ctx, chat, id, document).await
        }
    };
    let Some(bytes) = bytes else {
        return Err("تصویر دانلود نشد.");
    };
    let _slot = ctx.nsfw_slot().await;
    let original_still = matches!(&source, ExampleImage::Original(_));
    let image = match source {
        ExampleImage::OriginalFrames(_) => decode_original_example(bytes).await?,
        ExampleImage::Original(_) | ExampleImage::Thumb(_) => {
            if original_still {
                match super::nsfw::raster_kind_full(&bytes) {
                    Some(true) => return Err(ANIMATED_EXAMPLE_INSTRUCTION),
                    None => return Err("نوع تصویر مشخص نشد؛ نتیجه ذخیره نشد."),
                    Some(false) => {}
                }
            }
            let Ok(Some(image)) =
                tokio::task::spawn_blocking(move || super::nsfw::decode_still(&bytes).ok()).await
            else {
                return Err("تصویر باز نشد.");
            };
            image
        }
    };
    tokio::task::spawn_blocking(move || work(image))
        .await
        .map_err(|error| {
            log::error!("image filter: model worker failed: {error}");
            "مدل تصویر خطا داد؛ نتیجه ذخیره نشد."
        })
}

#[derive(Debug)]
pub enum FilterError {
    Invalid,
    Full,
    ModelUnavailable,
    Settings(crate::state::SettingsWriteError),
    Database(sqlx::Error),
    Person,
    Indistinct(Indistinct),
    NotUnderstood,
    NameTaken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indistinct {
    Game,
    App,
}

impl Indistinct {
    pub fn word(self) -> &'static str {
        match self {
            Self::Game => "یک بازی",
            Self::App => "یک برنامه یا برند",
        }
    }
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("invalid image-filter name"),
            Self::Full => formatter.write_str("image-filter capacity reached"),
            Self::ModelUnavailable => formatter.write_str("image model unavailable"),
            Self::Settings(error) => error.fmt(formatter),
            Self::Database(error) => {
                write!(formatter, "image-filter database write failed: {error}")
            }
            Self::Person => formatter.write_str("a person is recognised by face, not by name"),
            Self::Indistinct(kind) => write!(formatter, "{kind:?} cannot be told apart by sight"),
            Self::NotUnderstood => formatter.write_str("the image model does not know the phrase"),
            Self::NameTaken => formatter.write_str("the name belongs to another kind of filter"),
        }
    }
}

impl std::error::Error for FilterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Settings(error) => Some(error),
            Self::Database(error) => Some(error),
            Self::Invalid
            | Self::Full
            | Self::ModelUnavailable
            | Self::Person
            | Self::Indistinct(_)
            | Self::NotUnderstood
            | Self::NameTaken => None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Made {
    Filter { english: Option<&'static str> },
    Lock {
        key: &'static str,
        name: &'static str,
    },
    Face,
}

async fn text_direction(
    ctx: &Arc<Ctx>,
    phrase: &str,
    frames: super::imgtext::Frames,
) -> Result<Vec<f32>, FilterError> {
    let phrase_owned = phrase.to_owned();
    let slot = ctx.nsfw_slot().await;
    let vector = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        super::imgtext::try_embed_framed(&phrase_owned, frames)
    })
    .await;
    match vector {
        Ok(Ok(super::imgtext::EmbedOutcome::Embedding(vector))) => Ok(vector),
        Ok(Ok(super::imgtext::EmbedOutcome::Unavailable | super::imgtext::EmbedOutcome::Empty)) => {
            Err(FilterError::ModelUnavailable)
        }
        Ok(Err(error)) => {
            log::error!("image filter: text embedding failed: {error}; result not saved");
            Err(FilterError::ModelUnavailable)
        }
        Err(error) => {
            log::error!("image filter: text embedding worker failed: {error}; result not saved");
            Err(FilterError::ModelUnavailable)
        }
    }
}

pub fn phrase_source(phrase: &str) -> Option<(String, super::imgtext::Frames)> {
    match super::imgsubject::resolve(phrase) {
        Some(Route::Visual(english)) => {
            Some((english.to_owned(), super::imgtext::Frames::English))
        }
        Some(_) => None,
        None => Some((phrase.to_owned(), super::imgtext::Frames::Typed)),
    }
}

pub async fn create_from_phrase(
    ctx: &Arc<Ctx>,
    chat: i64,
    phrase: &str,
) -> Result<Made, FilterError> {
    if !acceptable(phrase) {
        return Err(FilterError::Invalid);
    }
    let route = super::imgsubject::resolve(phrase);
    match route {
        Some(Route::Explicit) => {
            return arm(ctx, chat, super::nsfw::LOCK, "محتوای غیراخلاقی").await;
        }
        Some(Route::Concept(key)) => {
            let Some(concept) = super::concepts::CONCEPTS
                .iter()
                .find(|concept| concept.key == key)
            else {
                return Err(FilterError::NotUnderstood);
            };
            return arm(ctx, chat, concept.key, concept.names[0]).await;
        }
        Some(Route::Person(english)) => {
            if super::facelock::person(english).is_none() {
                return Err(FilterError::Person);
            }
            if full(ctx, chat, phrase) {
                return Err(FilterError::Full);
            }
            let existing = ctx
                .settings
                .try_image_filter(chat, phrase)
                .await
                .map_err(FilterError::Database)?;
            let references = existing
                .as_ref()
                .and_then(|row| row.faces.as_deref())
                .and_then(super::facelock::decode)
                .unwrap_or_default();
            let live = existing
                .as_ref()
                .is_none_or(|row| row.faces.is_none() || row.live);
            save_face(ctx, chat, phrase, &references, live).await?;
            return Ok(Made::Face);
        }
        Some(Route::Game(_)) => return Err(FilterError::Indistinct(Indistinct::Game)),
        Some(Route::App(_)) => return Err(FilterError::Indistinct(Indistinct::App)),
        Some(Route::Visual(_)) | None => {}
    }
    if full(ctx, chat, phrase) {
        return Err(FilterError::Full);
    }
    match ctx.settings.try_image_filter(chat, phrase).await {
        Ok(Some(row)) if row.faces.is_some() => return Err(FilterError::NameTaken),
        Ok(_) => {}
        Err(error) => return Err(FilterError::Database(error)),
    }
    if let Some(Route::Visual(english)) = route
        && let Some(fitted) = fitted_filter(english)
    {
        save_core(ctx, chat, phrase, &fitted.vector, 0, None, Some(fitted.cut)).await?;
        return Ok(Made::Filter {
            english: Some(english),
        });
    }
    let Some((text, frames)) = phrase_source(phrase) else {
        return Err(FilterError::NotUnderstood);
    };
    let vector = text_direction(ctx, &text, frames).await?;
    let typed = route.is_none();
    let slot = ctx.nsfw_slot().await;
    let learned = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        if typed && !understanding(&vector).ok() {
            return None;
        }
        Some(learn(&vector))
    })
    .await
    .map_err(|error| {
        log::error!("image filter: learning worker failed: {error}; result not saved");
        FilterError::ModelUnavailable
    })?
    .ok_or(FilterError::NotUnderstood)?;
    save_core(
        ctx,
        chat,
        phrase,
        &learned.vector,
        0,
        None,
        Some(learned.cut),
    )
    .await?;
    Ok(Made::Filter {
        english: match route {
            Some(Route::Visual(english)) => Some(english),
            _ => None,
        },
    })
}

async fn arm(
    ctx: &Arc<Ctx>,
    chat: i64,
    key: &'static str,
    name: &'static str,
) -> Result<Made, FilterError> {
    super::locks::try_set(ctx, chat, key, true)
        .await
        .map_err(FilterError::Settings)?;
    Ok(Made::Lock { key, name })
}

async fn from_text(ctx: &Arc<Ctx>, message: &Message, chat: i64, phrase: &str) {
    if matches!(super::imgsubject::resolve(phrase), Some(Route::Person(_)))
        && message.reply_to_message_id().is_some()
    {
        return from_face(ctx, message, chat, phrase).await;
    }
    let (kind, reply) = match create_from_phrase(ctx, chat, phrase).await {
        Ok(Made::Filter { english }) => (
            ResponseKind::FilterManagement,
            super::premium::icon_html(
                Some(super::premium::Icon::Image),
                format!(
                    "<b>فیلتر تصویری</b>\n\nنام · <b>{}</b>{}\n\nحالت · فعال",
                    esc(phrase),
                    english.map_or_else(String::new, |english| format!(
                        "\nبرداشت · <code>{}</code>",
                        esc(english)
                    )),
                ),
            ),
        ),
        Ok(Made::Lock { name, .. }) => (
            ResponseKind::FilterManagement,
            super::premium::text(format!(
                "✓ برای «{}» قفل «{}» فعال شد؛ مدل این قفل برای همین کار ساخته و سنجیده شده است.",
                esc(phrase),
                esc(name),
            )),
        ),
        Ok(Made::Face) => (ResponseKind::FilterManagement, built_in_reply(phrase)),
        Err(FilterError::Person) => (
            ResponseKind::FilterManagement,
            super::premium::html(format!(
                "<b>قفل چهره</b>\n\n«{0}» یک شخص است و هیچ مدلی او را از روی نام نمی شناسد، از روی چهره می شناسد.\n\n\
                 روی یک عکس واضح از چهره اش ریپلای کنید و بنویسید «قفل چهره {0}».",
                esc(phrase),
            )),
        ),
        Err(FilterError::Indistinct(kind)) => (
            ResponseKind::FilterManagement,
            super::premium::html(format!(
                "<b>فیلتر تصویری</b>\n\n«{}» {} است و مدل تصویر آن را از نمونه های مشابهش تشخیص نمی دهد؛ \
                 فیلتری ساخته نشد تا عکس های بی ربط حذف نشوند.{}",
                esc(phrase),
                kind.word(),
                match kind {
                    Indistinct::Game => {
                        "\n\n<b>قفل تصویر بازی کامپیوتری</b> · عکس همه بازی ها را حذف می کند"
                    }
                    Indistinct::App => "",
                },
            )),
        ),
        Err(FilterError::NotUnderstood) => (
            ResponseKind::FilterManagement,
            super::premium::html(format!(
                "<b>فیلتر تصویری</b>\n\n«{0}» برای مدل تصویر شناخته شده نیست؛ فیلتری ساخته نشد تا عکس های بی ربط حذف نشوند.\n\n\
                 <b>قفل چهره {0}</b> · اگر شخص است، روی عکس چهره اش ریپلای کنید\n\
                 <b>فیلتر این {0}</b> · اگر چیز دیگری است، روی چند عکس نمونه ریپلای کنید\n\n\
                 <i>نام انگلیسی یا کلمه ساده تری از همین موضوع هم ممکن است شناخته شود.</i>",
                esc(phrase),
            )),
        ),
        Err(FilterError::NameTaken) => (
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                format!(
                    "«{}» نام یک قفل چهره است؛ اول آن را حذف کنید یا نام دیگری بدهید.",
                    esc(phrase)
                ),
            ),
        ),
        Err(FilterError::Invalid) => (
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                format!("این نام پذیرفته نمی شود: تا {MAX_NAME} حرف، بدون : و = و <."),
            ),
        ),
        Err(FilterError::Full) => (
            ResponseKind::FilterManagement,
            super::premium::text(format!("لیست فیلتر تصویری پر است ({MAX_FILTERS} مورد).")),
        ),
        Err(FilterError::ModelUnavailable) => (
            ResponseKind::CommandError,
            super::premium::text(
                "مدل عمومی تصویر روی این سرور نصب نیست، پس فیلتر متنی و «قفل تصویر» کار نمی کنند.\n\
                 با ریپلای روی یک عکس و «فیلتر این ‹نام›» می شود از روی نمونه فیلتر ساخت.",
            ),
        ),
        Err(error @ (FilterError::Settings(_) | FilterError::Database(_))) => {
            ::log::warn!("image filters: create for {chat}/{phrase} failed: {error}");
            (
                ResponseKind::CommandError,
                super::premium::text("فیلتر ذخیره نشد؛ دوباره تلاش کنید."),
            )
        }
    };
    super::respond(ctx, message, kind, reply).await;
}

async fn from_face(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str) {
    let refuse =
        |text: String| super::premium::icon_text(Some(super::premium::Icon::ErrorRed), text);
    if !acceptable(name) {
        let text = format!("این نام پذیرفته نمی شود: تا {MAX_NAME} حرف، بدون : و = و <.");
        super::respond(ctx, message, ResponseKind::CommandError, refuse(text)).await;
        return;
    }
    let existing = match ctx.settings.try_image_filter(chat, name).await {
        Ok(existing) => existing,
        Err(error) => {
            log::warn!("face lock: read for {chat}/{name} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "اطلاعات فیلتر خوانده نشد؛ دوباره تلاش کنید.",
            )
            .await;
            return;
        }
    };
    let replaced = existing.as_ref().is_some_and(|row| row.faces.is_none());
    let mut references = match existing.as_ref().and_then(|row| row.faces.as_deref()) {
        Some(blob) => super::facelock::decode(blob).unwrap_or_default(),
        None => Vec::new(),
    };
    if existing.is_none() && full(ctx, chat, name) {
        super::respond(
            ctx,
            message,
            ResponseKind::FilterManagement,
            format!("لیست فیلتر تصویری پر است ({MAX_FILTERS} مورد)."),
        )
        .await;
        return;
    }
    if references.len() >= super::facelock::MAX_REFERENCES {
        super::respond(
            ctx,
            message,
            ResponseKind::FilterManagement,
            format!(
                "«{}» از قبل {} چهره مرجع دارد و بیشتر از این دقیق تر نمی شود.",
                esc(name),
                super::facelock::MAX_REFERENCES
            ),
        )
        .await;
        return;
    }
    let built_in = matches!(
        super::imgsubject::resolve(name),
        Some(Route::Person(english)) if super::facelock::person(english).is_some()
    );
    let Ok(Some(reply)) = message.get_reply().await else {
        if built_in {
            let live = replaced || existing.as_ref().is_none_or(|row| row.live);
            if let Err(error) = save_face(ctx, chat, name, &references, live).await {
                ::log::warn!("face lock: save for {chat}/{name} failed: {error}");
                let text = "فیلتر ذخیره نشد؛ دوباره تلاش کنید.".to_owned();
                super::respond(ctx, message, ResponseKind::CommandError, refuse(text)).await;
                return;
            }
            super::respond(ctx, message, ResponseKind::FilterManagement, built_in_reply(name)).await;
            return;
        }
        let text = format!(
            "روی عکسی که چهره «{0}» در آن پیدا است ریپلای کنید و بنویسید «قفل چهره {0}».",
            esc(name)
        );
        super::respond(ctx, message, ResponseKind::CommandError, text).await;
        return;
    };
    let found = match with_reply_image(ctx, chat, &reply, |image| {
        super::facelock::reference(&image)
    })
    .await
    {
        Ok(found) => found,
        Err(why) => {
            super::respond(ctx, message, ResponseKind::CommandError, why).await;
            return;
        }
    };
    let (print, faces) = match found {
        Ok(super::facelock::Reference::Found { print, faces }) => (print, faces),
        Ok(super::facelock::Reference::NoFace) => {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "در این عکس چهره ای به اندازه کافی بزرگ و واضح پیدا نشد؛ عکسی بفرستید که صورت از روبه رو دیده شود.",
            )
            .await;
            return;
        }
        Ok(super::facelock::Reference::Unavailable) => {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "مدل تشخیص چهره روی این سرور در دسترس نیست.",
            )
            .await;
            return;
        }
        Err(error) => {
            log::error!("face lock: reference for {chat}/{name} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "مدل تشخیص چهره خطا داد؛ نتیجه ذخیره نشد.",
            )
            .await;
            return;
        }
    };
    references.push(print);
    let live = replaced || existing.as_ref().is_none_or(|row| row.live);
    if let Err(error) = save_face(ctx, chat, name, &references, live).await {
        let text = match error {
            FilterError::Full => {
                format!("فیلتر ذخیره نشد؛ هر گروه حداکثر {MAX_FILTERS} فیلتر دارد.")
            }
            other => {
                ::log::warn!("face lock: save for {chat}/{name} failed: {other}");
                "فیلتر ذخیره نشد؛ دوباره تلاش کنید.".to_owned()
            }
        };
        super::respond(ctx, message, ResponseKind::CommandError, refuse(text)).await;
        return;
    }
    let mut notes = Vec::new();
    if replaced {
        notes.push("فیلتر تصویری قبلی با همین نام جایگزین شد.".to_owned());
    }
    if faces > 1 {
        notes.push(format!("در این عکس {faces} چهره بود؛ بزرگ ترین برداشته شد."));
    }
    let crowd = if notes.is_empty() {
        String::new()
    } else {
        format!("\n\n<i>{}</i>", notes.join("\n"))
    };
    let hint = if built_in {
        "\n\nچهره او از قبل شناخته شده است؛ این عکس و فرستادن دوباره اش هم حذف می شود.".to_owned()
    } else if references.len() < 3 {
        format!(
            "\n\nبا همین دستور روی یکی دو عکس دیگر از «{}» هم ریپلای کنید؛ هر عکس تازه عکس های بیشتری از او را شناختنی می کند.",
            esc(name)
        )
    } else {
        String::new()
    };
    super::respond(
        ctx,
        message,
        ResponseKind::FilterManagement,
        super::premium::icon_html(
            Some(super::premium::Icon::Image),
            format!(
                "<b>قفل چهره</b>\n\nنام · <b>{}</b>\nچهره مرجع · <b>{}</b>\n\nحالت · {}{crowd}{hint}",
                esc(name),
                references.len(),
                if live { "فعال" } else { "بررسی بدون حذف" },
            ),
        ),
    )
    .await;
}

fn built_in_reply(name: &str) -> grammers_client::message::InputMessage {
    super::premium::icon_html(
        Some(super::premium::Icon::Image),
        format!(
            "<b>قفل چهره</b>\n\nنام · <b>{}</b>\nشناسایی · چهره او از قبل شناخته شده است\n\nحالت · فعال\n\n\
             <i>عکس، استیکر و تصویر کوچک او حذف می شود و چهره های شبیه نه. اگر عکس یا استیکری از او ماند، روی همان با همین دستور ریپلای کنید تا آن هم حذف شود.</i>",
            esc(name)
        ),
    )
}

pub enum Refresh {
    Learned(Learned),
    Inert(&'static str),
    Unavailable,
}

pub fn refresh(phrase: &str) -> Refresh {
    if let Some(Route::Visual(english)) = super::imgsubject::resolve(phrase)
        && let Some(fitted) = fitted_filter(english)
    {
        return Refresh::Learned(fitted);
    }
    let Some((text, frames)) = phrase_source(phrase) else {
        return Refresh::Inert("the subject table routes it to a lock, a face or a refusal");
    };
    let Some(vector) = super::imgtext::embed_framed(&text, frames) else {
        return Refresh::Unavailable;
    };
    if frames == super::imgtext::Frames::Typed && !understanding(&vector).ok() {
        return Refresh::Inert("the general model does not know the phrase");
    }
    Refresh::Learned(learn(&vector))
}

pub fn inert_vector() -> Vec<f32> {
    face_lock_vector()
}

pub const NEVER: f32 = 1e30;

fn face_lock_vector() -> Vec<f32> {
    vectors::BACKGROUND.iter().map(|b| 3.0 * b).collect()
}

async fn save_face(
    ctx: &Arc<Ctx>,
    chat: i64,
    name: &str,
    references: &[Vec<f32>],
    live: bool,
) -> Result<(), FilterError> {
    let (bytes, scale) = quantize(&face_lock_vector());
    let faces = super::facelock::encode(references);
    let saved = ctx
        .settings
        .try_save_image_filter(
            chat,
            crate::state::ImageFilterWrite {
                name,
                vector: &bytes,
                scale,
                cut: NEVER,
                rate: DEFAULT_RATE,
                live,
                samples: 0,
                calibrated: true,
                examples: None,
                faces: Some(&faces),
            },
        )
        .await
        .map_err(FilterError::Database)?;
    if !saved {
        return Err(FilterError::Full);
    }
    ctx.settings
        .try_set(chat, &key(name), true)
        .await
        .map_err(FilterError::Settings)?;
    ctx.forget_image_filters(chat);
    Ok(())
}

async fn from_example(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str) {
    if !acceptable(name) {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                format!("این نام پذیرفته نمی شود: تا {MAX_NAME} حرف، بدون : و = و <."),
            ),
        )
        .await;
        return;
    }
    if full(ctx, chat, name) {
        super::respond(
            ctx,
            message,
            ResponseKind::FilterManagement,
            format!("لیست فیلتر تصویری پر است ({MAX_FILTERS} مورد)."),
        )
        .await;
        return;
    }
    let Ok(Some(reply)) = message.get_reply().await else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "روی عکسی که می خواهید فیلتر شود ریپلای کنید و «فیلتر این ‹نام›» بنویسید.",
        )
        .await;
        return;
    };
    let existing = match ctx.settings.try_image_filter(chat, name).await {
        Ok(existing) => existing,
        Err(error) => {
            log::warn!("image filter: read for {chat}/{name} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "اطلاعات فیلتر خوانده نشد؛ دوباره تلاش کنید.",
            )
            .await;
            return;
        }
    };
    if existing.as_ref().is_some_and(|row| row.faces.is_some()) {
        return from_face(ctx, message, chat, name).await;
    }
    let (embedding, portrait) = match embed_reply(ctx, chat, &reply).await {
        Ok(found) => found,
        Err(why) => {
            super::respond(ctx, message, ResponseKind::CommandError, why).await;
            return;
        }
    };

    let (vector, samples, examples) = match &existing {
        Some(row) if row.samples > 0 && row.samples < MAX_SAMPLES => {
            let old = dequantize(&row.vector, row.scale);
            let weight = row.samples as f32;
            let mixed: Vec<f32> = old
                .iter()
                .zip(&embedding)
                .map(|(o, n)| (o * weight + n) / (weight + 1.0))
                .collect();
            let mut examples = row
                .examples
                .as_deref()
                .map(decode_examples)
                .filter(|kept| !kept.is_empty())
                .unwrap_or_else(|| vec![unit(&old)]);
            examples.push(unit(&embedding));
            (unit(&mixed), row.samples + 1, examples)
        }
        Some(row) if row.samples >= MAX_SAMPLES => {
            super::respond(
                ctx,
                message,
                ResponseKind::FilterManagement,
                format!(
                    "«{}» از قبل {MAX_SAMPLES} نمونه دارد و بیشتر از این دقیق تر نمی شود.",
                    esc(name)
                ),
            )
            .await;
            return;
        }
        _ => (unit(&embedding), 1, vec![unit(&embedding)]),
    };
    let examples = encode_examples(&examples);
    save(
        ctx,
        message,
        chat,
        name,
        Draft {
            vector: &vector,
            samples,
            examples: &examples,
            portrait,
        },
    )
    .await;
}

fn full(ctx: &Ctx, chat: i64, name: &str) -> bool {
    let names = names(ctx, chat);
    names.len() >= MAX_FILTERS && !names.iter().any(|known| known == name)
}

async fn save_core(
    ctx: &Arc<Ctx>,
    chat: i64,
    name: &str,
    vector: &[f32],
    samples: u32,
    examples: Option<&[u8]>,
    cut: Option<f32>,
) -> Result<(), FilterError> {
    let (bytes, scale) = quantize(vector);
    let cut = cut.unwrap_or_else(|| uncalibrated_cut(samples));
    let saved = ctx
        .settings
        .try_save_image_filter(
            chat,
            crate::state::ImageFilterWrite {
                name,
                vector: &bytes,
                scale,
                cut,
                rate: DEFAULT_RATE,
                live: true,
                samples,
                calibrated: true,
                examples,
                faces: None,
            },
        )
        .await
        .map_err(FilterError::Database)?;
    if !saved {
        return Err(FilterError::Full);
    }
    ctx.settings
        .try_set(chat, &key(name), true)
        .await
        .map_err(FilterError::Settings)?;
    ctx.forget_image_filters(chat);
    Ok(())
}

struct Draft<'a> {
    vector: &'a [f32],
    samples: u32,
    examples: &'a [u8],
    portrait: bool,
}

async fn save(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str, draft: Draft<'_>) {
    let Draft {
        vector,
        samples,
        examples,
        portrait,
    } = draft;
    if let Err(error) = save_core(ctx, chat, name, vector, samples, Some(examples), None).await {
        let reply = match error {
            FilterError::Full => format!(
                "فیلتر ذخیره نشد؛ هر گروه حداکثر {} فیلتر دارد.",
                MAX_FILTERS
            ),
            other => {
                ::log::warn!("image filters: save for {chat}/{name} failed: {other}");
                "فیلتر ذخیره نشد؛ دوباره تلاش کنید.".to_owned()
            }
        };
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(Some(super::premium::Icon::ErrorRed), reply),
        )
        .await;
        return;
    }

    let how = match samples {
        0 => "از روی متن".to_owned(),
        1 => "با ۱ نمونه".to_owned(),
        n => format!("با {n} نمونه"),
    };
    let state = "حالت · فعال\nتصمیم · خودکار";
    let hint = if portrait {
        format!(
            "\n\n<i>این عکس چهره یک شخص است. اگر منظورتان خود اوست، «قفل چهره {}» دقیق تر است؛ این فیلتر آدم های شبیه او را هم حذف می کند.</i>",
            esc(name)
        )
    } else if samples == 1 {
        "\n\nبرای پوشش عکس های متفاوت از همین موضوع، دو نمونهٔ متنوع دیگر هم اضافه کنید.".to_owned()
    } else {
        String::new()
    };
    super::respond(
        ctx,
        message,
        ResponseKind::FilterManagement,
        super::premium::icon_html(
            Some(super::premium::Icon::Image),
            format!(
                "<b>فیلتر تصویری</b>\n\nنام · <b>{}</b>\nساخت · {how}\n\n{state}{hint}",
                esc(name),
                how = esc(&how),
            ),
        ),
    )
    .await;
}

async fn remove(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str) {
    let Some(found) = names(ctx, chat).into_iter().find(|known| known == name) else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                format!("فیلتری با نام «{}» نیست.", esc(name)),
            ),
        )
        .await;
        return;
    };
    if let Err(error) = try_forget(ctx, chat, &found).await {
        ::log::warn!("image filter: delete for {chat}/{found} failed: {error}");
        let uncertain = matches!(
            error,
            DeleteError::Setting(ref error) if error.commit_outcome_unknown()
        );
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            if uncertain {
                "نتیجه حذف فیلتر نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
            } else {
                "فیلتر حذف نشد یا حذف آن کامل نشد؛ دوباره تلاش کنید."
            },
        )
        .await;
        return;
    }
    super::respond(
        ctx,
        message,
        ResponseKind::FilterManagement,
        super::premium::text(format!("✓ «{}» از فیلتر تصویری حذف شد.", esc(&found))),
    )
    .await;
}

async fn set_live(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str, live: bool) {
    let row = match ctx.settings.try_image_filter(chat, name).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    format!("فیلتری با نام «{}» نیست.", esc(name)),
                ),
            )
            .await;
            return;
        }
        Err(error) => {
            log::warn!("image filter: read for {chat}/{name} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "اطلاعات فیلتر خوانده نشد؛ دوباره تلاش کنید.",
            )
            .await;
            return;
        }
    };
    if live && is_inert(&row) {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            format!(
                "«{}» فیلتری است که مدل تصویر آن را نمی شناسد و روشن کردنش چیزی را حذف نمی کند؛ حذفش کنید.",
                esc(name)
            ),
        )
        .await;
        return;
    }
    let cut = row.cut;
    let saved = ctx
        .settings
        .try_save_image_filter(
            chat,
            crate::state::ImageFilterWrite {
                name,
                vector: &row.vector,
                scale: row.scale,
                cut,
                rate: row.rate,
                live,
                samples: row.samples,
                calibrated: true,
                examples: None,
                faces: None,
            },
        )
        .await;
    match saved {
        Ok(true) => {}
        Ok(false) => {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "فیلتر ذخیره نشد؛ ظرفیت پر است.",
            )
            .await;
            return;
        }
        Err(error) => {
            log::warn!("image filter: state change for {chat}/{name} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "تغییر فیلتر ذخیره نشد؛ دوباره تلاش کنید.",
            )
            .await;
            return;
        }
    }
    ctx.forget_image_filters(chat);
    super::respond(
        ctx,
        message,
        ResponseKind::FilterManagement,
        match live {
            true => format!("✓ «{}» فعال شد.", esc(name)),
            false => format!("✗ «{}» به حالت بررسی بدون حذف برگشت.", esc(name)),
        },
    )
    .await;
}

async fn recalibrate(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str, rate: Option<u32>) {
    let row = match ctx.settings.try_image_filter(chat, name).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    format!("فیلتری با نام «{}» نیست.", esc(name)),
                ),
            )
            .await;
            return;
        }
        Err(error) => {
            log::warn!("image filter: read for {chat}/{name} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "اطلاعات فیلتر خوانده نشد؛ دوباره تلاش کنید.",
            )
            .await;
            return;
        }
    };
    if row.faces.is_some() || is_inert(&row) {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            format!("«{}» واسنجی نمی شود.", esc(name)),
        )
        .await;
        return;
    }
    let rate = rate.unwrap_or(row.rate).clamp(RATE_RANGE.0, RATE_RANGE.1);
    let vector = dequantize(&row.vector, row.scale);
    let samples = ctx.samples();
    let Some(found) = calibrate(&vector, &samples, rate) else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            format!(
                "هنوز به اندازه کافی تصویر دیده نشده · {} از {MIN_SAMPLES}.\n\
                 کمی بعد دوباره بفرستید.",
                samples.len()
            ),
        )
        .await;
        return;
    };
    let saved = ctx
        .settings
        .try_save_image_filter(
            chat,
            crate::state::ImageFilterWrite {
                name,
                vector: &row.vector,
                scale: row.scale,
                cut: found.cut,
                rate,
                live: row.live,
                samples: row.samples,
                calibrated: true,
                examples: None,
                faces: None,
            },
        )
        .await;
    match saved {
        Ok(true) => {}
        Ok(false) => {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "فیلتر ذخیره نشد؛ ظرفیت پر است.",
            )
            .await;
            return;
        }
        Err(error) => {
            log::warn!("image filter: calibration write for {chat}/{name} failed: {error}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "واسنجی ذخیره نشد؛ دوباره تلاش کنید.",
            )
            .await;
            return;
        }
    }
    ctx.forget_image_filters(chat);
    super::respond(
        ctx,
        message,
        ResponseKind::FilterManagement,
        super::premium::html(format!(
            "<b>واسنجی</b>\n\nفیلتر · <b>{}</b>\nنرخ خطا · یک در <b>{rate}</b>\n\
             آستانه · <code>{:+.4}</code>\nنمونه · {} تصویر · میانگین <code>{:+.4}</code> · \
             پراکندگی <code>{:.4}</code>",
            esc(name),
            found.cut,
            found.samples,
            found.mean,
            found.sd
        )),
    )
    .await;
}

pub async fn listing(ctx: &Ctx, chat: i64) -> Result<Vec<(String, String)>, sqlx::Error> {
    let filters = ctx.image_filters(chat).await?;
    let mut out: Vec<(String, String)> = filters
        .iter()
        .map(|filter| {
            let label = match filter.live {
                true => filter.name.clone(),
                false => format!("{} ~", filter.name),
            };
            (filter.name.clone(), label)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub enum Armed {
    Toggled,
    Missing,
    Inert,
    Database(sqlx::Error),
}

pub async fn toggle_live(ctx: &Ctx, chat: i64, id: &str) -> Armed {
    let Some(name) = names(ctx, chat)
        .into_iter()
        .find(|known| super::lists::word_id(known) == id)
    else {
        return Armed::Missing;
    };
    let row = match ctx.settings.try_image_filter(chat, &name).await {
        Ok(Some(row)) => row,
        Ok(None) => return Armed::Missing,
        Err(error) => return Armed::Database(error),
    };
    let live = !row.live;
    if live && is_inert(&row) {
        return Armed::Inert;
    }
    let cut = row.cut;
    if let Err(error) = ctx
        .settings
        .try_save_image_filter(
            chat,
            crate::state::ImageFilterWrite {
                name: &name,
                vector: &row.vector,
                scale: row.scale,
                cut,
                rate: row.rate,
                live,
                samples: row.samples,
                calibrated: true,
                examples: None,
                faces: None,
            },
        )
        .await
    {
        return Armed::Database(error);
    }
    ctx.forget_image_filters(chat);
    Armed::Toggled
}

pub async fn panel_rows(ctx: &Ctx, chat: i64) -> Result<Vec<(String, bool, bool)>, sqlx::Error> {
    let filters = ctx.image_filters(chat).await?;
    let mut out: Vec<(String, bool, bool)> = filters
        .iter()
        .map(|filter| (filter.name.clone(), filter.live, true))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[derive(Debug)]
pub enum DeleteError {
    Setting(crate::state::SettingsWriteError),
    Database(sqlx::Error),
}

impl std::fmt::Display for DeleteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setting(error) => error.fmt(formatter),
            Self::Database(error) => {
                write!(formatter, "image-filter database delete failed: {error}")
            }
        }
    }
}

impl std::error::Error for DeleteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Setting(error) => Some(error),
            Self::Database(error) => Some(error),
        }
    }
}

pub fn is_inert(row: &crate::state::ImageFilterRow) -> bool {
    row.faces.is_none() && row.samples == 0 && row.cut >= NEVER
}

pub async fn try_forget(ctx: &Ctx, chat: i64, name: &str) -> Result<(), DeleteError> {
    ctx.settings
        .try_set(chat, &key(name), false)
        .await
        .map_err(DeleteError::Setting)?;
    ctx.settings
        .try_delete_image_filter(chat, name)
        .await
        .map_err(DeleteError::Database)?;
    ctx.forget_image_filters(chat);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_asks_the_picture_what_it_is() {
        let fitted_dog = Gate::for_english("a dog").expect("the vocabulary has dogs");
        assert_eq!(fitted_dog.top, GATE_FITTED);
        let dog = Gate { top: 1, ..fitted_dog };
        let as_picture = |english: &str| vocab().decode(vocab_row(english).unwrap() as usize);
        let cat = as_picture("a cat");
        assert!(dog.names(&as_picture("a dog")));
        assert!(!dog.names(&cat));
        assert_eq!(gated(-1.0, 0.0, Some(dog), &cat), -1.0);
        assert_eq!(gated(1.0, 0.0, Some(dog), &cat), -GATED_BELOW);
        assert_eq!(gated(1.0, 0.0, None, &cat), 1.0);
        assert!(Gate::for_english("no such english").is_none());
    }

    #[test]
    #[ignore = "needs GATE_CHECK_JSON"]
    fn the_gate_agrees_with_its_measurement() {
        let Ok(path) = std::env::var("GATE_CHECK_JSON") else {
            return;
        };
        let (mut agree, mut total) = (0, 0);
        for line in std::fs::read_to_string(path).expect("decisions").lines() {
            let case: serde_json::Value = serde_json::from_str(line).expect("a decision");
            let english = case["english"].as_str().expect("an English");
            let embedding: Vec<f32> = case["embedding"]
                .as_array()
                .expect("an embedding")
                .iter()
                .map(|value| value.as_f64().unwrap_or(0.0) as f32)
                .collect();
            let gate = Gate::for_english(english).expect("a gate");
            total += 1;
            if gate.names(&embedding) == case["names"].as_bool().expect("a decision") {
                agree += 1;
            }
        }
        println!("{agree} of {total} gate decisions agree");
        assert!(total > 0 && agree * 1000 >= total * 995, "{agree} of {total}");
    }

    #[test]
    fn a_photograph_on_a_built_in_lock_counts_only_as_the_same_face() {
        use super::super::facelock::{CUT, DUPLICATE, Person};
        let axis = |i: usize| {
            let mut v = vec![0.0f32; super::super::facelock::WIDTH];
            v[i] = 1.0;
            v
        };
        let person = Person::for_test(axis(0), 0.0, 10.0, vec![axis(0)]);
        let references = vec![axis(2)];
        let at = |cosine: f32| {
            let mut face = axis(2);
            face[2] = cosine;
            face[3] = (1.0 - cosine * cosine).sqrt();
            face
        };
        assert!(face_score(&references, Some(&person), &[at(DUPLICATE - 0.05)], false) < CUT);
        assert!(face_score(&references, Some(&person), &[at(DUPLICATE + 0.01)], false) >= CUT);
        assert!(face_score(&references, Some(&person), &[axis(2)], false) >= CUT);
        assert_eq!(face_score(&[], Some(&person), &[], false), -1.0);
    }

    #[test]
    #[ignore = "needs the second look's files under VISION_FILES and VERIFY_PICTURE"]
    fn the_second_look_vetoes_a_filter_the_picture_does_not_show() {
        super::super::nsfw::configure_model_tests().expect("model test configuration");
        let Ok(path) = std::env::var("VERIFY_PICTURE") else {
            return;
        };
        let image = Arc::new(image::open(path).expect("a picture").to_rgb8());
        let filter = |name: &str, subject: Option<&'static str>| Filter {
            name: name.to_owned(),
            vector: vec![0.0; DIM],
            cut: 0.02,
            live: true,
            print: 1,
            faces: None,
            person: None,
            gate: None,
            subject,
        };
        let filters = vec![
            filter("چنگ", Some("harp")),
            filter("ماشین ظرفشویی", Some("dishwasher")),
            filter("زیر", Some("dishwasher")),
            filter("نمونه", None),
        ];
        let mut margins = vec![0.05, 0.05, 0.01, 0.05];
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
        let unasked = runtime.block_on(second_look(&image, (0, 0), &filters, &mut margins));
        assert!(unasked.is_empty(), "every look answered");
        assert_eq!(margins[0], 0.05, "a harp is a harp");
        assert!(margins[1] < filters[1].cut, "and not a dishwasher");
        assert_eq!(margins[2], 0.01, "under its cut, not asked");
        assert_eq!(margins[3], 0.05, "no subject, not asked");
    }

    #[test]
    fn a_drawn_frame_on_a_built_in_lock_is_read_whole_too() {
        use super::super::facelock::{CUT, Person, WIDTH};
        let axis = |width: usize, i: usize| {
            let mut v = vec![0.0f32; width];
            v[i] = 1.0;
            v
        };
        let person = Person::for_test(axis(WIDTH, 0), 0.0, 10.0, vec![axis(WIDTH, 0)])
            .with_drawn_vision(axis(DIM, 0), 0.0, 0.5);
        let person: &'static Person = Box::leak(Box::new(person));
        let spec = Spec::Face(Vec::new(), Some(person), CUT);
        let embedding = axis(DIM, 0);
        let no_face: &[Vec<f32>] = &[];
        assert!(score_spec(&spec, &embedding, Some(no_face), true) > CUT, "no face found");
        assert!(score_spec(&spec, &embedding, None, true) > CUT, "faces not looked at");
        assert_eq!(score_spec(&spec, &embedding, Some(no_face), false), -1.0, "a photograph");
        assert_eq!(score_spec(&spec, &embedding, None, false), UNSEEN);
        assert!(score_spec(&spec, &axis(DIM, 1), Some(no_face), true) < CUT, "a frame off the direction");

        let taken = Person::for_test(axis(WIDTH, 0), 0.0, 0.5, vec![axis(WIDTH, 0)])
            .with_drawn_vision(axis(DIM, 0), 0.0, 5.0);
        let taken: &'static Person = Box::leak(Box::new(taken));
        let spec = Spec::Face(Vec::new(), Some(taken), CUT);
        let face = [axis(WIDTH, 0)];
        assert!(score_spec(&spec, &axis(DIM, 1), Some(&face), false) >= CUT, "a photograph: no veto");
        assert!(score_spec(&spec, &axis(DIM, 1), Some(&face), true) < CUT, "vetoed");
        let mut nearly = axis(DIM, 1);
        nearly[0] = 3.5;
        assert!(score_spec(&spec, &nearly, Some(&face), true) >= CUT, "not clearly no");
    }

    #[test]
    fn loaded_filter_respects_stored_shadow_and_calibration() {
        let mut vector = vec![0; DIM];
        vector[0] = 127;
        let row = crate::state::ImageFilterRow {
            name: "test".to_owned(),
            vector,
            scale: 1.0 / 127.0,
            cut: 0.77,
            rate: DEFAULT_RATE,
            live: false,
            samples: 1,
            calibrated: true,
            examples: None,
            faces: None,
        };
        let mut filter = super::super::filter_from_row(row).expect("a vector filter loads");
        assert_eq!(filter.cut, bank_cut(&filter.vector, rate_for(1)));
        assert_ne!(filter.cut, 0.77);
        let cut = filter.cut;
        assert!(!filter.live);
        assert!(worst(std::slice::from_ref(&filter), &[cut + 0.1], true).is_none());

        filter.live = true;
        assert!(worst(std::slice::from_ref(&filter), &[cut - 0.01], true).is_none());
        assert!(worst(std::slice::from_ref(&filter), &[cut + 0.01], true).is_some());
    }

    #[test]
    fn stored_examples_round_trip_and_the_fit_follows_them() {
        let mut a = vec![0.0f32; DIM];
        a[5] = 1.0;
        a[9] = 0.3;
        let mut b = vec![0.0f32; DIM];
        b[5] = 0.8;
        b[40] = 0.6;
        let examples = vec![unit(&a), unit(&b)];
        let bytes = encode_examples(&examples);
        assert_eq!(bytes.len(), 2 * (4 + DIM));
        let back = decode_examples(&bytes);
        assert_eq!(back.len(), 2);
        for (kept, original) in back.iter().zip(&examples) {
            assert!(dot(kept, original) > 0.999);
        }
        let learned = learned_examples(&bytes).expect("two examples");
        assert_eq!(learned_examples(&bytes), Some(learned.clone()), "memoised, same answer");
        let (vector, cut) = &learned;
        assert!(cut.is_finite());
        let margin = |e: &[f32]| dot(e, vector) - dot(e, &vectors::BACKGROUND);
        let mut other = vec![0.0f32; DIM];
        other[600] = 1.0;
        for example in &examples {
            assert!(margin(example) > margin(&other));
        }
        assert_eq!(learned_examples(&[]), None);
    }

    #[test]
    #[ignore = "a second of arithmetic per run; the time bound is checked under --release"]
    fn learning_a_phrase_is_deterministic_and_bounded() {
        let mut text = vec![0.0f32; DIM];
        text[3] = 1.0;
        text[100] = -0.5;
        let started = std::time::Instant::now();
        let first = learn(&text);
        let took = started.elapsed();
        let again = learn(&text);
        assert_eq!(first.vector, again.vector, "the same phrase, the same vector");
        assert_eq!(first.cut, again.cut, "and the same cut");
        assert!(first.cut.is_finite());
        let scored: Vec<f32> = first
            .vector
            .iter()
            .zip(vectors::BACKGROUND.iter())
            .map(|(v, b)| v - b)
            .collect();
        assert!(dot(&scored, &unit(&text)) > 0.0, "it still leans the phrase's way");
        let bank = bank();
        let highest = bank
            .dots(&first.vector)
            .into_iter()
            .zip(&bank.background)
            .map(|(dot, background)| dot - background)
            .fold(f32::MIN, f32::max);
        assert!(first.cut > highest, "no everyday picture of the bank gets through");
        let (bytes, scale) = quantize(&first.vector);
        assert!(written_by_learn(&dequantize(&bytes, scale)), "the startup refresh can tell");
        assert!(!written_by_learn(&unit(&text)), "from what an older binary stored");
        println!("learn took {took:?}");
        if !cfg!(debug_assertions) {
            assert!(took < std::time::Duration::from_secs(10), "learn took {took:?}");
        }
    }

    #[test]
    fn a_calibrated_cut_lets_one_everyday_picture_in_a_thousand_through() {
        let bank = bank();
        assert!(bank.scales.len() > 5_000, "the bank is everyday traffic, not a handful");
        let margins_of = |vector: &[f32]| -> Vec<f32> {
            bank.dots(vector)
                .into_iter()
                .zip(&bank.background)
                .map(|(dot, background)| dot - background)
                .collect()
        };
        let stride = 4 + bank.width;
        let mut sum = vec![0.0f32; DIM];
        for (at, scale) in bank.scales.iter().enumerate() {
            let row = &bank.rows[at * stride + 4..(at + 1) * stride];
            for (total, q) in sum.iter_mut().zip(row) {
                *total += f32::from(*q as i8) * scale;
            }
        }
        let mean = unit(&sum);
        let mut axis = vec![0.0; DIM];
        axis[7] = 1.0;
        for vector in [&mean, &axis] {
            let cut = bank_cut(vector, BANK_RATE);
            let over = margins_of(vector).iter().filter(|m| **m >= cut).count();
            assert!(
                (over as f64) <= bank.scales.len() as f64 * BANK_RATE,
                "{over} everyday pictures clear a cut of {cut}"
            );
        }
        assert!(bank_cut(&mean, BANK_RATE) > bank_cut(&axis, BANK_RATE) + 0.1);
        assert_eq!(cut_for(42, &axis, 1), bank_cut(&axis, BANK_RATE));
        assert_eq!(cut_for(42, &axis, 1), bank_cut(&axis, BANK_RATE), "the memo answers the same");
        assert!(cut_for(42, &axis, 0) >= cut_for(42, &axis, 1), "a phrase is held to the stricter rate");
    }

    #[test]
    fn example_command_rejects_known_animations_before_enrollment() {
        let media = |mime: &str, name: Option<&str>| {
            Media::Document(Document::from_raw_media(
                grammers_client::tl::types::MessageMediaDocument {
                    nopremium: false,
                    spoiler: false,
                    video: false,
                    round: false,
                    voice: false,
                    document: Some(
                        grammers_client::tl::types::Document {
                            id: 42,
                            access_hash: 7,
                            file_reference: vec![1],
                            date: 0,
                            mime_type: mime.to_owned(),
                            size: 128,
                            thumbs: None,
                            video_thumbs: None,
                            dc_id: 1,
                            attributes: name
                                .map(|file_name| {
                                    grammers_client::tl::types::DocumentAttributeFilename {
                                        file_name: file_name.to_owned(),
                                    }
                                    .into()
                                })
                                .into_iter()
                                .collect(),
                        }
                        .into(),
                    ),
                    alt_documents: None,
                    video_cover: None,
                    video_timestamp: None,
                    ttl_seconds: None,
                },
            ))
        };
        for mime in ["image/png", "image/jpeg", "image/webp"] {
            assert!(matches!(
                example_image(&media(mime, None)),
                Ok(Some(ExampleImage::Original(_)))
            ));
        }
        for mime in ["image/png", "image/webp"] {
            assert!(thumbnail_raster_document(&media(mime, None)).is_some());
        }
        assert!(thumbnail_raster_document(&media("image/jpeg", None)).is_none());
        assert!(thumbnail_raster_document(&media("application/pdf", None)).is_none());
        let Media::Document(mut thumbed_png) = media("image/png", None) else {
            unreachable!()
        };
        if let Some(grammers_client::tl::enums::Document::Document(raw)) =
            thumbed_png.raw.document.as_mut()
        {
            raw.thumbs = Some(vec![
                grammers_client::tl::types::PhotoSize {
                    r#type: "x".into(),
                    w: 320,
                    h: 240,
                    size: 1024,
                }
                .into(),
            ]);
        }
        let thumbed_png = Media::Document(thumbed_png);
        assert!(matches!(
            example_image(&thumbed_png),
            Ok(Some(ExampleImage::Thumb(_)))
        ));
        assert!(thumbnail_raster_document(&thumbed_png).is_some());
        for mime in [
            "image/avif",
            "image/tiff",
            "image/x-icon",
            "image/x-portable-anymap",
        ] {
            assert!(matches!(
                example_image(&media(mime, None)),
                Ok(Some(ExampleImage::OriginalFrames(_)))
            ));
        }
        let Media::Document(mut thumbed_avif) = media("image/avif", None) else {
            unreachable!()
        };
        if let Some(grammers_client::tl::enums::Document::Document(raw)) =
            thumbed_avif.raw.document.as_mut()
        {
            raw.thumbs = Some(vec![
                grammers_client::tl::types::PhotoSize {
                    r#type: "x".into(),
                    w: 320,
                    h: 240,
                    size: 1024,
                }
                .into(),
            ]);
        }
        assert!(matches!(
            example_image(&Media::Document(thumbed_avif)),
            Ok(Some(ExampleImage::OriginalFrames(_)))
        ));
        assert!(
            thumbnail_raster_document(&media("application/octet-stream", Some("a.PNG"))).is_some()
        );
        assert!(
            thumbnail_raster_document(&media("application/octet-stream", Some("a.WEBP"))).is_some()
        );
        assert_eq!(verify_example_raster(Some(false)), Ok(()));
        assert_eq!(
            verify_example_raster(Some(true)),
            Err(ANIMATED_EXAMPLE_INSTRUCTION)
        );
        assert_eq!(
            verify_example_raster(None),
            Err(UNVERIFIED_EXAMPLE_INSTRUCTION)
        );
        for mime in [
            "image/gif",
            "image/apng",
            "image/avif-sequence",
            "video/mp4",
            "application/x-tgsticker",
        ] {
            assert_eq!(
                example_image(&media(mime, None)).err(),
                Some(ANIMATED_EXAMPLE_INSTRUCTION),
                "{mime} must not reach filter saving through its cover"
            );
        }
        for name in ["clip.GIF", "clip.APNG", "clip.WEBM", "clip.TGS"] {
            assert_eq!(
                example_image(&media("application/octet-stream", Some(name))).err(),
                Some(ANIMATED_EXAMPLE_INSTRUCTION)
            );
        }
        let Media::Document(mut animated_webp) = media("image/webp", None) else {
            unreachable!()
        };
        if let Some(grammers_client::tl::enums::Document::Document(raw)) =
            animated_webp.raw.document.as_mut()
        {
            raw.attributes
                .push(grammers_client::tl::types::DocumentAttributeAnimated {}.into());
        }
        assert_eq!(
            example_image(&Media::Document(animated_webp)).err(),
            Some(ANIMATED_EXAMPLE_INSTRUCTION)
        );
        let Media::Document(mut sticker_document) = media("video/webm", None) else {
            unreachable!()
        };
        if let Some(grammers_client::tl::enums::Document::Document(raw)) =
            sticker_document.raw.document.as_mut()
        {
            raw.attributes.push(
                grammers_client::tl::types::DocumentAttributeSticker {
                    mask: false,
                    alt: String::new(),
                    stickerset: grammers_client::tl::enums::InputStickerSet::Empty,
                    mask_coords: None,
                }
                .into(),
            );
        }
        let sticker = grammers_client::media::Sticker::from_document(&sticker_document).unwrap();
        assert_eq!(
            example_image(&Media::Sticker(sticker)).err(),
            Some(ANIMATED_EXAMPLE_INSTRUCTION)
        );
        assert!(matches!(
            example_image(&media("application/pdf", None)),
            Ok(None)
        ));
    }

    #[tokio::test]
    async fn original_example_requires_one_complete_image() {
        use tiff::encoder::{TiffEncoder, colortype};

        let still = include_bytes!("../../tests/fixtures/media/opaque-still.avif");
        let animation = include_bytes!("../../tests/fixtures/media/opaque-animated.avif");
        assert!(decode_original_example(still.to_vec()).await.is_ok());
        assert_eq!(
            decode_original_example(animation.to_vec()).await.err(),
            Some(ANIMATED_EXAMPLE_INSTRUCTION)
        );

        let mut single_page = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = TiffEncoder::new(&mut single_page).unwrap();
            encoder
                .new_image::<colortype::RGB8>(2, 2)
                .unwrap()
                .write_data(&[255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0])
                .unwrap();
        }
        assert!(
            decode_original_example(single_page.into_inner())
                .await
                .is_ok()
        );

        let mut two_pages = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = TiffEncoder::new(&mut two_pages).unwrap();
            for color in [[255, 0, 0], [0, 0, 255]] {
                encoder
                    .new_image::<colortype::RGB8>(2, 2)
                    .unwrap()
                    .write_data(&color.repeat(4))
                    .unwrap();
            }
        }
        assert_eq!(
            decode_original_example(two_pages.into_inner()).await.err(),
            Some(ANIMATED_EXAMPLE_INSTRUCTION)
        );
    }

    #[test]
    fn parses_every_form() {
        assert_eq!(
            parse("فیلتر متنی خودرو لوکس"),
            Some(Ask::FromText("خودرو لوکس"))
        );
        assert_eq!(parse("قفل تصویر اسب"), Some(Ask::FromPhoto("اسب")));
        assert_eq!(parse("فیلتر این ورزشی"), Some(Ask::FromExample("ورزشی")));
        assert_eq!(parse("قفل چهره علی"), Some(Ask::FromFace("علی")));
        assert_eq!(parse("/LockFace Ali"), Some(Ask::FromFace("Ali")));
        assert_eq!(parse("فیلتر نمونه ورزشی"), Some(Ask::FromExample("ورزشی")));
        assert_eq!(parse("حذف فیلتر تصویری ورزشی"), Some(Ask::Remove("ورزشی")));
    }

    #[test]
    fn image_filter_aliases_preserve_names_and_do_not_capture_other_filters() {
        assert_eq!(
            parse("فيلتر\u{200c}متني Cars كيان ۱۲"),
            Some(Ask::FromText("Cars كيان ۱۲"))
        );
        assert_eq!(parse("فیلتر عکس Cars"), Some(Ask::FromPhoto("Cars")));
        assert_eq!(parse("/ImageFilter Cars"), Some(Ask::FromText("Cars")));
        assert_eq!(parse("/LockImage Cars"), Some(Ask::FromPhoto("Cars")));
        assert_eq!(parse("/FilterImage Cars"), Some(Ask::FromExample("Cars")));
        assert_eq!(parse("/UnfilterImage Cars"), Some(Ask::Remove("Cars")));
        assert_eq!(parse("حذف فیلتر تصویر Cars"), Some(Ask::Remove("Cars")));
        assert_eq!(parse("فیلتر ویس Cars"), None);
        assert_eq!(parse("/filter Cars"), None);
        assert_eq!(parse("فیلتر عکسها Cars"), None);
        assert_eq!(parse("/lockimages Cars"), None);
    }

    #[test]
    fn tuning_suffixes_fold_syntax_and_leave_filter_names_untouched() {
        assert_eq!(
            parse("فیلتر تصویری My كيان\u{200c}۱۲\u{200c}روشن"),
            Some(Ask::Live("My كيان\u{200c}۱۲", true))
        );
        assert_eq!(
            parse("فیلتر تصویری My كيان\u{200c}۱۲\tخاموش"),
            Some(Ask::Live("My كيان\u{200c}۱۲", false))
        );
        assert_eq!(parse("فیلتر تصویری My كيان ۱۲ حساسيت ۵۰۰"), None);
        assert_eq!(parse_tune("My Name حساسیت 500 please"), None);
        assert_eq!(parse_tune("My Name فعالی"), None);
        assert_eq!(parse_tune("فعال"), None);
        assert_eq!(parse("فیلتر My Name فعال"), None);
    }

    #[test]
    fn declines_what_is_not_a_command() {
        assert_eq!(parse("فیلتر شکن"), None);
        assert_eq!(parse("فیلترها"), None);
        assert_eq!(parse("فیلتر متنی"), None);
        assert_eq!(parse("قفل تصویر"), None);
        assert_eq!(parse("قفل تصویری"), None);
        assert_eq!(parse("فیلتر این"), None);
        assert_eq!(parse("قفل چهره"), None);
        assert_eq!(parse("کالیبره فیلتر ورزشی"), None);
        assert_eq!(parse("تست فیلتر"), None);
        assert_eq!(parse("فیلتر ورزشی زنده"), None);
        assert_eq!(parse("فیلتر ورزشی حساسیت ۵۰۰"), None);
        assert_eq!(parse("سلام"), None);
        assert_eq!(parse("فیلتر ورزشی چطوره"), None);
    }

    #[test]
    fn the_only_alias_that_needs_dispatch_order_is_the_documented_one() {
        let ours = [TEXT_ADD, PHOTO_ADD, FACE_ADD, SAMPLE_ADD, REMOVE, CALIBRATE, TUNE].concat();
        let theirs: Vec<&str> =
            [super::super::filters::ADD, super::super::filters::REMOVE].concat();
        let mut needs_order: Vec<&str> = Vec::new();
        for alias in &ours {
            for other in &theirs {
                if super::super::commands::strip_prefix(alias, other)
                    .is_some_and(|rest| !rest.is_empty())
                    && super::super::phrase_carries_text(other)
                {
                    needs_order.push(*alias);
                }
            }
        }
        needs_order.sort_unstable();
        needs_order.dedup();
        assert_eq!(
            needs_order,
            vec!["حذف فیلتر تصویر", "حذف فیلتر تصویری", "حذف فیلتر عکس"],
            "a new alias collides with the word filter and dispatch order has to be checked"
        );
    }

    #[test]
    fn the_normal_quantile_is_right() {
        for (p, expected) in [
            (0.9, 1.281_552),
            (0.975, 1.959_964),
            (0.99, 2.326_348),
            (0.999, 3.090_232),
            (0.999_9, 3.719_016),
            (0.5, 0.0),
        ] {
            assert!(
                (z_for(p) - expected).abs() < 1e-4,
                "z({p}) came out {} not {expected}",
                z_for(p)
            );
        }
    }

    #[test]
    fn the_cut_lands_where_the_rate_asks() {
        let mut vector = vec![0f32; DIM];
        vector[0] = 1.0;
        let mut samples: Vec<Box<[f32]>> = Vec::new();
        for i in 0..MIN_SAMPLES {
            let mut s = vec![0f32; DIM];
            s[0] = (i as f32 / (MIN_SAMPLES - 1) as f32 - 0.5) * 0.2;
            samples.push(s.into_boxed_slice());
        }
        let found = calibrate(&vector, &samples, 1_000).expect("enough samples");
        assert_eq!(found.samples, MIN_SAMPLES);
        assert!(found.mean.abs() < 1e-4, "mean came out {}", found.mean);
        let expected_sd = (0.2 / 12f32.sqrt()) * (1.0 - vectors::BACKGROUND[0]);
        assert!(
            (found.sd - expected_sd).abs() < 2e-3,
            "sd came out {} not {expected_sd}",
            found.sd
        );
        let n = MIN_SAMPLES as f32;
        let z = 3.090_232_f32;
        let t = z + (z * z * z + z) / (4.0 * (n - 1.0));
        let expected = t * (1.0 + 1.0 / n).sqrt();
        assert!(
            (found.cut - expected * found.sd).abs() < 1e-3,
            "the cut is not the 1 in 1000 prediction point: {} against {}",
            found.cut,
            expected * found.sd
        );
        assert!(
            expected > z,
            "the finite sample correction must widen, not narrow"
        );

        let mut many: Vec<Box<[f32]>> = Vec::new();
        for i in 0..1024 {
            let mut s = vec![0f32; DIM];
            s[0] = (i as f32 / 1023.0 - 0.5) * 0.2;
            many.push(s.into_boxed_slice());
        }
        let full = calibrate(&vector, &many, 1_000).expect("a full reservoir");
        assert!(
            (full.cut / full.sd - z).abs() < z * 1e-2,
            "a full reservoir should sit on the bare quantile, not {}",
            full.cut / full.sd
        );

        let strict = calibrate(&vector, &samples, 10_000).expect("enough samples");
        assert!(strict.cut > found.cut);
    }

    #[test]
    fn calibration_refuses_a_sample_too_small_to_carry_a_tail() {
        let vector = vec![0.1f32; DIM];
        let samples: Vec<Box<[f32]>> = (0..MIN_SAMPLES - 1)
            .map(|_| vec![0.01f32; DIM].into_boxed_slice())
            .collect();
        assert!(calibrate(&vector, &samples, 1_000).is_none());
    }

    #[test]
    fn a_flat_sample_still_gets_a_cut_above_the_mean() {
        let mut vector = vec![0f32; DIM];
        vector[0] = 1.0;
        let samples: Vec<Box<[f32]>> = (0..MIN_SAMPLES)
            .map(|_| vec![0f32; DIM].into_boxed_slice())
            .collect();
        let found = calibrate(&vector, &samples, 1_000).expect("enough samples");
        assert!(found.cut > found.mean);
    }

    #[test]
    fn quantisation_does_not_move_a_margin() {
        let raw: Vec<f32> = (0..DIM).map(|i| ((i % 17) as f32 - 8.0) / 100.0).collect();
        let vector = unit(&raw);
        let (bytes, scale) = quantize(&vector);
        assert_eq!(bytes.len(), DIM);
        let back = dequantize(&bytes, scale);

        let picture = unit(
            &(0..DIM)
                .map(|i| ((i % 13) as f32 - 6.0) / 100.0)
                .collect::<Vec<_>>(),
        );
        let before = margin(&picture, &vector);
        let after = margin(&picture, &back);
        assert!(
            (before - after).abs() < 1e-3,
            "quantisation moved the margin by {}",
            (before - after).abs()
        );
    }

    #[test]
    fn a_fingerprint_follows_the_direction_and_not_the_chat() {
        let a = unit(&(0..DIM).map(|i| (i % 7) as f32).collect::<Vec<_>>());
        let b = unit(&(0..DIM).map(|i| (i % 11) as f32).collect::<Vec<_>>());
        let (a_bytes, a_scale) = quantize(&a);
        let (b_bytes, b_scale) = quantize(&b);
        assert_eq!(
            fingerprint(&a_bytes, a_scale),
            fingerprint(&a_bytes, a_scale),
            "the same direction has to give the same number in every chat"
        );
        assert_ne!(
            fingerprint(&a_bytes, a_scale),
            fingerprint(&b_bytes, b_scale)
        );
    }

    #[test]
    fn a_smaller_sample_earns_a_higher_cut() {
        let mut direction = vec![0f32; DIM];
        direction[3] = 1.0;
        let direction = unit(&direction);

        let draw = |count: usize| -> Vec<Box<[f32]>> {
            (0..count)
                .map(|i| {
                    let mut s = vec![0f32; DIM];
                    s[3] = (i as f32 / (count - 1) as f32 - 0.5) * 0.02;
                    s.into_boxed_slice()
                })
                .collect()
        };

        let small = calibrate(&direction, &draw(MIN_SAMPLES), DEFAULT_RATE).expect("at the floor");
        let large = calibrate(&direction, &draw(1024), DEFAULT_RATE).expect("a full reservoir");

        assert!(
            small.cut > large.cut,
            "a {} sample cut of {} must be more careful than a 1024 sample cut of {}",
            MIN_SAMPLES,
            small.cut,
            large.cut
        );
        assert!(
            small.cut < large.cut * 1.5,
            "the small sample correction ran away: {} against {}",
            small.cut,
            large.cut
        );
    }

    #[test]
    fn the_floor_is_where_calibration_starts() {
        let mut direction = vec![0f32; DIM];
        direction[5] = 1.0;
        let direction = unit(&direction);
        let sample = |count: usize| -> Vec<Box<[f32]>> {
            (0..count)
                .map(|i| {
                    let mut s = vec![0f32; DIM];
                    s[5] = i as f32 / 1000.0;
                    s.into_boxed_slice()
                })
                .collect()
        };
        assert!(calibrate(&direction, &sample(MIN_SAMPLES - 1), DEFAULT_RATE).is_none());
        assert!(calibrate(&direction, &sample(MIN_SAMPLES), DEFAULT_RATE).is_some());
    }

    #[test]
    fn the_stored_reservoir_comes_back_as_the_same_margins() {
        let mut direction = vec![0f32; DIM];
        direction[7] = 1.0;
        let direction = unit(&direction);

        let samples: Vec<Vec<f32>> = (0..16)
            .map(|i| {
                let raw: Vec<f32> = (0..DIM)
                    .map(|d| {
                        let x = ((d * 37 + i * 101) % 211) as f32 / 211.0 - 0.5;
                        x + 0.01 * i as f32
                    })
                    .collect();
                unit(&raw)
            })
            .collect();

        let flat: Vec<f32> = samples.iter().flat_map(|s| s.iter().copied()).collect();
        let (bytes, scale) = quantize(&flat);
        let back = dequantize(&bytes, scale);
        assert_eq!(back.len(), flat.len(), "the blob lost or gained a sample");

        for (at, chunk) in back.as_chunks::<DIM>().0.iter().enumerate() {
            let before = margin(&samples[at], &direction);
            let after = margin(chunk, &direction);
            assert!(
                (before - after).abs() < 1e-3,
                "sample {at} moved from {before} to {after} across the store"
            );
        }
    }

    #[test]
    fn every_filter_is_usable_without_calibration() {
        let phrase = uncalibrated_cut(0);
        assert!(
            phrase.is_finite(),
            "a phrase must be armable with no reservoir"
        );
        assert_eq!(phrase, FIXED_MODEL_CUT);

        for samples in [1u32, 2, MAX_SAMPLES] {
            assert_eq!(uncalibrated_cut(samples), FIXED_EXAMPLE_CUT);
        }
    }

    #[test]
    fn calibration_replaces_the_fallback_rather_than_bounding_it() {
        let mut vector = vec![0f32; DIM];
        vector[0] = 1.0;
        let samples: Vec<Box<[f32]>> = (0..MIN_SAMPLES)
            .map(|i| {
                let mut s = vec![0f32; DIM];
                s[0] = i as f32 / MIN_SAMPLES as f32 * 0.01;
                s.into_boxed_slice()
            })
            .collect();
        let found = calibrate(&vector, &samples, DEFAULT_RATE).expect("enough samples");
        assert!(found.cut.is_finite());
        assert!(
            found.cut != uncalibrated_cut(0),
            "a measured cut that happened to equal the shipped one would hide a bug"
        );
    }

    #[test]
    fn the_quick_model_path_uses_the_high_precision_fixed_cut() {
        assert_eq!(FIXED_MODEL_CUT, 0.038);
        const { assert!(FIXED_EXAMPLE_CUT > FIXED_MODEL_CUT) };
    }

    #[test]
    fn the_winner_is_measured_against_its_own_cut() {
        let phrase = Filter {
            name: "متنی".to_owned(),
            vector: vec![0.0; DIM],
            cut: 0.02,
            live: true,
            print: 1,
            faces: None,
            person: None,
            gate: None,
            subject: None,
        };
        let prototype = Filter {
            name: "نمونه".to_owned(),
            vector: vec![0.0; DIM],
            cut: 0.40,
            live: true,
            print: 2,
            faces: None,
            person: None,
            gate: None,
            subject: None,
        };
        let filters = vec![phrase, prototype];
        let (won, _) = worst(&filters, &[0.05, 0.30], true).expect("one of them is over");
        assert_eq!(won.name, "متنی");

        assert!(worst(&filters, &[0.01, 0.30], true).is_none());
    }

    #[test]
    fn shadow_match_does_not_mask_a_live_match() {
        let filters = vec![
            Filter {
                name: "live".to_owned(),
                vector: vec![],
                cut: 0.02,
                live: true,
                print: 1,
                faces: None,
                person: None,
                gate: None,
                subject: None,
            },
            Filter {
                name: "shadow".to_owned(),
                vector: vec![],
                cut: 0.02,
                live: false,
                print: 2,
                faces: None,
                person: None,
                gate: None,
                subject: None,
            },
        ];
        let margins = [0.04, 0.30];
        assert_eq!(worst(&filters, &margins, false).unwrap().0.name, "shadow");
        assert_eq!(worst(&filters, &margins, true).unwrap().0.name, "live");
        assert!(worst(&filters, &[0.01, 0.30], true).is_none());
    }

    #[test]
    fn a_name_that_would_break_a_key_or_a_message_is_refused() {
        assert!(acceptable("ورزشی"));
        assert!(!acceptable(""));
        assert!(!acceptable("a:b"));
        assert!(!acceptable("a=b"));
        assert!(!acceptable("<b>"));
        assert!(!acceptable("bad\nname"));
        assert!(!acceptable(&"ط".repeat(MAX_NAME + 1)));
    }

    #[test]
    fn a_phrase_is_learned_from_what_the_table_says_it_is() {
        use super::super::imgtext::Frames;
        assert_eq!(
            phrase_source("سگ"),
            Some(("a dog".to_owned(), Frames::English))
        );
        assert_eq!(phrase_source("کص"), None, "the NSFW lock's");
        assert_eq!(phrase_source("سیگار"), None, "the cigarette lock's");
        assert_eq!(phrase_source("خامنه ای"), None, "a face");
        assert_eq!(phrase_source("پابجی"), None, "a game");
        assert_eq!(
            phrase_source("قلعه"),
            Some(("castle".to_owned(), Frames::English)),
            "the generated lexicon's"
        );
        assert_eq!(
            phrase_source("زن بی حجاب"),
            Some(("زن بی حجاب".to_owned(), Frames::Typed))
        );
    }

    #[test]
    fn fitted_filters_are_well_formed() {
        let englishes = super::super::imgsubject::visual_englishes();
        assert!(fitted().len() > 300, "imgfilter_concepts.bin is missing or truncated");
        for key in fitted().keys() {
            assert!(englishes.contains(key), "«{key}» is not a table's English");
            let filter = fitted_filter(key).expect("listed");
            assert!(written_by_learn(&filter.vector), "«{key}» would be re-learned at every start");
            assert!(filter.cut.is_finite() && filter.cut < NEVER);
        }
        assert!(fitted_filter("a dog").is_some(), "the commonest subjects are fitted");
    }

    #[test]
    #[ignore = "needs vision_text.onnx and its two files from an export"]
    fn understanding_matches_the_measurement() {
        use super::super::imgtext::{Frames, embed_framed};
        super::super::nsfw::configure_model_tests()
            .expect("configure the test's VISION_FILES bundle before loading the text tower");
        for (phrase, tail, regional, ok) in [
            ("قلعه", 5.924, 0.07, true),
            ("لگو", 8.458, 0.00, true),
            ("گوه", 3.775, 0.94, false),
            ("سپاه", 5.458, 0.91, false),
        ] {
            let text = embed_framed(phrase, Frames::Typed).expect("the text tower beside the test");
            let found = understanding(&text);
            assert!(
                (found.tail - tail).abs() < 0.05,
                "«{phrase}» tail {} against {tail}",
                found.tail
            );
            assert!(
                (found.regional - regional).abs() <= 0.021,
                "«{phrase}» regional {} against {regional}",
                found.regional
            );
            assert_eq!(found.ok(), ok, "«{phrase}»");
        }
    }

    #[test]
    fn the_strings_carry_no_half_space() {
        for text in ["فیلتر تصویری", "بررسی بدون حذف", "هنوز واسنجی نشده"]
        {
            assert!(!text.contains('\u{200c}'));
        }
    }
}
