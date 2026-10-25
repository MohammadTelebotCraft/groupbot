use std::collections::HashMap;
use std::sync::OnceLock;

const TABLE: &str = include_str!("../../assets/imgfilter_subjects.tsv");
const LEXICON: &str = include_str!("../../assets/imgfilter_lexicon.tsv");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Visual(&'static str),
    Explicit,
    Concept(&'static str),
    Person(&'static str),
    Game(&'static str),
    App(&'static str),
}

struct Row {
    route: Route,
    names: Vec<&'static str>,
}

fn rows(text: &'static str) -> impl Iterator<Item = Row> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut columns = line.split('\t');
            let (kind, argument, names) = (columns.next()?, columns.next()?, columns.next()?);
            let route = match kind {
                "visual" => Route::Visual(argument),
                "explicit" => Route::Explicit,
                "concept" => Route::Concept(argument),
                "person" => Route::Person(argument),
                "game" => Route::Game(argument),
                "app" => Route::App(argument),
                _ => return None,
            };
            Some(Row {
                route,
                names: names.split('|').filter(|name| !name.is_empty()).collect(),
            })
        })
}

pub fn key(text: &str) -> String {
    super::commands::normalize(text)
        .chars()
        .map(|c| match c {
            'آ' | 'أ' | 'إ' | 'ٱ' => 'ا',
            'ة' | 'ۀ' => 'ه',
            'ؤ' => 'و',
            'ئ' => 'ی',
            other => other,
        })
        .collect()
}

struct Tables {
    exact: HashMap<String, Route>,
    spaceless: HashMap<String, Route>,
}

fn tables() -> &'static Tables {
    static CELL: OnceLock<Tables> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut exact = HashMap::new();
        let mut spaceless = HashMap::new();
        for row in rows(TABLE).chain(rows(LEXICON)) {
            for name in row.names {
                let folded = key(name);
                spaceless.entry(folded.replace(' ', "")).or_insert(row.route);
                exact.entry(folded).or_insert(row.route);
            }
        }
        Tables { exact, spaceless }
    })
}

#[cfg(test)]
pub fn visual_englishes() -> std::collections::HashSet<&'static str> {
    rows(TABLE)
        .chain(rows(LEXICON))
        .filter_map(|row| match row.route {
            Route::Visual(english) => Some(english),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
pub fn person_englishes() -> std::collections::HashSet<&'static str> {
    rows(TABLE)
        .filter_map(|row| match row.route {
            Route::Person(english) => Some(english),
            _ => None,
        })
        .collect()
}

const LEADING: &[&str] = &["عکس های ", "عکسهای ", "تصاویر ", "تصویر ", "عکس "];
const TRAILING: &[&str] = &[" ها", "ها"];

pub fn resolve(phrase: &str) -> Option<Route> {
    let tables = tables();
    let lookup = |text: &str| {
        tables
            .exact
            .get(text)
            .or_else(|| tables.spaceless.get(&text.replace(' ', "")))
            .copied()
    };
    let key = key(phrase);
    if let Some(route) = lookup(&key) {
        return Some(route);
    }
    let mut stem: &str = &key;
    for lead in LEADING {
        if let Some(rest) = stem.strip_prefix(lead) {
            stem = rest;
            break;
        }
    }
    for trail in TRAILING {
        if let Some(rest) = stem.strip_suffix(trail)
            && !rest.is_empty()
            && let Some(route) = lookup(rest)
        {
            return Some(route);
        }
    }
    if stem.is_empty() {
        return None;
    }
    lookup(stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn well_formed(text: &'static str) -> Vec<Row> {
        let lines = text
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
            .count();
        let parsed: Vec<Row> = rows(text).collect();
        assert_eq!(parsed.len(), lines, "a row did not parse");
        let mut seen: HashMap<String, Route> = HashMap::new();
        for row in &parsed {
            assert!(!row.names.is_empty());
            match row.route {
                Route::Concept(key) => assert!(
                    super::super::concepts::CONCEPTS.iter().any(|c| c.key == key),
                    "no concept lock «{key}»"
                ),
                Route::Visual(english) | Route::Person(english) => {
                    assert!(!english.is_empty());
                    assert_eq!(
                        english,
                        english.to_lowercase(),
                        "«{english}» must be lowercase: the tower reads a capital as noise"
                    );
                }
                Route::Explicit | Route::Game(_) | Route::App(_) => {}
            }
            for name in &row.names {
                if let Some(earlier) = seen.insert(key(name), row.route)
                    && earlier != row.route
                {
                    panic!("«{name}» is both {earlier:?} and {:?}", row.route);
                }
            }
        }
        assert!(!text.contains('\u{200c}'), "no half-space anywhere");
        parsed
    }

    #[test]
    fn the_table_is_well_formed() {
        well_formed(TABLE);
    }

    #[test]
    fn the_lexicon_is_well_formed() {
        let table: std::collections::HashSet<String> =
            rows(TABLE).flat_map(|row| row.names).map(key).collect();
        let lexicon = well_formed(LEXICON);
        assert!(lexicon.len() > 1000, "the lexicon is missing or truncated");
        for row in &lexicon {
            assert!(matches!(row.route, Route::Visual(_)));
            for name in &row.names {
                assert!(!table.contains(&key(name)), "«{name}» is in both tables");
            }
        }
    }

    #[test]
    fn the_live_filters_that_failed_resolve_to_what_they_meant() {
        assert_eq!(resolve("خامنه ای"), Some(Route::Person("ali khamenei")));
        assert_eq!(resolve("پابجی"), Some(Route::Game("pubg")));
        assert_eq!(resolve("کالاف"), Some(Route::Game("call of duty")));
        assert_eq!(resolve("کص"), Some(Route::Explicit));
        assert_eq!(resolve("سیگار"), Some(Route::Concept("c_cig")));
        assert_eq!(resolve("سگ"), Some(Route::Visual("a dog")));
    }

    #[test]
    fn spelling_variants_fold() {
        assert_eq!(resolve("خامنه\u{200c}ای"), Some(Route::Person("ali khamenei")));
        assert_eq!(resolve("  PUBG "), Some(Route::Game("pubg")));
        assert_eq!(resolve("سيگار"), Some(Route::Concept("c_cig")));
        assert_eq!(resolve("عکس سگ"), Some(Route::Visual("a dog")));
        assert_eq!(resolve("گوسفندها"), Some(Route::Visual("a sheep")));
        assert_eq!(resolve("تصویر گربه ها"), Some(Route::Visual("a cat")));
        assert_eq!(resolve("ابجو"), resolve("آبجو"));
        assert_eq!(resolve("رييسي"), Some(Route::Person("ebrahim raisi")));
        assert_eq!(resolve("سسگوجه"), resolve("سس گوجه"));
        assert_eq!(resolve("چیزی که نیست"), None);
        assert_eq!(resolve(""), None);
        assert_eq!(resolve("عکس "), None);
    }

    #[test]
    fn ordinary_words_resolve_through_the_lexicon() {
        for word in ["چرتکه", "شکم", "اقاقیا"] {
            assert!(
                matches!(resolve(word), Some(Route::Visual(_))),
                "«{word}» did not resolve"
            );
        }
    }
}
