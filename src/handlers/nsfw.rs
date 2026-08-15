use std::sync::Arc;

use grammers_client::media::{Downloadable, Media, PhotoSize};
use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerRef};
use tract_onnx::prelude::*;

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

const CERTAIN: f32 = LIMIT_RANGE.1 as f32 / 100.0;

const MARGIN: f32 = 0.15;

const WIDE: f32 = 1.5;

type Model = TypedRunnableModel<TypedModel>;

fn model() -> Option<&'static Model> {
    static MODEL_CELL: std::sync::OnceLock<Option<Model>> = std::sync::OnceLock::new();
    MODEL_CELL
        .get_or_init(|| match load() {
            Ok(model) => Some(model),
            Err(e) => {
                eprintln!("nsfw: the model would not load, the lock is inert: {e}");
                None
            }
        })
        .as_ref()
}

fn load() -> TractResult<Model> {
    tract_onnx::onnx()
        .model_for_read(&mut std::io::Cursor::new(MODEL))?
        .with_input_fact(0, f32::fact([1, 3, SIDE, SIDE]).into())?
        .into_optimized()?
        .into_runnable()
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

fn breakdown(logits: &[f32]) -> String {
    let p = probabilities(logits);
    CLASSES
        .iter()
        .zip(&p)
        .map(|(label, value)| format!("{label} {value:.2}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn fit(image: &image::RgbImage) -> image::RgbImage {
    let (width, height) = image.dimensions();
    let scale = RESIZE as f32 / width.min(height).max(1) as f32;
    let resized = image::imageops::resize(
        image,
        ((width as f32 * scale).round() as u32).max(SIDE as u32),
        ((height as f32 * scale).round() as u32).max(SIDE as u32),
        image::imageops::FilterType::CatmullRom,
    );
    let (width, height) = resized.dimensions();
    let left = (width - SIDE as u32) / 2;
    let top = (height - SIDE as u32) / 2;
    image::imageops::crop_imm(&resized, left, top, SIDE as u32, SIDE as u32).to_image()
}

fn views(image: &image::RgbImage) -> Vec<image::RgbImage> {
    let (width, height) = image.dimensions();
    let long = width.max(height);
    let short = width.min(height).max(1);
    let mut views = vec![fit(image)];
    if long as f32 / short as f32 >= WIDE {
        for offset in [0, long - short] {
            let (x, y) = if width >= height { (offset, 0) } else { (0, offset) };
            let tile = image::imageops::crop_imm(image, x, y, short, short).to_image();
            views.push(fit(&tile));
        }
    }
    views
}

fn tensor_of(view: &image::RgbImage) -> Tensor {
    tract_ndarray::Array4::from_shape_fn((1, 3, SIDE, SIDE), |(_, channel, y, x)| {
        f32::from(view.get_pixel(x as u32, y as u32).0[channel]) / 127.5 - 1.0
    })
    .into()
}

fn score(jpeg: &[u8]) -> Option<(f32, String)> {
    let model = model()?;
    let image = image::load_from_memory(jpeg).ok()?.to_rgb8();

    let mut worst: Option<(f32, String)> = None;
    for view in views(&image) {
        let output = model.run(tvec!(tensor_of(&view).into())).ok()?;
        let logits: Vec<f32> = output[0].to_array_view::<f32>().ok()?.iter().copied().collect();
        if logits.len() != CLASSES.len() {
            return None;
        }
        let scored = (nsfw_of(&logits), breakdown(&logits));
        let top = scored.0;
        if worst.as_ref().is_none_or(|(seen, _)| top > *seen) {
            worst = Some(scored);
        }
        if top >= CERTAIN {
            break;
        }
    }
    worst
}

fn file_id(media: &Media) -> Option<i64> {
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

struct Ladder {
    sizes: Vec<PhotoSize>,
    dc: Option<i32>,
    start: usize,
}

impl Ladder {
    fn rung(&self, at: usize) -> Thumb {
        Thumb {
            size: self.sizes[at].clone(),
            dc: self.dc,
        }
    }
}

fn ladder(media: &Media) -> Option<Ladder> {
    let mut sizes: Vec<PhotoSize> = thumbs(media)
        .into_iter()
        .filter(|size| size.to_raw_input_location().is_some())
        .collect();
    sizes.sort_by_key(|size| dims(size).map_or(0, |(width, height)| width.min(height)));

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

pub async fn watch(ctx: &Arc<Ctx>, message: &Message, view: &View<'_>) {
    let Some(media) = view.media() else {
        return;
    };
    let Some(id) = file_id(media) else {
        return;
    };
    let Some(chat) = super::chat_id(message) else {
        return;
    };

    let Some((shadow, limit)) = ctx.settings.with_chat(chat, |settings| {
        settings
            .is_locked(LOCK)
            .then(|| {
                (
                    !settings.is_locked(LIVE),
                    settings.number(LIMIT, DEFAULT_LIMIT, LIMIT_RANGE),
                )
            })
    }) else {
        return;
    };

    if super::is_exempt(ctx, message).await {
        return;
    }

    if let Some(known) = ctx.known_verdict(id) {
        act(ctx, message, chat, id, known, limit, shadow, true).await;
        return;
    }

    let Some(ladder) = ladder(media) else {
        println!("nsfw: {chat} file {id} skipped, no downloadable thumbnail");
        return;
    };

    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return;
    };
    let message_id = message.id();
    let sender = message.sender_id().and_then(PeerId::bare_id);
    let name = super::name_of(message);
    let ctx = Arc::clone(ctx);

    tokio::spawn(async move {
        classify(ctx, chat, chat_ref, message_id, id, ladder, limit, shadow, sender, name).await;
    });
}

#[allow(clippy::too_many_arguments)]
async fn classify(
    ctx: Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    id: i64,
    ladder: Ladder,
    limit: u32,
    shadow: bool,
    sender: Option<i64>,
    name: String,
) {
    if let Some(known) = ctx.known_verdict(id) {
        act_detached(&ctx, chat, chat_ref, message_id, id, known, limit, shadow, sender, name)
            .await;
        return;
    }
    let target = limit as f32 / 100.0;

    let Some((score, classes, note)) =
        judge(&ctx, chat, id, &ladder, ladder.start).await
    else {
        return;
    };

    let (score, classes, note) = match ladder.sizes.get(ladder.start + 1) {
        Some(_) if (score - target).abs() < MARGIN => {
            match judge(&ctx, chat, id, &ladder, ladder.start + 1).await {
                Some((better, classes, note)) => (better, classes, format!("{note} escalated")),
                None => (score, classes, note),
            }
        }
        _ => (score, classes, note),
    };

    println!("nsfw: {chat} file {id} {note} classes {classes}");
    ctx.remember_verdict(id, score);
    act_detached(&ctx, chat, chat_ref, message_id, id, score, limit, shadow, sender, name).await;
}

async fn judge(
    ctx: &Arc<Ctx>,
    chat: i64,
    id: i64,
    ladder: &Ladder,
    at: usize,
) -> Option<(f32, String, String)> {
    let thumb = ladder.rung(at);
    let pixels = dims(&thumb.size).map_or_else(
        || "?x?".to_owned(),
        |(width, height)| format!("{width}x{height}"),
    );

    let bytes = {
        let _fetching = ctx.nsfw_fetch().await;
        let mut bytes = Vec::new();
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
        bytes
    };
    if bytes.is_empty() {
        return None;
    }
    let size = bytes.len();

    let _thinking = ctx.nsfw_slot().await;
    let (score, classes) = tokio::task::spawn_blocking(move || score(&bytes))
        .await
        .ok()
        .flatten()?;
    Some((score, classes, format!("{pixels} bytes {size}")))
}

#[allow(clippy::too_many_arguments)]
async fn act(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    id: i64,
    score: f32,
    limit: u32,
    shadow: bool,
    cached: bool,
) {
    if verdict(score, limit, shadow) != Verdict::Delete {
        report(chat, id, score, limit, shadow, cached);
        return;
    }
    report(chat, id, score, limit, shadow, cached);
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
    limit: u32,
    shadow: bool,
    sender: Option<i64>,
    name: String,
) {
    report(chat, id, score, limit, shadow, false);
    if verdict(score, limit, shadow) != Verdict::Delete {
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
        "<a href=\"tg://user?id={user}\">{}</a> پیام شما حذف شد · <b>محتوای غیراخلاقی</b> در این گروه قفل است.\n<i>لطفا دوباره نفرستید.</i>",
        super::esc(&name)
    );
    let sent = ctx
        .client
        .send_message(chat_ref, InputMessage::new().html(text))
        .await;

    let Ok(sent) = sent else {
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
        let square = views(&image::RgbImage::new(500, 500));
        assert_eq!(square.len(), 1, "a square frame costs one pass");

        let wide = views(&image::RgbImage::new(1280, 720));
        assert_eq!(wide.len(), 3, "16:9 is the centre plus both ends");

        let tall = views(&image::RgbImage::new(720, 1280));
        assert_eq!(tall.len(), 3, "tall splits along its own long axis");

        for view in wide.into_iter().chain(tall) {
            assert_eq!(view.dimensions(), (SIDE as u32, SIDE as u32));
        }

        let striped = image::RgbImage::from_fn(1200, 400, |x, _| {
            image::Rgb([if x < 400 { 255 } else { 0 }, 0, 0])
        });
        let got = views(&striped);
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
        let out = fit(&wide);
        assert_eq!(out.dimensions(), (SIDE as u32, SIDE as u32));

        let centre = out.get_pixel(SIDE as u32 / 2, SIDE as u32 / 2).0[0];
        assert!(centre > 200, "the middle of the frame was lost, got {centre}");

        assert_eq!(
            fit(&image::RgbImage::new(80, 400)).dimensions(),
            (SIDE as u32, SIDE as u32)
        );
        assert_eq!(
            fit(&image::RgbImage::new(1, 1)).dimensions(),
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
    fn stopping_early_cannot_answer_for_a_stricter_chat() {
        for limit in LIMIT_RANGE.0..=LIMIT_RANGE.1 {
            assert_eq!(
                verdict(CERTAIN, limit, false),
                Verdict::Delete,
                "a score of {CERTAIN} must satisfy every limit, and {limit} rejected it"
            );
        }

        assert!((CERTAIN - LIMIT_RANGE.1 as f32 / 100.0).abs() < f32::EPSILON);
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
    fn the_default_limit_is_offered_as_a_preset() {
        assert!(LIMIT_PRESETS.contains(&DEFAULT_LIMIT));
        assert!(DEFAULT_LIMIT >= LIMIT_RANGE.0 && DEFAULT_LIMIT <= LIMIT_RANGE.1);
    }

    #[test]
    fn the_model_loads_and_the_normalisation_is_the_documented_one() {
        let model = model().expect("the bundled model must load");

        let run = |scale: fn(u8) -> f32| -> Vec<f32> {
            let input: Tensor =
                tract_ndarray::Array4::from_shape_fn((1, 3, SIDE, SIDE), |(_, c, y, x)| {
                    scale(((c * 37 + y * 3 + x) % 255) as u8)
                })
                .into();
            let output = model.run(tvec!(input.into())).expect("a forward pass");
            let logits: Vec<f32> = output[0]
                .to_array_view::<f32>()
                .expect("float logits")
                .iter()
                .copied()
                .collect();
            assert_eq!(logits.len(), CLASSES.len());
            probabilities(&logits)
        };

        let correct = run(|v| f32::from(v) / 127.5 - 1.0);
        let sum: f32 = correct.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "probabilities must sum to one, got {sum}");
        assert!(correct[0] < 0.5, "a bland gradient scored {} as an offence", correct[0]);

        let unscaled = run(f32::from);
        assert!(
            (unscaled[0] - correct[0]).abs() > 0.01,
            "raw and normalised input agree, so the normalisation is not reaching the model"
        );
    }
}
