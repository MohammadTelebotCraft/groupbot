use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use image::RgbImage;

use super::nsfw::{self, ModelError};
use super::vision::beside_the_binary;

const VISION_FILE: &str = "verify_vision.onnx";
const TEXT_FILE: &str = "verify_text.onnx";
const PROMPTS_FILE: &str = "verify_prompts.bin";

pub const YES: f32 = 0.7;

pub const YES_UNTRAINED: f32 = 0.995;

const SIDE: usize = 384;
const PATCH: usize = 16;
const GRID: usize = SIDE / PATCH;
const PATCHES: usize = GRID * GRID;
const PATCH_VALUES: usize = 3 * 2 * PATCH * PATCH;
const MERGED: usize = GRID / 2;
const IMAGE_TOKENS: usize = MERGED * MERGED;
const HIDDEN: usize = 2048;
const HEAD: usize = 128;
const DEEPSTACK: usize = 3;
const THETA: f64 = 5_000_000.0;
const PREFIX: usize = 4;
const ANSWERS: usize = 10;

const THREADS: usize = 4;

fn threads() -> usize {
    std::env::var("VERIFY_THREADS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|threads| (1..=32).contains(threads))
        .unwrap_or(THREADS)
}

const IDLE: Duration = Duration::from_secs(600);
const QUEUE: Duration = Duration::from_secs(60);
const RUN: Duration = Duration::from_secs(120);

struct Prompts {
    scales: Vec<f32>,
    rows: Vec<i8>,
    prefix: Vec<u32>,
    subjects: HashMap<String, Vec<u32>>,
}

impl Prompts {
    fn parse(bytes: &[u8]) -> Result<Prompts, ModelError> {
        let bad = |what: &str| ModelError::contract(format!("{PROMPTS_FILE}: {what}"));
        let mut at = 0usize;
        let mut take = |count: usize| -> Result<&[u8], ModelError> {
            let slice = bytes.get(at..at + count).ok_or_else(|| bad("truncated"))?;
            at += count;
            Ok(slice)
        };
        if take(4)? != b"VPR1" {
            return Err(bad("not VPR1"));
        }
        let word = |slice: &[u8]| u32::from_le_bytes(slice.try_into().expect("four bytes"));
        let count = word(take(4)?) as usize;
        if word(take(4)?) as usize != HIDDEN {
            return Err(bad("rows are not the language model's width"));
        }
        let mut scales = Vec::with_capacity(count);
        let mut rows = Vec::with_capacity(count * HIDDEN);
        for _ in 0..count {
            scales.push(f32::from_le_bytes(take(4)?.try_into().expect("four bytes")));
            rows.extend(take(HIDDEN)?.iter().map(|&b| b as i8));
        }
        let index = |slice: &[u8]| -> Result<u32, ModelError> {
            let row = u32::from_le_bytes(slice.try_into().expect("four bytes"));
            if (row as usize) < count { Ok(row) } else { Err(bad("a row index past the table")) }
        };
        let length = word(take(4)?) as usize;
        let prefix = (0..length).map(|_| index(take(4)?)).collect::<Result<Vec<_>, _>>()?;
        if prefix.len() != PREFIX {
            return Err(bad("the prompt before the image is not the export's"));
        }
        let mut subjects = HashMap::new();
        for _ in 0..word(take(4)?) {
            let length = u16::from_le_bytes(take(2)?.try_into().expect("two bytes")) as usize;
            let key = std::str::from_utf8(take(length)?).map_err(|_| bad("a key is not UTF-8"))?.to_owned();
            let length = u16::from_le_bytes(take(2)?.try_into().expect("two bytes")) as usize;
            let after = (0..length).map(|_| index(take(4)?)).collect::<Result<Vec<_>, _>>()?;
            subjects.insert(key, after);
        }
        if at != bytes.len() {
            return Err(bad("trailing bytes"));
        }
        Ok(Prompts { scales, rows, prefix, subjects })
    }

    fn push_row(&self, row: u32, out: &mut Vec<f32>) {
        let row = row as usize;
        let scale = self.scales[row];
        out.extend(self.rows[row * HIDDEN..(row + 1) * HIDDEN].iter().map(|&q| f32::from(q) * scale));
    }
}

struct Model {
    vision: nsfw::Session,
    text: nsfw::Session,
    prompts: Prompts,
}

static LOADED: Mutex<Option<(Arc<Model>, Instant)>> = Mutex::new(None);
static REAPING: AtomicBool = AtomicBool::new(false);

fn paths() -> Result<Option<[std::path::PathBuf; 3]>, ModelError> {
    let (Some(vision), Some(text), Some(prompts)) = (
        beside_the_binary(VISION_FILE)?,
        beside_the_binary(TEXT_FILE)?,
        beside_the_binary(PROMPTS_FILE)?,
    ) else {
        return Ok(None);
    };
    Ok(Some([vision, text, prompts]))
}

pub fn installed() -> bool {
    matches!(paths(), Ok(Some(_)))
}

fn model() -> Result<Option<Arc<Model>>, ModelError> {
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((model, used)) = loaded.as_mut() {
        *used = Instant::now();
        return Ok(Some(Arc::clone(model)));
    }
    let Some([vision, text, prompts]) = paths()? else {
        return Ok(None);
    };
    let started = Instant::now();
    let bytes = std::fs::read(&prompts).map_err(|error| {
        ModelError::artifact(format!("could not read {}: {error}", prompts.display()))
    })?;
    let prompts = Prompts::parse(&bytes)?;
    let vision = nsfw::open_path_configured(
        &vision,
        "second-look vision tower",
        threads(),
        &[("session.disable_prepacking", "1")],
    )
        .ok_or_else(|| ModelError::Runtime("the second look's vision tower would not load".to_owned()))?;
    let text = nsfw::open_path_with(&text, "second-look language model", threads())
        .ok_or_else(|| ModelError::Runtime("the second look's language model would not load".to_owned()))?;
    let model = Arc::new(Model { vision, text, prompts });
    log::info!(
        "verify: loaded in {}ms, {} subjects",
        started.elapsed().as_millis(),
        model.prompts.subjects.len()
    );
    *loaded = Some((Arc::clone(&model), Instant::now()));
    if !REAPING.swap(true, Ordering::AcqRel) {
        std::thread::spawn(reap);
    }
    Ok(Some(model))
}

fn reap() {
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
        let idle = loaded.as_ref().is_none_or(|(_, used)| used.elapsed() >= IDLE);
        if idle {
            let dropped = loaded.take();
            REAPING.store(false, Ordering::Release);
            drop(loaded);
            if let Some(dropped) = dropped {
                drop(dropped);
                nsfw::trim::release_freed();
                log::info!("verify: weights dropped after {}s without a look", IDLE.as_secs());
            }
            return;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fit {
    Squashed,
    Letterboxed,
}

const BARS: u8 = 128;

fn fitted(image: &RgbImage, fit: Fit) -> RgbImage {
    let side = SIDE as u32;
    let filter = image::imageops::FilterType::Triangle;
    match fit {
        Fit::Squashed => image::imageops::resize(image, side, side, filter),
        Fit::Letterboxed => {
            let (width, height) = image.dimensions();
            let longest = width.max(height).max(1) as f32;
            let scaled = |length: u32| ((length as f32 * side as f32 / longest).round() as u32).clamp(1, side);
            let (width, height) = (scaled(width), scaled(height));
            let inner = image::imageops::resize(image, width, height, filter);
            let mut view = RgbImage::from_pixel(side, side, image::Rgb([BARS; 3]));
            image::imageops::replace(&mut view, &inner, i64::from((side - width) / 2), i64::from((side - height) / 2));
            view
        }
    }
}

fn patches(view: &RgbImage) -> Vec<f32> {
    debug_assert_eq!(view.dimensions(), (SIDE as u32, SIDE as u32));
    let raw = view.as_raw();
    let mut out = Vec::with_capacity(PATCHES * PATCH_VALUES);
    for token in 0..IMAGE_TOKENS {
        let (row, col) = (token / MERGED, token % MERGED);
        for inner in 0..4 {
            let (h, w) = (row * 2 + inner / 2, col * 2 + inner % 2);
            for channel in 0..3 {
                for _time in 0..2 {
                    for y in h * PATCH..(h + 1) * PATCH {
                        for x in w * PATCH..(w + 1) * PATCH {
                            out.push(f32::from(raw[(y * SIDE + x) * 3 + channel]) / 127.5 - 1.0);
                        }
                    }
                }
            }
        }
    }
    out
}

fn rope(length: usize) -> (Vec<f32>, Vec<f32>) {
    let mut cos = Vec::with_capacity(length * HEAD);
    let mut sin = Vec::with_capacity(length * HEAD);
    for at in 0..length {
        let (t, h, w) = if at < PREFIX {
            (at, at, at)
        } else if at < PREFIX + IMAGE_TOKENS {
            let token = at - PREFIX;
            (PREFIX, PREFIX + token / MERGED, PREFIX + token % MERGED)
        } else {
            let next = PREFIX + MERGED + (at - PREFIX - IMAGE_TOKENS);
            (next, next, next)
        };
        let angles: Vec<f64> = (0..HEAD / 2)
            .map(|k| {
                let position = match k % 3 {
                    1 if k < 60 => h,
                    2 if k < 60 => w,
                    _ => t,
                };
                position as f64 / THETA.powf(2.0 * k as f64 / HEAD as f64)
            })
            .collect();
        for _half in 0..2 {
            cos.extend(angles.iter().map(|a| a.cos() as f32));
            sin.extend(angles.iter().map(|a| a.sin() as f32));
        }
    }
    (cos, sin)
}

fn causal(length: usize) -> Vec<f32> {
    let mut mask = vec![0.0f32; length * length];
    for row in 0..length {
        for col in row + 1..length {
            mask[row * length + col] = f32::NEG_INFINITY;
        }
    }
    mask
}

fn answer(logits: &[f32]) -> f32 {
    let mass = |values: &[f32]| {
        let top = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        top + values.iter().map(|v| (v - top).exp()).sum::<f32>().ln()
    };
    let difference = mass(&logits[..ANSWERS / 2]) - mass(&logits[ANSWERS / 2..]);
    1.0 / (1.0 + (-difference).exp())
}

fn tensor(shape: Vec<i64>, values: Vec<f32>, what: &str) -> Result<ort::value::DynValue, ModelError> {
    ort::value::Value::from_array((shape, values))
        .map(|value| value.into_dyn())
        .map_err(|error| ModelError::contract(format!("second look {what}: {error}")))
}

fn floats(value: &ort::value::DynValue, what: &str, expected: usize) -> Result<Vec<f32>, ModelError> {
    let (_, values) = value
        .try_extract_tensor::<f32>()
        .map_err(|error| ModelError::contract(format!("second look {what} is not f32: {error}")))?;
    if values.len() != expected || values.iter().any(|v| !v.is_finite()) {
        return Err(ModelError::contract(format!(
            "second look {what}: {} values, expected {expected} finite",
            values.len()
        )));
    }
    Ok(values.to_vec())
}

struct Seen {
    merged: Vec<f32>,
    deepstack: Vec<f32>,
}

fn see(model: &Model, image: &RgbImage, fit: Fit) -> Result<Seen, ModelError> {
    let pixels = tensor(vec![PATCHES as i64, PATCH_VALUES as i64], patches(&fitted(image, fit)), "pixels")?;
    let mut session = model.vision.lock().map_err(|_| ModelError::Poisoned("second-look vision tower"))?;
    let output = session
        .run(ort::inputs![pixels])
        .map_err(|error| ModelError::Runtime(format!("second look vision pass: {error}")))?;
    let mut outputs = output.into_iter();
    let (Some((_, merged)), Some((_, deepstack)), None) = (outputs.next(), outputs.next(), outputs.next()) else {
        return Err(ModelError::contract("second look vision graph is not two outputs"));
    };
    Ok(Seen {
        merged: floats(&merged, "image tokens", IMAGE_TOKENS * HIDDEN)?,
        deepstack: floats(&deepstack, "deepstack", DEEPSTACK * IMAGE_TOKENS * HIDDEN)?,
    })
}

fn ask(model: &Model, seen: &Seen, subject: &str) -> Result<Option<f32>, ModelError> {
    let Some(after) = model.prompts.subjects.get(subject) else {
        return Ok(None);
    };
    let length = PREFIX + IMAGE_TOKENS + after.len();
    let mut embeds = Vec::with_capacity(length * HIDDEN);
    for &row in &model.prompts.prefix {
        model.prompts.push_row(row, &mut embeds);
    }
    embeds.extend_from_slice(&seen.merged);
    for &row in after {
        model.prompts.push_row(row, &mut embeds);
    }
    let (cos, sin) = rope(length);
    let length = length as i64;
    let inputs = [
        tensor(vec![1, length, HIDDEN as i64], embeds, "embeddings")?,
        tensor(vec![1, length, HEAD as i64], cos, "cos")?,
        tensor(vec![1, length, HEAD as i64], sin, "sin")?,
        tensor(vec![1, 1, length, length], causal(length as usize), "mask")?,
        tensor(
            vec![DEEPSTACK as i64, IMAGE_TOKENS as i64, HIDDEN as i64],
            seen.deepstack.clone(),
            "deepstack",
        )?,
    ];
    let mut session = model.text.lock().map_err(|_| ModelError::Poisoned("second-look language model"))?;
    let [embeds, cos, sin, mask, deepstack] = inputs;
    let output = session
        .run(ort::inputs![embeds, cos, sin, mask, deepstack])
        .map_err(|error| ModelError::Runtime(format!("second look language pass: {error}")))?;
    let (_, logits) = output
        .into_iter()
        .next()
        .ok_or_else(|| ModelError::contract("second look language graph has no output"))?;
    Ok(Some(answer(&floats(&logits, "answers", ANSWERS)?)))
}

#[cfg(test)]
pub fn p_yes(image: &RgbImage, subject: &str) -> Result<Option<f32>, ModelError> {
    let Some(model) = model()? else {
        return Ok(None);
    };
    if !model.prompts.subjects.contains_key(subject) {
        return Ok(None);
    }
    ask(&model, &see(&model, image, Fit::Squashed)?, subject)
}

pub type Asker<'a> = dyn FnMut(&str) -> Result<Option<f32>, ModelError> + 'a;

struct Judged {
    squashed: f32,
    letterboxed: Option<f32>,
}

impl Judged {
    fn answer(&self) -> f32 {
        self.letterboxed.map_or(self.squashed, |other| self.squashed.min(other))
    }
}

fn judged(
    image: &RgbImage,
    label: &str,
    bar: f32,
    decide: impl FnOnce(&mut Asker<'_>) -> Result<Option<f32>, ModelError>,
) -> Result<Option<Judged>, ModelError> {
    let Some(model) = model()? else {
        return Ok(None);
    };
    let mut seen: Option<Seen> = None;
    let mut asker = |subject: &str| -> Result<Option<f32>, ModelError> {
        if !model.prompts.subjects.contains_key(subject) {
            return Ok(None);
        }
        if seen.is_none() {
            seen = Some(see(&model, image, Fit::Squashed)?);
        }
        ask(&model, seen.as_ref().expect("seen just now"), subject)
    };
    let Some(squashed) = decide(&mut asker)? else {
        return Ok(None);
    };
    if squashed < bar {
        return Ok(Some(Judged { squashed, letterboxed: None }));
    }
    let letterboxed = ask(&model, &see(&model, image, Fit::Letterboxed)?, label)?;
    Ok(Some(Judged { squashed, letterboxed }))
}

pub fn print(subject: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in b"second-look-v1\0".iter().chain(subject.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

static TURN: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

pub async fn look(image: Arc<RgbImage>, subject: &'static str, bar: f32) -> Result<Option<f32>, ModelError> {
    judge(image, subject, bar, move |ask: &mut Asker<'_>| ask(subject)).await
}

pub async fn judge(
    image: Arc<RgbImage>,
    label: &'static str,
    bar: f32,
    decide: impl FnOnce(&mut Asker<'_>) -> Result<Option<f32>, ModelError> + Send + 'static,
) -> Result<Option<f32>, ModelError> {
    let permit = tokio::time::timeout(QUEUE, TURN.acquire())
        .await
        .map_err(|_| ModelError::Runtime("the second look's queue timed out".to_owned()))?
        .map_err(|_| ModelError::Runtime("the second look's queue closed".to_owned()))?;
    let started = Instant::now();
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        judged(&image, label, bar, decide)
    });
    let judged = tokio::time::timeout(RUN, worker)
        .await
        .map_err(|_| ModelError::Runtime("the second look took too long".to_owned()))?
        .map_err(|error| ModelError::Runtime(format!("the second look's worker failed: {error}")))??;
    if let Some(judged) = &judged {
        let elapsed = started.elapsed().as_millis();
        match judged.letterboxed {
            Some(other) => log::info!(
                "verify: «{label}» P(yes) {:.3}, letterboxed {other:.3}, so {:.3} in {elapsed}ms",
                judged.squashed,
                judged.answer()
            ),
            None => log::info!("verify: «{label}» P(yes) {:.3} in {elapsed}ms", judged.squashed),
        }
    }
    Ok(judged.map(|judged| judged.answer()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_fitted_two_ways() {
        let wide = RgbImage::from_pixel(800, 200, image::Rgb([255, 255, 255]));
        let squashed = fitted(&wide, Fit::Squashed);
        assert_eq!(squashed.dimensions(), (SIDE as u32, SIDE as u32));
        assert!(squashed.pixels().all(|p| p.0 == [255, 255, 255]));
        let boxed = fitted(&wide, Fit::Letterboxed);
        assert_eq!(boxed.dimensions(), (SIDE as u32, SIDE as u32));
        assert_eq!(boxed.get_pixel(10, 10).0, [BARS; 3]);
        assert_eq!(boxed.get_pixel(10, 143).0, [BARS; 3]);
        assert_eq!(boxed.get_pixel(10, 192).0, [255, 255, 255]);
        assert_eq!(boxed.get_pixel(10, 240).0, [BARS; 3]);
        assert!((f32::from(BARS) / 127.5 - 1.0).abs() < 0.01);
        let square = RgbImage::from_pixel(500, 500, image::Rgb([9, 9, 9]));
        assert_eq!(fitted(&square, Fit::Letterboxed), fitted(&square, Fit::Squashed));
    }

    #[test]
    fn patches_are_in_the_processors_order() {
        let image = RgbImage::from_fn(SIDE as u32, SIDE as u32, |x, y| image::Rgb([(x % 251) as u8, (y % 251) as u8, ((x + y) % 7) as u8]));
        let out = patches(&image);
        assert_eq!(out.len(), PATCHES * PATCH_VALUES);
        let value = |v: u32| v as f32 / 127.5 - 1.0;
        let row = 13 * 4 + 3;
        let (channel, time) = (1, 1);
        let at = row * PATCH_VALUES + (channel * 2 + time) * PATCH * PATCH + 5 * PATCH + 9;
        assert_eq!(out[at], value(3 * 16 + 5));
        let at = row * PATCH_VALUES + 5 * PATCH + 9;
        assert_eq!(out[at], value(3 * 16 + 9));
    }

    #[test]
    fn rope_turns_each_axis_where_the_export_does() {
        let (cos, sin) = rope(PREFIX + IMAGE_TOKENS + 3);
        let angle = |position: f64, k: usize| position / THETA.powf(2.0 * k as f64 / HEAD as f64);
        assert!((cos[2 * HEAD + 7] - angle(2.0, 7).cos() as f32).abs() < 1e-6);
        let token = PREFIX + 25;
        assert!((sin[token * HEAD + 1] - angle(6.0, 1).sin() as f32).abs() < 1e-6);
        assert!((sin[token * HEAD + 2] - angle(5.0, 2).sin() as f32).abs() < 1e-6);
        assert!((sin[token * HEAD + 3] - angle(4.0, 3).sin() as f32).abs() < 1e-6);
        assert!((sin[token * HEAD + 61] - angle(4.0, 61).sin() as f32).abs() < 1e-6);
        assert_eq!(sin[token * HEAD + 64 + 1], sin[token * HEAD + 1]);
        let after = PREFIX + IMAGE_TOKENS;
        assert!((cos[after * HEAD + 1] - angle(16.0, 1).cos() as f32).abs() < 1e-6);
    }

    #[test]
    fn the_answer_is_the_yes_mass_against_the_no_mass() {
        assert!((answer(&[0.0; ANSWERS]) - 0.5).abs() < 1e-6);
        let mut logits = [-20.0f32; ANSWERS];
        logits[1] = 5.0;
        assert!(answer(&logits) > 0.999);
        logits[7] = 5.0;
        assert!((answer(&logits) - 0.5).abs() < 1e-3);
        let mask = causal(3);
        assert_eq!(mask[1], f32::NEG_INFINITY);
        assert_eq!(mask[3], 0.0);
    }

    #[test]
    fn prompts_parse_and_refuse_what_is_not_them() {
        let mut blob = b"VPR1".to_vec();
        blob.extend(2u32.to_le_bytes());
        blob.extend((HIDDEN as u32).to_le_bytes());
        for scale in [0.5f32, 2.0] {
            blob.extend(scale.to_le_bytes());
            blob.extend(std::iter::repeat_n(2u8, HIDDEN));
        }
        blob.extend((PREFIX as u32).to_le_bytes());
        for row in [0u32, 1, 0, 1] {
            blob.extend(row.to_le_bytes());
        }
        blob.extend(1u32.to_le_bytes());
        blob.extend(3u16.to_le_bytes());
        blob.extend(b"dog");
        blob.extend(2u16.to_le_bytes());
        blob.extend(1u32.to_le_bytes());
        blob.extend(0u32.to_le_bytes());
        let prompts = Prompts::parse(&blob).expect("a well-formed table");
        assert_eq!(prompts.subjects["dog"], vec![1, 0]);
        let mut row = Vec::new();
        prompts.push_row(1, &mut row);
        assert_eq!(row.len(), HIDDEN);
        assert_eq!(row[0], 4.0);
        assert!(Prompts::parse(&blob[..blob.len() - 1]).is_err(), "truncated");
        let mut wrong = blob.clone();
        wrong[0] = b'X';
        assert!(Prompts::parse(&wrong).is_err(), "another format");
        let mut past = blob.clone();
        let at = past.len() - 4;
        past[at..].copy_from_slice(&9u32.to_le_bytes());
        assert!(Prompts::parse(&past).is_err(), "a row past the table");
    }

    #[test]
    #[ignore = "needs the second look's files under VISION_FILES, VERIFY_LIST and VERIFY_OUT"]
    fn answers_for_a_list() {
        nsfw::configure_model_tests().expect("model test configuration");
        let (Ok(list), Ok(out)) = (std::env::var("VERIFY_LIST"), std::env::var("VERIFY_OUT")) else {
            return;
        };
        let model = model().expect("the second look loads").expect("the second look is installed");
        let shown = |p: Option<f32>| p.map_or("none".to_owned(), |p| format!("{p:.4}"));
        let mut lines = Vec::new();
        let mut partial = std::fs::File::create(format!("{out}.partial")).expect("VERIFY_OUT.partial");
        for line in std::fs::read_to_string(list).expect("VERIFY_LIST").lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            let [subject, path, more @ ..] = fields.as_slice() else {
                continue;
            };
            let image = image::open(path).expect("a picture").to_rgb8();
            let squashed = see(&model, &image, Fit::Squashed).expect("a look");
            let mut answers = Vec::new();
            for question in std::iter::once(subject).chain(more.iter()) {
                answers.push(shown(ask(&model, &squashed, question).expect("an answer")));
            }
            let letterboxed = see(&model, &image, Fit::Letterboxed).expect("a look");
            answers.push(shown(ask(&model, &letterboxed, subject).expect("an answer")));
            lines.push(format!("{line}\t{}", answers.join("\t")));
            use std::io::Write as _;
            writeln!(partial, "{}", lines.last().expect("just pushed")).expect("VERIFY_OUT.partial");
        }
        std::fs::write(out, lines.join("\n") + "\n").expect("VERIFY_OUT");
        println!("{} lines", lines.len());
    }

    #[test]
    #[ignore = "needs verify_*.onnx and verify_prompts.bin under VISION_FILES"]
    fn the_installed_second_look_answers_like_the_export() {
        nsfw::configure_model_tests().expect("model test configuration");
        let Ok(path) = std::env::var("VERIFY_PICTURE") else {
            return;
        };
        let image = image::open(path).expect("a picture").to_rgb8();
        let started = Instant::now();
        let yes = p_yes(&image, "harp").expect("a look").expect("installed, and the prompts know a harp");
        let loaded = started.elapsed();
        let started = Instant::now();
        let no = p_yes(&image, "dishwasher").expect("a look").expect("and a dishwasher");
        println!(
            "harp {yes:.4}, dishwasher {no:.4}; first look with the load {}ms, the next {}ms at {} threads",
            loaded.as_millis(),
            started.elapsed().as_millis(),
            threads()
        );
        assert!(yes >= YES && no < YES);
    }
}
