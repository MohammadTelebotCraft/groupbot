use std::hash::{BuildHasher, RandomState};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::nsfw;

#[path = "ocr/signals.rs"]
pub mod signals;
pub use signals::{Classification, Decision};

pub const LOCK: &str = "advert";

pub const DETECTOR_VERSION: &str = "tesseract-5.3.4-fas-eng-rules-v21-support";

pub const SHADOW: &str = "ad_shadow";

const LIBARCHIVE: &[u8] = include_bytes!("../../assets/tesseract/libarchive.so.13.7.2");
const LIBGIF: &[u8] = include_bytes!("../../assets/tesseract/libgif.so.7.2.0");
const LIBICUDATA: &[u8] = include_bytes!("../../assets/tesseract/libicudata.so.74.2");
const LIBICUUC: &[u8] = include_bytes!("../../assets/tesseract/libicuuc.so.74.2");
const LIBLEPT: &[u8] = include_bytes!("../../assets/tesseract/liblept.so.5.0.4");
const LIBTESSERACT: &[u8] = include_bytes!("../../assets/tesseract/libtesseract.so.5.0.3");
const LIBXML2: &[u8] = include_bytes!("../../assets/tesseract/libxml2.so.2.9.14");
const TESSERACT: &[u8] = include_bytes!("../../assets/tesseract/tesseract");
const ENG: &[u8] = include_bytes!("../../assets/tesseract/eng.traineddata");
const FAS: &[u8] = include_bytes!("../../assets/tesseract/fas.traineddata");

const OCR_MAX_SIDE: u32 = 1000;
const OCR_MAX_TEXT_CHARS: usize = 8192;
const OCR_MAX_OUTPUT_BYTES: u64 = 64 * 1024;
const OCR_PROCESS_DEADLINE: Duration = Duration::from_secs(2);
const OCR_POLL: Duration = Duration::from_millis(10);

static ARTIFACT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static OCR_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn create_private_ocr_temp_dir() -> Result<std::path::PathBuf, nsfw::ModelError> {
    for _ in 0..16 {
        let sequence = OCR_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let random =
            |domain: u64| RandomState::new().hash_one((domain, sequence, std::process::id()));
        let temporary = std::env::temp_dir().join(format!(
            "groupbot-tesseract-{}-{sequence:016x}-{:016x}{:016x}",
            std::process::id(),
            random(0),
            random(1)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&temporary) {
            Ok(()) => return Ok(temporary),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(nsfw::ModelError::Runtime(format!(
                    "could not create private OCR temporary directory: {error}"
                )));
            }
        }
    }
    Err(nsfw::ModelError::Runtime(
        "could not claim a private OCR temporary directory after repeated collisions".to_owned(),
    ))
}

fn file_matches(path: &std::path::Path, expected: &[u8]) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if metadata.len() != expected.len() as u64 {
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

fn publish(path: &std::path::Path, bytes: &[u8]) -> Result<(), nsfw::ModelError> {
    if file_matches(path, bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            nsfw::ModelError::artifact(format!("could not create {}: {error}", parent.display()))
        })?;
    }
    let sequence = ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = path.with_extension(format!("{}.{}.partial", std::process::id(), sequence));
    if let Err(error) = std::fs::write(&staging, bytes) {
        let _ = std::fs::remove_file(&staging);
        return Err(nsfw::ModelError::artifact(format!(
            "could not stage {}: {error}",
            staging.display()
        )));
    }
    if let Err(error) = std::fs::rename(&staging, path) {
        if !file_matches(path, bytes) {
            let _ = std::fs::remove_file(&staging);
            return Err(nsfw::ModelError::artifact(format!(
                "could not publish {}: {error}",
                path.display()
            )));
        }
        let _ = std::fs::remove_file(&staging);
    }
    if !file_matches(path, bytes) {
        return Err(nsfw::ModelError::artifact(format!(
            "published Tesseract artifact {} failed verification",
            path.display()
        )));
    }
    Ok(())
}

fn executable_dir() -> Result<std::path::PathBuf, nsfw::ModelError> {
    let mut path = std::env::current_exe().map_err(|error| {
        nsfw::ModelError::artifact(format!("could not locate the running executable: {error}"))
    })?;
    path.pop();
    Ok(path)
}

struct Runtime {
    directory: std::path::PathBuf,
    executable: std::path::PathBuf,
    tessdata: std::path::PathBuf,
}

fn initialize_once_retry<'a, T, E>(
    cell: &'a OnceLock<T>,
    gate: &Mutex<()>,
    build: impl FnOnce() -> Result<T, E>,
) -> Result<&'a T, E> {
    if let Some(value) = cell.get() {
        return Ok(value);
    }
    let _guard = gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(value) = cell.get() {
        return Ok(value);
    }
    let value = build()?;
    Ok(cell.get_or_init(|| value))
}

fn runtime() -> Result<&'static Runtime, nsfw::ModelError> {
    static CELL: OnceLock<Runtime> = OnceLock::new();
    static GATE: Mutex<()> = Mutex::new(());
    initialize_once_retry(&CELL, &GATE, || {
        let mut digest = Sha256::new();
        for bytes in [
            LIBARCHIVE,
            LIBGIF,
            LIBICUDATA,
            LIBICUUC,
            LIBLEPT,
            LIBTESSERACT,
            LIBXML2,
            TESSERACT,
            ENG,
            FAS,
        ] {
            digest.update(bytes);
        }
        let directory = executable_dir()?.join(format!("tesseract-{:x}", digest.finalize()));
        let tessdata = directory.join("tessdata");
        for (name, bytes) in [
            ("libarchive.so.13", LIBARCHIVE),
            ("libgif.so.7", LIBGIF),
            ("libicudata.so.74", LIBICUDATA),
            ("libicuuc.so.74", LIBICUUC),
            ("liblept.so.5", LIBLEPT),
            ("libtesseract.so.5", LIBTESSERACT),
            ("libxml2.so.2", LIBXML2),
            ("tesseract", TESSERACT),
            ("tessdata/eng.traineddata", ENG),
            ("tessdata/fas.traineddata", FAS),
        ] {
            publish(&directory.join(name), bytes)?;
        }
        let executable = directory.join("tesseract");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&executable)
                .map_err(|error| {
                    nsfw::ModelError::artifact(format!(
                        "could not inspect {}: {error}",
                        executable.display()
                    ))
                })?
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable, permissions).map_err(|error| {
                nsfw::ModelError::artifact(format!(
                    "could not make {} executable: {error}",
                    executable.display()
                ))
            })?;
        }
        Ok(Runtime {
            directory,
            executable,
            tessdata,
        })
    })
}

fn command(runtime: &Runtime) -> Result<Command, nsfw::ModelError> {
    let mut paths = vec![runtime.directory.clone()];
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let library_path = std::env::join_paths(paths).map_err(|error| {
        nsfw::ModelError::artifact(format!(
            "could not construct Tesseract library path: {error}"
        ))
    })?;
    let mut command = Command::new(&runtime.executable);
    command
        .env("LD_LIBRARY_PATH", library_path)
        .env("TESSDATA_PREFIX", &runtime.tessdata)
        .env("OMP_THREAD_LIMIT", "1")
        .env("OMP_NUM_THREADS", "1")
        .env("LC_ALL", "C.UTF-8");
    Ok(command)
}

pub fn capacity_probe() -> usize {
    let Ok(runtime) = runtime() else {
        return 0;
    };
    usize::from(
        command(runtime)
            .and_then(|mut command| {
                command
                    .arg("--version")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map_err(|error| {
                        nsfw::ModelError::Runtime(format!(
                            "could not start embedded Tesseract: {error}"
                        ))
                    })
            })
            .is_ok_and(|status| status.success()),
    )
}

fn bounded_view(image: &image::RgbImage) -> std::borrow::Cow<'_, image::RgbImage> {
    let longest = image.width().max(image.height());
    if longest <= OCR_MAX_SIDE {
        return std::borrow::Cow::Borrowed(image);
    }
    let scale = OCR_MAX_SIDE as f32 / longest as f32;
    std::borrow::Cow::Owned(image::imageops::resize(
        image,
        (image.width() as f32 * scale).round().max(1.0) as u32,
        (image.height() as f32 * scale).round().max(1.0) as u32,
        image::imageops::FilterType::Triangle,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OcrProfile {
    Block,
    Auto,
    Sparse,
    RawLine,
}

impl OcrProfile {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Block => "block6",
            Self::Auto => "auto3",
            Self::Sparse => "sparse11",
            Self::RawLine => "raw13",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "block6" => Some(Self::Block),
            "auto3" => Some(Self::Auto),
            "sparse11" => Some(Self::Sparse),
            "raw13" => Some(Self::RawLine),
            _ => None,
        }
    }

    const fn page_segmentation_mode(self) -> &'static str {
        match self {
            Self::Block => "6",
            Self::Auto => "3",
            Self::Sparse => "11",
            Self::RawLine => "13",
        }
    }
}

pub fn read(image: &image::RgbImage) -> Result<String, nsfw::ModelError> {
    let image = bounded_view(image);
    let mut text = read_with_profile(&image, OcrProfile::Block)?;
    if !looks_sideways(&image, &text) {
        return maybe_read_high_contrast(image.as_ref(), text).map(|text| {
            maybe_read_line_contacts(
                image.as_ref(),
                maybe_read_contact_digits(image.as_ref(), text),
            )
        });
    }
    let mut quality = readable_words(&text);
    let mut selected_rotation = None;
    for rotated in [
        image::imageops::rotate90(image.as_ref()),
        image::imageops::rotate270(image.as_ref()),
    ] {
        if let Ok(candidate) = read_with_profile(&rotated, OcrProfile::Block) {
            let candidate_quality = readable_words(&candidate);
            if candidate_quality >= 2 && candidate_quality > quality {
                text = candidate;
                quality = candidate_quality;
                selected_rotation = Some(rotated);
            }
        }
    }
    let selected = selected_rotation.as_ref().unwrap_or(image.as_ref());
    maybe_read_high_contrast(selected, text)
        .map(|text| maybe_read_line_contacts(selected, maybe_read_contact_digits(selected, text)))
}

fn maybe_read_contact_digits(image: &image::RgbImage, text: String) -> String {
    let first = classify(&text);
    if first.decision == Decision::Block
        || (first.signals.strong_commercial_keywords == 0
            && !(first.signals.commercial_keywords > 0 && first.signals.call_to_action))
        || first.signals.editorial_context
        || first.signals.phone_number
        || !text.contains("تماس")
    {
        return text;
    }
    let Ok(tsv) = read_with_options(
        image,
        "6",
        &["-c", "tessedit_create_tsv=1", "-c", "tessedit_create_txt=0"],
        "tsv",
    ) else {
        return text;
    };
    let mut sale_heights = Vec::new();
    let mut contacts = Vec::new();
    for line in tsv.lines().skip(1) {
        let fields = line.splitn(12, '\t').collect::<Vec<_>>();
        if fields.len() != 12 || fields[0] != "5" {
            continue;
        }
        let (Ok(left), Ok(top), Ok(width), Ok(height)) = (
            fields[6].parse::<u32>(),
            fields[7].parse::<u32>(),
            fields[8].parse::<u32>(),
            fields[9].parse::<u32>(),
        ) else {
            continue;
        };
        if fields[11].contains("فروش") && (8..=60).contains(&height) {
            sale_heights.push(height);
        }
        if fields[11].contains("تماس") && width >= 12 && height >= 12 {
            contacts.push((left, top, height));
        }
    }
    sale_heights.sort_unstable();
    for (left, top, height) in contacts.into_iter().take(2) {
        let scale = sale_heights
            .get(sale_heights.len() / 2)
            .copied()
            .unwrap_or(height);
        let x0 = left
            .saturating_add((2.3 * scale as f32).round() as u32)
            .min(image.width());
        let x1 = x0
            .saturating_add(((9.0 * scale as f32).max(170.0)).round() as u32)
            .min(image.width());
        let center_y = top.saturating_add(height / 2);
        let pad_y = ((0.8 * scale as f32).max(15.0)).round() as u32;
        let y0 = center_y.saturating_sub(pad_y);
        let y1 = center_y.saturating_add(pad_y).min(image.height());
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let crop = image::imageops::crop_imm(image, x0, y0, x1 - x0, y1 - y0).to_image();
        let enlarged = image::imageops::resize(
            &crop,
            crop.width() * 2,
            crop.height() * 2,
            image::imageops::FilterType::CatmullRom,
        );
        let Ok(reading) = read_with_options(
            &enlarged,
            "8",
            &["-c", "tessedit_char_whitelist=۰۱۲۳۴۵۶۷۸۹0123456789"],
            "txt",
        ) else {
            continue;
        };
        let digits = reading
            .chars()
            .filter(|c| c.is_ascii_digit() || matches!(c, '۰'..='۹' | '٠'..='٩'))
            .collect::<String>();
        let count = digits.chars().count();
        let line = if count == 12
            && (digits.ends_with('0') || digits.ends_with('۰') || digits.ends_with('٠'))
        {
            let split = digits
                .char_indices()
                .nth(11)
                .map(|(at, _)| at)
                .unwrap_or(digits.len());
            format!("تماس {} {}", &digits[..split], &digits[split..])
        } else {
            format!("تماس {digits}")
        };
        if let Some(repaired) = signals::recovered_line_contact(&text, &line, true) {
            let candidate = format!("{text}\n{repaired}");
            if classify(&candidate).decision == Decision::Block {
                return candidate;
            }
        }
    }
    text
}

fn maybe_read_line_contacts(image: &image::RgbImage, text: String) -> String {
    let first = classify(&text);
    if first.decision == Decision::Block
        || first.signals.strong_commercial_keywords == 0
        || first.signals.editorial_context
    {
        return text;
    }
    let mut lines = Vec::new();
    let mut recovered = false;
    for (left, top, right, bottom) in line_band_boxes(image) {
        let crop =
            image::imageops::crop_imm(image, left, top, right - left, bottom - top).to_image();
        if let Ok(line) = read_with_profile(&crop, OcrProfile::RawLine) {
            if let Some(repaired) =
                signals::recovered_line_contact(&text, &line, first.signals.call_to_action)
            {
                lines.push(repaired);
                recovered = true;
            } else {
                lines.push(line);
            }
        }
    }
    if !recovered {
        return text;
    }
    let candidate = format!("{text}\n{}", lines.join("\n"));
    if classify(&candidate).decision == Decision::Block {
        candidate
    } else {
        text
    }
}

fn line_band_boxes(image: &image::RgbImage) -> Vec<(u32, u32, u32, u32)> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let (x0, x1) = (width / 20, width - width / 20);
    let (y0, y1) = (height / 20, height - height / 20);
    let mut channels: [Vec<u8>; 3] = std::array::from_fn(|_| Vec::new());
    for y in 0..y0.max(1) {
        for x in 0..x0.max(1) {
            for (channel, &value) in channels.iter_mut().zip(&image.get_pixel(x, y).0) {
                channel.push(value);
            }
        }
    }
    let background = channels.map(|mut channel| {
        channel.sort_unstable();
        channel[channel.len() / 2]
    });
    let foreground = |x: u32, y: u32| {
        image
            .get_pixel(x, y)
            .0
            .iter()
            .zip(background)
            .any(|(&value, base)| value.abs_diff(base) >= 45)
    };
    let mut groups: Vec<(u32, u32)> = Vec::new();
    for y in y0..y1 {
        let active =
            (x0..x1).filter(|&x| foreground(x, y)).count() >= (width / 100).max(3) as usize;
        if active {
            if let Some((_, last)) = groups.last_mut()
                && y - *last <= 18
            {
                *last = y;
            } else {
                groups.push((y, y));
            }
        }
    }
    let mut boxes = Vec::new();
    for &(top, last) in groups.iter().take(8) {
        let bottom = last + 1;
        if !(15..=height / 4).contains(&(bottom - top)) {
            continue;
        }
        let (mut min_x, mut max_x) = (width, 0);
        for y in top..bottom {
            for x in x0..x1 {
                if foreground(x, y) {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        if min_x == width || max_x - min_x + 1 < width / 5 {
            continue;
        }
        let pad_x = (width / 16).max(32);
        boxes.push((
            min_x.saturating_sub(pad_x),
            top.saturating_sub(12),
            max_x.saturating_add(pad_x + 1).min(width),
            bottom.saturating_add(13).min(height),
        ));
        if boxes.len() == 4 {
            break;
        }
    }
    boxes
}

fn maybe_read_high_contrast(
    image: &image::RgbImage,
    text: String,
) -> Result<String, nsfw::ModelError> {
    let first = classify(&text);
    if threshold_retry(&first) {
        let binary = image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
            let [r, g, b] = image.get_pixel(x, y).0;
            let luminance =
                (299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b) + 500) / 1000;
            let value = if luminance >= 192 { 255 } else { 0 };
            image::Rgb([value; 3])
        });
        if let Ok(candidate) = read_with_profile(&binary, OcrProfile::Block)
            && signals::high_contrast_candidate_blocks(&text, &candidate)
        {
            return Ok(candidate);
        }
    }
    Ok(text)
}

fn threshold_retry(first: &Classification) -> bool {
    let signals = &first.signals;
    first.decision != Decision::Block
        && !signals.editorial_context
        && (signals.commercial_keywords > 0
            || signals.call_to_action
            || signals.phone_number
            || signals.telegram_link
            || signals.web_url
            || signals.username)
}

fn readable_words(text: &str) -> usize {
    text.split(|c: char| !c.is_alphabetic() && c != '_')
        .filter(|word| word.chars().count() >= 4)
        .count()
}

fn looks_sideways(image: &image::RgbImage, text: &str) -> bool {
    if u64::from(image.height()) * 4 < u64::from(image.width()) * 5 || readable_words(text) >= 2 {
        return false;
    }
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    lines.len() >= 6
        && lines
            .iter()
            .filter(|line| line.chars().filter(|c| c.is_alphanumeric()).count() <= 3)
            .count()
            * 3
            >= lines.len() * 2
}

pub fn read_with_profile(
    image: &image::RgbImage,
    profile: OcrProfile,
) -> Result<String, nsfw::ModelError> {
    let runtime = runtime()?;
    let image = bounded_view(image);
    let temporary = create_private_ocr_temp_dir()?;
    read_with_profile_in_temporary(&image, profile, runtime, temporary)
}

fn read_with_options(
    image: &image::RgbImage,
    psm: &str,
    options: &[&str],
    extension: &str,
) -> Result<String, nsfw::ModelError> {
    let runtime = runtime()?;
    let temporary = create_private_ocr_temp_dir()?;
    read_with_options_in_temporary(image, psm, options, extension, runtime, temporary)
}

fn read_with_profile_in_temporary(
    image: &image::RgbImage,
    profile: OcrProfile,
    runtime: &Runtime,
    temporary: std::path::PathBuf,
) -> Result<String, nsfw::ModelError> {
    read_with_options_in_temporary(
        image,
        profile.page_segmentation_mode(),
        &[],
        "txt",
        runtime,
        temporary,
    )
}

fn read_with_options_in_temporary(
    image: &image::RgbImage,
    psm: &str,
    options: &[&str],
    extension: &str,
    runtime: &Runtime,
    temporary: std::path::PathBuf,
) -> Result<String, nsfw::ModelError> {
    let cleanup = TempOcr(temporary.clone());
    let input = temporary.join("input.ppm");
    let output_base = temporary.join("output");
    let error_path = temporary.join("stderr");
    let mut ppm = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();
    ppm.extend_from_slice(image.as_raw());
    std::fs::write(&input, ppm).map_err(|error| {
        nsfw::ModelError::Runtime(format!("could not write OCR input: {error}"))
    })?;
    let stderr = std::fs::File::create(&error_path).map_err(|error| {
        nsfw::ModelError::Runtime(format!("could not create OCR error file: {error}"))
    })?;
    let mut child = command(runtime)?
        .arg(&input)
        .arg(&output_base)
        .args(["-l", "fas+eng", "--psm", psm, "--dpi", "150"])
        .arg("--tessdata-dir")
        .arg(&runtime.tessdata)
        .args(options)
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            nsfw::ModelError::Runtime(format!("could not start embedded Tesseract: {error}"))
        })?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            nsfw::ModelError::Runtime(format!("could not wait for Tesseract: {error}"))
        })? {
            break status;
        }
        if started.elapsed() >= OCR_PROCESS_DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            return Err(nsfw::ModelError::Runtime(
                "Tesseract exceeded the two-second deadline".to_owned(),
            ));
        }
        std::thread::sleep(OCR_POLL);
    };
    if !status.success() {
        let detail = std::fs::read_to_string(&error_path)
            .unwrap_or_default()
            .chars()
            .take(1024)
            .collect::<String>();
        return Err(nsfw::ModelError::Runtime(format!(
            "Tesseract exited with {status}: {}",
            detail.trim()
        )));
    }
    let output = output_base.with_extension(extension);
    let size = std::fs::metadata(&output)
        .map_err(|error| nsfw::ModelError::Runtime(format!("missing OCR output: {error}")))?
        .len();
    if size > OCR_MAX_OUTPUT_BYTES {
        return Err(nsfw::ModelError::contract(format!(
            "Tesseract output exceeds {OCR_MAX_OUTPUT_BYTES} bytes"
        )));
    }
    let text = std::fs::read_to_string(&output)
        .map_err(|error| nsfw::ModelError::Runtime(format!("could not read OCR output: {error}")))?
        .chars()
        .take(if extension == "tsv" {
            OCR_MAX_OUTPUT_BYTES as usize
        } else {
            OCR_MAX_TEXT_CHARS
        })
        .collect::<String>();
    drop(cleanup);
    Ok(text.trim().to_owned())
}

struct TempOcr(std::path::PathBuf);

impl Drop for TempOcr {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "advert: could not remove OCR temporary directory {}: {error}",
                self.0.display()
            );
        }
    }
}

#[cfg(test)]
pub fn advertises(text: &str) -> Option<&'static str> {
    let result = classify(text);
    (result.decision == signals::Decision::Block).then(|| result.legacy_reason())
}

pub fn classify(text: &str) -> Classification {
    signals::classify(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    static OCR_TEMP_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn preprocessing_never_upscales_and_caps_the_long_side() {
        let small = image::RgbImage::new(320, 180);
        assert!(matches!(
            bounded_view(&small),
            std::borrow::Cow::Borrowed(_)
        ));
        let large = image::RgbImage::new(2400, 1200);
        let bounded = bounded_view(&large);
        assert_eq!(bounded.dimensions(), (1000, 500));
    }

    #[test]
    fn raw_line_retry_is_limited_to_four_coherent_bands() {
        let mut image = image::RgbImage::from_pixel(900, 650, image::Rgb([255, 255, 255]));
        for top in [65, 175, 285, 395, 505] {
            for y in top..top + 40 {
                for x in 300..600 {
                    image.put_pixel(x, y, image::Rgb([0, 0, 0]));
                }
            }
        }
        let boxes = line_band_boxes(&image);
        assert_eq!(boxes.len(), 4);
        assert_eq!(boxes[0].1, 53);
        assert_eq!(boxes[3].3, 448);
    }

    #[test]
    fn rotation_fallback_requires_fragmented_portrait_text() {
        let portrait = image::RgbImage::new(500, 800);
        let fragmented = "8\n0\n=\n9\no\n۳۳\nJ\n0\nts\n6 2\nQA 0";
        assert!(looks_sideways(&portrait, fragmented));
        assert_eq!(readable_words("ORDER @rotate_shop\nt.me/rotate"), 3);
        assert_eq!(readable_words("شماره 1234567\nمبلغ 100,000 تومان"), 3);
        assert!(!looks_sideways(
            &portrait,
            "ORDER @rotate_shop\nt.me/rotate"
        ));
        assert!(!looks_sideways(&image::RgbImage::new(800, 500), fragmented));
    }

    #[test]
    fn high_contrast_retry_needs_more_than_a_price_and_preserves_editorial_context() {
        assert!(threshold_retry(&classify("PROMO 091211234567")));
        assert!(threshold_retry(&classify("Contact 09121234567")));
        assert!(!threshold_retry(&classify("100,000 تومان")));
        assert!(!threshold_retry(&classify("RECEIPT SALE 09121234567")));
        assert!(!threshold_retry(&classify("SALE t.me/deals")));
    }

    #[test]
    fn high_contrast_retry_recovers_two_synthetic_ad_layouts() {
        for bytes in [
            &include_bytes!("../../tests/fixtures/media/ad-sparse-corners.png")[..],
            &include_bytes!("../../tests/fixtures/media/ad-small-compressed.jpg")[..],
        ] {
            let image = image::load_from_memory(bytes).unwrap().to_rgb8();
            let baseline = read_with_profile(&image, OcrProfile::Block).unwrap();
            assert_ne!(classify(&baseline).decision, Decision::Block);
            let result = read(&image).unwrap();
            assert_eq!(classify(&result).decision, Decision::Block);
        }
    }

    #[test]
    fn high_contrast_retry_does_not_promote_model_numbers_to_contacts() {
        for bytes in [
            &include_bytes!(
                "../../tests/fixtures/media/ad-market-discussion-model-number.png"
            )[..],
            &include_bytes!("../../tests/fixtures/media/ad-retail-model-number.png")[..],
        ] {
            let image = image::load_from_memory(bytes).unwrap().to_rgb8();
            let first = read_with_profile(&image, OcrProfile::Block).unwrap();
            assert_ne!(classify(&first).decision, Decision::Block);
            let result = read(&image).unwrap();
            assert_ne!(classify(&result).decision, Decision::Block);
        }
    }

    #[test]
    fn offline_profile_names_are_stable_and_default_remains_block6() {
        for (name, profile, psm) in [
            ("block6", OcrProfile::Block, "6"),
            ("auto3", OcrProfile::Auto, "3"),
            ("sparse11", OcrProfile::Sparse, "11"),
            ("raw13", OcrProfile::RawLine, "13"),
        ] {
            assert_eq!(OcrProfile::parse(name), Some(profile));
            assert_eq!(profile.name(), name);
            assert_eq!(profile.page_segmentation_mode(), psm);
        }
        assert_eq!(OcrProfile::parse(""), None);
        assert_eq!(OcrProfile::parse("sparse12"), None);
    }

    #[test]
    fn channel_screenshots_without_an_independent_ad_signal_are_not_deleted() {
        assert_eq!(
            advertises("12:28 $f166 TeleBotCraft 12.1Ksubscribers PinnedMessage X us"),
            Some("تصویر کانال تبلیغاتی")
        );
        assert_eq!(
            advertises("1:070X *1l64 TorexcIusN OETT newscenter CENTER 45.6Ksubscrib"),
            None
        );
    }

    #[test]
    fn it_catches_a_group_screenshot_too() {
        assert_eq!(
            advertises("33 2:43 : ID 6,642members,539online I C Message Unmute Leave"),
            None
        );
        assert_eq!(advertises("33 2:43 : ID 6,642members,539online"), None);
        assert_eq!(advertises("online now"), None);
        assert_eq!(advertises("team members"), None);
    }

    #[test]
    fn a_link_broken_across_boxes_still_counts() {
        assert_eq!(advertises("join t. me/promo"), Some("لینک تلگرام در تصویر"));
        assert_eq!(advertises("see t.me/news"), None);
        assert_eq!(
            advertises("shop at mystore .com now"),
            Some("آدرس سایت در تصویر")
        );
    }

    #[test]
    fn a_number_is_not_an_address() {
        assert!(!classify("12.1ksubscribers").signals.web_url);
        assert!(!classify("3.1416").signals.web_url);
        assert!(!classify("price45.60").signals.web_url);
        assert!(classify("visitmystore.com").signals.web_url);
    }

    #[test]
    fn it_recognises_the_markers_it_will_really_be_given() {
        assert_eq!(advertises("t.me/mvchanne"), None);
        assert_eq!(advertises("@mychannel"), Some("آیدی تبلیغاتی در تصویر"));
        assert_eq!(
            advertises("PROMO JOIN @promoNOW قیمت 100 تومان"),
            Some("قیمت و نشانه تجاری در تصویر")
        );

        assert_eq!(advertises("BLEDRC Ma"), None);
        assert_eq!(advertises(""), None);
        assert_eq!(advertises("@ab"), None);
        assert_eq!(advertises("price @ 20 usd"), None);
    }

    #[test]
    fn a_handle_is_only_a_signal_when_it_has_context() {
        assert!(classify("@promo_channel now").signals.username);
        assert!(classify("see @channel!").signals.username);
        assert!(!classify("name@example.com").signals.username);
    }

    #[test]
    fn the_tesseract_runtime_loads() {
        assert!(runtime().is_ok(), "the Tesseract runtime must publish");
        assert_eq!(capacity_probe(), 1);
        assert!(ENG.len() > 4_000_000, "English tessdata looks truncated");
        assert!(FAS.len() > 400_000, "Persian tessdata looks truncated");
    }

    #[test]
    fn transient_artifact_failure_can_be_retried() {
        let cell = OnceLock::new();
        let gate = Mutex::new(());
        let temporary = create_private_ocr_temp_dir().expect("private OCR directory");
        let blocked = temporary.join("blocked");
        std::fs::write(&blocked, b"obstruction").expect("create obstruction");
        let artifact = blocked.join("artifact");
        let initialize = || initialize_once_retry(&cell, &gate, || publish(&artifact, b"model"));
        assert!(initialize().is_err(), "a file cannot be a parent directory");
        assert!(
            cell.get().is_none(),
            "a temporary failure must not be cached"
        );

        std::fs::remove_file(&blocked).expect("remove obstruction");
        initialize().expect("retry after removing obstruction");
        assert!(file_matches(&artifact, b"model"));
        assert!(cell.get().is_some());

        std::fs::remove_file(&artifact).expect("remove test artifact");
        std::fs::remove_dir(&blocked).expect("remove test directory");
        std::fs::remove_dir(&temporary).expect("remove private OCR directory");
    }

    #[test]
    fn successful_ocr_removes_its_private_temporary_files() {
        let _test_guard = OCR_TEMP_TEST_LOCK.lock().unwrap();
        let temporary = create_private_ocr_temp_dir().expect("private OCR directory");
        let white = image::RgbImage::from_pixel(320, 180, image::Rgb([255, 255, 255]));
        assert_eq!(
            read_with_profile_in_temporary(
                &white,
                OcrProfile::Block,
                runtime().expect("embedded OCR runtime"),
                temporary.clone(),
            )
            .expect("blank OCR succeeds"),
            ""
        );
        assert!(
            !temporary.exists(),
            "OCR input/output must not survive the call"
        );
    }

    #[test]
    fn ocr_temporary_directories_are_unique_and_private() {
        let _test_guard = OCR_TEMP_TEST_LOCK.lock().unwrap();
        let first = create_private_ocr_temp_dir().expect("first private OCR directory");
        let second = create_private_ocr_temp_dir().expect("second private OCR directory");
        assert_ne!(first, second);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for directory in [&first, &second] {
                let mode = std::fs::metadata(directory)
                    .expect("OCR directory metadata")
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o077, 0, "OCR workspace must exclude group/other");
                assert_eq!(mode & 0o700, 0o700, "OCR owner needs full workspace access");
            }
        }
        std::fs::remove_dir(first).expect("remove first test directory");
        std::fs::remove_dir(second).expect("remove second test directory");
    }

    #[test]
    #[ignore = "set OCR_BENCH_IMAGES to semicolon-separated image paths"]
    fn benchmark_images_from_environment() {
        let paths = std::env::var("OCR_BENCH_IMAGES").expect("OCR_BENCH_IMAGES is required");
        let long_sides = std::env::var("OCR_BENCH_LONG_SIDES")
            .unwrap_or_else(|_| "0".to_owned())
            .split(',')
            .map(|side| side.parse::<u32>().expect("long sides are integers"))
            .collect::<Vec<_>>();
        for path in paths.split(';').filter(|path| !path.is_empty()) {
            let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"));
            let original = image::load_from_memory(&bytes)
                .unwrap_or_else(|error| panic!("{path}: {error}"))
                .to_rgb8();
            for long_side in &long_sides {
                let image = if *long_side == 0
                    || *long_side >= original.width().max(original.height())
                {
                    original.clone()
                } else {
                    let scale = *long_side as f32 / original.width().max(original.height()) as f32;
                    image::imageops::resize(
                        &original,
                        (original.width() as f32 * scale).round().max(1.0) as u32,
                        (original.height() as f32 * scale).round().max(1.0) as u32,
                        image::imageops::FilterType::Triangle,
                    )
                };

                let cold_started = std::time::Instant::now();
                let cold_text = read(&image).unwrap_or_else(|error| panic!("{path}: {error}"));
                let cold = cold_started.elapsed();
                let mut warm = Vec::with_capacity(10);
                for _ in 0..10 {
                    let started = std::time::Instant::now();
                    let text = read(&image).unwrap_or_else(|error| panic!("{path}: {error}"));
                    assert_eq!(text, cold_text, "OCR must be deterministic for {path}");
                    warm.push(started.elapsed());
                }
                warm.sort_unstable();
                eprintln!(
                    "ocr-bench path={path} requested_long_side={long_side} pixels={}x{} bytes={} cold_ms={} p50_ms={} p95_ms={} decision={:?} text={cold_text:?}",
                    image.width(),
                    image.height(),
                    bytes.len(),
                    cold.as_millis(),
                    warm[5].as_millis(),
                    warm[9].as_millis(),
                    classify(&cold_text).decision
                );
            }
        }
    }

    #[test]
    #[ignore = "set OCR_BENCH_IMAGES to at least one image path"]
    fn benchmark_bounded_concurrency_from_environment() {
        const PASSES_PER_WORKER: usize = 6;
        let path = std::env::var("OCR_BENCH_IMAGES")
            .expect("OCR_BENCH_IMAGES is required")
            .split(';')
            .next()
            .expect("at least one image path")
            .to_owned();
        let image = image::load_from_memory(&std::fs::read(&path).expect("benchmark image bytes"))
            .expect("benchmark image")
            .to_rgb8();
        read(&image).expect("warm OCR models");

        for workers in [1usize, 2, 4] {
            let started = std::time::Instant::now();
            std::thread::scope(|scope| {
                let handles = (0..workers)
                    .map(|_| {
                        scope.spawn(|| {
                            for _ in 0..PASSES_PER_WORKER {
                                read(&image).expect("concurrent OCR pass");
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                for handle in handles {
                    handle.join().expect("OCR worker did not panic");
                }
            });
            let elapsed = started.elapsed();
            let passes = workers * PASSES_PER_WORKER;
            eprintln!(
                "ocr-throughput workers={workers} sessions={} passes={passes} wall_ms={} images_per_second={:.2}",
                nsfw::infer_sessions(),
                elapsed.as_millis(),
                passes as f64 / elapsed.as_secs_f64()
            );
        }
    }
}
