use std::sync::OnceLock;

use super::nsfw::{self, SessionPool};

pub const DIM: usize = 768;

const MODEL_FILE: &str = "vision.onnx";

const SIDE: usize = 384;

pub const CHECKPOINT: &str = "siglip2-base-patch16-384";

const SCALE: f32 = 127.5;

pub struct Tower {
    pool: SessionPool,
    pub side: usize,
}

pub fn beside_the_binary(name: &str) -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("VISION_FILES") {
        let path = std::path::PathBuf::from(dir).join(name);
        if path.exists() {
            return Some(path);
        }
    }
    let mut path = std::env::current_exe().ok()?;
    path.pop();
    path.push(name);
    path.exists().then_some(path)
}

fn declared_side(session: &nsfw::Session) -> Option<usize> {
    let session = session.lock().ok()?;
    let input = session.inputs().first()?;
    let ort::value::ValueType::Tensor { shape, .. } = input.dtype() else {
        return None;
    };
    let [_, _, height, width] = shape[..] else {
        return None;
    };
    (height > 0 && height == width).then_some(height as usize)
}

fn tower() -> Option<&'static Tower> {
    static CELL: OnceLock<Option<Tower>> = OnceLock::new();
    CELL.get_or_init(|| {
        let Some(path) = beside_the_binary(MODEL_FILE) else {
            eprintln!(
                "vision: {MODEL_FILE} is not beside the binary, so the concept locks, the \
                 image filters and the NSFW arbiter are inert"
            );
            return None;
        };
        let pool = nsfw::open_path_pool(&path, "vision tower", nsfw::infer_sessions())?;
        let side = pool.with(declared_side).unwrap_or(SIDE);
        if side != SIDE {
            println!("vision: the tower beside the binary takes {side}px, not {SIDE}px");
        }
        Some(Tower { pool, side })
    })
    .as_ref()
}

pub fn present() -> bool {
    tower().is_some()
}

pub fn unit(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn view(image: &image::RgbImage, side: usize) -> image::RgbImage {
    image::imageops::resize(
        image,
        side as u32,
        side as u32,
        image::imageops::FilterType::Triangle,
    )
}

pub fn embed_of(image: &image::RgbImage) -> Option<Vec<f32>> {
    let tower = tower()?;
    let side = tower.side;
    let view = view(image, side);
    let pixels = nsfw::pixels_of(&view, side, |value| f32::from(value) / SCALE - 1.0);
    let out = tower.pool.with(|session| {
        nsfw::run_embedding(session, vec![1, 3, side as i64, side as i64], pixels)
    })?;
    (out.len() == DIM).then(|| unit(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs vision.onnx from an export"]
    fn the_pipeline_agrees_with_the_reference_implementation() {
        let side = SIDE as u32;
        let picture = image::RgbImage::from_fn(side, side, |x, y| {
            image::Rgb([
                ((x * 7 + y * 13) % 256) as u8,
                ((x * 3) % 256) as u8,
                ((y * 5) % 256) as u8,
            ])
        });
        let embedding = embed_of(&picture).expect("vision.onnx beside the test binary");
        assert_eq!(embedding.len(), DIM);

        let safe = dot(&embedding, &super::super::concept_vectors::SAFE);
        let explicit = dot(&embedding, &super::super::concept_vectors::EXPLICIT) - safe;
        let cigarette = dot(&embedding, &super::super::concept_vectors::CIGARETTE)
            - dot(&embedding, &super::super::concept_vectors::BACKGROUND);
        let head = super::super::nsfw_head::probability(&embedding);
        for (name, got, want) in [
            ("explicit", explicit, 0.020_843),
            ("cigarette", cigarette, -0.040_832),
            ("head", head, 0.000_064),
        ] {
            assert!(
                (got - want).abs() < 1e-4,
                "{name}: {got} against the reference {want}"
            );
        }
    }

    #[test]
    fn a_unit_vector_is_one_long() {
        let v = unit(&[3.0, 4.0]);
        assert!((dot(&v, &v) - 1.0).abs() < 1e-6);

        assert_eq!(unit(&[0.0, 0.0]), vec![0.0, 0.0]);
    }

    #[test]
    fn the_space_is_the_width_the_reservoir_assumes() {
        assert_eq!(DIM, super::super::imgfilter::DIM);
    }
}
