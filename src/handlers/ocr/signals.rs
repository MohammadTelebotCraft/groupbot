use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Allow,
    Suspicious,
    Block,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasonCode {
    CommercialKeyword,
    CallToAction,
    PhoneNumber,
    Price,
    TelegramLink,
    WebUrl,
    Username,
    TelegramUi,
    EditorialContext,
    ActiveDiscountOffer,
}

impl ReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CommercialKeyword => "commercial_keyword",
            Self::CallToAction => "call_to_action",
            Self::PhoneNumber => "phone_number",
            Self::Price => "price",
            Self::TelegramLink => "telegram_link",
            Self::WebUrl => "web_url",
            Self::Username => "username",
            Self::TelegramUi => "telegram_ui_signature",
            Self::EditorialContext => "editorial_context",
            Self::ActiveDiscountOffer => "active_discount_offer",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignalSet {
    pub commercial_keywords: u8,
    pub strong_commercial_keywords: u8,
    pub call_to_action: bool,
    pub phone_number: bool,
    pub price: bool,
    pub telegram_link: bool,
    pub web_url: bool,
    pub username: bool,
    pub telegram_ui_markers: u8,
    pub subscriber_or_group_metric: bool,
    pub editorial_context: bool,
}

impl SignalSet {
    fn family_count(&self) -> usize {
        usize::from(self.commercial_keywords > 0)
            + usize::from(self.call_to_action)
            + usize::from(self.phone_number)
            + usize::from(self.price)
            + usize::from(self.telegram_link)
            + usize::from(self.web_url)
            + usize::from(self.username)
            + usize::from(self.telegram_ui_markers > 0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Classification {
    pub decision: Decision,
    pub signals: SignalSet,
    pub reasons: Vec<ReasonCode>,
    pub matched_terms: Vec<&'static str>,
}

impl Classification {
    pub fn legacy_reason(&self) -> &'static str {
        if self.reasons.contains(&ReasonCode::TelegramUi) {
            "تصویر کانال تبلیغاتی"
        } else if self.reasons.contains(&ReasonCode::TelegramLink) {
            "لینک تلگرام در تصویر"
        } else if self.reasons.contains(&ReasonCode::PhoneNumber) {
            "شماره تماس تبلیغاتی در تصویر"
        } else if self.reasons.contains(&ReasonCode::Price) {
            "قیمت و نشانه تجاری در تصویر"
        } else if self.reasons.contains(&ReasonCode::WebUrl) {
            "آدرس سایت در تصویر"
        } else if self.reasons.contains(&ReasonCode::Username) {
            "آیدی تبلیغاتی در تصویر"
        } else {
            "نشانه های تبلیغاتی در تصویر"
        }
    }
}

const PERSIAN_COMMERCIAL: &[&str] = &[
    "فروش",
    "خرید",
    "قیمت",
    "تخفیف",
    "سفارش",
    "ارسال",
    "تماس",
    "شمارهتماس",
    "لینک",
    "کانال",
    "پیج",
];

const ENGLISH_COMMERCIAL: &[&str] = &[
    "sale", "buy", "sell", "price", "discount", "promo", "offer", "shop",
];

const STRONG_COMMERCIAL: &[&str] = &[
    "فروش",
    "خرید",
    "تخفیف",
    "sale",
    "buy",
    "sell",
    "discount",
    "promo",
    "offer",
    "shop",
];

const EDITORIAL: &[&str] = &[
    "خبر",
    "گزارش",
    "آموزش",
    "رسید",
    "فاکتور",
    "بررسی",
    "نوشت",
    "گفت",
    "news",
    "report",
    "tutorial",
    "receipt",
    "invoice",
    "discussion",
    "quote",
    "quoted",
];

const CTA: &[&str] = &[
    "ثبتسفارش",
    "سفارشدهید",
    "پیامبده",
    "تماسبگیرید",
    "دایرکت",
    "order now",
    "ordernow",
    "dm us",
    "dmfor",
    "contact us",
    "contactus",
    "shop now",
    "shopnow",
    "buy now",
    "buynow",
];

const CURRENCY: &[&str] = &["تومان", "تومن", "ریال", "میلیون", "million", "usdt", "ton"];

const TLDS: &[&str] = &[
    "com", "net", "org", "ir", "io", "me", "shop", "site", "xyz", "info", "co", "app", "dev",
    "store", "online", "pro", "biz", "ly", "gg", "ai", "edu", "gov",
];

const TELEGRAM_HOSTS: &[&str] = &["t.me", "telegram.me", "telegram.dog"];

pub fn normalize(text: &str) -> String {
    text.nfkc()
        .flat_map(|c| {
            if is_format_or_mark(c) || c == '\u{0640}' {
                None
            } else {
                Some(match c {
                    '\u{064a}' | '\u{0649}' => 'ی',
                    '\u{0643}' => 'ک',
                    '\u{0660}' | '\u{06f0}' => '0',
                    '\u{0661}' | '\u{06f1}' => '1',
                    '\u{0662}' | '\u{06f2}' => '2',
                    '\u{0663}' | '\u{06f3}' => '3',
                    '\u{0664}' | '\u{06f4}' => '4',
                    '\u{0665}' | '\u{06f5}' => '5',
                    '\u{0666}' | '\u{06f6}' => '6',
                    '\u{0667}' | '\u{06f7}' => '7',
                    '\u{0668}' | '\u{06f8}' => '8',
                    '\u{0669}' | '\u{06f9}' => '9',
                    other => other,
                })
            }
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_format_or_mark(c: char) -> bool {
    matches!(
        c,
        '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{e0000}'..='\u{e007f}'
            | '\u{064b}'..='\u{065f}'
            | '\u{0670}'
    )
}

fn compact_letters(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || is_persian_letter(*c))
        .collect()
}

fn is_persian_letter(c: char) -> bool {
    matches!(c, '\u{0600}'..='\u{06ff}' | '\u{0750}'..='\u{077f}')
}

fn obfuscated_latin(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '0' => 'o',
            '1' => 'i',
            '3' => 'e',
            '5' => 's',
            '7' => 't',
            _ => c,
        })
        .collect()
}

fn contains_term(normalized: &str, compact: &str, term: &str) -> bool {
    let term_compact: String = term.chars().filter(|c| !c.is_whitespace()).collect();
    if term.is_ascii() {
        let bounded = |text: &str| {
            text.match_indices(term).any(|(at, _)| {
                let before = text[..at].chars().next_back();
                let after = text[at + term.len()..].chars().next();
                !before.is_some_and(|c| c.is_ascii_alphanumeric())
                    && !after.is_some_and(|c| c.is_ascii_alphanumeric())
            })
        };
        bounded(normalized)
            || bounded(&obfuscated_latin(normalized))
            || (term_compact.len() >= 4
                && (compact.starts_with(&term_compact)
                    || obfuscated_latin(compact).starts_with(&term_compact)))
    } else {
        compact.contains(&term_compact) || normalized.contains(term)
    }
}

fn collect_terms(normalized: &str, compact: &str) -> Vec<&'static str> {
    PERSIAN_COMMERCIAL
        .iter()
        .chain(ENGLISH_COMMERCIAL)
        .copied()
        .filter(|term| contains_term(normalized, compact, term))
        .collect()
}

fn has_cta(normalized: &str, compact: &str) -> bool {
    CTA.iter()
        .any(|term| contains_term(normalized, compact, term))
        || normalized
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| word == "dm")
        || (contains_term(normalized, compact, "contact")
            && contains_term(normalized, compact, "now"))
        || (contains_term(normalized, compact, "shop") && contains_term(normalized, compact, "now"))
}

fn completed_support_notice(compact: &str) -> bool {
    compact.contains("تماسپشتیبانی")
        && compact.contains("فروشلغوشد")
        && compact.contains("سفارشبستهشد")
        && !["فروشجدید", "فروشویژه", "فروشفوری", "سفارشدهید", "خریدکنید"]
            .iter()
            .any(|cue| compact.contains(cue))
}

fn previous_sale_closed_notice(normalized: &str, signals: &SignalSet) -> bool {
    if !signals.phone_number
        || signals.call_to_action
        || signals.telegram_link
        || signals.web_url
        || signals.username
        || signals.telegram_ui_markers > 0
        || signals.subscriber_or_group_metric
    {
        return false;
    }
    let lines: Vec<_> = normalized
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    for (at, status) in lines.iter().enumerate() {
        if at + 1 >= lines.len() || status.chars().count() > 100 {
            continue;
        }
        let Some((sale, order)) = status.split_once('؛') else {
            continue;
        };
        let sale_compact = compact_letters(sale);
        let order_compact = compact_letters(order);
        let damaged_cancel = sale.split_once("فروش قبلی").is_some_and(|(_, rest)| {
            rest.split_once("شد").is_some_and(|(between, _)| {
                let noise = between.trim();
                (1..=5).contains(&noise.chars().count())
                    && noise.chars().any(|c| c.is_ascii_digit())
                    && noise
                        .chars()
                        .all(|c| c.is_ascii_digit() || c.is_ascii_punctuation())
            })
        });
        if !(sale_compact.contains("فروشقبلیلغوشد") || damaged_cancel)
            || !order_compact.contains("سفارشبستهشد")
            || order_compact.contains("فروش")
            || [
                "جدید",
                "ویژه",
                "امروز",
                "فوری",
                "تخفیف",
                "قیمت",
                "پیشنهاد",
                "خرید",
            ]
            .iter()
            .any(|cue| order_compact.contains(cue))
        {
            continue;
        }
        let following = &lines[at + 1..lines.len().min(at + 5)];
        let tracking = following
            .iter()
            .position(|line| compact_letters(line).contains("شمارهپیگیری"));
        let support = following
            .iter()
            .position(|line| compact_letters(line).contains("تماسپشتیبانی"));
        if !matches!((tracking, support), (Some(t), Some(s)) if t < s && t <= 1 && s <= t + 2) {
            continue;
        }
        if lines.iter().enumerate().any(|(other_at, line)| {
            other_at != at
                && [
                    "فروش",
                    "خرید",
                    "تخفیف",
                    "قیمت",
                    "پیشنهاد",
                    "سفارشدهید",
                    "ثبتسفارش",
                ]
                .iter()
                .any(|cue| compact_letters(line).contains(cue))
        }) {
            continue;
        }
        return true;
    }
    false
}

fn damaged_closed_support_notice(normalized: &str, signals: &SignalSet) -> bool {
    if !signals.phone_number
        || signals.strong_commercial_keywords == 0
        || signals.call_to_action
        || signals.telegram_link
        || signals.web_url
        || signals.username
        || signals.telegram_ui_markers > 0
        || signals.subscriber_or_group_metric
    {
        return false;
    }
    let compact = compact_letters(normalized);
    if [
        "جدید",
        "ویژه",
        "امروز",
        "فوری",
        "تخفیف",
        "قیمت",
        "سفارشدهید",
        "خریدکنید",
        "تومان",
        "تومن",
        "ریال",
        "میلیون",
    ]
    .iter()
    .any(|cue| compact.contains(cue))
        || normalized.chars().any(|c| matches!(c, '$' | '€' | '£'))
    {
        return false;
    }
    let lines = normalized
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    for (at, status) in lines.iter().enumerate() {
        if status.chars().count() > 90 || at + 2 >= lines.len() {
            continue;
        }
        let Some((sale, order)) = status.split_once('؛') else {
            continue;
        };
        let sale_compact = compact_letters(sale);
        let order_compact = compact_letters(order);
        let damaged_order =
            sale_compact.contains("فروشلغوشد") && order_compact.contains("سفارشبستهبشید");
        let damaged_cancel = order_compact.contains("سفارشبستهشد")
            && sale.split_once("فروش").is_some_and(|(_, after_sale)| {
                after_sale.split_once("شد").is_some_and(|(between, _)| {
                    let noise = between.trim();
                    (1..=5).contains(&noise.chars().count())
                        && noise.chars().any(|c| c.is_ascii_digit())
                        && noise
                            .chars()
                            .all(|c| c.is_ascii_digit() || c.is_ascii_punctuation())
                })
            });
        if !(damaged_order || damaged_cancel)
            || !compact_letters(lines[at + 1]).contains("شمارهپیگیری")
            || !compact_letters(lines[at + 2]).contains("تماسپشتیبانی")
        {
            continue;
        }
        if lines.iter().enumerate().any(|(other_at, line)| {
            other_at != at
                && ["فروش", "خرید", "تخفیف", "قیمت", "پیشنهاد"]
                    .iter()
                    .any(|cue| compact_letters(line).contains(cue))
        }) {
            continue;
        }
        return true;
    }
    false
}

fn active_discount_after_closed_support(normalized: &str) -> bool {
    let lines = normalized
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    lines.windows(4).any(|window| {
        let [status, offer, tracking, support] = window else {
            return false;
        };
        if status.chars().count() > 90 || offer.chars().count() > 90 {
            return false;
        }
        let Some((sale, order)) = status.split_once('؛') else {
            return false;
        };
        let sale = compact_letters(sale);
        let order = compact_letters(order);
        let offer_compact = compact_letters(offer);
        sale.contains("فروش")
            && sale.contains("شد")
            && order.contains("سفارشبستهشد")
            && offer_compact
                .find("تخفیف")
                .is_some_and(|discount| offer_compact[discount + "تخفیف".len()..].contains("خرید"))
            && (offer_compact.contains("تخفیفویژهامروز") || offer_compact.contains("خریدکنید"))
            && ![
                "خبر",
                "گزارش",
                "رسید",
                "فاکتور",
                "قبلی",
                "گذشته",
                "لغو",
                "نکنید",
                "نیست",
                "تکذیب",
                "تمام",
                "پایان",
            ]
            .iter()
            .any(|cue| offer_compact.contains(cue))
            && !offer
                .chars()
                .any(|c| matches!(c, '«' | '»' | '"' | '“' | '”'))
            && compact_letters(tracking).contains("شمارهپیگیری")
            && compact_letters(support).contains("تماسپشتیبانی")
    })
}

fn digit_value(c: char) -> Option<char> {
    c.is_ascii_digit().then_some(c)
}

fn number_candidates(normalized: &str) -> impl Iterator<Item = (usize, String)> + '_ {
    let mut candidates = Vec::new();
    let mut current = String::new();
    let mut start = 0;
    for (at, c) in normalized.char_indices() {
        if digit_value(c).is_some()
            || matches!(
                c,
                '+' | '-' | '(' | ')' | '.' | ',' | '\u{066b}' | '\u{066c}' | ' '
            )
        {
            if current.is_empty() {
                start = at;
            }
            current.push(c);
        } else {
            if current.chars().filter(|c| c.is_ascii_digit()).count() >= 3 {
                candidates.push((start, std::mem::take(&mut current)));
            } else {
                current.clear();
            }
        }
    }
    if current.chars().filter(|c| c.is_ascii_digit()).count() >= 3 {
        candidates.push((start, current));
    }
    candidates.into_iter()
}

fn product_identifier_before(normalized: &str, at: usize) -> bool {
    let prefix = normalized[..at]
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '#' | '=' | '-'));
    const LABELS: &[&str] = &[
        "product code",
        "item code",
        "model number",
        "sku",
        "serial number",
        "barcode",
        "parcel",
        "tracking number",
        "کد محصول",
        "شماره کالا",
        "شماره مدل",
        "شماره سریال",
        "شماره نمونه",
        "شماره پیگیری",
    ];
    LABELS.iter().any(|label| {
        prefix.strip_suffix(label).is_some_and(|before| {
            before
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
        })
    })
}

fn near_word(observed: &str, expected: &str) -> bool {
    observed.chars().count() == expected.chars().count()
        && observed
            .chars()
            .zip(expected.chars())
            .filter(|(left, right)| left != right)
            .count()
            <= 1
}

fn completed_sale_before(normalized: &str, line_start: usize) -> bool {
    let delivered = normalized[line_start..]
        .lines()
        .skip(1)
        .take(2)
        .any(|line| {
            let compact = compact_letters(line);
            ["تحویلکاملشد", "مرسولهتحویلشد", "بستهدریافتشد"]
                .iter()
                .any(|cue| compact.contains(cue))
        });
    for (skipped_noise, prior) in normalized[..line_start]
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        let compact = compact_letters(prior);
        if ["جدید", "ویژه", "امروز", "فوری"]
            .iter()
            .any(|term| compact.contains(term))
        {
            return false;
        }
        let sale = ["فروش", "خرید", "تخفیف", "سفارش"]
            .iter()
            .any(|term| compact.contains(term));
        let exact_status = ["قبلی", "گذشته", "بستهشد", "تمامشد", "انجامشد", "انجامشده"]
            .iter()
            .any(|term| compact.contains(term));
        let words = prior
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>();
        let completed = exact_status
            || words.windows(2).any(|pair| {
                (pair[1] == "یافت" && near_word(pair[0], "پایان"))
                    || (pair[1] == "شد" && near_word(pair[0], "لغو"))
            });
        let damaged_status = words.windows(3).any(|triplet| {
            triplet == ["فروش", "oll", "یافت"] || triplet == ["یافت", "oll", "فروش"]
        });
        if sale && (completed || (damaged_status && delivered && !has_cta(prior, &compact))) {
            return true;
        }
        if ["dm", "call", "contact", "order"]
            .iter()
            .any(|cue| compact.contains(cue))
        {
            return false;
        }
        if skipped_noise == 2 || prior.chars().filter(|c| c.is_alphabetic()).count() > 3 {
            return false;
        }
    }
    false
}

fn completed_tracking_before(normalized: &str, at: usize) -> bool {
    let line_start = normalized[..at].rfind('\n').map_or(0, |at| at + 1);
    let prefix = normalized[line_start..at].trim_end_matches(|c: char| !c.is_alphanumeric());
    let mut words = prefix
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .rev();
    if !words.next().is_some_and(|word| near_word(word, "پیگیری"))
        || words.next() != Some("شماره")
        || ["جدید", "ویژه", "امروز", "فوری"]
            .iter()
            .any(|term| compact_letters(prefix).contains(term))
    {
        return false;
    }
    completed_sale_before(normalized, line_start)
}

fn completed_tracking_after(normalized: &str, at: usize, number: &str) -> bool {
    let suffix = normalized[at + number.len()..]
        .split('\n')
        .next()
        .unwrap_or("");
    let mut label_words = suffix
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty());
    let mut first = label_words.next();
    if first.is_some_and(|word| {
        (1..=2).contains(&word.chars().count()) && word.chars().all(is_persian_letter)
    }) {
        first = label_words.next();
    }
    if first != Some("شماره")
        || !label_words
            .next()
            .is_some_and(|word| near_word(word, "پیگیری"))
    {
        return false;
    }

    let line_start = normalized[..at].rfind('\n').map_or(0, |at| at + 1);
    let same_line_prefix =
        normalized[line_start..at].trim_end_matches(|c: char| !c.is_alphanumeric());
    if ["تماس", "سفارش", "call", "contact", "order"]
        .iter()
        .any(|cue| same_line_prefix.ends_with(cue))
    {
        return false;
    }
    completed_sale_before(normalized, line_start)
}

fn identifier_for_number(normalized: &str, at: usize, number: &str) -> bool {
    product_identifier_before(normalized, at)
        || completed_tracking_before(normalized, at)
        || completed_tracking_after(normalized, at, number)
}

fn contact_with_trailing_digit(normalized: &str, at: usize, candidate: &str) -> bool {
    let mut parts = candidate.split_whitespace();
    let (Some(phone), Some(noise), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if phone.len() != 11
        || !phone.starts_with("09")
        || !phone.bytes().all(|byte| byte.is_ascii_digit())
        || noise.len() != 1
        || !noise.as_bytes()[0].is_ascii_digit()
    {
        return false;
    }
    let prefix = normalized[..at]
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '#' | '=' | '-'));
    if ["شماره سفارش", "کد سفارش", "order id"]
        .iter()
        .any(|label| prefix.ends_with(label))
    {
        return false;
    }
    ["تماس", "سفارش", "call", "contact", "order"]
        .iter()
        .any(|cue| {
            prefix.strip_suffix(cue).is_some_and(|before| {
                before
                    .chars()
                    .next_back()
                    .is_none_or(|character| !character.is_alphanumeric())
            })
        })
}

fn phone_number(normalized: &str) -> bool {
    number_candidates(normalized).any(|(at, candidate)| {
        if identifier_for_number(normalized, at, &candidate) {
            return false;
        }
        let digits: String = candidate.chars().filter_map(digit_value).collect();
        let contact_tail = contact_with_trailing_digit(normalized, at, &candidate);
        let candidate = candidate.trim_start();
        (digits.len() == 11 && digits.starts_with("09"))
            || (digits.len() == 12 && digits.starts_with("98"))
            || (digits.len() == 14 && digits.starts_with("0098"))
            || (candidate.starts_with('+') && (10..=15).contains(&digits.len()))
            || contact_tail
    })
}

pub(super) fn recovered_line_contact(first: &str, line: &str, first_cta: bool) -> Option<String> {
    let first = normalize(first);
    let mut line = normalize(line);
    for (at, candidate) in number_candidates(&line).collect::<Vec<_>>() {
        let digits = candidate
            .chars()
            .filter_map(digit_value)
            .collect::<String>();
        let contact_tail = contact_with_trailing_digit(&line, at, &candidate);
        let contact_digits = if contact_tail {
            candidate.split_whitespace().next().unwrap_or("")
        } else {
            &digits
        };
        let full_phone = digits.len() == 11 && digits.starts_with("09");
        let lost_zero = first_cta && digits.len() == 10 && digits.starts_with('9');
        if (!full_phone && !lost_zero && !contact_tail)
            || identifier_for_number(&line, at, &candidate)
        {
            continue;
        }
        let protected_in_first = number_candidates(&first).any(|(first_at, number)| {
            let prior = number.chars().filter_map(digit_value).collect::<String>();
            identifier_for_number(&first, first_at, &number)
                && (prior == contact_digits
                    || (prior.len() == 11 && prior.strip_prefix('0') == Some(contact_digits))
                    || (contact_digits.len() == 11
                        && contact_digits.strip_prefix('0') == Some(prior.as_str())))
        });
        if protected_in_first {
            continue;
        }
        if lost_zero {
            line.replace_range(at..at + candidate.len(), &format!("0{digits}"));
        }
        return Some(line);
    }
    None
}

pub(super) fn high_contrast_candidate_blocks(first: &str, candidate: &str) -> bool {
    let first = normalize(first);
    let protected = number_candidates(&first)
        .filter(|(at, number)| {
            identifier_for_number(&first, *at, number)
                && number.chars().filter(|c| c.is_ascii_digit()).count() >= 10
        })
        .map(|(_, number)| number.chars().filter_map(digit_value).collect::<String>())
        .collect::<Vec<_>>();
    if protected.is_empty() {
        return classify(candidate).decision == Decision::Block;
    }

    let normalized = normalize(candidate);
    let mut guarded = String::new();
    let mut cursor = 0;
    for (at, number) in number_candidates(&normalized) {
        let digits = number.chars().filter_map(digit_value).collect::<String>();
        if protected.contains(&digits) {
            guarded.push_str(&normalized[cursor..at]);
            guarded.push(' ');
            cursor = at + number.len();
        }
    }
    guarded.push_str(&normalized[cursor..]);
    classify(&guarded).decision == Decision::Block
}

fn price(normalized: &str, compact: &str) -> bool {
    let has_currency = CURRENCY
        .iter()
        .any(|term| contains_term(normalized, compact, term))
        || normalized.contains('$')
        || normalized.contains('€')
        || normalized.contains('£');
    let has_number = normalized.chars().any(|c| c.is_ascii_digit())
        || number_candidates(normalized).next().is_some()
        || normalized.split_whitespace().any(|token| {
            token
                .strip_suffix('k')
                .or_else(|| token.strip_suffix('m'))
                .or_else(|| token.strip_suffix("هزار"))
                .or_else(|| token.strip_suffix("میلیون"))
                .is_some_and(|prefix| prefix.chars().any(|c| c.is_ascii_digit()))
        });
    let has_unit = normalized.split_whitespace().any(|token| {
        token.ends_with('k')
            || token.ends_with('m')
            || token.ends_with("هزار")
            || token.ends_with("میلیون")
    });
    (has_currency || has_unit) && has_number
}

fn boundary_host(text: &str, host: &str) -> bool {
    text.match_indices(host).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + host.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '@')
            && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

fn squeeze_url(text: &str) -> String {
    let joins = |c: char| matches!(c, '.' | '/' | '@' | ':');
    let mut out = String::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        if c.is_whitespace() {
            let before = out.chars().next_back().is_some_and(joins);
            let after = text[at..].trim_start().starts_with(joins);
            if before || after {
                continue;
            }
        }
        out.push(c);
    }
    out
}

fn telegram_link(normalized: &str) -> bool {
    let squeezed = squeeze_url(normalized);
    TELEGRAM_HOSTS
        .iter()
        .any(|host| boundary_host(&squeezed, host))
}

fn web_url(normalized: &str) -> bool {
    let squeezed = squeeze_url(normalized);
    if squeezed.contains("http://") || squeezed.contains("https://") || squeezed.contains("www.") {
        return true;
    }
    squeezed.match_indices('.').any(|(at, _)| {
        let label = squeezed[..at]
            .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .next()
            .unwrap_or("");
        if label.len() < 3 || !label.chars().any(|c| c.is_ascii_alphabetic()) {
            return false;
        }
        TLDS.iter().any(|tld| {
            let rest = &squeezed[at + 1..];
            rest.starts_with(tld)
                && !rest[tld.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
        })
    })
}

fn username(normalized: &str) -> bool {
    normalized.char_indices().any(|(at, c)| {
        if c != '@'
            || normalized[..at]
                .chars()
                .next_back()
                .is_some_and(|prev| prev.is_ascii_alphanumeric() || prev == '_')
        {
            return false;
        }
        let mut run = String::new();
        let mut gaps = 0u8;
        for character in normalized[at + 1..].chars() {
            if character.is_ascii_alphanumeric() || character == '_' {
                run.push(character);
                if run.len() > 32 {
                    break;
                }
            } else if character.is_whitespace() && gaps < 2 {
                gaps += 1;
            } else {
                break;
            }
        }
        run.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) && (5..=32).contains(&run.len())
    })
}

fn standalone_username(normalized: &str) -> bool {
    let framed = normalized.trim_matches(|c: char| {
        c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '|' | ':' | '؛')
    });
    let Some(handle) = framed.strip_prefix('@') else {
        return false;
    };
    let handle = handle.trim_end_matches(['.', ',', '،', '!', '؟']);
    let compact = handle
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    compact
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && (5..=32).contains(&compact.len())
        && compact
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn telegram_ui(normalized: &str, compact: &str) -> (u8, bool) {
    const MARKERS: &[&str] = &[
        "subscrib",
        "pinnedmessage",
        "joinchannel",
        "viewinchannel",
        "sendmessage",
        "unmute",
    ];
    let markers = MARKERS
        .iter()
        .filter(|marker| compact.contains(**marker))
        .count() as u8;
    let title_letters = compact
        .strip_suffix("subscribers")
        .or_else(|| compact.strip_suffix("subscrib"))
        .unwrap_or(compact)
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .count();
    let channel_metric = compact.contains("subscrib")
        && normalized.chars().any(|c| c.is_ascii_digit())
        && title_letters >= 8;
    let group_metric = compact.contains("member")
        && compact.contains("online")
        && normalized.chars().any(|c| c.is_ascii_digit());
    let has_metric = channel_metric || group_metric;
    (markers, has_metric)
}

fn reasons(signals: &SignalSet, terms: &[&'static str]) -> Vec<ReasonCode> {
    let mut out = Vec::new();
    if signals.commercial_keywords > 0 {
        out.push(ReasonCode::CommercialKeyword);
    }
    if signals.call_to_action {
        out.push(ReasonCode::CallToAction);
    }
    if signals.phone_number {
        out.push(ReasonCode::PhoneNumber);
    }
    if signals.price {
        out.push(ReasonCode::Price);
    }
    if signals.telegram_link {
        out.push(ReasonCode::TelegramLink);
    }
    if signals.web_url {
        out.push(ReasonCode::WebUrl);
    }
    if signals.username {
        out.push(ReasonCode::Username);
    }
    if signals.telegram_ui_markers > 0 || signals.subscriber_or_group_metric {
        out.push(ReasonCode::TelegramUi);
    }
    if signals.editorial_context {
        out.push(ReasonCode::EditorialContext);
    }
    if out.is_empty() && !terms.is_empty() {
        out.push(ReasonCode::CommercialKeyword);
    }
    out
}

pub fn classify(text: &str) -> Classification {
    let normalized = normalize(text);
    let compact = compact_letters(&normalized);
    let matched_terms = collect_terms(&normalized, &compact);
    let strong_commercial_keywords = matched_terms
        .iter()
        .filter(|term| STRONG_COMMERCIAL.contains(term))
        .count()
        .min(u8::MAX as usize) as u8;
    let (telegram_ui_markers, subscriber_or_group_metric) = telegram_ui(&normalized, &compact);
    let mut signals = SignalSet {
        commercial_keywords: matched_terms.len().min(u8::MAX as usize) as u8,
        strong_commercial_keywords,
        call_to_action: has_cta(&normalized, &compact),
        phone_number: phone_number(&normalized),
        price: price(&normalized, &compact),
        telegram_link: telegram_link(&normalized),
        web_url: web_url(&normalized),
        username: username(&normalized),
        telegram_ui_markers,
        subscriber_or_group_metric,
        editorial_context: EDITORIAL
            .iter()
            .any(|term| contains_term(&normalized, &compact, term))
            || completed_support_notice(&compact),
    };
    if !signals.editorial_context && previous_sale_closed_notice(&normalized, &signals) {
        signals.editorial_context = true;
    }
    if !signals.editorial_context && damaged_closed_support_notice(&normalized, &signals) {
        signals.editorial_context = true;
    }

    let commercial = signals.commercial_keywords > 0;
    let many_commercial = signals.commercial_keywords >= 2;
    let strong_commercial = signals.strong_commercial_keywords > 0;
    let contact = signals.phone_number || signals.username || signals.telegram_link;
    let candidate = standalone_username(&normalized)
        || (signals.subscriber_or_group_metric && (signals.telegram_ui_markers >= 2 || contact))
        || (signals.telegram_link
            && (strong_commercial
                || (commercial && signals.call_to_action)
                || signals.price
                || signals.phone_number))
        || (signals.phone_number && strong_commercial)
        || (signals.phone_number && many_commercial && signals.call_to_action)
        || (signals.username && strong_commercial && signals.call_to_action)
        || (signals.price && strong_commercial && (signals.call_to_action || contact))
        || (signals.price && commercial && signals.call_to_action && signals.username)
        || (signals.web_url
            && strong_commercial
            && (signals.call_to_action || signals.phone_number || signals.username))
        || (strong_commercial && many_commercial && contact);
    let active_discount_offer = active_discount_after_closed_support(&normalized);
    let block = (candidate && !signals.editorial_context) || active_discount_offer;

    let family_count = signals.family_count();
    let suspicious = !block
        && (family_count >= 2
            || commercial
            || signals.telegram_ui_markers > 0
            || signals.subscriber_or_group_metric);
    let decision = if block {
        Decision::Block
    } else if suspicious {
        Decision::Suspicious
    } else {
        Decision::Allow
    };

    let mut reasons = reasons(&signals, &matched_terms);
    if active_discount_offer {
        reasons.push(ReasonCode::ActiveDiscountOffer);
    }
    Classification {
        decision,
        reasons,
        signals,
        matched_terms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(text: &str) -> Decision {
        classify(text).decision
    }

    #[test]
    fn normalizes_persian_variants_and_format_controls() {
        assert_eq!(normalize("فــروش\u{200b} ۱۲۳ ك ي"), "فروش 123 ک ی");
        assert!(classify("فــروش").signals.commercial_keywords > 0);
        assert!(classify("ف ر و ش").signals.commercial_keywords > 0);
    }

    #[test]
    fn single_weak_signals_never_block() {
        for text in [
            "فروش",
            "https://example.com",
            "09121234567",
            "100,000 تومان",
            "join channel",
            "دایرکت",
            "پیام بده",
            "فروش ویژه ثبت سفارش",
        ] {
            assert_ne!(decision(text), Decision::Block, "{text}");
        }
    }

    #[test]
    fn a_standalone_handle_blocks_without_condemning_quoted_usernames() {
        assert_eq!(decision("@channelname"), Decision::Block);
        assert_eq!(decision("(@ channel_name)"), Decision::Block);
        assert_ne!(decision("chat with @channelname"), Decision::Block);
        assert_ne!(decision("خبر از @channelname"), Decision::Block);
        assert_ne!(decision("name@example.com"), Decision::Block);
    }

    #[test]
    fn strong_persian_and_mixed_ads_block() {
        assert_eq!(
            decision("فروش ویژه قیمت ۱۰۰٬۰۰۰ تومان تماس ۰۹۱۲۱۲۳۴۵۶۷"),
            Decision::Block
        );
        assert_eq!(
            decision("ف ر و ش iPhone قیمت 150k DM @myshop"),
            Decision::Block
        );
        assert_eq!(decision("تخفیف قیمت @shop_channel"), Decision::Block);
        assert_eq!(
            decision(
                "iPhone \u{200f}ش\u{200e}\n\u{200e}DM @iphone_store\n\u{200f}قیمت ۰ تومان\u{200e}"
            ),
            Decision::Block
        );
    }

    #[test]
    fn english_ads_and_telegram_links_need_context() {
        assert_eq!(decision("SALE 50% ORDER NOW t.me/shop"), Decision::Block);
        assert_eq!(
            decision("DM for price https://shop.example.com"),
            Decision::Block
        );
        assert_ne!(decision("t.me/news"), Decision::Block);
    }

    #[test]
    fn phone_formats_are_conservative() {
        assert!(classify("تماس +98 912 123 4567 فروش").signals.phone_number);
        assert!(classify("تماس 0098-912-123-4567 فروش").signals.phone_number);
        assert!(
            !classify("سال 2024، شماره پرونده 1234567")
                .signals
                .phone_number
        );
    }

    #[test]
    fn a_product_code_is_not_a_contact_number() {
        let tag = "SALE\nNOW $19.99\nWAS $29.99\nProduct code 09121234567";
        assert!(!classify(tag).signals.phone_number);
        assert_ne!(decision(tag), Decision::Block);
        assert!(!classify("SALE SKU: 09121234567").signals.phone_number);
        assert!(!classify("فروش کد محصول ۰۹۱۲۱۲۳۴۵۶۷").signals.phone_number);

        let tag_with_contact = format!("{tag}\nContact 09121111111");
        assert!(classify(&tag_with_contact).signals.phone_number);
        assert_eq!(decision(&tag_with_contact), Decision::Block);
        assert!(
            classify("SALE Product code 09121234567 Contact 09121111111")
                .signals
                .phone_number
        );
    }

    #[test]
    fn a_model_number_is_not_a_contact_number() {
        let english_tag =
            "SALE NOW $99.99\nModel number 09181234567\nShelf label for blue headphones";
        assert!(!classify(english_tag).signals.phone_number);
        assert_ne!(decision(english_tag), Decision::Block);

        let persian_tag = "فروش ویژه ۲۴۰ هزار تومان\nشماره مدل ۰۹۱۸۱۲۳۴۵۶۷";
        assert!(!classify(persian_tag).signals.phone_number);
        assert_ne!(decision(persian_tag), Decision::Block);

        let persian_ad = "فروش ویژه\nشماره مدل ۰۹۱۸۱۲۳۴۵۶۷\nتماس 09191234567";
        assert!(classify(persian_ad).signals.phone_number);
        assert_eq!(decision(persian_ad), Decision::Block);

        let english_ad = "SALE TODAY\nModel number 09181234567\nCONTACT +1 415 555 0123";
        assert!(classify(english_ad).signals.phone_number);
        assert_eq!(decision(english_ad), Decision::Block);
    }

    #[test]
    fn raw_line_phone_recovery_keeps_identifier_and_editorial_guards() {
        let first = "فروش ویژه\nثبت سفارش تماس\n۹";
        let contact = recovered_line_contact(first, "۰۹۱۷۴۸۲۶۳۵۱", true).unwrap();
        assert_eq!(
            classify(&format!("{first}\n{contact}")).decision,
            Decision::Block
        );
        let lost_zero = recovered_line_contact(first, "۹۱۷۴۸۲۶۳۵۱", true).unwrap();
        assert!(lost_zero.contains("09174826351"));
        assert!(recovered_line_contact(first, "۹۱۷۴۸۲۶۳۵۱", false).is_none());

        let product = "فروش ویژه\nشماره مدل ۰۹۱۷۴۸۲۶۳۵۱";
        assert!(recovered_line_contact(product, "۰۹۱۷۴۸۲۶۳۵۱", true).is_none());
        assert!(recovered_line_contact(product, "۹۱۷۴۸۲۶۳۵۱", true).is_none());
        let product_without_zero = "فروش ویژه\nشماره مدل ۹۱۷۴۸۲۶۳۵۱";
        assert!(recovered_line_contact(product_without_zero, "۰۹۱۷۴۸۲۶۳۵۱", true).is_none());
        let serial = "فروش ویژه\nشماره سریال ۰۹۱۷۴۸۲۶۳۵۱\nثبت سفارش";
        assert!(recovered_line_contact(serial, "۹۱۷۴۸۲۶۳۵۱", true).is_none());
        assert!(recovered_line_contact(first, "شماره سریال ۰۹۱۷۴۸۲۶۳۵۱", true).is_none());
        let quote = "او نوشت فروش ویژه\nثبت سفارش تماس";
        assert!(classify(quote).signals.editorial_context);
        assert_ne!(
            classify(&format!("{quote}\n{contact}")).decision,
            Decision::Block
        );
    }

    #[test]
    fn a_serial_number_is_not_a_contact_number() {
        let tag = "فروشگاه لوازم\nقیمت ۲۴۰ هزار تومان\nشماره سریال ۰۹۱۴۷۲۶۳۸۵۹";
        assert!(!classify(tag).signals.phone_number);
        assert_ne!(decision(tag), Decision::Block);
        let lost_label = "فروشگاه لوازم\nقیمت ۲۴۰ هزار تومان\nشماره ۰۹۱۴۷۲۶۳۸۵۹";
        assert!(!high_contrast_candidate_blocks(tag, lost_label));
        let separate_contact = format!("{lost_label}\nتماس ۰۹۱۹۷۴۲۶۳۸۵");
        assert!(high_contrast_candidate_blocks(tag, &separate_contact));
    }

    #[test]
    fn a_sample_number_is_not_a_contact_number() {
        let example = "yujgol تشخیص اکهی پوشاک\nمنال فروش ویژه\nشماره نمونه ۰۹۱۵۷۳۸۲۴۶۹";
        assert!(!classify(example).signals.phone_number);
        assert_ne!(decision(example), Decision::Block);

        let advert = format!("{example}\nتماس ۰۹۱۹۱۲۳۴۵۶۷");
        assert!(classify(&advert).signals.phone_number);
        assert_eq!(decision(&advert), Decision::Block);

        let lost_label = "فروش ویژه\nشماره ۰۹۱۵۷۳۸۲۴۶۹";
        assert!(!high_contrast_candidate_blocks(example, lost_label));
        assert!(recovered_line_contact(example, "۰۹۱۵۷۳۸۲۴۶۹", true).is_none());
    }

    #[test]
    fn shipment_identifiers_are_not_contact_numbers() {
        for tag in [
            "SALE CANCELED\nPARCEL 09173582046",
            "SALE CANCELED\nTRACKING NUMBER 09173582046",
            "فروش لغو شد\nشماره پیگیری ۰۹۱۷۳۵۸۲۰۴۶",
        ] {
            assert!(!classify(tag).signals.phone_number, "{tag}");
            assert_ne!(classify(tag).decision, Decision::Block, "{tag}");
        }
        for advert in [
            "SALE TODAY\nPARCEL 09173582046\nCALL 09129876543",
            "SPECIAL OFFER\nTRACKING NUMBER 09173582046\nORDER 09129876543",
            "فروش ویژه\nشماره پیگیری ۰۹۱۷۳۵۸۲۰۴۶\nتماس ۰۹۱۲۹۸۷۶۵۴۳",
        ] {
            assert!(classify(advert).signals.phone_number, "{advert}");
            assert_eq!(classify(advert).decision, Decision::Block, "{advert}");
        }
    }

    #[test]
    fn completed_reversed_tracking_with_one_ocr_substitution_is_not_a_contact() {
        for receipt in [
            "فروش پابان یافت\n۰۹۱۷۳۵۸۲۰۴۶ شماره پیگیری\nتحویل اتجام شد",
            "فروش پایان یافت\n۰۹۱۷۳۵۸۲۰۴۶ شماره پیگیری\nتحویل انجام شد",
            "فروش لفو شد\n۰۹۱۷۳۵۸۲۰۴۶ شماره پیگیری\nمرسوله تحویل شد",
            "خرید گذشته\n۰۹۱۷۳۵۸۲۰۴۶ شماره پیگیری\nارسال کامل شد",
        ] {
            assert!(!classify(receipt).signals.phone_number, "{receipt}");
            assert_ne!(classify(receipt).decision, Decision::Block, "{receipt}");
        }
        let first = "فروش پابان یافت\n۰۹۱۷۳۵۸۲۰۴۶ شماره پیگیری\nتحویل اتجام شد";
        assert!(recovered_line_contact(first, "۰۹۱۷۳۵۸۲۰۴۶", true).is_none());
        assert!(!high_contrast_candidate_blocks(
            first,
            "فروش پابان یافت\n۰۹۱۷۳۵۸۲۰۴۶"
        ));
        assert!(high_contrast_candidate_blocks(
            first,
            "فروش پابان یافت\n۰۹۱۷۳۵۸۲۰۴۶\nتماس ۰۹۱۲۹۸۷۶۵۴۳"
        ));
    }

    #[test]
    fn completed_sale_context_keeps_active_new_sale_and_separate_contacts() {
        for advert in [
            "فروش پابان یافت؛ فروش جدید\n۰۹۱۷۳۵۸۲۰۴۶ شماره پیگیری",
            "فروش لفو شد؛ فروش ویژه\n۰۹۱۷۳۵۸۲۰۴۶ شماره پیگیری",
            "فروش پایان یافت\n۰۹۱۷۳۵۸۲۰۴۶ شماره پیگیری\nتماس ۰۹۱۲۹۸۷۶۵۴۳",
            "فروش پایان یافت\nتماس ۰۹۱۷۳۵۸۲۰۴۶ شماره پیگیری",
        ] {
            assert!(classify(advert).signals.phone_number, "{advert}");
            assert_eq!(classify(advert).decision, Decision::Block, "{advert}");
        }
    }

    #[test]
    fn damaged_tracking_label_and_short_noise_keep_completed_receipts_safe() {
        for receipt in [
            "فروش پایان یافت\nVa 6 A\n17/171 ۳\nHT ۰۹۱۶۰۰۰۰۲۷۸ شماره پیگیری\nتحویل کامل شد",
            "خرید قبلی\n۰۹۱۶۰۰۰۲۹۱۹ شماره پیکیری\nمرسوله دریافت شد",
            "فروش پایان یافت\n۰۹۱۶۰۰۰۳۰۵۸ ی شماره پیگیری\nتحویل کامل شد",
        ] {
            assert!(!classify(receipt).signals.phone_number, "{receipt}");
            assert_ne!(classify(receipt).decision, Decision::Block, "{receipt}");
        }
        let first = "فروش پایان یافت\n۰۹۱۶۰۰۰۳۰۵۸ ی شماره پیگیری\nتحویل کامل شد";
        assert!(recovered_line_contact(first, "۰۹۱۶۰۰۰۳۰۵۸", true).is_none());
        assert!(!high_contrast_candidate_blocks(
            first,
            "فروش پایان یافت\n۰۹۱۶۰۰۰۳۰۵۸"
        ));
    }

    #[test]
    fn damaged_tracking_label_keeps_new_sales_and_independent_contacts() {
        for advert in [
            "فروش پایان یافت؛ فروش جدید\n۰۹۱۶۰۰۰۲۹۱۹ شماره پیکیری",
            "فروش لغو شد؛ تخفیف امروز\n۷ ۱۷/۱۷ ۳\nVa ۶ A\n۰۹۱۶۰۰۰۳۰۵۸ ی شماره پیگیری",
            "فروش پایان یافت\n۰۹۱۶۰۰۰۲۹۱۹ شماره پیکیری\nتماس ۰۹۱۲۹۸۷۶۵۴۳",
            "فروش پایان یافت\nتماس ۰۹۱۶۰۰۰۳۰۵۸ ی شماره پیگیری",
        ] {
            assert!(classify(advert).signals.phone_number, "{advert}");
            assert_eq!(classify(advert).decision, Decision::Block, "{advert}");
        }
    }

    #[test]
    fn isolated_digit_after_labeled_contact_does_not_hide_an_ad() {
        for advert in [
            "فروش ویژه امروز\nسفارش ۰۹۱۲۷۵۸۴۳۶۹ ۳",
            "فروش ویژه امروز\nتماس ۰۹۱۲۷۵۸۴۳۶۹ 3",
            "SALE NOW\nContact 09127584369 3",
        ] {
            assert!(classify(advert).signals.phone_number, "{advert}");
            assert_eq!(classify(advert).decision, Decision::Block, "{advert}");
        }
        assert!(recovered_line_contact("فروش ویژه", "سفارش ۰۹۱۲۷۵۸۴۳۶۹ ۳", true).is_some());
    }

    #[test]
    fn trailing_digits_on_identifiers_and_receipts_do_not_become_contacts() {
        for safe in [
            "فروش ویژه\nشماره پیگیری ۰۹۱۲۷۵۸۴۳۶۹ ۳",
            "فروش ویژه\nشماره مدل ۰۹۱۲۷۵۸۴۳۶۹ ۳",
            "فروش ویژه\nشماره سفارش ۰۹۱۲۷۵۸۴۳۶۹ ۳",
            "فروش ویژه\nکد سفارش ۰۹۱۲۷۵۸۴۳۶۹ ۳",
            "SALE NOW\nOrder ID 09127584369 3",
            "رسید خرید گذشته\nتماس ۰۹۱۲۷۵۸۴۳۶۹ ۳",
        ] {
            assert_ne!(classify(safe).decision, Decision::Block, "{safe}");
        }
        assert!(!classify("فروش ویژه\n۰۹۱۲۷۵۸۴۳۶۹ ۳").signals.phone_number);
    }

    #[test]
    fn completed_receipts_protect_a_misspelled_tracking_label() {
        for receipt in [
            "فروش لغو شد\nشماره پیکیری ۰۹۱۸۰۰۰۰۲۷۴\nبسته دریافت شد",
            "فروش تمام شد\n۱ شماره پیکیری ۰۹۱۸۰۰۰۱۰۹۶\nمرسوله تحویل شد",
            "خرید گذشته\n۰۹۱۸۰۰۰۱۹۱۸ نز شماره پیکیری\nارسال انجام شد",
        ] {
            assert!(!classify(receipt).signals.phone_number, "{receipt}");
            assert_ne!(classify(receipt).decision, Decision::Block, "{receipt}");
        }
    }

    #[test]
    fn active_sales_with_a_tracking_typo_keep_separate_contacts() {
        for advert in [
            "فروش پایان یافت؛ فروش جدید\nشماره پیکیری ۰۹۱۸۰۰۰۰۲۷۴\nتماس ۰۹۱۲۳۰۰۰۰۰۰",
            "فروش لغو شد؛ تخفیف امروز\n۰۹۱۸۰۰۰۱۹۱۸ نز شماره پیکیری\nسفارش ۰۹۱۲۳۰۰۰۰۰۰",
            "فروش ویژه\nشماره پیکیری ۰۹۱۸۰۰۰۰۲۷۴",
        ] {
            assert!(classify(advert).signals.phone_number, "{advert}");
            assert_eq!(classify(advert).decision, Decision::Block, "{advert}");
        }
    }

    #[test]
    fn high_contrast_retry_preserves_a_first_pass_model_code() {
        let first = "فروش ویژه ۲۴۰ هزار تومان\nشماره مدل ۰۹۱۸۱۲۳۴۵۶۷";
        let reversed = "خرید ۲۴۰ هزار تومان\n۰۹۱۸۱۲۳۴۵۶۷ شماره مدل";
        let lost_label = "فروش ویژه ۲۴۰ هزار تومان\nشماره ۰۹۱۸۱۲۳۴۵۶۷";
        assert!(!high_contrast_candidate_blocks(first, reversed));
        assert!(!high_contrast_candidate_blocks(first, lost_label));

        let separate_contact = format!("{lost_label}\nتماس 09191234567");
        assert!(high_contrast_candidate_blocks(first, &separate_contact));
        let independent_url = format!("{lost_label}\nORDER NOW https://shop.example.com");
        assert!(high_contrast_candidate_blocks(first, &independent_url));

        let english_first = "SALE NOW $99.99\nModel number 09181234567";
        let english_retry = "SALE NOW $99.99\nNumber 09181234567";
        assert!(!high_contrast_candidate_blocks(
            english_first,
            english_retry
        ));
    }

    #[test]
    fn prices_require_currency_or_price_unit() {
        assert!(classify("قیمت ۱۰۰٬۰۰۰ تومان فروش").signals.price);
        assert!(classify("SALE 150k").signals.price);
        assert!(classify("قیمت ۱.۵ میلیون").signals.price);
        assert!(classify("price 1.5 million").signals.price);
        assert!(!classify("رسید شماره 100,000").signals.price);
    }

    #[test]
    fn ordinary_screenshots_do_not_block() {
        for text in [
            "خبر امروز: فروش سهام شرکت در بورس",
            "قیمت نان امروز 100,000 در رسید خرید",
            "برای خرید آیفون، قیمت 100,000 تومان منطقی است؟",
            "اطلاعات بیشتر در کانال t.me/news",
            "خبر فروش سهام؛ تماس 09121234567",
            "گزارش قیمت 100 تومان در example.com",
            "Contact us for a workshop at university.example.edu",
            "chat with @friendname about the news",
            "DM @friendname about the price of a phone",
            "خبر: قیمت گوشی ۰ تومان، DM @shop_channel",
            "members online now",
            "3.1416 and 12.1K subscribers",
            "name@example.com",
        ] {
            assert_ne!(decision(text), Decision::Block, "{text}");
        }
    }

    #[test]
    fn telegram_ui_signature_requires_context() {
        assert_eq!(
            decision("12:28 TeleBotCraft 12.1K subscribers PinnedMessage"),
            Decision::Block
        );
        assert_eq!(
            decision("6,642 members 539 online Unmute Leave"),
            Decision::Suspicious
        );
        assert_ne!(decision("join channel"), Decision::Block);
        assert_ne!(decision("online now"), Decision::Block);
    }

    #[test]
    fn reasons_are_stable_and_explainable() {
        let result = classify("فروش قیمت 100,000 تومان تماس 09121234567");
        assert_eq!(result.decision, Decision::Block);
        assert!(result.reasons.contains(&ReasonCode::CommercialKeyword));
        assert!(result.reasons.contains(&ReasonCode::PhoneNumber));
        assert!(result.reasons.contains(&ReasonCode::Price));
        assert!(result.matched_terms.contains(&"فروش"));
        assert_eq!(ReasonCode::PhoneNumber.as_str(), "phone_number");
    }

    #[test]
    fn spaced_ocr_username_is_a_signal_but_not_a_verdict() {
        let result = classify("نام کاربری @ user name");
        assert!(result.signals.username);
        assert_ne!(result.decision, Decision::Block);
    }

    #[test]
    fn labeled_corpus_is_a_non_regressing_release_gate() {
        let mut old_true_positives = 0usize;
        let mut new_true_positives = 0usize;
        let mut old_false_positives = 0usize;
        let mut new_false_positives = 0usize;
        let mut ads = 0usize;
        let mut legitimate = 0usize;

        for (line_number, line) in include_str!("../../../tests/fixtures/photo_ad_corpus.tsv")
            .lines()
            .enumerate()
            .skip(1)
        {
            let fields = line.split('\t').collect::<Vec<_>>();
            assert_eq!(fields.len(), 8, "malformed corpus line {}", line_number + 1);
            let truth = fields[2];
            let old_outcome = fields[3];
            let text = fields[6];
            let expected = match fields[7] {
                "ALLOW" => Decision::Allow,
                "SUSPICIOUS" => Decision::Suspicious,
                "BLOCK" => Decision::Block,
                other => panic!("unknown expected decision {other}"),
            };
            let actual = decision(text);
            assert_eq!(actual, expected, "corpus case {}: {text}", fields[0]);

            match truth {
                "AD" => {
                    ads += 1;
                    old_true_positives += usize::from(old_outcome == "TRUE_POSITIVE");
                    new_true_positives += usize::from(actual == Decision::Block);
                }
                "LEGIT" => {
                    legitimate += 1;
                    old_false_positives += usize::from(old_outcome == "FALSE_POSITIVE");
                    new_false_positives += usize::from(actual == Decision::Block);
                }
                "AMBIGUOUS" => {}
                other => panic!("unknown corpus truth {other}"),
            }
        }

        assert!(new_true_positives >= old_true_positives);
        assert!(new_false_positives <= old_false_positives);
        assert_eq!(new_false_positives, 0, "legitimate fixtures must fail open");
        eprintln!(
            "photo-ad text corpus: ads={ads} legitimate={legitimate} old_tp={old_true_positives} new_tp={new_true_positives} old_fp={old_false_positives} new_fp={new_false_positives}"
        );
    }

    #[test]
    fn damaged_completion_requires_specific_status_and_no_active_order() {
        for receipt in [
            "فروش oll یافت 31\nشماره پیکیری ۰۹۱۸۰۰۰۱۶۴۴\nتحویل کامل شد",
            "یافت oll فروش\nشماره پیکیری ۰۹۱۸۰۰۰۱۶۴۴\nبسته دریافت شد",
        ] {
            assert!(!classify(receipt).signals.phone_number, "{receipt}");
            assert_ne!(decision(receipt), Decision::Block, "{receipt}");
        }

        for active in [
            "فروش افزایش یافت؛ سفارش دهید\nشماره پیکیری ۰۹۱۲۶۰۰۰۰۰۰\nشماره سفارش ۱۲۳۴۵۶\nبسته دریافت شد",
            "فروش oll یافت؛ سفارش دهید\nشماره پیکیری ۰۹۱۲۶۰۰۰۰۰۰\nبسته دریافت شد",
        ] {
            assert!(classify(active).signals.phone_number, "{active}");
            assert_eq!(decision(active), Decision::Block, "{active}");
        }
    }

    #[test]
    fn order_prompt_with_contact_and_closed_support_notice() {
        for active in [
            "پیشنهاد ویژه امروز؛ سفارش دهید\nشماره پیگیری ۰۹۱۸۷۲۰۰۰۰۰\nتماس ۰۹۱۲۶۵۰۰۰۰۰",
            "قیمت مناسب امروز؛ سفارش دهید\n۰۹۱۸۷۲۰۰۰۰۰ شماره پیگیری\n۰۹۱۲۶۵۰۰۰۰۰ تماس",
            "ارسال فوری؛ سفارش دهید\nتماس ۰۹۱۲۶۵۰۰۰۰۰",
            "فروش لغو شد؛ سفارش بسته شد؛ فروش جدید\nتماس پشتیبانی ۰۹۱۲۶۵۰۰۰۰۰",
        ] {
            assert_eq!(decision(active), Decision::Block, "{active}");
        }
        for safe in [
            "فروش لغو شد؛ سفارش بسته شد\nشماره پیگیری ۰۹۱۸۷۲۰۰۰۰۰\nتماس پشتیبانی ۰۹۱۲۶۵۰۰۰۰۰",
            "تماس بگیرید ۰۹۱۲۶۵۰۰۰۰۰",
        ] {
            assert_ne!(decision(safe), Decision::Block, "{safe}");
        }
    }

    #[test]
    fn damaged_closed_support_notices_need_tracking_and_no_new_offer() {
        for safe in [
            "فروش )93 شد؛ سفارش بسته شد\nشماره پیگیری ۰۹۸۷۱۲۳۴۵۶۷\nتماس پشتیبانی ۰۹۳۲۶۱۸۹۷۵۴",
            "فروش لغو شد؛ سفارش بسته بشید\nشماره پیگیری ۰۹۸۷۱۲۳۴۵۶۷\nتماس پشتیبانی ۰۹۳۲۶۱۸۹۷۵۴",
        ] {
            assert_ne!(decision(safe), Decision::Block, "{safe}");
        }
        let active = "فروش )93 شد؛ سفارش بسته شد\nفروش جدید؛ سفارش دهید\nشماره پیگیری ۰۹۸۷۱۲۳۴۵۶۷\nتماس پشتیبانی ۰۹۳۲۶۱۸۹۷۵۴";
        assert_eq!(decision(active), Decision::Block);
    }

    #[test]
    fn previous_sale_support_notice_needs_its_own_closed_status_and_no_new_offer() {
        for safe in [
            "فروش قبلی لغو شد؛ سفارش بسته شد\nشماره پیگیری ۰۹۸۷۱۳۳۵۳۳۵\nدرخواست پشتیبانی ثبت شد\nتماس پشتیبانی ۰۹۳۲۶۴۵۹۳۲۶",
            "فروش قبلی )93 شد؛ سفارش بسته شد\nشماره پیگیری ۰۹۸۷۱۵۳۳۱۶۱\nدرخواست پشتیبانی ثبت شد\nتماس پشتیبانی ۰۹۳۲۶۸۶۳۶۸۴",
        ] {
            assert_ne!(decision(safe), Decision::Block, "{safe}");
        }
        for active in [
            "فروش قبلی لغو شد؛ سفارش بسته شد\nفروش جدید؛ سفارش دهید\nشماره پیگیری ۰۹۸۷۱۳۳۵۳۳۵\nتماس پشتیبانی ۰۹۳۲۶۴۵۹۳۲۶",
            "فروش قبلی لغو شد؛ سفارش بسته شد\nشماره پیگیری ۰۹۸۷۱۳۳۵۳۳۵\nپیشنهاد ویژه امروز\nتماس پشتیبانی ۰۹۳۲۶۴۵۹۳۲۶",
            "فروش قبلی لغو شد؛ سفارش بسته شد؛ فروش جدید\nشماره پیگیری ۰۹۸۷۱۳۳۵۳۳۵\nتماس پشتیبانی ۰۹۳۲۶۴۵۹۳۲۶",
        ] {
            assert_eq!(decision(active), Decision::Block, "{active}");
        }
    }

    #[test]
    fn closed_sale_followed_by_a_separate_discount_offer_survives_lost_numbers() {
        for active in [
            "فروش لغو شد؛ سفارش بسته شد\nتخفیف ویژه امروز؛ خرید MS\n۰ شماره پیگیری\n۱ تماس پشتیبانی",
            "فروش لفو شد؛ سفارش بسته شد\nتخفیف Lg pol o jug خرید کنید\n۶ شماره پیگیری\n۹ تماس پشتیبانی",
            "فروش لفو شد؛ سفارش بسته شد\nتخفیف ویژه امروز؛ خرید کنید\n۴ شماره پیگیری\n۳ تماس پشتیبانی",
        ] {
            let result = classify(active);
            assert_eq!(result.decision, Decision::Block, "{active}");
            assert!(result.reasons.contains(&ReasonCode::ActiveDiscountOffer));
        }
        for safe in [
            "فروش لغو شد؛ سفارش بسته شد\nشماره پیگیری ۰۹۸۷۱۲۳۴۵۶۷\nتماس پشتیبانی ۰۹۳۲۶۱۸۹۷۵۴",
            "فروش لغو شد؛ سفارش بسته شد\nتخفیف ویژه امروز لغو شد؛ خرید نکنید\nشماره پیگیری ۰۹۸۷۱۲۳۴۵۶۷\nتماس پشتیبانی ۰۹۳۲۶۱۸۹۷۵۴",
            "فروش لغو شد؛ سفارش بسته شد\nخبر: «تخفیف ویژه امروز؛ خرید کنید» تکذیب شد\nشماره پیگیری ۰۹۸۷۱۲۳۴۵۶۷\nتماس پشتیبانی ۰۹۳۲۶۱۸۹۷۵۴",
            "فروش لغو شد؛ سفارش بسته شد\nشماره پیگیری ۰۹۸۷۱۲۳۴۵۶۷\nتماس پشتیبانی ۰۹۳۲۶۱۸۹۷۵۴\nگزارش تخفیف ویژه امروز؛ خرید کنید",
            "رسید خرید گذشته؛ تخفیف ویژه امروز؛ خرید کنید\nشماره پیگیری ۰۹۸۷۱۲۳۴۵۶۷\nتماس پشتیبانی ۰۹۳۲۶۱۸۹۷۵۴",
            "فروش لغو شد؛ سفارش بسته شد\nتخفیف ویژه امروز؛ خرید کنید\nشماره پیگیری ۰۹۸۷۱۲۳۴۵۶۷",
        ] {
            assert_ne!(classify(safe).decision, Decision::Block, "{safe}");
        }
    }
}
