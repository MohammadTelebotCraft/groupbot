use std::borrow::Cow;

pub(crate) fn separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{200b}' | '\u{200c}')
}

pub(crate) fn ignored(c: char) -> bool {
    matches!(c, '\u{640}' | '\u{64b}'..='\u{65f}' | '\u{670}'
        | '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}'
        | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

fn fold(c: char) -> char {
    match c {
        '۰'..='۹' => char::from(b'0' + (c as u32 - '۰' as u32) as u8),
        '٠'..='٩' => char::from(b'0' + (c as u32 - '٠' as u32) as u8),
        'ي' | 'ى' => 'ی',
        'ك' => 'ک',
        other => other.to_ascii_lowercase(),
    }
}

fn characters(text: &str) -> impl Iterator<Item = (char, usize)> + '_ {
    let mut started = false;
    let mut spaced = false;
    text.char_indices().filter_map(move |(at, c)| {
        if ignored(c) {
            return None;
        }
        if separator(c) {
            if !started || spaced {
                return None;
            }
            spaced = true;
            return Some((' ', at + c.len_utf8()));
        }
        started = true;
        spaced = false;
        Some((fold(c), at + c.len_utf8()))
    })
}

pub fn normalize(text: &str) -> Cow<'_, str> {
    let text = text.trim_matches(separator);
    if characters(text).map(|(c, _)| c).eq(text.chars()) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        characters(text)
            .map(|(c, _)| c)
            .collect::<String>()
            .trim_end()
            .to_owned(),
    )
}

pub fn matches(text: &str, command: &str) -> bool {
    strip_prefix(text, command).is_some_and(|tail| tail.chars().all(|c| separator(c) || ignored(c)))
}

pub fn contains(commands: &[&str], text: &str) -> bool {
    commands.iter().any(|command| matches(text, command))
}

pub(crate) fn custom_shortcut_first(text: &str) -> bool {
    let head = text
        .trim_start_matches(separator)
        .split(separator)
        .next()
        .unwrap_or("");
    head.starts_with('/') && !contains(&["/start", "/ephemeral"], head)
}

pub(crate) fn reserved_shortcut(text: &str) -> bool {
    if !text.starts_with('/') {
        return false;
    }
    static ALIASES: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
        super::help::alias_groups()
            .into_iter()
            .flat_map(|(_, aliases)| aliases)
            .chain(super::sudo::COMMANDS.iter().copied())
            .chain(super::tune::COMMANDS.iter().copied())
            .chain(["/start", "/lock", "/unlock", "/cleanerlogin"])
            .filter(|alias| alias.starts_with('/'))
            .map(str::to_owned)
            .collect()
    });
    ALIASES
        .iter()
        .any(|alias| strip_prefix(text, alias).is_some())
}

pub(crate) fn reserved_trigger(text: &str) -> bool {
    if reserved_shortcut(text) || super::locks::is_management_command(text) {
        return true;
    }
    let normalized = normalize(text);
    if super::sudo::COMMANDS
        .iter()
        .any(|alias| strip_prefix(text, alias).is_some())
    {
        return true;
    }
    super::help::alias_groups()
        .into_iter()
        .flat_map(|(_, aliases)| aliases)
        .any(|alias| strip_prefix(text, alias).is_some())
        || super::help::TOPICS
            .iter()
            .flat_map(|topic| topic.commands.iter())
            .any(|command| normalize(command.form) == normalized)
}

pub fn strip_prefix<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let mut input = characters(text);
    let mut end = 0;
    let mut any = false;
    for (expected, _) in characters(command.trim_matches(separator)) {
        let (actual, source_end) = input.next()?;
        if expected != actual {
            return None;
        }
        end = source_end;
        any = true;
    }
    if !any {
        return None;
    }
    let rest = text[end..].trim_start_matches(ignored);
    if rest.is_empty() {
        return Some(rest);
    }
    rest.starts_with(separator)
        .then(|| rest.trim_start_matches(separator).trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_keyboard_and_spacing_rules() {
        for (typed, canonical) in [
            ("  قفل\tعكس  ", "قفل عکس"),
            ("حذف\u{200c}اخطار", "حذف اخطار"),
            ("لِيست   ادمين\u{200c}ها", "لیست ادمین ها"),
            ("/HeLP", "/help"),
            ("پرونده ۱۲٣", "پرونده 123"),
            ("\u{200f}قـفل عکس\u{200e}", "قفل عکس"),
        ] {
            assert!(matches(typed, canonical), "{typed}");
            assert_eq!(normalize(typed), canonical, "{typed}");
        }
        assert!(matches!(normalize("قفل عکس"), Cow::Borrowed(_)));
        assert!(matches("قفل عکس \u{200e}", "قفل عکس"));
    }

    #[test]
    fn raw_payload_is_not_folded_or_reconstructed() {
        let payload = "سَلام\u{200c}دنیا  ۱۲٣\nhttps://Example.com/AbC?X=Y";
        for command in ["تنظیم پاسخ", "تنظيم\tپاسخ", "تنظیم\u{200c}پاسخ"]
        {
            let text = format!("{command}  {payload}");
            assert_eq!(strip_prefix(&text, "تنظیم پاسخ"), Some(payload));
        }
        assert_eq!(strip_prefix("/FILTER   VPN", "/filter"), Some("VPN"));
        assert_eq!(strip_prefix("حذف ۱۲۳ payload", "حذف 123"), Some("payload"));
    }

    #[test]
    fn custom_shortcuts_keep_legacy_actions_without_allowing_new_builtin_collisions() {
        for text in ["/ban @Ali", "/purge 123", "/myshortcut"] {
            assert!(custom_shortcut_first(text), "{text}");
        }
        for text in ["/start", "/START payload", "/ephemeral status", "بن @Ali"] {
            assert!(!custom_shortcut_first(text), "{text}");
        }
        for text in ["/ban", "/PURGE", "/warn", "/lock photo", "/sudo"] {
            assert!(reserved_shortcut(text), "{text}");
        }
        assert!(!reserved_shortcut("/bananas"));
        assert!(!reserved_shortcut("/myshortcut"));
        for command in [
            "پنل",
            "قوانین",
            "پینگ",
            "داشبورد",
            "قفل عکس",
            "قفل گروه 2 ساعت",
            "تنظیم اخطار 5",
            "تنظیم اخطار 7",
            "تنظیم پاسخ درود",
            "افزودن فیلتر کفش",
            "پرونده 731",
            "بستن پرونده 731",
            "ضد رگبار 11 7",
            "حذف 42",
            "بن @Ali",
            "پاسخ بده",
        ] {
            assert!(reserved_trigger(command), "{command}");
        }
        assert!(!reserved_trigger("زنجیر"));
        assert!(!reserved_trigger("پنلدار"));
    }

    #[test]
    fn matching_does_not_guess_or_drop_unknown_tails() {
        for text in ["/help@otherbot", "/helpful", "/help?", "/help me"] {
            assert!(!matches(text, "/help"), "{text}");
        }
        assert_eq!(strip_prefix("بن میکنم", "بن"), Some("میکنم"));
        assert!(!matches("بن میکنم", "بن"));
        assert_eq!(strip_prefix("بنویس", "بن"), None);
        assert_eq!(strip_prefix("/ban@otherbot @ali", "/ban"), None);
        assert_eq!(strip_prefix("سلام", ""), None);
        assert!(matches("قفل\u{200c}ها", "قفل ها"));
        assert!(!matches("قفلها", "قفل ها"));
    }
}
