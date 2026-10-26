use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::io::Cursor;
use std::io::Read;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use grammers_client::InvocationError;
use grammers_client::media::{Document, Downloadable, Media, PhotoSize};
use grammers_client::message::Message;
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;
use grammers_client::tl::Serializable;
use image::{AnimationDecoder, ImageDecoder};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::locks::View;
use super::{Ctx, VisionRoute, VisionSource};

mod raster_kind;
use raster_kind::{RasterKind, raster_kind};

mod invoice_web;

#[path = "nsfw_tiff.rs"]
mod tiff_pages;

#[path = "nsfw_multipage.rs"]
mod multipage;

#[allow(unsafe_code)]
mod tgs;

#[allow(unsafe_code)]
pub mod trim;

pub const LOCK: &str = "nsfw";

const OCR_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);
const MEDIA_FETCH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);
const IMAGE_DECODE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
const TGS_RENDER_QUEUE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
const THUMBNAIL_BYTES_MAX: usize = 16 * 1024 * 1024;
pub(crate) const ANIMATED_BYTES_MAX: usize = 64 * 1024 * 1024;
pub(crate) const TGS_BYTES_MAX: usize = tgs::MAX_COMPRESSED_BYTES;
const IMAGE_DIMENSION_MAX: u32 = 8_192;
const IMAGE_PIXEL_MAX: u64 = 16_000_000;
const IMAGE_DECODE_BYTES_MAX: u64 = 64 * 1024 * 1024;

pub const DEFAULT_LIMIT: u32 = 50;
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

const STACK_VETO: f32 = 0.40;

#[derive(Clone, Copy, Debug)]
pub struct Arbiter {
    pub explicit: f32,
    pub revealing: f32,
    pub head: f32,
    pub stack: f32,
}

impl Arbiter {
    fn agrees(&self) -> bool {
        self.stack >= STACK_VETO
            && (self.head_sure() || (self.explicit >= AGREE && self.head >= HEAD_VETO))
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
        stack: 1.0,
    }
}

fn stack_features(
    score: f32,
    peak: f32,
    frail: bool,
    grade: Option<&Grade>,
    arbiter: &Arbiter,
) -> [f32; 12] {
    let head = arbiter.head.clamp(1e-6, 1.0 - 1e-6);
    let head_logit = (head / (1.0 - head)).ln();
    let (hard, neutral, drawn) = grade.map_or((0.0, 0.0, 0.0), |g| (g.hard, g.neutral, g.drawn));
    [
        head_logit,
        score,
        peak,
        hard,
        neutral,
        drawn,
        arbiter.explicit * 20.0,
        if frail { 1.0 } else { 0.0 },
        head_logit * drawn,
        head_logit * hard,
        hard * drawn,
        score * hard,
    ]
}

fn stack_of(features: &[f32; 12]) -> f32 {
    use super::nsfw_stack_weights::{BIAS, WEIGHTS};
    let z = super::vision::dot(features, &WEIGHTS) + BIAS;
    1.0 / (1.0 + (-z).exp())
}

const RUNTIME: &[u8] = include_bytes!("../../assets/libonnxruntime.so.1.20.1");
const REVIEWED_RUNTIME_SHA256: &str =
    "a5faaf78a37590d3fe640f887620e74f6022d34550172b91ad2131bf0ad77d64";

const DEFAULT_INFER_THREADS: usize = 2;
const DEFAULT_INFER_SESSIONS: usize = 2;

#[derive(Debug)]
struct ModelConfig {
    infer_threads: usize,
    infer_sessions: usize,
    runtime_path: Option<std::path::PathBuf>,
    model_files: Option<std::path::PathBuf>,
    #[cfg(feature = "cuda")]
    use_cuda: bool,
    #[cfg(feature = "cuda")]
    cuda_device: i32,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            infer_threads: DEFAULT_INFER_THREADS,
            infer_sessions: DEFAULT_INFER_SESSIONS,
            runtime_path: None,
            model_files: None,
            #[cfg(feature = "cuda")]
            use_cuda: false,
            #[cfg(feature = "cuda")]
            cuda_device: 0,
        }
    }
}

static MODEL_CONFIG: std::sync::OnceLock<ModelConfig> = std::sync::OnceLock::new();
static RUNTIME_STARTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn configure_from_environment() -> Result<(), String> {
    let configured_runtime_path = configured_path("ORT_LIBRARY_PATH")?;
    if let Some(path) = configured_runtime_path.as_deref()
        && !path.is_file()
    {
        return Err(format!(
            "ORT_LIBRARY_PATH does not point to a file: {}",
            path.display()
        ));
    }
    let model_files = configured_path("VISION_FILES")?;
    if let Some(path) = model_files.as_deref()
        && !path.is_dir()
    {
        return Err(format!(
            "VISION_FILES does not point to a directory: {}",
            path.display()
        ));
    }
    let use_cuda = configured_bool("ORT_USE_CUDA", false)?;
    #[cfg(not(feature = "cuda"))]
    if use_cuda {
        return Err("ORT_USE_CUDA requires building groupbot with the cuda feature".to_owned());
    }
    #[cfg(feature = "cuda")]
    let cuda_device = configured_i32("ORT_CUDA_DEVICE", 0, 0, i32::MAX)?;
    #[cfg(not(feature = "cuda"))]
    configured_i32("ORT_CUDA_DEVICE", 0, 0, i32::MAX)?;
    let config = ModelConfig {
        infer_threads: super::configured_usize("NSFW_INFER_THREADS", DEFAULT_INFER_THREADS, 1, 64)?,
        infer_sessions: super::configured_usize(
            "NSFW_INFER_SESSIONS",
            DEFAULT_INFER_SESSIONS,
            1,
            256,
        )?,
        runtime_path: configured_runtime_path,
        model_files,
        #[cfg(feature = "cuda")]
        use_cuda,
        #[cfg(feature = "cuda")]
        cuda_device,
    };
    MODEL_CONFIG
        .set(config)
        .map_err(|_| "model configuration was initialized more than once".to_owned())?;
    let path = runtime_path().map_err(|error| format!("ONNX Runtime path: {error}"))?;
    initialize_runtime(&path).map_err(|error| {
        format!(
            "could not initialize ONNX Runtime from {}: {error}",
            path.display()
        )
    })?;
    drop(builder().map_err(|error| format!("ONNX Runtime configuration: {error}"))?);
    RUNTIME_STARTED
        .set(true)
        .map_err(|_| "ONNX Runtime was initialized before model configuration".to_owned())?;
    Ok(())
}

#[cfg(test)]
pub(super) fn configure_model_tests() -> Result<(), String> {
    static CONFIG: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    CONFIG.get_or_init(configure_from_environment).clone()
}

fn configured_path(name: &str) -> Result<Option<std::path::PathBuf>, String> {
    match std::env::var_os(name) {
        None => Ok(None),
        Some(value) if value.is_empty() => Err(format!("{name} must not be empty")),
        Some(value) => Ok(Some(std::path::PathBuf::from(value))),
    }
}

fn configured_bool(name: &str, default: bool) -> Result<bool, String> {
    match std::env::var(name) {
        Ok(value) => parse_bool(name, &value),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not valid Unicode")),
    }
}

fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(format!(
            "{name} must be true, false, 1, or 0, got {value:?}"
        )),
    }
}

fn configured_i32(name: &str, default: i32, minimum: i32, maximum: i32) -> Result<i32, String> {
    let value = match std::env::var(name) {
        Ok(value) => parse_bounded_i32(name, &value, minimum, maximum)?,
        Err(std::env::VarError::NotPresent) => default,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{name} is not valid Unicode"));
        }
    };
    if !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "{name} must be in {minimum}..={maximum}, got {value}"
        ));
    }
    Ok(value)
}

fn parse_bounded_i32(name: &str, value: &str, minimum: i32, maximum: i32) -> Result<i32, String> {
    let value = value.parse::<i32>().map_err(|_| {
        format!("{name} must be an integer in {minimum}..={maximum}, got {value:?}")
    })?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "{name} must be in {minimum}..={maximum}, got {value}"
        ));
    }
    Ok(value)
}

#[cfg(not(test))]
fn model_config() -> &'static ModelConfig {
    MODEL_CONFIG
        .get()
        .expect("main initializes immutable model configuration before handlers start")
}

#[cfg(test)]
fn model_config() -> &'static ModelConfig {
    MODEL_CONFIG.get_or_init(ModelConfig::default)
}

fn infer_threads() -> usize {
    model_config().infer_threads
}

pub fn infer_sessions() -> usize {
    model_config().infer_sessions
}

pub(super) fn model_files_dir() -> Option<&'static std::path::Path> {
    model_config().model_files.as_deref()
}

pub type Session = std::sync::Mutex<ort::session::Session>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    Artifact(String),
    Runtime(String),
    ModelContract(String),
    Poisoned(&'static str),
}

impl ModelError {
    pub fn artifact(what: impl Into<String>) -> Self {
        Self::Artifact(what.into())
    }

    fn runtime(what: impl Into<String>) -> Self {
        Self::Runtime(what.into())
    }

    pub fn contract(what: impl Into<String>) -> Self {
        Self::ModelContract(what.into())
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Artifact(detail) => write!(formatter, "model artifact: {detail}"),
            Self::Runtime(detail) => write!(formatter, "model runtime: {detail}"),
            Self::ModelContract(detail) => write!(formatter, "model contract: {detail}"),
            Self::Poisoned(what) => write!(formatter, "model mutex poisoned: {what}"),
        }
    }
}

impl std::error::Error for ModelError {}

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

static RUNTIME_STAGING_SEQUENCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn file_matches(path: &std::path::Path, expected: &[u8]) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    let Ok(expected_len) = u64::try_from(expected.len()) else {
        return false;
    };
    if metadata.len() != expected_len {
        return false;
    }
    let mut reader = std::io::BufReader::new(file);
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => return false,
        };
        hash.update(&buffer[..read]);
    }
    hash.finalize() == Sha256::digest(expected)
}

fn publish_embedded(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    if file_matches(path, bytes) {
        return Ok(());
    }
    let sequence = RUNTIME_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = path.with_extension(format!("so.{}.{sequence}.partial", std::process::id()));
    std::fs::write(&staging, bytes).map_err(|error| {
        format!(
            "could not stage ONNX Runtime at {}: {error}",
            staging.display()
        )
    })?;
    if let Err(publish_error) = std::fs::rename(&staging, path) {
        if !file_matches(path, bytes) {
            let _cleanup_result = std::fs::remove_file(&staging);
            return Err(format!(
                "could not publish ONNX Runtime from {} to {}: {publish_error}",
                staging.display(),
                path.display()
            ));
        }
        if let Err(error) = std::fs::remove_file(&staging)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "nsfw: could not remove losing runtime staging file {}: {error}",
                staging.display()
            );
        }
    }
    if !file_matches(path, bytes) {
        return Err(format!(
            "published ONNX Runtime at {} failed integrity verification",
            path.display()
        ));
    }
    Ok(())
}

fn embedded_runtime_filename(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("libonnxruntime.so.1.20.1-{digest:x}")
}

fn runtime_path() -> Result<std::path::PathBuf, String> {
    if let Some(path) = model_config().runtime_path.as_ref() {
        return Ok(path.clone());
    }
    let actual = format!("{:x}", Sha256::digest(RUNTIME));
    if actual != REVIEWED_RUNTIME_SHA256 {
        return Err(format!(
            "embedded ONNX Runtime SHA-256 mismatch: expected {REVIEWED_RUNTIME_SHA256}, got {actual}"
        ));
    }
    let mut path = std::env::current_exe()
        .map_err(|error| format!("could not locate the running executable: {error}"))?;
    path.pop();
    path.push(embedded_runtime_filename(RUNTIME));
    publish_embedded(&path, RUNTIME)?;
    Ok(path)
}

fn initialize_runtime(path: &std::path::Path) -> Result<(), String> {
    let environment = ort::init_from(path).map_err(|error| error.to_string())?;
    if environment.commit() {
        Ok(())
    } else {
        Err("another ONNX Runtime environment was already committed".to_owned())
    }
}

fn builder() -> Result<ort::session::builder::SessionBuilder, String> {
    builder_with(infer_threads())
}

fn builder_with(threads: usize) -> Result<ort::session::builder::SessionBuilder, String> {
    let mut builder = ort::session::Session::builder().map_err(|error| error.to_string())?;
    builder = builder
        .with_intra_threads(threads)
        .map_err(|error| error.to_string())?;
    builder = builder
        .with_device_allocated_initializers()
        .map_err(|error| error.to_string())?;
    #[cfg(feature = "cuda")]
    if model_config().use_cuda {
        builder = builder
            .with_execution_providers([ort::ep::CUDA::default()
                .with_device_id(model_config().cuda_device)
                .build()
                .error_on_failure()])
            .map_err(|error| error.to_string())?;
    }
    Ok(builder)
}

fn started() -> bool {
    *RUNTIME_STARTED.get_or_init(|| match runtime_path() {
        Ok(path) => match initialize_runtime(&path) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("nsfw: the runtime would not start, the lock is inert: {e}");
                false
            }
        },
        Err(error) => {
            eprintln!("nsfw: {error}; model-backed moderation is inert");
            false
        }
    })
}

pub fn open_path(path: &std::path::Path, what: &str) -> Option<Session> {
    open_path_with(path, what, infer_threads())
}

pub fn open_path_with(path: &std::path::Path, what: &str, threads: usize) -> Option<Session> {
    open_path_configured(path, what, threads, &[])
}

pub fn open_path_configured(
    path: &std::path::Path,
    what: &str,
    threads: usize,
    entries: &[(&str, &str)],
) -> Option<Session> {
    if !started() {
        return None;
    }
    let built = || -> Result<ort::session::Session, String> {
        let mut builder = builder_with(threads)?;
        for (key, value) in entries {
            builder = builder.with_config_entry(*key, *value).map_err(|e| e.to_string())?;
        }
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
        let mut builder = match builder() {
            Ok(builder) => builder,
            Err(error) => {
                eprintln!("nsfw: the {what} pool builder failed: {error}");
                return None;
            }
        };
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
        let mut builder = match builder() {
            Ok(builder) => builder,
            Err(error) => {
                eprintln!("nsfw: the {what} pool builder failed: {error}");
                return None;
            }
        };
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

pub(super) fn initialize_pool_once_retry<'a, T>(
    cell: &'a std::sync::OnceLock<T>,
    gate: &std::sync::Mutex<Option<std::time::Instant>>,
    retry_delay: std::time::Duration,
    load: impl FnOnce() -> Option<T>,
) -> Option<&'a T> {
    if let Some(pool) = cell.get() {
        return Some(pool);
    }
    let mut failed_at = gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(pool) = cell.get() {
        return Some(pool);
    }
    if failed_at
        .as_ref()
        .is_some_and(|at| at.elapsed() < retry_delay)
    {
        return None;
    }
    let Some(pool) = load() else {
        *failed_at = Some(std::time::Instant::now());
        return None;
    };
    *failed_at = None;
    Some(cell.get_or_init(|| pool))
}

fn model() -> Option<&'static SessionPool> {
    static MODEL_CELL: std::sync::OnceLock<SessionPool> = std::sync::OnceLock::new();
    static GATE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    initialize_pool_once_retry(
        &MODEL_CELL,
        &GATE,
        std::time::Duration::from_secs(30),
        || open_pool(MODEL, "model", infer_sessions()),
    )
}

fn grader() -> Option<&'static SessionPool> {
    static GRADER: std::sync::OnceLock<SessionPool> = std::sync::OnceLock::new();
    static GATE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    initialize_pool_once_retry(&GRADER, &GATE, std::time::Duration::from_secs(30), || {
        open_pool(GRADE, "grader", infer_sessions())
    })
}

pub fn capacity_probe() -> Result<usize, ModelError> {
    let pools = [model(), grader()]
        .into_iter()
        .filter(Option::is_some)
        .count();
    let image = image::RgbImage::from_pixel(SIDE as u32, SIDE as u32, image::Rgb([127; 3]));
    for _ in 0..2 * infer_sessions() {
        look(&image)?;
        grade(&image)?;
    }
    Ok(pools)
}

pub fn run(session: &Session, shape: Vec<i64>, pixels: Vec<f32>) -> Result<Vec<f32>, ModelError> {
    run_shaped(session, shape, pixels).map(|(_, values)| values)
}

pub fn run_embedding(
    session: &Session,
    shape: Vec<i64>,
    pixels: Vec<f32>,
) -> Result<Vec<f32>, ModelError> {
    let input = ort::value::Value::from_array((shape, pixels))
        .map_err(|error| ModelError::contract(format!("embedding input tensor: {error}")))?;
    let mut session = session
        .lock()
        .map_err(|_| ModelError::Poisoned("embedding session"))?;
    let output = session
        .run(ort::inputs![input])
        .map_err(|error| ModelError::runtime(format!("embedding forward pass: {error}")))?;
    for (_, value) in output {
        let Ok((shape, values)) = value.try_extract_tensor::<f32>() else {
            continue;
        };
        let vector_shape =
            shape[..] == [super::vision::DIM as i64] || shape[..] == [1, super::vision::DIM as i64];
        if !vector_shape || values.len() != super::vision::DIM {
            continue;
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(ModelError::contract(
                "embedding output contains a non-finite value",
            ));
        }
        return Ok(values.to_vec());
    }
    Err(ModelError::contract(format!(
        "embedding graph exposed no f32 vector shaped [{0}] or [1, {0}]",
        super::vision::DIM
    )))
}

pub fn run_shaped(
    session: &Session,
    shape: Vec<i64>,
    pixels: Vec<f32>,
) -> Result<(Vec<i64>, Vec<f32>), ModelError> {
    let input = ort::value::Value::from_array((shape, pixels))
        .map_err(|error| ModelError::contract(format!("input tensor: {error}")))?;
    let mut session = session
        .lock()
        .map_err(|_| ModelError::Poisoned("inference session"))?;
    let output = session
        .run(ort::inputs![input])
        .map_err(|error| ModelError::runtime(format!("forward pass: {error}")))?;
    let (_, value) = output
        .into_iter()
        .next()
        .ok_or_else(|| ModelError::contract("graph exposed no output tensor"))?;
    let (shape, values) = value
        .try_extract_tensor::<f32>()
        .map_err(|error| ModelError::contract(format!("output is not an f32 tensor: {error}")))?;
    if values.iter().any(|value| !value.is_finite()) {
        return Err(ModelError::contract(
            "output tensor contains a non-finite value",
        ));
    }
    Ok((shape.to_vec(), values.to_vec()))
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

fn probabilities(logits: &[f32]) -> Result<Vec<f32>, ModelError> {
    if logits.is_empty() || logits.iter().any(|logit| !logit.is_finite()) {
        return Err(ModelError::contract("logits are empty or non-finite"));
    }
    let top = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp: Vec<f32> = logits.iter().map(|x| (x - top).exp()).collect();
    let sum: f32 = exp.iter().sum();
    if !sum.is_finite() || sum <= 0.0 {
        return Err(ModelError::contract("softmax denominator is invalid"));
    }
    Ok(exp.into_iter().map(|x| x / sum).collect())
}

fn nsfw_of(logits: &[f32]) -> Result<f32, ModelError> {
    probabilities(logits)?
        .first()
        .copied()
        .ok_or_else(|| ModelError::contract("classifier returned no classes"))
}

fn breakdown(labels: &[&str], logits: &[f32]) -> Result<String, ModelError> {
    let p = probabilities(logits)?;
    Ok(labels
        .iter()
        .zip(&p)
        .map(|(label, value)| format!("{label} {value:.2}"))
        .collect::<Vec<_>>()
        .join(" "))
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
    pub view_scores: [Option<f32>; 3],
    pub frail: bool,
    pub note: String,
    pub complete: bool,
}

fn judge(session: &Session, view: &image::RgbImage) -> Result<(f32, bool), ModelError> {
    let shape = || vec![1, 3, SIDE as i64, SIDE as i64];
    let scale = |value: u8| f32::from(value) / 127.5 - 1.0;

    let logits = run(session, shape(), pixels_of(view, SIDE, scale))?;
    if logits.len() != CLASSES.len() {
        return Err(ModelError::contract(format!(
            "NSFW classifier returned {} classes, expected {}",
            logits.len(),
            CLASSES.len()
        )));
    }
    let first = nsfw_of(&logits)?;
    if !undecided(first) {
        return Ok((first, true));
    }
    let mirror = image::imageops::flip_horizontal(view);
    let logits = match run(session, shape(), pixels_of(&mirror, SIDE, scale)) {
        Ok(logits) => logits,
        Err(error) => {
            log::warn!("nsfw: mirrored second pass failed; retaining first score: {error}");
            return Ok((first, false));
        }
    };
    if logits.len() != CLASSES.len() {
        log::warn!(
            "nsfw: mirrored second pass returned {} classes, expected {}; retaining first score",
            logits.len(),
            CLASSES.len()
        );
        return Ok((first, false));
    }
    Ok(((first + nsfw_of(&logits)?) / 2.0, true))
}

fn look(image: &image::RgbImage) -> Result<Option<Look>, ModelError> {
    let Some(pool) = model() else {
        return Ok(None);
    };
    pool.with(|session| look_with(session, image)).map(Some)
}

fn look_with(session: &Session, image: &image::RgbImage) -> Result<Look, ModelError> {
    let detail = image.width().min(image.height());
    let sharpness = sharpness(image);
    let frail = frail_of(detail, sharpness);

    let mut scores: Vec<f32> = Vec::new();
    let mut complete = true;
    for view in views(image, RESIZE, SIDE) {
        let (score, full) = judge(session, &view)?;
        scores.push(score);
        complete &= full;
    }
    let (&whole, ends) = scores
        .split_first()
        .ok_or_else(|| ModelError::contract("NSFW view set is empty"))?;
    let note = format!(
        "views {} detail {detail} sharp {sharpness:.0}{}",
        scores
            .iter()
            .map(|s| format!("{s:.2}"))
            .collect::<Vec<_>>()
            .join("/"),
        if frail { " frail" } else { "" }
    );
    Ok(Look {
        score: verdict_of(whole, ends, frail),
        peak: scores.iter().copied().fold(whole, f32::max),
        view_scores: [
            scores.first().copied(),
            scores.get(1).copied(),
            scores.get(2).copied(),
        ],
        frail,
        note,
        complete,
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
    pub drawn: f32,
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
    let drawn = seen
        .as_ref()
        .map_or(p[0] + p[1], |seen| seen.drawn.max(p[0] + p[1]));
    match seen {
        Some(seen) if seen.hard >= hard => Grade {
            neutral,
            drawn,
            ..seen
        },
        _ => Grade {
            hard,
            sexy: p[4],
            neutral,
            drawn,
            classes,
        },
    }
}

fn grade(image: &image::RgbImage) -> Result<Option<Grade>, ModelError> {
    let Some(pool) = grader() else {
        return Ok(None);
    };
    pool.with(|session| grade_with(session, image)).map(Some)
}

fn grade_with(session: &Session, image: &image::RgbImage) -> Result<Grade, ModelError> {
    grade_with_infer(image, |view| {
        let pixels = pixels_of(view, GRADE_SIDE, |value| f32::from(value) / 255.0);
        run(
            session,
            vec![3, GRADE_SIDE as i64, GRADE_SIDE as i64],
            pixels,
        )
    })
}

fn grade_with_infer(
    image: &image::RgbImage,
    mut infer: impl FnMut(&image::RgbImage) -> Result<Vec<f32>, ModelError>,
) -> Result<Grade, ModelError> {
    let mut seen: Option<Grade> = None;
    let mut notes: Vec<String> = Vec::new();
    for view in views(image, GRADE_RESIZE, GRADE_SIDE) {
        let logits = infer(&view)?;
        if logits.len() != GRADE_CLASSES.len() {
            return Err(ModelError::contract(format!(
                "NSFW grader returned {} classes, expected {}",
                logits.len(),
                GRADE_CLASSES.len()
            )));
        }
        let p = probabilities(&logits)?;
        notes.push(breakdown(&GRADE_CLASSES, &logits)?);
        let folded = fold(seen, &p, String::new());
        let done = folded.hard >= STRONG_BRIDGE_FLOOR;
        seen = Some(folded);
        if done {
            break;
        }
    }
    seen.map(|grade| Grade {
        classes: notes.join(" · "),
        ..grade
    })
    .ok_or_else(|| ModelError::contract("NSFW grader view set is empty"))
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

fn is_avif_document(document: &Document) -> bool {
    matches!(
        document.mime_type(),
        Some("image/avif" | "image/avif-sequence")
    ) || document.name().is_some_and(|name| {
        name.rsplit('.')
            .next()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("avif"))
    })
}

const VIDEO_FETCH_MAX: usize = 24 * 1024 * 1024;

fn is_animated_document(document: &Document) -> bool {
    let fetchable = document
        .size()
        .is_none_or(|size| size <= VIDEO_FETCH_MAX);
    is_tgs_document(document)
        || document.is_animated()
        || is_avif_document(document)
        || matches!(
            document.mime_type(),
            Some(
                "image/gif"
                    | "image/tiff"
                    | "image/x-tiff"
                    | "image/x-icon"
                    | "image/vnd.microsoft.icon"
                    | "image/x-portable-anymap"
                    | "image/x-portable-bitmap"
                    | "image/x-portable-graymap"
                    | "image/x-portable-pixmap"
                    | "image/x-portable-arbitrarymap"
            )
        )
        || matches!(document.mime_type(), Some("image/apng"))
        || (fetchable
            && document
                .mime_type()
                .is_some_and(|mime| mime.starts_with("video/")))
        || document.name().is_some_and(|name| {
            name.rsplit('.').next().is_some_and(|extension| {
                let video = ["mp4", "webm"]
                    .iter()
                    .any(|video| extension.eq_ignore_ascii_case(video));
                (fetchable && video)
                    || [
                        "gif", "apng", "tif", "tiff", "ico", "pnm", "pbm", "pgm", "ppm", "pam",
                    ]
                    .iter()
                    .any(|animated| extension.eq_ignore_ascii_case(animated))
            })
        })
}

fn is_tgs_document(document: &Document) -> bool {
    document.mime_type() == Some("application/x-tgsticker")
        || document.name().is_some_and(|name| {
            name.rsplit('.')
                .next()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tgs"))
        })
}

pub(super) fn animated_document(media: &Media) -> Option<&Document> {
    match media {
        Media::Document(document) if is_animated_document(document) => Some(document),
        Media::Sticker(sticker) if is_animated_document(&sticker.document) => {
            Some(&sticker.document)
        }
        _ => None,
    }
}

fn is_ordinary_raster_document(document: &Document) -> bool {
    let named_raster = document.name().is_some_and(|name| {
        name.rsplit('.').next().is_some_and(|extension| {
            ["png", "webp", "jpg", "jpeg", "jpe", "jfif"]
                .iter()
                .any(|raster| extension.eq_ignore_ascii_case(raster))
        })
    });
    !is_animated_document(document)
        && (matches!(
            document.mime_type(),
            Some("image/png" | "image/webp" | "image/jpeg")
        ) || named_raster)
}

fn ordinary_raster_document(media: &Media) -> Option<&Document> {
    let document = match media {
        Media::Document(document) => document,
        Media::Sticker(sticker) => &sticker.document,
        _ => return None,
    };
    is_ordinary_raster_document(document).then_some(document)
}

#[derive(Debug, PartialEq, Eq)]
struct RasterCacheRoute {
    probe: bool,
    animated: bool,
    cache_ready: bool,
}

fn raster_cache_route(
    metadata_animated: bool,
    raster_candidate: bool,
    known_animated: Option<bool>,
    personalized_cover: bool,
) -> RasterCacheRoute {
    let probe = raster_candidate && known_animated.is_none();
    RasterCacheRoute {
        probe,
        animated: metadata_animated || known_animated == Some(true),
        cache_ready: !personalized_cover && !probe,
    }
}

fn is_still_image_document(document: &Document) -> bool {
    !is_animated_document(document)
        && (document
            .mime_type()
            .is_some_and(|mime| mime.starts_with("image/"))
            || document.name().is_some_and(|name| {
                name.rsplit('.').next().is_some_and(|extension| {
                    ["jpg", "jpeg", "jpe", "jfif"]
                        .iter()
                        .any(|jpeg| extension.eq_ignore_ascii_case(jpeg))
                })
            })
            || is_ordinary_raster_document(document))
}

pub(super) fn still_image_without_thumb(media: &Media, has_thumb: bool) -> Option<Document> {
    if has_thumb {
        return None;
    }
    match media {
        Media::Document(document) if is_still_image_document(document) => Some(document.clone()),
        Media::Sticker(sticker) if is_still_image_document(&sticker.document) => {
            Some(sticker.document.clone())
        }
        _ => None,
    }
}

const MAX_ANIMATED_FRAMES: usize = 12;
const MAX_ANIMATED_SIDE: u32 = 768;
const ANIMATED_SIGNATURE_SIDE: u32 = 24;
const MAX_VIDEO_SIGNATURE_FRAMES: usize = 512;
const MAX_VIDEO_SIGNATURE_FPS: f64 = 60.0;
const MAX_ANIMATED_OCR_FRAMES: usize = 4;
const MAX_ANIMATED_OCR_EXTRA_FRAMES: usize = 2;
static TGS_RENDER_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

const MAX_NATIVE_GIF_SCAN_FRAMES: usize = 512;

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

fn sample_novel_indices(signatures: &[Vec<u8>], limit: usize) -> Vec<usize> {
    let total = signatures.len();
    if total <= limit || limit <= 1 {
        return sample_indices(total, limit);
    }
    let mut selected = sample_indices(total, (limit / 2).max(2));
    while selected.len() < limit {
        let mut best: Option<(usize, u64, usize)> = None;
        for at in 0..total {
            if selected.contains(&at) {
                continue;
            }
            let novelty = selected
                .iter()
                .map(|&other| {
                    signatures[at]
                        .iter()
                        .zip(&signatures[other])
                        .map(|(a, b)| u64::from(a.abs_diff(*b)))
                        .sum::<u64>()
                })
                .min()
                .unwrap_or(0);
            let gap = selected
                .iter()
                .map(|&other| at.abs_diff(other))
                .min()
                .unwrap_or(0);
            if best
                .is_none_or(|(_, best_novelty, best_gap)| (novelty, gap) > (best_novelty, best_gap))
            {
                best = Some((at, novelty, gap));
            }
        }
        let Some((at, _, _)) = best else { break };
        selected.push(at);
    }
    selected.sort_unstable();
    selected
}

fn animation_required(needs_nsfw: bool, needs_general: bool, needs_text: bool) -> bool {
    needs_nsfw || needs_general || needs_text
}

fn animated_ocr_indices(total: usize) -> Vec<usize> {
    sample_indices(total, MAX_ANIMATED_OCR_FRAMES)
}

fn frame_difference(a: &image::RgbImage, b: &image::RgbImage) -> u64 {
    if a.dimensions() != b.dimensions() {
        return u64::MAX;
    }
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(left, right)| u64::from(left.abs_diff(*right)))
        .sum()
}

fn add_distinct_cover(frames: &mut Vec<image::RgbImage>, cover: &image::RgbImage) {
    if !frames
        .iter()
        .any(|frame| frame_difference(frame, cover) == 0)
    {
        frames.push(cover.clone());
    }
}

fn animated_ocr_indices_for_frames(frames: &[image::RgbImage]) -> Vec<usize> {
    let base = animated_ocr_indices(frames.len());
    let mut selected: Vec<usize> = Vec::new();
    for &at in &base {
        if !selected
            .iter()
            .any(|&other| frame_difference(&frames[at], &frames[other]) == 0)
        {
            selected.push(at);
        }
    }
    for _ in 0..MAX_ANIMATED_OCR_EXTRA_FRAMES {
        let mut best: Option<(usize, u64)> = None;
        for at in 0..frames.len() {
            if base.contains(&at) || selected.contains(&at) {
                continue;
            }
            let novelty = selected
                .iter()
                .map(|&other| frame_difference(&frames[at], &frames[other]))
                .min()
                .unwrap_or(0);
            if novelty > best.map_or(0, |(_, score)| score) {
                best = Some((at, novelty));
            }
        }
        let Some((at, _)) = best else { break };
        selected.push(at);
    }
    selected.sort_unstable();
    selected
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

fn native_gif_allocation(width: u32, height: u32) -> Option<u64> {
    if width == 0 || height == 0 || width > MAX_ANIMATED_SIDE || height > MAX_ANIMATED_SIDE {
        return None;
    }
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let decoder = pixels.checked_mul(4)?.checked_mul(2)?;
    let retained = pixels
        .checked_mul(3)?
        .checked_mul(MAX_ANIMATED_FRAMES as u64)?;
    let signatures = u64::from(ANIMATED_SIGNATURE_SIDE)
        .checked_mul(u64::from(ANIMATED_SIGNATURE_SIDE))?
        .checked_mul(4)?
        .checked_mul(MAX_NATIVE_GIF_SCAN_FRAMES as u64)?;
    decoder.checked_add(retained)?.checked_add(signatures)
}

fn native_gif_decoder<R: std::io::BufRead + std::io::Seek>(
    reader: R,
) -> Option<image::codecs::gif::GifDecoder<R>> {
    let mut decoder = image::codecs::gif::GifDecoder::new(reader).ok()?;
    let (width, height) = decoder.dimensions();
    let allocation = native_gif_allocation(width, height)?;
    if allocation > IMAGE_DECODE_BYTES_MAX {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_ANIMATED_SIDE);
    limits.max_image_height = Some(MAX_ANIMATED_SIDE);
    limits.max_alloc = Some(IMAGE_DECODE_BYTES_MAX);
    decoder.set_limits(limits).ok()?;
    Some(decoder)
}

fn bounded_frame_count<T, E>(
    frames: impl IntoIterator<Item = Result<T, E>>,
    limit: usize,
) -> Option<usize> {
    let mut total = 0usize;
    for frame in frames {
        frame.ok()?;
        total = total.checked_add(1)?;
        if total > limit {
            return None;
        }
    }
    Some(total)
}

fn decode_native_frames<'a>(
    make_frames: impl Fn() -> Option<image::Frames<'a>>,
) -> Option<Vec<image::RgbImage>> {
    let mut signatures = Vec::new();
    bounded_frame_count(
        make_frames()?.map(|frame| {
            frame.map(|frame| {
                let signature = image::imageops::resize(
                    &frame.into_buffer(),
                    ANIMATED_SIGNATURE_SIDE,
                    ANIMATED_SIGNATURE_SIDE,
                    image::imageops::FilterType::Nearest,
                );
                signatures.push(rgba_over_neutral(signature).into_raw());
            })
        }),
        MAX_NATIVE_GIF_SCAN_FRAMES,
    )?;
    let wanted = sample_novel_indices(&signatures, MAX_ANIMATED_FRAMES);
    if wanted.is_empty() {
        return None;
    }

    let mut frames = Vec::with_capacity(wanted.len());
    for (index, frame) in make_frames()?.enumerate() {
        if wanted.binary_search(&index).is_err() {
            continue;
        }
        let frame = frame.ok()?;
        frames.push(rgba_over_neutral(frame.into_buffer()));
    }
    (!frames.is_empty()).then_some(frames)
}

fn decode_gif_frames(bytes: &[u8]) -> Option<Vec<image::RgbImage>> {
    decode_native_frames(|| Some(native_gif_decoder(Cursor::new(bytes))?.into_frames()))
}

fn decode_webp_frames(bytes: &[u8]) -> Option<Vec<image::RgbImage>> {
    decode_native_frames(|| {
        let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
        let (width, height) = decoder.dimensions();
        (native_gif_allocation(width, height)? <= IMAGE_DECODE_BYTES_MAX)
            .then(|| decoder.into_frames())
    })
}

fn decode_apng_frames(bytes: &[u8]) -> Option<Vec<image::RgbImage>> {
    decode_native_frames(|| {
        let mut decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).ok()?;
        let (width, height) = decoder.dimensions();
        if !decoder.is_apng().ok()?
            || native_gif_allocation(width, height)? > IMAGE_DECODE_BYTES_MAX
        {
            return None;
        }
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_ANIMATED_SIDE);
        limits.max_image_height = Some(MAX_ANIMATED_SIDE);
        limits.max_alloc = Some(IMAGE_DECODE_BYTES_MAX);
        decoder.set_limits(limits).ok()?;
        Some(decoder.apng().ok()?.into_frames())
    })
}

pub(crate) fn raster_kind_full(bytes: &[u8]) -> Option<bool> {
    match image::guess_format(bytes).ok() {
        Some(
            image::ImageFormat::Gif
            | image::ImageFormat::Tiff
            | image::ImageFormat::Ico
            | image::ImageFormat::Pnm,
        ) => Some(true),
        Some(image::ImageFormat::WebP | image::ImageFormat::Png) => {
            match raster_kind(bytes, bytes.len()) {
                RasterKind::Animated => Some(true),
                RasterKind::Static => Some(false),
                RasterKind::Unknown => None,
            }
        }
        _ => Some(false),
    }
}

pub(crate) fn is_animated_raster_bytes(bytes: &[u8]) -> bool {
    raster_kind_full(bytes).unwrap_or(true)
}

async fn animated_command(
    program: &str,
    args: &[String],
    limit: usize,
    deadline: std::time::Instant,
) -> Option<std::process::Output> {
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return None;
    }
    animated_command_limited(program, args, limit, remaining).await
}

const DECODER_THREADS: &str = "2";

fn decoder_args(program: &str, args: &[String]) -> Vec<String> {
    let mut all = Vec::with_capacity(args.len() + 2);
    if program == "ffmpeg" {
        all.push("-threads".to_owned());
        all.push(DECODER_THREADS.to_owned());
    }
    all.extend_from_slice(args);
    all
}

async fn animated_command_limited(
    program: &str,
    args: &[String],
    limit: usize,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    use tokio::io::AsyncReadExt;
    let mut child = Command::new(program)
        .args(decoder_args(program, args))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let result = tokio::time::timeout(timeout, async {
        let read = |pipe: Box<dyn tokio::io::AsyncRead + Send + Unpin>, cap: usize| async move {
            let mut bytes = Vec::new();
            pipe.take(cap as u64 + 1).read_to_end(&mut bytes).await?;
            if bytes.len() > cap {
                return Err(std::io::Error::other("decoder output limit exceeded"));
            }
            Ok::<_, std::io::Error>(bytes)
        };
        let (stdout, stderr) = tokio::try_join!(
            read(Box::new(stdout), limit),
            read(Box::new(stderr), 64 * 1024)
        )?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>(std::process::Output {
            status,
            stdout,
            stderr,
        })
    })
    .await;
    let output = match result {
        Ok(Ok(output)) => output,
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            log::warn!("nsfw: decoder timed out or exceeded its output budget");
            return None;
        }
    };
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
    let random = |domain: u64| RandomState::new().hash_one((domain, at, std::process::id()));
    std::env::temp_dir().join(format!(
        "groupbot-anim-{}-{at:016x}-{:016x}{:016x}.bin",
        std::process::id(),
        random(0),
        random(1)
    ))
}

async fn write_animated_scratch(bytes: &[u8]) -> Option<std::path::PathBuf> {
    for _ in 0..16 {
        let path = animated_scratch_path();
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = match options.open(&path).await {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        };
        if file.write_all(bytes).await.is_err() || file.flush().await.is_err() {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return None;
        }
        drop(file);
        return Some(path);
    }
    None
}

fn json_number(value: Option<&serde_json::Value>) -> Option<f64> {
    value
        .and_then(serde_json::Value::as_f64)
        .or_else(|| value.and_then(serde_json::Value::as_str)?.parse().ok())
}

fn verified_video_frame_count(stream: &serde_json::Value) -> Option<usize> {
    let declared = stream.get("nb_frames")?.as_str()?.parse::<usize>().ok()?;
    let decoded = stream
        .get("nb_read_frames")?
        .as_str()?
        .parse::<usize>()
        .ok()?;
    (declared > 0 && declared == decoded).then_some(declared)
}

fn opaque_avif_still(streams: &[serde_json::Value]) -> Option<(u32, u32)> {
    let [stream] = streams else {
        return None;
    };
    if stream.get("index")?.as_u64()? != 0
        || stream.get("codec_name")?.as_str()? != "av1"
        || stream.get("pix_fmt")?.as_str()? != "yuv420p"
        || video_declares_alpha(stream)
        || !stream
            .get("duration")
            .is_none_or(serde_json::Value::is_null)
        || verified_video_frame_count(stream)? != 1
    {
        return None;
    }
    let width = u32::try_from(stream.get("width")?.as_u64()?).ok()?;
    let height = u32::try_from(stream.get("height")?.as_u64()?).ok()?;
    (width > 0 && height > 0 && width <= IMAGE_DIMENSION_MAX && height <= IMAGE_DIMENSION_MAX)
        .then_some((width, height))
}

fn opaque_avif_poster_sequence(
    streams: &[serde_json::Value],
) -> Option<(&serde_json::Value, &serde_json::Value)> {
    let [poster, sequence] = streams else {
        return None;
    };
    if poster.get("index")?.as_u64()? != 0
        || sequence.get("index")?.as_u64()? != 1
        || ![poster, sequence].iter().all(|stream| {
            stream.get("codec_name").and_then(serde_json::Value::as_str) == Some("av1")
                && stream.get("pix_fmt").and_then(serde_json::Value::as_str) == Some("yuv420p")
                && !video_declares_alpha(stream)
        })
        || !poster
            .get("duration")
            .is_none_or(serde_json::Value::is_null)
        || verified_video_frame_count(poster)? != 1
        || verified_video_frame_count(sequence)? < 2
    {
        return None;
    }
    let width = poster.get("width")?.as_u64()?;
    let height = poster.get("height")?.as_u64()?;
    if width == 0
        || height == 0
        || width > u64::from(IMAGE_DIMENSION_MAX)
        || height > u64::from(IMAGE_DIMENSION_MAX)
        || sequence.get("width")?.as_u64()? != width
        || sequence.get("height")?.as_u64()? != height
        || !json_number(sequence.get("duration"))?.is_finite()
        || json_number(sequence.get("duration"))? <= 0.0
    {
        return None;
    }
    Some((poster, sequence))
}

fn video_signature_scan_complete(frame_count: usize, duration: f64, fps: f64) -> bool {
    if !duration.is_finite() || !fps.is_finite() || duration <= 0.0 || fps <= 0.0 {
        return false;
    }
    let expected = ((duration * fps).floor() as usize).min(MAX_VIDEO_SIGNATURE_FRAMES);
    frame_count > 0 && frame_count.saturating_add(2) >= expected
}

fn video_alpha_mode(stream: &serde_json::Value) -> Option<&serde_json::Value> {
    stream
        .get("tags")
        .and_then(serde_json::Value::as_object)
        .and_then(|tags| {
            tags.iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("alpha_mode"))
                .map(|(_, value)| value)
        })
}

fn video_declares_alpha(stream: &serde_json::Value) -> bool {
    video_alpha_mode(stream)
        .is_some_and(|value| value.as_str() != Some("0") && value.as_u64() != Some(0))
}

fn video_has_premultiplied_alpha(stream: &serde_json::Value) -> bool {
    video_alpha_mode(stream)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("premultiplied"))
}

fn video_needs_alpha_composite(stream: &serde_json::Value) -> bool {
    if video_declares_alpha(stream) {
        return true;
    }
    !matches!(
        stream.get("pix_fmt").and_then(serde_json::Value::as_str),
        Some(
            "yuv420p"
                | "yuv422p"
                | "yuv444p"
                | "yuvj420p"
                | "yuvj422p"
                | "yuvj444p"
                | "yuv420p10le"
                | "yuv422p10le"
                | "yuv444p10le"
                | "nv12"
                | "p010le"
                | "rgb24"
                | "bgr24"
                | "gray"
                | "gbrp"
        )
    )
}

const VIDEO_ALPHA_COMPOSITE: &str = "format=rgba,split[fg][bg];[bg]format=rgb24,lutrgb=r=128:g=128:b=128[neutral];[neutral][fg]overlay=format=rgb:alpha=straight:shortest=1,format=rgb24";
const VIDEO_PREMULTIPLIED_ALPHA_COMPOSITE: &str = "format=rgba,split[fg][bg];[bg]format=rgb24,lutrgb=r=128:g=128:b=128[neutral];[neutral][fg]overlay=format=rgb:alpha=premultiplied:shortest=1,format=rgb24";

fn video_alpha_composite_filter(stream: &serde_json::Value) -> Option<&'static str> {
    if video_has_premultiplied_alpha(stream) {
        Some(VIDEO_PREMULTIPLIED_ALPHA_COMPOSITE)
    } else if video_needs_alpha_composite(stream) {
        Some(VIDEO_ALPHA_COMPOSITE)
    } else {
        None
    }
}

async fn decode_video_frames(bytes: Vec<u8>) -> Option<Vec<image::RgbImage>> {
    let avif = avif_ftyp_len(&bytes).is_some();
    let path = write_animated_scratch(&bytes).await?;
    drop(bytes);
    let frames = decode_video_file(&path, avif).await;
    let _ = tokio::fs::remove_file(&path).await;
    frames
}

async fn decode_video_file(path: &std::path::Path, avif: bool) -> Option<Vec<image::RgbImage>> {
    let input = path.to_str()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut probe_args = vec!["-v".to_owned(), "error".to_owned()];
    if avif {
        probe_args.push("-count_frames".to_owned());
    }
    probe_args.extend(
        [
            "-select_streams",
            "v",
            "-show_entries",
            "stream=index,width,height,duration,pix_fmt,codec_name,nb_frames,nb_read_frames:stream_tags=alpha_mode:format=duration",
            "-of",
            "json",
            "-i",
            input,
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let probe = animated_command("ffprobe", &probe_args, 64 * 1024, deadline).await?;
    let json: serde_json::Value = serde_json::from_slice(&probe.stdout).ok()?;
    let streams = json.get("streams")?.as_array()?;
    if avif
        && let Some((width, height)) = opaque_avif_still(streams)
        && json_number(json.get("format")?.get("duration")).is_none()
    {
        let (width, height) = scaled_dimensions(width, height, MAX_ANIMATED_SIDE);
        let frame_bytes = width.checked_mul(height)?.checked_mul(3)? as usize;
        let filter = format!("scale={width}:{height}:flags=bilinear");
        let args = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            input,
            "-map",
            "0:v:0",
            "-vf",
            &filter,
            "-frames:v",
            "1",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let output = animated_command("ffmpeg", &args, frame_bytes, deadline).await?;
        if output.stdout.len() != frame_bytes {
            return None;
        }
        return Some(vec![image::RgbImage::from_raw(
            width,
            height,
            output.stdout,
        )?]);
    }
    let (stream, avif_sequence) = match streams.as_slice() {
        [stream] if !avif || verified_video_frame_count(stream).is_some_and(|count| count >= 2) => {
            (stream, false)
        }
        _ if avif => (opaque_avif_poster_sequence(streams)?.1, true),
        _ => return None,
    };
    let stream_map = avif_sequence.then_some("0:v:1");
    let alpha_filter = video_alpha_composite_filter(stream)
        .map(|filter| format!(",{filter}"))
        .unwrap_or_default();
    let decoder_flags: &[&str] = if video_declares_alpha(stream)
        && stream.get("codec_name").and_then(serde_json::Value::as_str) == Some("vp9")
    {
        &["-c:v", "libvpx-vp9"]
    } else {
        &[]
    };
    let width = json_number(stream.get("width"))? as u32;
    let height = json_number(stream.get("height"))? as u32;
    let duration = json_number(stream.get("duration"))
        .or_else(|| json_number(json.get("format")?.get("duration")))?
        .max(0.001);
    let (width, height) = scaled_dimensions(width, height, MAX_ANIMATED_SIDE);
    let fps = ((MAX_VIDEO_SIGNATURE_FRAMES - 1) as f64 / duration)
        .clamp(1.0 / 3600.0, MAX_VIDEO_SIGNATURE_FPS);
    let signature_side = ANIMATED_SIGNATURE_SIDE as usize;
    let signature_bytes = signature_side.checked_mul(signature_side)?.checked_mul(3)?;
    let signature_limit = MAX_VIDEO_SIGNATURE_FRAMES.checked_mul(signature_bytes)?;
    let scan_filter = format!(
        "fps={fps:.8}{alpha_filter},scale={ANIMATED_SIGNATURE_SIDE}:{ANIMATED_SIGNATURE_SIDE}:flags=bilinear"
    );
    let scan_args = ["-hide_banner", "-loglevel", "error"]
        .into_iter()
        .chain(decoder_flags.iter().copied())
        .chain(["-i", input])
        .chain(stream_map.into_iter().flat_map(|map| ["-map", map]))
        .chain([
            "-vf",
            scan_filter.as_str(),
            "-frames:v",
            &MAX_VIDEO_SIGNATURE_FRAMES.to_string(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let scan = animated_command("ffmpeg", &scan_args, signature_limit, deadline).await?;
    if scan.stdout.len() % signature_bytes != 0 {
        return None;
    }
    let signatures = scan
        .stdout
        .chunks_exact(signature_bytes)
        .map(|frame| frame.to_vec())
        .collect::<Vec<_>>();
    if !video_signature_scan_complete(signatures.len(), duration, fps) {
        return None;
    }
    let wanted = sample_novel_indices(&signatures, MAX_ANIMATED_FRAMES);
    if wanted.is_empty() {
        return None;
    }
    let select = wanted
        .iter()
        .map(|at| format!("eq(n\\,{at})"))
        .collect::<Vec<_>>()
        .join("+");
    let filter =
        format!("fps={fps:.8},select={select}{alpha_filter},scale={width}:{height}:flags=bilinear");
    let ffmpeg_args = ["-hide_banner", "-loglevel", "error"]
        .into_iter()
        .chain(decoder_flags.iter().copied())
        .chain(["-i", input])
        .chain(stream_map.into_iter().flat_map(|map| ["-map", map]))
        .chain([
            "-vf",
            filter.as_str(),
            "-frames:v",
            &MAX_ANIMATED_FRAMES.to_string(),
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let full_limit = MAX_ANIMATED_FRAMES
        .checked_mul(MAX_ANIMATED_SIDE as usize)?
        .checked_mul(MAX_ANIMATED_SIDE as usize)?
        .checked_mul(3)?;
    let output = animated_command("ffmpeg", &ffmpeg_args, full_limit, deadline).await?;
    let frame_bytes = width.checked_mul(height)?.checked_mul(3)? as usize;
    if output.stdout.len() % frame_bytes != 0 {
        return None;
    }
    let mut frames = Vec::new();
    for raw in output.stdout.chunks_exact(frame_bytes) {
        frames.push(image::RgbImage::from_raw(width, height, raw.to_vec())?);
    }
    if frames.len() != wanted.len() {
        return None;
    }
    if avif_sequence {
        let poster_filter = format!("scale={width}:{height}:flags=bilinear");
        let poster_args = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            input,
            "-map",
            "0:v:0",
            "-vf",
            &poster_filter,
            "-frames:v",
            "1",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let poster = animated_command("ffmpeg", &poster_args, frame_bytes, deadline).await?;
        if poster.stdout.len() != frame_bytes {
            return None;
        }
        let poster = image::RgbImage::from_raw(width, height, poster.stdout)?;
        add_distinct_cover(&mut frames, &poster);
    }
    Some(frames)
}

fn avif_ftyp_len(bytes: &[u8]) -> Option<usize> {
    if bytes.get(4..8) != Some(b"ftyp") {
        return None;
    }
    let size = bytes.get(0..4)?;
    let ftyp_size = u32::from_be_bytes(size.try_into().unwrap()) as usize;
    if !(16..=bytes.len()).contains(&ftyp_size)
        || !bytes[8..ftyp_size]
            .as_chunks::<4>()
            .0
            .iter()
            .any(|brand| brand == b"avif" || brand == b"avis")
    {
        return None;
    }
    Some(ftyp_size)
}

fn avif_has_unresolved_alpha(bytes: &[u8]) -> bool {
    let Some(mut at) = avif_ftyp_len(bytes) else {
        return false;
    };
    while at < bytes.len() {
        let Some(header) = bytes.get(at..at.saturating_add(8)) else {
            return true;
        };
        let declared = u32::from_be_bytes(header[..4].try_into().unwrap());
        let (box_header, size) = match declared {
            0 => (8, bytes.len() - at),
            1 => {
                let Some(large) = bytes.get(at + 8..at.saturating_add(16)) else {
                    return true;
                };
                let Ok(size) = usize::try_from(u64::from_be_bytes(large.try_into().unwrap()))
                else {
                    return true;
                };
                (16, size)
            }
            size => (8, size as usize),
        };
        if size < box_header || size > bytes.len() - at {
            return true;
        }
        if &header[4..8] == b"meta" {
            return bytes[at + box_header..at + size]
                .windows(4)
                .any(|marker| marker == b"auxC");
        }
        at += size;
    }
    true
}

pub(crate) async fn decode_animated_frames(bytes: Vec<u8>) -> Option<Vec<image::RgbImage>> {
    if avif_has_unresolved_alpha(&bytes) {
        return None;
    }
    let format = image::guess_format(&bytes).ok();
    if format == Some(image::ImageFormat::Tiff) {
        return tokio::task::spawn_blocking(move || tiff_pages::decode_frames(bytes))
            .await
            .ok()
            .flatten();
    }
    if format == Some(image::ImageFormat::Ico) {
        return tokio::task::spawn_blocking(move || multipage::decode_ico_frames(bytes))
            .await
            .ok()
            .flatten();
    }
    if format == Some(image::ImageFormat::Pnm) {
        return tokio::task::spawn_blocking(move || multipage::decode_pnm_frames(bytes))
            .await
            .ok()
            .flatten();
    }
    if format == Some(image::ImageFormat::Gif) && bytes.last() != Some(&0x3b) {
        return None;
    }
    if matches!(
        format,
        Some(image::ImageFormat::WebP | image::ImageFormat::Png)
    ) && raster_kind_full(&bytes) != Some(true)
    {
        return None;
    }
    let native = match format {
        Some(image::ImageFormat::Gif) => Some(decode_gif_frames as fn(&[u8]) -> _),
        Some(image::ImageFormat::WebP) => Some(decode_webp_frames as fn(&[u8]) -> _),
        Some(image::ImageFormat::Png) => Some(decode_apng_frames as fn(&[u8]) -> _),
        _ => None,
    };
    let bytes = if let Some(decode) = native {
        let (frames, bytes) = tokio::task::spawn_blocking(move || {
            let frames = std::panic::catch_unwind(|| decode(&bytes)).ok().flatten();
            (frames, bytes)
        })
        .await
        .ok()?;
        if let Some(frames) = frames {
            return Some(frames);
        }
        bytes
    } else {
        bytes
    };
    decode_video_frames(bytes).await
}

pub(crate) fn decode_tgs_frames_offline(bytes: &[u8]) -> Option<Vec<image::RgbImage>> {
    tgs::decode(bytes)
}

async fn decode_tgs_frames(ctx: &Arc<Ctx>, bytes: Vec<u8>) -> Option<Vec<image::RgbImage>> {
    let render_slot = tokio::time::timeout(TGS_RENDER_QUEUE_DEADLINE, TGS_RENDER_SLOT.acquire())
        .await
        .ok()?
        .ok()?;
    let decode_slot = tokio::time::timeout(IMAGE_DECODE_DEADLINE, ctx.media_decode_slot())
        .await
        .ok()?;
    let worker = tokio::task::spawn_blocking(move || {
        let _render_slot = render_slot;
        let _decode_slot = decode_slot;
        tgs::decode(&bytes)
    });
    tokio::time::timeout(IMAGE_DECODE_DEADLINE, worker)
        .await
        .ok()?
        .ok()?
}

struct AnimatedSelection {
    image: image::RgbImage,
    look: Option<Look>,
    judgement: Option<(Judgement, String)>,
    margins: Option<[f32; super::CONCEPT_SLOTS]>,
    custom_margins: Option<Vec<f32>>,
    nsfw_failed: bool,
    nsfw_cacheable: bool,
    general_failed: bool,
    concepts_failed: bool,
}

fn better_animated_action(
    current: Option<(usize, Judgement)>,
    candidate: (usize, Judgement),
    spare: bool,
) -> bool {
    let (index, judged) = candidate;
    if !allow_delete(
        judged.innocent,
        judged.explicit,
        spare,
        judged.weak,
        judged.confirmed,
        judged.arbiter,
    ) {
        return false;
    }
    current.is_none_or(|(at, best)| {
        judged.score > best.score || (judged.score == best.score && index < at)
    })
}

fn score_animated_frames(
    frames: Vec<image::RgbImage>,
    needs_nsfw: bool,
    needs_general: bool,
    needs_concepts: bool,
    filter_specs: Vec<super::imgfilter::Spec>,
) -> Result<Option<AnimatedSelection>, ModelError> {
    score_drawn_or_filmed_frames(
        frames,
        needs_nsfw,
        needs_general,
        needs_concepts,
        filter_specs,
        false,
    )
}

fn score_drawn_or_filmed_frames(
    frames: Vec<image::RgbImage>,
    needs_nsfw: bool,
    needs_general: bool,
    needs_concepts: bool,
    filter_specs: Vec<super::imgfilter::Spec>,
    drawn: bool,
) -> Result<Option<AnimatedSelection>, ModelError> {
    score_animated_frames_with(
        frames,
        needs_nsfw,
        needs_general,
        needs_concepts,
        filter_specs,
        drawn,
        look,
        grade,
        super::vision::embed_of,
    )
}

#[allow(clippy::too_many_arguments)]
fn score_animated_frames_with<L, G, E>(
    frames: Vec<image::RgbImage>,
    needs_nsfw: bool,
    needs_general: bool,
    needs_concepts: bool,
    filter_specs: Vec<super::imgfilter::Spec>,
    drawn: bool,
    look_fn: L,
    grade_fn: G,
    embed_fn: E,
) -> Result<Option<AnimatedSelection>, ModelError>
where
    L: Fn(&image::RgbImage) -> Result<Option<Look>, ModelError> + Sync,
    G: Fn(&image::RgbImage) -> Result<Option<Grade>, ModelError> + Sync,
    E: Fn(&image::RgbImage) -> Result<Option<Vec<f32>>, ModelError> + Sync,
{
    struct Lane {
        best_nsfw: Option<(usize, image::RgbImage, Look, Judgement)>,
        best_actionable: Option<(usize, Judgement, String)>,
        best_spare_actionable: Option<(usize, Judgement)>,
        best_general: Option<(usize, image::RgbImage, f32)>,
        margins: Option<[f32; super::CONCEPT_SLOTS]>,
        custom_margins: Option<Vec<f32>>,
        general_observed: bool,
        nsfw_missing: bool,
        arbiter_missing: bool,
        general_missing: bool,
        nsfw_error: Option<ModelError>,
        general_error: Option<ModelError>,
    }
    let total = frames.len();
    let lanes = infer_sessions().clamp(1, total.max(1));
    let mut shards: Vec<Vec<(usize, image::RgbImage)>> = (0..lanes).map(|_| Vec::new()).collect();
    for (index, frame) in frames.into_iter().enumerate() {
        shards[index % lanes].push((index, frame));
    }
    let filter_specs = &filter_specs;
    let look_fn = &look_fn;
    let grade_fn = &grade_fn;
    let embed_fn = &embed_fn;
    let partials: Vec<Lane> = std::thread::scope(|scope| {
        let handles: Vec<_> = shards
            .into_iter()
            .map(|shard| {
                scope.spawn(move || {
                    let mut lane = Lane {
                        best_nsfw: None,
                        best_actionable: None,
                        best_spare_actionable: None,
                        best_general: None,
                        margins: needs_concepts.then_some([f32::MIN; super::CONCEPT_SLOTS]),
                        custom_margins: needs_general.then(|| vec![f32::MIN; filter_specs.len()]),
                        general_observed: false,
                        nsfw_missing: false,
                        arbiter_missing: false,
                        general_missing: false,
                        nsfw_error: None,
                        general_error: None,
                    };
                    for (index, frame) in shard {
                        let look = if needs_nsfw {
                            match look_fn(&frame) {
                                Ok(Some(look)) => Some(look),
                                Ok(None) => {
                                    lane.nsfw_missing = true;
                                    None
                                }
                                Err(error) => {
                                    lane.nsfw_error.get_or_insert(error);
                                    None
                                }
                            }
                        } else {
                            None
                        };
                        let needs_arbiter =
                            look.as_ref().is_some_and(|look| look.peak >= GRADE_ABOVE);
                        let mut arbiter_failed = false;
                        let embedding = if needs_general || needs_arbiter {
                            match embed_fn(&frame) {
                                Ok(Some(embedding)) => Some(embedding),
                                Ok(None) => {
                                    lane.general_missing |= needs_general;
                                    lane.arbiter_missing |= needs_arbiter;
                                    None
                                }
                                Err(error) => {
                                    if needs_general {
                                        lane.general_error.get_or_insert_with(|| error.clone());
                                    }
                                    if needs_arbiter {
                                        lane.nsfw_error.get_or_insert(error);
                                        arbiter_failed = true;
                                    }
                                    None
                                }
                            }
                        } else {
                            None
                        };

                        if let Some(look) = look
                            && !arbiter_failed
                        {
                            let judged = (|| {
                                let grade = if look.score >= GRADE_ABOVE {
                                    grade_fn(&frame)?
                                } else {
                                    None
                                };
                                let arbiter = needs_arbiter
                                    .then(|| embedding.as_deref().map(arbiter_of))
                                    .flatten();
                                assemble_judgement(&look, grade.as_ref(), arbiter)
                            })();
                            match judged {
                                Ok(judged) => {
                                    if better_animated_action(
                                        lane.best_actionable
                                            .as_ref()
                                            .map(|(at, judged, _)| (*at, *judged)),
                                        (index, judged),
                                        false,
                                    ) {
                                        lane.best_actionable =
                                            Some((index, judged, look.note.clone()));
                                    }
                                    if better_animated_action(
                                        lane.best_spare_actionable,
                                        (index, judged),
                                        true,
                                    ) {
                                        lane.best_spare_actionable = Some((index, judged));
                                    }
                                    let better = lane.best_nsfw.as_ref().is_none_or(
                                        |(at, _, current, _)| {
                                            look.score > current.score
                                                || (look.score == current.score
                                                    && look.peak > current.peak)
                                                || (look.score == current.score
                                                    && look.peak == current.peak
                                                    && index < *at)
                                        },
                                    );
                                    if better {
                                        lane.best_nsfw = Some((index, frame.clone(), look, judged));
                                    }
                                }
                                Err(error) => {
                                    lane.nsfw_error.get_or_insert(error);
                                }
                            }
                        }

                        if let (true, Some(embedding)) = (needs_general, embedding) {
                            lane.general_observed = true;
                            let mut relevance = f32::MIN;
                            if let Some(maxima) = lane.margins.as_mut() {
                                let all = super::concepts::margins_from(&embedding);
                                for (maximum, value) in maxima.iter_mut().zip(all) {
                                    *maximum = maximum.max(value);
                                    relevance = relevance.max(value);
                                }
                            }
                            if let Some(maxima) = lane.custom_margins.as_mut() {
                                let prints = if filter_specs.iter().any(|spec| spec.is_face()) {
                                    let found = if drawn {
                                        super::facelock::drawn_prints(&frame)
                                    } else {
                                        super::facelock::prints(&frame)
                                    };
                                    found.unwrap_or_else(|error| {
                                        log::error!("face lock: frame {index} faces failed: {error}");
                                        None
                                    })
                                } else {
                                    None
                                };
                                for (maximum, spec) in maxima.iter_mut().zip(filter_specs) {
                                    let value = super::imgfilter::score_spec(
                                        spec,
                                        &embedding,
                                        prints.as_deref(),
                                        drawn,
                                    );
                                    *maximum = maximum.max(value);
                                    relevance = relevance.max(value - spec.cut());
                                }
                            }
                            let better =
                                lane.best_general.as_ref().is_none_or(|(at, _, current)| {
                                    relevance > *current || (relevance == *current && index < *at)
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

    let mut best_nsfw: Option<(usize, image::RgbImage, Look, Judgement)> = None;
    let mut best_actionable: Option<(usize, Judgement, String)> = None;
    let mut best_spare_actionable: Option<(usize, Judgement)> = None;
    let mut best_general: Option<(usize, image::RgbImage, f32)> = None;
    let mut margins = needs_concepts.then_some([f32::MIN; super::CONCEPT_SLOTS]);
    let mut custom_margins = needs_general.then(|| vec![f32::MIN; filter_specs.len()]);
    let mut general_observed = false;
    let mut nsfw_missing = false;
    let mut arbiter_missing = false;
    let mut general_missing = false;
    let mut nsfw_error = None;
    let mut general_error = None;
    for lane in partials {
        general_observed |= lane.general_observed;
        nsfw_missing |= lane.nsfw_missing;
        arbiter_missing |= lane.arbiter_missing;
        general_missing |= lane.general_missing;
        nsfw_error = nsfw_error.or(lane.nsfw_error);
        general_error = general_error.or(lane.general_error);
        if let Some((index, image, look, judged)) = lane.best_nsfw {
            let better = best_nsfw.as_ref().is_none_or(|(at, _, current, _)| {
                look.score > current.score
                    || (look.score == current.score && look.peak > current.peak)
                    || (look.score == current.score && look.peak == current.peak && index < *at)
            });
            if better {
                best_nsfw = Some((index, image, look, judged));
            }
        }
        if let Some((index, judged, note)) = lane.best_actionable
            && better_animated_action(
                best_actionable
                    .as_ref()
                    .map(|(at, judged, _)| (*at, *judged)),
                (index, judged),
                false,
            )
        {
            best_actionable = Some((index, judged, note));
        }
        if let Some(candidate) = lane.best_spare_actionable
            && better_animated_action(best_spare_actionable, candidate, true)
        {
            best_spare_actionable = Some(candidate);
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

    let nsfw_complete = needs_nsfw && !nsfw_missing && nsfw_error.is_none() && best_nsfw.is_some();
    let general_complete =
        needs_general && !general_missing && general_error.is_none() && general_observed;
    let had_partial = best_nsfw.is_some() || general_observed;
    if !nsfw_complete {
        best_nsfw = None;
        best_actionable = None;
        best_spare_actionable = None;
    }
    if !general_complete {
        best_general = None;
        margins = None;
        custom_margins = None;
    }
    if best_nsfw.is_none() && best_general.is_none() {
        if let Some(error) = nsfw_error.or(general_error) {
            return Err(error);
        }
        if had_partial {
            return Err(ModelError::runtime(
                "animated frame coverage was incomplete",
            ));
        }
        return Ok(None);
    }

    let (image, look, judgement) = if needs_nsfw {
        if let Some((index, image, mut look, fallback)) = best_nsfw {
            let (judged, note) = best_actionable.map_or_else(
                || {
                    (
                        fallback,
                        format!("animated frame {}/{} {}", index + 1, total, look.note),
                    )
                },
                |(at, judged, note)| {
                    (
                        judged,
                        format!("animated frame {}/{} {note}", at + 1, total),
                    )
                },
            );
            look.note = format!("animated frame {}/{} {}", index + 1, total, look.note);
            let judged =
                judged.with_animated_spare(best_spare_actionable.map(|(_, judged)| judged));
            (image, Some(look), Some((judged, note)))
        } else {
            let Some((_, image, _)) = best_general else {
                return Ok(None);
            };
            (image, None, None)
        }
    } else {
        let Some((_, image, _)) = best_general else {
            return Ok(None);
        };
        (image, None, None)
    };
    Ok(Some(AnimatedSelection {
        image,
        look,
        judgement,
        margins,
        custom_margins,
        nsfw_failed: needs_nsfw && !nsfw_complete,
        nsfw_cacheable: nsfw_complete && !arbiter_missing,
        general_failed: needs_general && !general_complete,
        concepts_failed: false,
    }))
}

#[derive(serde::Serialize)]
pub(crate) struct OfflineAnimatedVisual {
    pub frames_scored: usize,
    pub nsfw_score: Option<f32>,
    pub nsfw_would_delete: Option<bool>,
    pub nsfw_note: Option<String>,
    pub concept_margins: Option<[f32; super::CONCEPT_SLOTS]>,
    pub custom_margins: Option<Vec<f32>>,
}

pub(crate) fn evaluate_animated_visual_offline(
    frames: Vec<image::RgbImage>,
    needs_nsfw: bool,
    needs_concepts: bool,
    filter_specs: Vec<(Vec<f32>, f32)>,
    limit: u32,
    spare: bool,
) -> Result<OfflineAnimatedVisual, ModelError> {
    let frames_scored = frames.len();
    let needs_custom = !filter_specs.is_empty();
    let needs_general = needs_concepts || needs_custom;
    let filter_specs = filter_specs
        .into_iter()
        .map(|(vector, cut)| super::imgfilter::Spec::Vector(vector, cut, None))
        .collect();
    let selection = score_animated_frames(
        frames,
        needs_nsfw,
        needs_general,
        needs_concepts,
        filter_specs,
    )?
    .ok_or_else(|| ModelError::runtime("no animated frame produced a model score"))?;
    if (needs_nsfw && selection.nsfw_failed)
        || (needs_general && selection.general_failed)
        || (needs_concepts && selection.concepts_failed)
    {
        return Err(ModelError::runtime(
            "animated frame coverage was incomplete",
        ));
    }
    let (nsfw_score, nsfw_would_delete, nsfw_note) = if needs_nsfw {
        let (judged, note) = selection
            .judgement
            .ok_or_else(|| ModelError::runtime("animated NSFW judgement unavailable"))?;
        let policy = judged.for_policy(spare);
        let action = policy.is_some_and(|value| {
            value.score * 100.0 >= limit as f32
                && allow_delete(
                    value.innocent,
                    value.explicit,
                    spare,
                    value.weak,
                    value.confirmed,
                    value.arbiter,
                )
        });
        (policy.map(|value| value.score), Some(action), Some(note))
    } else {
        (None, None, None)
    };
    if needs_concepts && selection.margins.is_none() {
        return Err(ModelError::runtime("animated concept margins unavailable"));
    }
    if needs_custom && selection.custom_margins.is_none() {
        return Err(ModelError::runtime("animated custom margins unavailable"));
    }
    Ok(OfflineAnimatedVisual {
        frames_scored,
        nsfw_score,
        nsfw_would_delete,
        nsfw_note,
        concept_margins: selection.margins,
        custom_margins: selection.custom_margins,
    })
}

fn thumbs(media: &Media) -> (Vec<PhotoSize>, Option<i32>) {
    match media {
        Media::Photo(photo) => (photo.thumbs(), dc_of(media)),
        Media::Document(document) => {
            let document_sizes = document.thumbs();
            if let Some(cover) = document.raw.video_cover.clone() {
                let dc = match &cover {
                    grammers_client::tl::enums::Photo::Photo(photo) => Some(photo.dc_id),
                    grammers_client::tl::enums::Photo::Empty(_) => None,
                };
                let sizes = grammers_client::media::Photo::from_raw(cover).thumbs();
                let best_side = |sizes: &[PhotoSize]| {
                    sizes
                        .iter()
                        .filter(|size| usable(size))
                        .filter_map(dims)
                        .map(|(width, height)| width.min(height))
                        .max()
                        .unwrap_or(0)
                };
                if sizes.iter().any(usable)
                    && best_side(&sizes) >= best_side(&document_sizes).min(RESIZE as i32)
                {
                    return (sizes, dc);
                }
            }
            (document_sizes, dc_of(media))
        }
        Media::Sticker(sticker) => (sticker.document.thumbs(), dc_of(media)),
        Media::WebPage(page) => (
            webpage_photo(page).map_or_else(Vec::new, |photo| photo.thumbs()),
            dc_of(media),
        ),
        _ => (Vec::new(), None),
    }
}

fn webpage_photo(page: &grammers_client::media::WebPage) -> Option<grammers_client::media::Photo> {
    use grammers_client::tl::enums::WebPage as W;

    let W::Page(page) = &page.raw.webpage else {
        return None;
    };
    let photo = page.photo.clone()?;
    matches!(photo, tl::enums::Photo::Photo(_))
        .then(|| grammers_client::media::Photo::from_raw(photo))
}

fn raw_photo_media(photo: &tl::enums::Photo) -> Option<Media> {
    matches!(photo, tl::enums::Photo::Photo(_))
        .then(|| Media::Photo(grammers_client::media::Photo::from_raw(photo.clone())))
}

fn static_document_surfaces(media: &Media) -> Option<(Media, Media)> {
    let Media::Document(document) = media else {
        return None;
    };
    if !is_ordinary_raster_document(document) {
        return None;
    }
    let cover = document
        .raw
        .video_cover
        .as_ref()
        .and_then(raw_photo_media)?;
    let mut original = document.clone();
    original.raw.video_cover = None;
    Some((cover, Media::Document(original)))
}

fn raw_document_media(document: &tl::enums::Document) -> Option<Document> {
    if !matches!(document, tl::enums::Document::Document(_)) {
        return None;
    }
    Some(Document::from_raw_media(tl::types::MessageMediaDocument {
        nopremium: false,
        spoiler: false,
        video: false,
        round: false,
        voice: false,
        document: Some(document.clone()),
        alt_documents: None,
        video_cover: None,
        video_timestamp: None,
        ttl_seconds: None,
    }))
}

fn webpage_album_media(page: &tl::types::WebPage) -> Option<Vec<Media>> {
    if page.r#type.as_deref() != Some("telegram_album") {
        return None;
    }
    let Some(tl::enums::Page::Page(cached)) = &page.cached_page else {
        return None;
    };
    let items = cached.blocks.iter().find_map(|block| match block {
        tl::enums::PageBlock::Collage(collage) => Some(&collage.items),
        tl::enums::PageBlock::Slideshow(slideshow) => Some(&slideshow.items),
        _ => None,
    })?;
    let mut media = Vec::new();
    for item in items.iter().take(10) {
        let found = match item {
            tl::enums::PageBlock::Photo(item) => cached
                .photos
                .iter()
                .find(|photo| matches!(photo, tl::enums::Photo::Photo(raw) if raw.id == item.photo_id))
                .and_then(raw_photo_media),
            tl::enums::PageBlock::Video(item) => cached
                .documents
                .iter()
                .find(|document| matches!(document, tl::enums::Document::Document(raw) if raw.id == item.video_id))
                .and_then(raw_document_media)
                .map(Media::Document),
            _ => None,
        };
        if let Some(found) = found {
            media.push(found);
        }
    }
    (!media.is_empty()).then_some(media)
}

fn webpage_visual_media(page: &grammers_client::media::WebPage) -> Option<Vec<Media>> {
    let tl::enums::WebPage::Page(raw) = &page.raw.webpage else {
        return None;
    };
    if let Some(mut album) = webpage_album_media(raw) {
        if let Some(poster) = raw.photo.as_ref().and_then(raw_photo_media) {
            let poster_id = file_id(&poster);
            if !album
                .iter()
                .any(|item| matches!(item, Media::Photo(_)) && file_id(item) == poster_id)
            {
                album.push(poster);
            }
        }
        return Some(album);
    }
    let document = raw.document.as_ref().and_then(raw_document_media);
    if let Some(mut document) = document {
        if is_animated_document(&document) {
            if let Some(photo) = raw
                .photo
                .as_ref()
                .filter(|photo| matches!(photo, tl::enums::Photo::Photo(_)))
            {
                document.raw.video_cover = Some(photo.clone());
            }
            return Some(vec![Media::Document(document)]);
        }
        if is_ordinary_raster_document(&document) {
            let mut visuals = raw
                .photo
                .as_ref()
                .and_then(raw_photo_media)
                .into_iter()
                .collect::<Vec<_>>();
            visuals.push(Media::Document(document));
            return Some(visuals);
        }
        if raw.photo.as_ref().and_then(raw_photo_media).is_none() {
            return Some(vec![Media::Document(document)]);
        }
    }
    None
}

const PENDING_WEBPAGE_TTL: Duration = Duration::from_secs(240);
const PENDING_WEBPAGE_MAX: usize = 256;
const RECENT_WEBPAGE_MAX: usize = 64;
const RECENT_WEBPAGE_BYTES_MAX: usize = 128 * 1024;
const WEBPAGE_RESCAN_BATCH: usize = 8;
const WEBPAGE_RESCAN_RETRY: Duration = Duration::from_secs(5);
const WEBPAGE_REFETCH_DEADLINE: Duration = Duration::from_secs(4);

#[derive(Clone)]
struct PendingPreview {
    page_id: i64,
    generation: u64,
    at: Instant,
    retry_at: Instant,
    retried: bool,
    inflight: bool,
    resolved_seen: bool,
    next_visual: usize,
    visual_ids: Vec<Option<(i64, Option<i64>)>>,
}

struct RecentPage {
    page: tl::enums::WebPage,
    at: Instant,
}

struct PendingWork {
    chat: i64,
    message_id: i32,
    page_id: i64,
    generation: u64,
    resolved: Option<tl::enums::WebPage>,
    next_visual: usize,
    visual_ids: Vec<Option<(i64, Option<i64>)>>,
}

enum RescanMedia {
    Ready(Box<Media>),
    StillPending,
    Unrelated,
}

enum RescanOutcome {
    Done,
    StillPending,
    Retry,
    QueueFull {
        next_visual: usize,
        visual_ids: Vec<Option<(i64, Option<i64>)>>,
    },
}

#[derive(Default)]
pub(super) struct PendingWebPages {
    waiting: HashMap<(i64, i32), PendingPreview>,
    recent: HashMap<(Option<i64>, i64), RecentPage>,
    next_generation: u64,
}

struct SerializedLen(usize);

impl Extend<u8> for SerializedLen {
    fn extend<T: IntoIterator<Item = u8>>(&mut self, bytes: T) {
        self.0 = self.0.saturating_add(bytes.into_iter().count());
    }
}

fn webpage_id(page: &tl::enums::WebPage) -> Option<i64> {
    match page {
        tl::enums::WebPage::Page(page) => Some(page.id),
        tl::enums::WebPage::Pending(page) => Some(page.id),
        tl::enums::WebPage::Empty(page) => Some(page.id),
        tl::enums::WebPage::NotModified(_) => None,
    }
}

fn pending_retry_delay(date: i32, unix_now: i64) -> Duration {
    let after_start = (i64::from(date) + 30 - unix_now).clamp(20, 60);
    Duration::from_secs(after_start as u64)
}

impl PendingWebPages {
    fn prune(&mut self, now: Instant) {
        let expired = self.waiting.len();
        self.waiting
            .retain(|_, entry| now.duration_since(entry.at) < PENDING_WEBPAGE_TTL);
        let expired = expired - self.waiting.len();
        if expired != 0 {
            log::warn!("pending WebPage: {expired} previews expired before confirmed scan");
        }
        self.recent
            .retain(|_, entry| now.duration_since(entry.at) < PENDING_WEBPAGE_TTL);
    }

    fn recent_page(&self, chat: i64, page_id: i64) -> Option<&tl::enums::WebPage> {
        let scoped = self.recent.get(&(Some(chat), page_id));
        let global = self.recent.get(&(None, page_id));
        match (scoped, global) {
            (Some(scoped), Some(global)) => Some(if scoped.at >= global.at {
                &scoped.page
            } else {
                &global.page
            }),
            (Some(scoped), None) => Some(&scoped.page),
            (None, Some(global)) => Some(&global.page),
            (None, None) => None,
        }
    }

    fn register(
        &mut self,
        chat: i64,
        message_id: i32,
        page_id: i64,
        date: i32,
        now: Instant,
        unix_now: i64,
    ) {
        self.prune(now);
        if self
            .waiting
            .get(&(chat, message_id))
            .is_some_and(|entry| entry.page_id == page_id)
        {
            return;
        }
        if !self.waiting.contains_key(&(chat, message_id)) {
            if self.waiting.len() >= PENDING_WEBPAGE_MAX {
                log::warn!(
                    "pending WebPage: evicting oldest preview at capacity {PENDING_WEBPAGE_MAX}"
                );
            }
            super::make_room(&mut self.waiting, PENDING_WEBPAGE_MAX, |entry| entry.at);
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        let cached = self.recent_page(chat, page_id).is_some();
        self.waiting.insert(
            (chat, message_id),
            PendingPreview {
                page_id,
                generation: self.next_generation,
                at: now,
                retry_at: now
                    + if cached {
                        Duration::ZERO
                    } else {
                        pending_retry_delay(date, unix_now)
                    },
                retried: false,
                inflight: false,
                resolved_seen: false,
                next_visual: 0,
                visual_ids: Vec::new(),
            },
        );
    }

    fn resolved(
        &mut self,
        page: &tl::enums::WebPage,
        chat: Option<i64>,
        now: Instant,
    ) -> Option<i64> {
        let tl::enums::WebPage::Page(_) = page else {
            return None;
        };
        let page_id = webpage_id(page)?;
        self.prune(now);
        let mut bytes = SerializedLen(0);
        page.serialize(&mut bytes);
        if bytes.0 <= RECENT_WEBPAGE_BYTES_MAX {
            if !self.recent.contains_key(&(chat, page_id)) {
                super::make_room(&mut self.recent, RECENT_WEBPAGE_MAX, |entry| entry.at);
            }
            self.recent.insert(
                (chat, page_id),
                RecentPage {
                    page: page.clone(),
                    at: now,
                },
            );
        } else {
            log::warn!("pending WebPage: page {page_id} exceeds recent-cache byte cap");
        }
        for ((entry_chat, _), entry) in &mut self.waiting {
            if entry.page_id == page_id && chat.is_none_or(|chat| chat == *entry_chat) {
                entry.retry_at = now;
                if !entry.resolved_seen {
                    entry.at = now;
                    entry.retried = false;
                    entry.resolved_seen = true;
                }
            }
        }
        Some(page_id)
    }

    fn due(&mut self, now: Instant, page: Option<(i64, Option<i64>)>) -> Vec<PendingWork> {
        self.prune(now);
        let mut keys = self
            .waiting
            .iter()
            .filter(|((chat, _), entry)| {
                !entry.inflight
                    && entry.retry_at <= now
                    && page.is_none_or(|(id, scope)| {
                        entry.page_id == id && scope.is_none_or(|scope| scope == *chat)
                    })
                    && (!entry.retried
                        || self.recent_page(*chat, entry.page_id).is_some()
                        || page.is_some())
            })
            .map(|(key, entry)| (*key, entry.at))
            .collect::<Vec<_>>();
        keys.sort_by_key(|(_, at)| *at);
        keys.truncate(WEBPAGE_RESCAN_BATCH);
        keys.into_iter()
            .filter_map(|((chat, message_id), _)| {
                let page_id = self.waiting.get(&(chat, message_id))?.page_id;
                let resolved = self.recent_page(chat, page_id).cloned();
                let entry = self.waiting.get_mut(&(chat, message_id))?;
                entry.inflight = true;
                entry.retried = true;
                Some(PendingWork {
                    chat,
                    message_id,
                    page_id,
                    generation: entry.generation,
                    resolved,
                    next_visual: entry.next_visual,
                    visual_ids: entry.visual_ids.clone(),
                })
            })
            .collect()
    }

    fn finish(&mut self, work: &PendingWork, outcome: RescanOutcome, now: Instant) {
        let key = (work.chat, work.message_id);
        if self
            .waiting
            .get(&key)
            .is_none_or(|entry| entry.generation != work.generation)
        {
            return;
        }
        if matches!(outcome, RescanOutcome::Done) {
            self.waiting.remove(&key);
        } else if let Some(entry) = self.waiting.get_mut(&key) {
            entry.inflight = false;
            entry.retry_at = now + WEBPAGE_RESCAN_RETRY;
            if matches!(
                outcome,
                RescanOutcome::Retry | RescanOutcome::QueueFull { .. }
            ) {
                entry.retried = false;
            }
            if let RescanOutcome::QueueFull {
                next_visual,
                visual_ids,
            } = outcome
            {
                entry.next_visual = next_visual;
                entry.visual_ids = visual_ids;
            }
        }
    }
}

fn rescan_webpage_media(
    media: Option<&tl::enums::MessageMedia>,
    page_id: i64,
    resolved: Option<&tl::enums::WebPage>,
) -> RescanMedia {
    let Some(tl::enums::MessageMedia::WebPage(raw)) = media else {
        return RescanMedia::Unrelated;
    };
    if webpage_id(&raw.webpage) != Some(page_id) {
        return RescanMedia::Unrelated;
    }
    let mut current = raw.clone();
    match &current.webpage {
        tl::enums::WebPage::Page(_) => {}
        tl::enums::WebPage::Pending(_) => {
            let Some(tl::enums::WebPage::Page(_)) = resolved else {
                return RescanMedia::StillPending;
            };
            current.webpage = resolved.expect("checked resolved Page").clone();
        }
        _ => return RescanMedia::Unrelated,
    }
    RescanMedia::Ready(Box::new(Media::WebPage(
        grammers_client::media::WebPage::from_raw_media(current),
    )))
}

async fn rescan_pending_work(
    ctx: &Arc<Ctx>,
    work: PendingWork,
    update_page: Option<&tl::enums::WebPage>,
) {
    let outcome = rescan_pending_work_inner(ctx, &work, update_page).await;
    ctx.pending_webpages
        .lock()
        .unwrap()
        .finish(&work, outcome, Instant::now());
}

async fn rescan_pending_work_inner(
    ctx: &Arc<Ctx>,
    work: &PendingWork,
    update_page: Option<&tl::enums::WebPage>,
) -> RescanOutcome {
    let Some(peer) = ctx.resolve_group(work.chat).await else {
        return RescanOutcome::Retry;
    };
    let state = match ctx.admit_chat(work.chat, peer).await {
        Ok(state) => state,
        Err(error) => {
            ctx.admission_failed(work.chat, error);
            return RescanOutcome::Retry;
        }
    };
    let _slot = state.slot().await;
    let fetched = match tokio::time::timeout(
        WEBPAGE_REFETCH_DEADLINE,
        ctx.client.get_messages_by_id(peer, &[work.message_id]),
    )
    .await
    {
        Ok(Ok(mut messages)) => messages.pop().flatten(),
        Ok(Err(error)) => {
            log::warn!(
                "pending WebPage: fetch failed for {}:{}: {error}",
                work.chat,
                work.message_id
            );
            return RescanOutcome::Retry;
        }
        Err(_) => {
            log::warn!(
                "pending WebPage: fetch timed out for {}:{}",
                work.chat,
                work.message_id
            );
            return RescanOutcome::Retry;
        }
    };
    let Some(message) = fetched else {
        return RescanOutcome::Done;
    };
    if message.id() != work.message_id || message.peer_id().bot_api_dialog_id() != Some(work.chat) {
        return RescanOutcome::Done;
    }
    let tl::enums::Message::Message(raw) = &message.raw else {
        return RescanOutcome::Done;
    };
    let resolved = update_page.or(work.resolved.as_ref());
    match rescan_webpage_media(raw.media.as_ref(), work.page_id, resolved) {
        RescanMedia::Ready(media) => match watch_pending_media(ctx, &message, work, &media).await {
            Ok(()) => RescanOutcome::Done,
            Err((next_visual, visual_ids)) => RescanOutcome::QueueFull {
                next_visual,
                visual_ids,
            },
        },
        RescanMedia::StillPending => RescanOutcome::StillPending,
        RescanMedia::Unrelated => RescanOutcome::Done,
    }
}

pub async fn on_webpage_update(ctx: &Arc<Ctx>, raw: &grammers_client::update::Raw) {
    let (page, scope) = match &raw.raw {
        tl::enums::Update::WebPage(update) => (&update.webpage, None),
        tl::enums::Update::ChannelWebPage(update) => {
            let Some(chat) = PeerId::channel(update.channel_id).and_then(PeerId::bot_api_dialog_id)
            else {
                return;
            };
            (&update.webpage, Some(chat))
        }
        _ => return,
    };
    let now = Instant::now();
    let work = {
        let mut pending = ctx.pending_webpages.lock().unwrap();
        let Some(page_id) = pending.resolved(page, scope, now) else {
            return;
        };
        pending.due(now, Some((page_id, scope)))
    };
    for item in work {
        rescan_pending_work(ctx, item, Some(page)).await;
    }
}

pub async fn sweep_pending_webpages(ctx: &Arc<Ctx>) {
    let work = ctx
        .pending_webpages
        .lock()
        .unwrap()
        .due(Instant::now(), None);
    for item in work {
        rescan_pending_work(ctx, item, None).await;
    }
}

fn live_photo_media(photo: &tl::types::MessageMediaPhoto) -> Option<Media> {
    let mut document = raw_document_media(photo.video.as_ref()?)?;
    if !is_animated_document(&document) {
        return None;
    }
    document.raw.video_cover = photo
        .photo
        .as_ref()
        .filter(|photo| matches!(photo, tl::enums::Photo::Photo(_)))
        .cloned();
    Some(Media::Document(document))
}

fn embedded_visual_media_raw(media: &tl::enums::MessageMedia) -> Option<Media> {
    match media {
        tl::enums::MessageMedia::Game(game) => game_visual_media(game).into_iter().next(),
        tl::enums::MessageMedia::Story(story) => {
            let tl::enums::StoryItem::Item(story) = story.story.as_ref()? else {
                return None;
            };
            if let tl::enums::MessageMedia::Photo(photo) = &story.media
                && let Some(media) = live_photo_media(photo)
            {
                return Some(media);
            }
            Media::from_raw(story.media.clone())
        }
        _ => None,
    }
}

fn embedded_visual_media(message: &Message) -> Option<Media> {
    let tl::enums::Message::Message(raw) = &message.raw else {
        return None;
    };
    embedded_visual_media_raw(raw.media.as_ref()?)
}

fn direct_visual_media(raw: &tl::enums::MessageMedia) -> Option<Media> {
    match raw {
        tl::enums::MessageMedia::Photo(photo) => {
            live_photo_media(photo).or_else(|| Media::from_raw(raw.clone()))
        }
        tl::enums::MessageMedia::Document(_) | tl::enums::MessageMedia::WebPage(_) => {
            Media::from_raw(raw.clone())
        }
        tl::enums::MessageMedia::Game(_) | tl::enums::MessageMedia::Story(_) => {
            embedded_visual_media_raw(raw)
        }
        _ => None,
    }
}

fn extended_visual_media(raw: &tl::enums::MessageExtendedMedia) -> Option<Media> {
    match raw {
        tl::enums::MessageExtendedMedia::Media(full) => direct_visual_media(&full.media),
        tl::enums::MessageExtendedMedia::Preview(_) => None,
    }
}

fn game_visual_media(raw: &tl::types::MessageMediaGame) -> Vec<Media> {
    let tl::enums::Game::Game(game) = &raw.game;
    let photo = raw_photo_media(&game.photo);
    let Some(mut document) = game.document.as_ref().and_then(raw_document_media) else {
        return photo.into_iter().collect();
    };
    if is_animated_document(&document) {
        document.raw.video_cover =
            matches!(&game.photo, tl::enums::Photo::Photo(_)).then(|| game.photo.clone());
        return vec![Media::Document(document)];
    }
    let mut visuals = photo.into_iter().collect::<Vec<_>>();
    if is_still_image_document(&document) {
        visuals.push(Media::Document(document));
    }
    visuals
}

fn poll_visual_media(poll: &tl::types::MessageMediaPoll) -> Vec<Media> {
    let tl::enums::PollResults::Results(results) = &poll.results;
    [
        poll.attached_media.as_ref(),
        results.solution_media.as_ref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(direct_visual_media)
    .collect()
}

fn invoice_visual_media(
    invoice: &tl::types::MessageMediaInvoice,
) -> (Option<Media>, Option<&tl::enums::WebDocument>) {
    (
        invoice
            .extended_media
            .as_ref()
            .and_then(extended_visual_media),
        invoice.photo.as_ref(),
    )
}

fn direct_invoice_web_photo(
    photo: &tl::enums::WebDocument,
) -> Option<&tl::types::WebDocumentNoProxy> {
    match photo {
        tl::enums::WebDocument::NoProxy(photo) => Some(photo),
        tl::enums::WebDocument::Document(_) => None,
    }
}

fn embedded_visual_media_set_raw(media: &tl::enums::MessageMedia) -> Option<Vec<Media>> {
    match media {
        tl::enums::MessageMedia::Game(game) => Some(game_visual_media(game)),
        tl::enums::MessageMedia::Poll(poll) => Some(poll_visual_media(poll)),
        tl::enums::MessageMedia::PaidMedia(paid) => Some(
            paid.extended_media
                .iter()
                .take(10)
                .filter_map(extended_visual_media)
                .collect(),
        ),
        tl::enums::MessageMedia::Invoice(invoice) => {
            Some(invoice_visual_media(invoice).0.into_iter().collect())
        }
        _ => None,
    }
}

fn why_skipped(media: &Media) -> String {
    let (sizes, _) = thumbs(media);
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

#[derive(Clone)]
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

    fn source(&self, at: usize) -> VisionSource {
        photo_source(&self.sizes[at])
    }

    fn source_index(&self, source: &VisionSource) -> Option<usize> {
        self.sizes
            .iter()
            .position(|size| photo_source(size) == *source)
    }
}

fn photo_source(size: &PhotoSize) -> VisionSource {
    let (variant, payload_hash) = match size {
        PhotoSize::Size(_) => (0, 0),
        PhotoSize::Cached(size) => (1, source_hash(&size.bytes)),
        PhotoSize::Stripped(size) => (2, source_hash(&size.bytes)),
        PhotoSize::Progressive(size) => (3, source_hash(&size.sizes)),
        PhotoSize::Empty(_) => (4, 0),
        PhotoSize::Path(size) => (5, source_hash(&size.bytes)),
    };
    let (width, height) = dims(size).unwrap_or_default();
    VisionSource::Rung {
        photo_type: size.photo_type().to_owned(),
        width,
        height,
        declared_bytes: size.size(),
        variant,
        payload_hash,
    }
}

fn source_hash(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn scoring_source(
    nsfw_armed: bool,
    cached_verdict: Option<&(Judgement, VisionSource)>,
    animated: bool,
    ladder: Option<&Ladder>,
    original: bool,
) -> Option<VisionSource> {
    if nsfw_armed && let Some((_, source)) = cached_verdict {
        return source_available(source, animated, ladder, original).then(|| source.clone());
    }
    if animated {
        return Some(VisionSource::Animated);
    }
    let start = ladder
        .map(|ladder| ladder.source(ladder.start))
        .or_else(|| original.then_some(VisionSource::Original))?;
    if nsfw_armed && ladder.is_some_and(|ladder| ladder.sizes.len() > ladder.start + 1) {
        None
    } else {
        Some(start)
    }
}

fn source_available(
    source: &VisionSource,
    animated: bool,
    ladder: Option<&Ladder>,
    original: bool,
) -> bool {
    match source {
        VisionSource::Animated => animated,
        VisionSource::Original => !animated && original,
        VisionSource::Rung { .. } => {
            !animated && ladder.is_some_and(|ladder| ladder.source_index(source).is_some())
        }
    }
}

fn verdict_route(animated: bool, ladder: Option<&Ladder>, original: bool) -> Option<VisionRoute> {
    if animated {
        return Some(VisionRoute::Animated);
    }
    ladder
        .map(|ladder| VisionRoute::Rungs {
            start: ladder.source(ladder.start),
            sharper: (ladder.sizes.len() > ladder.start + 1)
                .then(|| ladder.source(ladder.start + 1)),
        })
        .or_else(|| original.then_some(VisionRoute::Original))
}

fn ladder_from(sizes: Vec<PhotoSize>, dc: Option<i32>) -> Option<Ladder> {
    let sizes: Vec<PhotoSize> = sizes.into_iter().filter(usable).collect();
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
    Some(Ladder { sizes, dc, start })
}

pub fn ladder(media: &Media) -> Option<Ladder> {
    let (sizes, dc) = thumbs(media);
    ladder_from(sizes, dc)
}

fn visual_ladder(media: &Media, force_original: bool) -> Option<Ladder> {
    (!force_original).then(|| ladder(media)).flatten()
}

fn explicit_video_cover_ladder(document: &Document) -> Option<Ladder> {
    let cover = document.raw.video_cover.clone()?;
    let dc = match &cover {
        grammers_client::tl::enums::Photo::Photo(photo) => Some(photo.dc_id),
        grammers_client::tl::enums::Photo::Empty(_) => None,
    };
    ladder_from(grammers_client::media::Photo::from_raw(cover).thumbs(), dc)
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
    spare_frame: Option<Option<SpareFrame>>,
}

#[derive(Clone, Copy, Debug)]
struct SpareFrame {
    score: f32,
    innocent: bool,
    explicit: bool,
    confirmed: bool,
    weak: bool,
    arbiter: Option<Arbiter>,
}

impl Judgement {
    fn with_animated_spare(mut self, frame: Option<Self>) -> Self {
        self.spare_frame = Some(frame.map(|frame| SpareFrame {
            score: frame.score,
            innocent: frame.innocent,
            explicit: frame.explicit,
            confirmed: frame.confirmed,
            weak: frame.weak,
            arbiter: frame.arbiter,
        }));
        self
    }

    fn for_policy(self, spare: bool) -> Option<Self> {
        match (spare, self.spare_frame) {
            (true, Some(Some(frame))) => Some(Self {
                score: frame.score,
                innocent: frame.innocent,
                explicit: frame.explicit,
                confirmed: frame.confirmed,
                weak: frame.weak,
                arbiter: frame.arbiter,
                spare_frame: None,
            }),
            (true, Some(None)) => None,
            _ => Some(self),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct OfflineNsfwEvaluation {
    pub width: u32,
    pub height: u32,
    pub view_scores: [Option<f32>; 3],
    pub score: f32,
    pub peak: f32,
    pub frail: bool,
    pub grader_hard: Option<f32>,
    pub grader_sexy: Option<f32>,
    pub grader_neutral: Option<f32>,
    pub grader_drawn: Option<f32>,
    pub arbiter_explicit: Option<f32>,
    pub arbiter_revealing: Option<f32>,
    pub fitted_head: Option<f32>,
    pub fitted_veto: Option<f32>,
    pub arbiter_requested: bool,
    pub crop_probe_view: Option<usize>,
    pub crop_probe_grader_hard: Option<f32>,
    pub crop_probe_arbiter_explicit: Option<f32>,
    pub crop_probe_head: Option<f32>,
    pub crop_rescued: bool,
    pub innocent: bool,
    pub explicit: bool,
    pub confirmed: bool,
    pub weak: bool,
    pub limit: u32,
    pub spare: bool,
    pub would_delete: bool,
    pub classifier_ms: u64,
    pub grader_ms: Option<u64>,
    pub arbiter_ms: Option<u64>,
    pub crop_probe_ms: Option<u64>,
    pub total_ms: u64,
}

fn elapsed_ms(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn assemble_judgement(
    look: &Look,
    grade: Option<&Grade>,
    arbiter: Option<Arbiter>,
) -> Result<Judgement, ModelError> {
    if !look.complete {
        return Err(ModelError::runtime(
            "NSFW mirrored verification was incomplete",
        ));
    }
    let (innocent, mut explicit, confirmed, weak) = if look.score >= GRADE_ABOVE {
        let grade = grade.ok_or_else(|| {
            ModelError::runtime("NSFW grader was required but produced no result")
        })?;
        (
            innocent(grade, look.score),
            grade.explicit(),
            confirms(look.score, grade),
            look.frail && !grade.corroborates(),
        )
    } else {
        (false, true, look.score >= CONFIDENT, false)
    };

    let mut score = if arbiter.is_some_and(|value| value.sure()) {
        look.peak
    } else {
        look.score
    };
    if let Some(value) = arbiter.filter(|value| value.head_deletes()) {
        score = score.max(value.head);
        explicit = true;
    }
    let arbiter = arbiter.map(|value| Arbiter {
        stack: stack_of(&stack_features(score, look.peak, look.frail, grade, &value)),
        ..value
    });
    Ok(Judgement {
        score,
        innocent,
        explicit,
        confirmed,
        weak,
        arbiter,
        spare_frame: None,
    })
}

fn arbiter_from_embedding(
    embedding: Result<Option<Vec<f32>>, ModelError>,
) -> Result<Option<Arbiter>, ModelError> {
    embedding.map(|embedding| embedding.as_deref().map(arbiter_of))
}

fn still_crop_candidate(look: &Look, full: Judgement) -> Option<(usize, f32)> {
    if !look.complete
        || full.arbiter.is_none()
        || allow_delete(
            full.innocent,
            full.explicit,
            false,
            full.weak,
            full.confirmed,
            full.arbiter,
        )
    {
        return None;
    }
    let whole = look.view_scores[0]?;
    [(1, look.view_scores[1]), (2, look.view_scores[2])]
        .into_iter()
        .filter_map(|(at, score)| score.map(|score| (at, score)))
        .filter(|(_, score)| *score >= BRIDGE_SCORE && *score > whole)
        .max_by(|left, right| left.1.total_cmp(&right.1))
}

struct CropProbe {
    judged: Option<Judgement>,
    grade: Grade,
    arbiter: Option<Arbiter>,
    frail: bool,
}

fn probe_still_crop_with<G, A>(
    image: &image::RgbImage,
    at: usize,
    score: f32,
    grade_with: G,
    arbiter_with: A,
) -> Result<CropProbe, ModelError>
where
    G: FnOnce(&image::RgbImage) -> Result<Option<Grade>, ModelError>,
    A: FnOnce(&image::RgbImage) -> Result<Option<Arbiter>, ModelError>,
{
    let regions = tiles(image);
    let crop = regions
        .get(at)
        .ok_or_else(|| ModelError::contract("NSFW crop view is missing"))?
        .as_ref();
    let frail = frail_of(crop.width().min(crop.height()), sharpness(crop));
    let grade =
        grade_with(crop)?.ok_or_else(|| ModelError::runtime("NSFW crop grader unavailable"))?;
    if grade.hard < EXPLICIT_FLOOR {
        return Ok(CropProbe {
            judged: None,
            grade,
            arbiter: None,
            frail,
        });
    }
    let arbiter =
        arbiter_with(crop)?.ok_or_else(|| ModelError::runtime("NSFW crop arbiter unavailable"))?;
    let crop_look = Look {
        score,
        peak: score,
        view_scores: [Some(score), None, None],
        frail,
        note: format!("crop view {at}"),
        complete: true,
    };
    let judged = assemble_judgement(&crop_look, Some(&grade), Some(arbiter))?;
    let rescued = judged.explicit
        && allow_delete(
            judged.innocent,
            judged.explicit,
            true,
            judged.weak,
            judged.confirmed,
            judged.arbiter,
        );
    Ok(CropProbe {
        judged: rescued.then_some(judged),
        grade,
        arbiter: Some(arbiter),
        frail,
    })
}

pub fn evaluate_image_offline(
    image: &image::RgbImage,
    limit: u32,
    spare: bool,
) -> Result<OfflineNsfwEvaluation, ModelError> {
    if !(LIMIT_RANGE.0..=LIMIT_RANGE.1).contains(&limit) {
        return Err(ModelError::contract(format!(
            "NSFW evaluation limit must be in {}..={}, got {limit}",
            LIMIT_RANGE.0, LIMIT_RANGE.1
        )));
    }
    let total_started = std::time::Instant::now();
    let classifier_started = std::time::Instant::now();
    let look = look(image)?.ok_or_else(|| ModelError::runtime("NSFW classifier unavailable"))?;
    let classifier_ms = elapsed_ms(classifier_started);

    let (mut grade, grader_ms) = if look.score >= GRADE_ABOVE {
        let started = std::time::Instant::now();
        let grade = grade(image)?.ok_or_else(|| ModelError::runtime("NSFW grader unavailable"))?;
        (Some(grade), Some(elapsed_ms(started)))
    } else {
        (None, None)
    };

    let arbiter_requested = look.peak >= GRADE_ABOVE;
    let (mut arbiter, arbiter_ms) = if arbiter_requested {
        let started = std::time::Instant::now();
        let arbiter = arbiter_from_embedding(super::vision::embed_of(image))?;
        (arbiter, Some(elapsed_ms(started)))
    } else {
        (None, None)
    };
    let mut judged = assemble_judgement(&look, grade.as_ref(), arbiter)?;
    let mut frail = look.frail;
    let mut crop_probe_view = None;
    let mut crop_probe_grader_hard = None;
    let mut crop_probe_arbiter_explicit = None;
    let mut crop_probe_head = None;
    let mut crop_probe_ms = None;
    let mut crop_rescued = false;
    if let Some((at, score)) = still_crop_candidate(&look, judged) {
        let started = std::time::Instant::now();
        let probe = probe_still_crop_with(image, at, score, self::grade, |crop| {
            arbiter_from_embedding(super::vision::embed_of(crop))
        })?;
        crop_probe_ms = Some(elapsed_ms(started));
        crop_probe_view = Some(at);
        crop_probe_grader_hard = Some(probe.grade.hard);
        crop_probe_arbiter_explicit = probe.arbiter.map(|value| value.explicit);
        crop_probe_head = probe.arbiter.map(|value| value.head);
        if let Some(rescued) = probe.judged {
            judged = rescued;
            grade = Some(probe.grade);
            arbiter = probe.arbiter;
            frail = probe.frail;
            crop_rescued = true;
        }
    }
    let would_delete = judged.score * 100.0 >= limit as f32
        && allow_delete(
            judged.innocent,
            judged.explicit,
            spare,
            judged.weak,
            judged.confirmed,
            judged.arbiter,
        );

    Ok(OfflineNsfwEvaluation {
        width: image.width(),
        height: image.height(),
        view_scores: look.view_scores,
        score: judged.score,
        peak: look.peak,
        frail,
        grader_hard: grade.as_ref().map(|value| value.hard),
        grader_sexy: grade.as_ref().map(|value| value.sexy),
        grader_neutral: grade.as_ref().map(|value| value.neutral),
        grader_drawn: grade.as_ref().map(|value| value.drawn),
        arbiter_explicit: arbiter.map(|value| value.explicit),
        arbiter_revealing: arbiter.map(|value| value.revealing),
        fitted_head: arbiter.map(|value| value.head),
        fitted_veto: judged.arbiter.map(|value| value.stack),
        arbiter_requested,
        crop_probe_view,
        crop_probe_grader_hard,
        crop_probe_arbiter_explicit,
        crop_probe_head,
        crop_rescued,
        innocent: judged.innocent,
        explicit: judged.explicit,
        confirmed: judged.confirmed,
        weak: judged.weak,
        limit,
        spare,
        would_delete,
        classifier_ms,
        grader_ms,
        arbiter_ms,
        crop_probe_ms,
        total_ms: elapsed_ms(total_started),
    })
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
    if let tl::enums::Message::Message(raw) = &message.raw
        && let Some(tl::enums::MessageMedia::Photo(photo)) = raw.media.as_ref()
        && let Some(media) = live_photo_media(photo)
    {
        watch_media(ctx, message, chat, &media).await;
        return;
    }
    if let tl::enums::Message::Message(raw) = &message.raw
        && let Some(tl::enums::MessageMedia::Invoice(invoice)) = raw.media.as_ref()
    {
        let (extended, web_photo) = invoice_visual_media(invoice);
        if let Some(media) = extended {
            let _ = watch_media(ctx, message, chat, &media).await;
        }
        if let Some(photo) = web_photo {
            let _ = watch_invoice_photo(ctx, message, chat, photo).await;
        }
        return;
    }
    if let tl::enums::Message::Message(raw) = &message.raw
        && let Some(visuals) = raw.media.as_ref().and_then(embedded_visual_media_set_raw)
    {
        for media in visuals {
            if !watch_media(ctx, message, chat, &media).await {
                break;
            }
        }
        return;
    }
    if let Some(media) = view.media() {
        if let Media::WebPage(page) = media
            && let tl::enums::WebPage::Pending(pending) = &page.raw.webpage
        {
            if armed(ctx, chat) {
                let unix_now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |time| time.as_secs() as i64);
                ctx.pending_webpages.lock().unwrap().register(
                    chat,
                    message.id(),
                    pending.id,
                    pending.date,
                    Instant::now(),
                    unix_now,
                );
            }
            return;
        }
        let _ = watch_media(ctx, message, chat, media).await;
    } else if let Some(media) = embedded_visual_media(message) {
        let _ = watch_media(ctx, message, chat, &media).await;
    }
}

async fn watch_media(ctx: &Arc<Ctx>, message: &Message, chat: i64, media: &Media) -> bool {
    if let Media::WebPage(page) = media
        && let Some(visuals) = webpage_visual_media(page)
    {
        for visual in &visuals {
            if !watch_one(ctx, message, chat, visual, webpage_original(visual)).await {
                return false;
            }
        }
        return true;
    }
    if let Some((cover, original)) = static_document_surfaces(media) {
        let original_ready = watch_one(ctx, message, chat, &original, true).await;
        let cover_ready = watch_one(ctx, message, chat, &cover, false).await;
        return cover_ready && original_ready;
    }
    let sticker_itself = sticker_for_faces(ctx, chat, media).await;
    watch_one(ctx, message, chat, media, sticker_itself).await
}

async fn sticker_for_faces(ctx: &Arc<Ctx>, chat: i64, media: &Media) -> bool {
    let Media::Sticker(sticker) = media else {
        return false;
    };
    if !is_still_image_document(&sticker.document) || !super::imgfilter::any(ctx, chat) {
        return false;
    }
    ctx.image_filters(chat)
        .await
        .is_ok_and(|filters| filters.iter().any(|filter| filter.is_face()))
}

fn webpage_original(media: &Media) -> bool {
    matches!(media, Media::Document(document) if is_ordinary_raster_document(document))
}

fn pending_visual_key(media: &Media) -> Option<(i64, Option<i64>)> {
    let file = file_id(media)?;
    let cover = if let Media::Document(document) = media {
        match document.raw.video_cover.as_ref() {
            Some(tl::enums::Photo::Photo(photo)) => Some(photo.id),
            _ => None,
        }
    } else {
        None
    };
    Some((file, cover))
}

fn pending_resume_index(work: &PendingWork, visual_ids: &[Option<(i64, Option<i64>)>]) -> usize {
    if work.visual_ids == visual_ids {
        work.next_visual.min(visual_ids.len())
    } else {
        0
    }
}

async fn watch_pending_media(
    ctx: &Arc<Ctx>,
    message: &Message,
    work: &PendingWork,
    media: &Media,
) -> Result<(), (usize, Vec<Option<(i64, Option<i64>)>>)> {
    let visuals = if let Media::WebPage(page) = media {
        webpage_visual_media(page).unwrap_or_else(|| vec![media.clone()])
    } else {
        vec![media.clone()]
    };
    let visual_ids = visuals.iter().map(pending_visual_key).collect::<Vec<_>>();
    let start = pending_resume_index(work, &visual_ids);
    for (at, visual) in visuals.iter().enumerate().skip(start) {
        if !watch_one(ctx, message, work.chat, visual, webpage_original(visual)).await {
            return Err((at, visual_ids));
        }
    }
    Ok(())
}

pub fn armed(ctx: &Ctx, chat: i64) -> bool {
    ctx.settings.with_chat(chat, |settings| {
        settings.is_locked(LOCK)
            || settings.is_locked(super::ocr::LOCK)
            || super::concepts::CONCEPTS
                .iter()
                .any(|concept| settings.is_locked(concept.key))
    }) || super::imgfilter::any(ctx, chat)
}

fn armed_image_policies(
    ctx: &Ctx,
    chat: i64,
) -> (
    Option<Armed>,
    Option<super::concepts::Armed>,
    Option<AdvertArmed>,
    bool,
) {
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
    (nsfw, concepts, advert, super::imgfilter::any(ctx, chat))
}

async fn watch_invoice_photo(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    photo: &tl::enums::WebDocument,
) -> bool {
    let (nsfw, concepts, advert, custom) = armed_image_policies(ctx, chat);
    if nsfw.is_none() && concepts.is_none() && advert.is_none() && !custom {
        return true;
    }
    if super::is_exempt(ctx, message).await {
        return true;
    }
    let filters = if custom {
        match ctx.image_filters(chat).await {
            Ok(filters) => filters,
            Err(error) => {
                log::error!(
                    "image filters: could not load policy for {chat}; skipping custom filters for this update: {error}"
                );
                no_filters()
            }
        }
    } else {
        no_filters()
    };
    if nsfw.is_none() && concepts.is_none() && advert.is_none() && filters.is_empty() {
        return true;
    }
    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return true;
    };
    let id = -i64::from(message.id());
    let Some(task_slot) = ctx.try_nsfw_task_slot() else {
        log::warn!(
            "image moderation: chat {chat} msg {} invoice photo queue full",
            message.id()
        );
        return false;
    };
    let classification = Classification {
        ctx: Arc::clone(ctx),
        chat,
        chat_ref,
        message_id: message.id(),
        media_id: id,
        ladder: None,
        animation_cover: None,
        personalized_cover: false,
        gif_document: None,
        still_document: None,
        raster_probe_document: None,
        web_photo: Some(photo.clone()),
        kind: "invoice_web",
        nsfw,
        concepts,
        advert,
        verdict: None,
        margins: None,
        reading: None,
        filters,
        custom_margins: None,
        sender: message.sender_id().and_then(PeerId::bare_id),
        name: super::name_of(message),
    };
    Arc::clone(ctx).spawn_owned(async move {
        let _task_slot = task_slot;
        classify(classification).await;
    });
    true
}

async fn watch_one(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    media: &Media,
    force_original: bool,
) -> bool {
    let Some(id) = file_id(media) else {
        return true;
    };
    let metadata_animation = animated_document(media);
    let raster_document = ordinary_raster_document(media);
    let known_raster_animation = raster_document.and_then(|_| ctx.known_raster_animation(id));
    let personalized_cover = metadata_animation
        .or(raster_document)
        .is_some_and(|document| document.raw.video_cover.is_some());
    let route = raster_cache_route(
        metadata_animation.is_some(),
        raster_document.is_some(),
        known_raster_animation,
        personalized_cover,
    );
    let raster_unresolved = route.probe;
    let animated_document = metadata_animation.or_else(|| {
        known_raster_animation
            .filter(|animated| *animated)
            .and(raster_document)
    });
    let animated_media = route.animated;

    let (nsfw, concepts, advert, custom) = armed_image_policies(ctx, chat);
    if nsfw.is_none() && concepts.is_none() && advert.is_none() && !custom {
        return true;
    }

    let ladder = visual_ladder(media, force_original);
    let still_document = animated_document
        .is_none()
        .then(|| still_image_without_thumb(media, ladder.is_some()))
        .flatten();
    let known_original_animation = still_document
        .as_ref()
        .and_then(|_| ctx.known_raster_animation(id));
    let original_route = raster_cache_route(
        false,
        still_document.is_some(),
        known_original_animation,
        false,
    );
    let gif_document = if original_route.animated {
        still_document.clone()
    } else {
        animated_document.cloned()
    };
    let animated_media = animated_media || original_route.animated;
    let still_document = (!original_route.animated)
        .then_some(still_document)
        .flatten();
    let cache_ready = route.cache_ready && original_route.cache_ready;

    let current_route = verdict_route(animated_media, ladder.as_ref(), still_document.is_some());
    let verdict = (cache_ready && nsfw.is_some())
        .then(|| {
            current_route
                .as_ref()
                .and_then(|route| ctx.known_verdict(id, route))
        })
        .flatten()
        .filter(|(_, source)| {
            source_available(
                source,
                animated_media,
                ladder.as_ref(),
                still_document.is_some(),
            )
        });
    let source = scoring_source(
        nsfw.is_some(),
        verdict.as_ref(),
        animated_media,
        ladder.as_ref(),
        still_document.is_some(),
    );
    let margins = cache_ready
        .then(|| {
            source
                .as_ref()
                .and_then(|source| ctx.known_margins(id, source))
        })
        .flatten();
    let advert_source = scoring_source(
        false,
        None,
        animated_media,
        ladder.as_ref(),
        still_document.is_some(),
    );
    let reading = cache_ready
        .then(|| {
            advert_source
                .as_ref()
                .and_then(|source| ctx.known_advert(id, source))
        })
        .flatten();
    let wants_nsfw = nsfw.is_some() && verdict.is_none();
    let wants_concepts = match (&concepts, &margins) {
        (Some(armed), Some(all)) => {
            super::concepts::needs_look(ctx, id, source.as_ref(), all, armed)
        }
        (Some(_), None) => true,
        (None, _) => false,
    };
    let wants_text = advert.is_some() && reading.is_none();

    let filters = match custom {
        true => match ctx.image_filters(chat).await {
            Ok(filters) => filters,
            Err(error) => {
                log::error!(
                    "image filters: could not load policy for {chat}; skipping custom filters for this update: {error}"
                );
                no_filters()
            }
        },
        false => no_filters(),
    };
    let custom_margins = cache_ready
        .then(|| {
            source
                .as_ref()
                .and_then(|source| super::imgfilter::cached_margins(ctx, id, &filters, source))
        })
        .flatten();
    let wants_custom = !filters.is_empty() && custom_margins.is_none();

    if super::is_exempt(ctx, message).await {
        return true;
    }

    if !wants_nsfw && !wants_concepts && !wants_text && !wants_custom {
        if let (Some(armed), Some((judged, _))) = (&nsfw, verdict) {
            act(ctx, message, chat, id, judged, armed, true).await;
        }
        if let (Some(armed), Some(all)) = (&concepts, margins) {
            let judged = super::concepts::with_verdicts(
                ctx,
                id,
                source.as_ref(),
                &all,
                armed,
                &std::collections::HashMap::new(),
            );
            super::concepts::act_known(ctx, message, chat, id, &judged, armed, true).await;
        }
        if let (Some(armed), Some(verdict)) = (&advert, reading) {
            act_advert(ctx, message, chat, id, verdict.why(), armed, true).await;
        }
        if let Some(margins) = &custom_margins
            && !filters.is_empty()
        {
            super::imgfilter::act_known(ctx, message, chat, id, &filters, margins).await;
        }
        return true;
    }

    let raster_probe_document = raster_unresolved
        .then(|| raster_document.cloned())
        .flatten();
    let animation_cover = if personalized_cover {
        gif_document
            .as_ref()
            .or(raster_probe_document.as_ref())
            .and_then(explicit_video_cover_ladder)
    } else {
        ladder.clone()
    };
    let ladder = if personalized_cover {
        animation_cover.clone().or(ladder)
    } else {
        ladder
    };
    if ladder.is_none()
        && gif_document.is_none()
        && still_document.is_none()
        && raster_probe_document.is_none()
    {
        log::info!(
            "nsfw: {chat} file {id} skipped, kind {}, {}",
            kind_of(media),
            why_skipped(media)
        );
        return true;
    }

    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return true;
    };
    let message_id = message.id();
    let kind = kind_of(media);
    let sender = message.sender_id().and_then(PeerId::bare_id);
    let name = super::name_of(message);
    let ctx = Arc::clone(ctx);
    let Some(task_slot) = ctx.try_nsfw_task_slot() else {
        log::warn!("image moderation: chat {chat} file {id} skipped because its queue is full");
        return false;
    };
    if !ctx.claim_media(id, || RacingCopy {
        message: message.clone(),
        chat,
        media: media.clone(),
        force_original,
    }) {
        return true;
    }
    let claim = MediaClaim {
        ctx: Arc::clone(&ctx),
        id,
        settled: false,
    };

    Arc::clone(&ctx).spawn_owned(async move {
        let _task_slot = task_slot;
        let mut claim = claim;
        let owner = Arc::clone(&ctx);
        classify(Classification {
            ctx,
            chat,
            chat_ref,
            message_id,
            media_id: id,
            ladder,
            animation_cover,
            personalized_cover,
            gif_document,
            still_document,
            raster_probe_document,
            web_photo: None,
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
        })
        .await;
        claim.settled = true;
        for copy in owner.settle_media(id) {
            owner.retry_racing_copy(copy);
        }
    });
    true
}

pub async fn run_racing_copies(
    ctx: Arc<Ctx>,
    mut copies: tokio::sync::mpsc::Receiver<RacingCopy>,
    mut stopping: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = stopping.changed() => break,
            copy = copies.recv() => {
                let Some(copy) = copy else { break };
                let owner = Arc::clone(&ctx);
                Arc::clone(&ctx).spawn_owned(async move {
                    watch_one(
                        &owner,
                        &copy.message,
                        copy.chat,
                        &copy.media,
                        copy.force_original,
                    )
                    .await;
                });
            }
        }
    }
}

async fn fetch_still(
    ctx: &Arc<Ctx>,
    (chat, id, kind): (i64, i64, &'static str),
    web_still_bytes: &mut Option<Vec<u8>>,
    ladder: Option<&Ladder>,
    still_document: Option<&Document>,
    probed_full_bytes: &mut Option<Vec<u8>>,
) -> Option<(Vec<u8>, String)> {
    if let Some(bytes) = web_still_bytes.take() {
        Some((bytes, "original".to_owned()))
    } else if let Some(ladder) = ladder {
        let bytes = fetch(ctx, chat, id, ladder, ladder.start).await?;
        let pixels = dims(&ladder.sizes[ladder.start]).map_or_else(
            || "?x?".to_owned(),
            |(width, height)| format!("{width}x{height}"),
        );
        Some((bytes, pixels))
    } else if let Some(document) = still_document {
        let bytes = if let Some(bytes) = probed_full_bytes.take() {
            (bytes.len() <= THUMBNAIL_BYTES_MAX).then_some(bytes)
        } else {
            fetch_document(ctx, chat, id, document, THUMBNAIL_BYTES_MAX).await
        };
        Some((bytes?, "original".to_owned()))
    } else {
        log::info!(
            "nsfw: {chat} file {id} skipped, kind {}, animated media had no usable decoded frame",
            kind
        );
        None
    }
}

pub struct RacingCopy {
    message: Message,
    chat: i64,
    media: Media,
    force_original: bool,
}

struct MediaClaim {
    ctx: Arc<Ctx>,
    id: i64,
    settled: bool,
}

impl Drop for MediaClaim {
    fn drop(&mut self) {
        if !self.settled {
            self.ctx.settle_media(self.id);
        }
    }
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

async fn embed(
    ctx: &Arc<Ctx>,
    image: &Arc<image::RgbImage>,
    remember: bool,
) -> Result<Option<Vec<f32>>, ModelError> {
    let frame = Arc::clone(image);
    let embedding = match tokio::task::spawn_blocking(move || super::vision::embed_of(&frame)).await
    {
        Ok(Ok(Some(embedding))) => embedding,
        Ok(Ok(None)) => return Ok(None),
        Ok(Err(error)) => return Err(error),
        Err(error) => {
            return Err(ModelError::runtime(format!(
                "vision embedding worker: {error}"
            )));
        }
    };
    if remember {
        ctx.remember_sample(&embedding);
    }
    Ok(Some(embedding))
}

struct Classification {
    ctx: Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    media_id: i64,
    ladder: Option<Ladder>,
    animation_cover: Option<Ladder>,
    personalized_cover: bool,
    gif_document: Option<Document>,
    still_document: Option<Document>,
    raster_probe_document: Option<Document>,
    web_photo: Option<tl::enums::WebDocument>,
    kind: &'static str,
    nsfw: Option<Armed>,
    concepts: Option<super::concepts::Armed>,
    advert: Option<AdvertArmed>,
    verdict: Option<(Judgement, VisionSource)>,
    margins: Option<[f32; super::CONCEPT_SLOTS]>,
    reading: Option<super::AdvertVerdict>,
    filters: Arc<Vec<super::imgfilter::Filter>>,
    custom_margins: Option<Vec<f32>>,
    sender: Option<i64>,
    name: String,
}

fn advert_rank(decision: super::ocr::Decision) -> u8 {
    match decision {
        super::ocr::Decision::Allow => 0,
        super::ocr::Decision::Suspicious => 1,
        super::ocr::Decision::Block => 2,
    }
}

fn scan_advert_frames_with(
    frames: &[Arc<image::RgbImage>],
    mut read: impl FnMut(&image::RgbImage) -> Result<String, ModelError>,
) -> Result<(super::AdvertVerdict, usize, usize), ModelError> {
    let mut best = None;
    let mut text_bytes = 0;
    let mut first_error = None;
    for (at, frame) in frames.iter().enumerate() {
        let text = match read(frame) {
            Ok(text) => text,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        text_bytes += text.len();
        let classification = super::ocr::classify(&text);
        let rank = advert_rank(classification.decision);
        let verdict = super::AdvertVerdict::from_classification(&classification);
        if best.as_ref().is_none_or(|(current, _)| rank > *current) {
            best = Some((rank, verdict));
        }
        if classification.decision == super::ocr::Decision::Block {
            return Ok((verdict, text_bytes, at + 1));
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    let Some((_, verdict)) = best else {
        return Err(ModelError::runtime("no image frame available for OCR"));
    };
    Ok((verdict, text_bytes, frames.len()))
}

fn scan_advert_frames(
    frames: &[Arc<image::RgbImage>],
) -> Result<(super::AdvertVerdict, usize, usize), ModelError> {
    scan_advert_frames_with(frames, super::ocr::read)
}

#[derive(serde::Serialize)]
pub(crate) struct OfflineAnimatedAdvert {
    pub decision: &'static str,
    pub signal_names: String,
    pub sampled_frames: usize,
    pub ocr_frames_selected: usize,
    pub ocr_frames_read: usize,
    pub ocr_text_bytes: usize,
}

pub(crate) fn evaluate_animated_advert_offline(
    frames: &[image::RgbImage],
) -> Result<OfflineAnimatedAdvert, ModelError> {
    let indices = animated_ocr_indices_for_frames(frames);
    let selected = indices
        .iter()
        .map(|&at| Arc::new(frames[at].clone()))
        .collect::<Vec<_>>();
    let (verdict, ocr_text_bytes, ocr_frames_read) = scan_advert_frames(&selected)?;
    Ok(OfflineAnimatedAdvert {
        decision: verdict.decision_name(),
        signal_names: verdict.signal_names(),
        sampled_frames: frames.len(),
        ocr_frames_selected: selected.len(),
        ocr_frames_read,
        ocr_text_bytes,
    })
}

fn pin_still_ocr_source(
    image: &Arc<image::RgbImage>,
    needs_text: bool,
    has_animation_frames: bool,
) -> Option<Arc<image::RgbImage>> {
    (needs_text && !has_animation_frames).then(|| Arc::clone(image))
}

fn use_sharper_rung(escalate: bool, from_animation: bool, has_sharper: bool) -> bool {
    escalate && !from_animation && has_sharper
}

async fn decode_cached_rung(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    ladder: &Ladder,
    at: usize,
) -> Option<image::RgbImage> {
    let bytes = fetch(ctx, chat, id, ladder, at).await?;
    let decode_slot =
        match tokio::time::timeout(IMAGE_DECODE_DEADLINE, ctx.media_decode_slot()).await {
            Ok(slot) => slot,
            Err(_) => {
                log::warn!(
                    "image moderation: chat {chat} file {id} cached rung decode admission timed out"
                );
                return None;
            }
        };
    let worker = tokio::task::spawn_blocking(move || {
        let _decode_slot = decode_slot;
        decode_still(&bytes)
    });
    match tokio::time::timeout(IMAGE_DECODE_DEADLINE, worker).await {
        Ok(Ok(Ok(image))) => Some(image),
        Ok(Ok(Err(error))) => {
            log::warn!(
                "image moderation: chat {chat} file {id} cached rung decode failed: {error}"
            );
            None
        }
        Ok(Err(error)) => {
            log::warn!(
                "image moderation: chat {chat} file {id} cached rung worker failed: {error}"
            );
            None
        }
        Err(_) => {
            log::warn!("image moderation: chat {chat} file {id} cached rung decode timed out");
            None
        }
    }
}

async fn decode_animation_cover(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    ladder: &Ladder,
) -> Option<image::RgbImage> {
    let bytes = fetch(ctx, chat, id, ladder, ladder.start).await?;
    let decode_slot = match tokio::time::timeout(IMAGE_DECODE_DEADLINE, ctx.media_decode_slot())
        .await
    {
        Ok(slot) => slot,
        Err(_) => {
            log::warn!("image moderation: chat {chat} file {id} cover decode admission timed out");
            return None;
        }
    };
    let worker = tokio::task::spawn_blocking(move || {
        let _decode_slot = decode_slot;
        decode_still(&bytes)
    });
    match tokio::time::timeout(IMAGE_DECODE_DEADLINE, worker).await {
        Ok(Ok(Ok(image))) => Some(image),
        Ok(Ok(Err(error))) => {
            log::warn!("image moderation: chat {chat} file {id} cover decode failed: {error}");
            None
        }
        Ok(Err(error)) => {
            log::warn!("image moderation: chat {chat} file {id} cover worker failed: {error}");
            None
        }
        Err(_) => {
            log::warn!("image moderation: chat {chat} file {id} cover decode timed out");
            None
        }
    }
}

async fn advert_read(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    frames: Vec<Arc<image::RgbImage>>,
    source: &VisionSource,
    cacheable: bool,
) -> Result<super::AdvertVerdict, String> {
    let claim = if cacheable {
        ctx.claim_advert(id, source).await
    } else {
        super::AdvertClaim::Leader
    };
    match claim {
        super::AdvertClaim::Cached(verdict) => {
            log::info!(
                "advert: chat {chat} file {id} cache_hit verdict={} signals={}",
                verdict.decision_name(),
                verdict.signal_names()
            );
            Ok(verdict)
        }
        super::AdvertClaim::Leader => {
            let started = std::time::Instant::now();
            let (global_slot, chat_slot) =
                match tokio::time::timeout(OCR_DEADLINE, ctx.ocr_slots(chat)).await {
                    Ok(slots) => slots,
                    Err(_) => {
                        if cacheable {
                            ctx.release_advert(id, source);
                        }
                        return Err("OCR admission deadline exceeded".to_owned());
                    }
                };
            let admitted = started.elapsed();
            let ocr_started = std::time::Instant::now();
            let frame_count = frames.len();
            let worker = tokio::task::spawn_blocking(move || {
                let _global_slot = global_slot;
                let _chat_slot = chat_slot;
                scan_advert_frames(&frames)
            });
            let (verdict, text_bytes, frames_read) =
                match tokio::time::timeout(OCR_DEADLINE.saturating_mul(frame_count as u32), worker)
                    .await
                {
                    Ok(Ok(Ok(result))) => result,
                    Ok(Ok(Err(error))) => {
                        if cacheable {
                            ctx.release_advert(id, source);
                        }
                        return Err(error.to_string());
                    }
                    Ok(Err(error)) => {
                        if cacheable {
                            ctx.release_advert(id, source);
                        }
                        return Err(format!("OCR worker failed: {error}"));
                    }
                    Err(_) => {
                        if cacheable {
                            ctx.release_advert(id, source);
                        }
                        return Err("OCR deadline exceeded".to_owned());
                    }
                };
            let ocr_elapsed = ocr_started.elapsed();
            if cacheable {
                ctx.finish_advert(id, source, verdict);
            }
            log::info!(
                "advert: chat {chat} file {id} ocr_admission_ms={} ocr_ms={} ocr_total_ms={} frames={} text_bytes={} verdict={} signals={}",
                admitted.as_millis(),
                ocr_elapsed.as_millis(),
                started.elapsed().as_millis(),
                frames_read,
                text_bytes,
                verdict.decision_name(),
                verdict.signal_names()
            );
            Ok(verdict)
        }
    }
}

fn content_animation(bytes: &[u8]) -> Option<bool> {
    match image::guess_format(bytes).ok() {
        Some(image::ImageFormat::Jpeg) => Some(false),
        Some(
            image::ImageFormat::Gif
            | image::ImageFormat::Tiff
            | image::ImageFormat::Ico
            | image::ImageFormat::Pnm,
        ) => Some(true),
        Some(image::ImageFormat::Png | image::ImageFormat::WebP) => raster_kind_full(bytes),
        Some(
            image::ImageFormat::Bmp
            | image::ImageFormat::Hdr
            | image::ImageFormat::Qoi
            | image::ImageFormat::Farbfeld,
        ) => Some(false),
        _ if bytes.get(4..8) == Some(b"ftyp") || bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) => {
            Some(true)
        }
        _ => None,
    }
}

enum OriginalAnimatedBytes {
    Still(Vec<u8>),
    AnimatedOrUnknown(Vec<u8>),
    OversizedStill,
}

fn route_original_animated_bytes(bytes: Vec<u8>) -> OriginalAnimatedBytes {
    match content_animation(&bytes) {
        Some(false) if bytes.len() <= THUMBNAIL_BYTES_MAX => OriginalAnimatedBytes::Still(bytes),
        Some(false) => OriginalAnimatedBytes::OversizedStill,
        _ => OriginalAnimatedBytes::AnimatedOrUnknown(bytes),
    }
}

async fn fetch_invoice_photo(
    ctx: &Arc<Ctx>,
    photo: &tl::enums::WebDocument,
) -> Result<Vec<u8>, &'static str> {
    let _fetching = ctx.nsfw_fetch().await;
    if let Some(direct) = direct_invoice_web_photo(photo) {
        return invoice_web::fetch_no_proxy(&direct.url, direct.size).await;
    }
    let user = ctx
        .user_client()
        .ok_or("invoice user session unavailable")?;
    tokio::time::timeout(MEDIA_FETCH_DEADLINE, async {
        if !user
            .is_authorized()
            .await
            .map_err(|_| "invoice user authorization check failed")?
        {
            return Err("invoice user session is not authorized");
        }
        user.download_web_document(photo, THUMBNAIL_BYTES_MAX, MEDIA_FETCH_DEADLINE)
            .await
            .map_err(|_| "invoice proxied webfile fetch failed")
    })
    .await
    .map_err(|_| "invoice proxied webfile deadline exceeded")?
}

async fn classify(classification: Classification) {
    let Classification {
        ctx,
        chat,
        chat_ref,
        message_id,
        media_id: id,
        ladder,
        animation_cover,
        personalized_cover,
        gif_document,
        still_document,
        raster_probe_document,
        web_photo,
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
    } = classification;
    let buffers = ctx.media_buffer_slot().await;
    let probe = if let Some(document) = raster_probe_document.as_ref() {
        probe_raster_document(&ctx, chat, id, document).await
    } else {
        RasterProbe {
            kind: RasterKind::Unknown,
            full_bytes: None,
        }
    };
    if raster_probe_document.is_some() {
        match probe.kind {
            RasterKind::Static => ctx.remember_raster_animation(id, false),
            RasterKind::Animated => ctx.remember_raster_animation(id, true),
            RasterKind::Unknown => log::warn!(
                "image moderation: chat {chat} file {id} PNG/WebP animation kind unresolved; result will not be cached"
            ),
        }
    }
    let raster_unknown = raster_probe_document.is_some() && probe.kind == RasterKind::Unknown;
    let animation_suspect =
        probe.kind == RasterKind::Animated || (raster_unknown && probe.full_bytes.is_some());
    let mut gif_document = if animation_suspect {
        raster_probe_document.clone().or(gif_document)
    } else {
        gif_document
    };
    let mut still_document = if probe.kind == RasterKind::Animated {
        None
    } else {
        still_document
    };
    let mut probed_full_bytes = probe.full_bytes;
    if ladder.is_some() && gif_document.is_none() {
        probed_full_bytes = None;
    }
    if ladder.is_none()
        && raster_probe_document.is_none()
        && let Some(document) = still_document.as_ref()
    {
        let Some(bytes) = fetch_document(&ctx, chat, id, document, THUMBNAIL_BYTES_MAX).await
        else {
            log::warn!(
                "image moderation: chat {chat} file {id} original image unscanned: fetch failed or exceeded byte limit"
            );
            return;
        };
        match content_animation(&bytes) {
            Some(true) => {
                ctx.remember_raster_animation(id, true);
                gif_document = Some(document.clone());
                still_document = None;
            }
            Some(false) => ctx.remember_raster_animation(id, false),
            None => {
                log::warn!(
                    "image moderation: chat {chat} file {id} original image unscanned: unsupported or incomplete image format"
                );
                return;
            }
        }
        probed_full_bytes = Some(bytes);
    }
    let needs_nsfw = nsfw.is_some() && verdict.is_none();
    let needs_concepts = concepts.is_some() && margins.is_none();
    let needs_custom = !filters.is_empty() && custom_margins.is_none();
    let needs_general = needs_concepts || needs_custom;
    let needs_text = advert.is_some() && reading.is_none();
    let filter_specs = filters
        .iter()
        .map(super::imgfilter::spec)
        .collect::<Vec<_>>();
    let mut margins = margins;
    let mut custom_margins = custom_margins;

    let mut metadata_static_original = false;
    if ladder.is_none()
        && raster_probe_document.is_none()
        && let Some(document) = gif_document.clone()
        && !is_tgs_document(&document)
    {
        let Some(bytes) = fetch_document(&ctx, chat, id, &document, ANIMATED_BYTES_MAX).await
        else {
            return;
        };
        match route_original_animated_bytes(bytes) {
            OriginalAnimatedBytes::Still(bytes) => {
                metadata_static_original = true;
                still_document = Some(document);
                gif_document = None;
                probed_full_bytes = Some(bytes);
            }
            OriginalAnimatedBytes::AnimatedOrUnknown(bytes) => {
                probed_full_bytes = Some(bytes);
            }
            OriginalAnimatedBytes::OversizedStill => {
                log::warn!(
                    "image moderation: chat {chat} file {id} static original exceeds the byte limit"
                );
                return;
            }
        }
    }

    let mut web_still_bytes = None;
    let mut web_animation = None;
    if let Some(photo) = web_photo.as_ref() {
        let fetched = fetch_invoice_photo(&ctx, photo).await;
        let bytes = match fetched {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!(
                    "image moderation: chat {chat} msg {message_id} invoice photo unscanned: {error}"
                );
                return;
            }
        };
        match content_animation(&bytes) {
            Some(true) => {
                let size = bytes.len();
                let Some(frames) = decode_animated_frames(bytes).await else {
                    log::warn!(
                        "image moderation: chat {chat} msg {message_id} animated invoice photo unscanned: decode failed"
                    );
                    return;
                };
                if frames.is_empty() {
                    log::warn!(
                        "image moderation: chat {chat} msg {message_id} animated invoice photo unscanned: no frames"
                    );
                    return;
                }
                web_animation = Some((frames, size));
            }
            Some(false) => web_still_bytes = Some(bytes),
            None => {
                log::warn!(
                    "image moderation: chat {chat} msg {message_id} invoice photo unscanned: unsupported or incomplete image format"
                );
                return;
            }
        }
    }
    let animated = gif_document.is_some() || web_animation.is_some();

    let full_animation = if let Some((frames, size)) = web_animation {
        Some((frames, size, false))
    } else if animation_required(needs_nsfw, needs_general, needs_text) {
        if let Some(document) = gif_document.as_ref() {
            let tgs = is_tgs_document(document);
            let max_bytes = if tgs {
                tgs::MAX_COMPRESSED_BYTES
            } else {
                ANIMATED_BYTES_MAX
            };
            let bytes = if tgs {
                fetch_document(&ctx, chat, id, document, max_bytes).await
            } else if let Some(bytes) = probed_full_bytes.take() {
                Some(bytes)
            } else {
                fetch_document(&ctx, chat, id, document, max_bytes).await
            };
            match bytes {
                Some(bytes) => {
                    let size = bytes.len();
                    let decoded = if tgs {
                        decode_tgs_frames(&ctx, bytes).await
                    } else {
                        decode_animated_frames(bytes).await
                    };
                    match decoded {
                        Some(frames) => Some((frames, size, tgs)),
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

    let cover_expected =
        full_animation.is_some() && (personalized_cover || animation_cover.is_some());
    let animation_cover_image = if full_animation.is_some() {
        match animation_cover.as_ref() {
            Some(ladder) => decode_animation_cover(&ctx, chat, id, ladder).await,
            None => None,
        }
    } else {
        None
    };
    let cover_incomplete = cover_expected && animation_cover_image.is_none();
    let cacheable_media = !personalized_cover
        && web_photo.is_none()
        && !cover_incomplete
        && !raster_unknown
        && !metadata_static_original
        && (gif_document.is_none() || full_animation.is_some());

    let thinking = std::time::Instant::now();
    let mut prefetched_still = if full_animation.is_none() {
        let fetched = fetch_still(
            &ctx,
            (chat, id, kind),
            &mut web_still_bytes,
            ladder.as_ref(),
            still_document.as_ref(),
            &mut probed_full_bytes,
        )
        .await;
        if fetched.is_none() {
            return;
        }
        fetched
    } else {
        None
    };
    let visual_slot = if needs_nsfw || needs_general {
        Some(ctx.nsfw_slot().await)
    } else {
        None
    };

    let had_full_animation = full_animation.is_some();
    let mut ocr_frames: Option<(Vec<image::RgbImage>, usize)> = None;
    let mut nsfw_failed = false;
    let mut animated_nsfw_cacheable = false;
    let mut general_failed = false;
    let mut concepts_failed = false;
    let selected_animation = if let Some((mut frames, size, drawn)) = full_animation {
        if needs_text {
            let mut sampled: Vec<image::RgbImage> = animated_ocr_indices_for_frames(&frames)
                .into_iter()
                .map(|at| frames[at].clone())
                .collect();
            if let Some(cover) = animation_cover_image.as_ref() {
                add_distinct_cover(&mut sampled, cover);
            }
            ocr_frames = Some((sampled, size));
        }
        if needs_nsfw || needs_general {
            if let Some(cover) = animation_cover_image.as_ref() {
                add_distinct_cover(&mut frames, cover);
            }
            let fallback_frame = frames.first().cloned();
            let selection = tokio::task::spawn_blocking(move || {
                score_drawn_or_filmed_frames(
                    frames,
                    needs_nsfw,
                    needs_general,
                    needs_concepts,
                    filter_specs,
                    drawn,
                )
            })
            .await;
            let selection = match selection {
                Ok(Ok(Some(selection))) => Some(selection),
                Ok(Ok(None)) => fallback_frame.map(|image| AnimatedSelection {
                    image,
                    look: None,
                    judgement: None,
                    margins: None,
                    custom_margins: None,
                    nsfw_failed: needs_nsfw,
                    nsfw_cacheable: false,
                    general_failed: needs_general,
                    concepts_failed: needs_concepts,
                }),
                Ok(Err(error)) => {
                    log::error!(
                        "nsfw: {chat} file {id} animated model failure: {error}; result not cached"
                    );
                    fallback_frame.map(|image| AnimatedSelection {
                        image,
                        look: None,
                        judgement: None,
                        margins: None,
                        custom_margins: None,
                        nsfw_failed: needs_nsfw,
                        nsfw_cacheable: false,
                        general_failed: needs_general,
                        concepts_failed: needs_concepts,
                    })
                }
                Err(error) => {
                    log::error!(
                        "nsfw: {chat} file {id} animated worker failure: {error}; result not cached"
                    );
                    fallback_frame.map(|image| AnimatedSelection {
                        image,
                        look: None,
                        judgement: None,
                        margins: None,
                        custom_margins: None,
                        nsfw_failed: needs_nsfw,
                        nsfw_cacheable: false,
                        general_failed: needs_general,
                        concepts_failed: needs_concepts,
                    })
                }
            };
            match selection {
                Some(selection) => {
                    nsfw_failed = selection.nsfw_failed;
                    animated_nsfw_cacheable = selection.nsfw_cacheable;
                    general_failed = selection.general_failed;
                    concepts_failed = selection.concepts_failed;
                    if nsfw_failed {
                        log::error!(
                            "nsfw: {chat} file {id} animated NSFW coverage incomplete; result not cached"
                        );
                    }
                    if general_failed {
                        log::error!(
                            "vision: {chat} file {id} animated general-model coverage incomplete; result not cached"
                        );
                    }
                    if concepts_failed {
                        log::error!(
                            "vision: {chat} file {id} animated concept coverage incomplete; result not cached"
                        );
                    }
                    if let Some(all) = selection.margins {
                        if cacheable_media && !concepts_failed {
                            ctx.remember_margins(id, all, VisionSource::Animated);
                        }
                        margins = Some(all);
                    }
                    if let Some(mut all) = selection.custom_margins {
                        let unasked = if super::imgfilter::wants_second_look(&filters, &all) {
                            let frame = Arc::new(selection.image.clone());
                            super::imgfilter::second_look(&frame, (chat, id), &filters, &mut all)
                                .await
                        } else {
                            Vec::new()
                        };
                        if cacheable_media {
                            super::imgfilter::remember_all(
                                &ctx,
                                id,
                                &filters,
                                &all,
                                &unasked,
                                &VisionSource::Animated,
                            );
                        }
                        custom_margins = Some(all);
                    }
                    Some((selection.image, selection.look, selection.judgement, size))
                }
                None => None,
            }
        } else {
            None
        }
    } else {
        None
    };
    if had_full_animation && selected_animation.is_none() && !needs_text {
        log::warn!(
            "nsfw: {chat} file {id} animated {kind} decoded but no frame produced a model score; falling back to thumbnail"
        );
    }

    let from_animation = selected_animation.is_some();
    let (image, initial_look, animated_judgement, size, pixels) = if let Some((
        image,
        look,
        judged,
        size,
    )) = selected_animation
    {
        let pixels = format!("{}x{}", image.width(), image.height());
        (image, look, judged, size, pixels)
    } else if let Some((frames, size)) = ocr_frames.as_ref() {
        let Some(image) = frames.first().cloned() else {
            return;
        };
        let pixels = format!("{}x{}", image.width(), image.height());
        (image, None, None, *size, pixels)
    } else {
        let fetched = match prefetched_still.take() {
            Some(fetched) => Some(fetched),
            None => {
                fetch_still(
                    &ctx,
                    (chat, id, kind),
                    &mut web_still_bytes,
                    ladder.as_ref(),
                    still_document.as_ref(),
                    &mut probed_full_bytes,
                )
                .await
            }
        };
        let Some((bytes, pixels)) = fetched else {
            return;
        };
        let size = bytes.len();

        let decode_slot =
            match tokio::time::timeout(IMAGE_DECODE_DEADLINE, ctx.media_decode_slot()).await {
                Ok(slot) => slot,
                Err(_) => {
                    log::warn!("nsfw: {chat} file {id} decode admission deadline exceeded");
                    return;
                }
            };
        let mut decoded = tokio::task::spawn_blocking(move || {
            let _decode_slot = decode_slot;
            decode_still(&bytes)
        });
        let image = match tokio::time::timeout(IMAGE_DECODE_DEADLINE, &mut decoded).await {
            Ok(Ok(Ok(image))) => image,
            Ok(Ok(Err(e))) => {
                log::warn!("nsfw: {chat} file {id} {pixels} could not be decoded: {e}");
                return;
            }
            Ok(Err(e)) => {
                log::warn!("nsfw: {chat} file {id} decode task failed: {e}");
                return;
            }
            Err(_) => {
                log::warn!("nsfw: {chat} file {id} {pixels} decode deadline exceeded");
                return;
            }
        };
        (image, None, None, size, pixels)
    };

    let mut image = Arc::new(image);
    drop(probed_full_bytes);
    drop(web_still_bytes);
    drop(animation_cover_image);
    let ocr_still_image = pin_still_ocr_source(&image, needs_text, ocr_frames.is_some());
    let mut initial_look = initial_look;
    let mut selected_source = if from_animation || ocr_frames.is_some() {
        VisionSource::Animated
    } else if let Some(ladder) = ladder.as_ref() {
        ladder.source(ladder.start)
    } else {
        VisionSource::Original
    };
    let mut general_source_ready = true;
    if needs_general
        && let Some((_, cached_source)) = verdict.as_ref()
        && *cached_source != selected_source
    {
        let selected = ladder
            .as_ref()
            .and_then(|ladder| ladder.source_index(cached_source).map(|at| (ladder, at)));
        if let Some((ladder, at)) = selected {
            if let Some(frame) = decode_cached_rung(&ctx, chat, id, ladder, at).await {
                image = Arc::new(frame);
                selected_source = cached_source.clone();
            } else {
                general_source_ready = false;
                log::warn!(
                    "image moderation: chat {chat} file {id} cached NSFW source unavailable; pending general scores suppressed"
                );
            }
        } else {
            general_source_ready = false;
            log::warn!(
                "image moderation: chat {chat} file {id} cached NSFW rung absent; pending general scores suppressed"
            );
        }
    }

    let wants_embedding = general_source_ready
        && ((concepts.is_some() && margins.is_none() && !concepts_failed)
            || (!filters.is_empty() && custom_margins.is_none()));
    let mut embedding: Option<Vec<f32>> = None;
    let mut embedded = false;

    if let Some(armed) = nsfw.filter(|_| !nsfw_failed) {
        let judged = match verdict {
            Some((judged, _)) => Some(judged),
            None if animated_judgement.is_some() => {
                let (judged, note) = animated_judgement.as_ref().unwrap();
                if cacheable_media && animated_nsfw_cacheable {
                    ctx.remember_verdict(
                        id,
                        VisionRoute::Animated,
                        *judged,
                        VisionSource::Animated,
                    );
                }
                log::info!(
                    "nsfw: {chat} file {id} {kind} {pixels} bytes {size} think {}ms {} score {:.4}",
                    thinking.elapsed().as_millis(),
                    note,
                    judged.score,
                );
                Some(*judged)
            }
            None => 'nsfw_analysis: {
                let mut look = match initial_look.take() {
                    Some(look) => look,
                    None => {
                        let frame = Arc::clone(&image);
                        match tokio::task::spawn_blocking(move || look(&frame)).await {
                            Ok(Ok(Some(look))) => look,
                            Ok(Ok(None)) => break 'nsfw_analysis None,
                            Ok(Err(error)) => {
                                log::error!(
                                    "nsfw: {chat} file {id} classifier failed: {error}; result not cached"
                                );
                                break 'nsfw_analysis None;
                            }
                            Err(error) => {
                                log::error!(
                                    "nsfw: {chat} file {id} classifier worker failed: {error}; result not cached"
                                );
                                break 'nsfw_analysis None;
                            }
                        }
                    }
                };
                if !look.complete {
                    log::error!(
                        "nsfw: {chat} file {id} mirrored verification incomplete; suppressing moderation"
                    );
                    break 'nsfw_analysis None;
                }

                let escalate = look.score >= GRADE_ABOVE && (undecided(look.score) || look.frail);
                let sharper = ladder.as_ref().and_then(|ladder| {
                    use_sharper_rung(
                        escalate,
                        from_animation,
                        ladder.sizes.len() > ladder.start + 1,
                    )
                    .then_some(ladder)
                });
                if let Some(ladder) = sharper {
                    let Some(sharper) = fetch(&ctx, chat, id, ladder, ladder.start + 1).await
                    else {
                        log::error!(
                            "nsfw: {chat} file {id} required sharper rung unavailable; suppressing moderation"
                        );
                        break 'nsfw_analysis None;
                    };
                    match tokio::time::timeout(IMAGE_DECODE_DEADLINE, ctx.media_decode_slot()).await
                    {
                        Ok(decode_slot) => {
                            let mut better = tokio::task::spawn_blocking(move || {
                                let _decode_slot = decode_slot;
                                let frame =
                                    decode_still(&sharper).map_err(|error| error.to_string())?;
                                self::look(&frame)
                                    .map_err(|error| error.to_string())
                                    .map(|look| look.map(|look| (look, frame)))
                            });
                            match tokio::time::timeout(IMAGE_DECODE_DEADLINE, &mut better).await {
                                Ok(Ok(Ok(Some((better, frame))))) => {
                                    if !better.complete {
                                        log::error!(
                                            "nsfw: {chat} file {id} sharper mirrored verification incomplete; suppressing moderation"
                                        );
                                        break 'nsfw_analysis None;
                                    }
                                    look = Look {
                                        note: format!("{} escalated", better.note),
                                        ..better
                                    };
                                    image = Arc::new(frame);
                                    selected_source = ladder.source(ladder.start + 1);
                                }
                                Ok(Ok(Ok(None))) => {
                                    log::error!(
                                        "nsfw: {chat} file {id} sharper classifier unavailable; suppressing moderation"
                                    );
                                    break 'nsfw_analysis None;
                                }
                                Ok(Ok(Err(error))) => {
                                    log::error!(
                                        "nsfw: {chat} file {id} sharper rung failed: {error}; suppressing moderation"
                                    );
                                    break 'nsfw_analysis None;
                                }
                                Ok(Err(error)) => {
                                    log::error!(
                                        "nsfw: {chat} file {id} sharper worker failed: {error}; suppressing moderation"
                                    );
                                    break 'nsfw_analysis None;
                                }
                                Err(_) => {
                                    log::error!(
                                        "nsfw: {chat} file {id} sharper decode deadline exceeded; suppressing moderation"
                                    );
                                    break 'nsfw_analysis None;
                                }
                            }
                        }
                        Err(_) => {
                            log::error!(
                                "nsfw: {chat} file {id} sharper decode admission deadline exceeded; suppressing moderation"
                            );
                            break 'nsfw_analysis None;
                        }
                    }
                }

                let mut arbiter = if look.peak >= GRADE_ABOVE {
                    if !embedded {
                        match embed(&ctx, &image, wants_embedding).await {
                            Ok(found) => embedding = found,
                            Err(error) => {
                                log::error!(
                                    "nsfw: {chat} file {id} arbiter failed: {error}; suppressing moderation"
                                );
                                embedded = true;
                                break 'nsfw_analysis None;
                            }
                        }
                        embedded = true;
                    }
                    embedding.as_deref().map(arbiter_of)
                } else {
                    None
                };

                let grade = if look.score >= GRADE_ABOVE {
                    let frame = Arc::clone(&image);
                    match tokio::task::spawn_blocking(move || grade(&frame)).await {
                        Ok(Ok(Some(grade))) => {
                            log::info!("nsfw: {chat} file {id} grade [{}]", grade.classes);
                            Some(grade)
                        }
                        Ok(Ok(None)) => {
                            log::error!(
                                "nsfw: {chat} file {id} grader unavailable; suppressing moderation"
                            );
                            break 'nsfw_analysis None;
                        }
                        Ok(Err(error)) => {
                            log::error!(
                                "nsfw: {chat} file {id} grader failed: {error}; suppressing moderation"
                            );
                            break 'nsfw_analysis None;
                        }
                        Err(error) => {
                            log::error!(
                                "nsfw: {chat} file {id} grader worker failed: {error}; suppressing moderation"
                            );
                            break 'nsfw_analysis None;
                        }
                    }
                } else {
                    None
                };
                let mut judged = match assemble_judgement(&look, grade.as_ref(), arbiter) {
                    Ok(judged) => judged,
                    Err(error) => {
                        log::error!(
                            "nsfw: {chat} file {id} verification incomplete: {error}; suppressing moderation"
                        );
                        break 'nsfw_analysis None;
                    }
                };
                if !animated && let Some((at, score)) = still_crop_candidate(&look, judged) {
                    let frame = Arc::clone(&image);
                    let probe = tokio::task::spawn_blocking(move || {
                        probe_still_crop_with(&frame, at, score, self::grade, |crop| {
                            arbiter_from_embedding(super::vision::embed_of(crop))
                        })
                    })
                    .await;
                    let probe = match probe {
                        Ok(Ok(probe)) => probe,
                        Ok(Err(error)) => {
                            log::error!(
                                "nsfw: {chat} file {id} crop verification failed: {error}; suppressing moderation"
                            );
                            break 'nsfw_analysis None;
                        }
                        Err(error) => {
                            log::error!(
                                "nsfw: {chat} file {id} crop worker failed: {error}; suppressing moderation"
                            );
                            break 'nsfw_analysis None;
                        }
                    };
                    log::info!(
                        "nsfw: {chat} file {id} crop view {at} grade hard {:.3} arbiter {} rescued {}",
                        probe.grade.hard,
                        probe.arbiter.map_or_else(
                            || "not needed".to_owned(),
                            |value| format!("{:+.4} head {:.3}", value.explicit, value.head)
                        ),
                        probe.judged.is_some(),
                    );
                    if let Some(rescued) = probe.judged {
                        judged = rescued;
                        arbiter = probe.arbiter;
                    }
                }
                if cacheable_media
                    && (look.peak < GRADE_ABOVE || arbiter.is_some())
                    && let Some(route) =
                        verdict_route(from_animation, ladder.as_ref(), still_document.is_some())
                {
                    ctx.remember_verdict(id, route, judged, selected_source.clone());
                }
                log::info!(
                    "nsfw: {chat} file {id} {kind} {pixels} bytes {size} think {}ms {}{}{}",
                    thinking.elapsed().as_millis(),
                    look.note,
                    if judged.score > look.score {
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
                Some(judged)
            }
        };
        if let Some(judged) = judged {
            act_detached(
                &ctx,
                DetachedNsfw {
                    chat,
                    chat_ref,
                    message_id,
                    media_id: id,
                    judged,
                    armed: &armed,
                    sender,
                    name: name.clone(),
                },
            )
            .await;
        }
    }

    if wants_embedding && !embedded && !general_failed {
        match embed(&ctx, &image, true).await {
            Ok(found) => embedding = found,
            Err(error) => {
                log::error!(
                    "nsfw: {chat} file {id} vision embedding failed: {error}; result not cached"
                );
            }
        }
    }

    let prints = if !filters.is_empty()
        && !general_failed
        && custom_margins.is_none()
        && filters.iter().any(|filter| filter.is_face())
    {
        let frame = Arc::clone(&image);
        match tokio::task::spawn_blocking(move || super::facelock::prints(&frame)).await {
            Ok(Ok(prints)) => prints,
            Ok(Err(error)) => {
                log::error!("face lock: {chat} file {id} faces failed: {error}");
                None
            }
            Err(error) => {
                log::error!("face lock: {chat} file {id} face worker failed: {error}");
                None
            }
        }
    } else {
        None
    };
    drop(visual_slot);

    if let Some(armed) = concepts.filter(|_| !general_failed) {
        let all = match margins {
            Some(all) => Some(all),
            None => {
                if let Some(embedding) = embedding.as_ref() {
                    let whole = super::concepts::margins_from(embedding);
                    if cacheable_media {
                        ctx.remember_margins(id, whole, selected_source.clone());
                    }
                    Some(whole)
                } else {
                    None
                }
            }
        };
        if let Some(all) = all {
            let fresh = super::concepts::second_look(
                &ctx,
                &image,
                (chat, id),
                &selected_source,
                cacheable_media,
                &all,
                &armed,
            )
            .await;
            let judged = super::concepts::with_verdicts(
                &ctx,
                id,
                Some(&selected_source),
                &all,
                &armed,
                &fresh,
            );
            super::concepts::act_detached(
                &ctx,
                super::concepts::Detached {
                    chat,
                    chat_ref,
                    message_id,
                    media_id: id,
                    margins: &judged,
                    armed: &armed,
                    sender,
                    name: &name,
                },
            )
            .await;
        }
    }

    if !filters.is_empty() && !general_failed {
        let scored = match (custom_margins, embedding.as_ref()) {
            (Some(margins), _) => Some(margins),
            (None, Some(embedding)) => {
                let mut margins =
                    super::imgfilter::score_all(&filters, embedding, prints.as_deref());
                let unasked =
                    super::imgfilter::second_look(&image, (chat, id), &filters, &mut margins)
                        .await;
                if cacheable_media {
                    super::imgfilter::remember_all(
                        &ctx,
                        id,
                        &filters,
                        &margins,
                        &unasked,
                        &selected_source,
                    );
                }
                Some(margins)
            }
            (None, None) => None,
        };
        if let Some(scored) = scored {
            super::imgfilter::act_detached(
                &ctx,
                super::imgfilter::Detached {
                    chat,
                    chat_ref,
                    message_id,
                    media_id: id,
                    filters: &filters,
                    margins: &scored,
                    sender,
                    name: &name,
                },
            )
            .await;
        }
    }

    drop(image);

    if let Some(armed) = advert {
        let verdict = match reading {
            Some(verdict) => verdict,
            None => {
                let (frames, ocr_from_animation) = match ocr_frames.take() {
                    Some((frames, _)) => (frames.into_iter().map(Arc::new).collect(), true),
                    None => (
                        vec![ocr_still_image.expect("pending OCR pins its still image")],
                        false,
                    ),
                };
                let ocr_source = if ocr_from_animation {
                    VisionSource::Animated
                } else if let Some(ladder) = ladder.as_ref() {
                    ladder.source(ladder.start)
                } else {
                    VisionSource::Original
                };
                match advert_read(&ctx, chat, id, frames, &ocr_source, cacheable_media).await {
                    Ok(verdict) => {
                        log::info!(
                            "advert[{}]: chat {chat} file {id} verdict {} signals={}",
                            if armed.live { "live" } else { "shadow" },
                            verdict.decision_name(),
                            verdict.signal_names()
                        );
                        verdict
                    }
                    Err(error) => {
                        log::error!(
                            "advert: OCR failed for chat {chat} file {id}: {error}; result not cached"
                        );
                        return;
                    }
                }
            }
        };
        drop(buffers);
        let Some(why) = verdict.why() else {
            return;
        };
        if !armed.live {
            return;
        }
        match ctx
            .client
            .delete_messages_critical(chat_ref, &[message_id])
            .await
        {
            Ok(0) => {
                eprintln!("advert: delete returned zero events in {chat} msg {message_id}");
                return;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("advert: could not delete in {chat} msg {message_id}: {e}");
                return;
            }
        }
        if !ctx.claim_moderation(chat, message_id) {
            return;
        }
        ctx.bump(chat, super::stats::DELETED);
        punish_and_notify(
            &ctx,
            DetachedModeration {
                chat,
                chat_ref,
                message_id,
                sender,
                name: &name,
                cause: super::ocr::LOCK,
                reason: why,
            },
        )
        .await;
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
    match message.delete_critical_counted().await {
        Ok(0) => return,
        Ok(_) => {}
        Err(e) => {
            eprintln!("advert: could not delete in {chat}: {e}");
            return;
        }
    }
    if !ctx.claim_moderation(chat, message.id()) {
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    let chances = match super::strict::punish(ctx, message, chat, super::ocr::LOCK).await {
        super::strict::Outcome::Announced => {
            let action = super::cases::action_key(super::strict::action_of(ctx, chat));
            super::cases::record_delete(ctx, message, super::ocr::LOCK, why, action).await;
            return;
        }
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::cases::record_delete(ctx, message, super::ocr::LOCK, why, "delete").await;
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
    let expected = Downloadable::size(&thumb).unwrap_or(0);
    if expected > THUMBNAIL_BYTES_MAX {
        log::warn!("nsfw: chat {chat} file {id} thumbnail exceeds the byte limit");
        return None;
    }
    let mut bytes = Vec::with_capacity(expected);
    let mut download = ctx.client.iter_download(&thumb);
    let fetched = tokio::time::timeout(MEDIA_FETCH_DEADLINE, async {
        loop {
            match download.next().await {
                Ok(Some(chunk)) => {
                    if bytes.len().saturating_add(chunk.len()) > THUMBNAIL_BYTES_MAX {
                        return Err(
                            "thumbnail exceeded the byte limit while downloading".to_owned()
                        );
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => return Ok(()),
                Err(error) => return Err(error.to_string()),
            }
        }
    })
    .await;
    match fetched {
        Ok(Ok(())) if expected != 0 && bytes.len() != expected => {
            log::warn!(
                "nsfw: chat {chat} file {id} document ended at {} of {expected} declared bytes",
                bytes.len()
            );
            None
        }
        Ok(Ok(())) => (!bytes.is_empty()).then_some(bytes),
        Ok(Err(error)) => {
            log::warn!("nsfw: could not fetch a thumbnail in {chat} for {id}: {error}");
            None
        }
        Err(_) => {
            log::warn!("nsfw: thumbnail fetch deadline exceeded in {chat} for {id}");
            None
        }
    }
}

fn retry_document_fetch(attempt: usize, error: &InvocationError) -> bool {
    attempt == 0 && matches!(error, InvocationError::Rpc(rpc) if rpc.code == -503)
}

enum DocumentFetchError {
    TooLarge,
    Telegram(InvocationError),
}

async fn fetch_document(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    document: &Document,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    let _fetching = ctx.nsfw_fetch().await;
    let expected = Downloadable::size(document).unwrap_or(0);
    if expected > max_bytes {
        log::warn!("nsfw: chat {chat} file {id} document exceeds the byte limit");
        return None;
    }
    for attempt in 0..2 {
        let mut bytes = Vec::with_capacity(expected);
        let mut download = ctx.client.iter_download(document);
        let fetched = tokio::time::timeout(MEDIA_FETCH_DEADLINE, async {
            loop {
                match download.next().await {
                    Ok(Some(chunk)) => {
                        if bytes.len().saturating_add(chunk.len()) > max_bytes {
                            return Err(DocumentFetchError::TooLarge);
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    Ok(None) => return Ok(()),
                    Err(error) => return Err(DocumentFetchError::Telegram(error)),
                }
            }
        })
        .await;
        match fetched {
            Ok(Ok(())) if expected != 0 && bytes.len() != expected => {
                log::warn!(
                    "nsfw: chat {chat} file {id} document ended at {} of {expected} declared bytes",
                    bytes.len()
                );
                return None;
            }
            Ok(Ok(())) => return (!bytes.is_empty()).then_some(bytes),
            Ok(Err(DocumentFetchError::Telegram(error)))
                if retry_document_fetch(attempt, &error) =>
            {
                log::warn!(
                    "nsfw: transient document fetch failed in {chat} for {id}: {error}; retrying once"
                );
            }
            Ok(Err(DocumentFetchError::Telegram(error))) => {
                log::warn!("nsfw: could not fetch document in {chat} for {id}: {error}");
                return None;
            }
            Ok(Err(DocumentFetchError::TooLarge)) => {
                log::warn!(
                    "nsfw: could not fetch document in {chat} for {id}: document exceeded the byte limit while downloading"
                );
                return None;
            }
            Err(_) => {
                log::warn!("nsfw: document fetch deadline exceeded in {chat} for {id}");
                return None;
            }
        }
    }
    None
}

pub(super) async fn fetch_still_example(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    document: &Document,
) -> Option<Vec<u8>> {
    fetch_document(ctx, chat, id, document, THUMBNAIL_BYTES_MAX).await
}

struct RasterProbe {
    kind: RasterKind,
    full_bytes: Option<Vec<u8>>,
}

fn probed_document_kind(prefix: &[u8], declared_size: usize) -> RasterKind {
    if declared_size == 0 || prefix.len() > declared_size {
        return RasterKind::Unknown;
    }
    match image::guess_format(prefix).ok() {
        Some(image::ImageFormat::Jpeg) => RasterKind::Static,
        Some(image::ImageFormat::Gif) => RasterKind::Animated,
        Some(image::ImageFormat::Png | image::ImageFormat::WebP) => {
            raster_kind(prefix, declared_size)
        }
        _ if prefix.get(4..8) == Some(b"ftyp") || prefix.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) => {
            RasterKind::Animated
        }
        _ => RasterKind::Unknown,
    }
}

async fn probe_raster_document(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    document: &Document,
) -> RasterProbe {
    let declared = document.size().filter(|size| *size > 0);
    if let Some(size) = declared {
        let prefix = {
            let _fetching = ctx.nsfw_fetch().await;
            let mut download = ctx.client.iter_download(document).chunk_size(4 * 1024);
            tokio::time::timeout(MEDIA_FETCH_DEADLINE, async {
                let mut prefix = download.next().await.ok()??;
                if prefix.is_empty() || prefix.len() > 4 * 1024 {
                    return None;
                }
                if prefix.len() == 4 * 1024
                    && prefix.len() < size
                    && probed_document_kind(&prefix, size) == RasterKind::Unknown
                {
                    download = download.chunk_size(64 * 1024);
                    let extra = download.next().await.ok()??;
                    if extra.is_empty() || extra.len() > 64 * 1024 {
                        return None;
                    }
                    let short = extra.len() < 64 * 1024;
                    prefix.extend(extra);
                    if short && prefix.len() != size {
                        return None;
                    }
                }
                Some(prefix)
            })
            .await
            .ok()
            .flatten()
        };
        if let Some(prefix) = prefix
            && (prefix.len() >= size || prefix.len() >= 4 * 1024)
        {
            let kind = probed_document_kind(&prefix, size);
            if kind != RasterKind::Unknown {
                return RasterProbe {
                    kind,
                    full_bytes: (prefix.len() == size).then_some(prefix),
                };
            }
        }
    }

    let Some(bytes) = fetch_document(ctx, chat, id, document, ANIMATED_BYTES_MAX).await else {
        return RasterProbe {
            kind: RasterKind::Unknown,
            full_bytes: None,
        };
    };
    let complete = declared.is_none_or(|size| size == bytes.len());
    let decode_slot =
        match tokio::time::timeout(IMAGE_DECODE_DEADLINE, ctx.media_decode_slot()).await {
            Ok(slot) => slot,
            Err(_) => {
                return RasterProbe {
                    kind: RasterKind::Unknown,
                    full_bytes: None,
                };
            }
        };
    let worker = tokio::task::spawn_blocking(move || {
        let _decode_slot = decode_slot;
        let kind = if complete {
            probed_document_kind(&bytes, bytes.len())
        } else {
            RasterKind::Unknown
        };
        RasterProbe {
            kind,
            full_bytes: Some(bytes),
        }
    });
    match tokio::time::timeout(IMAGE_DECODE_DEADLINE, worker).await {
        Ok(Ok(probe)) => probe,
        _ => RasterProbe {
            kind: RasterKind::Unknown,
            full_bytes: None,
        },
    }
}

pub(super) async fn probe_example_raster(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    document: &Document,
) -> Option<bool> {
    match probe_raster_document(ctx, chat, id, document).await.kind {
        RasterKind::Static => Some(false),
        RasterKind::Animated => Some(true),
        RasterKind::Unknown => None,
    }
}

fn rgba_over_neutral(image: image::RgbaImage) -> image::RgbImage {
    const BACKGROUND: u16 = 128;
    let mut rgb = image::RgbImage::new(image.width(), image.height());
    for (source, target) in image.pixels().zip(rgb.pixels_mut()) {
        let alpha = u16::from(source[3]);
        let background = BACKGROUND * (255 - alpha);
        *target = image::Rgb([
            ((u16::from(source[0]) * alpha + background + 127) / 255) as u8,
            ((u16::from(source[1]) * alpha + background + 127) / 255) as u8,
            ((u16::from(source[2]) * alpha + background + 127) / 255) as u8,
        ]);
    }
    rgb
}

fn vision_rgb(image: image::DynamicImage) -> image::RgbImage {
    if image.color().has_alpha() {
        rgba_over_neutral(image.into_rgba8())
    } else {
        image.into_rgb8()
    }
}

pub(crate) fn decode_still(bytes: &[u8]) -> image::ImageResult<image::RgbImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(IMAGE_DIMENSION_MAX);
    limits.max_image_height = Some(IMAGE_DIMENSION_MAX);
    limits.max_alloc = Some(IMAGE_DECODE_BYTES_MAX);
    let mut header = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    header.limits(limits.clone());
    let (width, height) = header.into_dimensions()?;
    let mut pixels = image::Limits::default();
    pixels.max_alloc = Some(IMAGE_PIXEL_MAX);
    pixels.reserve(u64::from(width) * u64::from(height))?;
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder()?;
    limits.reserve(decoder.total_bytes())?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(vision_rgb(image))
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
    let Some(judged) = judged.for_policy(armed.spare) else {
        return;
    };
    report(chat, id, judged, armed, cached);
    if verdict(judged.score, armed.limit, armed.shadow) != Verdict::Delete
        || spared(judged, armed.spare, chat, id)
    {
        return;
    }
    match message.delete_critical_counted().await {
        Ok(0) => return,
        Ok(_) => {}
        Err(e) => {
            eprintln!("nsfw: could not delete in {chat}: {e}");
            return;
        }
    }
    if !ctx.claim_moderation(chat, message.id()) {
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    let chances = match super::strict::punish(ctx, message, chat, LOCK).await {
        super::strict::Outcome::Announced => {
            let action = super::cases::action_key(super::strict::action_of(ctx, chat));
            super::cases::record_delete(ctx, message, LOCK, "محتوای غیراخلاقی", action).await;
            return;
        }
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::cases::record_delete(ctx, message, LOCK, "محتوای غیراخلاقی", "delete").await;
    super::notice::send(ctx, message, chat, "محتوای غیراخلاقی", chances).await;
}

struct DetachedNsfw<'a> {
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    media_id: i64,
    judged: Judgement,
    armed: &'a Armed,
    sender: Option<i64>,
    name: String,
}

async fn act_detached(ctx: &Arc<Ctx>, detached: DetachedNsfw<'_>) {
    let DetachedNsfw {
        chat,
        chat_ref,
        message_id,
        media_id,
        judged,
        armed,
        sender,
        name,
    } = detached;
    let Some(judged) = judged.for_policy(armed.spare) else {
        return;
    };
    report(chat, media_id, judged, armed, false);
    if verdict(judged.score, armed.limit, armed.shadow) != Verdict::Delete
        || spared(judged, armed.spare, chat, media_id)
    {
        return;
    }
    match ctx
        .client
        .delete_messages_critical(chat_ref, &[message_id])
        .await
    {
        Ok(0) => {
            eprintln!("nsfw: delete returned zero events in {chat} msg {message_id}");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("nsfw: could not delete in {chat} msg {message_id}: {e}");
            return;
        }
    }
    if !ctx.claim_moderation(chat, message_id) {
        return;
    }
    ctx.bump(chat, super::stats::DELETED);

    punish_and_notify(
        ctx,
        DetachedModeration {
            chat,
            chat_ref,
            message_id,
            sender,
            name: &name,
            cause: LOCK,
            reason: "محتوای غیراخلاقی",
        },
    )
    .await;
}

pub struct DetachedModeration<'a> {
    pub chat: i64,
    pub chat_ref: PeerRef,
    pub message_id: i32,
    pub sender: Option<i64>,
    pub name: &'a str,
    pub cause: &'a str,
    pub reason: &'a str,
}

pub async fn punish_and_notify(ctx: &Arc<Ctx>, action: DetachedModeration<'_>) {
    let DetachedModeration {
        chat,
        chat_ref,
        message_id,
        sender,
        name,
        cause,
        reason,
    } = action;
    let chances =
        match super::strict::punish_detached(ctx, chat, chat_ref, sender, name, cause).await {
            super::strict::Outcome::Announced => {
                let action = super::cases::action_key(super::strict::action_of(ctx, chat));
                super::cases::record_detached(
                    ctx,
                    super::cases::DetachedRecord {
                        chat,
                        subject: sender,
                        subject_name: name,
                        message: message_id,
                        rule: cause,
                        reason,
                        action,
                    },
                )
                .await;
                return;
            }
            super::strict::Outcome::Chances(left) => Some(left),
            super::strict::Outcome::Nothing => None,
        };
    super::cases::record_detached(
        ctx,
        super::cases::DetachedRecord {
            chat,
            subject: sender,
            subject_name: name,
            message: message_id,
            rule: cause,
            reason,
            action: "delete",
        },
    )
    .await;
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
        .send_message(chat_ref, super::premium::html(text))
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
    } else if judged.arbiter.is_some_and(|a| a.stack < STACK_VETO) {
        "the fitted veto reads it as an ordinary picture"
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
        format!(
            " arbiter {:+.4} head {:.3} stack {:.3}",
            a.explicit, a.head, a.stack
        )
    });
    log::info!(
        "nsfw[{mode}]: chat {chat} file {id} score {score:.4} limit {limit} {over}{weak}{confirmed}{arbiter}{how}"
    );
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[tokio::test]
    async fn decoder_children_have_deadlines_and_output_caps() {
        let started = std::time::Instant::now();
        assert!(
            super::animated_command_limited(
                "/bin/sleep",
                &["30".into()],
                1024,
                std::time::Duration::from_millis(30)
            )
            .await
            .is_none()
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(
            super::animated_command_limited(
                "/usr/bin/head",
                &["-c".into(), "131072".into(), "/dev/zero".into()],
                1024,
                std::time::Duration::from_secs(2)
            )
            .await
            .is_none()
        );
    }
    use super::*;

    #[test]
    fn document_fetch_retries_only_the_first_minus_503() {
        let rpc = |code| {
            InvocationError::Rpc(grammers_client::sender::RpcError {
                code,
                name: "TEMPORARY_FAILURE".into(),
                value: None,
                caused_by: None,
            })
        };
        assert!(retry_document_fetch(0, &rpc(-503)));
        assert!(!retry_document_fetch(1, &rpc(-503)));
        assert!(!retry_document_fetch(0, &rpc(400)));
        assert!(!retry_document_fetch(0, &rpc(503)));
        assert!(!retry_document_fetch(
            0,
            &InvocationError::Io(std::io::Error::from(std::io::ErrorKind::TimedOut))
        ));
    }

    #[test]
    fn a_failed_pool_load_is_retried_then_success_is_cached() {
        let cell = std::sync::OnceLock::new();
        let gate = std::sync::Mutex::new(None);
        let calls = AtomicUsize::new(0);
        assert!(
            initialize_pool_once_retry(&cell, &gate, std::time::Duration::ZERO, || {
                calls.fetch_add(1, Ordering::SeqCst);
                None::<u8>
            })
            .is_none()
        );
        assert!(
            cell.get().is_none(),
            "a failed load must leave the cell empty"
        );
        assert!(
            initialize_pool_once_retry(&cell, &gate, std::time::Duration::from_secs(30), || {
                panic!("the failed load should be rate-limited")
            })
            .is_none()
        );

        assert_eq!(
            initialize_pool_once_retry(&cell, &gate, std::time::Duration::ZERO, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Some(7u8)
            }),
            Some(&7)
        );
        assert_eq!(
            initialize_pool_once_retry(&cell, &gate, std::time::Duration::ZERO, || panic!(
                "successful pool loaded twice"
            )),
            Some(&7)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    fn document_with_mime(mime: &str) -> Document {
        Document::from_raw_media(grammers_client::tl::types::MessageMediaDocument {
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
                    attributes: Vec::new(),
                }
                .into(),
            ),
            alt_documents: None,
            video_cover: None,
            video_timestamp: None,
            ttl_seconds: None,
        })
    }

    #[test]
    fn a_decode_is_held_to_its_thread_budget() {
        let args = vec!["-v".to_owned(), "error".to_owned(), "-i".to_owned(), "x".to_owned()];
        assert_eq!(
            super::decoder_args("ffmpeg", &args),
            ["-threads", super::DECODER_THREADS, "-v", "error", "-i", "x"]
        );
        assert_eq!(super::decoder_args("ffprobe", &args), args);
    }

    #[test]
    fn only_a_video_that_can_arrive_in_time_is_downloaded_whole() {
        let sized = |mime: &str, size: i64, animated: bool| {
            let tl::enums::Document::Document(mut raw) =
                document_with_mime(mime).raw.document.unwrap()
            else {
                unreachable!()
            };
            raw.size = size;
            if animated {
                raw.attributes
                    .push(tl::types::DocumentAttributeAnimated {}.into());
            }
            Document::from_raw_media(tl::types::MessageMediaDocument {
                nopremium: false,
                spoiler: false,
                video: false,
                round: false,
                voice: false,
                document: Some(raw.into()),
                alt_documents: None,
                video_cover: None,
                video_timestamp: None,
                ttl_seconds: None,
            })
        };
        let max = super::VIDEO_FETCH_MAX as i64;
        assert!(super::is_animated_document(&sized("video/mp4", max, false)));
        assert!(!super::is_animated_document(&sized("video/mp4", max + 1, false)));
        assert!(!super::is_animated_document(&sized("video/webm", 60 << 20, false)));
        assert!(super::is_animated_document(&sized("video/mp4", 60 << 20, true)));
        assert!(super::is_animated_document(&sized("image/gif", 60 << 20, false)));
    }

    fn routing_photo(id: i64, dc_id: i32) -> tl::enums::Photo {
        tl::types::Photo {
            has_stickers: false,
            id,
            access_hash: 7,
            file_reference: vec![1],
            date: 0,
            sizes: vec![
                tl::types::PhotoSize {
                    r#type: "x".into(),
                    w: 640,
                    h: 480,
                    size: 10_000,
                }
                .into(),
            ],
            video_sizes: None,
            dc_id,
        }
        .into()
    }

    fn routing_document(id: i64, mime: &str, with_thumb: bool) -> tl::enums::Document {
        let tl::enums::Document::Document(mut raw) = document_with_mime(mime).raw.document.unwrap()
        else {
            unreachable!()
        };
        raw.id = id;
        raw.dc_id = 3;
        if with_thumb {
            raw.thumbs = Some(vec![
                tl::types::PhotoSize {
                    r#type: "x".into(),
                    w: 640,
                    h: 360,
                    size: 8_000,
                }
                .into(),
            ]);
        }
        raw.into()
    }

    fn extended_media(media: tl::enums::MessageMedia) -> tl::enums::MessageExtendedMedia {
        tl::types::MessageExtendedMedia { media }.into()
    }

    fn routing_webpage(
        kind: &str,
        photo: Option<tl::enums::Photo>,
        document: Option<tl::enums::Document>,
        cached_page: Option<tl::enums::Page>,
    ) -> grammers_client::media::WebPage {
        grammers_client::media::WebPage::from_raw_media(tl::types::MessageMediaWebPage {
            force_large_media: false,
            force_small_media: false,
            manual: false,
            safe: false,
            webpage: tl::types::WebPage {
                has_large_media: false,
                video_cover_photo: false,
                id: 100,
                url: "https://example.org/a".into(),
                display_url: "example.org/a".into(),
                hash: 0,
                r#type: Some(kind.into()),
                site_name: None,
                title: None,
                description: None,
                photo,
                embed_url: None,
                embed_type: None,
                embed_width: None,
                embed_height: None,
                duration: None,
                author: None,
                document,
                cached_page,
                attributes: None,
            }
            .into(),
        })
    }

    fn routing_caption() -> tl::enums::PageCaption {
        tl::types::PageCaption {
            text: tl::enums::RichText::TextEmpty,
            credit: tl::enums::RichText::TextEmpty,
        }
        .into()
    }

    #[test]
    fn document_only_webpage_reuses_document_animation_and_still_routes() {
        let page = routing_webpage(
            "gif",
            None,
            Some(routing_document(42, "video/mp4", true)),
            None,
        );
        let visuals = webpage_visual_media(&page).unwrap();
        assert_eq!(visuals.len(), 1);
        assert_eq!(file_id(&visuals[0]), Some(42));
        assert!(animated_document(&visuals[0]).is_some());
        let route_ladder = ladder(&visuals[0]).unwrap();
        assert_eq!(route_ladder.dc, Some(3));
        assert!(matches!(
            route_ladder.rung(route_ladder.start).to_raw_input_location(),
            Some(tl::enums::InputFileLocation::InputDocumentFileLocation(location)) if location.id == 42
        ));

        let page = routing_webpage(
            "document",
            None,
            Some(routing_document(43, "image/png", false)),
            None,
        );
        let visuals = webpage_visual_media(&page).unwrap();
        assert_eq!(file_id(&visuals[0]), Some(43));
        assert!(ladder(&visuals[0]).is_none());
        assert!(still_image_without_thumb(&visuals[0], false).is_some());
    }

    #[test]
    fn webpage_video_scores_its_photo_poster_and_document_frames() {
        let page = routing_webpage(
            "video",
            Some(routing_photo(99, 7)),
            Some(routing_document(42, "video/mp4", true)),
            None,
        );
        let visuals = webpage_visual_media(&page).unwrap();
        assert_eq!(visuals.len(), 1);
        assert_eq!(file_id(&visuals[0]), Some(42));
        let document = animated_document(&visuals[0]).unwrap();
        let cover = explicit_video_cover_ladder(document).unwrap();
        assert_eq!(cover.dc, Some(7));
        assert!(matches!(
            cover.rung(cover.start).to_raw_input_location(),
            Some(tl::enums::InputFileLocation::InputPhotoFileLocation(location)) if location.id == 99
        ));
        let ordinary = routing_webpage(
            "document",
            Some(routing_photo(99, 7)),
            Some(routing_document(43, "image/png", true)),
            None,
        );
        let visuals = webpage_visual_media(&ordinary).unwrap();
        assert_eq!(
            visuals.iter().map(file_id).collect::<Vec<_>>(),
            [Some(99), Some(43)]
        );
        assert!(!webpage_original(&visuals[0]));
        assert!(visual_ladder(&visuals[0], webpage_original(&visuals[0])).is_some());
        let Media::Document(_) = &visuals[1] else {
            panic!("raster document")
        };
        assert_eq!(
            file_id(&Media::WebPage(ordinary)),
            Some(99),
            "the page still has its separate poster"
        );
        assert!(ordinary_raster_document(&visuals[1]).is_some());
        assert!(ladder(&visuals[1]).is_some(), "document has a thumbnail");
        assert!(webpage_original(&visuals[1]));
        assert!(visual_ladder(&visuals[1], webpage_original(&visuals[1])).is_none());
        assert!(still_image_without_thumb(&visuals[1], false).is_some());
        assert_eq!(
            verdict_route(false, None, true),
            Some(VisionRoute::Original)
        );
    }

    #[test]
    fn telegram_album_routes_displayed_media_and_distinct_poster() {
        let items = vec![
            tl::types::PageBlockPhoto {
                spoiler: false,
                photo_id: 11,
                caption: routing_caption(),
                url: None,
                webpage_id: None,
            }
            .into(),
            tl::types::PageBlockVideo {
                autoplay: false,
                r#loop: false,
                spoiler: false,
                video_id: 22,
                caption: routing_caption(),
            }
            .into(),
        ];
        for block in [
            tl::types::PageBlockCollage {
                items: items.clone(),
                caption: routing_caption(),
            }
            .into(),
            tl::types::PageBlockSlideshow {
                items,
                caption: routing_caption(),
            }
            .into(),
        ] {
            let cached_page = tl::types::Page {
                part: false,
                rtl: false,
                v2: true,
                url: "https://t.me/a/1".into(),
                blocks: vec![block],
                photos: vec![routing_photo(11, 1), routing_photo(44, 1)],
                documents: vec![
                    routing_document(22, "video/mp4", true),
                    routing_document(55, "video/mp4", true),
                ],
                views: None,
            };
            let page = routing_webpage(
                "telegram_album",
                Some(routing_photo(44, 1)),
                None,
                Some(cached_page.into()),
            );
            let visuals = webpage_visual_media(&page).unwrap();
            assert_eq!(visuals.len(), 3);
            assert_eq!(
                visuals.iter().map(file_id).collect::<Vec<_>>(),
                vec![Some(11), Some(22), Some(44)]
            );
            assert!(animated_document(&visuals[1]).is_some());
        }

        let items = (0..11)
            .map(|id| {
                tl::enums::PageBlock::Photo(tl::types::PageBlockPhoto {
                    spoiler: false,
                    photo_id: id,
                    caption: routing_caption(),
                    url: None,
                    webpage_id: None,
                })
            })
            .collect();
        let cached_page = tl::types::Page {
            part: false,
            rtl: false,
            v2: true,
            url: "https://t.me/a/1".into(),
            blocks: vec![
                tl::types::PageBlockCollage {
                    items,
                    caption: routing_caption(),
                }
                .into(),
            ],
            photos: (0..11).map(|id| routing_photo(id, 1)).collect(),
            documents: Vec::new(),
            views: None,
        };
        let page = routing_webpage(
            "telegram_album",
            Some(routing_photo(0, 1)),
            None,
            Some(cached_page.into()),
        );
        assert_eq!(webpage_visual_media(&page).unwrap().len(), 10);
    }

    #[test]
    fn telegram_album_without_cached_visuals_falls_back_to_top_level_preview() {
        let photo = routing_photo(77, 2);
        let page = routing_webpage("telegram_album", Some(photo.clone()), None, None);
        assert!(webpage_visual_media(&page).is_none());
        assert_eq!(file_id(&Media::WebPage(page)), Some(77));

        let cached_page = tl::types::Page {
            part: false,
            rtl: false,
            v2: true,
            url: "https://t.me/a/1".into(),
            blocks: Vec::new(),
            photos: Vec::new(),
            documents: Vec::new(),
            views: None,
        };
        let page = routing_webpage(
            "telegram_album",
            Some(photo),
            None,
            Some(cached_page.into()),
        );
        assert!(webpage_visual_media(&page).is_none());
        assert_eq!(file_id(&Media::WebPage(page)), Some(77));
    }

    #[test]
    fn pending_webpage_event_first_completion_is_seen_on_registration() {
        let page = routing_webpage("photo", Some(routing_photo(77, 2)), None, None)
            .raw
            .webpage;
        let now = Instant::now();
        let mut pending = PendingWebPages::default();
        assert_eq!(pending.resolved(&page, Some(-100), now), Some(100));
        pending.register(-100, 7, 100, 1_000, now, 1_000);
        pending.register(-200, 8, 100, 1_000, now, 1_000);
        let work = pending.due(now, None);
        assert_eq!(work.len(), 1);
        assert_eq!((work[0].chat, work[0].message_id), (-100, 7));
        assert!(work[0].resolved.is_some());
        pending.finish(&work[0], RescanOutcome::Done, now);
        assert!(!pending.waiting.contains_key(&(-100, 7)));
    }

    #[test]
    fn pending_webpage_delayed_fetch_is_single_when_still_pending_but_retries_errors() {
        let now = Instant::now();
        let mut pending = PendingWebPages::default();
        pending.register(-100, 7, 100, 1_015, now, 1_000);
        assert!(pending.due(now + Duration::from_secs(44), None).is_empty());
        let work = pending.due(now + Duration::from_secs(45), None);
        assert_eq!(work.len(), 1);
        assert!(work[0].resolved.is_none());
        pending.finish(
            &work[0],
            RescanOutcome::StillPending,
            now + Duration::from_secs(45),
        );
        assert!(pending.due(now + Duration::from_secs(70), None).is_empty());

        pending.register(-100, 8, 101, 1_000, now, 1_000);
        let work = pending.due(now + Duration::from_secs(30), None);
        assert_eq!(work.len(), 1);
        pending.finish(
            &work[0],
            RescanOutcome::Retry,
            now + Duration::from_secs(30),
        );
        assert_eq!(pending.due(now + Duration::from_secs(35), None).len(), 1);
    }

    #[test]
    fn pending_webpage_registry_caps_messages_and_drains_within_ttl() {
        let now = Instant::now();
        let mut pending = PendingWebPages::default();
        for id in 0..=PENDING_WEBPAGE_MAX {
            pending.register(
                -100,
                id as i32,
                100,
                1_030,
                now + Duration::from_millis(id as u64),
                1_000,
            );
        }
        assert_eq!(pending.waiting.len(), PENDING_WEBPAGE_MAX);
        assert!(!pending.waiting.contains_key(&(-100, 0)));
        let mut drained = 0;
        for tick in 0..32 {
            let at = now + Duration::from_secs(61 + tick * 5);
            let work = pending.due(at, None);
            drained += work.len();
            for item in &work {
                pending.finish(item, RescanOutcome::Done, at);
            }
        }
        assert_eq!(drained, PENDING_WEBPAGE_MAX);
        assert!(pending.waiting.is_empty());

        pending.register(-100, 999, 100, 1_000, now, 1_000);
        pending.prune(now + PENDING_WEBPAGE_TTL + Duration::from_secs(1));
        assert!(pending.waiting.is_empty());
    }

    #[test]
    fn pending_webpage_partial_album_retry_skips_scheduled_visuals() {
        let items = (0..10)
            .map(|id| {
                tl::enums::PageBlock::Photo(tl::types::PageBlockPhoto {
                    spoiler: false,
                    photo_id: id,
                    caption: routing_caption(),
                    url: None,
                    webpage_id: None,
                })
            })
            .collect();
        let cached_page = tl::types::Page {
            part: false,
            rtl: false,
            v2: true,
            url: "https://t.me/a/1".into(),
            blocks: vec![
                tl::types::PageBlockCollage {
                    items,
                    caption: routing_caption(),
                }
                .into(),
            ],
            photos: (0..10).map(|id| routing_photo(id, 1)).collect(),
            documents: Vec::new(),
            views: None,
        };
        let page = routing_webpage(
            "telegram_album",
            Some(routing_photo(99, 1)),
            None,
            Some(cached_page.into()),
        );
        let visuals = webpage_visual_media(&page).unwrap();
        assert_eq!(visuals.len(), 11);
        let ids = visuals.iter().map(pending_visual_key).collect::<Vec<_>>();
        let now = Instant::now();
        let mut pending = PendingWebPages::default();
        pending.register(-100, 7, 100, 1_000, now, 1_000);
        let work = pending.due(now + Duration::from_secs(30), None);
        pending.finish(
            &work[0],
            RescanOutcome::QueueFull {
                next_visual: 5,
                visual_ids: ids.clone(),
            },
            now + Duration::from_secs(30),
        );
        pending.register(-100, 7, 100, 1_000, now + Duration::from_secs(34), 1_034);
        let work = pending.due(now + Duration::from_secs(35), None);
        assert_eq!(pending_resume_index(&work[0], &ids), 5);
        let mut changed = ids.clone();
        changed.swap(0, 1);
        assert_eq!(pending_resume_index(&work[0], &changed), 0);
        let mut with_unscorable_prefix = work;
        with_unscorable_prefix[0].visual_ids.insert(0, None);
        with_unscorable_prefix[0].next_visual = 6;
        let mut aligned = ids;
        aligned.insert(0, None);
        assert_eq!(
            pending_resume_index(&with_unscorable_prefix[0], &aligned),
            6
        );
    }

    #[test]
    fn pending_webpage_oversized_completion_racing_a_fetch_gets_one_more_try() {
        let now = Instant::now();
        let mut pending = PendingWebPages::default();
        pending.register(-100, 7, 100, 1_000, now, 1_000);
        let work = pending.due(now + Duration::from_secs(30), None);
        assert_eq!(work.len(), 1);
        let mut page = routing_webpage("photo", Some(routing_photo(77, 2)), None, None)
            .raw
            .webpage;
        let tl::enums::WebPage::Page(raw) = &mut page else {
            panic!("resolved page")
        };
        raw.description = Some("x".repeat(RECENT_WEBPAGE_BYTES_MAX));
        assert_eq!(
            pending.resolved(&page, None, now + Duration::from_secs(31)),
            Some(100)
        );
        assert!(pending.recent.is_empty());
        assert!(pending.due(now + Duration::from_secs(31), None).is_empty());
        pending.finish(
            &work[0],
            RescanOutcome::StillPending,
            now + Duration::from_secs(32),
        );
        let retry = pending.due(now + Duration::from_secs(37), None);
        assert_eq!(retry.len(), 1);
        assert!(retry[0].resolved.is_none());
        pending.finish(
            &retry[0],
            RescanOutcome::StillPending,
            now + Duration::from_secs(37),
        );
        assert!(pending.due(now + Duration::from_secs(50), None).is_empty());
    }

    #[test]
    fn pending_webpage_refetch_rejects_edited_media_and_uses_current_page() {
        let resolved = routing_webpage("photo", Some(routing_photo(77, 2)), None, None)
            .raw
            .webpage;
        let mut current = routing_webpage("photo", None, None, None).raw;
        current.webpage = tl::types::WebPagePending {
            id: 100,
            url: None,
            date: 0,
        }
        .into();
        let media = tl::enums::MessageMedia::WebPage(*current.clone());
        assert!(matches!(
            rescan_webpage_media(Some(&media), 100, None),
            RescanMedia::StillPending
        ));
        assert!(matches!(
            rescan_webpage_media(Some(&media), 100, Some(&resolved)),
            RescanMedia::Ready(media) if matches!(*media, Media::WebPage(_))
        ));
        current.webpage = tl::types::WebPagePending {
            id: 101,
            url: None,
            date: 0,
        }
        .into();
        let edited = tl::enums::MessageMedia::WebPage(*current);
        assert!(matches!(
            rescan_webpage_media(Some(&edited), 100, Some(&resolved)),
            RescanMedia::Unrelated
        ));
        assert!(matches!(
            rescan_webpage_media(None, 100, Some(&resolved)),
            RescanMedia::Unrelated
        ));

        let current = routing_webpage("photo", Some(routing_photo(88, 2)), None, None).raw;
        let current = tl::enums::MessageMedia::WebPage(*current);
        let RescanMedia::Ready(media) = rescan_webpage_media(Some(&current), 100, Some(&resolved))
        else {
            panic!("current resolved preview should be scanned");
        };
        assert_eq!(file_id(&media), Some(88));
    }

    #[test]
    fn pending_webpage_channel_update_has_group_dispatch_key() {
        let page = routing_webpage("photo", None, None, None).raw.webpage;
        let state = grammers_session::updates::State {
            date: 0,
            seq: 0,
            message_box: None,
        };
        let channel = grammers_client::update::Raw {
            raw: tl::types::UpdateChannelWebPage {
                channel_id: 123,
                webpage: page.clone(),
                pts: 1,
                pts_count: 1,
            }
            .into(),
            state: state.clone(),
        };
        let chat = PeerId::channel(123)
            .and_then(PeerId::bot_api_dialog_id)
            .unwrap();
        assert_eq!(super::super::raw_chat_id(&channel), Some(chat));
        assert_eq!(
            super::super::dispatch_key(&grammers_client::update::Update::Raw(channel)),
            chat
        );
        let global = grammers_client::update::Raw {
            raw: tl::types::UpdateWebPage {
                webpage: page,
                pts: 1,
                pts_count: 1,
            }
            .into(),
            state,
        };
        assert_eq!(super::super::raw_chat_id(&global), None);
    }

    #[test]
    fn game_story_and_live_photo_expose_their_pixels() {
        let photo = routing_photo(11, 1);
        let game = tl::enums::MessageMedia::Game(tl::types::MessageMediaGame {
            game: tl::types::Game {
                id: 1,
                access_hash: 2,
                short_name: "game".into(),
                title: "Game".into(),
                description: String::new(),
                photo: photo.clone(),
                document: None,
            }
            .into(),
        });
        assert_eq!(
            file_id(&embedded_visual_media_raw(&game).unwrap()),
            Some(11)
        );

        let still = tl::types::MessageMediaPhoto {
            spoiler: false,
            live_photo: false,
            photo: Some(photo.clone()),
            ttl_seconds: None,
            video: None,
        };
        let mut story = tl::enums::MessageMedia::Story(Box::new(tl::types::MessageMediaStory {
            via_mention: false,
            peer: tl::types::PeerUser { user_id: 1 }.into(),
            id: 1,
            story: Some(
                tl::types::StoryItem {
                    pinned: false,
                    public: true,
                    close_friends: false,
                    min: false,
                    noforwards: false,
                    edited: false,
                    contacts: false,
                    selected_contacts: false,
                    out: false,
                    id: 1,
                    date: 0,
                    from_id: None,
                    fwd_from: None,
                    expire_date: 0,
                    caption: None,
                    entities: None,
                    media: still.into(),
                    media_areas: None,
                    privacy: None,
                    views: None,
                    sent_reaction: None,
                    albums: None,
                    music: None,
                }
                .into(),
            ),
        }));
        assert_eq!(
            file_id(&embedded_visual_media_raw(&story).unwrap()),
            Some(11)
        );

        let live = tl::types::MessageMediaPhoto {
            spoiler: false,
            live_photo: true,
            photo: Some(photo),
            ttl_seconds: None,
            video: Some(routing_document(22, "video/mp4", true)),
        };
        let media = live_photo_media(&live).unwrap();
        assert_eq!(file_id(&media), Some(22));
        let document = animated_document(&media).unwrap();
        assert!(explicit_video_cover_ladder(document).is_some());
        let tl::enums::MessageMedia::Story(story) = &mut story else {
            unreachable!()
        };
        let Some(tl::enums::StoryItem::Item(item)) = &mut story.story else {
            unreachable!()
        };
        item.media = live.into();
        let nested = tl::enums::MessageMedia::Story(story.clone());
        assert_eq!(
            file_id(&embedded_visual_media_raw(&nested).unwrap()),
            Some(22)
        );
    }

    #[test]
    fn poll_attached_and_solution_pictures_reach_the_image_pipeline() {
        let mut poll = tl::types::MessageMediaPoll {
            poll: tl::types::Poll {
                id: 1,
                closed: false,
                public_voters: false,
                multiple_choice: false,
                quiz: false,
                open_answers: false,
                revoting_disabled: false,
                shuffle_answers: false,
                hide_results_until_close: false,
                creator: false,
                subscribers_only: false,
                question: tl::types::TextWithEntities {
                    text: "Question".into(),
                    entities: Vec::new(),
                }
                .into(),
                answers: Vec::new(),
                close_period: None,
                close_date: None,
                countries_iso2: None,
                hash: 0,
            }
            .into(),
            results: tl::types::PollResults {
                min: false,
                has_unread_votes: false,
                can_view_stats: false,
                results: None,
                total_voters: None,
                recent_voters: None,
                solution: None,
                solution_entities: None,
                solution_media: Some(document_with_mime("image/png").raw.into()),
            }
            .into(),
            attached_media: Some(
                tl::types::MessageMediaPhoto {
                    spoiler: false,
                    live_photo: false,
                    photo: Some(routing_photo(11, 1)),
                    ttl_seconds: None,
                    video: None,
                }
                .into(),
            ),
        };
        assert!(matches!(
            Media::from_raw(tl::enums::MessageMedia::Poll(Box::new(poll.clone()))),
            Some(Media::Poll(_))
        ));
        assert_eq!(
            poll_visual_media(&poll)
                .iter()
                .map(file_id)
                .collect::<Vec<_>>(),
            [Some(11), Some(42)]
        );
        poll.attached_media = None;
        let tl::enums::PollResults::Results(results) = &mut poll.results;
        results.solution_media = None;
        assert!(poll_visual_media(&poll).is_empty());
    }

    #[test]
    fn paid_album_images_reach_the_image_pipeline_with_a_ten_item_cap() {
        let full = |id| {
            extended_media(
                tl::types::MessageMediaPhoto {
                    spoiler: false,
                    live_photo: false,
                    photo: Some(routing_photo(id, 1)),
                    ttl_seconds: None,
                    video: None,
                }
                .into(),
            )
        };
        let raw = tl::types::MessageMediaPaidMedia {
            stars_amount: 1,
            extended_media: std::iter::once(
                tl::types::MessageExtendedMediaPreview {
                    w: Some(16),
                    h: Some(16),
                    thumb: None,
                    video_duration: None,
                }
                .into(),
            )
            .chain((1..=11).map(full))
            .collect(),
        };
        let visuals = embedded_visual_media_set_raw(&raw.into()).unwrap();
        assert_eq!(visuals.len(), 9);
        assert_eq!(
            visuals.iter().map(file_id).collect::<Vec<_>>(),
            (1..=9).map(Some).collect::<Vec<_>>()
        );
    }

    #[test]
    fn invoice_extended_photo_reaches_the_image_pipeline() {
        let mut raw = tl::types::MessageMediaInvoice {
            shipping_address_requested: false,
            test: false,
            title: "item".into(),
            description: String::new(),
            photo: None,
            receipt_msg_id: None,
            currency: "USD".into(),
            total_amount: 1,
            start_param: String::new(),
            extended_media: Some(extended_media(
                tl::types::MessageMediaPhoto {
                    spoiler: false,
                    live_photo: false,
                    photo: Some(routing_photo(77, 1)),
                    ttl_seconds: None,
                    video: None,
                }
                .into(),
            )),
        };
        raw.photo = Some(
            tl::types::WebDocumentNoProxy {
                url: "https://cdn.example.com/invoice.jpg".into(),
                size: 1024,
                mime_type: "image/jpeg".into(),
                attributes: Vec::new(),
            }
            .into(),
        );
        let (extended, web) = invoice_visual_media(&raw);
        assert_eq!(extended.as_ref().and_then(file_id), Some(77));
        assert!(web.and_then(direct_invoice_web_photo).is_some());
        let visuals = embedded_visual_media_set_raw(&raw.clone().into()).unwrap();
        assert_eq!(visuals.iter().map(file_id).collect::<Vec<_>>(), [Some(77)]);

        raw.photo = Some(
            tl::types::WebDocument {
                url: "https://cdn.example.com/proxied.jpg".into(),
                access_hash: 123,
                size: 1024,
                mime_type: "image/jpeg".into(),
                attributes: Vec::new(),
            }
            .into(),
        );
        let (extended, web) = invoice_visual_media(&raw);
        assert_eq!(extended.as_ref().and_then(file_id), Some(77));
        assert!(web.and_then(direct_invoice_web_photo).is_none());
    }

    #[test]
    fn content_animation_uses_bytes_and_rejects_unknown_containers() {
        assert_eq!(content_animation(b"\xff\xd8\xff\xd9"), Some(false));
        assert_eq!(content_animation(b"GIF89a"), Some(true));
        for signature in [
            b"BM".as_slice(),
            b"#?RADIANCE".as_slice(),
            b"qoif".as_slice(),
            b"farbfeld".as_slice(),
        ] {
            assert_eq!(content_animation(signature), Some(false));
        }
        assert_eq!(content_animation(b"P6\n"), Some(true));
        assert_eq!(content_animation(b"\0\0\x01\0"), Some(true));
        assert_eq!(content_animation(b"II*\0"), Some(true));
        assert_eq!(content_animation(b"\0\0\0\x18ftypisom"), Some(true));
        assert_eq!(content_animation(b"\x1a\x45\xdf\xa3"), Some(true));
        assert_eq!(content_animation(b"not an image"), None);
    }

    #[tokio::test]
    async fn tiff_alpha_metadata_does_not_double_multiply() {
        use tiff::encoder::{TiffEncoder, colortype};
        use tiff::tags::ExtraSamples;

        let make = |alpha| {
            let mut bytes = Cursor::new(Vec::new());
            {
                let mut encoder = TiffEncoder::new(&mut bytes).unwrap();
                let mut image = encoder.new_image::<colortype::RGB8>(1, 1).unwrap();
                image.extra_samples(&[alpha]).unwrap();
                image.write_data(&[100, 50, 25, 128]).unwrap();
            }
            bytes.into_inner()
        };
        let straight = decode_animated_frames(make(ExtraSamples::UnassociatedAlpha))
            .await
            .expect("straight-alpha TIFF");
        assert_eq!(straight.len(), 1);
        assert_eq!(straight[0].get_pixel(0, 0).0, [114, 89, 76]);
        assert!(
            decode_animated_frames(make(ExtraSamples::AssociatedAlpha))
                .await
                .is_none()
        );
        let mut missing = Cursor::new(Vec::new());
        {
            let mut encoder = TiffEncoder::new(&mut missing).unwrap();
            encoder
                .new_image::<colortype::RGBA8>(1, 1)
                .unwrap()
                .write_data(&[100, 50, 25, 128])
                .unwrap();
        }
        assert!(decode_animated_frames(missing.into_inner()).await.is_none());
    }

    #[test]
    fn bmp_still_decoder_keeps_pixels_and_rejects_invalid_or_oversized_input() {
        let source = image::RgbImage::from_fn(2, 2, |x, y| {
            image::Rgb([(x * 100) as u8, (y * 100) as u8, 42])
        });
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(source.clone())
            .write_to(&mut encoded, image::ImageFormat::Bmp)
            .expect("encode BMP fixture");
        assert_eq!(decode_still(encoded.get_ref()).unwrap(), source);
        assert!(decode_still(b"BM\0\0").is_err());

        let wide = image::RgbImage::new(IMAGE_DIMENSION_MAX + 1, 1);
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(wide)
            .write_to(&mut encoded, image::ImageFormat::Bmp)
            .expect("encode oversized BMP fixture");
        assert!(decode_still(encoded.get_ref()).is_err());
    }

    #[test]
    fn game_animation_and_still_document_reach_the_image_pipeline() {
        let make_game = |document| {
            tl::enums::MessageMedia::Game(tl::types::MessageMediaGame {
                game: tl::types::Game {
                    id: 1,
                    access_hash: 2,
                    short_name: "game".into(),
                    title: "Game".into(),
                    description: String::new(),
                    photo: routing_photo(11, 1),
                    document: Some(document),
                }
                .into(),
            })
        };
        let animated =
            embedded_visual_media_set_raw(&make_game(routing_document(22, "video/mp4", true)))
                .unwrap();
        assert_eq!(animated.iter().map(file_id).collect::<Vec<_>>(), [Some(22)]);
        let Media::Document(document) = &animated[0] else {
            panic!("game animation was not routed as a document");
        };
        assert!(explicit_video_cover_ladder(document).is_some());

        let still =
            embedded_visual_media_set_raw(&make_game(routing_document(33, "image/png", false)))
                .unwrap();
        assert_eq!(
            still.iter().map(file_id).collect::<Vec<_>>(),
            [Some(11), Some(33)]
        );
    }

    #[test]
    fn video_cover_ladder_keeps_its_photo_source_and_datacentre() {
        use grammers_client::tl::{enums, types};

        let mut document = document_with_mime("video/mp4");
        if let Some(enums::Document::Document(raw)) = document.raw.document.as_mut() {
            raw.thumbs = Some(vec![
                types::PhotoSize {
                    r#type: "x".into(),
                    w: 1024,
                    h: 576,
                    size: 1024,
                }
                .into(),
            ]);
        }
        document.raw.video_cover = Some(
            types::Photo {
                has_stickers: false,
                id: 99,
                access_hash: 8,
                file_reference: vec![2],
                date: 0,
                sizes: vec![
                    types::PhotoSize {
                        r#type: "x".into(),
                        w: 720,
                        h: 405,
                        size: 20_000,
                    }
                    .into(),
                    types::PhotoSize {
                        r#type: "y".into(),
                        w: 1280,
                        h: 720,
                        size: 40_000,
                    }
                    .into(),
                ],
                video_sizes: None,
                dc_id: 7,
            }
            .into(),
        );

        let cover = ladder(&Media::Document(document.clone())).unwrap();
        assert_eq!(cover.start, 0);
        assert_eq!(cover.sizes.len(), 2);
        for at in 0..2 {
            let rung = cover.rung(at);
            assert_eq!(rung.dc_id(), Some(7));
            assert!(matches!(
                rung.to_raw_input_location(),
                Some(enums::InputFileLocation::InputPhotoFileLocation(location)) if location.id == 99
            ));
        }

        if let Some(enums::Photo::Photo(cover)) = document.raw.video_cover.as_mut() {
            cover.sizes = vec![
                types::PhotoStrippedSize {
                    r#type: "i".into(),
                    bytes: vec![0x01, 40, 20, 0xff, 0xd9],
                }
                .into(),
            ];
        }
        let fallback = ladder(&Media::Document(document.clone())).unwrap();
        assert!(matches!(
            fallback.rung(fallback.start).to_raw_input_location(),
            Some(enums::InputFileLocation::InputDocumentFileLocation(location)) if location.id == 42
        ));

        if let Some(enums::Photo::Photo(cover)) = document.raw.video_cover.as_mut() {
            cover.sizes.clear();
        }
        let fallback = ladder(&Media::Document(document)).unwrap();
        assert_eq!(fallback.rung(fallback.start).dc_id(), Some(1));
        assert!(matches!(
            fallback.rung(fallback.start).to_raw_input_location(),
            Some(enums::InputFileLocation::InputDocumentFileLocation(location)) if location.id == 42
        ));
    }

    #[test]
    fn static_document_with_independent_cover_routes_both_images() {
        let mut document = document_with_mime("image/png");
        if let Some(tl::enums::Document::Document(raw)) = document.raw.document.as_mut() {
            raw.thumbs = Some(vec![
                tl::types::PhotoSize {
                    r#type: "x".into(),
                    w: 640,
                    h: 360,
                    size: 8_000,
                }
                .into(),
            ]);
        }
        document.raw.video_cover = Some(routing_photo(99, 7));
        let media = Media::Document(document);
        let (cover, original) = static_document_surfaces(&media).unwrap();
        assert_eq!(file_id(&cover), Some(99));
        assert_eq!(file_id(&original), Some(42));
        assert!(ladder(&cover).is_some());
        assert!(visual_ladder(&original, true).is_none());
        assert!(still_image_without_thumb(&original, false).is_some());
        assert!(matches!(
            original,
            Media::Document(ref document) if document.raw.video_cover.is_none()
        ));
        assert!(
            static_document_surfaces(&Media::Document(document_with_mime("video/mp4"))).is_none()
        );
    }

    #[test]
    fn image_documents_without_thumbnails_are_routed_to_the_original() {
        let document = document_with_mime("image/png");
        let media = Media::Document(document.clone());
        assert!(ladder(&media).is_none());
        assert_eq!(still_image_without_thumb(&media, false).unwrap(), document);
        assert!(still_image_without_thumb(&media, true).is_none());

        let mut pdf = document;
        if let Some(grammers_client::tl::enums::Document::Document(raw)) = pdf.raw.document.as_mut()
        {
            raw.mime_type = "application/pdf".to_owned();
        }
        assert!(still_image_without_thumb(&Media::Document(pdf), false).is_none());
    }

    #[test]
    fn jpeg_documents_with_generic_mime_and_no_thumbnail_require_probe() {
        for extension in ["jpg", "jpeg", "jpe", "jfif"] {
            let mut document = document_with_mime("application/octet-stream");
            if let Some(tl::enums::Document::Document(raw)) = document.raw.document.as_mut() {
                raw.size = (THUMBNAIL_BYTES_MAX + 1) as i64;
                raw.attributes.push(
                    tl::types::DocumentAttributeFilename {
                        file_name: format!("camera.{extension}"),
                    }
                    .into(),
                );
            }
            let media = Media::Document(document.clone());
            assert!(ladder(&media).is_none());
            assert_eq!(still_image_without_thumb(&media, false), Some(document));
            let raster_candidate = ordinary_raster_document(&media);
            assert!(raster_candidate.is_some());
            assert!(raster_cache_route(false, raster_candidate.is_some(), None, false).probe);
        }
    }

    #[tokio::test]
    async fn jpeg_named_document_with_thumbnail_must_probe_original_before_cache() {
        let mut document = document_with_mime("application/octet-stream");
        if let Some(tl::enums::Document::Document(raw)) = document.raw.document.as_mut() {
            raw.attributes.push(
                tl::types::DocumentAttributeFilename {
                    file_name: "camera.jpg".into(),
                }
                .into(),
            );
            raw.thumbs = Some(vec![
                tl::types::PhotoSize {
                    r#type: "x".into(),
                    w: 640,
                    h: 360,
                    size: 8_000,
                }
                .into(),
            ]);
        }
        let media = Media::Document(document);
        assert!(ladder(&media).is_some());
        assert!(still_image_without_thumb(&media, true).is_none());
        let candidate = ordinary_raster_document(&media);
        assert!(candidate.is_some());
        let route = raster_cache_route(false, candidate.is_some(), None, false);
        assert!(route.probe);
        assert!(!route.cache_ready);
        let apng = include_bytes!("../../tests/fixtures/media/two-frame.apng");
        assert_eq!(probed_document_kind(apng, apng.len()), RasterKind::Animated);
        let frames = decode_animated_frames(apng.to_vec()).await.unwrap();
        assert_eq!(frames.len(), 2);
        assert_ne!(frames[0].get_pixel(0, 0), frames[1].get_pixel(0, 0));

        let mime_only = Media::Document(document_with_mime("image/jpeg"));
        assert!(ordinary_raster_document(&mime_only).is_some());
    }

    #[test]
    fn jpeg_document_probe_distinguishes_ordinary_and_disguised_moving_bytes() {
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([30, 90, 180]),
        ))
        .write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .unwrap();
        assert_eq!(probed_document_kind(&jpeg, jpeg.len()), RasterKind::Static);

        let mut large_jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(128, 128, |x, y| {
            image::Rgb([
                (x.wrapping_mul(73) ^ y.wrapping_mul(151)) as u8,
                (x.wrapping_mul(37) ^ y.wrapping_mul(91)) as u8,
                (x.wrapping_mul(113) ^ y.wrapping_mul(67)) as u8,
            ])
        }))
        .write_to(&mut Cursor::new(&mut large_jpeg), image::ImageFormat::Jpeg)
        .unwrap();
        assert!(large_jpeg.len() > 4 * 1024);
        assert_eq!(
            probed_document_kind(&large_jpeg[..4 * 1024], large_jpeg.len()),
            RasterKind::Static
        );

        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            for color in [image::Rgba([240, 0, 0, 255]), image::Rgba([0, 240, 0, 255])] {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(8, 8, color)))
                    .unwrap();
            }
        }
        assert_eq!(probed_document_kind(&gif, gif.len()), RasterKind::Animated);
        for bytes in [
            include_bytes!("../../tests/fixtures/media/two-frame.apng").as_slice(),
            include_bytes!("../../tests/fixtures/media/two-frame.webp").as_slice(),
        ] {
            assert_eq!(
                probed_document_kind(bytes, bytes.len()),
                RasterKind::Animated
            );
        }
        assert_eq!(
            probed_document_kind(&gif[..8], gif.len()),
            RasterKind::Animated
        );
        assert_eq!(
            probed_document_kind(&jpeg[..8], jpeg.len()),
            RasterKind::Static
        );
        assert_eq!(
            probed_document_kind(b"\0\0\0\x18ftypisom", 128),
            RasterKind::Animated
        );
        assert_eq!(
            probed_document_kind(b"broken image", 128),
            RasterKind::Unknown
        );
    }

    #[tokio::test]
    async fn generic_jpeg_name_with_gif_bytes_samples_both_frames() {
        let mut document = document_with_mime("application/octet-stream");
        if let Some(tl::enums::Document::Document(raw)) = document.raw.document.as_mut() {
            raw.attributes.push(
                tl::types::DocumentAttributeFilename {
                    file_name: "camera.jpg".into(),
                }
                .into(),
            );
        }
        let media = Media::Document(document);
        assert!(still_image_without_thumb(&media, false).is_some());
        let unresolved = raster_cache_route(false, true, None, false);
        assert!(unresolved.probe && !unresolved.cache_ready);

        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            for color in [image::Rgba([240, 0, 0, 255]), image::Rgba([0, 240, 0, 255])] {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(8, 8, color)))
                    .unwrap();
            }
        }
        let first_only = decode_still(&gif).unwrap();
        assert!(first_only.get_pixel(0, 0)[0] > first_only.get_pixel(0, 0)[1]);
        assert_eq!(content_animation(&gif), Some(true));
        let frames = decode_animated_frames(gif).await.unwrap();
        assert_eq!(frames.len(), 2);
        assert_ne!(frames[0].get_pixel(0, 0), frames[1].get_pixel(0, 0));
        assert_eq!(
            content_animation(include_bytes!(
                "../../tests/fixtures/media/two-frame.apng"
            )),
            Some(true)
        );
        assert_eq!(
            content_animation(include_bytes!(
                "../../tests/fixtures/media/two-frame.webp"
            )),
            Some(true)
        );
        let animated = raster_cache_route(false, true, Some(true), false);
        assert!(animated.animated && animated.cache_ready);

        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([30, 90, 180]),
        ))
        .write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .unwrap();
        assert_eq!(content_animation(&jpeg), Some(false));
        assert!(decode_still(&jpeg).is_ok());
        let still = raster_cache_route(false, true, Some(false), false);
        assert!(!still.animated && still.cache_ready);
    }

    #[test]
    fn ordinary_raster_kind_must_be_known_before_any_moderation_cache() {
        let pending = raster_cache_route(false, true, None, false);
        assert_eq!(
            pending,
            RasterCacheRoute {
                probe: true,
                animated: false,
                cache_ready: false,
            }
        );
        assert_eq!(
            raster_cache_route(false, true, Some(true), false),
            RasterCacheRoute {
                probe: false,
                animated: true,
                cache_ready: true,
            }
        );
        assert_eq!(
            raster_cache_route(false, true, Some(false), false),
            RasterCacheRoute {
                probe: false,
                animated: false,
                cache_ready: true,
            }
        );
        assert!(!raster_cache_route(false, true, Some(true), true).cache_ready);
        assert!(raster_cache_route(true, false, None, false).animated);

        let mut generic = document_with_mime("application/octet-stream");
        if let Some(tl::enums::Document::Document(raw)) = generic.raw.document.as_mut() {
            raw.attributes.push(
                tl::types::DocumentAttributeFilename {
                    file_name: "clip.webp".into(),
                }
                .into(),
            );
        }
        let media = Media::Document(generic);
        assert!(ordinary_raster_document(&media).is_some());
        assert!(still_image_without_thumb(&media, false).is_some());

        for mime in ["", "application/pdf", "image/jpeg"] {
            let mut named = document_with_mime(mime);
            if let Some(tl::enums::Document::Document(raw)) = named.raw.document.as_mut() {
                raw.attributes.push(
                    tl::types::DocumentAttributeFilename {
                        file_name: "clip.png".into(),
                    }
                    .into(),
                );
            }
            let media = Media::Document(named);
            assert!(ordinary_raster_document(&media).is_some(), "{mime}");
            assert!(still_image_without_thumb(&media, false).is_some(), "{mime}");
        }
    }

    #[test]
    fn animated_image_documents_are_routed_to_frame_scanning() {
        let apng = Media::Document(document_with_mime("image/apng"));
        assert!(animated_document(&apng).is_some());
        assert!(still_image_without_thumb(&apng, false).is_none());

        for mime in [
            "image/tiff",
            "image/x-tiff",
            "image/avif",
            "image/avif-sequence",
            "image/x-icon",
            "image/vnd.microsoft.icon",
            "image/x-portable-anymap",
            "image/x-portable-bitmap",
            "image/x-portable-graymap",
            "image/x-portable-pixmap",
            "image/x-portable-arbitrarymap",
        ] {
            let original = Media::Document(document_with_mime(mime));
            assert!(animated_document(&original).is_some());
            assert!(still_image_without_thumb(&original, false).is_none());
            assert!(still_image_without_thumb(&original, true).is_none());
        }

        let mut named = document_with_mime("image/png");
        if let Some(tl::enums::Document::Document(raw)) = named.raw.document.as_mut() {
            raw.attributes.push(
                tl::types::DocumentAttributeFilename {
                    file_name: "clip.apng".into(),
                }
                .into(),
            );
        }
        let named = Media::Document(named);
        assert!(animated_document(&named).is_some());
        assert!(still_image_without_thumb(&named, false).is_none());

        let mut named_tiff = document_with_mime("application/octet-stream");
        if let Some(tl::enums::Document::Document(raw)) = named_tiff.raw.document.as_mut() {
            raw.attributes.push(
                tl::types::DocumentAttributeFilename {
                    file_name: "scan.tif".into(),
                }
                .into(),
            );
        }
        assert!(animated_document(&Media::Document(named_tiff)).is_some());

        let mut named_avif = document_with_mime("application/octet-stream");
        if let Some(tl::enums::Document::Document(raw)) = named_avif.raw.document.as_mut() {
            raw.attributes.push(
                tl::types::DocumentAttributeFilename {
                    file_name: "clip.avif".into(),
                }
                .into(),
            );
        }
        assert!(animated_document(&Media::Document(named_avif)).is_some());

        for name in [
            "icons.ico",
            "images.pnm",
            "images.pbm",
            "images.pgm",
            "images.ppm",
            "images.pam",
        ] {
            let mut document = document_with_mime("application/octet-stream");
            if let Some(tl::enums::Document::Document(raw)) = document.raw.document.as_mut() {
                raw.attributes.push(
                    tl::types::DocumentAttributeFilename {
                        file_name: name.into(),
                    }
                    .into(),
                );
            }
            assert!(
                animated_document(&Media::Document(document)).is_some(),
                "{name}"
            );
        }

        let mut webp = document_with_mime("image/webp");
        if let Some(tl::enums::Document::Document(raw)) = webp.raw.document.as_mut() {
            raw.attributes
                .push(tl::types::DocumentAttributeAnimated {}.into());
        }
        let webp = Media::Document(webp);
        assert!(animated_document(&webp).is_some());
        assert!(still_image_without_thumb(&webp, false).is_none());
    }

    #[tokio::test]
    async fn avif_auxiliary_alpha_never_scores_color_only() {
        let opaque = include_bytes!("../../tests/fixtures/media/opaque-animated.avif");
        let alpha_animation = include_bytes!("../../tests/fixtures/media/alpha-animated.avif");
        let alpha_still = include_bytes!("../../tests/fixtures/media/alpha-still.avif");
        assert!(!avif_has_unresolved_alpha(opaque));
        assert!(avif_has_unresolved_alpha(alpha_animation));
        assert!(avif_has_unresolved_alpha(alpha_still));
        assert!(
            decode_animated_frames(alpha_animation.to_vec())
                .await
                .is_none()
        );
        assert!(decode_animated_frames(alpha_still.to_vec()).await.is_none());
    }

    #[tokio::test]
    async fn opaque_avif_still_decodes_one_complete_frame() {
        let still = include_bytes!("../../tests/fixtures/media/opaque-still.avif");
        assert!(!avif_has_unresolved_alpha(still));
        let mut probe = serde_json::json!({"streams": [
            {"index": 0, "codec_name": "av1", "pix_fmt": "yuv420p", "width": 16,
             "height": 16, "nb_frames": "1", "nb_read_frames": "1"}
        ]});
        assert_eq!(
            opaque_avif_still(probe["streams"].as_array().unwrap()),
            Some((16, 16))
        );
        probe["streams"][0]["nb_read_frames"] = serde_json::json!("0");
        assert!(opaque_avif_still(probe["streams"].as_array().unwrap()).is_none());
        probe["streams"][0]["nb_read_frames"] = serde_json::json!("1");
        probe["streams"][0]["duration"] = serde_json::json!("0.1");
        assert!(opaque_avif_still(probe["streams"].as_array().unwrap()).is_none());

        let frames = decode_animated_frames(still.to_vec()).await.unwrap();
        assert_eq!(frames.len(), 1);
        let pixel = frames[0].get_pixel(8, 8);
        assert!(pixel[0] > 240 && pixel[1] < 20 && pixel[2] < 20);
    }

    #[tokio::test]
    async fn opaque_avif_sequence_requires_complete_poster_and_later_frames() {
        let opaque = include_bytes!("../../tests/fixtures/media/opaque-animated.avif");
        assert!(avif_ftyp_len(opaque).is_some());
        let mut probe = serde_json::json!({"streams": [
            {"index": 0, "codec_name": "av1", "pix_fmt": "yuv420p", "width": 16,
             "height": 16, "nb_frames": "1", "nb_read_frames": "1"},
            {"index": 1, "codec_name": "av1", "pix_fmt": "yuv420p", "width": 16,
             "height": 16, "duration": "0.200000", "nb_frames": "2", "nb_read_frames": "2"}
        ]});
        let streams = probe["streams"].as_array().unwrap();
        assert!(opaque_avif_poster_sequence(streams).is_some());
        probe["streams"][1]["nb_read_frames"] = serde_json::json!("1");
        assert!(opaque_avif_poster_sequence(probe["streams"].as_array().unwrap()).is_none());
        probe["streams"][1]["nb_read_frames"] = serde_json::json!("2");
        probe["streams"][1]["pix_fmt"] = serde_json::json!("gray");
        assert!(opaque_avif_poster_sequence(probe["streams"].as_array().unwrap()).is_none());
        probe["streams"][1]["pix_fmt"] = serde_json::json!("yuv420p");
        probe["streams"][1]["duration"] = serde_json::Value::Null;
        assert!(opaque_avif_poster_sequence(probe["streams"].as_array().unwrap()).is_none());

        let frames = decode_animated_frames(opaque.to_vec()).await.unwrap();
        assert!(frames.iter().any(|frame| frame.get_pixel(8, 8)[0] > 240));
        assert!(frames.iter().any(|frame| frame.get_pixel(8, 8)[2] > 240));
    }

    #[test]
    fn generic_video_name_without_thumbnail_reaches_bounded_animation_route() {
        for extension in ["mp4", "webm"] {
            let mut document = document_with_mime("application/octet-stream");
            if let Some(tl::enums::Document::Document(raw)) = document.raw.document.as_mut() {
                raw.attributes.push(
                    tl::types::DocumentAttributeFilename {
                        file_name: format!("clip.{extension}"),
                    }
                    .into(),
                );
            }
            let media = Media::Document(document);
            assert!(ladder(&media).is_none());
            assert!(animated_document(&media).is_some());
            assert!(still_image_without_thumb(&media, false).is_none());
        }
    }

    #[tokio::test]
    async fn generic_video_name_with_static_bytes_uses_bounded_original() {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([30, 90, 180]),
        ))
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
        for extension in ["mp4", "webm"] {
            let mut document = document_with_mime("application/octet-stream");
            if let Some(tl::enums::Document::Document(raw)) = document.raw.document.as_mut() {
                raw.attributes.push(
                    tl::types::DocumentAttributeFilename {
                        file_name: format!("clip.{extension}"),
                    }
                    .into(),
                );
            }
            let media = Media::Document(document);
            assert!(ladder(&media).is_none());
            assert!(animated_document(&media).is_some());
            assert!(ordinary_raster_document(&media).is_none());
            assert!(decode_animated_frames(png.clone()).await.is_none());
            let OriginalAnimatedBytes::Still(bytes) = route_original_animated_bytes(png.clone())
            else {
                panic!("a supported static original must reach still scoring");
            };
            assert_eq!(decode_still(&bytes).unwrap().dimensions(), (8, 8));
        }

        assert!(matches!(
            route_original_animated_bytes(b"not an image".to_vec()),
            OriginalAnimatedBytes::AnimatedOrUnknown(_)
        ));
        let mut oversized_jpeg = vec![0; THUMBNAIL_BYTES_MAX + 1];
        oversized_jpeg[..4].copy_from_slice(b"\xff\xd8\xff\xd9");
        assert!(matches!(
            route_original_animated_bytes(oversized_jpeg),
            OriginalAnimatedBytes::OversizedStill
        ));
    }

    #[test]
    fn a_static_sticker_can_be_read_whole() {
        let mut document = document_with_mime("image/webp");
        if let Some(grammers_client::tl::enums::Document::Document(raw)) =
            document.raw.document.as_mut()
        {
            raw.thumbs = Some(vec![
                grammers_client::tl::types::PhotoSize {
                    r#type: "m".to_owned(),
                    w: 128,
                    h: 128,
                    size: 5000,
                }
                .into(),
            ]);
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
        let sticker = grammers_client::media::Sticker::from_document(&document).unwrap();
        let media = Media::Sticker(sticker);
        assert!(visual_ladder(&media, false).is_some(), "the preview, by default");
        assert!(visual_ladder(&media, true).is_none());
        assert!(still_image_without_thumb(&media, false).is_some(), "the sticker itself");
        assert!(animated_document(&media).is_none());
    }

    #[test]
    fn video_stickers_use_the_bounded_animation_decoder() {
        let mut document = document_with_mime("video/webm");
        if let Some(grammers_client::tl::enums::Document::Document(raw)) =
            document.raw.document.as_mut()
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
        let sticker = grammers_client::media::Sticker::from_document(&document).unwrap();
        let media = Media::Sticker(sticker);
        assert!(ladder(&media).is_none());
        assert!(animated_document(&media).is_some());
        assert!(still_image_without_thumb(&media, false).is_none());
    }

    #[test]
    fn tgs_documents_and_stickers_reach_animation_even_without_raster_thumbnails() {
        let mut document = document_with_mime("application/x-tgsticker");
        let plain = Media::Document(document.clone());
        assert!(ladder(&plain).is_none());
        assert!(animated_document(&plain).is_some());
        assert_eq!(kind_of(&plain), "animated-sticker");
        assert!(still_image_without_thumb(&plain, false).is_none());

        if let Some(grammers_client::tl::enums::Document::Document(raw)) =
            document.raw.document.as_mut()
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
        let sticker = grammers_client::media::Sticker::from_document(&document).unwrap();
        let media = Media::Sticker(sticker);
        assert!(ladder(&media).is_none());
        assert!(animated_document(&media).is_some());
        assert!(still_image_without_thumb(&media, false).is_none());
    }

    #[test]
    fn animated_advert_reads_the_same_sample_with_or_without_visual_locks() {
        assert!(animation_required(false, false, true));
        assert!(animation_required(true, true, true));
        assert_eq!(animated_ocr_indices(12), vec![0, 3, 7, 11]);
        assert_eq!(animated_ocr_indices(3), vec![0, 1, 2]);
        assert_eq!(advert_rank(super::super::ocr::Decision::Allow), 0);
        assert_eq!(advert_rank(super::super::ocr::Decision::Block), 2);
    }

    #[test]
    fn animated_advert_checks_later_frames_after_an_ocr_error() {
        let frames = (0..3)
            .map(|_| Arc::new(image::RgbImage::new(2, 2)))
            .collect::<Vec<_>>();
        let ad = "SALE t.me/deals";
        assert_eq!(
            super::super::ocr::classify(ad).decision,
            super::super::ocr::Decision::Block
        );

        let mut reads = 0;
        let (verdict, _, frames_read) = scan_advert_frames_with(&frames, |_| {
            reads += 1;
            match reads {
                1 => Err(ModelError::runtime("first frame unreadable")),
                2 => Ok(ad.to_owned()),
                _ => panic!("confirmed ad stops the scan"),
            }
        })
        .expect("a later confirmed ad is actionable");
        assert_eq!(verdict.decision_name(), "block");
        assert_eq!(frames_read, 2);
        assert_eq!(reads, 2);

        let mut reads = 0;
        assert!(
            scan_advert_frames_with(&frames, |_| {
                reads += 1;
                if reads == 1 {
                    Err(ModelError::runtime("first frame unreadable"))
                } else {
                    Ok("ordinary photo".to_owned())
                }
            })
            .is_err()
        );
        assert_eq!(
            reads,
            frames.len(),
            "incomplete clean scans must still abstain"
        );
    }

    #[test]
    fn distinct_video_cover_is_one_extra_visual_and_ocr_frame() {
        let base = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30]));
        let poster = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 30, 20]));
        let mut visual = vec![base.clone(); MAX_ANIMATED_FRAMES];
        let mut ocr = animated_ocr_indices_for_frames(&visual)
            .into_iter()
            .map(|at| visual[at].clone())
            .collect::<Vec<_>>();
        add_distinct_cover(&mut visual, &poster);
        add_distinct_cover(&mut ocr, &poster);
        assert_eq!(visual.len(), MAX_ANIMATED_FRAMES + 1);
        assert_eq!(ocr.last(), Some(&poster));
        assert!(ocr.len() <= MAX_ANIMATED_OCR_FRAMES + MAX_ANIMATED_OCR_EXTRA_FRAMES + 1);

        add_distinct_cover(&mut visual, &poster);
        add_distinct_cover(&mut ocr, &poster);
        assert_eq!(visual.len(), MAX_ANIMATED_FRAMES + 1);
        assert_eq!(ocr.iter().filter(|frame| *frame == &poster).count(), 1);
    }

    #[test]
    fn animated_advert_adds_a_brief_distinct_frame_without_repeating_static_frames() {
        let blank = image::RgbImage::from_pixel(64, 64, image::Rgb([240, 240, 240]));
        let mut frames = vec![blank.clone(); 12];
        for y in 16..48 {
            for x in 16..48 {
                frames[5].put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
        }
        assert_eq!(animated_ocr_indices_for_frames(&frames), vec![0, 5]);

        for (index, frame) in frames.iter_mut().enumerate() {
            frame.put_pixel(index as u32, 0, image::Rgb([index as u8, 0, 0]));
        }
        let selected = animated_ocr_indices_for_frames(&frames);
        assert!(selected.contains(&5));
        for at in [0, 3, 7, 11] {
            assert!(selected.contains(&at));
        }
        assert!(selected.len() <= MAX_ANIMATED_OCR_FRAMES + MAX_ANIMATED_OCR_EXTRA_FRAMES);
    }

    #[tokio::test]
    async fn gif_flash_ad_is_read_even_when_it_misses_the_four_fixed_positions() {
        let bytes = include_bytes!("../../tests/fixtures/media/flash-ad-frame5.gif");
        let frames = decode_animated_frames(bytes.to_vec())
            .await
            .expect("decode the twelve-frame GIF");
        assert_eq!(frames.len(), 12);
        let selected = animated_ocr_indices_for_frames(&frames);
        assert!(!animated_ocr_indices(frames.len()).contains(&5));
        assert!(selected.contains(&5), "brief ad frame must reach OCR");
        let selected = selected
            .into_iter()
            .map(|at| Arc::new(frames[at].clone()))
            .collect::<Vec<_>>();
        let (verdict, _, frames_read) =
            scan_advert_frames(&selected).expect("OCR every chosen frame");
        assert_eq!(verdict.decision_name(), "block");
        assert!(frames_read <= MAX_ANIMATED_OCR_FRAMES + MAX_ANIMATED_OCR_EXTRA_FRAMES);
    }

    #[test]
    fn still_advert_keeps_its_starting_image_when_visual_analysis_escalates() {
        let base = Arc::new(image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30])));
        let ocr = pin_still_ocr_source(&base, true, false).unwrap();
        let mut visual = Arc::clone(&base);
        assert!(Arc::ptr_eq(&ocr, &visual));
        visual = Arc::new(image::RgbImage::from_pixel(2, 2, image::Rgb([90, 80, 70])));
        assert_eq!(ocr.get_pixel(0, 0).0, [10, 20, 30]);
        assert_eq!(visual.get_pixel(0, 0).0, [90, 80, 70]);
        assert!(pin_still_ocr_source(&base, true, true).is_none());
        assert!(pin_still_ocr_source(&base, false, false).is_none());
    }

    #[test]
    fn animation_frame_cannot_be_replaced_by_an_unrelated_cover_rung() {
        assert!(use_sharper_rung(true, false, true));
        assert!(!use_sharper_rung(true, true, true));
        assert!(!use_sharper_rung(true, false, false));
    }

    #[tokio::test]
    async fn timed_out_ocr_worker_keeps_capacity_until_native_work_finishes() {
        let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = std::sync::Arc::clone(&slots)
            .acquire_owned()
            .await
            .expect("test semaphore is open");
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marked = std::sync::Arc::clone(&finished);
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            std::thread::sleep(std::time::Duration::from_millis(40));
            marked.store(true, std::sync::atomic::Ordering::Release);
        });
        let mut worker = worker;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), &mut worker)
                .await
                .is_err()
        );
        assert!(
            slots.try_acquire().is_err(),
            "timeout must not release a permit while native OCR still runs"
        );
        worker.await.expect("blocking worker completes");
        assert!(finished.load(std::sync::atomic::Ordering::Acquire));
        assert!(slots.try_acquire().is_ok());
    }

    #[test]
    fn still_decoder_rejects_oversized_dimensions_before_rgb_allocation() {
        let image =
            image::DynamicImage::ImageRgb8(image::RgbImage::new(super::IMAGE_DIMENSION_MAX + 1, 1));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Png)
            .expect("encode bounded test fixture");
        assert!(super::decode_still(encoded.get_ref()).is_err());

        let image = image::DynamicImage::ImageLuma8(image::GrayImage::new(4_000, 4_001));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Png)
            .expect("encode pixel-overflow test fixture");
        assert!(super::decode_still(encoded.get_ref()).is_err());

        let image = image::DynamicImage::ImageLuma8(image::GrayImage::new(4_624, 1_193));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Png)
            .expect("encode wide camera test fixture");
        assert_eq!(
            super::decode_still(encoded.get_ref()).unwrap().dimensions(),
            (4_624, 1_193)
        );

        let tiny = image::DynamicImage::ImageRgb8(image::RgbImage::new(1, 1));
        let mut encoded = std::io::Cursor::new(Vec::new());
        tiny.write_to(&mut encoded, image::ImageFormat::Png)
            .expect("encode tiny test fixture");
        assert_eq!(
            super::decode_still(encoded.get_ref()).unwrap().dimensions(),
            (1, 1)
        );
    }

    #[test]
    fn still_decoder_applies_jpeg_exif_orientation() {
        let mut pixels = image::RgbImage::new(3, 2);
        pixels.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        pixels.put_pixel(2, 0, image::Rgb([0, 255, 0]));
        pixels.put_pixel(0, 1, image::Rgb([0, 0, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Jpeg)
            .expect("encode JPEG fixture");

        let exif = [
            b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1,
            0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let jpeg = encoded.into_inner();
        let mut tagged = jpeg[..2].to_vec();
        tagged.extend_from_slice(&[0xff, 0xe1, 0, 34]);
        tagged.extend_from_slice(&exif);
        tagged.extend_from_slice(&jpeg[2..]);

        let untagged = super::decode_still(&jpeg).expect("decode plain JPEG");
        let oriented = super::decode_still(&tagged).expect("decode EXIF JPEG");
        assert_eq!(oriented, image::imageops::rotate90(&untagged));
    }

    #[test]
    fn still_decoder_ignores_hidden_rgb_and_blends_visible_alpha() {
        let mut pixels = image::RgbaImage::new(3, 1);
        pixels.put_pixel(0, 0, image::Rgba([255, 0, 0, 0]));
        pixels.put_pixel(1, 0, image::Rgba([10, 20, 30, 128]));
        pixels.put_pixel(2, 0, image::Rgba([7, 8, 9, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .expect("encode RGBA PNG");

        let decoded = super::decode_still(encoded.get_ref()).expect("decode RGBA PNG");
        assert_eq!(decoded.get_pixel(0, 0).0, [128, 128, 128]);
        assert_eq!(decoded.get_pixel(1, 0).0, [69, 74, 79]);
        assert_eq!(decoded.get_pixel(2, 0).0, [7, 8, 9]);
    }

    #[test]
    fn native_gif_decoder_ignores_transparent_pixels() {
        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            encoder
                .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                    8,
                    8,
                    image::Rgba([255, 0, 0, 0]),
                )))
                .expect("encode transparent GIF frame");
        }
        let frames = super::decode_gif_frames(&gif).expect("decode GIF frames");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].get_pixel(4, 4).0, [128, 128, 128]);
    }

    #[test]
    fn operator_switches_and_device_ids_are_strict() {
        assert!(parse_bool("ORT_USE_CUDA", " TRUE ").unwrap());
        assert!(!parse_bool("ORT_USE_CUDA", "0").unwrap());
        assert!(parse_bool("ORT_USE_CUDA", "enabled").is_err());
        assert_eq!(parse_bounded_i32("ORT_CUDA_DEVICE", "3", 0, 8).unwrap(), 3);
        assert!(parse_bounded_i32("ORT_CUDA_DEVICE", "-1", 0, 8).is_err());
        assert!(parse_bounded_i32("ORT_CUDA_DEVICE", "gpu0", 0, 8).is_err());
    }

    #[test]
    fn concurrent_runtime_publishers_converge_on_verified_bytes() {
        let unique = RUNTIME_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "groupbot-onnx-publish-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("unique runtime test directory is creatable");
        let target = directory.join("runtime.so");
        let bytes = b"verified runtime fixture";
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let target = target.clone();
                std::thread::spawn(move || publish_embedded(&target, bytes))
            })
            .collect();
        for thread in threads {
            thread
                .join()
                .expect("runtime publisher does not panic")
                .expect("concurrent publisher accepts the verified winner");
        }
        assert!(file_matches(&target, bytes));
        std::fs::remove_file(&target).expect("runtime test artifact is removable");
        std::fs::remove_dir(&directory).expect("empty runtime test directory is removable");
    }

    #[test]
    fn rolling_runtime_versions_never_share_a_publication_target() {
        let first = embedded_runtime_filename(b"runtime build one");
        let same = embedded_runtime_filename(b"runtime build one");
        let second = embedded_runtime_filename(b"runtime build two");
        assert_eq!(first, same);
        assert_ne!(first, second);
        assert!(first.starts_with("libonnxruntime.so.1.20.1-"));
    }

    #[test]
    fn embedded_runtime_matches_reviewed_digest() {
        assert_eq!(
            format!("{:x}", Sha256::digest(RUNTIME)),
            REVIEWED_RUNTIME_SHA256
        );
    }

    #[test]
    fn animated_sampling_spans_the_whole_document() {
        assert_eq!(sample_indices(0, MAX_ANIMATED_FRAMES), Vec::<usize>::new());
        assert_eq!(sample_indices(4, MAX_ANIMATED_FRAMES), vec![0, 1, 2, 3]);
        assert_eq!(sample_indices(100, 4), vec![0, 33, 66, 99]);
        assert_eq!(sample_indices(100, 1), vec![0]);
    }

    #[test]
    fn an_absent_general_model_never_becomes_cached_sentinel_margins() {
        let frame = image::RgbImage::from_fn(SIDE as u32, SIDE as u32, |x, y| {
            image::Rgb([((x + y) % 255) as u8, (x % 255) as u8, (y % 255) as u8])
        });
        match super::super::vision::embed_of(&frame) {
            Ok(Some(_)) => return,
            Err(expected) => {
                match score_animated_frames(vec![frame], true, true, true, Vec::new()) {
                    Err(observed) => assert_eq!(observed, expected),
                    Ok(Some(selection)) => {
                        assert!(selection.general_failed);
                        assert!(!selection.nsfw_failed);
                        assert!(selection.judgement.is_some());
                        assert!(selection.margins.is_none());
                    }
                    Ok(None) => panic!("a broken model cannot count as an absent model"),
                }
                return;
            }
            Ok(None) => {}
        }
        let selection = score_animated_frames(vec![frame], true, true, true, Vec::new())
            .expect("the bundled classifier runs")
            .expect("the NSFW frame still provides a selection");
        assert!(selection.general_failed);
        assert!(!selection.nsfw_failed);
        assert!(selection.judgement.is_some());
        assert!(selection.margins.is_none());
        assert!(selection.custom_margins.is_none());
    }

    #[tokio::test]
    async fn native_apng_and_webp_decoders_reach_later_frames() {
        for bytes in [
            include_bytes!("../../tests/fixtures/media/two-frame.apng").as_slice(),
            include_bytes!("../../tests/fixtures/media/two-frame.webp").as_slice(),
        ] {
            let frames = decode_animated_frames(bytes.to_vec())
                .await
                .expect("animated image decodes");
            assert_eq!(frames.len(), 2);
            assert_eq!(animated_ocr_indices_for_frames(&frames), vec![0, 1]);
            let first = frames[0].get_pixel(48, 48);
            let second = frames[1].get_pixel(48, 48);
            assert!(first[0] > first[1], "first frame is red");
            assert!(second[1] > second[0], "second frame is green");
        }
    }

    #[tokio::test]
    async fn malformed_animated_rasters_never_fall_through_to_a_first_frame() {
        let mut webp = include_bytes!("../../tests/fixtures/media/two-frame.webp").to_vec();
        webp[4] ^= 1;
        assert_eq!(raster_kind_full(&webp), None);
        assert!(decode_animated_frames(webp).await.is_none());

        let mut apng = include_bytes!("../../tests/fixtures/media/two-frame.apng").to_vec();
        let actl = apng.windows(4).position(|part| part == b"acTL").unwrap();
        apng[actl + 12] ^= 1;
        assert_eq!(raster_kind_full(&apng), None);
        assert!(decode_animated_frames(apng).await.is_none());
    }

    #[tokio::test]
    async fn gif_missing_its_trailer_cannot_be_cleared_from_early_frames() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for value in [20, 220] {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                        8,
                        8,
                        image::Rgba([value, 0, 0, 255]),
                    )))
                    .unwrap();
            }
        }
        assert_eq!(bytes.last(), Some(&0x3b));
        assert_eq!(
            decode_animated_frames(bytes.clone()).await.unwrap().len(),
            2
        );
        bytes.pop();
        assert!(decode_animated_frames(bytes).await.is_none());
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
        let frames = decode_gif_frames(&bytes).expect("GIF decodes");
        assert_eq!(frames.len(), MAX_ANIMATED_FRAMES);
        assert_eq!(frames.first().unwrap().get_pixel(0, 0)[0], 0);
        assert_eq!(
            frames.last().unwrap().get_pixel(0, 0)[0],
            MAX_ANIMATED_FRAMES as u8
        );
    }

    #[test]
    fn native_gif_sampler_keeps_a_visual_flash_between_uniform_positions() {
        let total = 23;
        let flash_at = 11;
        assert!(!sample_indices(total, MAX_ANIMATED_FRAMES).contains(&flash_at));
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for at in 0..total {
                let color = if at == flash_at {
                    image::Rgba([255, 0, 0, 255])
                } else {
                    image::Rgba([0, 0, 0, 255])
                };
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(8, 8, color)))
                    .expect("GIF frame encodes");
            }
        }
        let frames = decode_gif_frames(&bytes).expect("GIF decodes");
        assert_eq!(frames.len(), MAX_ANIMATED_FRAMES);
        assert!(frames.iter().any(|frame| frame.get_pixel(0, 0)[0] == 255));
        assert_eq!(frames.first().unwrap().get_pixel(0, 0)[0], 0);
        assert_eq!(frames.last().unwrap().get_pixel(0, 0)[0], 0);
    }

    #[tokio::test]
    async fn native_gif_rejects_an_unsafe_canvas_before_frame_decode() {
        assert!(native_gif_allocation(MAX_ANIMATED_SIDE, MAX_ANIMATED_SIDE).is_some());
        assert!(native_gif_allocation(MAX_ANIMATED_SIDE + 1, 1).is_none());
        assert!(native_gif_allocation(1, MAX_ANIMATED_SIDE + 1).is_none());
        assert!(native_gif_allocation(0, 1).is_none());

        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                MAX_ANIMATED_SIDE + 1,
                1,
                image::Rgba([0, 0, 0, 255]),
            )))
            .expect("oversized GIF still encodes");
        assert!(
            decode_gif_frames(&bytes).is_none(),
            "an unsafe native canvas must signal the bounded ffmpeg fallback"
        );
        let frames = decode_animated_frames(bytes)
            .await
            .expect("original bytes survive native rejection for the bounded FFmpeg fallback");
        assert!(!frames.is_empty());
        assert!(
            frames
                .iter()
                .all(|frame| frame.width() <= MAX_ANIMATED_SIDE)
        );
    }

    #[test]
    fn native_gif_frame_discovery_stops_at_its_limit() {
        let within = (0..MAX_NATIVE_GIF_SCAN_FRAMES).map(|_| Ok::<(), ()>(()));
        assert_eq!(
            bounded_frame_count(within, MAX_NATIVE_GIF_SCAN_FRAMES),
            Some(MAX_NATIVE_GIF_SCAN_FRAMES)
        );
        let beyond = (0..=MAX_NATIVE_GIF_SCAN_FRAMES).map(|_| Ok::<(), ()>(()));
        assert_eq!(
            bounded_frame_count(beyond, MAX_NATIVE_GIF_SCAN_FRAMES),
            None,
            "a huge animation must signal the bounded ffmpeg fallback"
        );
        assert_eq!(
            bounded_frame_count([Ok::<(), ()>(()), Err(())], MAX_NATIVE_GIF_SCAN_FRAMES),
            None,
            "a malformed frame must fail open through the fallback"
        );

        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for value in 0..=MAX_NATIVE_GIF_SCAN_FRAMES {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                        1,
                        1,
                        image::Rgba([value as u8, 0, 0, 255]),
                    )))
                    .expect("GIF frame encodes");
            }
        }
        assert!(
            decode_gif_frames(&bytes).is_none(),
            "an actual over-limit GIF must signal the bounded ffmpeg fallback"
        );
    }

    #[tokio::test]
    async fn animation_scratch_is_atomically_private() {
        let path = write_animated_scratch(b"animation fixture")
            .await
            .expect("private animation scratch file");
        assert_eq!(
            tokio::fs::read(&path).await.expect("scratch contents"),
            b"animation fixture"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("animation scratch metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "animation spill must exclude group/other");
            assert_eq!(
                mode & 0o600,
                0o600,
                "animation owner needs read/write access"
            );
        }
        tokio::fs::remove_file(path)
            .await
            .expect("remove animation scratch fixture");
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
        let started = std::time::Instant::now();
        let frames = decode_video_frames(output.stdout)
            .await
            .expect("MP4 animation decodes");
        eprintln!(
            "fragmented MP4 decode: {} frames in {:?}",
            frames.len(),
            started.elapsed()
        );
        assert!(!frames.is_empty());
        assert_eq!(frames[0].dimensions(), (16, 16));
    }

    #[test]
    fn video_signature_coverage_rejects_an_early_only_decode() {
        assert!(video_signature_scan_complete(60, 1.0, 60.0));
        assert!(video_signature_scan_complete(58, 1.0, 60.0));
        assert!(!video_signature_scan_complete(3, 1.0, 60.0));
        assert!(!video_signature_scan_complete(0, 1.0, 60.0));
        assert!(!video_signature_scan_complete(60, f64::INFINITY, 60.0));
    }

    #[test]
    fn video_alpha_route_preserves_proven_opaque_formats() {
        let opaque = serde_json::json!({"pix_fmt": "yuv420p", "codec_name": "h264"});
        assert!(!video_needs_alpha_composite(&opaque));
        assert_eq!(video_alpha_composite_filter(&opaque), None);
        let declared = serde_json::json!({"pix_fmt": "yuv420p", "codec_name": "vp9",
                                          "tags": {"alpha_mode": "1"}});
        assert!(video_declares_alpha(&declared));
        assert!(video_needs_alpha_composite(&declared));
        assert_eq!(
            video_alpha_composite_filter(&declared),
            Some(VIDEO_ALPHA_COMPOSITE)
        );
        let palette = serde_json::json!({"pix_fmt": "pal8"});
        assert!(video_needs_alpha_composite(&palette));
        assert!(video_needs_alpha_composite(&serde_json::json!({})));
    }

    #[test]
    fn video_alpha_route_requires_explicit_premultiplied_metadata() {
        let premultiplied = serde_json::json!({"pix_fmt": "bgra",
                                               "tags": {"ALPHA_MODE": "premultiplied"}});
        assert_eq!(
            video_alpha_composite_filter(&premultiplied),
            Some(VIDEO_PREMULTIPLIED_ALPHA_COMPOSITE)
        );
        let straight = serde_json::json!({"pix_fmt": "bgra",
                                          "tags": {"alpha_mode": "straight"}});
        let unknown = serde_json::json!({"pix_fmt": "bgra"});
        assert_eq!(
            video_alpha_composite_filter(&straight),
            Some(VIDEO_ALPHA_COMPOSITE)
        );
        assert_eq!(
            video_alpha_composite_filter(&unknown),
            Some(VIDEO_ALPHA_COMPOSITE)
        );
    }

    #[tokio::test]
    async fn video_premultiplied_alpha_matches_straight_reference() {
        let expected = [[128, 128, 128], [164, 114, 89], [200, 100, 50]];
        for (alpha_mode, pixels) in [
            (
                "premultiplied",
                [[0, 0, 0, 0], [100, 50, 25, 128], [200, 100, 50, 255]],
            ),
            (
                "straight",
                [[200, 100, 50, 0], [200, 100, 50, 128], [200, 100, 50, 255]],
            ),
        ] {
            let raw_path = animated_scratch_path();
            let mkv_path = animated_scratch_path();
            let mut raw = Vec::with_capacity(2 * 3 * 4);
            for _ in 0..2 {
                for pixel in pixels {
                    raw.extend_from_slice(&pixel);
                }
            }
            tokio::fs::write(&raw_path, raw)
                .await
                .expect("raw FFV1 frames");
            let output = Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "rawvideo",
                    "-pixel_format",
                    "rgba",
                    "-video_size",
                    "3x1",
                    "-framerate",
                    "2",
                    "-i",
                    raw_path.to_str().unwrap(),
                    "-frames:v",
                    "2",
                    "-c:v",
                    "ffv1",
                    "-pix_fmt",
                    "bgra",
                    "-metadata:s:v:0",
                    &format!("alpha_mode={alpha_mode}"),
                    "-f",
                    "matroska",
                    mkv_path.to_str().unwrap(),
                ])
                .output()
                .await
                .expect("ffmpeg is installed for animated media");
            assert!(
                output.status.success(),
                "FFV1 {alpha_mode} fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let movie = tokio::fs::read(&mkv_path).await.expect("FFV1 movie");
            let _ = tokio::fs::remove_file(&raw_path).await;
            let _ = tokio::fs::remove_file(&mkv_path).await;
            let frames = decode_video_frames(movie).await.expect("FFV1 alpha frames");
            assert!(!frames.is_empty(), "{alpha_mode}");
            for frame in frames {
                assert_eq!(frame.dimensions(), (3, 1), "{alpha_mode}");
                for (x, wanted) in expected.into_iter().enumerate() {
                    assert_eq!(frame.get_pixel(x as u32, 0).0, wanted, "{alpha_mode} x={x}");
                }
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg with qtrle to encode the alpha MOV fixtures"]
    async fn video_alpha_scan_ignores_hidden_flash_and_keeps_visible_flash() {
        for (alpha, expected, visible) in [
            (0u8, [128, 128, 128], false),
            (128, [192, 64, 64], true),
            (255, [255, 0, 0], true),
        ] {
            let raw_path = animated_scratch_path();
            let mov_path = animated_scratch_path();
            let mut raw = Vec::with_capacity(23 * 4 * 4 * 4);
            for at in 0..23 {
                let pixel = if at == 11 {
                    [alpha, 255, 0, 0]
                } else {
                    [255, 128, 128, 128]
                };
                for _ in 0..16 {
                    raw.extend_from_slice(&pixel);
                }
            }
            tokio::fs::write(&raw_path, raw)
                .await
                .expect("raw frame data");
            let output = Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "rawvideo",
                    "-pixel_format",
                    "argb",
                    "-video_size",
                    "4x4",
                    "-framerate",
                    "23",
                    "-i",
                    raw_path.to_str().unwrap(),
                    "-frames:v",
                    "23",
                    "-an",
                    "-c:v",
                    "qtrle",
                    "-pix_fmt",
                    "argb",
                    "-f",
                    "mov",
                    mov_path.to_str().unwrap(),
                ])
                .output()
                .await
                .expect("ffmpeg is installed for animated media");
            assert!(
                output.status.success(),
                "qtrle fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let movie = tokio::fs::read(&mov_path).await.expect("qtrle movie");
            let _ = tokio::fs::remove_file(&raw_path).await;
            let _ = tokio::fs::remove_file(&mov_path).await;
            let frames = decode_video_frames(movie).await.expect("qtrle frame scan");
            assert_eq!(frames.len(), MAX_ANIMATED_FRAMES);
            assert!(frames.iter().all(|frame| frame.dimensions() == (4, 4)));
            assert!(
                frames
                    .iter()
                    .any(|frame| frame.get_pixel(2, 2).0 == expected),
                "visible frame 11 must be sampled when its alpha is nonzero"
            );
            if !visible {
                assert!(
                    frames
                        .iter()
                        .all(|frame| frame.get_pixel(2, 2).0 == [128, 128, 128]),
                    "hidden frame RGB must never reach vision"
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg with libvpx-vp9 to encode the alpha WebM fixture"]
    async fn vp9_alpha_metadata_uses_decoder_that_preserves_alpha() {
        let raw_path = animated_scratch_path();
        let webm_path = animated_scratch_path();
        let mut raw = Vec::with_capacity(2 * 4 * 4 * 4);
        for pixel in [[0, 255, 0, 0], [255, 0, 0, 255]] {
            for _ in 0..16 {
                raw.extend_from_slice(&pixel);
            }
        }
        tokio::fs::write(&raw_path, raw)
            .await
            .expect("raw alpha frames");
        let encoded = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pixel_format",
                "argb",
                "-video_size",
                "4x4",
                "-framerate",
                "2",
                "-i",
                raw_path.to_str().unwrap(),
                "-frames:v",
                "2",
                "-an",
                "-c:v",
                "libvpx-vp9",
                "-lossless",
                "1",
                "-pix_fmt",
                "yuva420p",
                "-f",
                "webm",
                webm_path.to_str().unwrap(),
            ])
            .output()
            .await
            .expect("ffmpeg is installed for animated media");
        assert!(
            encoded.status.success(),
            "VP9 alpha fixture: {}",
            String::from_utf8_lossy(&encoded.stderr)
        );
        let movie = tokio::fs::read(&webm_path).await.expect("alpha WebM");
        let _ = tokio::fs::remove_file(&raw_path).await;
        let _ = tokio::fs::remove_file(&webm_path).await;
        let frames = decode_video_frames(movie).await.expect("alpha WebM frames");
        assert!(
            frames
                .iter()
                .any(|frame| frame.get_pixel(2, 2).0 == [128, 128, 128])
        );
        assert!(frames.iter().any(|frame| {
            let [r, g, b] = frame.get_pixel(2, 2).0;
            b > 180 && r < 70 && g < 70
        }));
    }

    #[tokio::test]
    async fn truncated_video_cannot_be_cleared_from_its_first_frame() {
        for bytes in [
            include_bytes!("../../tests/fixtures/media/partial-after-frame0.mp4").as_slice(),
            include_bytes!("../../tests/fixtures/media/partial-after-frame0.webm").as_slice(),
        ] {
            assert!(decode_video_frames(bytes.to_vec()).await.is_none());
        }
        for bytes in [
            include_bytes!("../../tests/fixtures/media/flash-ad-frame11-faststart.mp4")
                .as_slice(),
            include_bytes!("../../tests/fixtures/media/flash-ad-frame11.webm").as_slice(),
            include_bytes!("../../tests/fixtures/media/valid-one-frame.mp4").as_slice(),
            include_bytes!("../../tests/fixtures/media/valid-one-frame.webm").as_slice(),
        ] {
            assert!(decode_video_frames(bytes.to_vec()).await.is_some());
        }
    }

    #[tokio::test]
    #[ignore = "requires ffmpeg with libx264 and libvpx-vp9; run explicitly for the video sampling regression"]
    async fn video_sampler_keeps_a_one_frame_flash_between_uniform_positions() {
        assert!(!sample_indices(23, MAX_ANIMATED_FRAMES).contains(&11));
        for (format, codec) in [("mp4", "libx264"), ("webm", "libvpx-vp9")] {
            let path = animated_scratch_path();
            let mut args = vec![
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=64x64:r=23:d=1",
                "-vf",
                "drawbox=x=0:y=0:w=iw:h=ih:color=red:t=fill:enable='eq(n,11)'",
                "-frames:v",
                "23",
                "-an",
                "-c:v",
                codec,
                "-pix_fmt",
                "yuv420p",
            ];
            args.extend(["-f", format, path.to_str().unwrap()]);
            let output = Command::new("ffmpeg")
                .args(args)
                .output()
                .await
                .expect("ffmpeg is installed for this ignored regression");
            assert!(
                output.status.success(),
                "{format} encoding failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let bytes = tokio::fs::read(&path).await.expect("encoded video exists");
            tokio::fs::remove_file(path)
                .await
                .expect("generated video is removable");
            let started = std::time::Instant::now();
            let frames = decode_video_frames(bytes)
                .await
                .unwrap_or_else(|| panic!("the generated {format} decodes"));
            eprintln!("{format} flash decode: {:?}", started.elapsed());
            assert_eq!(frames.len(), MAX_ANIMATED_FRAMES, "{format}");
            assert!(
                frames.iter().any(|frame| {
                    let pixel = frame.get_pixel(32, 32).0;
                    pixel[0] > 150 && pixel[1] < 80 && pixel[2] < 80
                }),
                "the one red {format} frame must survive the twelve-frame selection"
            );
        }
    }

    #[tokio::test]
    async fn mp4_with_a_trailing_moov_atom_still_decodes() {
        let path =
            std::env::temp_dir().join(format!("groupbot-test-moov-{}.mp4", std::process::id()));
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
        let started = std::time::Instant::now();
        let frames = decode_video_frames(bytes)
            .await
            .expect("a trailing-moov MP4 decodes");
        eprintln!(
            "trailing-moov MP4 decode: {} frames in {:?}",
            frames.len(),
            started.elapsed()
        );
        assert!(!frames.is_empty());
        assert_eq!(frames[0].dimensions(), (16, 16));
    }

    fn arbiter(explicit: f32) -> Option<Arbiter> {
        Some(Arbiter {
            explicit,
            revealing: 0.0,
            head: HEAD_VETO,
            stack: 1.0,
        })
    }

    fn head(head: f32) -> Option<Arbiter> {
        Some(Arbiter {
            explicit: 0.0,
            revealing: 0.0,
            head,
            stack: 1.0,
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
                    let without =
                        allow_delete(innocent, true, spare, weak, confirmed, arbiter(explicit));
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
                            stack: 1.0,
                        }),
                    );
                    assert_eq!(
                        with, without,
                        "innocent {innocent} weak {weak} confirmed {confirmed} margin {explicit} head {head}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_fitted_veto_only_withholds() {
        for (innocent, weak, confirmed, explicit, head) in [
            (false, false, true, 0.065, 0.5),
            (false, false, true, RECOVER, HEAD_SURE),
            (true, true, false, RECOVER, 1.0),
            (false, false, true, 0.005, 0.5),
            (false, false, false, AGREE, 0.3),
        ] {
            for spare in [false, true] {
                let with = |stack: f32| {
                    allow_delete(
                        innocent,
                        true,
                        spare,
                        weak,
                        confirmed,
                        Some(Arbiter {
                            explicit,
                            revealing: 0.0,
                            head,
                            stack,
                        }),
                    )
                };
                assert!(!with(STACK_VETO - 0.01), "under the veto nothing is deleted");
                assert_eq!(with(STACK_VETO), with(1.0), "at the veto it is silent");
            }
        }
        assert_eq!(arbiter_of(&[0.0; super::super::vision::DIM]).stack, 1.0);
    }

    #[test]
    fn the_fitted_veto_is_the_trainers() {
        use super::super::nsfw_stack_weights::{MEASURED_VETO, PIN_FEATURES, PIN_PROBABILITY};
        assert!((stack_of(&PIN_FEATURES) - PIN_PROBABILITY).abs() < 1e-4);
        assert!((MEASURED_VETO - STACK_VETO).abs() < 1e-6, "the constant is the one measured");
        let grade = Grade {
            hard: 0.5,
            sexy: 0.1,
            neutral: 0.2,
            drawn: 0.7,
            classes: String::new(),
        };
        let arbiter = Arbiter {
            explicit: 0.03,
            revealing: 0.0,
            head: 0.6,
            stack: 1.0,
        };
        let features = stack_features(0.8, 0.9, true, Some(&grade), &arbiter);
        let logit = (0.6f32 / 0.4).ln();
        assert!((features[0] - logit).abs() < 1e-6);
        for (got, want) in features[1..8]
            .iter()
            .zip([0.8, 0.9, 0.5, 0.2, 0.7, 0.6, 1.0])
        {
            assert!((got - want).abs() < 1e-6, "{got} != {want}");
        }
        assert!((features[8] - logit * 0.7).abs() < 1e-6);
        assert!((features[11] - 0.8 * 0.5).abs() < 1e-6);
        let none = stack_features(0.05, 0.12, false, None, &arbiter);
        assert_eq!(none[3..6], [0.0, 0.0, 0.0], "no grade reads as nothing graded");
        let certain = Arbiter {
            head: 1.0,
            ..arbiter
        };
        assert!(stack_features(0.8, 0.9, false, None, &certain)[0].is_finite());
    }

    #[test]
    fn a_head_that_reads_ordinary_withholds() {
        let with_head = |head: f32| {
            Some(Arbiter {
                explicit: 0.065,
                revealing: 0.0,
                head,
                stack: 1.0,
            })
        };
        assert!(
            !allow_delete(false, true, false, false, true, with_head(HEAD_VETO - 0.01)),
            "a confident first model and a sure prompt margin are still vetoed"
        );
        assert!(allow_delete(
            false,
            true,
            false,
            false,
            true,
            with_head(HEAD_VETO)
        ));
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
        for (innocent, weak, confirmed) in [
            (true, false, true),
            (false, true, true),
            (false, false, false),
        ] {
            assert!(allow_delete(
                innocent,
                true,
                false,
                weak,
                confirmed,
                head(HEAD_SURE)
            ));
        }
        assert!(!allow_delete(false, false, true, false, true, head(1.0)));
    }

    #[test]
    fn the_head_thresholds_are_ordered_and_strict() {
        const {
            assert!(
                HEAD_VETO < 0.41,
                "the reverted head's hentai gif must not be vetoed again"
            )
        };
        const { assert!(HEAD_VETO < HEAD_SURE, "the silent band exists") };
        const {
            assert!(
                HEAD_SURE >= 0.5,
                "sure is at least a coin toss in the head's own terms"
            )
        };
        const {
            assert!(
                HEAD_DELETE >= HEAD_SURE,
                "deleting alone is the stricter act"
            )
        };
        const {
            assert!(
                HEAD_DELETE >= 0.90,
                "the head acts alone only when it is nearly certain"
            )
        };
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

    fn observed_look(score: f32, peak: f32, complete: bool) -> Look {
        Look {
            score,
            peak,
            view_scores: [Some(score), Some(peak), None],
            frail: false,
            note: "injected observation".to_owned(),
            complete,
        }
    }

    fn observed_grade() -> Grade {
        Grade {
            hard: 0.80,
            sexy: 0.05,
            neutral: 0.05,
            drawn: 0.0,
            classes: "injected grade".to_owned(),
        }
    }

    #[test]
    fn still_crop_probe_has_a_bounded_gate() {
        let mut look = observed_look(0.20, 0.91, true);
        look.view_scores = [Some(0.20), Some(0.75), Some(0.91)];
        let mut full = Judgement {
            score: 0.20,
            innocent: false,
            explicit: true,
            confirmed: false,
            weak: false,
            arbiter: arbiter(0.0),
            spare_frame: None,
        };
        assert_eq!(still_crop_candidate(&look, full), Some((2, 0.91)));
        look.complete = false;
        assert_eq!(still_crop_candidate(&look, full), None);
        look.complete = true;
        look.view_scores = [Some(0.20), Some(0.69), None];
        assert_eq!(still_crop_candidate(&look, full), None);
        look.view_scores = [Some(0.91), Some(0.90), Some(0.80)];
        assert_eq!(still_crop_candidate(&look, full), None);
        look.view_scores = [Some(0.20), Some(0.91), None];
        full.arbiter = None;
        assert_eq!(still_crop_candidate(&look, full), None);
        full.arbiter = arbiter(RECOVER);
        assert_eq!(still_crop_candidate(&look, full), None);
    }

    #[test]
    fn still_crop_probe_keeps_primary_grader_and_arbiter_on_one_end() {
        let image = image::RgbImage::from_fn(900, 450, |x, _| {
            if x >= 450 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 255, 0])
            }
        });
        let see_right = |crop: &image::RgbImage| {
            assert_eq!(crop.dimensions(), (450, 450));
            assert_eq!(*crop.get_pixel(0, 0), image::Rgb([255, 0, 0]));
            assert_eq!(*crop.get_pixel(449, 449), image::Rgb([255, 0, 0]));
        };
        let probe = probe_still_crop_with(
            &image,
            2,
            0.91,
            |crop| {
                see_right(crop);
                Ok(Some(observed_grade()))
            },
            |crop| {
                see_right(crop);
                Ok(arbiter(RECOVER))
            },
        )
        .expect("same-crop witnesses");
        let judged = probe.judged.expect("the same crop is corroborated");
        assert_eq!(judged.score, 0.91);
        assert!(judged.explicit);
        assert!(allow_delete(
            judged.innocent,
            judged.explicit,
            true,
            judged.weak,
            judged.confirmed,
            judged.arbiter
        ));

        let weak_grade = probe_still_crop_with(
            &image,
            2,
            0.91,
            |_| {
                Ok(Some(Grade {
                    hard: 0.05,
                    sexy: 0.0,
                    neutral: 0.95,
                    drawn: 0.0,
                    classes: "crop is neutral".to_owned(),
                }))
            },
            |_| panic!("the full-frame grade cannot stand in for this crop"),
        )
        .expect("a negative crop grade is complete");
        assert!(weak_grade.judged.is_none());
        assert!(weak_grade.arbiter.is_none());
    }

    #[test]
    fn still_crop_probe_abstains_on_veto_or_failed_required_witness() {
        let image = image::RgbImage::new(900, 450);
        let veto = probe_still_crop_with(
            &image,
            2,
            0.91,
            |_| Ok(Some(observed_grade())),
            |_| Ok(arbiter(0.0)),
        )
        .expect("negative arbiter is complete");
        assert!(veto.judged.is_none());
        assert!(
            probe_still_crop_with(
                &image,
                2,
                0.91,
                |_| Ok(None),
                |_| panic!("missing grader must abstain before arbiter"),
            )
            .is_err()
        );
        assert!(
            probe_still_crop_with(
                &image,
                2,
                0.91,
                |_| Ok(Some(observed_grade())),
                |_| Err(ModelError::runtime("injected crop arbiter failure")),
            )
            .is_err()
        );
    }

    #[test]
    fn animated_model_failures_preserve_only_the_independently_complete_feature() {
        let frames = || {
            [0, 1]
                .map(|marker| image::RgbImage::from_pixel(2, 2, image::Rgb([marker, 0, 0])))
                .to_vec()
        };
        let embedding = || vec![0.0; super::super::imgfilter::DIM];

        let general = score_animated_frames_with(
            frames(),
            true,
            true,
            true,
            Vec::new(),
            false,
            |frame| {
                if frame.get_pixel(0, 0)[0] == 0 {
                    Err(ModelError::runtime("injected NSFW failure"))
                } else {
                    Ok(Some(observed_look(0.05, 0.05, true)))
                }
            },
            |_| Ok(None),
            |_| Ok(Some(embedding())),
        )
        .unwrap()
        .unwrap();
        assert!(general.nsfw_failed);
        assert!(!general.general_failed);
        assert!(general.judgement.is_none());
        assert!(general.margins.is_some());

        let nsfw = score_animated_frames_with(
            frames(),
            true,
            true,
            true,
            Vec::new(),
            false,
            |frame| {
                let score = if frame.get_pixel(0, 0)[0] == 0 {
                    0.05
                } else {
                    0.88
                };
                Ok(Some(observed_look(score, score, true)))
            },
            |_| Ok(Some(observed_grade())),
            |frame| {
                if frame.get_pixel(0, 0)[0] == 0 {
                    Err(ModelError::runtime("injected general failure"))
                } else {
                    Ok(Some(embedding()))
                }
            },
        )
        .unwrap()
        .unwrap();
        assert!(!nsfw.nsfw_failed);
        assert!(nsfw.general_failed);
        assert!(nsfw.judgement.is_some());
        assert!(nsfw.margins.is_none());
        assert!(nsfw.custom_margins.is_none());
    }

    #[test]
    fn animated_verdict_is_not_cached_if_any_requested_arbiter_was_missing() {
        let score = |missing_nonwinner: bool, low_only: bool| {
            score_animated_frames_with(
                [0, 1]
                    .map(|marker| image::RgbImage::from_pixel(2, 2, image::Rgb([marker, 0, 0])))
                    .to_vec(),
                true,
                false,
                false,
                Vec::new(),
                false,
                move |frame| {
                    let marker = frame.get_pixel(0, 0)[0];
                    let (score, peak) = if low_only {
                        (0.05, 0.05)
                    } else if marker == 0 {
                        (0.88, 0.88)
                    } else {
                        (0.05, 0.70)
                    };
                    Ok(Some(observed_look(score, peak, true)))
                },
                |_| Ok(Some(observed_grade())),
                move |frame| {
                    assert!(!low_only, "a low-peak frame needs no arbiter embedding");
                    if missing_nonwinner && frame.get_pixel(0, 0)[0] == 1 {
                        Ok(None)
                    } else {
                        Ok(Some(vec![0.0; super::super::imgfilter::DIM]))
                    }
                },
            )
            .expect("injected scoring succeeds")
            .expect("frames produce a selection")
        };

        let partial = score(true, false);
        assert!(!partial.nsfw_failed);
        assert_eq!(partial.look.as_ref().map(|look| look.score), Some(0.88));
        assert!(
            partial
                .judgement
                .expect("winning judgement")
                .0
                .arbiter
                .is_some()
        );
        assert!(
            !partial.nsfw_cacheable,
            "another frame lacked a requested arbiter"
        );

        let complete = score(false, false);
        assert!(complete.nsfw_cacheable);
        let low = score(false, true);
        assert!(low.judgement.is_some());
        assert!(low.nsfw_cacheable, "low-peak frames request no arbiter");
    }

    #[test]
    fn animated_frame_verification_does_not_let_a_vetoed_top_frame_mask_an_offence() {
        let agrees = Arbiter {
            explicit: AGREE,
            revealing: 0.0,
            head: 0.5,
            stack: 1.0,
        };
        let blank = Grade {
            hard: 0.0,
            sexy: 0.0,
            neutral: 0.99,
            drawn: 0.0,
            classes: String::new(),
        };
        let vetoed =
            assemble_judgement(&observed_look(0.93, 0.93, true), Some(&blank), Some(agrees))
                .expect("the top frame was fully verified");
        let explicit = assemble_judgement(
            &observed_look(0.88, 0.88, true),
            Some(&observed_grade()),
            Some(agrees),
        )
        .expect("the lower frame was fully verified");
        let choose = |frames: &[(usize, Judgement)], spare| {
            let mut best = None;
            for &frame in frames {
                if better_animated_action(best, frame, spare) {
                    best = Some(frame);
                }
            }
            best.map(|(index, _)| index)
        };
        assert_eq!(choose(&[(0, vetoed), (1, explicit)], false), Some(1));
        assert_eq!(choose(&[(1, explicit), (0, vetoed)], false), Some(1));
        assert_eq!(choose(&[(0, vetoed)], false), None);
        assert!(vetoed.with_animated_spare(None).for_policy(true).is_none());

        let suggestive = Grade {
            hard: 0.01,
            sexy: 0.90,
            neutral: 0.01,
            drawn: 0.0,
            classes: String::new(),
        };
        let top_suggestive = assemble_judgement(
            &observed_look(0.93, 0.93, true),
            Some(&suggestive),
            Some(agrees),
        )
        .expect("the suggestive frame was fully verified");
        let frames = [(0, top_suggestive), (1, explicit)];
        assert_eq!(choose(&frames, false), Some(0));
        assert_eq!(choose(&frames, true), Some(1));
        let cached = top_suggestive.with_animated_spare(Some(explicit));
        assert_eq!(cached.for_policy(false).unwrap().score, 0.93);
        let spare = cached.for_policy(true).unwrap();
        assert_eq!(spare.score, 0.88);
        assert!(spare.explicit);
    }

    #[test]
    fn animated_concepts_are_one_pass_per_frame_and_fold_to_the_best_frame() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = AtomicUsize::new(0);
        let frames: Vec<_> = (0..5u8)
            .map(|index| image::RgbImage::from_pixel(8, 4, image::Rgb([index, 0, 0])))
            .collect();
        let weapon = super::super::concepts::CONCEPTS
            .iter()
            .position(|concept| concept.key == "c_gun")
            .expect("weapon concept exists");
        let along = super::super::vision::unit(
            &super::super::concept_head_vectors::HEADS[weapon].weights,
        );
        let plain = vec![0.0f32; super::super::vision::DIM];
        let selection = score_animated_frames_with(
            frames,
            false,
            true,
            true,
            Vec::new(),
            false,
            |_| Ok(None),
            |_| Ok(None),
            |frame| {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(Some(if frame.get_pixel(0, 0)[0] == 3 {
                    along.clone()
                } else {
                    plain.clone()
                }))
            },
        )
        .expect("injected embeddings succeed")
        .expect("frames produce a selection");
        assert_eq!(calls.load(Ordering::Relaxed), 5, "one pass per frame, nothing more");
        assert!(!selection.concepts_failed);
        let margins = selection.margins.expect("concept margins");
        assert_eq!(
            margins[weapon],
            super::super::concepts::margins_from(&along)[weapon]
        );
        assert!(margins[weapon] > super::super::concepts::margins_from(&plain)[weapon]);
    }

    #[test]
    #[ignore = "needs the cached public media-eval images and VISION_FILES model bundle"]
    fn public_mixed_frames_keep_late_nsfw_and_weapon_evidence() {
        configure_model_tests().expect("configure VISION_FILES before model access");
        let frame = |relative: &str| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
            let bytes = std::fs::read(&path).expect("cached public evaluation image");
            let image = decode_still(&bytes).expect("decode public evaluation image");
            let (width, height) =
                scaled_dimensions(image.width(), image.height(), MAX_ANIMATED_SIDE);
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle)
        };
        let safe = frame("target/media-eval-public/nsfw/people/00000.jpg");
        let weapon_slot = super::super::concepts::CONCEPTS
            .iter()
            .position(|concept| concept.key == "c_gun")
            .expect("weapon concept exists");
        let safe_only = score_animated_frames(vec![safe.clone()], true, true, true, Vec::new())
            .expect("models scored the safe frame")
            .expect("safe frame produced a selection");
        let safe_judged = safe_only.judgement.expect("safe NSFW judgement").0;
        assert!(
            safe_judged.score * 100.0 < DEFAULT_LIMIT as f32
                || !allow_delete(
                    safe_judged.innocent,
                    safe_judged.explicit,
                    false,
                    safe_judged.weak,
                    safe_judged.confirmed,
                    safe_judged.arbiter,
                ),
            "the first frame must be harmless on its own"
        );
        assert!(!super::super::concepts::over(
            safe_only.margins.expect("safe concept margins")[weapon_slot],
            super::super::concepts::DEFAULT_LIMIT,
        ));
        let frames = vec![
            safe,
            frame("target/media-eval-public/nsfw/nsfw/00000.jpg"),
            frame("target/weapon-diagnostic/openimages-test/00203708377265a0.jpg"),
        ];
        let selection = score_animated_frames(frames, true, true, true, Vec::new())
            .expect("models scored every frame")
            .expect("animation produced a selection");
        assert!(!selection.nsfw_failed);
        assert!(!selection.general_failed);
        let judged = selection
            .judgement
            .expect("NSFW judgement")
            .0
            .for_policy(false)
            .expect("normal-policy judgement");
        assert!(
            judged.score * 100.0 >= DEFAULT_LIMIT as f32
                && allow_delete(
                    judged.innocent,
                    judged.explicit,
                    false,
                    judged.weak,
                    judged.confirmed,
                    judged.arbiter,
                ),
            "late explicit frame must remain actionable"
        );
        let margins = selection.margins.expect("concept margins");
        assert!(
            super::super::concepts::over(
                margins[weapon_slot],
                super::super::concepts::DEFAULT_LIMIT,
            ),
            "late rifle frame must remain actionable"
        );
    }

    #[test]
    fn incomplete_or_missing_verification_cannot_form_a_judgement() {
        let incomplete = observed_look(0.99, 0.99, false);
        assert!(matches!(
            assemble_judgement(&incomplete, Some(&observed_grade()), None),
            Err(ModelError::Runtime(_))
        ));

        let needs_grader = observed_look(0.99, 0.99, true);
        assert!(matches!(
            assemble_judgement(&needs_grader, None, None),
            Err(ModelError::Runtime(_))
        ));
    }

    #[test]
    fn the_second_look_waits_without_holding_a_picture_slot() {
        let source = include_str!("nsfw.rs");
        let classify = source
            .split_once("async fn classify(classification: Classification)")
            .expect("classify exists")
            .1
            .split_once("\n}\n")
            .expect("classify ends")
            .0;
        let released = classify.find("drop(visual_slot);").expect("the slot is released");
        let prints = classify
            .find("super::facelock::prints(&frame)")
            .expect("face prints are found");
        let concept_look = classify
            .find("super::concepts::second_look(")
            .expect("the concept second look");
        let filter_look = classify
            .find("super::imgfilter::second_look(&image")
            .expect("the filter second look");
        assert!(prints < released, "face prints must run under the slot");
        assert!(released < concept_look && released < filter_look);
        assert_eq!(classify.matches("drop(visual_slot);").count(), 1);
        let prefetched = classify
            .find("let mut prefetched_still")
            .expect("the still picture is prefetched");
        let taken = classify.find("ctx.nsfw_slot().await").expect("the slot");
        assert!(prefetched < taken);
    }

    #[test]
    fn racing_copies_wait_for_the_first_classification() {
        let source = include_str!("nsfw.rs");
        let watch_one = source
            .split_once("async fn watch_one(")
            .expect("watch_one exists")
            .1
            .split_once("\n}\n")
            .expect("watch_one ends")
            .0;
        let claimed = watch_one.find("ctx.claim_media(id").expect("a claim");
        let spawned = watch_one.find("spawn_owned(").expect("a spawn");
        let classified = watch_one.find("classify(Classification {").expect("classify");
        let handed_back = watch_one
            .find("owner.retry_racing_copy(copy)")
            .expect("waiting copies are handed back");
        assert!(claimed < spawned && classified < handed_back);
    }

    #[test]
    fn nsfw_abstention_cannot_return_from_the_shared_media_pipeline() {
        let source = include_str!("nsfw.rs");
        let analysis = source
            .split_once("None => 'nsfw_analysis: {")
            .expect("the NSFW analysis has a scoped abstention boundary")
            .1
            .split_once("if wants_embedding && !embedded && !general_failed {")
            .expect("independent media consumers follow NSFW analysis")
            .0;
        assert!(analysis.contains("break 'nsfw_analysis None"));
        assert!(
            !analysis.lines().any(|line| line.trim() == "return;"),
            "an NSFW-only failure must not skip concept/custom consumers or advert OCR"
        );
        let independent = source
            .split_once("if wants_embedding && !embedded && !general_failed {")
            .unwrap()
            .1;
        assert!(
            independent.contains("if let Some(armed) = advert"),
            "advert OCR must remain after the scoped NSFW abstention"
        );
    }

    #[test]
    fn optional_encoder_absence_differs_from_a_configured_failure() {
        assert!(
            arbiter_from_embedding(Ok(None))
                .expect("an absent optional encoder is valid")
                .is_none()
        );
        let injected = ModelError::runtime("injected configured encoder failure");
        match arbiter_from_embedding(Err(injected.clone())) {
            Err(error) => assert_eq!(error, injected),
            Ok(_) => panic!("a configured encoder failure must not become absence"),
        }
    }

    #[test]
    fn shared_assembly_preserves_the_successful_live_policy() {
        let look = observed_look(0.60, 0.88, true);
        let grade = observed_grade();
        let arbiter = Arbiter {
            explicit: RECOVER,
            revealing: 0.01,
            head: HEAD_DELETE,
            stack: 1.0,
        };
        let judged = assemble_judgement(&look, Some(&grade), Some(arbiter))
            .expect("complete observations assemble");

        assert_eq!(judged.score, look.peak.max(arbiter.head));
        assert_eq!(judged.innocent, innocent(&grade, look.score));
        assert!(
            judged.explicit,
            "a deleting fitted head is explicit evidence"
        );
        assert_eq!(judged.confirmed, confirms(look.score, &grade));
        assert_eq!(judged.weak, look.frail && !grade.corroborates());
        assert!(allow_delete(
            judged.innocent,
            judged.explicit,
            false,
            judged.weak,
            judged.confirmed,
            judged.arbiter,
        ));
    }

    #[test]
    fn offline_evaluator_emits_bounded_serializable_real_evidence() {
        let image = image::RgbImage::from_fn(SIDE as u32, SIDE as u32, |x, y| {
            image::Rgb([
                ((x + y) % 251) as u8,
                ((x * 3 + y) % 253) as u8,
                ((x + y * 5) % 255) as u8,
            ])
        });
        let evaluated = evaluate_image_offline(&image, DEFAULT_LIMIT, false)
            .expect("the production still-image evaluator succeeds");
        assert_eq!((evaluated.width, evaluated.height), image.dimensions());
        assert!(evaluated.score.is_finite());
        assert!(evaluated.peak.is_finite());
        assert!(
            evaluated
                .view_scores
                .iter()
                .flatten()
                .all(|score| score.is_finite())
        );
        let json = serde_json::to_value(&evaluated).expect("offline evidence serializes");
        assert!(json.get("view_scores").is_some());
        assert!(json.get("would_delete").is_some());
    }

    #[test]
    fn the_offending_class_is_the_first_one() {
        assert_eq!(CLASSES, ["nsfw", "sfw"]);
        assert!(
            (nsfw_of(&[10.0, -10.0]).expect("finite logits") - 1.0).abs() < 0.01,
            "index 0 must be the offence"
        );
        assert!(
            nsfw_of(&[-10.0, 10.0]).expect("finite logits") < 0.01,
            "index 1 must be the safe one"
        );

        assert!(nsfw_of(&[0.0, 0.0]).expect("finite logits") < 0.51);
        assert!(nsfw_of(&[]).is_err(), "an empty output is not a score");
        assert!(
            nsfw_of(&[f32::NAN, 1.0]).is_err(),
            "a non-finite output is not a clean score"
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
    fn d002_rung_cache_is_order_independent_and_nsfw_warm_uses_selected_source() {
        let tl::enums::Photo::Photo(mut photo) = routing_photo(78_002, 2) else {
            unreachable!()
        };
        photo.sizes = vec![
            tl::types::PhotoSize {
                r#type: "x".into(),
                w: 512,
                h: 384,
                size: 16_038,
            }
            .into(),
            tl::types::PhotoSize {
                r#type: "y".into(),
                w: 800,
                h: 600,
                size: 42_391,
            }
            .into(),
        ];
        let media = raw_photo_media(&photo.into()).unwrap();
        let ladder = ladder(&media).unwrap();
        assert_eq!(ladder.start, 0);
        let low = ladder.source(0);
        let high = ladder.source(1);
        assert_ne!(low, high);
        assert_eq!(ladder.source_index(&high), Some(1));
        let high_only = ladder_from(vec![ladder.sizes[1].clone()], Some(2)).unwrap();
        assert_eq!(high_only.source_index(&high), Some(0));
        let low_only = ladder_from(vec![ladder.sizes[0].clone()], Some(2)).unwrap();
        assert!(!source_available(&high, false, Some(&low_only), false));
        let full_route = verdict_route(false, Some(&ladder), false).unwrap();
        let low_only_route = verdict_route(false, Some(&low_only), false).unwrap();
        assert_ne!(full_route, low_only_route);

        const LOW_MARGIN: f32 = 0.018_582_977;
        const HIGH_MARGIN: f32 = 0.025_072_463;
        assert!(!super::super::concepts::over(
            LOW_MARGIN,
            super::super::concepts::DEFAULT_LIMIT
        ));
        assert!(super::super::concepts::over(
            HIGH_MARGIN,
            super::super::concepts::DEFAULT_LIMIT
        ));
        let look = 0.283_435;
        assert!(look >= GRADE_ABOVE && undecided(look));
        assert!(use_sharper_rung(
            true,
            false,
            ladder.sizes.len() > ladder.start + 1
        ));

        let judged = Judgement {
            score: 0.19,
            innocent: true,
            explicit: false,
            confirmed: false,
            weak: false,
            arbiter: None,
            spare_frame: None,
        };
        let mut cached_verdicts = HashMap::new();
        cached_verdicts.insert((78_002, low_only_route.clone()), (judged, low.clone()));
        assert!(
            !cached_verdicts.contains_key(&(78_002, full_route.clone())),
            "a low-only verdict must not answer a later full ladder"
        );
        cached_verdicts.insert((78_002, full_route.clone()), (judged, high.clone()));
        assert_eq!(
            cached_verdicts
                .get(&(78_002, low_only_route))
                .map(|(_, source)| source),
            Some(&low)
        );
        for order in [[false, true], [true, false]] {
            let mut old_by_file = HashMap::<i64, f32>::new();
            let mut by_source = HashMap::<(i64, VisionSource), f32>::new();
            let mut cached_verdict = None;
            for (at, with_nsfw) in order.into_iter().enumerate() {
                let lookup = scoring_source(
                    with_nsfw,
                    cached_verdict.as_ref(),
                    false,
                    Some(&ladder),
                    false,
                );
                if with_nsfw && cached_verdict.is_none() {
                    assert_eq!(lookup, None, "fresh NSFW must defer the margin lookup");
                } else {
                    assert_eq!(lookup, Some(low.clone()));
                }
                let (selected, observed) = if with_nsfw {
                    (high.clone(), HIGH_MARGIN)
                } else {
                    (low.clone(), LOW_MARGIN)
                };
                let old = *old_by_file.entry(78_002).or_insert(observed);
                let current = *by_source.entry((78_002, selected)).or_insert(observed);
                assert_eq!(current, observed);
                if at == 1 {
                    assert_ne!(
                        old, observed,
                        "file-id-only cache must expose the order bug"
                    );
                }
                if with_nsfw {
                    cached_verdict = Some((judged, high.clone()));
                }
            }
            let warm = scoring_source(true, cached_verdict.as_ref(), false, Some(&ladder), false)
                .expect("cached NSFW verdict keeps its selected rung");
            assert_eq!(warm, high);
            assert_eq!(ladder.source_index(&warm), Some(1));
            assert_eq!(by_source.get(&(78_002, warm)), Some(&HIGH_MARGIN));
        }
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
            drawn: 0.0,
            classes: String::new(),
        };
        let bikini = Grade {
            hard: 0.05,
            sexy: 0.01,
            neutral: 0.92,
            drawn: 0.0,
            classes: String::new(),
        };
        let bridge = Grade {
            hard: 0.10,
            sexy: 0.01,
            neutral: 0.50,
            drawn: 0.0,
            classes: String::new(),
        };
        let strong = Grade {
            hard: 0.75,
            sexy: 0.00,
            neutral: 0.05,
            drawn: 0.0,
            classes: String::new(),
        };
        let hardcore = Grade {
            hard: 0.97,
            sexy: 0.00,
            neutral: 0.03,
            drawn: 0.0,
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
            drawn: 0.0,
            classes: String::new(),
        };
        let blank = Grade {
            hard: 0.00,
            sexy: 0.00,
            neutral: 1.00,
            drawn: 0.0,
            classes: String::new(),
        };
        let glimpse = Grade {
            hard: EXPLICIT_FLOOR,
            sexy: 0.10,
            neutral: 0.30,
            drawn: 0.0,
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
    fn grader_checks_edge_after_a_weak_centre_sighting() {
        let image = image::RgbImage::new(960, 480);
        let views = [
            [0.10_f32, 0.15, 0.55, 0.15, 0.05],
            [0.02, 0.40, 0.10, 0.45, 0.03],
            [0.96, 0.01, 0.01, 0.01, 0.01],
        ];
        let centre = fold(None, &views[0], String::new());
        assert!(centre.corroborates());
        assert!(!confirms(0.60, &centre));

        let mut calls = 0;
        let grade = grade_with_infer(&image, |_| {
            let logits = views[calls].into_iter().map(f32::ln).collect();
            calls += 1;
            Ok(logits)
        })
        .expect("mocked grader views");
        assert_eq!(calls, 2, "the strong edge must be graded before early exit");
        assert!((grade.hard - 0.85).abs() < 1e-5);
        assert!(confirms(0.60, &grade));
    }

    #[test]
    fn grader_checks_every_view_below_the_bridge_floor() {
        let image = image::RgbImage::new(960, 480);
        let views = [
            [0.90_f32, 0.02, 0.04, 0.02, 0.02],
            [0.83, 0.04, 0.05, 0.04, 0.04],
            [0.86, 0.03, 0.06, 0.03, 0.02],
        ];
        let mut calls = 0;
        let grade = grade_with_infer(&image, |_| {
            let logits = views[calls].into_iter().map(f32::ln).collect();
            calls += 1;
            Ok(logits)
        })
        .expect("mocked grader views");
        assert_eq!(calls, 3);
        assert!(grade.hard < EXPLICIT_FLOOR);
        assert!((grade.neutral - 0.04).abs() < 1e-5);
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
            spare_frame: None,
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
        let p = probabilities(&logits).expect("finite logits");
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
            probabilities(&logits).expect("finite logits")
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
    #[ignore = "explicit inference benchmark; excluded from the correctness suite"]
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
    #[ignore = "explicit 200-image capacity benchmark; excluded from the correctness suite"]
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
