use std::io::Read;
use std::sync::Mutex;

use flate2::read::GzDecoder;
use image::RgbImage;
use serde::Deserialize;
use thorvg::{ColorSpace, EngineOption, Paint, Thorvg};

use super::{ANIMATED_SIGNATURE_SIDE, MAX_ANIMATED_FRAMES, sample_novel_indices};

pub(super) const MAX_COMPRESSED_BYTES: usize = 128 * 1024;
const MAX_JSON_BYTES: u64 = 2 * 1024 * 1024;
const SIDE: u32 = 512;
const MAX_FRAMES: usize = 180;
const BACKGROUND: u16 = 128;
static RENDER_LOCK: Mutex<()> = Mutex::new(());

#[derive(Deserialize)]
struct Header {
    w: u32,
    h: u32,
    fr: f32,
    ip: f32,
    op: f32,
}

fn valid_header(json: &[u8]) -> bool {
    let Ok(header) = serde_json::from_slice::<Header>(json) else {
        return false;
    };
    let frames = header.op - header.ip;
    header.w == SIDE
        && header.h == SIDE
        && header.fr.is_finite()
        && header.fr > 0.0
        && header.fr <= 60.0
        && header.ip.is_finite()
        && header.ip >= 0.0
        && frames.is_finite()
        && (1.0..=MAX_FRAMES as f32).contains(&frames)
        && frames / header.fr <= 3.0
}

fn flatten(pixels: &[u32]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(pixels.len() * 3);
    for pixel in pixels {
        let [red, green, blue, alpha] = pixel.to_le_bytes();
        let backdrop = BACKGROUND * u16::from(255 - alpha) / 255;
        for channel in [red, green, blue] {
            rgb.push((u16::from(channel) + backdrop).min(255) as u8);
        }
    }
    rgb
}

pub(super) fn decode(bytes: &[u8]) -> Option<Vec<RgbImage>> {
    if bytes.len() > MAX_COMPRESSED_BYTES {
        return None;
    }
    let mut json = Vec::new();
    GzDecoder::new(bytes)
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut json)
        .ok()?;
    if json.len() as u64 > MAX_JSON_BYTES || !valid_header(&json) {
        return None;
    }

    let _render = RENDER_LOCK.lock().ok()?;
    let engine = Thorvg::init().ok()?;
    let mut animation = engine.lottie_animation().ok()?;
    animation.load_data(&json).ok()?;
    let total = animation.total_frame().ok()?;
    if !total.is_finite() || !(1.0..=MAX_FRAMES as f32).contains(&total) {
        return None;
    }
    let total = total.ceil() as usize;

    let signature_side = ANIMATED_SIGNATURE_SIDE;
    animation
        .set_size(signature_side as f32, signature_side as f32)
        .ok()?;
    let mut signature_pixels = vec![0u32; (signature_side * signature_side) as usize];
    let mut signature_canvas = engine.sw_canvas(EngineOption::Default).ok()?;
    unsafe {
        signature_canvas
            .set_target(
                &mut signature_pixels,
                signature_side,
                signature_side,
                signature_side,
                ColorSpace::ABGR8888,
            )
            .ok()?;
    }
    let mut signatures = Vec::with_capacity(total);
    for at in 0..total {
        if at != 0 {
            animation.set_frame(at as f32).ok()?;
        }
        signature_canvas.clear().ok()?;
        signature_canvas
            .add(animation.picture().duplicate()?)
            .ok()?;
        signature_canvas.render().ok()?;
        signatures.push(flatten(&signature_pixels));
    }
    drop(signature_canvas);

    let wanted = sample_novel_indices(&signatures, MAX_ANIMATED_FRAMES);
    if wanted.is_empty() {
        return None;
    }
    animation.set_size(SIDE as f32, SIDE as f32).ok()?;
    let mut pixels = vec![0u32; (SIDE * SIDE) as usize];
    let mut canvas = engine.sw_canvas(EngineOption::Default).ok()?;
    unsafe {
        canvas
            .set_target(&mut pixels, SIDE, SIDE, SIDE, ColorSpace::ABGR8888)
            .ok()?;
    }
    let mut frames = Vec::with_capacity(wanted.len());
    for at in wanted {
        if animation.frame().ok()? != at as f32 {
            animation.set_frame(at as f32).ok()?;
        }
        canvas.clear().ok()?;
        canvas.add(animation.picture().duplicate()?).ok()?;
        canvas.render().ok()?;
        frames.push(RgbImage::from_raw(SIDE, SIDE, flatten(&pixels))?);
    }
    Some(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_TGS: &[u8] =
        include_bytes!("../../../tests/fixtures/media/telegram-animated-sticker.tgs");
    const REAL_TGS_2: &[u8] =
        include_bytes!("../../../tests/fixtures/media/telegram-animated-sticker-2.tgs");
    const REAL_TGS_3: &[u8] =
        include_bytes!("../../../tests/fixtures/media/telegram-animated-sticker-3.tgs");

    #[test]
    fn official_telegram_fixture_renders_distinct_bounded_frames() {
        let frames = decode(REAL_TGS).expect("render Telegram animated sticker");
        assert_eq!(frames.len(), MAX_ANIMATED_FRAMES);
        assert!(
            frames
                .iter()
                .all(|frame| frame.dimensions() == (SIDE, SIDE))
        );
        assert!(frames.iter().any(|frame| frame != &frames[0]));
        assert_ne!(frames[0].get_pixel(256, 256).0, [128, 128, 128]);
        let small = image::imageops::resize(
            &frames[0],
            ANIMATED_SIGNATURE_SIDE,
            ANIMATED_SIGNATURE_SIDE,
            image::imageops::FilterType::Nearest,
        );
        let stretched =
            image::imageops::resize(&small, SIDE, SIDE, image::imageops::FilterType::Nearest);
        let mean_detail = frames[0]
            .as_raw()
            .iter()
            .zip(stretched.as_raw())
            .map(|(left, right)| f64::from(left.abs_diff(*right)))
            .sum::<f64>()
            / frames[0].as_raw().len() as f64;
        assert!(mean_detail > 1.0, "512px redraw lost detail: {mean_detail}");
    }

    #[test]
    fn three_native_renderers_can_run_concurrently() {
        let workers = [REAL_TGS, REAL_TGS_2, REAL_TGS_3].map(|fixture| {
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                let frames = decode(fixture).expect("concurrent TGS render");
                (frames.len(), started.elapsed())
            })
        });
        for worker in workers {
            let (frames, elapsed) = worker.join().unwrap();
            assert_eq!(frames, MAX_ANIMATED_FRAMES);
            eprintln!("TGS 180-frame scan and 12 full renders: {elapsed:?}");
        }
    }

    #[test]
    fn native_engine_survives_repeated_init_and_drop() {
        for fixture in [REAL_TGS, REAL_TGS_2, REAL_TGS_3]
            .into_iter()
            .cycle()
            .take(10)
        {
            assert_eq!(decode(fixture).unwrap().len(), MAX_ANIMATED_FRAMES);
        }
    }

    #[test]
    fn invalid_and_oversized_tgs_cannot_reach_native_renderer() {
        assert!(decode(b"not a gzip stream").is_none());
        assert!(decode(&vec![0; MAX_COMPRESSED_BYTES + 1]).is_none());
        let too_large_json = vec![b' '; MAX_JSON_BYTES as usize + 1];
        let mut zipped = Vec::new();
        {
            use std::io::Write;
            let mut encoder =
                flate2::write::GzEncoder::new(&mut zipped, flate2::Compression::fast());
            encoder.write_all(&too_large_json).unwrap();
            encoder.finish().unwrap();
        }
        assert!(decode(&zipped).is_none());
    }
}
