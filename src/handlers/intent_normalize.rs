
use unicode_normalization::UnicodeNormalization;

const WORDS: &[(&str, &str)] = &[
    ("فروش", "فروش"),
    ("فروشی", "فروشی"),
    ("فروشیه", "فروشیه"),
    ("میفروشم", "میفروشم"),
    ("میفروشیم", "میفروشیم"),
    ("نمیفروشم", "نمیفروشم"),
    ("نمیفروشیم", "نمیفروشیم"),
    ("میفروشه", "میفروشه"),
    ("میفروشی", "میفروشی"),
    ("خرید", "خرید"),
    ("خریدار", "خریدار"),
    ("خریدارم", "خریدارم"),
    ("فروشنده", "فروشنده"),
    ("میخرم", "میخرم"),
    ("نمیخرم", "نمیخرم"),
    ("تعویض", "تعویض"),
    ("معاوضه", "معاوضه"),
    ("ترید", "ترید"),
    ("تبادل", "تبادل"),
    ("واگذار", "واگذار"),
    ("واگذاری", "واگذاری"),
    ("گوشی", "گوشی"),
    ("ماشین", "ماشین"),
    ("تبلت", "تبلت"),
    ("کتاب", "کتاب"),
    ("اکانت", "اکانت"),
    ("آیتم", "آیتم"),
    ("اسکین", "اسکین"),
    ("قیمت", "قیمت"),
    ("توافقی", "توافقی"),
    ("تومن", "تومن"),
    ("تومان", "تومان"),
    ("پیوی", "پیوی"),
    ("دایرکت", "دایرکت"),
    ("میکنم", "میکنم"),
    ("نمیکنم", "نمیکنم"),
    ("میشه", "میشه"),
    ("نمیشه", "نمیشه"),
    ("میخوام", "میخوام"),
    ("نمیخوام", "نمیخوام"),
    ("میو", "میو"),
    ("فورش", "فروش"),
    ("فورشی", "فروشی"),
    ("میفورشم", "میفروشم"),
    ("میفرشم", "میفروشم"),
    ("خربدار", "خریدار"),
    ("خربدارم", "خریدارم"),
    ("خریدرام", "خریدارم"),
    ("تعاویض", "تعویض"),
    ("sell", "sell"),
    ("selling", "selling"),
    ("sale", "sale"),
    ("buy", "buy"),
    ("buying", "buying"),
    ("buyer", "buyer"),
    ("trade", "trade"),
    ("trading", "trading"),
    ("swap", "swap"),
    ("exchange", "exchange"),
    ("account", "account"),
    ("acc", "acc"),
    ("price", "price"),
    ("offer", "offer"),
    ("dm", "dm"),
    ("pv", "pv"),
    ("forosh", "فروش"),
    ("foroosh", "فروش"),
    ("foroshi", "فروشی"),
    ("forooshi", "فروشی"),
    ("miforosham", "میفروشم"),
    ("mifrosham", "میفروشم"),
    ("miforoosham", "میفروشم"),
    ("nemiforosham", "نمیفروشم"),
    ("nemiforoosham", "نمیفروشم"),
    ("kharidar", "خریدار"),
    ("kharidaram", "خریدارم"),
    ("gheymat", "قیمت"),
    ("toman", "تومان"),
    ("tavafoqi", "توافقی"),
    ("tavafoghi", "توافقی"),
    ("moaveze", "معاوضه"),
];

fn invisible(c: char) -> bool {
    matches!(c, '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{640}' |
        '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' |
        '\u{2060}'..='\u{206f}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}')
        || matches!(c, '\u{64b}'..='\u{65f}' | '\u{670}' | '\u{6d6}'..='\u{6ed}')
}

fn soft(c: char) -> bool {
    c == ' '
        || ".·-_*/\\|~".contains(c)
        || matches!(c, '\u{2600}'..='\u{27bf}' | '\u{1f000}'..='\u{1faff}')
}

fn lookalike(c: char) -> char {
    match c {
        'а' | 'α' => 'a',
        'е' | 'ε' => 'e',
        'о' | 'ο' => 'o',
        'р' | 'ρ' => 'p',
        'с' => 'c',
        'ѕ' => 's',
        'і' | 'ι' => 'i',
        'ӏ' => 'l',
        'у' => 'y',
        'х' | 'χ' => 'x',
        'т' => 't',
        other => other,
    }
}

fn runs(s: &str) -> Vec<(char, usize)> {
    let mut out = Vec::new();
    for c in s.chars() {
        if let Some((last, count)) = out.last_mut()
            && *last == c
        {
            *count += 1;
        } else {
            out.push((c, 1));
        }
    }
    out
}

fn lookup(s: &str) -> Option<&'static str> {
    if let Some((_, canonical)) = WORDS.iter().find(|(word, _)| *word == s) {
        return Some(canonical);
    }
    let candidate = runs(s);
    type Pattern = (Vec<(char, usize)>, &'static str);
    static PATTERNS: std::sync::OnceLock<Vec<Pattern>> = std::sync::OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            WORDS
                .iter()
                .map(|(word, canonical)| (runs(word), *canonical))
                .collect()
        })
        .iter()
        .find_map(|(expected, canonical)| {
            (candidate.len() == expected.len()
                && candidate
                    .iter()
                    .zip(expected)
                    .all(|((a, n), (b, m))| a == b && n >= m))
            .then_some(*canonical)
        })
}

pub fn descramble(text: &str) -> (String, bool) {
    let mut tampered = false;
    let folded: Vec<char> = text
        .nfkc()
        .flat_map(char::to_lowercase)
        .filter_map(|c| {
            if invisible(c) {
                tampered = true;
                return matches!(c, '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}')
                    .then_some(' ');
            }
            Some(match c {
                'ي' | 'ى' => 'ی',
                'ك' => 'ک',
                '\u{660}'..='\u{669}' => char::from_u32(c as u32 - 0x660 + 0x6f0).unwrap(),
                space if space.is_whitespace() => ' ',
                other => other,
            })
        })
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < folded.len() {
        let c = folded[at];
        if at == 0 || folded[at - 1].is_whitespace() {
            let end = folded[at..]
                .iter()
                .position(|c| c.is_whitespace())
                .map_or(folded.len(), |end| at + end);
            let token: String = folded[at..end].iter().collect();
            if token.contains("://")
                || token.starts_with("t.me/")
                || token.starts_with("www.")
                || token.contains('@')
            {
                out.push_str(&token);
                at = end;
                continue;
            }
        }
        if soft(c) && !c.is_whitespace() && at > 0 && folded[at - 1].is_alphabetic() {
            let mut end = at + 1;
            while end < folded.len() && soft(folded[end]) && !folded[end].is_whitespace() {
                end += 1;
            }
            if end < folded.len() && folded[end].is_alphabetic() {
                out.push(' ');
                tampered = true;
                at = end;
                continue;
            }
        }
        if c.is_alphabetic() && (at == 0 || !folded[at - 1].is_alphanumeric()) {
            let mut candidate = String::new();
            let mut best = None;
            for end in at..folded.len().min(at + 96) {
                let next = folded[end];
                if next.is_alphabetic() {
                    candidate.push(lookalike(next));
                } else if !soft(next) {
                    break;
                } else {
                    continue;
                }
                if candidate.chars().count() > 32 {
                    break;
                }
                if (end + 1 == folded.len() || !folded[end + 1].is_alphanumeric())
                    && let Some(word) = lookup(&candidate)
                {
                    best = Some((end + 1, word));
                }
            }
            if let Some((end, word)) = best {
                tampered |= folded[at..end].iter().collect::<String>() != word;
                out.push_str(word);
                at = end;
                continue;
            }
        }
        let mut end = at + 1;
        while end < folded.len() && folded[end] == c {
            end += 1;
        }
        let copies = if c.is_alphabetic() && ('\u{600}'..='\u{6ff}').contains(&c) && end - at >= 3 {
            1
        } else {
            end - at
        };
        out.extend(std::iter::repeat_n(c, copies));
        at = end;
    }
    let words: Vec<_> = out
        .split_whitespace()
        .map(|word| {
            let parts: Vec<_> = word
                .split(|c: char| ".·-_*/\\|~".contains(c))
                .filter(|part| !part.is_empty())
                .collect();
            if parts.len() > 1 && parts.iter().all(|part| lookup(part).is_some()) {
                tampered = true;
                parts.join(" ")
            } else {
                word.to_owned()
            }
        })
        .collect();
    (words.join(" "), tampered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalent_spellings_share_one_interpretation() {
        for variant in [
            "ف.ر.و.ش",
            "ف-ر-و-ش",
            "ف_ر_و_ش",
            "ف ر و ش",
            "فـروش",
            "ف💰ر💰و💰ش",
            "ف...روش",
            "ف/ر/و/ش",
            "ف|ر|و|ش",
            "ف\u{200d}رو\u{2060}ش",
        ] {
            assert_eq!(descramble(variant).0, "فروش", "{variant}");
        }
        for variant in [
            "s e l l",
            "s.e.l.l",
            "s-e-l-l",
            "s💰e💰l💰l",
            "ѕеll",
            "ｓｅｌｌ",
            "selllll",
        ] {
            assert_eq!(descramble(variant).0, "sell", "{variant}");
        }
        for variant in [
            "می.فروشم",
            "می_فروشم",
            "می ف ر و ش م",
            "م.ی.ف.ر.و.ش.م",
            "می\u{200c}فروشم",
            "میفورشم",
            "miforoosham",
        ] {
            assert_eq!(descramble(variant).0, "میفروشم", "{variant}");
        }
        assert_eq!(
            descramble("ا.ک.ا.ن.ت ف.ر.و.ش.ی قیمت توافقی").0,
            "اکانت فروشی قیمت توافقی"
        );
        assert_eq!(descramble("تعا...ویض").0, "تعویض");
        assert_eq!(descramble("trade.acc").0, "trade acc");
        assert_eq!(descramble("s\te\tl\tl").0, "sell");
        assert_eq!(descramble("اکانت\u{200b}فروشی").0, "اکانت فروشی");
        assert_eq!(descramble("اكـانت فروشي ٨٠٠").0, "اکانت فروشی ۸۰۰");
        assert_eq!(descramble("اکانت💰فروشی").0, "اکانت فروشی");
        assert_eq!(
            descramble("قیمت.بده.اگه.خوب.باشه.میفروشم").0,
            "قیمت بده اگه خوب باشه میفروشم"
        );
    }

    #[test]
    fn preserves_boundaries_negation_quotes_and_amounts() {
        for text in [
            "t.me/shop",
            "۲.۵ میلیون",
            "۵۰۰۰",
            "100000",
            "فروش یعنی چی؟",
            "خرید و فروش توی این گروه ممنوعه",
            "«اکانت فروشی»",
            "don't sell",
            "a b c",
            "سلام،خوبی؟",
            "sell. no thanks",
            "دیروز اکانتم رو فروختم",
        ] {
            assert_eq!(descramble(text).0, text, "{text}");
        }
        assert_eq!(descramble("ن می ف ر و ش م").0, "نمیفروشم");
        assert_eq!(descramble("نمی\u{200c}فروشم").0, "نمیفروشم");
        assert_eq!(descramble("nemiforoosham").0, "نمیفروشم");
    }
}
