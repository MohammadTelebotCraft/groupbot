use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::handlers::{CONCEPT_SLOTS, concepts, imgfilter, imgtext, nsfw, ocr, vision};

const INPUT_BYTES_MAX: u64 = 16 * 1024 * 1024;
const ANIMATED_INPUT_BYTES_MAX: u64 = nsfw::ANIMATED_BYTES_MAX as u64;
const REQUEST_BYTES_MAX: u64 = 16 * 1024;

const IMAGE_FILTER_PHRASES: [&str; 10] = [
    "tench",
    "English springer",
    "cassette player",
    "chain saw",
    "church",
    "French horn",
    "garbage truck",
    "gas pump",
    "golf ball",
    "parachute",
];

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Pipeline {
    #[default]
    Nsfw,
    Advert,
    Concepts,
    ImageFilter,
    ExampleFilter,
    ExampleFilterThree,
    ExampleFilterCandidates,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MediaKind {
    Animated,
    Tgs,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    media_path: PathBuf,
    #[serde(default)]
    pipeline: Pipeline,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    spare: bool,
    #[serde(default)]
    ocr_profile: Option<String>,
    #[serde(default)]
    include_text: bool,
    #[serde(default)]
    media_kind: Option<MediaKind>,
}

fn valid_limit(request: &Request) -> bool {
    match request.pipeline {
        Pipeline::Concepts => {
            let limit = request.limit.unwrap_or(concepts::DEFAULT_LIMIT);
            (concepts::LIMIT_RANGE.0..=concepts::LIMIT_RANGE.1).contains(&limit)
        }
        Pipeline::Nsfw | Pipeline::Advert => {
            let limit = request.limit.unwrap_or(nsfw::DEFAULT_LIMIT);
            (10..=95).contains(&limit)
        }
        Pipeline::ImageFilter
        | Pipeline::ExampleFilter
        | Pipeline::ExampleFilterThree
        | Pipeline::ExampleFilterCandidates => request.limit.is_none(),
    }
}

fn configure_models() -> Result<(), &'static str> {
    static CONFIG: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(nsfw::configure_from_environment)
        .as_ref()
        .map(|_| ())
        .map_err(|_| "model_configuration")
}

fn concept_choice(
    margins: &[f32; CONCEPT_SLOTS],
    limit: u32,
) -> Result<(&'static concepts::Concept, f32, bool), &'static str> {
    let armed = concepts::Armed {
        concepts: concepts::CONCEPTS.iter().collect(),
        limit,
        live: true,
    };
    let (winner, margin) = concepts::worst(margins, &armed).ok_or("model_contract")?;
    Ok((winner, margin, concepts::over(margin, limit)))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn read_media(path: &std::path::Path, max_bytes: u64) -> Result<Vec<u8>, &'static str> {
    let file = std::fs::File::open(path).map_err(|_| "media_unreadable")?;
    let metadata = file.metadata().map_err(|_| "media_unreadable")?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err("media_size_or_type");
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "media_unreadable")?;
    if bytes.len() as u64 > max_bytes {
        return Err("media_size_or_type");
    }
    Ok(bytes)
}

fn decode_still(bytes: &[u8]) -> Result<image::RgbImage, &'static str> {
    if nsfw::is_animated_raster_bytes(bytes) {
        return Err("animation_not_supported");
    }
    nsfw::decode_still(bytes).map_err(|_| "media_decode")
}

fn load_image(path: &std::path::Path) -> Result<image::RgbImage, &'static str> {
    decode_still(&read_media(path, INPUT_BYTES_MAX)?)
}

fn looks_like_video(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(b"ftyp") || bytes.starts_with(b"\x1a\x45\xdf\xa3")
}

fn looks_like_tgs(bytes: &[u8], path: &std::path::Path) -> bool {
    bytes.starts_with(b"\x1f\x8b")
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tgs"))
}

enum LoadedMedia {
    Still(image::RgbImage),
    Animated(Vec<image::RgbImage>),
}

fn animation_runtime() -> Result<&'static tokio::runtime::Runtime, &'static str> {
    static RUNTIME: std::sync::OnceLock<Result<tokio::runtime::Runtime, ()>> =
        std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| ())
        })
        .as_ref()
        .map_err(|_| "decoder_runtime")
}

fn load_media(request: &Request) -> Result<LoadedMedia, &'static str> {
    let tgs_name = request
        .media_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tgs"));
    let max_bytes = if matches!(request.media_kind, Some(MediaKind::Tgs)) || tgs_name {
        nsfw::TGS_BYTES_MAX as u64
    } else {
        ANIMATED_INPUT_BYTES_MAX
    };
    let bytes = read_media(&request.media_path, max_bytes)?;
    decode_media_bytes(request, bytes)
}

fn decode_media_bytes(request: &Request, bytes: Vec<u8>) -> Result<LoadedMedia, &'static str> {
    nsfw::raster_kind_full(&bytes).ok_or("media_decode")?;
    let is_tgs = matches!(request.media_kind, Some(MediaKind::Tgs))
        || (request.media_kind.is_none() && looks_like_tgs(&bytes, &request.media_path));
    if is_tgs {
        if bytes.len() > nsfw::TGS_BYTES_MAX {
            return Err("media_size_or_type");
        }
        return nsfw::decode_tgs_frames_offline(&bytes)
            .map(LoadedMedia::Animated)
            .ok_or("animation_decode");
    }
    let animated = matches!(request.media_kind, Some(MediaKind::Animated))
        || nsfw::is_animated_raster_bytes(&bytes)
        || looks_like_video(&bytes);
    if animated {
        let frames = std::thread::spawn(move || {
            animation_runtime()?
                .block_on(nsfw::decode_animated_frames(bytes))
                .ok_or("animation_decode")
        })
        .join()
        .map_err(|_| "decoder_runtime")??;
        return Ok(LoadedMedia::Animated(frames));
    }
    if bytes.len() as u64 > INPUT_BYTES_MAX {
        return Err("media_size_or_type");
    }
    decode_still(&bytes).map(LoadedMedia::Still)
}

fn model_error(error: nsfw::ModelError) -> &'static str {
    match error {
        nsfw::ModelError::Artifact(_) => "model_artifact",
        nsfw::ModelError::Runtime(_) => "model_runtime",
        nsfw::ModelError::ModelContract(_) => "model_contract",
        nsfw::ModelError::Poisoned(_) => "model_poisoned",
    }
}

type LearnedPhrases = Vec<(&'static str, Vec<f32>, f32)>;

fn image_filter_learned() -> Result<&'static LearnedPhrases, &'static str> {
    static LEARNED: std::sync::OnceLock<LearnedPhrases> = std::sync::OnceLock::new();
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    imgtext::initialize_once_retry(&LEARNED, &GATE, || -> Result<Option<_>, &'static str> {
        let directions = IMAGE_FILTER_PHRASES
            .iter()
            .map(|&phrase| {
                let text = match imgtext::try_embed(phrase).map_err(model_error)? {
                    imgtext::EmbedOutcome::Embedding(vector) => vector,
                    imgtext::EmbedOutcome::Unavailable | imgtext::EmbedOutcome::Empty => {
                        return Err("model_artifact");
                    }
                };
                let learned = imgfilter::learn(&text);
                let (bytes, scale) = imgfilter::quantize(&learned.vector);
                Ok((phrase, imgfilter::dequantize(&bytes, scale), learned.cut))
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        Ok(Some(directions))
    })?
    .ok_or("model_artifact")
}

fn image_filter_directions() -> Result<&'static Vec<(&'static str, Vec<f32>)>, &'static str> {
    static DIRECTIONS: std::sync::OnceLock<Vec<(&'static str, Vec<f32>)>> =
        std::sync::OnceLock::new();
    let learned = image_filter_learned()?;
    Ok(DIRECTIONS.get_or_init(|| {
        learned
            .iter()
            .map(|(phrase, vector, _)| (*phrase, vector.clone()))
            .collect()
    }))
}

fn filter_cuts(
    directions: &[(&'static str, Vec<f32>)],
    samples: u32,
) -> Result<Vec<f32>, &'static str> {
    if samples == 0 {
        return Ok(image_filter_learned()?.iter().map(|(_, _, cut)| *cut).collect());
    }
    Ok(directions
        .iter()
        .map(|(_, vector)| imgfilter::bank_cut(vector, imgfilter::rate_for(samples)))
        .collect())
}

fn example_filter_directions(
    examples: usize,
) -> Result<&'static Vec<(&'static str, Vec<f32>)>, &'static str> {
    type Directions = Vec<(&'static str, Vec<f32>)>;
    static ONE: std::sync::OnceLock<Directions> = std::sync::OnceLock::new();
    static THREE: std::sync::OnceLock<Directions> = std::sync::OnceLock::new();
    static ONE_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static THREE_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let (cell, gate) = if examples == 1 {
        (&ONE, &ONE_GATE)
    } else {
        (&THREE, &THREE_GATE)
    };
    imgtext::initialize_once_retry(cell, gate, || -> Result<Option<_>, &'static str> {
        let directions = IMAGE_FILTER_PHRASES
            .iter()
            .enumerate()
            .map(|(index, &name)| {
                let mut saved: Option<(Vec<u8>, f32)> = None;
                for sample in 0..examples {
                    let path = format!(
                        "target/imgfilter-eval/imagenette/{index:03}_{}/{sample:05}.jpg",
                        name.replace(' ', "_")
                    );
                    let source = load_image(std::path::Path::new(&path))?;
                    let embedding = vision::embed_of(&source)
                        .map_err(model_error)?
                        .ok_or("model_artifact")?;
                    let direction = match &saved {
                        Some((bytes, scale)) => {
                            let old = imgfilter::dequantize(bytes, *scale);
                            let weight = sample as f32;
                            let mixed: Vec<f32> = old
                                .iter()
                                .zip(&embedding)
                                .map(|(previous, next)| (previous * weight + next) / (weight + 1.0))
                                .collect();
                            vision::unit(&mixed)
                        }
                        None => vision::unit(&embedding),
                    };
                    saved = Some(imgfilter::quantize(&direction));
                }
                let (bytes, scale) = saved.ok_or("model_contract")?;
                Ok((name, vision::unit(&imgfilter::dequantize(&bytes, scale))))
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        Ok(Some(directions))
    })?
    .ok_or("model_artifact")
}

type Directions = Vec<(&'static str, Vec<Vec<f32>>)>;

fn individual_example_directions() -> Result<&'static Directions, &'static str> {
    static INDIVIDUAL: std::sync::OnceLock<Directions> = std::sync::OnceLock::new();
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    imgtext::initialize_once_retry(&INDIVIDUAL, &GATE, || -> Result<Option<_>, &'static str> {
        let directions = IMAGE_FILTER_PHRASES
            .iter()
            .enumerate()
            .map(|(index, &name)| {
                let mut directions = Vec::with_capacity(3);
                for sample in 0..3 {
                    let path = format!(
                        "target/imgfilter-eval/imagenette/{index:03}_{}/{sample:05}.jpg",
                        name.replace(' ', "_")
                    );
                    let source = load_image(std::path::Path::new(&path))?;
                    let embedding = vision::embed_of(&source)
                        .map_err(model_error)?
                        .ok_or("model_artifact")?;
                    let (bytes, scale) = imgfilter::quantize(&vision::unit(&embedding));
                    directions.push(vision::unit(&imgfilter::dequantize(&bytes, scale)));
                }
                Ok((name, directions))
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        Ok(Some(directions))
    })?
    .ok_or("model_artifact")
}

fn evaluate_still(
    request: &Request,
    image: image::RgbImage,
    decode_ms: f64,
) -> Result<(Value, Value), &'static str> {
    match request.pipeline {
        Pipeline::Nsfw => {
            configure_models()?;
            let result = nsfw::evaluate_image_offline(
                &image,
                request.limit.unwrap_or(nsfw::DEFAULT_LIMIT),
                request.spare,
            )
            .map_err(model_error)?;
            let mut stages = json!({"decode":decode_ms,"classifier":result.classifier_ms});
            if let Some(ms) = result.grader_ms {
                stages["grader"] = json!(ms);
            }
            if let Some(ms) = result.arbiter_ms {
                stages["arbiter"] = json!(ms);
            }
            let prediction = json!({
                "predicted_label": if result.would_delete { "nsfw" } else { "safe" },
                "decision": if result.would_delete { "ACTION" } else { "ALLOW" },
                "stage_ms": stages,
                "model_version": if result.fitted_head.is_some() {
                    "marqo384-mobilenetv4-siglip2base384-fittedhead"
                } else if result.arbiter_requested {
                    "marqo384-mobilenetv4-arbiter-absent"
                } else {
                    "marqo384-mobilenetv4-arbiter-not-requested"
                },
            });
            Ok((
                prediction,
                serde_json::to_value(result).map_err(|_| "serialization")?,
            ))
        }
        Pipeline::Advert => {
            let started = Instant::now();
            let text = match request.ocr_profile.as_deref() {
                Some(name) => ocr::read_with_profile(
                    &image,
                    ocr::OcrProfile::parse(name).ok_or("invalid_ocr_profile")?,
                ),
                None => ocr::read(&image),
            }
            .map_err(model_error)?;
            let ocr_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            let classification = ocr::classify(&text);
            let classify_ms = started.elapsed().as_secs_f64() * 1000.0;
            let (label, decision) = match classification.decision {
                ocr::Decision::Allow => ("legitimate", "ALLOW"),
                ocr::Decision::Suspicious => ("suspicious", "UNCERTAIN"),
                ocr::Decision::Block => ("advert", "ACTION"),
            };
            let mut evidence = json!({
                "width":image.width(), "height":image.height(),
                "text_characters":text.chars().count(),
                "ocr_profile":request.ocr_profile.as_deref().and_then(ocr::OcrProfile::parse).map(|p| p.name()).unwrap_or("production"),
                "reasons":classification.reasons.iter().map(|reason| reason.as_str()).collect::<Vec<_>>(),
            });
            if request.include_text {
                evidence["text"] = json!(text);
            }
            Ok((
                json!({
                    "predicted_label":label, "decision":decision,
                    "stage_ms":{"decode":decode_ms,"ocr":ocr_ms,"classify":classify_ms},
                "model_version":format!("tesseract-pinned-fas-eng-{}", request.ocr_profile.as_deref().unwrap_or("production")),
                }),
                evidence,
            ))
        }
        Pipeline::Concepts => {
            configure_models()?;
            let started = Instant::now();
            let embedding = vision::embed_of(&image)
                .map_err(model_error)?
                .ok_or("model_artifact")?;
            let embedding_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            let margins = concepts::margins_from(&embedding);
            let concepts_ms = started.elapsed().as_secs_f64() * 1000.0;
            let limit = request.limit.unwrap_or(concepts::DEFAULT_LIMIT);
            let (winner, winning_margin, action) = concept_choice(&margins, limit)?;
            let by_concept = concepts::CONCEPTS
                .iter()
                .enumerate()
                .map(|(slot, concept)| json!({"key":concept.key,"margin":margins[slot]}))
                .collect::<Vec<_>>();
            Ok((
                json!({
                    "predicted_label":if action { winner.key } else { "safe" },
                    "decision":if action { "ACTION" } else { "ALLOW" },
                    "stage_ms":{"decode":decode_ms,"embedding":embedding_ms,"concepts":concepts_ms},
                    "model_version":format!("{}-concepts", vision::CHECKPOINT),
                }),
                json!({
                    "width":image.width(),"height":image.height(),
                    "limit":limit,"winning_concept":winner.key,
                    "winning_margin":winning_margin,"margins":by_concept,
                }),
            ))
        }
        Pipeline::ImageFilter | Pipeline::ExampleFilter | Pipeline::ExampleFilterThree => {
            configure_models()?;
            let started = Instant::now();
            let (directions, kind, samples) = match request.pipeline {
                Pipeline::ImageFilter => (image_filter_directions()?, "custom-text", 0),
                Pipeline::ExampleFilter => (example_filter_directions(1)?, "custom-example", 1),
                Pipeline::ExampleFilterThree => {
                    (example_filter_directions(3)?, "custom-example-three", 3)
                }
                _ => unreachable!(),
            };
            let cuts = filter_cuts(directions, samples)?;
            let setup_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            let embedding = vision::embed_of(&image)
                .map_err(model_error)?
                .ok_or("model_artifact")?;
            let embedding_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            let mut margins = Vec::with_capacity(directions.len());
            let mut winner = ("", f32::MIN);
            for ((phrase, vector), cut) in directions.iter().zip(&cuts) {
                let margin = imgfilter::margin(&embedding, vector);
                margins.push(json!({"phrase":phrase,"margin":margin,"cut":cut}));
                if margin - cut > winner.1 {
                    winner = (*phrase, margin - cut);
                }
            }
            let score_ms = started.elapsed().as_secs_f64() * 1000.0;
            let action = winner.1 >= 0.0;
            Ok((
                json!({
                    "predicted_label":if action { winner.0 } else { "safe" },
                    "decision":if action { "ACTION" } else { "ALLOW" },
                    "stage_ms":{"decode":decode_ms,"filter_setup":setup_ms,"embedding":embedding_ms,"score":score_ms},
                    "model_version":format!("{}-{kind}", vision::CHECKPOINT),
                }),
                json!({
                    "width":image.width(),"height":image.height(),
                    "margins":margins,
                }),
            ))
        }
        Pipeline::ExampleFilterCandidates => {
            configure_models()?;
            let started = Instant::now();
            let averaged = example_filter_directions(3)?;
            let individual = individual_example_directions()?;
            let setup_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            let embedding = vision::embed_of(&image)
                .map_err(model_error)?
                .ok_or("model_artifact")?;
            let embedding_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            let mut margins = Vec::with_capacity(averaged.len());
            let mut winner = ("", f32::MIN);
            for ((phrase, average), (individual_phrase, directions)) in
                averaged.iter().zip(individual)
            {
                if phrase != individual_phrase || directions.len() != 3 {
                    return Err("model_contract");
                }
                let live_average = imgfilter::margin(&embedding, average);
                let per_example = directions
                    .iter()
                    .map(|direction| imgfilter::margin(&embedding, direction))
                    .collect::<Vec<_>>();
                let mut sorted = per_example.clone();
                sorted.sort_by(|a, b| b.total_cmp(a));
                let per_example_max = sorted[0];
                let top_two_mean = (sorted[0] + sorted[1]) / 2.0;
                margins.push(json!({
                    "phrase":phrase, "live_average":live_average,
                    "per_example":per_example, "per_example_max":per_example_max,
                    "top_two_mean":top_two_mean,
                }));
                if live_average > winner.1 {
                    winner = (*phrase, live_average);
                }
            }
            let score_ms = started.elapsed().as_secs_f64() * 1000.0;
            let cut = imgfilter::FIXED_EXAMPLE_CUT;
            let action = winner.1 >= cut;
            Ok((
                json!({
                    "predicted_label":if action { winner.0 } else { "safe" },
                    "decision":if action { "ACTION" } else { "ALLOW" },
                    "stage_ms":{"decode":decode_ms,"filter_setup":setup_ms,"embedding":embedding_ms,"score":score_ms},
                    "model_version":format!("{}-custom-example-candidates", vision::CHECKPOINT),
                }),
                json!({
                    "width":image.width(),"height":image.height(),
                    "cut":cut,"margins":margins,
                }),
            ))
        }
    }
}

fn evaluate_animated(
    request: &Request,
    frames: Vec<image::RgbImage>,
    decode_ms: f64,
) -> Result<(Value, Value, usize), &'static str> {
    match request.pipeline {
        Pipeline::Nsfw => {
            configure_models()?;
            let started = Instant::now();
            let limit = request.limit.unwrap_or(nsfw::DEFAULT_LIMIT);
            let scored = nsfw::evaluate_animated_visual_offline(
                frames,
                true,
                false,
                Vec::new(),
                limit,
                request.spare,
            )
            .map_err(model_error)?;
            let visual_ms = started.elapsed().as_secs_f64() * 1000.0;
            let action = scored.nsfw_would_delete.ok_or("model_contract")?;
            let count = scored.frames_scored;
            let evidence = serde_json::to_value(&scored).map_err(|_| "serialization")?;
            Ok((
                json!({
                    "predicted_label":if action { "nsfw" } else { "safe" },
                    "decision":if action { "ACTION" } else { "ALLOW" },
                    "stage_ms":{"decode":decode_ms,"visual":visual_ms},
                    "model_version":"marqo384-mobilenetv4-animated",
                }),
                json!({"limit":limit,"spare":request.spare,"visual":evidence}),
                count,
            ))
        }
        Pipeline::Advert => {
            if request.ocr_profile.is_some() || request.include_text {
                return Err("animation_ocr_options_unsupported");
            }
            let started = Instant::now();
            let result = nsfw::evaluate_animated_advert_offline(&frames).map_err(model_error)?;
            let ocr_ms = started.elapsed().as_secs_f64() * 1000.0;
            let (label, decision) = match result.decision {
                "allow" => ("legitimate", "ALLOW"),
                "suspicious" => ("suspicious", "UNCERTAIN"),
                "block" => ("advert", "ACTION"),
                _ => return Err("model_contract"),
            };
            let count = result.ocr_frames_read;
            Ok((
                json!({
                    "predicted_label":label,"decision":decision,
                    "stage_ms":{"decode":decode_ms,"ocr":ocr_ms},
                    "model_version":"tesseract-pinned-fas-eng-production-animated",
                }),
                serde_json::to_value(result).map_err(|_| "serialization")?,
                count,
            ))
        }
        Pipeline::Concepts => {
            configure_models()?;
            let started = Instant::now();
            let scored = nsfw::evaluate_animated_visual_offline(
                frames,
                false,
                true,
                Vec::new(),
                nsfw::DEFAULT_LIMIT,
                false,
            )
            .map_err(model_error)?;
            let visual_ms = started.elapsed().as_secs_f64() * 1000.0;
            let margins = scored.concept_margins.ok_or("model_contract")?;
            let limit = request.limit.unwrap_or(concepts::DEFAULT_LIMIT);
            let (winner, winning_margin, action) = concept_choice(&margins, limit)?;
            let by_concept = concepts::CONCEPTS
                .iter()
                .enumerate()
                .map(|(slot, concept)| json!({"key":concept.key,"margin":margins[slot]}))
                .collect::<Vec<_>>();
            Ok((
                json!({
                    "predicted_label":if action { winner.key } else { "safe" },
                    "decision":if action { "ACTION" } else { "ALLOW" },
                    "stage_ms":{"decode":decode_ms,"visual":visual_ms},
                    "model_version":format!("{}-concepts-animated", vision::CHECKPOINT),
                }),
                json!({
                    "sampled_frames":scored.frames_scored,"limit":limit,
                    "winning_concept":winner.key,"winning_margin":winning_margin,
                    "margins":by_concept,
                }),
                scored.frames_scored,
            ))
        }
        Pipeline::ImageFilter | Pipeline::ExampleFilter | Pipeline::ExampleFilterThree => {
            configure_models()?;
            let (directions, kind, samples) = match request.pipeline {
                Pipeline::ImageFilter => (image_filter_directions()?, "custom-text", 0),
                Pipeline::ExampleFilter => (example_filter_directions(1)?, "custom-example", 1),
                Pipeline::ExampleFilterThree => {
                    (example_filter_directions(3)?, "custom-example-three", 3)
                }
                _ => unreachable!(),
            };
            let cuts = filter_cuts(directions, samples)?;
            let specs = directions
                .iter()
                .zip(&cuts)
                .map(|((_, vector), cut)| (vector.clone(), *cut))
                .collect();
            let started = Instant::now();
            let scored = nsfw::evaluate_animated_visual_offline(
                frames,
                false,
                false,
                specs,
                nsfw::DEFAULT_LIMIT,
                false,
            )
            .map_err(model_error)?;
            let visual_ms = started.elapsed().as_secs_f64() * 1000.0;
            let margins = scored.custom_margins.ok_or("model_contract")?;
            if margins.len() != directions.len() {
                return Err("model_contract");
            }
            let mut winner = ("", f32::MIN);
            let by_filter = directions
                .iter()
                .zip(&margins)
                .zip(&cuts)
                .map(|(((phrase, _), &margin), &cut)| {
                    if margin - cut > winner.1 {
                        winner = (*phrase, margin - cut);
                    }
                    json!({"phrase":phrase,"margin":margin,"cut":cut})
                })
                .collect::<Vec<_>>();
            let action = winner.1 >= 0.0;
            Ok((
                json!({
                    "predicted_label":if action { winner.0 } else { "safe" },
                    "decision":if action { "ACTION" } else { "ALLOW" },
                    "stage_ms":{"decode":decode_ms,"visual":visual_ms},
                    "model_version":format!("{}-{kind}-animated", vision::CHECKPOINT),
                }),
                json!({"sampled_frames":scored.frames_scored,"margins":by_filter}),
                scored.frames_scored,
            ))
        }
        Pipeline::ExampleFilterCandidates => {
            configure_models()?;
            let averaged = example_filter_directions(3)?;
            let individual = individual_example_directions()?;
            let cut = imgfilter::FIXED_EXAMPLE_CUT;
            let mut specs = Vec::with_capacity(averaged.len() * 4);
            for ((phrase, average), (individual_phrase, vectors)) in averaged.iter().zip(individual)
            {
                if phrase != individual_phrase || vectors.len() != 3 {
                    return Err("model_contract");
                }
                specs.push((average.clone(), cut));
                specs.extend(vectors.iter().cloned().map(|vector| (vector, cut)));
            }
            let started = Instant::now();
            let scored = nsfw::evaluate_animated_visual_offline(
                frames,
                false,
                false,
                specs,
                nsfw::DEFAULT_LIMIT,
                false,
            )
            .map_err(model_error)?;
            let visual_ms = started.elapsed().as_secs_f64() * 1000.0;
            let margins = scored.custom_margins.ok_or("model_contract")?;
            if margins.len() != averaged.len() * 4 {
                return Err("model_contract");
            }
            let mut winner = ("", f32::MIN);
            let mut by_filter = Vec::with_capacity(averaged.len());
            for (index, (phrase, _)) in averaged.iter().enumerate() {
                let values = &margins[index * 4..index * 4 + 4];
                let average = values[0];
                let per_example = values[1..].to_vec();
                let mut sorted = per_example.clone();
                sorted.sort_by(|a, b| b.total_cmp(a));
                by_filter.push(json!({
                    "phrase":phrase,"live_average":average,"per_example":per_example,
                    "per_example_max":sorted[0],"top_two_mean":(sorted[0]+sorted[1])/2.0,
                }));
                if average > winner.1 {
                    winner = (*phrase, average);
                }
            }
            let action = winner.1 >= cut;
            Ok((
                json!({
                    "predicted_label":if action { winner.0 } else { "safe" },
                    "decision":if action { "ACTION" } else { "ALLOW" },
                    "stage_ms":{"decode":decode_ms,"visual":visual_ms},
                    "model_version":format!("{}-custom-example-candidates-animated", vision::CHECKPOINT),
                }),
                json!({"sampled_frames":scored.frames_scored,"cut":cut,"margins":by_filter}),
                scored.frames_scored,
            ))
        }
    }
}

fn evaluate(request: &Request) -> Result<(Value, Value, usize, bool), &'static str> {
    let decode_started = Instant::now();
    let media = load_media(request)?;
    let decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
    match media {
        LoadedMedia::Still(image) => evaluate_still(request, image, decode_ms)
            .map(|(prediction, evidence)| (prediction, evidence, 1, false)),
        LoadedMedia::Animated(frames) => evaluate_animated(request, frames, decode_ms)
            .map(|(prediction, evidence, count)| (prediction, evidence, count, true)),
    }
}

fn process(raw: &[u8]) -> Value {
    let started = Instant::now();
    let request = serde_json::from_slice::<Request>(raw);
    let id = request
        .as_ref()
        .ok()
        .filter(|r| valid_id(&r.id))
        .map(|r| r.id.as_str());
    let outcome = match &request {
        Ok(request) if valid_id(&request.id) && valid_limit(request) => evaluate(request),
        _ => Err("invalid_request"),
    };
    let (mut prediction, evidence, frames_processed, animated) = match outcome {
        Ok(result) => result,
        Err(code) => (
            json!({"decision":"ERROR","error":code}),
            Value::Null,
            0,
            false,
        ),
    };
    prediction["id"] = json!(id);
    prediction["latency_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
    prediction["pipeline_version"] = json!(if animated {
        "rust-animation-v1"
    } else {
        "rust-still-v1"
    });
    prediction["cache_hit"] = json!(false);
    if !evidence.is_null() {
        prediction["frames_processed"] = json!(frames_processed);
    }
    json!({"id":id, "prediction":prediction, "evidence":evidence})
}

pub fn run() -> crate::Result {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    loop {
        let mut raw = Vec::new();
        let count = (&mut input)
            .take(REQUEST_BYTES_MAX + 1)
            .read_until(b'\n', &mut raw)?;
        if count == 0 {
            return Ok(());
        }
        if count as u64 > REQUEST_BYTES_MAX {
            return Err("media evaluation request exceeds 16 KiB".into());
        }
        if raw.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        serde_json::to_writer(&mut output, &process(&raw))?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_labels_and_unknown_fields() {
        let output = process(br#"{"id":"sample-1","media_path":"missing.png","label":"nsfw"}"#);
        assert_eq!(output["prediction"]["error"], "invalid_request");
        assert!(output["prediction"].get("predicted_label").is_none());
    }

    #[test]
    fn errors_are_not_clean_predictions_and_do_not_leak_paths() {
        let output = process(br#"{"id":"sample-2","media_path":"/private/missing/image.jpg"}"#);
        assert_eq!(output["prediction"]["decision"], "ERROR");
        assert!(output["prediction"].get("frames_processed").is_none());
        assert!(!output.to_string().contains("private"));
    }

    #[test]
    fn request_contract_is_bounded() {
        assert!(valid_id("sample-1_a.b"));
        assert!(!valid_id(""));
        assert!(!valid_id("private/path"));
        assert!(!valid_id(&"a".repeat(129)));
        assert_eq!(
            process(br#"{"id":"s","media_path":"x","limit":0}"#)["prediction"]["error"],
            "invalid_request"
        );
    }

    #[test]
    fn concept_requests_use_the_concept_limit_range() {
        let parse = |limit: Option<u32>| {
            serde_json::from_value::<Request>(json!({
                "id":"sample", "media_path":"missing.png",
                "pipeline":"concepts", "limit":limit,
            }))
            .expect("concept pipeline is supported")
        };
        assert!(valid_limit(&parse(None)));
        assert!(valid_limit(&parse(Some(concepts::LIMIT_RANGE.0))));
        assert!(valid_limit(&parse(Some(concepts::LIMIT_RANGE.1))));
        assert!(!valid_limit(&parse(Some(concepts::LIMIT_RANGE.0 - 1))));
        assert!(!valid_limit(&parse(Some(concepts::LIMIT_RANGE.1 + 1))));
        let output =
            process(br#"{"id":"sample","media_path":"missing.png","pipeline":"concepts"}"#);
        assert_eq!(output["prediction"]["decision"], "ERROR");
        assert_eq!(output["prediction"]["error"], "media_unreadable");
        assert!(output["prediction"].get("predicted_label").is_none());
    }

    #[test]
    fn concept_diagnostic_arms_every_builtin_and_uses_the_worst_margin() {
        assert_eq!(concepts::CONCEPTS.len(), 6);
        let mut margins = [f32::MIN; CONCEPT_SLOTS];
        margins[0] = 0.019;
        margins[5] = 0.021;
        let (winner, margin, action) = concept_choice(&margins, concepts::DEFAULT_LIMIT).unwrap();
        assert_eq!(winner.key, concepts::CONCEPTS[5].key);
        assert_eq!(margin, 0.021);
        assert!(action);
    }

    #[test]
    fn animation_detection_reads_chunks_not_compressed_payload_strings() {
        assert!(nsfw::is_animated_raster_bytes(include_bytes!(
            "../tests/fixtures/media/two-frame.apng"
        )));
        assert!(nsfw::is_animated_raster_bytes(include_bytes!(
            "../tests/fixtures/media/two-frame.webp"
        )));
        assert!(!nsfw::is_animated_raster_bytes(include_bytes!(
            "../tests/fixtures/media/ad-retail-model-number.png"
        )));
        assert!(looks_like_video(b"\0\0\0\x18ftypisom"));
        assert!(looks_like_video(b"\x1a\x45\xdf\xa3\0\0\0\0"));
    }

    #[test]
    fn animated_images_decode_multiple_distinct_frames_before_policy_scoring() {
        for (name, expected) in [("two-frame.apng", 2), ("two-frame.webp", 2)] {
            let request: Request = serde_json::from_value(json!({
                "id":"animation-test",
                "media_path":format!(
                    "{}/tests/fixtures/media/{name}",
                    env!("CARGO_MANIFEST_DIR")
                ),
            }))
            .unwrap();
            let LoadedMedia::Animated(frames) = load_media(&request).unwrap() else {
                panic!("{name} must reach the animated decoder");
            };
            assert_eq!(frames.len(), expected, "{name}");
            assert_ne!(frames[0], frames[1], "{name}");
        }
    }

    #[tokio::test]
    async fn animation_decode_works_from_the_bot_tokio_main() {
        let request: Request = serde_json::from_value(json!({
            "id":"tokio-main",
            "media_path":format!(
                "{}/tests/fixtures/media/two-frame.apng",
                env!("CARGO_MANIFEST_DIR")
            ),
        }))
        .unwrap();
        let LoadedMedia::Animated(frames) = load_media(&request).unwrap() else {
            panic!("APNG must reach frame scanning");
        };
        assert_eq!(frames.len(), 2);
    }

    #[test]
    fn malformed_raster_headers_are_errors_even_with_an_animation_hint() {
        let request: Request = serde_json::from_value(json!({
            "id":"malformed-raster", "media_path":"ignored.webp", "media_kind":"animated",
        }))
        .unwrap();
        let mut webp = include_bytes!("../tests/fixtures/media/two-frame.webp").to_vec();
        webp[4] ^= 1;
        assert!(nsfw::raster_kind_full(&webp).is_none());
        assert!(matches!(
            decode_media_bytes(&request, webp),
            Err("media_decode")
        ));

        let mut apng = include_bytes!("../tests/fixtures/media/two-frame.apng").to_vec();
        apng[29] ^= 1;
        assert!(nsfw::raster_kind_full(&apng).is_none());
        assert!(matches!(
            decode_media_bytes(&request, apng),
            Err("media_decode")
        ));
    }

    #[test]
    fn gif_missing_its_trailer_cannot_be_reported_clean_after_frame_zero() {
        let partial = include_bytes!("../tests/fixtures/media/partial-after-frame0.gif");
        assert_ne!(partial.last(), Some(&0x3b));
        let request: Request = serde_json::from_value(json!({
            "id":"partial-gif", "media_path":"partial.gif", "media_kind":"animated",
        }))
        .unwrap();
        assert!(matches!(
            decode_media_bytes(&request, partial.to_vec()),
            Err("animation_decode")
        ));
    }

    #[test]
    fn partial_videos_cannot_look_clean_from_their_safe_first_frame() {
        for (name, partial) in [
            (
                "mp4",
                include_bytes!("../tests/fixtures/media/partial-after-frame0.mp4").as_slice(),
            ),
            (
                "webm",
                include_bytes!("../tests/fixtures/media/partial-after-frame0.webm").as_slice(),
            ),
        ] {
            let request: Request = serde_json::from_value(json!({
                "id":format!("partial-{name}"),
                "media_path":format!("partial.{name}"),
                "media_kind":"animated",
            }))
            .unwrap();
            assert!(matches!(
                decode_media_bytes(&request, partial.to_vec()),
                Err("animation_decode")
            ));
        }
    }

    #[test]
    fn tgs_hint_and_bad_animation_return_an_error_without_scoring_a_cover() {
        let request = json!({
            "id":"bad-animation",
            "media_path":format!(
                "{}/tests/fixtures/media/ad-retail-model-number.png",
                env!("CARGO_MANIFEST_DIR")
            ),
            "media_kind":"tgs",
        });
        let output = process(serde_json::to_string(&request).unwrap().as_bytes());
        assert_eq!(output["prediction"]["decision"], "ERROR");
        assert_eq!(output["prediction"]["error"], "animation_decode");
        assert!(output["prediction"].get("frames_processed").is_none());
    }
}
