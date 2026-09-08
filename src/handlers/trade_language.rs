
use super::context;

fn amount(text: &str) -> bool {
    let digits = super::super::digits(text);
    !digits.is_empty()
        && digits.chars().any(|c| c.is_ascii_digit())
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || ".٫,".contains(c))
}

pub fn proposition(text: &str) -> Option<String> {
    let text = text.trim_end_matches(['.', '!', '؟', '?']);
    let words: Vec<_> = text.split_whitespace().collect();
    if let ["برای", "این", asset, price, currency, "میخوام"] = words.as_slice()
        && context::object(asset).is_some()
        && amount(price)
        && ["تومن", "تومان", "دلار"].contains(currency)
    {
        return Some(format!("{asset} فروشی، قیمت {price} {currency}"));
    }
    if let ["پیشنهاد", "بالای", price, "قبول"] = words.as_slice()
        && amount(price)
    {
        return Some(format!("فروشی، پیشنهاد قیمت بالای {price} قبول میکنم"));
    }
    if let [who, "خریدار", state, "پیام", "بده"] = words.as_slice()
        && ["هرکی", "هرکس", "هرکسی"].contains(who)
        && ["بود", "هست"].contains(state)
    {
        return Some("فروشی، خریدار پیام بده".to_owned());
    }
    if let ["کسی", "اینو", "میخواد؟", "قیمت", "توافقی"] = words.as_slice() {
        return Some("این فروشی، قیمت توافقی".to_owned());
    }
    if let ["قیمت", "بده", conditional, "خوب", "باشه", "میفروشم"] = words.as_slice()
        && ["اگه", "اگر"].contains(conditional)
    {
        return Some("میفروشم با قیمت توافقی، پیشنهاد قیمت بده".to_owned());
    }
    if let Some(body) = text
        .strip_prefix("این ")
        .and_then(|s| s.strip_suffix(" تعویض میشه"))
        && let Some((left, right)) = body.split_once(" با ")
        && context::object(left).is_some()
        && context::object(right).is_some()
        && left.split_whitespace().count() <= 4
        && right.split_whitespace().count() <= 4
    {
        return Some(format!("این {left} رو با {right} معاوضه میکنم"));
    }
    None
}

pub fn corroborated(text: &str) -> bool {
    if context::object(text).is_none() {
        return false;
    }
    let price = text.chars().any(|c| c.is_numeric())
        && ["$", "تومن", "تومان", "دلار", "usdt", "قیمت"]
            .iter()
            .any(|w| text.contains(w));
    let owns = [
        "my account",
        "my phone",
        "my skin",
        "اکانتم",
        "این اکانت",
        "این گوشی",
    ]
    .iter()
    .any(|w| text.contains(w));
    let active = [
        "میفروشم",
        "خریدارم",
        "selling",
        "sell my",
        "trading my",
        "معاوضه میکنم",
    ]
    .iter()
    .any(|w| text.contains(w));
    let exchange = text.contains("trading my") && text.contains(" for ");
    active && owns && (price || exchange)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_requires_the_entire_utterance() {
        for text in [
            "پیشنهاد بالای ۵۰۰ قبول",
            "هرکی خریدار بود پیام بده",
            "برای این اکانت ۲ تومن میخوام",
            "قیمت بده اگه خوب باشه میفروشم",
        ] {
            assert!(proposition(text).is_some(), "{text}");
            assert!(proposition(&format!("گفت {text}")).is_none());
            assert!(proposition(&format!("{text} ولی شوخی کردم")).is_none());
            assert!(proposition(&format!("«{text}»")).is_none());
        }
        assert!(proposition("برای این اکانت ۲ تومن نمیخوام").is_none());
        assert!(!corroborated("فقط نازتو خریدارم"));
        assert!(!corroborated("قیمت اکانت ۵۰۰ تومن"));
    }
}
