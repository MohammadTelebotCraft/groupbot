use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use image::RgbImage;

use super::nsfw::{self, ModelError, SessionPool};

const DETECT: &[u8] = include_bytes!("../../assets/face_detect.onnx");
const EMBED: &[u8] = include_bytes!("../../assets/face_embed.onnx");

pub const WIDTH: usize = 128;
const TAG: &[u8; 4] = b"SF01";
pub const MAX_REFERENCES: usize = 8;
pub const CUT: f32 = 0.55;

const PEOPLE: &[u8] = include_bytes!("../../assets/face_people.bin");

const DRAWN_VISION: &[u8] = include_bytes!("../../assets/face_drawn_vision.bin");

#[derive(Debug)]
pub struct Person {
    pub name: &'static str,
    photographed: Model,
    drawn: Option<Model>,
    drawn_vision: Option<Direction>,
}

#[derive(Debug)]
struct Direction {
    weights: Vec<f32>,
    bias: f32,
    cut: f32,
}

#[derive(Debug)]
struct Model {
    weights: Vec<f32>,
    bias: f32,
    cut: f32,
    gallery: Vec<Vec<f32>>,
}

impl Model {
    fn logit(&self, print: &[f32]) -> f32 {
        super::vision::dot(&self.weights, print) + self.bias
    }

    fn nearest(&self, print: &[f32]) -> f32 {
        self.gallery
            .iter()
            .map(|face| super::vision::dot(print, face))
            .fold(-1.0, f32::max)
    }

    fn read(at: usize) -> (Model, usize) {
        let word = |at: usize| u32::from_le_bytes(PEOPLE[at..at + 4].try_into().expect("word"));
        let float = |at: usize| f32::from_le_bytes(PEOPLE[at..at + 4].try_into().expect("float"));
        let (bias, cut) = (float(at), float(at + 4));
        let weights = (0..WIDTH).map(|i| float(at + 8 + 4 * i)).collect();
        let at = at + 8 + 4 * WIDTH;
        let faces = word(at) as usize;
        let at = at + 4;
        let gallery = PEOPLE[at..at + faces * (4 + WIDTH)]
            .as_chunks::<{ 4 + WIDTH }>()
            .0
            .iter()
            .map(|chunk| {
                let scale = f32::from_le_bytes(chunk[..4].try_into().expect("four bytes"));
                super::vision::unit(&super::imgfilter::dequantize(&chunk[4..], scale))
            })
            .collect();
        let model = Model {
            weights,
            bias,
            cut,
            gallery,
        };
        (model, at + faces * (4 + WIDTH))
    }

    fn likeness(&self, prints: &[Vec<f32>]) -> f32 {
        prints
            .iter()
            .map(|print| {
                let model = LOGIT_SCALE * (self.logit(print) - self.cut);
                let near = self.nearest(print) - GALLERY;
                (CUT + model.min(near)).clamp(-1.0, 1.0)
            })
            .fold(-1.0, f32::max)
    }
}

impl Person {
    #[cfg(test)]
    pub fn for_test(weights: Vec<f32>, bias: f32, cut: f32, gallery: Vec<Vec<f32>>) -> Self {
        Person {
            name: "test",
            photographed: Model {
                weights,
                bias,
                cut,
                gallery,
            },
            drawn: None,
            drawn_vision: None,
        }
    }

    #[cfg(test)]
    pub fn with_drawn_vision(mut self, weights: Vec<f32>, bias: f32, cut: f32) -> Self {
        self.drawn_vision = Some(Direction { weights, bias, cut });
        self
    }
}

fn directions() -> std::collections::HashMap<&'static str, Direction> {
    let word = |at: usize| u32::from_le_bytes(DRAWN_VISION[at..at + 4].try_into().expect("header"));
    let float = |at: usize| f32::from_le_bytes(DRAWN_VISION[at..at + 4].try_into().expect("float"));
    assert!(
        &DRAWN_VISION[..4] == b"FDV1" && word(8) as usize == super::vision::DIM,
        "face_drawn_vision.bin is not the general model's"
    );
    let mut out = std::collections::HashMap::new();
    let mut at = 12;
    for _ in 0..word(4) {
        let length = u16::from_le_bytes(DRAWN_VISION[at..at + 2].try_into().expect("length")) as usize;
        let name = std::str::from_utf8(&DRAWN_VISION[at + 2..at + 2 + length]).expect("name");
        at += 2 + length;
        let (bias, cut) = (float(at), float(at + 4));
        let weights = (0..super::vision::DIM).map(|i| float(at + 8 + 4 * i)).collect();
        at += 8 + 4 * super::vision::DIM;
        out.insert(name, Direction { weights, bias, cut });
    }
    assert_eq!(at, DRAWN_VISION.len(), "face_drawn_vision.bin is truncated");
    out
}

fn people() -> &'static std::collections::HashMap<&'static str, Person> {
    static CELL: OnceLock<std::collections::HashMap<&'static str, Person>> = OnceLock::new();
    CELL.get_or_init(|| {
        let word = |at: usize| u32::from_le_bytes(PEOPLE[at..at + 4].try_into().expect("header"));
        assert!(&PEOPLE[..4] == b"FPM3" && word(8) as usize == WIDTH, "face_people.bin is not SFace");
        let mut directions = directions();
        let mut out = std::collections::HashMap::new();
        let mut at = 12;
        for _ in 0..word(4) {
            let length = u16::from_le_bytes(PEOPLE[at..at + 2].try_into().expect("length")) as usize;
            let name = std::str::from_utf8(&PEOPLE[at + 2..at + 2 + length]).expect("name");
            let (photographed, next) = Model::read(at + 2 + length);
            let drawn = if PEOPLE[next] == 1 {
                let (model, after) = Model::read(next + 1);
                at = after;
                Some(model)
            } else {
                at = next + 1;
                None
            };
            out.insert(
                name,
                Person {
                    name,
                    photographed,
                    drawn,
                    drawn_vision: directions.remove(name),
                },
            );
        }
        assert_eq!(at, PEOPLE.len(), "face_people.bin is truncated");
        assert!(directions.is_empty(), "face_drawn_vision.bin names somebody face_people.bin does not");
        out
    })
}

pub fn person(english: &str) -> Option<&'static Person> {
    people().get(english)
}

const LOGIT_SCALE: f32 = 0.05;

pub const GALLERY: f32 = 0.50;

pub fn person_likeness(prints: &[Vec<f32>], person: &Person) -> f32 {
    person.photographed.likeness(prints)
}

pub fn drawn_likeness(prints: &[Vec<f32>], person: &Person) -> f32 {
    person
        .drawn
        .as_ref()
        .unwrap_or(&person.photographed)
        .likeness(prints)
}

pub const DRAWN_VETO: f32 = 0.1;

pub fn drawn_vision_likeness(embedding: &[f32], person: &Person) -> Option<f32> {
    let direction = person.drawn_vision.as_ref()?;
    if embedding.len() != direction.weights.len() {
        return None;
    }
    let score = super::vision::dot(&direction.weights, embedding) + direction.bias;
    Some((CUT + LOGIT_SCALE * (score - direction.cut)).clamp(-1.0, 1.0))
}

pub const DUPLICATE: f32 = 0.80;

pub const DRAWN_BLUR: f32 = 2.0;

pub fn drawn_prints(image: &RgbImage) -> Result<Option<Vec<Vec<f32>>>, ModelError> {
    prints(&image::imageops::blur(image, DRAWN_BLUR))
}

const CAP: u32 = 1280;
const SCORE: f32 = 0.7;
const NMS: f32 = 0.3;
pub const MIN_FACE: f32 = 24.0;
const MAX_FACES: usize = 6;
const SIDE: usize = 112;
const TEMPLATE: [[f64; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub score: f32,
    pub bbox: [f32; 4],
    pub marks: [[f32; 2]; 5],
}

const SESSIONS: usize = 1;

fn detector() -> Option<&'static SessionPool> {
    static CELL: OnceLock<SessionPool> = OnceLock::new();
    static GATE: Mutex<Option<Instant>> = Mutex::new(None);
    nsfw::initialize_pool_once_retry(&CELL, &GATE, Duration::from_secs(30), || {
        nsfw::open_pool(DETECT, "face detector", SESSIONS)
    })
}

fn embedder() -> Option<&'static SessionPool> {
    static CELL: OnceLock<SessionPool> = OnceLock::new();
    static GATE: Mutex<Option<Instant>> = Mutex::new(None);
    nsfw::initialize_pool_once_retry(&CELL, &GATE, Duration::from_secs(30), || {
        nsfw::open_pool(EMBED, "face embedder", SESSIONS)
    })
}

fn run_detector(
    session: &nsfw::Session,
    shape: Vec<i64>,
    pixels: Vec<f32>,
) -> Result<std::collections::HashMap<String, Vec<f32>>, ModelError> {
    let input = ort::value::Value::from_array((shape, pixels))
        .map_err(|error| ModelError::contract(format!("face detector input: {error}")))?;
    let mut session = session
        .lock()
        .map_err(|_| ModelError::Poisoned("face detector"))?;
    let output = session
        .run(ort::inputs![input])
        .map_err(|error| ModelError::Runtime(format!("face detector pass: {error}")))?;
    let mut named = std::collections::HashMap::new();
    for (name, value) in output {
        let (_, values) = value.try_extract_tensor::<f32>().map_err(|error| {
            ModelError::contract(format!("face detector output {name}: {error}"))
        })?;
        if values.iter().any(|value| !value.is_finite()) {
            return Err(ModelError::contract(format!(
                "face detector output {name} is not finite"
            )));
        }
        named.insert(name.to_string(), values.to_vec());
    }
    Ok(named)
}

fn decode_outputs(
    outputs: &std::collections::HashMap<String, Vec<f32>>,
    width: usize,
    height: usize,
) -> Result<Vec<Face>, ModelError> {
    let mut found = Vec::new();
    for stride in [8usize, 16, 32] {
        let (cols, rows) = (width / stride, height / stride);
        let anchors = cols * rows;
        let get = |name: &str, per: usize| -> Result<&[f32], ModelError> {
            let key = format!("{name}_{stride}");
            let values = outputs
                .get(&key)
                .ok_or_else(|| ModelError::contract(format!("face detector has no {key}")))?;
            if values.len() != anchors * per {
                return Err(ModelError::contract(format!(
                    "face detector {key} has {} values, expected {}",
                    values.len(),
                    anchors * per
                )));
            }
            Ok(values)
        };
        let (cls, obj, bbox, kps) = (get("cls", 1)?, get("obj", 1)?, get("bbox", 4)?, get("kps", 10)?);
        let s = stride as f32;
        for at in 0..anchors {
            let score = (cls[at].clamp(0.0, 1.0) * obj[at].clamp(0.0, 1.0)).sqrt();
            if score < SCORE {
                continue;
            }
            let (row, col) = ((at / cols) as f32, (at % cols) as f32);
            let (cx, cy) = ((col + bbox[at * 4]) * s, (row + bbox[at * 4 + 1]) * s);
            let (w, h) = (bbox[at * 4 + 2].exp() * s, bbox[at * 4 + 3].exp() * s);
            let mut marks = [[0f32; 2]; 5];
            for (point, mark) in marks.iter_mut().enumerate() {
                *mark = [
                    (kps[at * 10 + 2 * point] + col) * s,
                    (kps[at * 10 + 2 * point + 1] + row) * s,
                ];
            }
            found.push(Face {
                score,
                bbox: [cx - w / 2.0, cy - h / 2.0, w, h],
                marks,
            });
        }
    }
    Ok(found)
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = ((a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0])).max(0.0);
    let iy = ((a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

fn suppress(mut faces: Vec<Face>) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::with_capacity(faces.len());
    for face in faces {
        if kept.iter().all(|other| iou(&face.bbox, &other.bbox) <= NMS) {
            kept.push(face);
        }
    }
    kept
}

pub fn detect(image: &RgbImage) -> Result<Option<Vec<Face>>, ModelError> {
    let Some(pool) = detector() else {
        return Ok(None);
    };
    let (w, h) = image.dimensions();
    if w < 32 || h < 32 {
        return Ok(Some(Vec::new()));
    }
    let scale = (CAP as f32 / w.max(h) as f32).min(1.0);
    let resized;
    let view = if scale < 1.0 {
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        resized = image::imageops::resize(image, nw, nh, image::imageops::FilterType::Triangle);
        &resized
    } else {
        image
    };
    let (nw, nh) = view.dimensions();
    let (pw, ph) = (nw.div_ceil(32) * 32, nh.div_ceil(32) * 32);
    let plane = (pw * ph) as usize;
    let mut pixels = vec![0f32; 3 * plane];
    for (x, y, pixel) in view.enumerate_pixels() {
        let at = (y * pw + x) as usize;
        pixels[at] = f32::from(pixel[2]);
        pixels[plane + at] = f32::from(pixel[1]);
        pixels[2 * plane + at] = f32::from(pixel[0]);
    }
    let outputs = pool.with(|session| {
        run_detector(session, vec![1, 3, i64::from(ph), i64::from(pw)], pixels)
    })?;
    let mut faces = decode_outputs(&outputs, pw as usize, ph as usize)?;
    for face in &mut faces {
        for value in &mut face.bbox {
            *value /= scale;
        }
        for mark in &mut face.marks {
            mark[0] /= scale;
            mark[1] /= scale;
        }
    }
    Ok(Some(suppress(faces)))
}

fn similarity(marks: &[[f32; 2]; 5]) -> [f64; 4] {
    let mut mean = [0f64; 4];
    for (mark, target) in marks.iter().zip(&TEMPLATE) {
        mean[0] += f64::from(mark[0]) / 5.0;
        mean[1] += f64::from(mark[1]) / 5.0;
        mean[2] += target[0] / 5.0;
        mean[3] += target[1] / 5.0;
    }
    let (mut norm, mut p, mut q) = (0f64, 0f64, 0f64);
    for (mark, target) in marks.iter().zip(&TEMPLATE) {
        let (ax, ay) = (f64::from(mark[0]) - mean[0], f64::from(mark[1]) - mean[1]);
        let (bx, by) = (target[0] - mean[2], target[1] - mean[3]);
        norm += ax * ax + ay * ay;
        p += ax * bx + ay * by;
        q += ax * by - ay * bx;
    }
    if norm <= f64::EPSILON {
        return [1.0, 0.0, 0.0, 0.0];
    }
    let (p, q) = (p / norm, q / norm);
    [
        p,
        q,
        mean[2] - (p * mean[0] - q * mean[1]),
        mean[3] - (q * mean[0] + p * mean[1]),
    ]
}

fn warp(image: &RgbImage, [p, q, tx, ty]: [f64; 4]) -> Vec<f32> {
    let det = p * p + q * q;
    let (w, h) = (i64::from(image.width()), i64::from(image.height()));
    let raw = image.as_raw();
    let fetch = |x: i64, y: i64, channel: usize| -> f64 {
        if x < 0 || y < 0 || x >= w || y >= h {
            return 0.0;
        }
        f64::from(raw[((y * w + x) * 3) as usize + channel])
    };
    let plane = SIDE * SIDE;
    let mut out = vec![0f32; 3 * plane];
    if det <= f64::EPSILON {
        return out;
    }
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (dx, dy) = (x as f64 - tx, y as f64 - ty);
            let (sx, sy) = ((p * dx + q * dy) / det, (-q * dx + p * dy) / det);
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            for channel in 0..3 {
                let value = fetch(x0, y0, channel) * (1.0 - fx) * (1.0 - fy)
                    + fetch(x0 + 1, y0, channel) * fx * (1.0 - fy)
                    + fetch(x0, y0 + 1, channel) * (1.0 - fx) * fy
                    + fetch(x0 + 1, y0 + 1, channel) * fx * fy;
                out[channel * plane + y * SIDE + x] = value as f32;
            }
        }
    }
    out
}

pub fn embed(image: &RgbImage, face: &Face) -> Result<Option<Vec<f32>>, ModelError> {
    let Some(pool) = embedder() else {
        return Ok(None);
    };
    let pixels = warp(image, similarity(&face.marks));
    let out = pool.with(|session| nsfw::run(session, vec![1, 3, SIDE as i64, SIDE as i64], pixels))?;
    if out.len() != WIDTH {
        return Err(ModelError::contract(format!(
            "face embedding has {} values, expected {WIDTH}",
            out.len()
        )));
    }
    super::vision::validated_unit(&out, "face embedding").map(Some)
}

pub fn prints(image: &RgbImage) -> Result<Option<Vec<Vec<f32>>>, ModelError> {
    let Some(mut faces) = detect(image)? else {
        return Ok(None);
    };
    faces.retain(|face| face.bbox[2].min(face.bbox[3]) >= MIN_FACE);
    faces.sort_by(|a, b| (b.bbox[2] * b.bbox[3]).total_cmp(&(a.bbox[2] * a.bbox[3])));
    faces.truncate(MAX_FACES);
    let mut out = Vec::with_capacity(faces.len());
    for face in &faces {
        match embed(image, face)? {
            Some(print) => out.push(print),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

pub fn portrait(image: &RgbImage) -> bool {
    let Ok(Some(faces)) = detect(image) else {
        return false;
    };
    let area = image.width() as f32 * image.height() as f32;
    faces.iter().any(|face| {
        face.bbox[2].min(face.bbox[3]) >= MIN_FACE
            && face.bbox[2] * face.bbox[3] >= PORTRAIT_SHARE * area
    })
}

const PORTRAIT_SHARE: f32 = 0.04;

#[derive(Debug, PartialEq)]
pub enum Reference {
    Unavailable,
    NoFace,
    Found { print: Vec<f32>, faces: usize },
}

pub fn reference(image: &RgbImage) -> Result<Reference, ModelError> {
    Ok(match prints(image)? {
        None => Reference::Unavailable,
        Some(prints) if prints.is_empty() => Reference::NoFace,
        Some(mut prints) => {
            let faces = prints.len();
            Reference::Found {
                print: prints.swap_remove(0),
                faces,
            }
        }
    })
}

pub fn likeness(prints: &[Vec<f32>], references: &[Vec<f32>]) -> f32 {
    prints
        .iter()
        .flat_map(|print| {
            references
                .iter()
                .map(move |reference| super::vision::dot(print, reference))
        })
        .fold(-1.0, f32::max)
}

const COHORT: &[u8] = include_bytes!("../../assets/face_cohort.bin");

struct Cohort {
    z: f32,
    mean: Vec<f32>,
    solve: Vec<f32>,
    spread: Vec<f32>,
}

fn cohort() -> &'static Cohort {
    static CELL: OnceLock<Cohort> = OnceLock::new();
    CELL.get_or_init(|| {
        let float = |at: usize| f32::from_le_bytes(COHORT[at..at + 4].try_into().expect("float"));
        let floats = |from: usize, count: usize| (0..count).map(|i| float(from + 4 * i)).collect();
        assert!(
            &COHORT[..4] == b"FCO1"
                && u32::from_le_bytes(COHORT[4..8].try_into().expect("width")) as usize == WIDTH
                && COHORT.len() == 12 + 4 * WIDTH + 8 * WIDTH * WIDTH,
            "face_cohort.bin is not SFace's"
        );
        Cohort {
            z: float(8),
            mean: floats(12, WIDTH),
            solve: floats(12 + 4 * WIDTH, WIDTH * WIDTH),
            spread: floats(12 + 4 * WIDTH + 4 * WIDTH * WIDTH, WIDTH * WIDTH),
        }
    })
}

fn exemplar(references: &[Vec<f32>]) -> (Vec<f32>, f32) {
    let cohort = cohort();
    let count = references.len() as f32;
    let away: Vec<f32> = (0..WIDTH)
        .map(|i| references.iter().map(|r| r[i]).sum::<f32>() / count - cohort.mean[i])
        .collect();
    let times = |matrix: &[f32], v: &[f32]| -> Vec<f32> {
        matrix
            .as_chunks::<WIDTH>()
            .0
            .iter()
            .map(|row| super::vision::dot(row, v))
            .collect()
    };
    let spread = super::vision::dot(&away, &times(&cohort.spread, &away))
        .max(1e-12)
        .sqrt();
    let direction: Vec<f32> = times(&cohort.solve, &away).iter().map(|x| x / spread).collect();
    let offset = super::vision::dot(&direction, &cohort.mean);
    (direction, offset)
}

pub const NEAR: f32 = 0.50;
const Z_SCALE: f32 = 0.05;

pub fn photographs_likeness(prints: &[Vec<f32>], references: &[Vec<f32>]) -> f32 {
    if references.is_empty() || prints.is_empty() {
        return -1.0;
    }
    let (direction, offset) = exemplar(references);
    let z = cohort().z;
    prints
        .iter()
        .map(|print| {
            let nearest = references
                .iter()
                .map(|reference| super::vision::dot(print, reference))
                .fold(-1.0, f32::max);
            let own = Z_SCALE * (super::vision::dot(&direction, print) - offset - z);
            (CUT + own.min(nearest - NEAR)).clamp(-1.0, 1.0)
        })
        .fold(-1.0, f32::max)
}

pub fn encode(references: &[Vec<f32>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + references.len() * (4 + WIDTH));
    out.extend_from_slice(TAG);
    for reference in references {
        let (bytes, scale) = super::imgfilter::quantize(reference);
        out.extend_from_slice(&scale.to_le_bytes());
        out.extend(bytes);
    }
    out
}

pub fn decode(bytes: &[u8]) -> Option<Vec<Vec<f32>>> {
    let rest = bytes.strip_prefix(TAG)?;
    if rest.len() % (4 + WIDTH) != 0 {
        return None;
    }
    Some(
        rest.as_chunks::<{ 4 + WIDTH }>()
            .0
            .iter()
            .map(|chunk| {
                let scale = f32::from_le_bytes(chunk[..4].try_into().expect("four bytes"));
                super::vision::unit(&super::imgfilter::dequantize(&chunk[4..], scale))
            })
            .collect(),
    )
}

pub fn fingerprint(bytes: &[u8]) -> u64 {
    super::imgfilter::fingerprint(bytes, 0.0) ^ 0x5f0c_e10c_c0de_f00d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_recovers_a_known_transform() {
        let identity = similarity(&TEMPLATE.map(|[x, y]| [x as f32, y as f32]));
        for (got, want) in identity.iter().zip([1.0, 0.0, 0.0, 0.0]) {
            assert!((got - want).abs() < 1e-4, "{identity:?}");
        }
        let (angle, scale, t) = (0.3f64, 0.5f64, [12.0f64, -7.0f64]);
        let (p, q) = (scale * angle.cos(), scale * angle.sin());
        let det = p * p + q * q;
        let marks = TEMPLATE.map(|[x, y]| {
            let (dx, dy) = (x - t[0], y - t[1]);
            [((p * dx + q * dy) / det) as f32, ((-q * dx + p * dy) / det) as f32]
        });
        let fit = similarity(&marks);
        for (got, want) in fit.iter().zip([p, q, t[0], t[1]]) {
            assert!((got - want).abs() < 1e-3, "{fit:?} against {:?}", [p, q, t[0], t[1]]);
        }
    }

    #[test]
    fn the_identity_warp_copies_and_pads_with_black() {
        let image = RgbImage::from_fn(100, 120, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 7]));
        let out = warp(&image, [1.0, 0.0, 0.0, 0.0]);
        let plane = SIDE * SIDE;
        assert_eq!(out.len(), 3 * plane);
        assert_eq!(out[5 * SIDE + 9], 9.0, "red is x");
        assert_eq!(out[plane + 5 * SIDE + 9], 5.0, "green is y");
        assert_eq!(out[2 * plane + 5 * SIDE + 9], 7.0, "blue is the constant");
        assert_eq!(out[5 * SIDE + 105], 0.0, "x 105 is past a 100 wide picture");
    }

    #[test]
    fn suppression_keeps_the_best_of_overlapping_boxes() {
        let face = |score: f32, x: f32| Face {
            score,
            bbox: [x, 0.0, 10.0, 10.0],
            marks: [[0.0; 2]; 5],
        };
        let kept = suppress(vec![face(0.8, 0.0), face(0.9, 1.0), face(0.95, 50.0)]);
        assert_eq!(kept.iter().map(|f| f.score).collect::<Vec<_>>(), vec![0.95, 0.9]);
    }

    #[test]
    fn decoding_places_an_anchor_where_yunet_does() {
        let (width, height) = (64usize, 32usize);
        let mut outputs = std::collections::HashMap::new();
        for stride in [8usize, 16, 32] {
            let anchors = (width / stride) * (height / stride);
            for (name, per) in [("cls", 1), ("obj", 1), ("bbox", 4), ("kps", 10)] {
                outputs.insert(format!("{name}_{stride}"), vec![0f32; anchors * per]);
            }
        }
        let at = 10;
        outputs.get_mut("cls_8").unwrap()[at] = 0.9;
        outputs.get_mut("obj_8").unwrap()[at] = 0.9;
        outputs.get_mut("bbox_8").unwrap()[at * 4..at * 4 + 4].copy_from_slice(&[0.5, 0.5, 0.0, 0.0]);
        outputs.get_mut("kps_8").unwrap()[at * 10] = 0.25;
        let faces = decode_outputs(&outputs, width, height).unwrap();
        assert_eq!(faces.len(), 1);
        let face = &faces[0];
        assert!((face.score - 0.9).abs() < 1e-6);
        assert_eq!(face.bbox, [16.0, 8.0, 8.0, 8.0]);
        assert_eq!(face.marks[0], [18.0, 8.0]);
        assert_eq!(face.marks[1], [16.0, 8.0]);
    }

    #[test]
    fn references_round_trip_and_another_models_are_refused() {
        let a: Vec<f32> = super::super::vision::unit(&(0..WIDTH).map(|i| (i % 7) as f32 - 3.0).collect::<Vec<_>>());
        let b: Vec<f32> = super::super::vision::unit(&(0..WIDTH).map(|i| (i % 5) as f32 - 2.0).collect::<Vec<_>>());
        let bytes = encode(&[a.clone(), b.clone()]);
        let back = decode(&bytes).expect("our own tag");
        assert_eq!(back.len(), 2);
        assert!(super::super::vision::dot(&back[0], &a) > 0.999);
        assert!(super::super::vision::dot(&back[1], &b) > 0.999);
        let mut other = bytes.clone();
        other[..4].copy_from_slice(b"AF01");
        assert_eq!(decode(&other), None, "another model's faces are not ours to compare");
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode(TAG), Some(Vec::new()), "a built-in person's lock with no photographs");
        assert!((likeness(&back, &[a]) - 1.0).abs() < 1e-2);
        assert_eq!(likeness(&[], &back), -1.0);
    }

    #[test]
    fn built_in_people_are_the_tables_people() {
        assert!(people().len() >= 10, "face_people.bin is missing or truncated");
        let table = super::super::imgsubject::person_englishes();
        for (name, person) in people() {
            assert!(table.contains(name), "«{name}» is not a person the table routes to the face lock");
            for (kind, model) in [("photographed", Some(&person.photographed)), ("drawn", person.drawn.as_ref())] {
                let Some(model) = model else {
                    continue;
                };
                assert!(model.weights.iter().all(|w| w.is_finite()) && model.cut.is_finite());
                assert!(model.gallery.len() >= 20, "«{name}» has a {kind} gallery");
                let taken = model
                    .gallery
                    .iter()
                    .filter(|face| model.likeness(std::slice::from_ref(*face)) >= CUT)
                    .count();
                let share = if kind == "drawn" { 10 } else { 2 };
                assert!(taken * share > model.gallery.len(), "«{name}» {kind}: {taken} of {}", model.gallery.len());
            }
        }
        assert!(person("ali khamenei").is_some_and(|person| person.drawn.is_some()));
        assert!(person("ali khamenei").is_some_and(|person| person
            .drawn_vision
            .as_ref()
            .is_some_and(|d| d.weights.iter().all(|w| w.is_finite()) && d.cut.is_finite())));
        assert_eq!(person_likeness(&[], people().values().next().unwrap()), -1.0);
    }

    #[test]
    fn a_drawing_is_read_whole_on_the_faces_scale() {
        let axis = |width: usize, i: usize| {
            let mut v = vec![0.0f32; width];
            v[i] = 1.0;
            v
        };
        let plain = Person::for_test(axis(WIDTH, 0), 0.0, 1.0, vec![axis(WIDTH, 0)]);
        assert_eq!(drawn_vision_likeness(&axis(super::super::vision::DIM, 0), &plain), None);
        let person = plain.with_drawn_vision(axis(super::super::vision::DIM, 0), 1.0, 3.0);
        let at = |cosine: f32| {
            let mut v = axis(super::super::vision::DIM, 1);
            v[0] = cosine;
            v
        };
        assert!((drawn_vision_likeness(&at(2.0), &person).unwrap() - CUT).abs() < 1e-6);
        assert!(drawn_vision_likeness(&at(1.0), &person).unwrap() < CUT);
        assert!((drawn_vision_likeness(&at(3.0), &person).unwrap() - (CUT + LOGIT_SCALE)).abs() < 1e-6);
        assert_eq!(drawn_vision_likeness(&[1.0; 4], &person), None, "an embedding of another width");
    }

    #[test]
    fn photographs_alone_find_the_person_and_nobody_else() {
        for name in ["hassan rouhani", "qasem soleimani"] {
            let gallery = &people()[name].photographed.gallery;
            let picked = [0, gallery.len() / 3, 2 * gallery.len() / 3];
            let references: Vec<Vec<f32>> = picked.iter().map(|&at| gallery[at].clone()).collect();
            let rest: Vec<&Vec<f32>> = (0..gallery.len())
                .filter(|at| !picked.contains(at))
                .map(|at| &gallery[at])
                .collect();
            let taken = rest
                .iter()
                .filter(|face| photographs_likeness(std::slice::from_ref(**face), &references) >= CUT)
                .count();
            assert!(taken * 2 > rest.len(), "{name}: {taken} of {}", rest.len());
            for (other, person) in people() {
                if *other != name {
                    let took = person
                        .photographed
                        .gallery
                        .iter()
                        .filter(|face| photographs_likeness(std::slice::from_ref(*face), &references) >= CUT)
                        .count();
                    assert_eq!(took, 0, "{name}'s photographs took {took} faces of {other}");
                }
            }
        }
        assert_eq!(photographs_likeness(&[], &[vec![0.0; WIDTH]]), -1.0);
    }

    #[test]
    fn a_built_in_match_needs_the_gallery_too() {
        let axis = |i: usize| {
            let mut v = vec![0.0f32; WIDTH];
            v[i] = 1.0;
            v
        };
        let person = Person::for_test(axis(0), 0.0, 0.5, vec![axis(1)]);
        assert!(person_likeness(&[axis(0)], &person) < CUT, "the model alone");
        assert!(person_likeness(&[axis(1)], &person) < CUT, "the gallery alone");
        let both = super::super::vision::unit(&[&[1.0f32, 1.0][..], &[0.0; WIDTH - 2]].concat());
        let score = person_likeness(&[axis(0), both], &person);
        assert!((CUT..CUT + 0.02).contains(&score), "{score}");
    }

    #[test]
    #[ignore = "loads the ONNX runtime; run with the other model tests"]
    fn the_embedder_agrees_with_the_reference_pipeline() {
        nsfw::configure_model_tests().expect("model test configuration");
        let image = RgbImage::from_fn(200, 220, |x, y| {
            image::Rgb([((x * 7 + y * 3) % 256) as u8, ((x * 2 + y * 5) % 256) as u8, ((x ^ y) % 256) as u8])
        });
        let face = Face {
            score: 1.0,
            bbox: [40.0, 50.0, 100.0, 120.0],
            marks: [[70.0, 90.0], [130.0, 88.0], [100.0, 120.0], [76.0, 150.0], [126.0, 148.0]],
        };
        let print = embed(&image, &face).expect("embedding").expect("embedder loads");
        assert_eq!(print.len(), WIDTH);
        for (at, want) in REFERENCE_PRINT {
            assert!((print[at] - want).abs() < 2e-3, "component {at}: {} against {want}", print[at]);
        }
        let detected = detect(&image).expect("detection").expect("detector loads");
        assert!(detected.is_empty(), "a gradient has no face");
    }

    #[test]
    #[ignore = "needs FACE_BENCH_IMAGES and FACE_BENCH_JSON"]
    fn prints_agree_with_the_reference_on_real_photographs() {
        let (Ok(dir), Ok(json)) = (
            std::env::var("FACE_BENCH_IMAGES"),
            std::env::var("FACE_BENCH_JSON"),
        ) else {
            return;
        };
        nsfw::configure_model_tests().expect("model test configuration");
        let reference: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(json).expect("reference json"))
                .expect("reference json parses");
        let (mut pictures, mut same_count, mut close) = (0, 0, 0);
        for (name, want) in reference.as_object().expect("an object of pictures") {
            let image = image::open(std::path::Path::new(&dir).join(name))
                .expect("a reference picture")
                .to_rgb8();
            let prints = prints(&image).expect("faces").expect("models load");
            pictures += 1;
            if prints.len() as u64 == want["faces"].as_u64().unwrap_or(0) {
                same_count += 1;
            }
            if let (Some(first), Some(largest)) = (prints.first(), want["largest"].as_array()) {
                let vector = |values: &Vec<serde_json::Value>| -> Vec<f32> {
                    values
                        .iter()
                        .map(|value| value.as_f64().unwrap_or(0.0) as f32)
                        .collect()
                };
                let cosine = super::super::vision::dot(first, &vector(largest));
                let nearest = want["all"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|face| face.as_array())
                    .map(|face| super::super::vision::dot(first, &vector(face)))
                    .fold(-1.0f32, f32::max);
                if cosine > 0.97 {
                    close += 1;
                }
                if cosine <= 0.99 {
                    println!("{name}: largest {cosine:.4}, nearest reference face {nearest:.4}, areas {}", want["areas"]);
                }
            }
        }
        let with_face = reference
            .as_object()
            .unwrap()
            .values()
            .filter(|want| !want["largest"].is_null())
            .count();
        println!("{same_count}/{pictures} same face count, {close}/{with_face} largest faces within 0.97");
        assert!(same_count * 10 >= pictures * 9, "face counts disagree too often");
        assert!(close * 10 >= with_face * 9, "embeddings disagree too often");
    }

    const REFERENCE_PRINT: [(usize, f32); 4] = [(0, 0.049093), (31, -0.020207), (64, 0.026384), (127, -0.080252)];

    #[test]
    #[ignore = "needs LOTTIE_DIR and LOTTIE_OUT"]
    fn render_lottie_stickers() {
        let (Ok(dir), Ok(out)) = (std::env::var("LOTTIE_DIR"), std::env::var("LOTTIE_OUT")) else {
            return;
        };
        let mut rendered = 0;
        for entry in std::fs::read_dir(&dir).expect("LOTTIE_DIR") {
            let path = entry.expect("an entry").path();
            let Some(stem) = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()) else {
                continue;
            };
            let last = std::path::Path::new(&out).join(format!("{stem}_4.png"));
            if path.extension().is_none_or(|extension| extension != "tgs") || last.exists() {
                continue;
            }
            let Some(frames) = nsfw::decode_tgs_frames_offline(&std::fs::read(&path).expect("a sticker"))
            else {
                continue;
            };
            let raw = std::env::var("LOTTIE_RAW").is_ok();
            for at in 0..5 {
                let frame = &frames[at * (frames.len() - 1) / 4];
                let frame = if raw { frame.clone() } else { image::imageops::blur(frame, DRAWN_BLUR) };
                frame
                    .save(std::path::Path::new(&out).join(format!("{stem}_{at}.png")))
                    .expect("a frame");
            }
            rendered += 1;
        }
        println!("rendered {rendered}");
    }

    #[test]
    #[ignore = "needs DRAWN_FRAMES and DRAWN_TERMS_OUT"]
    fn drawn_terms_of_frames() {
        let (Ok(dirs), Ok(out)) = (std::env::var("DRAWN_FRAMES"), std::env::var("DRAWN_TERMS_OUT")) else {
            return;
        };
        let person = person("ali khamenei").expect("Khamenei's built-in model");
        let model = person.drawn.as_ref().expect("a drawn model");
        let mut files: Vec<_> = dirs
            .split(':')
            .flat_map(|dir| std::fs::read_dir(dir).expect("a frame directory"))
            .map(|entry| entry.expect("an entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
            .collect();
        files.sort();
        let lines = Mutex::new(Vec::with_capacity(files.len()));
        std::thread::scope(|scope| {
            for chunk in files.chunks(files.len().div_ceil(8).max(1)) {
                let lines = &lines;
                scope.spawn(move || {
                    for path in chunk {
                        let frame = image::open(path).expect("a frame").to_rgb8();
                        let found = drawn_prints(&frame).expect("faces").unwrap_or_default();
                        let mut terms = (f32::MIN, -1.0f32);
                        for print in &found {
                            let pair = (model.logit(print) - model.cut, model.nearest(print));
                            if pair.0.min((pair.1 - GALLERY) / LOGIT_SCALE)
                                > terms.0.min((terms.1 - GALLERY) / LOGIT_SCALE)
                            {
                                terms = pair;
                            }
                        }
                        let likeness = if found.is_empty() { -1.0 } else { drawn_likeness(&found, person) };
                        lines.lock().unwrap().push(format!(
                            "{}\t{}\t{:.4}\t{:.4}\t{:.4}",
                            path.display(),
                            found.len(),
                            terms.0.max(-99.0),
                            terms.1,
                            likeness
                        ));
                    }
                });
            }
        });
        let lines = lines.into_inner().unwrap();
        std::fs::write(&out, lines.join("\n") + "\n").expect("DRAWN_TERMS_OUT");
        println!("{} frames", lines.len());
    }

    #[test]
    #[ignore = "needs FACE_STICKERS and ffmpeg; run with the other model tests"]
    fn stickers_of_a_built_in_person_through_the_production_decoders() {
        let Ok(dir) = std::env::var("FACE_STICKERS") else {
            return;
        };
        nsfw::configure_model_tests().expect("model test configuration");
        let person = person("ali khamenei").expect("Khamenei's built-in model");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for the animated decoder");
        let mut tally: std::collections::BTreeMap<(String, &str), (usize, usize)> =
            std::collections::BTreeMap::new();
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .expect("FACE_STICKERS")
            .map(|entry| entry.expect("an entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let label = name.split("__").next().unwrap_or("").to_owned();
            let bytes = std::fs::read(&path).expect("a sticker");
            let video = name.ends_with(".webm");
            let lottie = name.ends_with(".tgs");
            let frames = if video {
                runtime
                    .block_on(nsfw::decode_animated_frames(bytes))
                    .expect("the video sticker decodes")
            } else if lottie {
                nsfw::decode_tgs_frames_offline(&bytes).expect("the Lottie sticker renders")
            } else {
                vec![nsfw::decode_still(&bytes).expect("the sticker decodes")]
            };
            if let Ok(dump) = std::env::var("FACE_STICKERS_DUMP") {
                frames[0]
                    .save(std::path::Path::new(&dump).join(format!("{name}.png")))
                    .expect("a dumped frame");
            }
            let mut faces_seen = 0;
            let mut whole_best = -1.0f32;
            let spec = super::super::imgfilter::Spec::Face(Vec::new(), Some(person), CUT);
            let score = frames
                .iter()
                .map(|frame| {
                    let found = if lottie { drawn_prints(frame) } else { prints(frame) };
                    let found = found.expect("faces");
                    faces_seen += found.as_ref().map_or(0, Vec::len);
                    let embedding = if lottie {
                        super::super::vision::embed_of(frame).expect("the general model").unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    if let Some(whole) = drawn_vision_likeness(&embedding, person).filter(|_| lottie) {
                        whole_best = whole_best.max(whole);
                    }
                    let value = super::super::imgfilter::score_spec(&spec, &embedding, found.as_deref(), lottie);
                    if value == super::super::imgfilter::UNSEEN { -1.0 } else { value }
                })
                .fold(-1.0, f32::max);
            if std::env::var("FACE_STICKERS_VERBOSE").is_ok() {
                let mut terms = (f32::MIN, -1.0f32);
                if lottie && let Some(model) = person.drawn.as_ref() {
                    for frame in &frames {
                        for print in drawn_prints(frame).expect("faces").unwrap_or_default() {
                            let pair = (model.logit(&print) - model.cut, model.nearest(&print));
                            if pair.0.min((pair.1 - GALLERY) * 20.0) > terms.0.min((terms.1 - GALLERY) * 20.0) {
                                terms = pair;
                            }
                        }
                    }
                }
                println!(
                    "{name}: {} frames, {faces_seen} faces, score {score:.3}, logit over cut {:.2}, nearest {:.3}, whole {whole_best:.3}",
                    frames.len(),
                    terms.0,
                    terms.1
                );
            }
            let kind = if video {
                "video"
            } else if lottie {
                "lottie"
            } else {
                "static"
            };
            let entry = tally.entry((label.clone(), kind)).or_default();
            entry.1 += 1;
            if score >= CUT {
                entry.0 += 1;
                if label != "khamenei" {
                    println!("{name} taken at {score:.3}");
                }
            }
        }
        for ((label, kind), (taken, of)) in &tally {
            println!("{label:10} {kind:6} {taken}/{of} taken");
        }
        let count = |label: &str, kind: &str| tally.get(&(label.to_owned(), kind)).copied().unwrap_or((0, 0));
        for kind in ["static", "video", "lottie"] {
            let (taken, of) = count("khamenei", kind);
            let tenths = if kind == "lottie" { 2 } else { 8 };
            assert!(taken * 10 >= of * tenths, "Khamenei's {kind} stickers: {taken} of {of}");
            for other in ["lookalike", "everyday"] {
                assert_eq!(count(other, kind).0, 0, "{other} {kind} stickers taken for Khamenei");
            }
        }
    }
}
