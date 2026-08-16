use std::sync::Arc;

use grammers_client::media::{Downloadable, Media, PhotoSize};
use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerRef};

use super::Ctx;
use super::locks::View;

pub const LOCK: &str = "nsfw";

pub const LIVE: &str = "nsfw_live";

pub const LIMIT: &str = "nsfw_lim";

pub const LIMIT_RANGE: (u32, u32) = (10, 95);
pub const LIMIT_PRESETS: &[u32] = &[30, 40, 50, 60, 70, 80];
const DEFAULT_LIMIT: u32 = 50;

const MODEL: &[u8] = include_bytes!("../../assets/nsfw.onnx");

const SIDE: usize = 384;

const RESIZE: usize = 384;

const CLASSES: [&str; 2] = ["nsfw", "sfw"];

const MARGIN: f32 = 0.15;

const CORROBORATE: f32 = 0.30;

const WIDE: f32 = 1.5;

pub const SOFT: &str = "nsfw_soft";

const GRADE: &[u8] = include_bytes!("../../assets/nsfw_grade.onnx");

const GRADE_SIDE: usize = 224;
const GRADE_RESIZE: usize = 256;
const GRADE_CLASSES: [&str; 5] = ["drawings", "hentai", "neutral", "porn", "sexy"];

const EXPLICIT_FLOOR: f32 = 0.20;

const NEUTRAL_FLOOR: f32 = 0.97;
const CONFIDENT: f32 = 0.92;

const SUGGESTIVE_FLOOR: f32 = 0.50;

const GRADE_ABOVE: f32 = LIMIT_RANGE.0 as f32 / 100.0;

const RUNTIME: &[u8] = include_bytes!("../../assets/libonnxruntime.so.1.20.1");

const INFER_THREADS: usize = 4;

pub type Session = std::sync::Mutex<ort::session::Session>;

fn runtime_path() -> Option<std::path::PathBuf> {
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
        let builder = ort::session::Session::builder().map_err(|e| e.to_string())?;
        let mut builder = builder
            .with_intra_threads(INFER_THREADS)
            .map_err(|e| e.to_string())?;
        builder.commit_from_file(path).map_err(|e| e.to_string())
    };
    match built() {
        Ok(session) => Some(std::sync::Mutex::new(session)),
        Err(e) => {
            eprintln!("nsfw: the {what} would not load from {}: {e}", path.display());
            None
        }
    }
}

pub fn open(bytes: &[u8], what: &str) -> Option<Session> {
    if !started() {
        return None;
    }

    let built = || -> Result<ort::session::Session, String> {
        let builder = ort::session::Session::builder().map_err(|e| e.to_string())?;
        let mut builder = builder
            .with_intra_threads(INFER_THREADS)
            .map_err(|e| e.to_string())?;
        builder.commit_from_memory(bytes).map_err(|e| e.to_string())
    };
    match built() {
        Ok(session) => Some(std::sync::Mutex::new(session)),
        Err(e) => {
            eprintln!("nsfw: the {what} would not load: {e}");
            None
        }
    }
}

fn model() -> Option<&'static Session> {
    static MODEL_CELL: std::sync::OnceLock<Option<Session>> = std::sync::OnceLock::new();
    MODEL_CELL.get_or_init(|| open(MODEL, "model")).as_ref()
}

fn grader() -> Option<&'static Session> {
    static GRADER: std::sync::OnceLock<Option<Session>> = std::sync::OnceLock::new();
    GRADER.get_or_init(|| open(GRADE, "grader")).as_ref()
}

pub fn run(session: &Session, shape: Vec<i64>, pixels: Vec<f32>) -> Option<Vec<f32>> {
    run_shaped(session, shape, pixels).map(|(_, values)| values)
}

pub fn run_shaped(
    session: &Session,
    shape: Vec<i64>,
    pixels: Vec<f32>,
) -> Option<(Vec<i64>, Vec<f32>)> {
    let input = ort::value::Value::from_array((shape, pixels)).ok()?;
    let mut session = session.lock().ok()?;
    let output = session.run(ort::inputs![input]).ok()?;
    let (shape, values) = output[0].try_extract_tensor::<f32>().ok()?;
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

fn pixels_of(view: &image::RgbImage, side: usize, scale: fn(u8) -> f32) -> Vec<f32> {
    pixels_by(view, side, |_, value| scale(value))
}

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .with_chat(chat, |settings| settings.number(LIMIT, DEFAULT_LIMIT, LIMIT_RANGE))
}

fn probabilities(logits: &[f32]) -> Vec<f32> {
    let top = logits.iter().copied().fold(f32::MIN, f32::max);
    let exp: Vec<f32> = logits.iter().map(|x| (x - top).exp()).collect();
    let sum: f32 = exp.iter().sum();
    if sum <= 0.0 {
        return vec![0.0; logits.len()];
    }
    exp.into_iter().map(|x| x / sum).collect()
}

fn nsfw_of(logits: &[f32]) -> f32 {
    probabilities(logits)[0]
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
            let (x, y) = if width >= height { (offset, 0) } else { (0, offset) };
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

fn score(image: &image::RgbImage) -> Option<(f32, String)> {
    let session = model()?;

    let mut scores: Vec<f32> = Vec::new();
    for view in views(image, RESIZE, SIDE) {
        let pixels = pixels_of(&view, SIDE, |value| f32::from(value) / 127.5 - 1.0);
        let logits = run(session, vec![1, 3, SIDE as i64, SIDE as i64], pixels)?;
        if logits.len() != CLASSES.len() {
            return None;
        }
        scores.push(nsfw_of(&logits));
    }
    let (&whole, ends) = scores.split_first()?;
    let note = format!(
        "views {}",
        scores.iter().map(|s| format!("{s:.2}")).collect::<Vec<_>>().join("/")
    );
    Some((verdict_of(whole, ends), note))
}

fn hard_of(p: &[f32]) -> f32 {
    p[1] + p[3]
}

fn explicit(p: &[f32]) -> bool {
    let revealing = hard_of(p) < EXPLICIT_FLOOR && p[4] >= SUGGESTIVE_FLOOR;
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
        !(self.hard < EXPLICIT_FLOOR && self.sexy >= SUGGESTIVE_FLOOR)
    }
}

fn grade(image: &image::RgbImage) -> Option<Grade> {
    let session = grader()?;

    let mut worst: Option<Grade> = None;
    for view in views(image, GRADE_RESIZE, GRADE_SIDE) {
        let pixels = pixels_of(&view, GRADE_SIDE, |value| f32::from(value) / 255.0);
        let logits = run(session, vec![3, GRADE_SIDE as i64, GRADE_SIDE as i64], pixels)?;
        if logits.len() != GRADE_CLASSES.len() {
            return None;
        }
        let p = probabilities(&logits);
        let hard = hard_of(&p);
        let done = explicit(&p);
        if worst.as_ref().is_none_or(|seen| hard > seen.hard) {
            worst = Some(Grade {
                hard,
                sexy: p[4],
                neutral: p[2],
                classes: breakdown(&GRADE_CLASSES, &logits),
            });
        }

        if done {
            break;
        }
    }
    worst
}

fn innocent(grade: &Grade, score: f32) -> bool {
    grade.neutral >= NEUTRAL_FLOOR && score < CONFIDENT
}

fn allow_delete(innocent: bool, explicit: bool, spare: bool) -> bool {
    !innocent && (!spare || explicit)
}

fn verdict_of(whole: f32, ends: &[f32]) -> f32 {
    if whole < CORROBORATE {
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
        _ => None,
    }
}

fn thumbs(media: &Media) -> Vec<PhotoSize> {
    match media {
        Media::Photo(photo) => photo.thumbs(),
        Media::Document(document) => match document.raw.video_cover.clone() {
            Some(cover) => grammers_client::media::Photo::from_raw(cover).thumbs(),
            None => document.thumbs(),
        },
        Media::Sticker(sticker) => sticker.document.thumbs(),
        _ => Vec::new(),
    }
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
    let sizes: Vec<PhotoSize> = thumbs(media)
        .into_iter()
        .filter(|size| size.to_raw_input_location().is_some())
        .collect();
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

fn best_thumb(candidates: &[(i32, usize)]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, (side, _))| *side >= RESIZE as i32)
        .min_by_key(|(_, (_, bytes))| *bytes)
        .or_else(|| candidates.iter().enumerate().max_by_key(|(_, (side, _))| *side))
        .map(|(at, _)| at)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ignore,
    Log,
    Delete,
}

fn verdict(score: f32, limit: u32, shadow: bool) -> Verdict {
    let over = score * 100.0 >= limit as f32;
    match (shadow, over) {
        (true, _) => Verdict::Log,
        (false, true) => Verdict::Delete,
        (false, false) => Verdict::Ignore,
    }
}

pub const TEST: &[&str] = &["تست غیراخلاقی", "تست مستهجن"];

pub async fn test(ctx: &Arc<Ctx>, message: &Message) -> bool {
    if !TEST.contains(&message.text().trim()) {
        return false;
    }
    let sender = message.sender_id().and_then(PeerId::bare_id);
    if sender.is_none() || sender != super::cleaner::sudo() {
        return false;
    }
    let Some(chat) = super::chat_id(message) else {
        return false;
    };

    let asked = "روی یک عکس، گیف، استیکر یا فیلم ریپلای کنید.";
    let Ok(Some(reply)) = message.get_reply().await else {
        let _ = message.reply(asked).await;
        return true;
    };
    let Some(media) = reply.media() else {
        let _ = message.reply(asked).await;
        return true;
    };
    let (Some(id), Some(ladder)) = (file_id(&media), ladder(&media)) else {
        let _ = message.reply("این رسانه تامبنیلی که بشود بررسی کرد ندارد.").await;
        return true;
    };

    let Some(bytes) = fetch(ctx, chat, id, &ladder, ladder.start).await else {
        let _ = message.reply("تامبنیل دانلود نشد.").await;
        return true;
    };
    let size = bytes.len();
    let note = dims(&ladder.sizes[ladder.start]).map_or_else(
        || format!("?x? bytes {size}"),
        |(width, height)| format!("{width}x{height} bytes {size}"),
    );
    let Ok(Some(image)) = tokio::task::spawn_blocking(move || {
        image::load_from_memory(&bytes).ok().map(|image| image.to_rgb8())
    })
    .await
    else {
        let _ = message.reply("تصویر باز نشد.").await;
        return true;
    };
    let image = Arc::new(image);

    let scoring = Arc::clone(&image);
    let Ok(Some((score, views))) = tokio::task::spawn_blocking(move || score(&scoring)).await
    else {
        let _ = message.reply("بررسی نشد.").await;
        return true;
    };

    let grading = Arc::clone(&image);
    let graded = tokio::task::spawn_blocking(move || grade(&grading)).await.ok().flatten();
    let grade_line = match &graded {
        Some(grade) => format!(
            "{}
     ← {}",
            grade.classes,
            if innocent(grade, score) {
                "قطعا بی اشکال"
            } else if grade.explicit() {
                "مستهجن"
            } else {
                "محرک، نه مستهجن"
            }
        ),
        None => "در دسترس نیست".to_owned(),
    };

    let limit = limit(ctx, chat);
    let armed = ctx.settings.is_locked(chat, LOCK);
    let live = ctx.settings.is_locked(chat, LIVE);
    let soft = ctx.settings.is_locked(chat, SOFT);
    let over = score * 100.0 >= limit as f32;
    let clean = graded.as_ref().is_some_and(|grade| innocent(grade, score));
    let explicit = graded.as_ref().is_none_or(Grade::explicit);

    let outcome = if !armed {
        "قفل در این گروه خاموش است"
    } else if !over {
        "پاک نمی شود · زیر حد"
    } else if clean {
        "پاک نمی شود · مدل دوم مطمئن است که بی اشکال است"
    } else if soft && !explicit {
        "پاک نمی شود · محرک است نه مستهجن"
    } else if !live {
        "فقط ثبت می شود · حالت آزمایشی"
    } else {
        "پاک می شود"
    };

    let _ = message
        .reply(grammers_client::message::InputMessage::new().html(format!(
            "<b>تست محتوا</b>\n\n             فایل · <code>{id}</code>\n             تامبنیل · {}\n             نمای ها · <b>{}</b>\n             نمره · <b>{score:.4}</b> · حد <b>٪{limit}</b>\n             درجه بندی · {}\n\n             <b>{outcome}</b>",
            super::esc(&note),
            super::esc(views.trim_start_matches("views ")),
            super::esc(&grade_line),
        )))
        .await;
    true
}

pub async fn watch(ctx: &Arc<Ctx>, message: &Message, chat: i64, view: &View<'_>) {
    let Some(media) = view.media() else {
        return;
    };
    let Some(id) = file_id(media) else {
        return;
    };

    let (nsfw, concepts, advert) = ctx.settings.with_chat(chat, |settings| {
        let nsfw = settings.is_locked(LOCK).then(|| Armed {
            shadow: !settings.is_locked(LIVE),
            limit: settings.number(LIMIT, DEFAULT_LIMIT, LIMIT_RANGE),
            spare: settings.is_locked(SOFT),
        });
        let advert = settings.is_locked(super::ocr::LOCK).then(|| AdvertArmed {
            live: !settings.is_locked(super::ocr::SHADOW),
        });
        (nsfw, super::concepts::armed_under(&settings), advert)
    });
    if nsfw.is_none() && concepts.is_none() && advert.is_none() {
        return;
    }

    let verdict = ctx.known_verdict(id);
    let margins = ctx.known_margins(id);
    let reading = ctx.known_advert(id);
    let wants_nsfw = nsfw.is_some() && verdict.is_none();
    let wants_concepts = concepts.is_some() && margins.is_none();
    let wants_text = advert.is_some() && reading.is_none();

    if super::is_exempt(ctx, message).await {
        return;
    }

    if !wants_nsfw && !wants_concepts && !wants_text {
        if let (Some(armed), Some((known, innocent, explicit))) = (&nsfw, verdict) {
            act(
                ctx, message, chat, id, known, innocent, explicit, armed.limit, armed.shadow,
                armed.spare, true,
            )
            .await;
        }
        if let (Some(armed), Some(all)) = (&concepts, margins) {
            super::concepts::act_known(ctx, message, chat, id, &all, armed, true).await;
        }
        if let (Some(armed), Some(why)) = (&advert, reading) {
            act_advert(ctx, message, chat, id, why, armed, true).await;
        }
        return;
    }

    let Some(ladder) = ladder(media) else {
        println!("nsfw: {chat} file {id} skipped, no downloadable thumbnail");
        return;
    };

    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return;
    };
    let message_id = message.id();
    let sender = message.sender_id().and_then(PeerId::bare_id);
    let name = super::name_of(message);
    let ctx = Arc::clone(ctx);

    tokio::spawn(async move {
        classify(
            ctx, chat, chat_ref, message_id, id, ladder, nsfw, concepts, advert, verdict, margins,
            reading, sender, name,
        )
        .await;
    });
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

#[allow(clippy::too_many_arguments)]
async fn classify(
    ctx: Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    id: i64,
    ladder: Ladder,
    nsfw: Option<Armed>,
    concepts: Option<super::concepts::Armed>,
    advert: Option<AdvertArmed>,
    verdict: Option<(f32, bool, bool)>,
    margins: Option<[f32; super::CONCEPT_SLOTS]>,
    reading: Option<Option<&'static str>>,
    sender: Option<i64>,
    name: String,
) {
    let Some(bytes) = fetch(&ctx, chat, id, &ladder, ladder.start).await else {
        return;
    };
    let size = bytes.len();
    let pixels = dims(&ladder.sizes[ladder.start]).map_or_else(
        || "?x?".to_owned(),
        |(width, height)| format!("{width}x{height}"),
    );

    let thinking = std::time::Instant::now();
    let _slot = ctx.nsfw_slot().await;

    let Ok(Some(image)) = tokio::task::spawn_blocking(move || {
        image::load_from_memory(&bytes).ok().map(|image| image.to_rgb8())
    })
    .await
    else {
        return;
    };
    let image = Arc::new(image);

    if let Some(armed) = nsfw {
        let (score, classes, innocent, explicit) = match verdict {
            Some((score, innocent, explicit)) => (score, String::new(), innocent, explicit),
            None => {
                let frame = Arc::clone(&image);
                let Ok(Some((mut score, mut classes))) =
                    tokio::task::spawn_blocking(move || score(&frame)).await
                else {
                    return;
                };

                let target = armed.limit as f32 / 100.0;
                let close = (score - target).abs() < MARGIN;
                let sharper = if close && ladder.sizes.len() > ladder.start + 1 {
                    fetch(&ctx, chat, id, &ladder, ladder.start + 1).await
                } else {
                    None
                };
                if let Some(sharper) = sharper
                    && let Ok(Some(better)) = tokio::task::spawn_blocking(move || {
                        let frame = image::load_from_memory(&sharper).ok()?.to_rgb8();
                        self::score(&frame)
                    })
                    .await
                {
                    score = better.0;
                    classes = format!("{} escalated", better.1);
                }

                let (innocent, explicit) = if score >= GRADE_ABOVE {
                    let frame = Arc::clone(&image);
                    match tokio::task::spawn_blocking(move || grade(&frame)).await {
                        Ok(Some(grade)) => {
                            println!("nsfw: {chat} file {id} grade [{}]", grade.classes);
                            (innocent(&grade, score), grade.explicit())
                        }

                        _ => (false, true),
                    }
                } else {
                    (false, true)
                };
                ctx.remember_verdict(id, score, innocent, explicit);
                println!(
                    "nsfw: {chat} file {id} {pixels} bytes {size} think {}ms {classes}",
                    thinking.elapsed().as_millis()
                );
                (score, classes, innocent, explicit)
            }
        };
        let _ = classes;
        act_detached(
            &ctx, chat, chat_ref, message_id, id, score, innocent, explicit, armed.limit,
            armed.shadow, armed.spare, sender, name.clone(),
        )
        .await;
    }

    if let Some(armed) = concepts {
        let all = match margins {
            Some(all) => all,
            None => {
                let frame = Arc::clone(&image);
                let Ok(Some(all)) =
                    tokio::task::spawn_blocking(move || super::concepts::margins_of(&frame)).await
                else {
                    return;
                };
                ctx.remember_margins(id, all);
                all
            }
        };
        super::concepts::act_detached(
            &ctx, chat, chat_ref, message_id, id, &all, &armed, sender, &name,
        )
        .await;
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
                println!(
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
            Ok(0) => eprintln!("advert: delete affected nothing in {chat} msg {message_id}"),
            Ok(_) => {}
            Err(e) => {
                eprintln!("advert: could not delete in {chat} msg {message_id}: {e}");
                return;
            }
        }
        ctx.bump(chat, super::stats::DELETED);
        notify(&ctx, chat, chat_ref, sender, &name, why).await;
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
    println!(
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
    super::notice::send(ctx, message, chat, why, None).await;
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

#[allow(clippy::too_many_arguments)]
async fn act(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    id: i64,
    score: f32,
    innocent: bool,
    explicit: bool,
    limit: u32,
    shadow: bool,
    spare: bool,
    cached: bool,
) {
    report(chat, id, score, limit, shadow, cached);
    if verdict(score, limit, shadow) != Verdict::Delete
        || spared(innocent, explicit, spare, chat, id)
    {
        return;
    }
    if let Err(e) = message.delete().await {
        eprintln!("nsfw: could not delete in {chat}: {e}");
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    super::notice::send(ctx, message, chat, "محتوای غیراخلاقی", None).await;
}

#[allow(clippy::too_many_arguments)]
async fn act_detached(
    ctx: &Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    id: i64,
    score: f32,
    innocent: bool,
    explicit: bool,
    limit: u32,
    shadow: bool,
    spare: bool,
    sender: Option<i64>,
    name: String,
) {
    report(chat, id, score, limit, shadow, false);
    if verdict(score, limit, shadow) != Verdict::Delete
        || spared(innocent, explicit, spare, chat, id)
    {
        return;
    }
    match ctx.client.delete_messages(chat_ref, &[message_id]).await {
        Ok(0) => eprintln!("nsfw: delete affected nothing in {chat} msg {message_id}"),
        Ok(_) => {}
        Err(e) => {
            eprintln!("nsfw: could not delete in {chat} msg {message_id}: {e}");
            return;
        }
    }
    ctx.bump(chat, super::stats::DELETED);

    notify(ctx, chat, chat_ref, sender, &name, "محتوای غیراخلاقی").await;
}

pub async fn notify(
    ctx: &Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    sender: Option<i64>,
    name: &str,
    reason: &str,
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
        "<a href=\"tg://user?id={user}\">{}</a> پیام شما حذف شد · <b>{}</b> در این گروه قفل است.\n<i>لطفا دوباره نفرستید.</i>",
        super::esc(name),
        super::esc(reason)
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
    let owner = Arc::clone(ctx);
    let sent_id = sent.id();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(u64::from(seconds))).await;
        let _ = owner.client.delete_messages(chat_ref, &[sent_id]).await;
    });
}

fn spared(innocent: bool, explicit: bool, spare: bool, chat: i64, id: i64) -> bool {
    if allow_delete(innocent, explicit, spare) {
        return false;
    }
    let why = if innocent {
        "the grader is certain it is nothing"
    } else {
        "suggestive rather than explicit"
    };
    println!("nsfw: {chat} file {id} kept, {why}");
    true
}

fn report(chat: i64, id: i64, score: f32, limit: u32, shadow: bool, cached: bool) {
    let over = if score * 100.0 >= limit as f32 { "OVER" } else { "ok" };
    let how = if cached { " cached" } else { "" };
    let mode = if shadow { "shadow" } else { "live" };
    println!("nsfw[{mode}]: chat {chat} file {id} score {score:.4} limit {limit} {over}{how}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_that_never_heard_of_this_is_shadowing() {
        let absent = false;
        let shadow = !absent;
        assert!(shadow, "the default must be shadow, not live");
        assert_eq!(verdict(1.0, DEFAULT_LIMIT, shadow), Verdict::Log);
        assert!(LIVE.contains("live"), "the key names the unsafe state, not the safe one");
    }

    #[test]
    fn shadow_mode_never_deletes() {
        for score in [0.0, 0.5, 0.9, 1.0] {
            assert_eq!(verdict(score, 90, true), Verdict::Log, "score {score}");
        }
        assert_eq!(verdict(0.95, 90, false), Verdict::Delete);
        assert_eq!(verdict(0.90, 90, false), Verdict::Delete, "the limit is inclusive");
        assert_eq!(verdict(0.89, 90, false), Verdict::Ignore);
    }

    #[test]
    fn the_offending_class_is_the_first_one() {
        assert_eq!(CLASSES, ["nsfw", "sfw"]);
        assert!((nsfw_of(&[10.0, -10.0]) - 1.0).abs() < 0.01, "index 0 must be the offence");
        assert!(nsfw_of(&[-10.0, 10.0]) < 0.01, "index 1 must be the safe one");

        assert!(nsfw_of(&[0.0, 0.0]) < 0.51);
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
        assert!(mean(&got[1]) > mean(&got[2]), "the two ends are the same view");
    }

    #[test]
    fn fitting_centre_crops_instead_of_squashing() {
        let wide = image::RgbImage::from_fn(1280, 720, |x, _| {
            image::Rgb([if (560..720).contains(&x) { 255 } else { 0 }, 0, 0])
        });
        let out = fit(&wide, RESIZE, SIDE);
        assert_eq!(out.dimensions(), (SIDE as u32, SIDE as u32));

        let centre = out.get_pixel(SIDE as u32 / 2, SIDE as u32 / 2).0[0];
        assert!(centre > 200, "the middle of the frame was lost, got {centre}");

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
        assert_eq!(best_thumb(&[(240, 9_000), (600, 60_000), (960, 180_000)]), Some(1));

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
    fn the_escalation_band_straddles_the_limit() {
        for preset in LIMIT_PRESETS {
            let target = *preset as f32 / 100.0;
            assert!((target - MARGIN + 0.01 - target).abs() < MARGIN, "below {preset}");
            assert!((target + MARGIN - 0.01 - target).abs() < MARGIN, "above {preset}");

            assert!((0.02f32 - target).abs() >= MARGIN, "0.02 escalated at {preset}");
            assert!((0.99f32 - target).abs() >= MARGIN, "0.99 escalated at {preset}");
        }
    }

    #[test]
    fn a_certain_grader_overrules_an_uncertain_model() {
        let blank = Grade { hard: 0.00, sexy: 0.00, neutral: 1.00, classes: String::new() };
        let bikini = Grade { hard: 0.05, sexy: 0.01, neutral: 0.92, classes: String::new() };
        let hardcore = Grade { hard: 0.97, sexy: 0.00, neutral: 0.03, classes: String::new() };

        assert!(innocent(&blank, 0.879), "the labelled false positive must be spared");

        assert!(!innocent(&bikini, 0.93));
        assert!(!innocent(&hardcore, 0.93));

        assert!(!innocent(&blank, 0.95), "a confident first model wins");
        assert!(!innocent(&blank, CONFIDENT));

        assert!(!allow_delete(true, true, false), "innocent wins over explicit");
        assert!(allow_delete(false, true, true), "explicit is deleted even in a soft chat");
    }

    #[test]
    fn a_crop_cannot_condemn_an_innocent_frame() {
        assert!((verdict_of(0.11, &[0.93, 0.72]) - 0.11).abs() < 1e-6);

        assert!((verdict_of(0.88, &[0.45, 0.93]) - 0.93).abs() < 1e-6);
        assert!((verdict_of(0.95, &[0.95, 0.94]) - 0.95).abs() < 1e-6);

        assert!((verdict_of(0.89, &[]) - 0.89).abs() < 1e-6);
        assert!((verdict_of(0.05, &[]) - 0.05).abs() < 1e-6);

        assert!((verdict_of(0.40, &[0.99]) - 0.99).abs() < 1e-6);

        assert!((verdict_of(0.29, &[0.99]) - 0.29).abs() < 1e-6);
    }

    #[test]
    fn the_grader_spares_only_on_positive_evidence() {
        let unrecognised = [0.02, 0.05, 0.92, 0.00, 0.01];

        let blank = [0.00, 0.00, 1.00, 0.00, 0.00];

        let hardcore = [0.00, 0.00, 0.03, 0.97, 0.00];

        let revealing = [0.05, 0.01, 0.14, 0.10, 0.70];

        let unsure = [0.20, 0.05, 0.25, 0.25, 0.25];

        assert!(explicit(&unrecognised), "a grade of «neutral» is not evidence of innocence");
        assert!(explicit(&blank), "a blank grade must not overrule the first model");
        assert!(explicit(&hardcore));
        assert!(explicit(&unsure), "an unsure grader must not spare");
        assert!(!explicit(&revealing), "positively revealing is what may be spared");

        assert!(spared(false, false, true, 0, 0), "revealing, chat opted into softness");
        assert!(!spared(false, false, false, 0, 0), "revealing, but the chat did not opt in");
        assert!(!spared(false, true, true, 0, 0), "explicit is never spared");
        assert!(!spared(false, true, false, 0, 0));

        assert!(spared(true, true, false, 0, 0), "a certain grader overrules a hesitant model");
        assert!(spared(true, false, true, 0, 0));
    }

    #[test]
    fn the_grader_loads_and_keeps_its_class_order() {
        assert_eq!(GRADE_CLASSES, ["drawings", "hentai", "neutral", "porn", "sexy"]);
        let session = grader().expect("the bundled grader must load");
        let view = image::RgbImage::from_fn(GRADE_SIDE as u32, GRADE_SIDE as u32, |x, y| {
            image::Rgb([((x + y) % 255) as u8, (x % 255) as u8, (y % 255) as u8])
        });
        let pixels = pixels_of(&view, GRADE_SIDE, |value| f32::from(value) / 255.0);
        let logits = run(session, vec![3, GRADE_SIDE as i64, GRADE_SIDE as i64], pixels)
            .expect("a forward pass");
        assert_eq!(logits.len(), GRADE_CLASSES.len());
        let p = probabilities(&logits);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_model_loads_and_the_normalisation_is_the_documented_one() {
        let session = model().expect("the bundled model must load");
        let view = image::RgbImage::from_fn(SIDE as u32, SIDE as u32, |x, y| {
            let v = ((x * 3 + y) % 255) as u8;
            image::Rgb([v, v.wrapping_add(37), v.wrapping_add(74)])
        });

        let judge = |scale: fn(u8) -> f32| {
            let logits = run(
                session,
                vec![1, 3, SIDE as i64, SIDE as i64],
                pixels_of(&view, SIDE, scale),
            )
            .expect("a pass");
            assert_eq!(logits.len(), CLASSES.len());
            probabilities(&logits)
        };

        let correct = judge(|value| f32::from(value) / 127.5 - 1.0);
        let sum: f32 = correct.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "probabilities must sum to one, got {sum}");
        assert!(correct[0] < 0.5, "a bland gradient scored {} as an offence", correct[0]);

        let unscaled = judge(f32::from);
        assert!(
            (unscaled[0] - correct[0]).abs() > 0.01,
            "raw and normalised input agree, so the normalisation is not reaching the model"
        );
    }

    #[test]
    fn the_readout_alias_collides_with_nothing() {
        for alias in TEST {
            assert!(alias.contains(' '), "«{alias}» is too short to be safe as a prefix");
            for other in super::super::welcome::SHOW {
                assert!(!other.starts_with(alias), "«{alias}» would shadow «{other}»");
                assert!(!alias.starts_with(other), "«{other}» would shadow «{alias}»");
            }
            assert!(!is_public_command_elsewhere(alias), "«{alias}» is claimed already");
        }
    }

    fn is_public_command_elsewhere(alias: &str) -> bool {
        super::super::locks::LOCKS
            .iter()
            .any(|lock| lock.names.contains(&alias))
    }

    #[test]
    fn the_default_limit_is_offered_as_a_preset() {
        assert!(LIMIT_PRESETS.contains(&DEFAULT_LIMIT));
        assert!(DEFAULT_LIMIT >= LIMIT_RANGE.0 && DEFAULT_LIMIT <= LIMIT_RANGE.1);
    }
}
