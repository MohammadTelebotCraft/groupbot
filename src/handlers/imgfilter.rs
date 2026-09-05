use std::sync::Arc;

use grammers_client::message::Message;
use grammers_client::session::types::PeerRef;

use super::concept_vectors as vectors;
use super::vision::{dot, unit};
use super::{Ctx, esc};

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
}

pub const TEXT_ADD: &[&str] = &["فیلتر متنی"];

pub const PHOTO_ADD: &[&str] = &["قفل تصویر"];
pub const SAMPLE_ADD: &[&str] = &["فیلتر این", "فیلتر نمونه"];
pub const REMOVE: &[&str] = &["حذف فیلتر تصویری", "حذف فیلتر عکس"];

pub const CALIBRATE: &[&str] = &[];

pub const TUNE: &[&str] = &[];
const LIVE_WORD: &[&str] = &["زنده", "فعال"];
const SHADOW_WORD: &[&str] = &["خاموش", "آزمایشی"];
const RATE_WORD: &[&str] = &["حساسیت"];

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

fn uncalibrated_cut(samples: u32) -> f32 {
    match samples {
        0 => FIXED_MODEL_CUT,
        _ => FIXED_EXAMPLE_CUT,
    }
}

pub fn worst<'a>(filters: &'a [Filter], margins: &[f32]) -> Option<(&'a Filter, f32)> {
    filters
        .iter()
        .zip(margins)
        .filter(|(filter, m)| **m >= filter.cut)
        .max_by(|a, b| (a.1 - a.0.cut).total_cmp(&(b.1 - b.0.cut)))
        .map(|(filter, m)| (filter, *m))
}

fn report(chat: i64, id: i64, filters: &[Filter], margins: &[f32], cached: bool) {
    let matched = worst(filters, margins).map_or("none", |(filter, _)| filter.name.as_str());
    log::info!(
        "imgfilter: chat {chat} file {id} decision={}{}",
        matched,
        if cached { " cached" } else { "" }
    );
}

pub fn cached_margins(ctx: &Ctx, id: i64, filters: &[Filter], animated: bool) -> Option<Vec<f32>> {
    filters
        .iter()
        .map(|filter| ctx.known_custom(id, filter.print, animated))
        .collect()
}

pub fn score_all(
    ctx: &Ctx,
    id: i64,
    filters: &[Filter],
    embedding: &[f32],
    from_animation: bool,
) -> Vec<f32> {
    filters
        .iter()
        .map(|filter| {
            let m = margin(embedding, &filter.vector);
            ctx.remember_custom(id, filter.print, m, from_animation);
            m
        })
        .collect()
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
    let Some((filter, _)) = worst(filters, margins) else {
        return;
    };
    if !filter.live {
        return;
    }
    if let Err(e) = message.delete().await {
        eprintln!("imgfilter: could not delete in {chat}: {e}");
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    let chances = match super::strict::punish(ctx, message, chat, CAUSE).await {
        super::strict::Outcome::Announced => return,
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::notice::send(ctx, message, chat, &format!("تصویر {}", filter.name), chances).await;
}

#[allow(clippy::too_many_arguments)]
pub async fn act_detached(
    ctx: &Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    id: i64,
    filters: &[Filter],
    margins: &[f32],
    sender: Option<i64>,
    name: &str,
) {
    report(chat, id, filters, margins, false);
    let Some((filter, _)) = worst(filters, margins) else {
        return;
    };
    if !filter.live {
        return;
    }
    match ctx.client.delete_messages(chat_ref, &[message_id]).await {
        Ok(0) => {
            eprintln!("imgfilter: delete affected nothing in {chat} msg {message_id}");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("imgfilter: could not delete in {chat} msg {message_id}: {e}");
            return;
        }
    }
    ctx.bump(chat, super::stats::DELETED);
    super::nsfw::punish_and_notify(
        ctx,
        chat,
        chat_ref,
        sender,
        name,
        CAUSE,
        &format!("تصویر {}", filter.name),
    )
    .await;
}

#[derive(Debug, PartialEq)]
enum Ask<'a> {
    FromText(&'a str),

    FromPhoto(&'a str),

    FromExample(&'a str),
    Remove(&'a str),
    Recalibrate(&'a str),
    Live(&'a str, bool),
    Rate(&'a str, u32),
}

fn after<'a>(text: &'a str, aliases: &[&str]) -> Option<&'a str> {
    for alias in aliases {
        let Some(rest) = text.strip_prefix(alias) else {
            continue;
        };

        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return Some(rest.trim());
        }
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
    if let Some(rest) = after(text, SAMPLE_ADD) {
        return (!rest.is_empty()).then_some(Ask::FromExample(rest));
    }
    if let Some(rest) = after(text, REMOVE) {
        return (!rest.is_empty()).then_some(Ask::Remove(rest));
    }
    if let Some(rest) = after(text, CALIBRATE) {
        return (!rest.is_empty()).then_some(Ask::Recalibrate(rest));
    }

    let rest = after(text, TUNE)?;
    let (head, tail) = rest.rsplit_once(char::is_whitespace)?;
    let (head, tail) = (head.trim(), tail.trim());
    if head.is_empty() {
        return None;
    }
    if LIVE_WORD.contains(&tail) {
        return Some(Ask::Live(head, true));
    }
    if SHADOW_WORD.contains(&tail) {
        return Some(Ask::Live(head, false));
    }

    let number = super::numbers_in(tail)?;
    let [rate] = number[..] else {
        return None;
    };
    let (name, word) = head.rsplit_once(char::is_whitespace)?;
    RATE_WORD
        .contains(&word.trim())
        .then(|| Ask::Rate(name.trim(), rate))
        .filter(|_| !name.trim().is_empty())
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
        Ask::FromText(phrase) => from_text(ctx, message, chat, phrase, false).await,
        Ask::FromPhoto(phrase) => from_text(ctx, message, chat, phrase, true).await,
        Ask::FromExample(name) => from_example(ctx, message, chat, name).await,
        Ask::Remove(name) => remove(ctx, message, chat, name).await,
        Ask::Recalibrate(name) => recalibrate(ctx, message, chat, name, None).await,
        Ask::Live(name, live) => set_live(ctx, message, chat, name, live).await,
        Ask::Rate(name, rate) => recalibrate(ctx, message, chat, name, Some(rate)).await,
    }
    true
}

async fn embed_reply(ctx: &Arc<Ctx>, chat: i64, reply: &Message) -> Result<Vec<f32>, &'static str> {
    let Some(media) = reply.media() else {
        return Err("روی یک عکس، گیف، استیکر یا فیلم ریپلای کنید.");
    };
    let (Some(id), Some(ladder)) = (super::nsfw::file_id(&media), super::nsfw::ladder(&media))
    else {
        return Err("این رسانه تامبنیلی که بشود بررسی کرد ندارد.");
    };
    let Some(bytes) = super::nsfw::fetch(ctx, chat, id, &ladder, ladder.start).await else {
        return Err("تامبنیل دانلود نشد.");
    };

    let _slot = ctx.nsfw_slot().await;
    let Ok(Some(image)) = tokio::task::spawn_blocking(move || {
        image::load_from_memory(&bytes)
            .ok()
            .map(|image| image.to_rgb8())
    })
    .await
    else {
        return Err("تصویر باز نشد.");
    };
    match tokio::task::spawn_blocking(move || super::vision::embed_of(&image)).await {
        Ok(Some(embedding)) => Ok(embedding),
        _ => Err("مدل تصویر در دسترس نیست."),
    }
}

pub enum FilterError {
    Invalid,
    Full,
    ModelUnavailable,
}

pub async fn create_from_phrase(ctx: &Arc<Ctx>, chat: i64, phrase: &str) -> Result<(), FilterError> {
    if !acceptable(phrase) {
        return Err(FilterError::Invalid);
    }
    if full(ctx, chat, phrase) {
        return Err(FilterError::Full);
    }
    let phrase_owned = phrase.to_owned();

    let slot = ctx.nsfw_slot().await;
    let vector = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        super::imgtext::embed(&phrase_owned)
    })
    .await
    .ok()
    .flatten();
    let Some(vector) = vector else {
        return Err(FilterError::ModelUnavailable);
    };
    if save_core(ctx, chat, phrase, &unit(&vector), 0, DEFAULT_RATE).await {
        Ok(())
    } else {
        Err(FilterError::Full)
    }
}

async fn from_text(ctx: &Arc<Ctx>, message: &Message, chat: i64, phrase: &str, _activate: bool) {
    match create_from_phrase(ctx, chat, phrase).await {
        Ok(()) => {
            let _ = message
                .reply(grammers_client::message::InputMessage::new().html(format!(
                    "<b>فیلتر تصویری</b>\n\nنام · <b>{}</b>\nساخت · از روی متن\n\nحالت · فعال\nتصمیم · خودکار",
                    esc(phrase),
                )))
                .await;
        }
        Err(FilterError::Invalid) => {
            let _ = message
                .reply(format!(
                    "این نام پذیرفته نمی شود: تا {MAX_NAME} حرف، بدون : و = و <."
                ))
                .await;
        }
        Err(FilterError::Full) => {
            let _ = message
                .reply(format!("لیست فیلتر تصویری پر است ({MAX_FILTERS} مورد)."))
                .await;
        }
        Err(FilterError::ModelUnavailable) => {
            let _ = message
                .reply(
                    "مدل عمومی تصویر روی این سرور نصب نیست، پس فیلتر متنی و «قفل تصویر» کار نمی کنند.\n\
                     با ریپلای روی یک عکس و «فیلتر این ‹نام›» می شود از روی نمونه فیلتر ساخت.",
                )
                .await;
        }
    }
}

async fn from_example(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str) {
    if !acceptable(name) {
        let _ = message
            .reply(format!(
                "این نام پذیرفته نمی شود: تا {MAX_NAME} حرف، بدون : و = و <."
            ))
            .await;
        return;
    }
    if full(ctx, chat, name) {
        let _ = message
            .reply(format!("لیست فیلتر تصویری پر است ({MAX_FILTERS} مورد)."))
            .await;
        return;
    }
    let Ok(Some(reply)) = message.get_reply().await else {
        let _ = message
            .reply("روی عکسی که می خواهید فیلتر شود ریپلای کنید و «فیلتر این ‹نام›» بنویسید.")
            .await;
        return;
    };
    let embedding = match embed_reply(ctx, chat, &reply).await {
        Ok(embedding) => embedding,
        Err(why) => {
            let _ = message.reply(why).await;
            return;
        }
    };

    let existing = ctx.settings.image_filter(chat, name).await;
    let (vector, samples) = match &existing {
        Some(row) if row.samples > 0 && row.samples < MAX_SAMPLES => {
            let old = dequantize(&row.vector, row.scale);
            let weight = row.samples as f32;
            let mixed: Vec<f32> = old
                .iter()
                .zip(&embedding)
                .map(|(o, n)| (o * weight + n) / (weight + 1.0))
                .collect();
            (unit(&mixed), row.samples + 1)
        }
        Some(row) if row.samples >= MAX_SAMPLES => {
            let _ = message
                .reply(format!(
                    "«{}» از قبل {MAX_SAMPLES} نمونه دارد و بیشتر از این دقیق تر نمی شود.",
                    esc(name)
                ))
                .await;
            return;
        }
        _ => (unit(&embedding), 1),
    };
    let rate = existing.map_or(DEFAULT_RATE, |row| row.rate);
    save(ctx, message, chat, name, &vector, samples, rate, false).await;
}

fn full(ctx: &Ctx, chat: i64, name: &str) -> bool {
    let names = names(ctx, chat);
    names.len() >= MAX_FILTERS && !names.iter().any(|known| known == name)
}

async fn save_core(ctx: &Arc<Ctx>, chat: i64, name: &str, vector: &[f32], samples: u32, rate: u32) -> bool {
    let (bytes, scale) = quantize(vector);
    let cut = uncalibrated_cut(samples);
    let saved = ctx
        .settings
        .save_image_filter(chat, name, &bytes, scale, cut, rate, true, samples, true)
        .await;
    if !saved {
        return false;
    }
    ctx.settings.set(chat, &key(name), true).await;
    ctx.forget_image_filters(chat);
    true
}

#[allow(clippy::too_many_arguments)]
async fn save(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    name: &str,
    vector: &[f32],
    samples: u32,
    rate: u32,
    _activate: bool,
) {
    if !save_core(ctx, chat, name, vector, samples, rate).await {
        let _ = message
            .reply(format!(
                "✗ فیلتر ذخیره نشد؛ هر گروه حداکثر {} فیلتر دارد.",
                MAX_FILTERS
            ))
            .await;
        return;
    }

    let how = match samples {
        0 => "از روی متن".to_owned(),
        1 => "با ۱ نمونه".to_owned(),
        n => format!("با {n} نمونه"),
    };

    let state = "حالت · فعال\nتصمیم · خودکار";
    let _ = message
        .reply(grammers_client::message::InputMessage::new().html(format!(
            "<b>فیلتر تصویری</b>\n\nنام · <b>{}</b>\nساخت · {how}\n\n{state}",
            esc(name),
            how = esc(&how),
        )))
        .await;
}

async fn remove(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str) {
    let Some(found) = names(ctx, chat).into_iter().find(|known| known == name) else {
        let _ = message
            .reply(format!("✗ فیلتری با نام «{}» نیست.", esc(name)))
            .await;
        return;
    };
    ctx.settings.set(chat, &key(&found), false).await;
    ctx.settings.delete_image_filter(chat, &found).await;
    ctx.forget_image_filters(chat);
    let _ = message
        .reply(format!("✓ «{}» از فیلتر تصویری حذف شد.", esc(&found)))
        .await;
}

async fn set_live(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str, live: bool) {
    let Some(row) = ctx.settings.image_filter(chat, name).await else {
        let _ = message
            .reply(format!("✗ فیلتری با نام «{}» نیست.", esc(name)))
            .await;
        return;
    };

    if live && row.cut >= f32::MAX {
        let _ = message
            .reply(format!(
                "✗ «{}» هنوز واسنجی نشده. اول «کالیبره فیلتر {}» را بفرستید.",
                esc(name),
                esc(name)
            ))
            .await;
        return;
    }
    ctx.settings
        .save_image_filter(
            chat,
            name,
            &row.vector,
            row.scale,
            row.cut,
            row.rate,
            live,
            row.samples,
            row.calibrated,
        )
        .await;
    ctx.forget_image_filters(chat);
    let _ = message
        .reply(match live {
            true => format!("✓ «{}» فعال شد.", esc(name)),
            false => format!("✗ «{}» به حالت بررسی بدون حذف برگشت.", esc(name)),
        })
        .await;
}

async fn recalibrate(ctx: &Arc<Ctx>, message: &Message, chat: i64, name: &str, rate: Option<u32>) {
    let Some(row) = ctx.settings.image_filter(chat, name).await else {
        let _ = message
            .reply(format!("✗ فیلتری با نام «{}» نیست.", esc(name)))
            .await;
        return;
    };
    let rate = rate.unwrap_or(row.rate).clamp(RATE_RANGE.0, RATE_RANGE.1);
    let vector = dequantize(&row.vector, row.scale);
    let samples = ctx.samples();
    let Some(found) = calibrate(&vector, &samples, rate) else {
        let _ = message
            .reply(format!(
                "هنوز به اندازه کافی تصویر دیده نشده · {} از {MIN_SAMPLES}.\n\
                 کمی بعد دوباره بفرستید.",
                samples.len()
            ))
            .await;
        return;
    };
    ctx.settings
        .save_image_filter(
            chat,
            name,
            &row.vector,
            row.scale,
            found.cut,
            rate,
            row.live,
            row.samples,
            true,
        )
        .await;
    ctx.forget_image_filters(chat);
    let _ = message
        .reply(grammers_client::message::InputMessage::new().html(format!(
            "<b>واسنجی</b>\n\nفیلتر · <b>{}</b>\nنرخ خطا · یک در <b>{rate}</b>\n\
             آستانه · <code>{:+.4}</code>\nنمونه · {} تصویر · میانگین <code>{:+.4}</code> · \
             پراکندگی <code>{:.4}</code>",
            esc(name),
            found.cut,
            found.samples,
            found.mean,
            found.sd
        )))
        .await;
}

pub async fn listing(ctx: &Ctx, chat: i64) -> Vec<(String, String)> {
    let filters = ctx.image_filters(chat).await;
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
    out
}

pub enum Armed {
    Toggled,
    Missing,
}

pub async fn toggle_live(ctx: &Ctx, chat: i64, id: &str) -> Armed {
    let Some(name) = names(ctx, chat)
        .into_iter()
        .find(|known| super::lists::word_id(known) == id)
    else {
        return Armed::Missing;
    };
    let Some(row) = ctx.settings.image_filter(chat, &name).await else {
        return Armed::Missing;
    };
    let live = !row.live;
    let cut = if row.samples == 0 {
        FIXED_MODEL_CUT
    } else {
        FIXED_EXAMPLE_CUT
    };
    ctx.settings
        .save_image_filter(
            chat,
            &name,
            &row.vector,
            row.scale,
            cut,
            row.rate,
            live,
            row.samples,
            true,
        )
        .await;
    ctx.forget_image_filters(chat);
    Armed::Toggled
}

pub async fn panel_rows(ctx: &Ctx, chat: i64) -> Vec<(String, bool, bool)> {
    let filters = ctx.image_filters(chat).await;
    let mut out: Vec<(String, bool, bool)> = filters
        .iter()
        .map(|filter| (filter.name.clone(), filter.live, true))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub async fn forget(ctx: &Ctx, chat: i64, name: &str) {
    ctx.settings.set(chat, &key(name), false).await;
    ctx.settings.delete_image_filter(chat, name).await;
    ctx.forget_image_filters(chat);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_form() {
        assert_eq!(
            parse("فیلتر متنی خودرو لوکس"),
            Some(Ask::FromText("خودرو لوکس"))
        );
        assert_eq!(parse("قفل تصویر اسب"), Some(Ask::FromPhoto("اسب")));
        assert_eq!(parse("فیلتر این ورزشی"), Some(Ask::FromExample("ورزشی")));
        assert_eq!(parse("فیلتر نمونه ورزشی"), Some(Ask::FromExample("ورزشی")));
        assert_eq!(parse("حذف فیلتر تصویری ورزشی"), Some(Ask::Remove("ورزشی")));
    }

    #[test]
    fn declines_what_is_not_a_command() {
        assert_eq!(parse("فیلتر شکن"), None);
        assert_eq!(parse("فیلترها"), None);
        assert_eq!(parse("فیلتر متنی"), None);
        assert_eq!(parse("قفل تصویر"), None);
        assert_eq!(parse("قفل تصویری"), None);
        assert_eq!(parse("فیلتر این"), None);
        assert_eq!(parse("کالیبره فیلتر ورزشی"), None);
        assert_eq!(parse("تست فیلتر"), None);
        assert_eq!(parse("فیلتر ورزشی زنده"), None);
        assert_eq!(parse("فیلتر ورزشی حساسیت ۵۰۰"), None);
        assert_eq!(parse("سلام"), None);

        assert_eq!(parse("فیلتر ورزشی چطوره"), None);
    }

    #[test]
    fn the_only_alias_that_needs_dispatch_order_is_the_documented_one() {
        let ours = [TEXT_ADD, PHOTO_ADD, SAMPLE_ADD, REMOVE, CALIBRATE, TUNE].concat();
        let theirs: Vec<&str> =
            [super::super::filters::ADD, super::super::filters::REMOVE].concat();
        let mut needs_order: Vec<&str> = Vec::new();
        for alias in &ours {
            for other in &theirs {
                if alias.starts_with(other)
                    && *alias != *other
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
            vec!["حذف فیلتر تصویری", "حذف فیلتر عکس"],
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
        };
        let prototype = Filter {
            name: "نمونه".to_owned(),
            vector: vec![0.0; DIM],
            cut: 0.40,
            live: true,
            print: 2,
        };

        let filters = vec![phrase, prototype];
        let (won, _) = worst(&filters, &[0.05, 0.30]).expect("one of them is over");
        assert_eq!(won.name, "متنی");

        assert!(worst(&filters, &[0.01, 0.30]).is_none());
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
    fn the_strings_carry_no_half_space() {
        for text in ["فیلتر تصویری", "بررسی بدون حذف", "هنوز واسنجی نشده"]
        {
            assert!(!text.contains('\u{200c}'));
        }
    }
}
