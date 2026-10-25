use std::{
    io::Read,
    sync::{Mutex, Once, OnceLock},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

use super::nsfw::{self, SessionPool};

pub const DIM: usize = 768;

const MODEL_FILE: &str = "vision.onnx";

const SIDE: usize = 384;

pub const CHECKPOINT: &str = "siglip2-base-patch16-384";

pub const MODEL_SHA256: &str = "f5a9453926a4bb481b16979c1cdf29b0b65f095c5021695ff30b62ce223aff52";

const EQUIVALENT_MODEL_SHA256: &str =
    "59367a01b9d2cad897c1f7b34a6ed7d8a9f62a6c759bc6c4034be43012488015";

const ACCEPTED_MODEL_SHA256: [&str; 2] = [MODEL_SHA256, EQUIVALENT_MODEL_SHA256];

const SCALE: f32 = 127.5;

pub struct Tower {
    pool: SessionPool,
}

pub fn beside_the_binary(name: &str) -> Result<Option<std::path::PathBuf>, nsfw::ModelError> {
    if let Some(dir) = nsfw::model_files_dir() {
        let path = dir.join(name);
        return existing_path(path);
    }
    let mut path = std::env::current_exe().map_err(|error| {
        nsfw::ModelError::artifact(format!("could not locate executable for {name}: {error}"))
    })?;
    path.pop();
    path.push(name);
    existing_path(path)
}

fn existing_path(path: std::path::PathBuf) -> Result<Option<std::path::PathBuf>, nsfw::ModelError> {
    match path.try_exists() {
        Ok(true) => Ok(Some(path)),
        Ok(false) => Ok(None),
        Err(error) => Err(nsfw::ModelError::artifact(format!(
            "could not inspect {}: {error}",
            path.display()
        ))),
    }
}

fn validate_input_shape(shape: &[i64]) -> Result<(), nsfw::ModelError> {
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
    if *height != SIDE as i64 || *width != SIDE as i64 {
        return Err(nsfw::ModelError::contract(format!(
            "vision input is {height}x{width}, expected the pinned {SIDE}x{SIDE} contract"
        )));
    }
    Ok(())
}

fn validate_output_shape(shape: &[i64]) -> Result<(), nsfw::ModelError> {
    let [batch, width] = shape else {
        return Err(nsfw::ModelError::contract(format!(
            "vision output rank is {}, expected 2",
            shape.len()
        )));
    };
    if !matches!(batch, -1 | 1) || *width != DIM as i64 {
        return Err(nsfw::ModelError::contract(format!(
            "vision output is {shape:?}, expected [-1|1, {DIM}]"
        )));
    }
    Ok(())
}

fn validate_session_contract(session: &nsfw::Session) -> Result<(), nsfw::ModelError> {
    let session = session
        .lock()
        .map_err(|_| nsfw::ModelError::Poisoned("vision tower"))?;
    let [input] = session.inputs() else {
        return Err(nsfw::ModelError::contract(format!(
            "vision graph declares {} inputs, expected 1",
            session.inputs().len()
        )));
    };
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
    validate_input_shape(shape)?;

    let [output] = session.outputs() else {
        return Err(nsfw::ModelError::contract(format!(
            "vision graph declares {} outputs, expected 1",
            session.outputs().len()
        )));
    };
    let ort::value::ValueType::Tensor { ty, shape, .. } = output.dtype() else {
        return Err(nsfw::ModelError::contract(
            "vision graph output is not a tensor",
        ));
    };
    if *ty != ort::value::TensorElementType::Float32 {
        return Err(nsfw::ModelError::contract(format!(
            "vision graph output is {ty:?}, expected f32"
        )));
    }
    validate_output_shape(shape)
}

fn digest_hex(digest: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

pub(super) fn sha256_file(path: &std::path::Path) -> Result<String, nsfw::ModelError> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        nsfw::ModelError::artifact(format!("could not open {}: {error}", path.display()))
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            nsfw::ModelError::artifact(format!("could not hash {}: {error}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest_hex(&digest.finalize()))
}

fn validate_model_identity(actual: &str) -> Result<(), nsfw::ModelError> {
    if ACCEPTED_MODEL_SHA256.contains(&actual) {
        return Ok(());
    }
    Err(nsfw::ModelError::artifact(format!(
        "{MODEL_FILE} SHA-256 is {actual}, expected one of the reviewed {ACCEPTED_MODEL_SHA256:?} for {CHECKPOINT}; refusing an unreviewed embedding space"
    )))
}

fn validate_model_file(path: &std::path::Path) -> Result<(), nsfw::ModelError> {
    validate_model_identity(&sha256_file(path)?)
}

fn load_tower() -> Result<Option<Tower>, nsfw::ModelError> {
    static MISSING_WARNED: Once = Once::new();
    let path = match beside_the_binary(MODEL_FILE) {
        Ok(path) => path,
        Err(error) => {
            log::error!("vision: could not locate {MODEL_FILE}: {error}");
            return Err(error);
        }
    };
    let Some(path) = path else {
        MISSING_WARNED.call_once(|| {
            eprintln!(
                "vision: {MODEL_FILE} is not beside the binary, so the concept locks, the \
                 image filters and the NSFW arbiter are inert"
            );
        });
        return Ok(None);
    };
    if let Err(error) = validate_model_file(&path) {
        log::error!("vision: rejecting {}: {error}", path.display());
        return Err(error);
    }
    let pool = nsfw::open_path_pool(&path, "vision tower", nsfw::infer_sessions())
        .ok_or_else(|| nsfw::ModelError::Runtime("vision tower would not load".to_owned()))?;
    if let Err(error) = pool.with(validate_session_contract) {
        log::error!("vision: {MODEL_FILE} is incompatible: {error}");
        return Err(error);
    }
    Ok(Some(Tower { pool }))
}

fn tower() -> Result<Option<&'static Tower>, nsfw::ModelError> {
    static CELL: OnceLock<Tower> = OnceLock::new();
    static RETRY: Mutex<Option<(Instant, nsfw::ModelError)>> = Mutex::new(None);
    if let Some(tower) = CELL.get() {
        return Ok(Some(tower));
    }
    let mut retry = RETRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(tower) = CELL.get() {
        return Ok(Some(tower));
    }
    if let Some((at, error)) = retry.as_ref()
        && at.elapsed() < Duration::from_secs(30)
    {
        return Err(error.clone());
    }
    match load_tower() {
        Ok(Some(tower)) => {
            *retry = None;
            Ok(Some(CELL.get_or_init(|| tower)))
        }
        Ok(None) => {
            *retry = None;
            Ok(None)
        }
        Err(error) => {
            *retry = Some((Instant::now(), error.clone()));
            Err(error)
        }
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
    let side = SIDE;
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
        let head = super::super::nsfw_head::probability(&embedding);
        for (name, got, want) in [
            ("explicit", explicit, 0.020_843),
            ("head", head, 0.000_064),
        ] {
            assert!(
                (got - want).abs() < 1e-4,
                "{name}: {got} against the reference {want}"
            );
        }
        for (slot, want) in super::super::concept_head_vectors::SYNTHETIC_LOGITS
            .iter()
            .enumerate()
        {
            let got = super::super::concepts::logit(slot, &embedding);
            assert!(
                (got - want).abs() < 2e-3,
                "concept head {slot}: {got} against the reference {want}"
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
    fn model_shape_is_the_exact_reviewed_contract() {
        assert_eq!(validate_input_shape(&[1, 3, 384, 384]), Ok(()));
        assert_eq!(validate_input_shape(&[-1, 3, 384, 384]), Ok(()));
        assert!(validate_input_shape(&[1, 3, -1, -1]).is_err());
        assert!(validate_input_shape(&[1, 3, 224, 224]).is_err());
        assert!(validate_input_shape(&[1, 3, 224, 384]).is_err());
        assert!(validate_input_shape(&[1, 1, 384, 384]).is_err());
        assert!(validate_input_shape(&[1, 384, 384]).is_err());

        assert_eq!(validate_output_shape(&[-1, DIM as i64]), Ok(()));
        assert_eq!(validate_output_shape(&[1, DIM as i64]), Ok(()));
        assert!(validate_output_shape(&[1, 512]).is_err());
        assert!(validate_output_shape(&[DIM as i64]).is_err());
    }

    #[test]
    fn only_the_reviewed_encoder_identity_is_accepted() {
        const STALE_PATCH16_224: &str =
            "f81c9d7c49afd1fef6c8ef2b4fc71f529b1b02d0ab705ca783f956fc1bc3996b";
        assert_eq!(validate_model_identity(MODEL_SHA256), Ok(()));
        assert_eq!(validate_model_identity(EQUIVALENT_MODEL_SHA256), Ok(()));
        let error = validate_model_identity(STALE_PATCH16_224).expect_err("224 tower is rejected");
        assert!(matches!(error, nsfw::ModelError::Artifact(_)));
        assert!(error.to_string().contains("unreviewed embedding space"));
    }

    #[test]
    fn only_confirmed_nonexistence_is_optional() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock is after the Unix epoch")
            .as_nanos();
        let missing = std::env::temp_dir().join(format!(
            "groupbot-vision-definitely-missing-{}-{nonce}",
            std::process::id(),
        ));
        assert_eq!(existing_path(missing), Ok(None));

        let invalid = std::path::PathBuf::from("vision\0onnx");
        let error = existing_path(invalid).expect_err("lookup failure is not absence");
        assert!(matches!(error, nsfw::ModelError::Artifact(_)));
        assert!(error.to_string().contains("could not inspect"));
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
