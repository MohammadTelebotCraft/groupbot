use std::io::Cursor;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use grammers_client::media::{Document, Downloadable, Media, PhotoSize};
use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerRef};
use image::AnimationDecoder;
use tokio::process::Command;

use super::Ctx;
use super::locks::View;

pub const LOCK: &str = "nsfw";

const DEFAULT_LIMIT: u32 = 50;

const LIMIT_RANGE: (u32, u32) = (10, 95);
#[cfg(test)]
const LIMIT_PRESETS: &[u32] = &[30, 40, 50, 60, 70, 80];

const MODEL: &[u8] = include_bytes!("../../assets/nsfw.onnx");

const SIDE: usize = 384;

const RESIZE: usize = 384;

const CLASSES: [&str; 2] = ["nsfw", "sfw"];

const CORROBORATE: f32 = 0.30;

const WIDE: f32 = 1.5;

const DETAIL_FLOOR: u32 = RESIZE as u32;

const SHARPNESS_FLOOR: f32 = 100.0;

fn undecided(score: f32) -> bool {
    score > GRADE_ABOVE && score < CONFIDENT
}

const GRADE: &[u8] = include_bytes!("../../assets/nsfw_grade.onnx");

const GRADE_SIDE: usize = 224;
const GRADE_RESIZE: usize = 256;
const GRADE_CLASSES: [&str; 5] = ["drawings", "hentai", "neutral", "porn", "sexy"];

const EXPLICIT_FLOOR: f32 = 0.20;

const BRIDGE_SCORE: f32 = 0.70;
const BRIDGE_FLOOR: f32 = 0.10;

const STRONG_BRIDGE_SCORE: f32 = 0.50;
const STRONG_BRIDGE_FLOOR: f32 = 0.75;

const NEUTRAL_FLOOR: f32 = 0.97;

const GRADED_CONFIDENT: f32 = 0.92;
const CONFIDENT: f32 = 0.95;

const SUGGESTIVE_FLOOR: f32 = 0.50;

const GRADE_ABOVE: f32 = LIMIT_RANGE.0 as f32 / 100.0;

const AGREE: f32 = 0.020;

const RECOVER: f32 = 0.045;

const HEAD_VETO: f32 = 0.20;

const HEAD_SURE: f32 = 0.95;

const HEAD_DELETE: f32 = 0.95;

#[derive(Clone, Copy, Debug)]
pub struct Arbiter {
    pub explicit: f32,
    pub revealing: f32,

    pub head: f32,
}

impl Arbiter {
    fn agrees(&self) -> bool {
        self.head_sure() || (self.explicit >= AGREE && self.head >= HEAD_VETO)
    }

    fn sure(&self) -> bool {
        self.explicit >= RECOVER || self.head_sure()
    }

    fn head_sure(&self) -> bool {
        self.head >= HEAD_SURE
    }

    fn head_deletes(&self) -> bool {
        self.head >= HEAD_DELETE
    }
}

pub fn arbiter_of(embedding: &[f32]) -> Arbiter {
    let ordinary = super::vision::dot(embedding, &super::concept_vectors::SAFE);
    Arbiter {
        explicit: super::vision::dot(embedding, &super::concept_vectors::EXPLICIT) - ordinary,
        revealing: super::vision::dot(embedding, &super::concept_vectors::SUGGESTIVE) - ordinary,
        head: super::nsfw_head::probability(embedding),
    }
}

const RUNTIME: &[u8] = include_bytes!("../../assets/libonnxruntime.so.1.20.1");

const DEFAULT_INFER_THREADS: usize = 2;
const DEFAULT_INFER_SESSIONS: usize = 2;

fn infer_threads() -> usize {
    std::env::var("NSFW_INFER_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_INFER_THREADS)
        .clamp(1, 64)
}

pub fn infer_sessions() -> usize {
    std::env::var("NSFW_INFER_SESSIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_INFER_SESSIONS)
        .clamp(1, 256)
}

pub type Session = std::sync::Mutex<ort::session::Session>;

pub struct SessionPool {
    sessions: Box<[Session]>,
    next: AtomicUsize,
}

impl SessionPool {
    pub fn with<T>(&self, run: impl FnOnce(&Session) -> T) -> T {
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.sessions.len();
        run(&self.sessions[index])
    }
}

fn runtime_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("ORT_LIBRARY_PATH") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
        eprintln!(
            "nsfw: ORT_LIBRARY_PATH does not point to a runtime: {}",
            path.display()
        );
    }
    let mut path = std::env::current_exe().ok()?;
    path.pop();
    path.push("libonnxruntime.so.1.20.1");
    let present = std::fs::metadata(&path).is_ok_and(|meta| meta.len() as usize == RUNTIME.len());
    if !present {
        let staging = path.with_extension("so.partial");
        std::fs::write(&staging, RUNTIME).ok()?;
        std::fs::rename(&staging, &path).ok()?;
    }
    Some(path)
}

fn builder() -> Option<ort::session::builder::SessionBuilder> {
    let mut builder = ort::session::Session::builder().ok()?;
    builder = builder.with_intra_threads(infer_threads()).ok()?;
    #[cfg(feature = "cuda")]
    if std::env::var("ORT_USE_CUDA").is_ok_and(|value| value == "1" || value == "true") {
        let device = std::env::var("ORT_CUDA_DEVICE")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        builder = builder
            .with_execution_providers([ort::ep::CUDA::default()
                .with_device_id(device)
                .build()
                .error_on_failure()])
            .ok()?;
    }
    Some(builder)
}

fn started() -> bool {
    static STARTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *STARTED.get_or_init(|| match runtime_path() {
        Some(path) => match ort::init_from(&path) {
            Ok(environment) => {
                environment.commit();
                true
            }
            Err(e) => {
                eprintln!("nsfw: the runtime would not start, the lock is inert: {e}");
                false
            }
        },
        None => {
            eprintln!("nsfw: could not place the runtime beside the binary, the lock is inert");
            false
        }
    })
}

pub fn open_path(path: &std::path::Path, what: &str) -> Option<Session> {
    if !started() {
        return None;
    }
    let built = || -> Result<ort::session::Session, String> {
        let mut builder = builder().ok_or_else(|| "session builder failed".to_owned())?;
        builder.commit_from_file(path).map_err(|e| e.to_string())
    };
    match built() {
        Ok(session) => Some(std::sync::Mutex::new(session)),
        Err(e) => {
            eprintln!(
                "nsfw: the {what} would not load from {}: {e}",
                path.display()
            );
            None
        }
    }
}

pub fn open_path_pool(path: &std::path::Path, what: &str, count: usize) -> Option<SessionPool> {
    if !started() || count == 0 {
        return None;
    }
    let mut sessions = Vec::with_capacity(count);
    for _ in 0..count {
        let mut builder = builder()?;
        match builder.commit_from_file(path) {
            Ok(session) => sessions.push(std::sync::Mutex::new(session)),
            Err(e) => {
                eprintln!(
                    "nsfw: the {what} pool would not load from {}: {e}",
                    path.display()
                );
                return None;
            }
        }
    }
    Some(SessionPool {
        sessions: sessions.into_boxed_slice(),
        next: AtomicUsize::new(0),
    })
}

pub fn open_pool(bytes: &[u8], what: &str, count: usize) -> Option<SessionPool> {
    if !started() || count == 0 {
        return None;
    }
    let mut sessions = Vec::with_capacity(count);
    for _ in 0..count {
        let mut builder = builder()?;
        match builder.commit_from_memory(bytes) {
            Ok(session) => sessions.push(std::sync::Mutex::new(session)),
            Err(e) => {
                eprintln!("nsfw: the {what} pool would not load: {e}");
                return None;
            }
        }
    }
    Some(SessionPool {
        sessions: sessions.into_boxed_slice(),
        next: AtomicUsize::new(0),
    })
}

fn model() -> Option<&'static SessionPool> {
    static MODEL_CELL: std::sync::OnceLock<Option<SessionPool>> = std::sync::OnceLock::new();
    MODEL_CELL
        .get_or_init(|| open_pool(MODEL, "model", infer_sessions()))
        .as_ref()
}

fn grader() -> Option<&'static SessionPool> {
    static GRADER: std::sync::OnceLock<Option<SessionPool>> = std::sync::OnceLock::new();
    GRADER
        .get_or_init(|| open_pool(GRADE, "grader", infer_sessions()))
        .as_ref()
}

pub fn capacity_probe() -> usize {
    [model(), grader()]
        .into_iter()
        .filter(Option::is_some)
        .count()
}

pub fn run(session: &Session, shape: Vec<i64>, pixels: Vec<f32>) -> Option<Vec<f32>> {
    run_shaped(session, shape, pixels).map(|(_, values)| values)
}

pub fn run_embedding(session: &Session, shape: Vec<i64>, pixels: Vec<f32>) -> Option<Vec<f32>> {
    let input = ort::value::Value::from_array((shape, pixels)).ok()?;
    let mut session = session.lock().ok()?;
    let output = session.run(ort::inputs![input]).ok()?;
    output.into_iter().find_map(|(_, value)| {
        let (_, values) = value.try_extract_tensor::<f32>().ok()?;
        (values.len() == super::vision::DIM).then(|| values.to_vec())
    })
}

pub fn run_shaped(
    session: &Session,
    shape: Vec<i64>,
    pixels: Vec<f32>,
) -> Option<(Vec<i64>, Vec<f32>)> {
    let input = ort::value::Value::from_array((shape, pixels)).ok()?;
    let mut session = session.lock().ok()?;
    let output = session.run(ort::inputs![input]).ok()?;
    let (_, value) = output.into_iter().next()?;
    let (shape, values) = value.try_extract_tensor::<f32>().ok()?;
    Some((shape.to_vec(), values.to_vec()))
}

pub fn pixels_by(
    view: &image::RgbImage,
    side: usize,
    scale: impl Fn(usize, u8) -> f32,
) -> Vec<f32> {
    let raw = view.as_raw();
    let plane = side * side;
    let mut out = Vec::with_capacity(3 * plane);
    for channel in 0..3 {
        for at in 0..plane {
            out.push(scale(channel, raw[at * 3 + channel]));
        }
    }
    out
}

pub fn pixels_of(view: &image::RgbImage, side: usize, scale: fn(u8) -> f32) -> Vec<f32> {
    pixels_by(view, side, |_, value| scale(value))
}

#[allow(dead_code)]
pub fn limit(_ctx: &Ctx, _chat: i64) -> u32 {
    DEFAULT_LIMIT
}

fn probabilities(logits: &[f32]) -> Vec<f32> {
    if logits.is_empty() || logits.iter().any(|logit| !logit.is_finite()) {
        return vec![0.0; logits.len()];
    }
    let top = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp: Vec<f32> = logits.iter().map(|x| (x - top).exp()).collect();
    let sum: f32 = exp.iter().sum();
    if sum <= 0.0 {
        return vec![0.0; logits.len()];
    }
    exp.into_iter().map(|x| x / sum).collect()
}

fn nsfw_of(logits: &[f32]) -> f32 {
    probabilities(logits).first().copied().unwrap_or(0.0)
}

fn breakdown(labels: &[&str], logits: &[f32]) -> String {
    let p = probabilities(logits);
    labels
        .iter()
        .zip(&p)
        .map(|(label, value)| format!("{label} {value:.2}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn fit(image: &image::RgbImage, resize: usize, side: usize) -> image::RgbImage {
    let (width, height) = image.dimensions();
    let scale = resize as f32 / width.min(height).max(1) as f32;
    let resized = image::imageops::resize(
        image,
        ((width as f32 * scale).round() as u32).max(side as u32),
        ((height as f32 * scale).round() as u32).max(side as u32),
        image::imageops::FilterType::CatmullRom,
    );
    let (width, height) = resized.dimensions();
    let left = (width - side as u32) / 2;
    let top = (height - side as u32) / 2;
    image::imageops::crop_imm(&resized, left, top, side as u32, side as u32).to_image()
}

pub fn tiles(image: &image::RgbImage) -> Vec<std::borrow::Cow<'_, image::RgbImage>> {
    let (width, height) = image.dimensions();
    let long = width.max(height);
    let short = width.min(height).max(1);
    let mut out = vec![std::borrow::Cow::Borrowed(image)];
    if long as f32 / short as f32 >= WIDE {
        for offset in [0, long - short] {
            let (x, y) = if width >= height {
                (offset, 0)
            } else {
                (0, offset)
            };
            out.push(std::borrow::Cow::Owned(
                image::imageops::crop_imm(image, x, y, short, short).to_image(),
            ));
        }
    }
    out
}

fn views(image: &image::RgbImage, resize: usize, side: usize) -> Vec<image::RgbImage> {
    tiles(image)
        .iter()
        .map(|tile| fit(tile, resize, side))
        .collect()
}

fn sharpness(image: &image::RgbImage) -> f32 {
    let (width, height) = image.dimensions();
    if width < 3 || height < 3 {
        return 0.0;
    }
    let raw = image.as_raw();
    let luma: Vec<f32> = raw
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]))
        .collect();

    let (width, height) = (width as usize, height as usize);

    let (mut sum, mut squares) = (0f64, 0f64);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let at = y * width + x;
            let value =
                luma[at - width] + luma[at + width] + luma[at - 1] + luma[at + 1] - 4.0 * luma[at];
            sum += f64::from(value);
            squares += f64::from(value) * f64::from(value);
        }
    }
    let count = ((width - 2) * (height - 2)) as f64;
    let mean = sum / count;
    ((squares / count) - mean * mean).max(0.0) as f32
}

fn frail_of(detail: u32, sharpness: f32) -> bool {
    detail < DETAIL_FLOOR || sharpness < SHARPNESS_FLOOR
}

pub struct Look {
    pub score: f32,

    pub peak: f32,

    pub frail: bool,
    pub note: String,
}

fn judge(session: &Session, view: &image::RgbImage) -> Option<f32> {
    let shape = || vec![1, 3, SIDE as i64, SIDE as i64];
    let scale = |value: u8| f32::from(value) / 127.5 - 1.0;

    let logits = run(session, shape(), pixels_of(view, SIDE, scale))?;
    if logits.len() != CLASSES.len() {
        return None;
    }
    let first = nsfw_of(&logits);
    if !undecided(first) {
        return Some(first);
    }
    let mirror = image::imageops::flip_horizontal(view);

    let Some(logits) = run(session, shape(), pixels_of(&mirror, SIDE, scale)) else {
        return Some(first);
    };
    if logits.len() != CLASSES.len() {
        return Some(first);
    }
    Some((first + nsfw_of(&logits)) / 2.0)
}

fn look(image: &image::RgbImage) -> Option<Look> {
    let pool = model()?;
    pool.with(|session| look_with(session, image))
}

fn look_with(session: &Session, image: &image::RgbImage) -> Option<Look> {
    let detail = image.width().min(image.height());
    let sharpness = sharpness(image);
    let frail = frail_of(detail, sharpness);

    let mut scores: Vec<f32> = Vec::new();
    for view in views(image, RESIZE, SIDE) {
        scores.push(judge(session, &view)?);
    }
    let (&whole, ends) = scores.split_first()?;
    let note = format!(
        "views {} detail {detail} sharp {sharpness:.0}{}",
        scores
            .iter()
            .map(|s| format!("{s:.2}"))
            .collect::<Vec<_>>()
            .join("/"),
        if frail { " frail" } else { "" }
    );
    Some(Look {
        score: verdict_of(whole, ends, frail),
        peak: scores.iter().copied().fold(whole, f32::max),
        frail,
        note,
    })
}

fn hard_of(p: &[f32]) -> f32 {
    p[1] + p[3]
}

fn explicit_of(hard: f32, sexy: f32) -> bool {
    let revealing = hard < EXPLICIT_FLOOR && sexy >= SUGGESTIVE_FLOOR;
    !revealing
}

pub struct Grade {
    pub hard: f32,

    pub sexy: f32,

    pub neutral: f32,
    pub classes: String,
}

impl Grade {
    pub fn explicit(&self) -> bool {
        explicit_of(self.hard, self.sexy)
    }

    pub fn corroborates(&self) -> bool {
        self.hard >= EXPLICIT_FLOOR
    }
}

fn confirms(score: f32, grade: &Grade) -> bool {
    score >= GRADED_CONFIDENT
        || (score >= BRIDGE_SCORE && grade.hard >= BRIDGE_FLOOR)
        || (score >= STRONG_BRIDGE_SCORE && grade.hard >= STRONG_BRIDGE_FLOOR)
}

fn fold(seen: Option<Grade>, p: &[f32], classes: String) -> Grade {
    let hard = hard_of(p);
    let neutral = seen.as_ref().map_or(p[2], |seen| seen.neutral.min(p[2]));
    match seen {
        Some(seen) if seen.hard >= hard => Grade { neutral, ..seen },
        _ => Grade {
            hard,
            sexy: p[4],
            neutral,
            classes,
        },
    }
}

fn grade(image: &image::RgbImage) -> Option<Grade> {
    let pool = grader()?;
    pool.with(|session| grade_with(session, image))
}

fn grade_with(session: &Session, image: &image::RgbImage) -> Option<Grade> {
    let mut seen: Option<Grade> = None;
    let mut notes: Vec<String> = Vec::new();
    for view in views(image, GRADE_RESIZE, GRADE_SIDE) {
        let pixels = pixels_of(&view, GRADE_SIDE, |value| f32::from(value) / 255.0);
        let logits = run(
            session,
            vec![3, GRADE_SIDE as i64, GRADE_SIDE as i64],
            pixels,
        )?;
        if logits.len() != GRADE_CLASSES.len() {
            return None;
        }
        let p = probabilities(&logits);
        notes.push(breakdown(&GRADE_CLASSES, &logits));
        let folded = fold(seen, &p, String::new());
        let done = folded.corroborates();
        seen = Some(folded);
        if done {
            break;
        }
    }
    seen.map(|grade| Grade {
        classes: notes.join(" · "),
        ..grade
    })
}

fn innocent(grade: &Grade, score: f32) -> bool {
    grade.neutral >= NEUTRAL_FLOOR && score < CONFIDENT
}

fn allow_delete(
    innocent: bool,
    explicit: bool,
    spare: bool,
    weak: bool,
    confirmed: bool,
    arbiter: Option<Arbiter>,
) -> bool {
    let Some(arbiter) = arbiter else {
        return !innocent && !weak && confirmed && (!spare || explicit);
    };
    if !arbiter.agrees() {
        return false;
    }
    let sure = arbiter.sure();
    (confirmed || sure) && (!innocent || sure) && (!weak || sure) && (!spare || explicit)
}

fn verdict_of(whole: f32, ends: &[f32], frail: bool) -> f32 {
    if whole < CORROBORATE || frail {
        return whole;
    }
    ends.iter().copied().fold(whole, f32::max)
}

pub fn file_id(media: &Media) -> Option<i64> {
    use grammers_client::tl::enums::{Document, Photo};

    match media {
        Media::Photo(photo) => match photo.raw.photo.as_ref()? {
            Photo::Photo(photo) => Some(photo.id),
            Photo::Empty(_) => None,
        },
        Media::Document(document) => match document.raw.document.as_ref()? {
            Document::Document(document) => Some(document.id),
            Document::Empty(_) => None,
        },
        Media::Sticker(sticker) => match sticker.document.raw.document.as_ref()? {
            Document::Document(document) => Some(document.id),
            Document::Empty(_) => None,
        },
        Media::WebPage(page) => match webpage_photo(page)?.raw.photo? {
            Photo::Photo(photo) => Some(photo.id),
            Photo::Empty(_) => None,
        },
        _ => None,
    }
}

pub fn kind_of(media: &Media) -> &'static str {
    match media {
        Media::Photo(_) => "photo",
        Media::Sticker(_) => "sticker",
        Media::WebPage(_) => "link-preview",
        Media::Document(document) => match document.mime_type() {
            Some("image/gif") => "gif",
            Some(mime) if document.is_animated() && mime.starts_with("video/") => "gif",
            Some(mime) if mime.starts_with("video/") => "video",
            Some(mime) if mime.starts_with("image/") => "image-document",
            Some("application/x-tgsticker") => "animated-sticker",
            _ => "document",
        },
        _ => "other",
    }
}

fn is_animated_document(document: &Document) -> bool {
    matches!(document.mime_type(), Some("image/gif"))
        || document
            .mime_type()
            .is_some_and(|mime| mime.starts_with("video/"))
        || document.name().is_some_and(|name| {
            name.rsplit('.')
                .next()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
        })
}

const MAX_ANIMATED_FRAMES: usize = 12;
const MAX_ANIMATED_SIDE: u32 = 768;

fn sample_indices(total: usize, limit: usize) -> Vec<usize> {
    if total == 0 || limit == 0 {
        return Vec::new();
    }
    if total <= limit {
        return (0..total).collect();
    }
    if limit == 1 {
        return vec![0];
    }
    (0..limit)
        .map(|at| at * (total - 1) / (limit - 1))
        .collect()
}

fn scaled_dimensions(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    if longest <= max_side {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(max_side) / f64::from(longest);
    (
        (f64::from(width) * scale).round().max(1.0) as u32,
        (f64::from(height) * scale).round().max(1.0) as u32,
    )
}

fn decode_gif_frames(bytes: Vec<u8>) -> Option<Vec<image::RgbImage>> {
    let total = image::codecs::gif::GifDecoder::new(Cursor::new(&bytes))
        .ok()?
        .into_frames()
        .count();
    let wanted = sample_indices(total, MAX_ANIMATED_FRAMES);
    if wanted.is_empty() {
        return None;
    }

    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
    let mut frames = Vec::with_capacity(wanted.len());
    for (index, frame) in decoder.into_frames().enumerate() {
        if wanted.binary_search(&index).is_err() {
            continue;
        }
        let frame = frame.ok()?;
        frames.push(image::DynamicImage::ImageRgba8(frame.into_buffer()).to_rgb8());
    }
    (!frames.is_empty()).then_some(frames)
}

async fn animated_command(program: &str, args: &[String]) -> Option<std::process::Output> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::info!(
            "nsfw: {program} failed: {}",
            stderr.lines().next().unwrap_or("no error output")
        );
        return None;
    }
    Some(output)
}

fn animated_scratch_path() -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let at = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("groupbot-anim-{}-{at}.bin", std::process::id()))
}

fn json_number(value: Option<&serde_json::Value>) -> Option<f64> {
    value
        .and_then(serde_json::Value::as_f64)
        .or_else(|| value.and_then(serde_json::Value::as_str)?.parse().ok())
}

async fn decode_video_frames(bytes: Vec<u8>) -> Option<Vec<image::RgbImage>> {
    let path = animated_scratch_path();
    if tokio::fs::write(&path, &bytes).await.is_err() {
        return None;
    }
    drop(bytes);
    let frames = decode_video_file(&path).await;
    let _ = tokio::fs::remove_file(&path).await;
    frames
}

async fn decode_video_file(path: &std::path::Path) -> Option<Vec<image::RgbImage>> {
    let input = path.to_str()?;
    let probe_args = [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height,duration:format=duration",
        "-of",
        "json",
        "-i",
        input,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let probe = animated_command("ffprobe", &probe_args).await?;
    let json: serde_json::Value = serde_json::from_slice(&probe.stdout).ok()?;
    let stream = json.get("streams")?.as_array()?.first()?;
    let width = json_number(stream.get("width"))? as u32;
    let height = json_number(stream.get("height"))? as u32;
    let duration = json_number(stream.get("duration"))
        .or_else(|| json_number(json.get("format")?.get("duration")))?
        .max(0.001);
    let (width, height) = scaled_dimensions(width, height, MAX_ANIMATED_SIDE);
    let fps = (MAX_ANIMATED_FRAMES as f64 / duration).clamp(1.0 / 3600.0, 12.0);
    let filter = format!("fps={fps:.8},scale={width}:{height}:flags=bilinear");
    let ffmpeg_args = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        input,
        "-vf",
        filter.as_str(),
        "-frames:v",
        &MAX_ANIMATED_FRAMES.to_string(),
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "pipe:1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let output = animated_command("ffmpeg", &ffmpeg_args).await?;
    let frame_bytes = width.checked_mul(height)?.checked_mul(3)? as usize;
    let mut frames = Vec::new();
    for raw in output.stdout.chunks_exact(frame_bytes) {
        frames.push(image::RgbImage::from_raw(width, height, raw.to_vec())?);
    }
    (!frames.is_empty()).then_some(frames)
}

async fn decode_animated_frames(
    bytes: Vec<u8>,
    mime: Option<&str>,
) -> Option<Vec<image::RgbImage>> {
    if mime == Some("image/gif") {
        let native = bytes.clone();
        if let Ok(Some(frames)) =
            tokio::task::spawn_blocking(move || decode_gif_frames(native)).await
        {
            return Some(frames);
        }
    }
    decode_video_frames(bytes).await
}

struct AnimatedSelection {
    image: image::RgbImage,
    look: Option<Look>,
    margins: Option<[f32; super::CONCEPT_SLOTS]>,
    custom_margins: Option<Vec<f32>>,
}

fn score_animated_frames(
    frames: Vec<image::RgbImage>,
    needs_nsfw: bool,
    needs_general: bool,
    filter_specs: Vec<(Vec<f32>, f32)>,
) -> Option<AnimatedSelection> {
    struct Lane {
        best_nsfw: Option<(usize, image::RgbImage, Look)>,
        best_general: Option<(usize, image::RgbImage, f32)>,
        margins: Option<[f32; super::CONCEPT_SLOTS]>,
        custom_margins: Option<Vec<f32>>,
    }
    let total = frames.len();
    let lanes = infer_sessions().clamp(1, total.max(1));
    let mut shards: Vec<Vec<(usize, image::RgbImage)>> = (0..lanes).map(|_| Vec::new()).collect();
    for (index, frame) in frames.into_iter().enumerate() {
        shards[index % lanes].push((index, frame));
    }
    let filter_specs = &filter_specs;
    let partials: Vec<Lane> = std::thread::scope(|scope| {
        let handles: Vec<_> = shards
            .into_iter()
            .map(|shard| {
                scope.spawn(move || {
                    let mut lane = Lane {
                        best_nsfw: None,
                        best_general: None,
                        margins: needs_general.then_some([f32::MIN; super::CONCEPT_SLOTS]),
                        custom_margins: needs_general
                            .then(|| vec![f32::MIN; filter_specs.len()]),
                    };
                    for (index, frame) in shard {
                        let look = needs_nsfw.then(|| look(&frame)).flatten();
                        let embedding = needs_general
                            .then(|| super::vision::embed_of(&frame))
                            .flatten();

                        if let Some(look) = look {
                            let better = lane.best_nsfw.as_ref().is_none_or(|(at, _, current)| {
                                look.score > current.score
                                    || (look.score == current.score && look.peak > current.peak)
                                    || (look.score == current.score
                                        && look.peak == current.peak
                                        && index < *at)
                            });
                            if better {
                                lane.best_nsfw = Some((index, frame.clone(), look));
                            }
                        }

                        if let Some(embedding) = embedding {
                            let all = super::concepts::margins_from(&embedding);
                            let mut relevance = f32::MIN;
                            if let Some(maxima) = lane.margins.as_mut() {
                                for (maximum, value) in maxima.iter_mut().zip(all) {
                                    *maximum = maximum.max(value);
                                    relevance = relevance.max(value);
                                }
                            }
                            if let Some(maxima) = lane.custom_margins.as_mut() {
                                for (maximum, (vector, cut)) in
                                    maxima.iter_mut().zip(filter_specs)
                                {
                                    let value = super::imgfilter::margin(&embedding, vector);
                                    *maximum = maximum.max(value);
                                    relevance = relevance.max(value - *cut);
                                }
                            }
                            let better = lane
                                .best_general
                                .as_ref()
                                .is_none_or(|(at, _, current)| {
                                    relevance > *current
                                        || (relevance == *current && index < *at)
                                });
                            if better {
                                lane.best_general = Some((index, frame, relevance));
                            }
                        }
                    }
                    lane
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a frame lane panicked"))
            .collect()
    });

    let mut best_nsfw: Option<(usize, image::RgbImage, Look)> = None;
    let mut best_general: Option<(usize, image::RgbImage, f32)> = None;
    let mut margins = needs_general.then_some([f32::MIN; super::CONCEPT_SLOTS]);
    let mut custom_margins = needs_general.then(|| vec![f32::MIN; filter_specs.len()]);
    for lane in partials {
        if let Some((index, image, look)) = lane.best_nsfw {
            let better = best_nsfw.as_ref().is_none_or(|(at, _, current)| {
                look.score > current.score
                    || (look.score == current.score && look.peak > current.peak)
                    || (look.score == current.score
                        && look.peak == current.peak
                        && index < *at)
            });
            if better {
                best_nsfw = Some((index, image, look));
            }
        }
        if let Some((index, image, relevance)) = lane.best_general {
            let better = best_general.as_ref().is_none_or(|(at, _, current)| {
                relevance > *current || (relevance == *current && index < *at)
            });
            if better {
                best_general = Some((index, image, relevance));
            }
        }
        if let (Some(maxima), Some(from)) = (margins.as_mut(), lane.margins) {
            for (maximum, value) in maxima.iter_mut().zip(from) {
                *maximum = maximum.max(value);
            }
        }
        if let (Some(maxima), Some(from)) = (custom_margins.as_mut(), lane.custom_margins) {
            for (maximum, value) in maxima.iter_mut().zip(from) {
                *maximum = maximum.max(value);
            }
        }
    }

    let (image, look) = if needs_nsfw {
        let (index, image, mut look) = best_nsfw?;
        look.note = format!("animated frame {}/{} {}", index + 1, total, look.note);
        (image, Some(look))
    } else {
        let (_, image, _) = best_general?;
        (image, None)
    };
    Some(AnimatedSelection {
        image,
        look,
        margins,
        custom_margins,
    })
}

fn thumbs(media: &Media) -> Vec<PhotoSize> {
    match media {
        Media::Photo(photo) => photo.thumbs(),
        Media::Document(document) => {
            let mut sizes = match document.raw.video_cover.clone() {
                Some(cover) => grammers_client::media::Photo::from_raw(cover).thumbs(),
                None => Vec::new(),
            };
            sizes.extend(document.thumbs());
            sizes
        }
        Media::Sticker(sticker) => sticker.document.thumbs(),

        Media::WebPage(page) => webpage_photo(page).map_or_else(Vec::new, |photo| photo.thumbs()),
        _ => Vec::new(),
    }
}

fn webpage_photo(page: &grammers_client::media::WebPage) -> Option<grammers_client::media::Photo> {
    use grammers_client::tl::enums::WebPage as W;

    let W::Page(page) = &page.raw.webpage else {
        return None;
    };
    Some(grammers_client::media::Photo::from_raw(page.photo.clone()?))
}

fn why_skipped(media: &Media) -> String {
    let sizes = thumbs(media);
    let reason = skip_reason(
        sizes.len(),
        sizes.iter().all(|size| matches!(size, PhotoSize::Path(_))),
    );

    let detail = match media {
        Media::Document(document) => match document.raw.document.as_ref() {
            Some(grammers_client::tl::enums::Document::Document(raw)) => format!(
                " [thumbs {} video_thumbs {} cover {} mime {}]",
                raw.thumbs.as_ref().map_or(0, Vec::len),
                raw.video_thumbs.as_ref().map_or(0, Vec::len),
                u8::from(document.raw.video_cover.is_some()),
                raw.mime_type
            ),
            Some(_) => " [document is empty]".to_owned(),
            None => " [no document]".to_owned(),
        },
        _ => String::new(),
    };
    format!("{reason}{detail}")
}

fn skip_reason(count: usize, all_vector: bool) -> &'static str {
    if count == 0 {
        return "telegram attached no thumbnail";
    }
    if all_vector {
        return "only a vector outline";
    }
    "every thumbnail was rejected"
}

fn dc_of(media: &Media) -> Option<i32> {
    use grammers_client::tl::enums::{Document, Photo};

    match media {
        Media::Photo(photo) => match photo.raw.photo.as_ref()? {
            Photo::Photo(photo) => Some(photo.dc_id),
            Photo::Empty(_) => None,
        },
        Media::Document(document) => match document.raw.document.as_ref()? {
            Document::Document(document) => Some(document.dc_id),
            Document::Empty(_) => None,
        },
        Media::Sticker(sticker) => match sticker.document.raw.document.as_ref()? {
            Document::Document(document) => Some(document.dc_id),
            Document::Empty(_) => None,
        },
        Media::WebPage(page) => match webpage_photo(page)?.raw.photo? {
            Photo::Photo(photo) => Some(photo.dc_id),
            Photo::Empty(_) => None,
        },
        _ => None,
    }
}

struct Thumb {
    size: PhotoSize,
    dc: Option<i32>,
}

impl Downloadable for Thumb {
    fn to_raw_input_location(&self) -> Option<grammers_client::tl::enums::InputFileLocation> {
        self.size.to_raw_input_location()
    }

    fn to_data(&self) -> Option<Vec<u8>> {
        self.size.to_data()
    }

    fn size(&self) -> Option<usize> {
        Some(self.size.size())
    }

    fn dc_id(&self) -> Option<i32> {
        self.dc
    }
}

fn dims(size: &PhotoSize) -> Option<(i32, i32)> {
    match size {
        PhotoSize::Size(size) => Some((size.width, size.height)),
        PhotoSize::Cached(size) => Some((size.width, size.height)),
        PhotoSize::Progressive(size) => Some((size.width, size.height)),

        PhotoSize::Stripped(size) => match size.bytes.as_slice() {
            [0x01, width, height, ..] => Some((i32::from(*width), i32::from(*height))),
            _ => None,
        },
        _ => None,
    }
}

pub struct Ladder {
    sizes: Vec<PhotoSize>,
    dc: Option<i32>,
    pub start: usize,
}

impl Ladder {
    fn rung(&self, at: usize) -> Thumb {
        Thumb {
            size: self.sizes[at].clone(),
            dc: self.dc,
        }
    }
}

pub fn ladder(media: &Media) -> Option<Ladder> {
    let sizes: Vec<PhotoSize> = thumbs(media).into_iter().filter(usable).collect();
    let numbers: Vec<(i32, usize)> = sizes
        .iter()
        .map(|size| {
            (
                dims(size).map_or(0, |(width, height)| width.min(height)),
                size.size(),
            )
        })
        .collect();
    let start = best_thumb(&numbers)?;
    Some(Ladder {
        sizes,
        dc: dc_of(media),
        start,
    })
}

fn usable(size: &PhotoSize) -> bool {
    !matches!(size, PhotoSize::Path(_))
        && (size.to_raw_input_location().is_some() || size.to_data().is_some())
}

fn best_thumb(candidates: &[(i32, usize)]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, (side, _))| *side >= RESIZE as i32)
        .min_by_key(|(_, (_, bytes))| *bytes)
        .or_else(|| {
            candidates
                .iter()
                .enumerate()
                .max_by_key(|(_, (side, _))| *side)
        })
        .map(|(at, _)| at)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ignore,
    Log,
    Delete,
}

#[derive(Clone, Copy, Debug)]
pub struct Judgement {
    pub score: f32,

    pub innocent: bool,

    pub explicit: bool,

    pub confirmed: bool,

    pub weak: bool,

    pub arbiter: Option<Arbiter>,
}

fn verdict(score: f32, limit: u32, shadow: bool) -> Verdict {
    let over = score * 100.0 >= limit as f32;
    match (shadow, over) {
        (true, _) => Verdict::Log,
        (false, true) => Verdict::Delete,
        (false, false) => Verdict::Ignore,
    }
}

pub async fn watch(ctx: &Arc<Ctx>, message: &Message, chat: i64, view: &View<'_>) {
    let Some(media) = view.media() else {
        return;
    };
    let Some(id) = file_id(media) else {
        return;
    };

    let animated_media =
        matches!(media, Media::Document(document) if is_animated_document(document));

    let (nsfw, concepts, advert) = ctx.settings.with_chat(chat, |settings| {
        let nsfw = settings.is_locked(LOCK).then_some(Armed {
            shadow: false,
            limit: DEFAULT_LIMIT,
            spare: false,
        });
        let advert = settings.is_locked(super::ocr::LOCK).then(|| AdvertArmed {
            live: !settings.is_locked(super::ocr::SHADOW),
        });
        (nsfw, super::concepts::armed_under(&settings), advert)
    });

    let custom = super::imgfilter::any(ctx, chat);
    if nsfw.is_none() && concepts.is_none() && advert.is_none() && !custom {
        return;
    }

    let verdict = ctx.known_verdict(id, animated_media);
    let margins = ctx.known_margins(id, animated_media);
    let reading = ctx.known_advert(id);
    let wants_nsfw = nsfw.is_some() && verdict.is_none();
    let wants_concepts = concepts.is_some() && margins.is_none();
    let wants_text = advert.is_some() && reading.is_none();

    let filters = match custom {
        true => ctx.image_filters(chat).await,
        false => no_filters(),
    };
    let custom_margins = super::imgfilter::cached_margins(ctx, id, &filters, animated_media);
    let wants_custom = !filters.is_empty() && custom_margins.is_none();

    if super::is_exempt(ctx, message).await {
        return;
    }

    if !wants_nsfw && !wants_concepts && !wants_text && !wants_custom {
        if let (Some(armed), Some(judged)) = (&nsfw, verdict) {
            act(ctx, message, chat, id, judged, armed, true).await;
        }
        if let (Some(armed), Some(all)) = (&concepts, margins) {
            super::concepts::act_known(ctx, message, chat, id, &all, armed, true).await;
        }
        if let (Some(armed), Some(why)) = (&advert, reading) {
            act_advert(ctx, message, chat, id, why, armed, true).await;
        }
        if let Some(margins) = &custom_margins
            && !filters.is_empty()
        {
            super::imgfilter::act_known(ctx, message, chat, id, &filters, margins).await;
        }
        return;
    }

    let ladder = ladder(media);
    let gif_document = match media {
        Media::Document(document) if animated_media => Some(document.clone()),
        _ => None,
    };
    if ladder.is_none() && gif_document.is_none() {
        log::info!(
            "nsfw: {chat} file {id} skipped, kind {}, {}",
            kind_of(media),
            why_skipped(media)
        );
        return;
    }

    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return;
    };
    let message_id = message.id();

    let kind = kind_of(media);
    let sender = message.sender_id().and_then(PeerId::bare_id);
    let name = super::name_of(message);
    let ctx = Arc::clone(ctx);
    let task_slot = ctx.nsfw_task_slot().await;

    tokio::spawn(async move {
        let _task_slot = task_slot;
        classify(
            ctx,
            chat,
            chat_ref,
            message_id,
            id,
            ladder,
            gif_document,
            kind,
            nsfw,
            concepts,
            advert,
            verdict,
            margins,
            reading,
            filters,
            custom_margins,
            sender,
            name,
        )
        .await;
    });
}

fn no_filters() -> Arc<Vec<super::imgfilter::Filter>> {
    static EMPTY: std::sync::OnceLock<Arc<Vec<super::imgfilter::Filter>>> =
        std::sync::OnceLock::new();
    Arc::clone(EMPTY.get_or_init(|| Arc::new(Vec::new())))
}

#[derive(Clone, Copy)]
pub struct AdvertArmed {
    pub live: bool,
}

#[derive(Clone, Copy)]
pub struct Armed {
    pub shadow: bool,
    pub limit: u32,
    pub spare: bool,
}

async fn embed(ctx: &Arc<Ctx>, image: &Arc<image::RgbImage>, remember: bool) -> Option<Vec<f32>> {
    let frame = Arc::clone(image);
    let embedding = tokio::task::spawn_blocking(move || super::vision::embed_of(&frame))
        .await
        .ok()
        .flatten()?;
    if remember {
        ctx.remember_sample(&embedding);
    }
    Some(embedding)
}

#[allow(clippy::too_many_arguments)]
async fn classify(
    ctx: Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    id: i64,
    ladder: Option<Ladder>,
    gif_document: Option<Document>,
    kind: &'static str,
    nsfw: Option<Armed>,
    concepts: Option<super::concepts::Armed>,
    advert: Option<AdvertArmed>,
    verdict: Option<Judgement>,
    margins: Option<[f32; super::CONCEPT_SLOTS]>,
    reading: Option<Option<&'static str>>,
    filters: Arc<Vec<super::imgfilter::Filter>>,
    custom_margins: Option<Vec<f32>>,
    sender: Option<i64>,
    name: String,
) {
    let needs_nsfw = nsfw.is_some() && verdict.is_none();
    let needs_concepts = concepts.is_some() && margins.is_none();
    let needs_custom = !filters.is_empty() && custom_margins.is_none();
    let needs_general = needs_concepts || needs_custom;
    let filter_specs = filters
        .iter()
        .map(|filter| (filter.vector.clone(), filter.cut))
        .collect::<Vec<_>>();
    let mut margins = margins;
    let mut custom_margins = custom_margins;

    let full_animation = if needs_nsfw || needs_general {
        if let Some(document) = gif_document.as_ref() {
            let mime = document.mime_type().map(str::to_owned);
            match fetch_document(&ctx, chat, id, document).await {
                Some(bytes) => {
                    let size = bytes.len();
                    match decode_animated_frames(bytes, mime.as_deref()).await {
                        Some(frames) => Some((frames, size)),
                        None => {
                            log::warn!(
                                "nsfw: {chat} file {id} animated {kind} could not be decoded; falling back to thumbnail"
                            );
                            None
                        }
                    }
                }
                None => {
                    log::warn!(
                        "nsfw: {chat} file {id} animated {kind} could not be downloaded; falling back to thumbnail"
                    );
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let thinking = std::time::Instant::now();
    let _slot = ctx.nsfw_slot().await;

    let had_full_animation = full_animation.is_some();
    let selected_animation = if let Some((frames, size)) = full_animation {
        let selection = tokio::task::spawn_blocking(move || {
            score_animated_frames(frames, needs_nsfw, needs_general, filter_specs)
        })
        .await
        .ok()
        .flatten();
        match selection {
            Some(selection) => {
                if let Some(all) = selection.margins {
                    ctx.remember_margins(id, all, true);
                    margins = Some(all);
                }
                if let Some(all) = selection.custom_margins {
                    for (filter, margin) in filters.iter().zip(&all) {
                        ctx.remember_custom(id, filter.print, *margin, true);
                    }
                    custom_margins = Some(all);
                }
                Some((selection.image, selection.look, size))
            }
            None => None,
        }
    } else {
        None
    };
    if had_full_animation && selected_animation.is_none() {
        log::warn!(
            "nsfw: {chat} file {id} animated {kind} decoded but no frame produced a model score; falling back to thumbnail"
        );
    }

    let from_animation = selected_animation.is_some();
    let (image, initial_look, size, pixels) = if let Some((image, look, size)) = selected_animation
    {
        let pixels = format!("{}x{}", image.width(), image.height());
        (image, look, size, pixels)
    } else {
        let Some(ladder) = ladder.as_ref() else {
            log::info!(
                "nsfw: {chat} file {id} skipped, kind {}, animated media had no usable decoded frame",
                kind
            );
            return;
        };
        let Some(bytes) = fetch(&ctx, chat, id, ladder, ladder.start).await else {
            return;
        };
        let size = bytes.len();
        let pixels = dims(&ladder.sizes[ladder.start]).map_or_else(
            || "?x?".to_owned(),
            |(width, height)| format!("{width}x{height}"),
        );

        let decoded = tokio::task::spawn_blocking(move || {
            image::load_from_memory(&bytes).map(|image| image.to_rgb8())
        })
        .await;
        let image = match decoded {
            Ok(Ok(image)) => image,
            Ok(Err(e)) => {
                log::warn!("nsfw: {chat} file {id} {pixels} could not be decoded: {e}");
                return;
            }
            Err(e) => {
                log::warn!("nsfw: {chat} file {id} decode task failed: {e}");
                return;
            }
        };
        (image, None, size, pixels)
    };

    let mut image = Arc::new(image);
    let mut initial_look = initial_look;

    let wants_embedding = (concepts.is_some() && margins.is_none())
        || (!filters.is_empty() && custom_margins.is_none());
    let mut embedding: Option<Vec<f32>> = None;
    let mut embedded = false;

    if let Some(armed) = nsfw {
        let judged = match verdict {
            Some(judged) => judged,
            None => {
                let mut look = match initial_look.take() {
                    Some(look) => look,
                    None => {
                        let frame = Arc::clone(&image);
                        let Ok(Some(look)) =
                            tokio::task::spawn_blocking(move || look(&frame)).await
                        else {
                            return;
                        };
                        look
                    }
                };

                let escalate = look.score >= GRADE_ABOVE && (undecided(look.score) || look.frail);
                let sharper = ladder.as_ref().and_then(|ladder| {
                    (escalate && ladder.sizes.len() > ladder.start + 1).then_some(ladder)
                });
                let sharper = match sharper {
                    Some(ladder) => fetch(&ctx, chat, id, ladder, ladder.start + 1).await,
                    None => None,
                };

                if let Some(sharper) = sharper
                    && let Ok(Some((better, frame))) = tokio::task::spawn_blocking(move || {
                        let frame = image::load_from_memory(&sharper).ok()?.to_rgb8();
                        self::look(&frame).map(|look| (look, frame))
                    })
                    .await
                {
                    look = Look {
                        note: format!("{} escalated", better.note),
                        ..better
                    };
                    image = Arc::new(frame);
                }

                let arbiter = if look.peak >= GRADE_ABOVE {
                    if !embedded {
                        embedding = embed(&ctx, &image, wants_embedding).await;
                        embedded = true;
                    }
                    embedding.as_deref().map(arbiter_of)
                } else {
                    None
                };

                let (innocent, explicit, confirmed, weak) = if look.score >= GRADE_ABOVE {
                    let frame = Arc::clone(&image);
                    match tokio::task::spawn_blocking(move || grade(&frame)).await {
                        Ok(Some(grade)) => {
                            log::info!("nsfw: {chat} file {id} grade [{}]", grade.classes);
                            (
                                innocent(&grade, look.score),
                                grade.explicit(),
                                confirms(look.score, &grade),
                                look.frail && !grade.corroborates(),
                            )
                        }

                        _ => (false, true, look.score >= CONFIDENT, false),
                    }
                } else {
                    (false, true, look.score >= CONFIDENT, false)
                };

                let score = match arbiter.is_some_and(|a| a.sure()) {
                    true => look.peak,
                    false => look.score,
                };

                let (score, explicit) = match arbiter.filter(|a| a.head_deletes()) {
                    Some(a) => (score.max(a.head), true),
                    None => (score, explicit),
                };
                let judged = Judgement {
                    score,
                    innocent,
                    explicit,
                    confirmed,
                    weak,
                    arbiter,
                };
                ctx.remember_verdict(id, judged, from_animation);
                log::info!(
                    "nsfw: {chat} file {id} {kind} {pixels} bytes {size} think {}ms {}{}{}",
                    thinking.elapsed().as_millis(),
                    look.note,
                    if score > look.score {
                        " raised"
                    } else {
                        ""
                    },
                    arbiter.map_or_else(
                        || " arbiter none".to_owned(),
                        |a| format!(
                            " arbiter {:+.4}/{:+.4} head {:.3}",
                            a.explicit, a.revealing, a.head
                        )
                    )
                );
                judged
            }
        };
        act_detached(
            &ctx,
            chat,
            chat_ref,
            message_id,
            id,
            judged,
            &armed,
            sender,
            name.clone(),
        )
        .await;
    }

    if wants_embedding && !embedded {
        embedding = embed(&ctx, &image, true).await;
    }

    if let Some(armed) = concepts {
        let all = match margins {
            Some(all) => Some(all),
            None => embedding.as_ref().map(|embedding| {
                let all = super::concepts::margins_from(embedding);
                ctx.remember_margins(id, all, from_animation);
                all
            }),
        };
        if let Some(all) = all {
            super::concepts::act_detached(
                &ctx, chat, chat_ref, message_id, id, &all, &armed, sender, &name,
            )
            .await;
        }
    }

    if !filters.is_empty() {
        let scored = match custom_margins {
            Some(margins) => Some(margins),
            None => embedding.as_ref().map(|embedding| {
                super::imgfilter::score_all(&ctx, id, &filters, embedding, from_animation)
            }),
        };
        if let Some(scored) = scored {
            super::imgfilter::act_detached(
                &ctx, chat, chat_ref, message_id, id, &filters, &scored, sender, &name,
            )
            .await;
        }
    }

    if let Some(armed) = advert {
        let why = match reading {
            Some(why) => why,
            None => {
                let frame = Arc::clone(&image);
                let text = tokio::task::spawn_blocking(move || super::ocr::read(&frame))
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let why = super::ocr::advertises(&text);
                log::info!(
                    "advert[{}]: chat {chat} file {id} read {:?} → {}",
                    if armed.live { "live" } else { "shadow" },
                    text.chars().take(60).collect::<String>(),
                    why.unwrap_or("ok")
                );
                ctx.remember_advert(id, why);
                why
            }
        };
        let Some(why) = why else {
            return;
        };
        if !armed.live {
            return;
        }
        match ctx.client.delete_messages(chat_ref, &[message_id]).await {
            Ok(0) => {
                eprintln!("advert: delete affected nothing in {chat} msg {message_id}");
                return;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("advert: could not delete in {chat} msg {message_id}: {e}");
                return;
            }
        }
        ctx.bump(chat, super::stats::DELETED);
        punish_and_notify(&ctx, chat, chat_ref, sender, &name, super::ocr::LOCK, why).await;
    }
}

async fn act_advert(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    id: i64,
    why: Option<&'static str>,
    armed: &AdvertArmed,
    cached: bool,
) {
    log::info!(
        "advert[{}]: chat {chat} file {id} {}{}",
        if armed.live { "live" } else { "shadow" },
        why.unwrap_or("ok"),
        if cached { " cached" } else { "" }
    );
    let Some(why) = why else {
        return;
    };
    if !armed.live {
        return;
    }
    if let Err(e) = message.delete().await {
        eprintln!("advert: could not delete in {chat}: {e}");
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    let chances = match super::strict::punish(ctx, message, chat, super::ocr::LOCK).await {
        super::strict::Outcome::Announced => return,
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::notice::send(ctx, message, chat, why, chances).await;
}

pub async fn fetch(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    ladder: &Ladder,
    at: usize,
) -> Option<Vec<u8>> {
    let thumb = ladder.rung(at);
    let _fetching = ctx.nsfw_fetch().await;

    let mut bytes = Vec::with_capacity(Downloadable::size(&thumb).unwrap_or(0));
    let mut download = ctx.client.iter_download(&thumb);
    loop {
        match download.next().await {
            Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(e) => {
                eprintln!("nsfw: could not fetch a thumbnail in {chat} for {id}: {e}");
                return None;
            }
        }
    }
    (!bytes.is_empty()).then_some(bytes)
}

async fn fetch_document(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    document: &Document,
) -> Option<Vec<u8>> {
    let _fetching = ctx.nsfw_fetch().await;
    let mut bytes = Vec::with_capacity(Downloadable::size(document).unwrap_or(0));
    let mut download = ctx.client.iter_download(document);
    loop {
        match download.next().await {
            Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(e) => {
                eprintln!("nsfw: could not fetch animated media in {chat} for {id}: {e}");
                return None;
            }
        }
    }
    (!bytes.is_empty()).then_some(bytes)
}

async fn act(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    id: i64,
    judged: Judgement,
    armed: &Armed,
    cached: bool,
) {
    report(chat, id, judged, armed, cached);
    if verdict(judged.score, armed.limit, armed.shadow) != Verdict::Delete
        || spared(judged, armed.spare, chat, id)
    {
        return;
    }
    if let Err(e) = message.delete().await {
        eprintln!("nsfw: could not delete in {chat}: {e}");
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    let chances = match super::strict::punish(ctx, message, chat, LOCK).await {
        super::strict::Outcome::Announced => return,
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::notice::send(ctx, message, chat, "محتوای غیراخلاقی", chances).await;
}

#[allow(clippy::too_many_arguments)]
async fn act_detached(
    ctx: &Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    id: i64,
    judged: Judgement,
    armed: &Armed,
    sender: Option<i64>,
    name: String,
) {
    report(chat, id, judged, armed, false);
    if verdict(judged.score, armed.limit, armed.shadow) != Verdict::Delete
        || spared(judged, armed.spare, chat, id)
    {
        return;
    }
    match ctx.client.delete_messages(chat_ref, &[message_id]).await {
        Ok(0) => {
            eprintln!("nsfw: delete affected nothing in {chat} msg {message_id}");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("nsfw: could not delete in {chat} msg {message_id}: {e}");
            return;
        }
    }
    ctx.bump(chat, super::stats::DELETED);

    punish_and_notify(ctx, chat, chat_ref, sender, &name, LOCK, "محتوای غیراخلاقی").await;
}

pub async fn punish_and_notify(
    ctx: &Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    sender: Option<i64>,
    name: &str,
    cause: &str,
    reason: &str,
) {
    let chances = match super::strict::punish_detached(ctx, chat, chat_ref, sender, name, cause)
        .await
    {
        super::strict::Outcome::Announced => return,
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    notify(ctx, chat, chat_ref, sender, name, reason, chances).await;
}

pub async fn notify(
    ctx: &Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    sender: Option<i64>,
    name: &str,
    reason: &str,
    chances: Option<u32>,
) {
    if !ctx.settings.is_locked(chat, super::notice::MODE) {
        return;
    }
    let Some(user) = sender else {
        return;
    };
    if !ctx.may_notify_lock(chat, user) {
        return;
    }
    let text = format!(
        "<a href=\"tg://user?id={user}\">{}</a> پیام شما حذف شد · <b>{}</b> در این گروه قفل است.\n<i>لطفا دوباره نفرستید.</i>{}",
        super::esc(name),
        super::esc(reason),
        match chances {
            Some(left) => format!("\n{}", super::strict::chances_line(left)),
            None => String::new(),
        }
    );
    let Ok(sent) = ctx
        .client
        .send_message(chat_ref, InputMessage::new().html(text))
        .await
    else {
        return;
    };
    let seconds = super::notice::ttl(ctx, chat);
    if seconds == 0 {
        return;
    }
    let sent_id = sent.id();
    ctx.schedule_delete(
        chat,
        sent_id,
        std::time::Instant::now() + std::time::Duration::from_secs(u64::from(seconds)),
    );
}

fn spared(judged: Judgement, spare: bool, chat: i64, id: i64) -> bool {
    if allow_delete(
        judged.innocent,
        judged.explicit,
        spare,
        judged.weak,
        judged.confirmed,
        judged.arbiter,
    ) {
        return false;
    }
    let sure = judged.arbiter.is_some_and(|a| a.sure());
    let why = if judged.arbiter.is_some_and(|a| a.head < HEAD_VETO) {
        "the head reads it as an ordinary picture"
    } else if judged.arbiter.is_some_and(|a| !a.agrees()) {
        "the general model reads it as an ordinary photograph"
    } else if judged.innocent && !sure {
        "the grader is certain it is nothing"
    } else if judged.weak && !sure {
        "too little real detail to judge, and the grader saw nothing"
    } else if !judged.confirmed && !sure {
        "the two models did not positively agree"
    } else {
        "suggestive rather than explicit"
    };
    log::info!("nsfw: {chat} file {id} kept, {why}");
    true
}

fn report(chat: i64, id: i64, judged: Judgement, armed: &Armed, cached: bool) {
    let Judgement {
        score,
        weak,
        confirmed,
        arbiter,
        ..
    } = judged;
    let limit = armed.limit;
    let over = if score * 100.0 >= limit as f32 {
        "OVER"
    } else {
        "ok"
    };
    let how = if cached { " cached" } else { "" };
    let mode = if armed.shadow { "shadow" } else { "live" };
    let weak = if weak { " weak" } else { "" };
    let confirmed = if confirmed { "" } else { " unconfirmed" };
    let arbiter = arbiter.map_or_else(String::new, |a| {
        format!(" arbiter {:+.4} head {:.3}", a.explicit, a.head)
    });
    log::info!(
        "nsfw[{mode}]: chat {chat} file {id} score {score:.4} limit {limit} {over}{weak}{confirmed}{arbiter}{how}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animated_sampling_spans_the_whole_document() {
        assert_eq!(sample_indices(0, MAX_ANIMATED_FRAMES), Vec::<usize>::new());
        assert_eq!(sample_indices(4, MAX_ANIMATED_FRAMES), vec![0, 1, 2, 3]);
        assert_eq!(sample_indices(100, 4), vec![0, 33, 66, 99]);
        assert_eq!(sample_indices(100, 1), vec![0]);
    }

    #[test]
    fn native_gif_decoder_keeps_first_and_last_sampled_frames() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for value in 0..=MAX_ANIMATED_FRAMES as u8 {
                let frame = image::Frame::new(image::RgbaImage::from_pixel(
                    8,
                    8,
                    image::Rgba([value, 0, 0, 255]),
                ));
                encoder.encode_frame(frame).expect("GIF frame encodes");
            }
        }
        let frames = decode_gif_frames(bytes).expect("GIF decodes");
        assert_eq!(frames.len(), MAX_ANIMATED_FRAMES);
        assert_eq!(frames.first().unwrap().get_pixel(0, 0)[0], 0);
        assert_eq!(
            frames.last().unwrap().get_pixel(0, 0)[0],
            MAX_ANIMATED_FRAMES as u8
        );
    }

    #[tokio::test]
    async fn mp4_animation_decoder_reads_telegram_style_media() {
        let args = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=16x16:d=1:r=12",
            "-an",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "frag_keyframe+empty_moov",
            "-f",
            "mp4",
            "pipe:1",
        ];
        let output = Command::new("ffmpeg")
            .args(args)
            .output()
            .await
            .expect("ffmpeg is installed for animated media");
        assert!(
            output.status.success(),
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let frames = decode_video_frames(output.stdout)
            .await
            .expect("MP4 animation decodes");
        assert!(!frames.is_empty());
        assert_eq!(frames[0].dimensions(), (16, 16));
    }

    #[tokio::test]
    async fn mp4_with_a_trailing_moov_atom_still_decodes() {
        let path = std::env::temp_dir().join(format!("groupbot-test-moov-{}.mp4", std::process::id()));
        let args = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=16x16:d=1:r=12",
            "-an",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "mp4",
            path.to_str().unwrap(),
        ];
        let output = Command::new("ffmpeg")
            .args(args)
            .output()
            .await
            .expect("ffmpeg is installed for animated media");
        assert!(
            output.status.success(),
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = tokio::fs::read(&path).await.expect("the muxed file exists");
        let _ = tokio::fs::remove_file(&path).await;
        let frames = decode_video_frames(bytes)
            .await
            .expect("a trailing-moov MP4 decodes");
        assert!(!frames.is_empty());
        assert_eq!(frames[0].dimensions(), (16, 16));
    }

    fn arbiter(explicit: f32) -> Option<Arbiter> {
        Some(Arbiter {
            explicit,
            revealing: 0.0,
            head: HEAD_VETO,
        })
    }

    fn head(head: f32) -> Option<Arbiter> {
        Some(Arbiter {
            explicit: 0.0,
            revealing: 0.0,
            head,
        })
    }

    #[test]
    fn a_middling_head_changes_nothing() {
        for (innocent, weak, confirmed, explicit) in [
            (false, false, true, 0.005),
            (false, false, true, AGREE),
            (true, false, true, AGREE),
            (false, true, true, AGREE),
            (false, false, false, AGREE),
            (false, false, true, RECOVER),
            (true, true, false, RECOVER),
        ] {
            for spare in [false, true] {
                for head in [HEAD_VETO, 0.41, HEAD_SURE - 0.01] {
                    let without = allow_delete(innocent, true, spare, weak, confirmed, arbiter(explicit));
                    let with = allow_delete(
                        innocent,
                        true,
                        spare,
                        weak,
                        confirmed,
                        Some(Arbiter {
                            explicit,
                            revealing: 0.0,
                            head,
                        }),
                    );
                    assert_eq!(with, without, "innocent {innocent} weak {weak} confirmed {confirmed} margin {explicit} head {head}");
                }
            }
        }
    }

    #[test]
    fn a_head_that_reads_ordinary_withholds() {
        let with_head = |head: f32| {
            Some(Arbiter {
                explicit: 0.065,
                revealing: 0.0,
                head,
            })
        };
        assert!(
            !allow_delete(false, true, false, false, true, with_head(HEAD_VETO - 0.01)),
            "a confident first model and a sure prompt margin are still vetoed"
        );
        assert!(allow_delete(false, true, false, false, true, with_head(HEAD_VETO)));
    }

    #[test]
    fn a_sure_head_is_positive_evidence() {
        assert!(
            !allow_delete(false, true, false, false, true, arbiter(0.005)),
            "vetoed on the prompt margin alone"
        );
        assert!(
            allow_delete(false, true, false, false, true, head(HEAD_SURE)),
            "the head lifts the veto the captions could not see past"
        );
        for (innocent, weak, confirmed) in [(true, false, true), (false, true, true), (false, false, false)] {
            assert!(allow_delete(innocent, true, false, weak, confirmed, head(HEAD_SURE)));
        }

        assert!(!allow_delete(false, false, true, false, true, head(1.0)));
    }

    #[test]
    fn the_head_thresholds_are_ordered_and_strict() {
        const { assert!(HEAD_VETO < 0.41, "the reverted head's hentai gif must not be vetoed again") };
        const { assert!(HEAD_VETO < HEAD_SURE, "the silent band exists") };
        const { assert!(HEAD_SURE >= 0.5, "sure is at least a coin toss in the head's own terms") };
        const { assert!(HEAD_DELETE >= HEAD_SURE, "deleting alone is the stricter act") };
        const { assert!(HEAD_DELETE >= 0.90, "the head acts alone only when it is nearly certain") };
        assert!(head(HEAD_SURE).is_some_and(|a| a.sure() && a.agrees()));
        assert!(head(HEAD_SURE - 0.01).is_some_and(|a| !a.sure() && !a.agrees()));
        assert!(head(HEAD_DELETE).is_some_and(|a| a.head_deletes()));
        assert!(arbiter(RECOVER).is_some_and(|a| a.sure()));
    }

    #[test]
    fn the_general_model_can_only_withhold_a_deletion() {
        assert!(
            !allow_delete(false, true, false, false, true, arbiter(0.005)),
            "an ordinary photograph is kept however sure the first two models are"
        );

        assert!(allow_delete(false, true, false, false, true, None));

        assert!(
            !allow_delete(false, false, true, false, true, arbiter(0.9)),
            "a soft chat still keeps a merely revealing picture"
        );
    }

    #[test]
    fn positive_evidence_answers_the_gates_that_asked_for_it() {
        for (innocent, weak, confirmed, what) in [
            (true, false, true, "the neutral veto"),
            (false, true, true, "the frail rule"),
            (false, false, false, "the confidence ladder"),
        ] {
            assert!(
                !allow_delete(innocent, true, false, weak, confirmed, arbiter(AGREE)),
                "{what} still holds while the general model is merely not objecting"
            );
            assert!(
                allow_delete(innocent, true, false, weak, confirmed, arbiter(RECOVER)),
                "{what} is answered once the general model is sure"
            );
        }
    }

    #[test]
    fn the_arbiter_thresholds_bracket_the_measured_gap() {
        const { assert!(AGREE < RECOVER, "agreeing is a lower bar than being sure") };
        const { assert!(AGREE > 0.007, "above the median ordinary photograph") };
        const { assert!(RECOVER < 0.065, "below the median offending picture") };
        assert!(
            arbiter(0.026).is_some_and(|a| !a.sure()),
            "the 95th percentile of ordinary photographs is not positive evidence"
        );
        assert!(
            arbiter(0.039).is_some_and(|a| a.agrees()),
            "the 5th percentile of offending pictures is not vetoed"
        );
    }

    #[test]
    fn a_frail_wide_frame_reports_its_whole_and_keeps_its_peak() {
        let ends = [0.93, 0.71];

        assert!((verdict_of(0.10, &ends, true) - 0.10).abs() < 1e-6);

        assert!((verdict_of(0.40, &ends, false) - 0.93).abs() < 1e-6);

        assert!((verdict_of(0.10, &ends, false) - 0.10).abs() < 1e-6);
    }

    #[test]
    fn only_a_sure_arbiter_unlocks_the_crops() {
        let (careful, peak) = (0.10f32, 0.93f32);
        let choose = |arbiter: Option<Arbiter>| match arbiter.is_some_and(|a| a.sure()) {
            true => peak,
            false => careful,
        };

        assert!((choose(None) - careful).abs() < 1e-6);

        assert!((choose(arbiter(AGREE)) - careful).abs() < 1e-6);

        assert!((choose(arbiter(0.026)) - careful).abs() < 1e-6);

        assert!((choose(arbiter(RECOVER)) - peak).abs() < 1e-6);
    }

    #[test]
    fn the_skip_reason_separates_a_hole_from_a_non_picture() {
        assert_eq!(skip_reason(0, true), "telegram attached no thumbnail");
        assert_eq!(skip_reason(3, true), "only a vector outline");

        assert_eq!(skip_reason(0, false), "telegram attached no thumbnail");

        assert_eq!(skip_reason(2, false), "every thumbnail was rejected");
    }

    #[test]
    fn shadow_mode_never_deletes() {
        for score in [0.0, 0.5, 0.9, 1.0] {
            assert_eq!(verdict(score, 90, true), Verdict::Log, "score {score}");
        }
        assert_eq!(verdict(0.95, 90, false), Verdict::Delete);
        assert_eq!(
            verdict(0.90, 90, false),
            Verdict::Delete,
            "the limit is inclusive"
        );
        assert_eq!(verdict(0.89, 90, false), Verdict::Ignore);
    }

    #[test]
    fn the_offending_class_is_the_first_one() {
        assert_eq!(CLASSES, ["nsfw", "sfw"]);
        assert!(
            (nsfw_of(&[10.0, -10.0]) - 1.0).abs() < 0.01,
            "index 0 must be the offence"
        );
        assert!(
            nsfw_of(&[-10.0, 10.0]) < 0.01,
            "index 1 must be the safe one"
        );

        assert!(nsfw_of(&[0.0, 0.0]) < 0.51);
        assert_eq!(nsfw_of(&[]), 0.0, "an empty output fails closed");
        assert_eq!(
            nsfw_of(&[f32::NAN, 1.0]),
            0.0,
            "a non-finite output fails closed"
        );
    }

    #[test]
    fn a_wide_frame_is_judged_in_pieces() {
        let square = views(&image::RgbImage::new(500, 500), RESIZE, SIDE);
        assert_eq!(square.len(), 1, "a square frame costs one pass");

        let wide = views(&image::RgbImage::new(1280, 720), RESIZE, SIDE);
        assert_eq!(wide.len(), 3, "16:9 is the centre plus both ends");

        let tall = views(&image::RgbImage::new(720, 1280), RESIZE, SIDE);
        assert_eq!(tall.len(), 3, "tall splits along its own long axis");

        for view in wide.into_iter().chain(tall) {
            assert_eq!(view.dimensions(), (SIDE as u32, SIDE as u32));
        }

        let striped = image::RgbImage::from_fn(1200, 400, |x, _| {
            image::Rgb([if x < 400 { 255 } else { 0 }, 0, 0])
        });
        let got = views(&striped, RESIZE, SIDE);
        let mean = |v: &image::RgbImage| {
            v.pixels().map(|p| u32::from(p.0[0])).sum::<u32>() / (SIDE * SIDE) as u32
        };
        assert!(
            mean(&got[1]) > mean(&got[2]),
            "the two ends are the same view"
        );
    }

    #[test]
    fn fitting_centre_crops_instead_of_squashing() {
        let wide = image::RgbImage::from_fn(1280, 720, |x, _| {
            image::Rgb([if (560..720).contains(&x) { 255 } else { 0 }, 0, 0])
        });
        let out = fit(&wide, RESIZE, SIDE);
        assert_eq!(out.dimensions(), (SIDE as u32, SIDE as u32));

        let centre = out.get_pixel(SIDE as u32 / 2, SIDE as u32 / 2).0[0];
        assert!(
            centre > 200,
            "the middle of the frame was lost, got {centre}"
        );

        assert_eq!(
            fit(&image::RgbImage::new(80, 400), RESIZE, SIDE).dimensions(),
            (SIDE as u32, SIDE as u32)
        );
        assert_eq!(
            fit(&image::RgbImage::new(1, 1), RESIZE, SIDE).dimensions(),
            (SIDE as u32, SIDE as u32)
        );
    }

    #[test]
    fn the_cheapest_thumbnail_that_needs_no_upscaling_wins() {
        assert_eq!(
            best_thumb(&[(240, 9_000), (600, 60_000), (960, 180_000)]),
            Some(1)
        );

        assert_eq!(best_thumb(&[(180, 7_000)]), Some(0));

        assert_eq!(best_thumb(&[(180, 3_000), (1440, 1_500_000)]), Some(1));
        assert_eq!(best_thumb(&[(600, 60_000), (1440, 1_500_000)]), Some(0));

        assert_eq!(best_thumb(&[(600, 90_000), (600, 40_000)]), Some(1));
        assert_eq!(best_thumb(&[]), None);
    }

    #[test]
    fn the_top_of_the_range_satisfies_every_limit() {
        let top = LIMIT_RANGE.1 as f32 / 100.0;
        for limit in LIMIT_RANGE.0..=LIMIT_RANGE.1 {
            assert_eq!(
                verdict(top, limit, false),
                Verdict::Delete,
                "a score of {top} must satisfy every limit, and {limit} rejected it"
            );
        }
        assert!(GRADE_ABOVE <= top, "nothing deletable may skip the grader");
    }

    #[test]
    fn the_escalation_band_covers_every_preset_and_nothing_else() {
        for preset in LIMIT_PRESETS {
            let target = *preset as f32 / 100.0;
            assert!(
                undecided(target),
                "a limit of {preset} sits outside the band"
            );
            assert!(undecided(target - 0.05), "just below {preset}");
            assert!(undecided(target + 0.05), "just above {preset}");
        }

        assert!(!undecided(0.037), "an ordinary photo must not escalate");
        assert!(!undecided(0.067));
        assert!(!undecided(0.95), "a settled offence must not escalate");
        assert!(!undecided(1.0));

        assert!(GRADE_ABOVE < LIMIT_RANGE.0 as f32 / 100.0 + 0.01);
        assert!(CONFIDENT > LIMIT_RANGE.1 as f32 / 100.0 - 0.05);
    }

    #[test]
    fn a_frail_frame_is_escalated_even_when_the_model_sounds_sure() {
        let escalates =
            |score: f32, frail: bool| score >= GRADE_ABOVE && (undecided(score) || frail);
        assert!(
            escalates(0.95, true),
            "a confident score on invented detail is not confident"
        );
        assert!(!escalates(0.95, false));
        assert!(
            escalates(0.50, false),
            "the unsure band escalates on its own"
        );

        assert!(!escalates(0.05, true));
    }

    #[test]
    fn a_certain_grader_overrules_an_uncertain_model() {
        let blank = Grade {
            hard: 0.00,
            sexy: 0.00,
            neutral: 1.00,
            classes: String::new(),
        };
        let bikini = Grade {
            hard: 0.05,
            sexy: 0.01,
            neutral: 0.92,
            classes: String::new(),
        };
        let bridge = Grade {
            hard: 0.10,
            sexy: 0.01,
            neutral: 0.50,
            classes: String::new(),
        };
        let strong = Grade {
            hard: 0.75,
            sexy: 0.00,
            neutral: 0.05,
            classes: String::new(),
        };
        let hardcore = Grade {
            hard: 0.97,
            sexy: 0.00,
            neutral: 0.03,
            classes: String::new(),
        };

        assert!(
            innocent(&blank, 0.879),
            "the labelled false positive must be spared"
        );

        assert!(
            innocent(&blank, 0.93),
            "a neutral grader now vetoes the known 0.93 false positive"
        );
        assert!(!innocent(&bikini, 0.93));
        assert!(!innocent(&hardcore, 0.93));

        assert!(!innocent(&blank, 0.95), "a confident first model wins");
        assert!(!innocent(&blank, CONFIDENT));

        assert!(
            confirms(GRADED_CONFIDENT, &blank),
            "a successful grader permits the .92 shortcut"
        );
        assert!(
            !allow_delete(
                innocent(&blank, 0.93),
                true,
                false,
                false,
                confirms(0.93, &blank),
                None
            ),
            "the neutral veto still protects the known .93 false positive"
        );
        assert!(
            confirms(BRIDGE_SCORE, &bridge),
            "the calibrated bridge is inclusive"
        );
        assert!(
            !confirms(BRIDGE_SCORE - 0.01, &bridge),
            "a low score cannot cross the bridge"
        );
        assert!(
            !confirms(BRIDGE_SCORE, &bikini),
            "weak grader evidence cannot cross the bridge"
        );
        assert!(
            confirms(STRONG_BRIDGE_SCORE, &strong),
            "strong hard evidence recovers a low main score"
        );
        assert!(
            !confirms(STRONG_BRIDGE_SCORE - 0.01, &strong),
            "the strong bridge has a score floor"
        );
        assert!(
            !confirms(STRONG_BRIDGE_SCORE, &bridge),
            "the strong bridge needs overwhelming hard evidence"
        );
        assert!(
            confirms(CONFIDENT, &blank),
            "the high-confidence shortcut remains intact"
        );

        assert!(
            !allow_delete(true, true, false, false, true, None),
            "innocent wins over explicit"
        );
        assert!(
            allow_delete(false, true, true, false, true, None),
            "explicit is deleted even in a soft chat"
        );
        assert!(
            !allow_delete(false, true, false, false, false, None),
            "an uncertain model needs confirmation"
        );
        assert!(
            allow_delete(false, true, false, false, true, None),
            "a confirmed model may delete"
        );
    }

    #[test]
    fn a_crop_cannot_condemn_an_innocent_frame() {
        assert!((verdict_of(0.11, &[0.93, 0.72], false) - 0.11).abs() < 1e-6);

        assert!((verdict_of(0.88, &[0.45, 0.93], false) - 0.93).abs() < 1e-6);
        assert!((verdict_of(0.95, &[0.95, 0.94], false) - 0.95).abs() < 1e-6);

        assert!((verdict_of(0.89, &[], false) - 0.89).abs() < 1e-6);
        assert!((verdict_of(0.05, &[], false) - 0.05).abs() < 1e-6);

        assert!((verdict_of(0.40, &[0.99], false) - 0.99).abs() < 1e-6);

        assert!((verdict_of(0.29, &[0.99], false) - 0.29).abs() < 1e-6);
    }

    #[test]
    fn a_crop_of_a_frail_frame_cannot_speak_at_all() {
        assert!((verdict_of(0.35, &[0.93, 0.72], true) - 0.35).abs() < 1e-6);
        assert!((verdict_of(0.90, &[0.99], true) - 0.90).abs() < 1e-6);

        assert!((verdict_of(0.95, &[], true) - 0.95).abs() < 1e-6);
        assert!((verdict_of(0.04, &[0.99], true) - 0.04).abs() < 1e-6);
    }

    #[test]
    fn a_frame_is_frail_when_it_is_small_or_smeared() {
        assert!(
            frail_of(180, 4_000.0),
            "an upscaled thumbnail is not something to delete on"
        );

        assert!(!frail_of(600, 4_000.0));

        assert!(
            frail_of(600, 12.0),
            "resolution is not the same thing as detail"
        );

        assert!(!frail_of(DETAIL_FLOOR, SHARPNESS_FLOOR));
        assert!(frail_of(DETAIL_FLOOR - 1, SHARPNESS_FLOOR));
        assert!(frail_of(DETAIL_FLOOR, SHARPNESS_FLOOR - 1.0));
    }

    #[test]
    fn sharpness_tells_detail_from_smear() {
        let side = 128u32;

        let flat = image::RgbImage::from_pixel(side, side, image::Rgb([120, 120, 120]));
        assert!(
            sharpness(&flat) < 1.0,
            "a flat frame scored {}",
            sharpness(&flat)
        );

        let smooth = image::RgbImage::from_fn(side, side, |x, y| {
            let v = ((x + y) / 2) as u8;
            image::Rgb([v, v, v])
        });
        assert!(
            sharpness(&smooth) < SHARPNESS_FLOOR,
            "a gradient is not detail"
        );

        let detailed = image::RgbImage::from_fn(side, side, |x, y| {
            let v = if (x / 3 + y / 3) % 2 == 0 { 20 } else { 235 };
            image::Rgb([v, v, v])
        });
        assert!(
            sharpness(&detailed) > SHARPNESS_FLOOR,
            "detail scored {}",
            sharpness(&detailed)
        );
        assert!(sharpness(&detailed) > sharpness(&smooth) * 10.0);

        assert_eq!(sharpness(&image::RgbImage::new(1, 1)), 0.0);
        assert_eq!(sharpness(&image::RgbImage::new(2, 40)), 0.0);
    }

    #[test]
    fn a_frail_frame_needs_a_second_witness() {
        let seen = Grade {
            hard: 0.85,
            sexy: 0.10,
            neutral: 0.02,
            classes: String::new(),
        };
        let blank = Grade {
            hard: 0.00,
            sexy: 0.00,
            neutral: 1.00,
            classes: String::new(),
        };
        let glimpse = Grade {
            hard: EXPLICIT_FLOOR,
            sexy: 0.10,
            neutral: 0.30,
            classes: String::new(),
        };

        assert!(seen.corroborates());
        assert!(glimpse.corroborates(), "the floor is inclusive");
        assert!(
            !blank.corroborates(),
            "recognising nothing is not a second witness"
        );

        let weak = |frail: bool, grade: &Grade| frail && !grade.corroborates();
        assert!(
            weak(true, &blank),
            "a frail frame the grader could not read is not deleted"
        );
        assert!(
            !weak(true, &seen),
            "a frail frame both models agree on still goes"
        );
        assert!(
            !weak(false, &blank),
            "a frame with real detail needs no second witness"
        );

        assert!(
            !allow_delete(false, true, false, true, true, None),
            "weak is never deleted"
        );
        assert!(allow_delete(false, true, false, false, true, None));
        assert!(
            !allow_delete(true, true, false, false, true, None),
            "innocent still wins"
        );
    }

    #[test]
    fn the_grader_folds_every_view_it_looks_at() {
        let blank = [0.00, 0.00, 1.00, 0.00, 0.00];
        let hardcore = [0.00, 0.00, 0.03, 0.97, 0.00];

        let centre = fold(None, &blank, "centre".to_owned());
        assert!(
            !centre.corroborates(),
            "a blank grade must not end the loop"
        );

        let both = fold(Some(centre), &hardcore, "end".to_owned());
        assert!(both.corroborates());
        assert!(
            (both.hard - 0.97).abs() < 1e-6,
            "the worst view has to win on hard"
        );
        assert!(
            (both.neutral - 0.03).abs() < 1e-6,
            "neutral has to be the least innocent view, or the veto spares a real offence"
        );
        assert!(!innocent(&both, 0.90), "this must still be deletable");

        let reversed = fold(
            Some(fold(None, &hardcore, "end".to_owned())),
            &blank,
            "centre".to_owned(),
        );
        assert!((reversed.hard - both.hard).abs() < 1e-6);
        assert!((reversed.neutral - both.neutral).abs() < 1e-6);

        let all_blank = fold(
            Some(fold(None, &blank, String::new())),
            &blank,
            String::new(),
        );
        assert!((all_blank.neutral - 1.00).abs() < 1e-6);
        assert!(
            innocent(&all_blank, 0.879),
            "the labelled false positive is still spared"
        );
    }

    #[test]
    fn the_grader_spares_only_on_positive_evidence() {
        let unrecognised = [0.02, 0.05, 0.92, 0.00, 0.01];

        let blank = [0.00, 0.00, 1.00, 0.00, 0.00];

        let hardcore = [0.00, 0.00, 0.03, 0.97, 0.00];

        let revealing = [0.05, 0.01, 0.14, 0.10, 0.70];

        let unsure = [0.20, 0.05, 0.25, 0.25, 0.25];
        let explicit = |p: &[f32; 5]| explicit_of(hard_of(p), p[4]);

        assert!(
            explicit(&unrecognised),
            "a grade of «neutral» is not evidence of innocence"
        );
        assert!(
            explicit(&blank),
            "a blank grade must not overrule the first model"
        );
        assert!(explicit(&hardcore));
        assert!(explicit(&unsure), "an unsure grader must not spare");
        assert!(
            !explicit(&revealing),
            "positively revealing is what may be spared"
        );

        let judged = |innocent: bool, explicit: bool| Judgement {
            score: 0.9,
            innocent,
            explicit,
            confirmed: true,
            weak: false,

            arbiter: None,
        };
        assert!(
            spared(judged(false, false), true, 0, 0),
            "revealing, chat opted into softness"
        );
        assert!(
            !spared(judged(false, false), false, 0, 0),
            "revealing, chat did not opt in"
        );
        assert!(
            !spared(judged(false, true), true, 0, 0),
            "explicit is never spared"
        );
        assert!(!spared(judged(false, true), false, 0, 0));

        assert!(
            spared(judged(true, true), false, 0, 0),
            "a certain grader overrules a hesitant model"
        );
        assert!(spared(judged(true, false), true, 0, 0));

        assert!(spared(
            Judgement {
                weak: true,
                ..judged(false, true)
            },
            false,
            0,
            0
        ));
    }

    #[test]
    fn the_grader_loads_and_keeps_its_class_order() {
        assert_eq!(
            GRADE_CLASSES,
            ["drawings", "hentai", "neutral", "porn", "sexy"]
        );
        let pool = grader().expect("the bundled grader must load");
        let view = image::RgbImage::from_fn(GRADE_SIDE as u32, GRADE_SIDE as u32, |x, y| {
            image::Rgb([((x + y) % 255) as u8, (x % 255) as u8, (y % 255) as u8])
        });
        let pixels = pixels_of(&view, GRADE_SIDE, |value| f32::from(value) / 255.0);
        let logits = pool
            .with(|session| {
                run(
                    session,
                    vec![3, GRADE_SIDE as i64, GRADE_SIDE as i64],
                    pixels,
                )
            })
            .expect("a forward pass");
        assert_eq!(logits.len(), GRADE_CLASSES.len());
        let p = probabilities(&logits);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_model_loads_and_the_normalisation_is_the_documented_one() {
        let pool = model().expect("the bundled model must load");
        let view = image::RgbImage::from_fn(SIDE as u32, SIDE as u32, |x, y| {
            let v = ((x * 3 + y) % 255) as u8;
            image::Rgb([v, v.wrapping_add(37), v.wrapping_add(74)])
        });

        let judge = |scale: fn(u8) -> f32| {
            let logits = pool
                .with(|session| {
                    run(
                        session,
                        vec![1, 3, SIDE as i64, SIDE as i64],
                        pixels_of(&view, SIDE, scale),
                    )
                })
                .expect("a pass");
            assert_eq!(logits.len(), CLASSES.len());
            probabilities(&logits)
        };

        let correct = judge(|value| f32::from(value) / 127.5 - 1.0);
        let sum: f32 = correct.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-3,
            "probabilities must sum to one, got {sum}"
        );
        assert!(
            correct[0] < 0.5,
            "a bland gradient scored {} as an offence",
            correct[0]
        );

        let unscaled = judge(f32::from);
        assert!(
            (unscaled[0] - correct[0]).abs() > 0.01,
            "raw and normalised input agree, so the normalisation is not reaching the model"
        );
    }

    #[test]
    #[ignore]
    fn measures_pooled_new_image_inference() {
        let pool = model().expect("the bundled model must load");
        let view = image::RgbImage::from_fn(SIDE as u32, SIDE as u32, |x, y| {
            image::Rgb([((x * 3 + y) % 255) as u8, (x % 255) as u8, (y % 255) as u8])
        });
        let pass = || {
            pool.with(|session| look_with(session, &view))
                .expect("a forward pass")
        };

        let started = std::time::Instant::now();
        for _ in 0..8 {
            pass();
        }
        let serial = started.elapsed();

        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8).map(|_| scope.spawn(pass)).collect();
            for handle in handles {
                handle.join().expect("inference thread must finish");
            }
        });
        let pooled = started.elapsed();
        println!("uncached image inference: serial={serial:?}, pooled={pooled:?}");
    }

    #[test]
    #[ignore]
    fn measures_two_hundred_uncached_image_passes() {
        let pool = model().expect("the bundled model must load");
        let images: Vec<_> = (0..200)
            .map(|seed| {
                image::RgbImage::from_fn(SIDE as u32, SIDE as u32, |x, y| {
                    image::Rgb([
                        ((x * 3 + y + seed) % 255) as u8,
                        ((x + seed * 5) % 255) as u8,
                        ((y + seed * 7) % 255) as u8,
                    ])
                })
            })
            .collect();
        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            let handles: Vec<_> = images
                .iter()
                .map(|image| {
                    scope.spawn(move || {
                        pool.with(|session| look_with(session, image))
                            .expect("a forward pass")
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("inference thread must finish");
            }
        });
        let elapsed = started.elapsed();
        println!(
            "200 uncached image passes: elapsed={elapsed:?}, throughput={:.2}/sec",
            200.0 / elapsed.as_secs_f64()
        );
    }

    #[test]
    fn hypervision_is_binary_at_the_product_boundary() {
        let keys: Vec<&str> = super::super::setting::SETTINGS
            .iter()
            .map(|setting| setting.key)
            .collect();
        assert!(!keys.contains(&"nsfw_live"));
        assert!(!keys.contains(&"nsfw_soft"));
        assert!(!keys.contains(&"nsfw_lim"));
    }
}
