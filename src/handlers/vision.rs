
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

pub fn beside_the_binary(name: &str) -> Result<Option<std::path::PathBuf>, nsfw::ModelError> {
    if let Some(dir) = nsfw::model_files_dir() {
        let path = dir.join(name);
        return Ok(path.exists().then_some(path));
    }
    let mut path = std::env::current_exe().map_err(|error| {
        nsfw::ModelError::artifact(format!("could not locate executable for {name}: {error}"))
    })?;
    path.pop();
    path.push(name);
    Ok(path.exists().then_some(path))
}

fn declared_side_from_shape(shape: &[i64]) -> Result<Option<usize>, nsfw::ModelError> {
    let [batch, channels, height, width] = shape else {
        return Err(nsfw::ModelError::contract(format!(
            "vision input rank is {}, expected 4",
            shape.len()
        )));
    };
    if !matches!(batch, -1 | 1) || *channels != 3 {
        return Err(nsfw::ModelError::contract(format!(
            "vision input starts [{batch}, {channels}], expected [-1|1, 3]"
        )));
    }
    if *height <= 0 || *width <= 0 {
        return if *height <= 0 && *width <= 0 {
            Ok(None)
        } else {
            Err(nsfw::ModelError::contract(
                "vision input has only one symbolic spatial dimension",
            ))
        };
    }
    if height != width {
        return Err(nsfw::ModelError::contract(format!(
            "vision input is {height}x{width}, expected a square"
        )));
    }
    usize::try_from(*height)
        .map(Some)
        .map_err(|_| nsfw::ModelError::contract("vision input side does not fit usize"))
}

fn declared_side(session: &nsfw::Session) -> Result<Option<usize>, nsfw::ModelError> {
    let session = session
        .lock()
        .map_err(|_| nsfw::ModelError::Poisoned("vision tower"))?;
    let input = session
        .inputs()
        .first()
        .ok_or_else(|| nsfw::ModelError::contract("vision graph has no input"))?;
    let ort::value::ValueType::Tensor { ty, shape, .. } = input.dtype() else {
        return Err(nsfw::ModelError::contract(
            "vision graph input is not a tensor",
        ));
    };
    if *ty != ort::value::TensorElementType::Float32 {
        return Err(nsfw::ModelError::contract(format!(
            "vision graph input is {ty:?}, expected f32"
        )));
    }
    declared_side_from_shape(shape)
}

fn tower() -> Result<Option<&'static Tower>, nsfw::ModelError> {
    static CELL: OnceLock<Result<Option<Tower>, nsfw::ModelError>> = OnceLock::new();
    match CELL.get_or_init(|| {
        let path = match beside_the_binary(MODEL_FILE) {
            Ok(path) => path,
            Err(error) => {
                log::error!("vision: could not locate {MODEL_FILE}: {error}");
                return Err(error);
            }
        };
        let Some(path) = path else {
            eprintln!(
                "vision: {MODEL_FILE} is not beside the binary, so the concept locks, the \
                 image filters and the NSFW arbiter are inert"
            );
            return Ok(None);
        };
        let pool = nsfw::open_path_pool(&path, "vision tower", nsfw::infer_sessions())
            .ok_or_else(|| nsfw::ModelError::Runtime("vision tower would not load".to_owned()))?;
        let side = match pool.with(declared_side) {
            Ok(Some(side)) => side,
            Ok(None) => SIDE,
            Err(error) => {
                log::error!("vision: {MODEL_FILE} is incompatible: {error}");
                return Err(error);
            }
        };
        if side != SIDE {
            println!("vision: the tower beside the binary takes {side}px, not {SIDE}px");
        }
        Ok(Some(Tower { pool, side }))
    }) {
        Ok(Some(tower)) => Ok(Some(tower)),
        Ok(None) => Ok(None),
        Err(error) => Err(error.clone()),
    }
}

pub fn present() -> bool {
    matches!(tower(), Ok(Some(_)))
}

pub fn unit(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

pub(super) fn validated_unit(v: &[f32], what: &str) -> Result<Vec<f32>, nsfw::ModelError> {
    if v.iter().any(|value| !value.is_finite()) {
        return Err(nsfw::ModelError::contract(format!(
            "{what} contains a non-finite value"
        )));
    }
    let squared = v.iter().map(|value| value * value).sum::<f32>();
    if !squared.is_finite() || squared <= 0.0 {
        return Err(nsfw::ModelError::contract(format!(
            "{what} has no finite positive norm"
        )));
    }
    Ok(v.iter().map(|value| value / squared.sqrt()).collect())
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

pub fn embed_of(image: &image::RgbImage) -> Result<Option<Vec<f32>>, nsfw::ModelError> {
    let Some(tower) = tower()? else {
        return Ok(None);
    };
    let side = tower.side;
    let view = view(image, side);
    let pixels = nsfw::pixels_of(&view, side, |value| f32::from(value) / SCALE - 1.0);
    let out = tower.pool.with(|session| {
        nsfw::run_embedding(session, vec![1, 3, side as i64, side as i64], pixels)
    })?;
    if out.len() != DIM {
        return Err(nsfw::ModelError::contract(format!(
            "vision embedding has {} values, expected {DIM}",
            out.len()
        )));
    }
    validated_unit(&out, "vision embedding").map(Some)
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
        let embedding = embed_of(&picture)
            .expect("vision inference succeeds")
            .expect("vision.onnx beside the test binary");
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
    fn model_shape_accepts_only_symbolic_or_square_spatial_dimensions() {
        assert_eq!(declared_side_from_shape(&[1, 3, 384, 384]), Ok(Some(384)));
        assert_eq!(declared_side_from_shape(&[1, 3, -1, -1]), Ok(None));
        assert!(declared_side_from_shape(&[1, 3, -1, 384]).is_err());
        assert!(declared_side_from_shape(&[1, 3, 224, 384]).is_err());
        assert!(declared_side_from_shape(&[1, 1, 384, 384]).is_err());
        assert!(declared_side_from_shape(&[1, 384, 384]).is_err());
    }

    #[test]
    fn model_embeddings_need_a_finite_positive_norm() {
        assert!(validated_unit(&[0.0, 0.0], "test").is_err());
        assert!(validated_unit(&[f32::NAN, 1.0], "test").is_err());
        assert!(validated_unit(&[f32::INFINITY, 1.0], "test").is_err());
        let unit = validated_unit(&[3.0, 4.0], "test").expect("valid direction");
        assert!((dot(&unit, &unit) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_space_is_the_width_the_reservoir_assumes() {
        assert_eq!(DIM, super::super::imgfilter::DIM);
    }
}
