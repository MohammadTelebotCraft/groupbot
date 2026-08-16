use super::nsfw;

pub const LOCK: &str = "advert";

pub const SHADOW: &str = "ad_shadow";

const DET: &[u8] = include_bytes!("../../assets/ocr_det.onnx");
const REC: &[u8] = include_bytes!("../../assets/ocr_rec.onnx");
const DICT: &str = include_str!("../../assets/ocr_dict.txt");

const DET_SIDE: u32 = 640;

const REC_HEIGHT: u32 = 48;

const INK: f32 = 0.3;

const SPECKLE: usize = 12;

const MAX_BOXES: usize = 16;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

fn detector() -> Option<&'static nsfw::Session> {
    static CELL: std::sync::OnceLock<Option<nsfw::Session>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| nsfw::open(DET, "text detector")).as_ref()
}

fn reader() -> Option<&'static nsfw::Session> {
    static CELL: std::sync::OnceLock<Option<nsfw::Session>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| nsfw::open(REC, "text reader")).as_ref()
}

fn dictionary() -> &'static Vec<&'static str> {
    static CELL: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| DICT.lines().collect())
}

type Box = (u32, u32, u32, u32);

fn detect(image: &image::RgbImage) -> Option<Vec<Box>> {
    let session = detector()?;
    let (width, height) = image.dimensions();
    let scale = (DET_SIDE as f32 / width.max(height) as f32).min(1.0);

    let to = |value: u32| ((value as f32 * scale) as u32).max(32).div_ceil(32) * 32;
    let (wide, tall) = (to(width), to(height));

    let view = image::imageops::resize(image, wide, tall, image::imageops::FilterType::Triangle);
    let raw = view.as_raw();
    let plane = (wide * tall) as usize;
    let mut input = Vec::with_capacity(3 * plane);
    for channel in 0..3 {
        for at in 0..plane {
            input.push((f32::from(raw[at * 3 + channel]) / 255.0 - MEAN[channel]) / STD[channel]);
        }
    }

    let (shape, probability) =
        nsfw::run_shaped(session, vec![1, 3, i64::from(tall), i64::from(wide)], input)?;
    if shape.len() < 4 {
        return None;
    }
    let (rows, columns) = (shape[2] as usize, shape[3] as usize);
    if probability.len() < rows * columns {
        return None;
    }

    let across = width as f32 / columns as f32;
    let down = height as f32 / rows as f32;
    let mut seen = vec![false; rows * columns];
    let mut boxes: Vec<Box> = Vec::new();

    for start in 0..rows * columns {
        if seen[start] || probability[start] < INK {
            continue;
        }
        let (mut left, mut top, mut right, mut bottom) = (columns, rows, 0usize, 0usize);
        let mut stack = vec![start];
        let mut ink = 0usize;
        seen[start] = true;
        while let Some(at) = stack.pop() {
            ink += 1;
            let (y, x) = (at / columns, at % columns);
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= columns as i32 || ny >= rows as i32 {
                    continue;
                }
                let next = ny as usize * columns + nx as usize;
                if !seen[next] && probability[next] >= INK {
                    seen[next] = true;
                    stack.push(next);
                }
            }
        }
        if ink < SPECKLE {
            continue;
        }
        let height_of = (bottom - top + 1) as f32;
        let pad_x = height_of * 0.6;
        let pad_y = height_of * 0.3;
        let x0 = ((left as f32 - pad_x) * across).max(0.0);
        let y0 = ((top as f32 - pad_y) * down).max(0.0);
        let x1 = ((right as f32 + 1.0 + pad_x) * across).min(width as f32);
        let y1 = ((bottom as f32 + 1.0 + pad_y) * down).min(height as f32);
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        if w > 4 && h > 4 {
            boxes.push((x0 as u32, y0 as u32, w, h));
        }
    }

    boxes.sort_by_key(|(x, y, ..)| (*y, *x));
    Some(boxes)
}

fn read_box(image: &image::RgbImage, at: Box) -> Option<String> {
    let session = reader()?;
    let crop = image::imageops::crop_imm(image, at.0, at.1, at.2, at.3).to_image();
    let (width, height) = crop.dimensions();
    let wide = ((width as f32 * REC_HEIGHT as f32 / height.max(1) as f32) as u32).clamp(16, 1200);
    let view = image::imageops::resize(&crop, wide, REC_HEIGHT, image::imageops::FilterType::Triangle);

    let raw = view.as_raw();
    let plane = (wide * REC_HEIGHT) as usize;
    let mut input = Vec::with_capacity(3 * plane);
    for channel in 0..3 {
        for i in 0..plane {
            input.push(f32::from(raw[i * 3 + channel]) / 127.5 - 1.0);
        }
    }

    let (shape, logits) =
        nsfw::run_shaped(session, vec![1, 3, i64::from(REC_HEIGHT), i64::from(wide)], input)?;
    if shape.len() < 3 {
        return None;
    }
    let (steps, classes) = (shape[1] as usize, shape[2] as usize);
    let dictionary = dictionary();

    let mut text = String::new();
    let mut previous = usize::MAX;
    for step in 0..steps {
        let row = logits.get(step * classes..(step + 1) * classes)?;
        let mut best = 0usize;
        let mut score = f32::MIN;
        for (index, value) in row.iter().enumerate() {
            if *value > score {
                score = *value;
                best = index;
            }
        }
        if best != 0
            && best != previous
            && let Some(character) = dictionary.get(best - 1)
        {
            text.push_str(character);
        }
        previous = best;
    }
    Some(text)
}

pub fn read(image: &image::RgbImage) -> Option<String> {
    let boxes = detect(image)?;
    if boxes.is_empty() {
        return Some(String::new());
    }
    let mut words: Vec<String> = Vec::new();
    for at in boxes.into_iter().take(MAX_BOXES) {
        if let Some(text) = read_box(image, at)
            && !text.trim().is_empty()
        {
            words.push(text);
        }
    }
    Some(words.join(" "))
}

const CHROME: [&str; 6] = [
    "subscrib",
    "pinnedmessage",
    "joinchannel",
    "viewinchannel",
    "sendmessage",
    "unmute",
];

const TLDS: [&str; 10] = [
    "com", "net", "org", "ir", "io", "me", "shop", "site", "xyz", "info",
];

pub fn advertises(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    let tight: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    let squeezed = squeeze(&lower);

    if super::locks::has_telegram_link(&lower) || super::locks::has_telegram_link(&squeezed) {
        return Some("لینک تلگرام در تصویر");
    }
    if handle_of(&lower).is_some() {
        return Some("آیدی کانال در تصویر");
    }

    if CHROME.iter().any(|marker| tight.contains(marker)) {
        return Some("تصویر کانال تبلیغاتی");
    }

    if tight.contains("member") && tight.contains("online") {
        return Some("تصویر گروه تبلیغاتی");
    }
    if domain_in(&squeezed).is_some() {
        return Some("آدرس سایت در تصویر");
    }
    None
}

fn squeeze(lower: &str) -> String {
    let mut out = String::with_capacity(lower.len());
    let joins = |c: char| matches!(c, '.' | '/' | '@' | ':');
    for (at, c) in lower.char_indices() {
        if c.is_whitespace() {
            let before = out.chars().next_back().is_some_and(joins);
            let after = lower[at..].trim_start().starts_with(joins);
            if before || after {
                continue;
            }
        }
        out.push(c);
    }
    out
}

fn handle_of(lower: &str) -> Option<&str> {
    let at = lower.find('@')?;
    let rest = &lower[at + 1..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    (end >= 5).then(|| &rest[..end])
}

fn domain_in(tight: &str) -> Option<&str> {
    for (at, _) in tight.match_indices('.') {
        let label = tight[..at]
            .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .next()
            .unwrap_or("");
        if label.len() < 3 || !label.chars().any(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        let rest = &tight[at + 1..];
        let Some(suffix) = TLDS.iter().find(|tld| rest.starts_with(**tld)) else {
            continue;
        };

        if rest[suffix.len()..].starts_with(|c: char| c.is_ascii_alphanumeric()) {
            continue;
        }
        return Some(label);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_catches_the_channel_screenshots_that_got_through() {
        assert_eq!(
            advertises("12:28 $f166 TeleBotCraft 12.1Ksubscribers PinnedMessage X us"),
            Some("تصویر کانال تبلیغاتی")
        );
        assert_eq!(
            advertises("1:070X *1l64 TorexcIusN OETT newscenter CENTER 45.6Ksubscrib"),
            Some("تصویر کانال تبلیغاتی")
        );
    }

    #[test]
    fn it_catches_a_group_screenshot_too() {
        assert_eq!(
            advertises("33 2:43 : ID 6,642members,539online I C Message Unmute Leave"),
            Some("تصویر کانال تبلیغاتی")
        );

        assert_eq!(
            advertises("33 2:43 : ID 6,642members,539online"),
            Some("تصویر گروه تبلیغاتی")
        );

        assert_eq!(advertises("online now"), None);
        assert_eq!(advertises("team members"), None);
    }

    #[test]
    fn a_link_broken_across_boxes_still_counts() {
        assert_eq!(advertises("join t. me/promo"), Some("لینک تلگرام در تصویر"));
        assert_eq!(advertises("shop at mystore .com now"), Some("آدرس سایت در تصویر"));
    }

    #[test]
    fn a_number_is_not_an_address() {
        assert_eq!(domain_in("12.1ksubscribers"), None);
        assert_eq!(domain_in("3.1416"), None);
        assert_eq!(domain_in("price45.60"), None);
        assert_eq!(domain_in("canon.company"), None);
        assert_eq!(domain_in("visitmystore.com"), Some("visitmystore"));
    }

    #[test]
    fn it_recognises_the_markers_it_will_really_be_given() {
        assert_eq!(advertises("t.me/mvchanne"), Some("لینک تلگرام در تصویر"));
        assert_eq!(advertises("@mychannel"), Some("آیدی کانال در تصویر"));
        assert_eq!(advertises("JOIN@promoNOW"), Some("آیدی کانال در تصویر"));

        assert_eq!(advertises("BLEDRC Ma"), None);
        assert_eq!(advertises(""), None);

        assert_eq!(advertises("@ab"), None);
        assert_eq!(advertises("price @ 20 usd"), None);
    }

    #[test]
    fn a_handle_stops_at_what_cannot_be_in_one() {
        assert_eq!(handle_of("@promo_channel now"), Some("promo_channel"));
        assert_eq!(handle_of("see @channel!"), Some("channel"));
        assert_eq!(handle_of("nothing here"), None);
    }

    #[test]
    fn the_models_and_the_dictionary_load() {
        assert!(detector().is_some(), "the text detector must load");
        assert!(reader().is_some(), "the text reader must load");
        assert!(dictionary().len() > 90, "the dictionary looks truncated");
    }
}
