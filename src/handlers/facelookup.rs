use std::sync::{Arc, LazyLock};
use std::time::Duration;

use serde_json::Value;

use super::Ctx;

const WIKIDATA: &str = "https://www.wikidata.org/w/api.php";
const COMMONS: &str = "https://commons.wikimedia.org/w/api.php";
const ASKED: usize = 5;
const PHOTOGRAPHS: usize = 30;
const WIDTH: &str = "960";
const OTHER_WIDTH: &str = "500";
const PHOTOGRAPH_BYTES: usize = 8 << 20;
const AT_ONCE: usize = 2;
const TRIES: usize = 3;
const WHOLE: Duration = Duration::from_secs(25);
pub const SAME: f32 = 0.50;
const CENTRE_ROUNDS: usize = 4;

static HTTP: LazyLock<Option<reqwest::Client>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(format!("groupbot/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .build()
        .ok()
});

#[derive(Debug, PartialEq)]
pub enum Miss {
    Nobody,
    Faceless,
    Unreachable,
}

pub struct Found {
    pub label: String,
    pub prints: Vec<Vec<f32>>,
}

pub struct Photographs {
    pub label: String,
    pub portrait: Vec<u8>,
    pub others: Vec<Vec<u8>>,
}

fn wait_of(response: &reqwest::Response) -> Duration {
    let asked = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok()?.parse().ok())
        .unwrap_or(2);
    Duration::from_secs(asked.clamp(1, 4))
}

async fn ask(url: &str, query: &[(&str, &str)]) -> Result<Value, Miss> {
    let client = HTTP.as_ref().ok_or(Miss::Unreachable)?;
    let unreachable = |why: String| {
        log::warn!("face lookup: {url} {why}");
        Miss::Unreachable
    };
    let mut tries = 0;
    let response = loop {
        let response = client
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(|error| unreachable(error.to_string()))?;
        tries += 1;
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS || tries == TRIES {
            break response;
        }
        tokio::time::sleep(wait_of(&response)).await;
    };
    if !response.status().is_success() {
        return Err(unreachable(format!("answered {}", response.status())));
    }
    let body = response.bytes().await.map_err(|error| unreachable(error.to_string()))?;
    serde_json::from_slice(&body).map_err(|error| unreachable(error.to_string()))
}

fn ranked(
    found: &Value,
    name: &str,
    sure: bool,
    pages: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let wanted = super::imgsubject::key(&name.to_lowercase());
    let mut rows: Vec<(bool, String, String)> = found["search"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let matched = row["match"]["text"].as_str()?;
            Some((
                super::imgsubject::key(&matched.to_lowercase()) == wanted,
                row["id"].as_str()?.to_owned(),
                row["label"].as_str().unwrap_or(matched).to_owned(),
            ))
        })
        .collect();
    if !sure {
        rows.truncate(1);
        rows.retain(|row| row.0);
        return rows.into_iter().map(|(_, id, label)| (id, label)).collect();
    }
    rows.sort_by_key(|row| !row.0);
    rows.truncate(ASKED);
    let exact = rows.iter().filter(|row| row.0).count();
    let mut rows = rows.into_iter().map(|(_, id, label)| (id, label));
    let mut asked: Vec<(String, String)> = rows.by_ref().take(exact).collect();
    for row in pages.into_iter().chain(rows) {
        if !asked.iter().any(|known| known.0 == row.0) {
            asked.push(row);
        }
    }
    asked
}

fn values<'a>(entity: &'a Value, property: &str) -> impl Iterator<Item = &'a Value> {
    entity["claims"][property]
        .as_array()
        .into_iter()
        .flatten()
        .map(|claim| &claim["mainsnak"]["datavalue"]["value"])
}

type Person = (String, String, String, Option<String>);

fn pick(asked: &[(String, String)], answer: &Value) -> Option<Person> {
    asked.iter().find_map(|(id, label)| {
        let entity = &answer["entities"][id];
        if !values(entity, "P31").any(|kind| kind["id"] == "Q5") {
            return None;
        }
        let file = values(entity, "P18").find_map(Value::as_str)?;
        let category = values(entity, "P373").find_map(Value::as_str);
        Some((id.clone(), label.clone(), file.to_owned(), category.map(str::to_owned)))
    })
}

async fn person_among(asked: &[(String, String)]) -> Result<Option<Person>, Miss> {
    if asked.is_empty() {
        return Ok(None);
    }
    let ids: Vec<&str> = asked.iter().map(|(id, _)| id.as_str()).collect();
    let answer = ask(
        WIKIDATA,
        &[
            ("action", "wbgetentities"),
            ("ids", &ids.join("|")),
            ("props", "claims"),
            ("format", "json"),
        ],
    )
    .await?;
    Ok(pick(asked, &answer))
}

fn loosely(name: &str) -> String {
    name.split_whitespace()
        .map(|word| if word.chars().count() > 3 { format!("{word}~") } else { word.to_owned() })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn thumbnails(which: &[(&str, &str)], width: &str) -> Result<Vec<String>, Miss> {
    let mut query = vec![
        ("action", "query"),
        ("prop", "imageinfo"),
        ("iiprop", "url|mime"),
        ("iiurlwidth", width),
        ("format", "json"),
    ];
    query.extend_from_slice(which);
    let answer = ask(COMMONS, &query).await?;
    let mut pages: Vec<(i64, String)> = answer["query"]["pages"]
        .as_object()
        .into_iter()
        .flat_map(|pages| pages.values())
        .filter_map(|page| {
            let info = &page["imageinfo"][0];
            matches!(info["mime"].as_str()?, "image/jpeg" | "image/png" | "image/webp")
                .then_some(())?;
            let address = info["thumburl"].as_str()?;
            let host = reqwest::Url::parse(address).ok()?.host_str()?.to_owned();
            host.ends_with(".wikimedia.org")
                .then(|| (page["index"].as_i64().unwrap_or(i64::MAX), address.to_owned()))
        })
        .collect();
    pages.sort();
    Ok(pages.into_iter().map(|(_, address)| address).collect())
}

async fn found_photographs(search: &str) -> Result<Vec<String>, Miss> {
    let limit = PHOTOGRAPHS.to_string();
    thumbnails(
        &[
            ("generator", "search"),
            ("gsrsearch", search),
            ("gsrnamespace", "6"),
            ("gsrlimit", &limit),
        ],
        OTHER_WIDTH,
    )
    .await
}

async fn encyclopedia(name: &str, language: &str) -> Result<Vec<(String, String)>, Miss> {
    let answer = ask(
        &format!("https://{language}.wikipedia.org/w/api.php"),
        &[
            ("action", "query"),
            ("generator", "search"),
            ("gsrsearch", name),
            ("gsrlimit", "3"),
            ("gsrenablerewrites", "1"),
            ("prop", "pageprops"),
            ("ppprop", "wikibase_item"),
            ("format", "json"),
        ],
    )
    .await?;
    let mut pages: Vec<(i64, String, String)> = answer["query"]["pages"]
        .as_object()
        .into_iter()
        .flat_map(|pages| pages.values())
        .filter_map(|page| {
            Some((
                page["index"].as_i64()?,
                page["pageprops"]["wikibase_item"].as_str()?.to_owned(),
                page["title"].as_str()?.to_owned(),
            ))
        })
        .collect();
    pages.sort();
    Ok(pages.into_iter().map(|(_, id, title)| (id, title)).collect())
}

async fn download(address: String) -> Option<Vec<u8>> {
    let client = HTTP.as_ref()?;
    let mut response = client.get(&address).send().await;
    if let Ok(refused) = &response
        && refused.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        tokio::time::sleep(wait_of(refused)).await;
        response = client.get(&address).send().await;
    }
    let mut response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            log::warn!("face lookup: {address} answered {}", response.status());
            return None;
        }
        Err(error) => {
            log::warn!("face lookup: {address} {error}");
            return None;
        }
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > PHOTOGRAPH_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}

pub async fn photographs(
    name: &str,
    english: Option<&str>,
    sure: bool,
) -> Result<Photographs, Miss> {
    let deadline = tokio::time::Instant::now() + WHOLE;
    let (search, language) = match english {
        Some(english) => (english, "en"),
        None if name.chars().any(|c| c.is_ascii_alphabetic()) => (name, "en"),
        None => (name, "fa"),
    };
    let found = ask(
        WIKIDATA,
        &[
            ("action", "wbsearchentities"),
            ("search", search),
            ("language", language),
            ("uselang", language),
            ("type", "item"),
            ("limit", "10"),
            ("format", "json"),
        ],
    )
    .await;
    let pages = if sure { encyclopedia(search, language).await } else { Err(Miss::Nobody) };
    if found.is_err() && pages.is_err() {
        return Err(Miss::Unreachable);
    }
    let asked = ranked(&found.unwrap_or_default(), search, sure, pages.unwrap_or_default());
    let mut person = person_among(&asked).await?;
    if person.is_none() && sure {
        let near = encyclopedia(&loosely(search), language).await.unwrap_or_default();
        person = person_among(&near).await?;
    }
    let (id, label, file, category) = person.ok_or(Miss::Nobody)?;
    let portrait = thumbnails(&[("titles", &format!("File:{file}"))], WIDTH)
        .await?
        .pop()
        .ok_or(Miss::Faceless)?;
    let portrait = download(portrait).await.ok_or(Miss::Unreachable)?;

    let mut addresses =
        found_photographs(&format!("haswbstatement:P180={id} filetype:bitmap")).await?;
    if addresses.len() < PHOTOGRAPHS / 2
        && let Some(category) = category
    {
        let more = found_photographs(&format!("deepcat:\"{category}\" filetype:bitmap"))
            .await
            .unwrap_or_default();
        for address in more {
            if addresses.len() < PHOTOGRAPHS && !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    Ok(Photographs {
        label,
        portrait,
        others: downloads(addresses, deadline).await,
    })
}

async fn downloads(addresses: Vec<String>, deadline: tokio::time::Instant) -> Vec<Vec<u8>> {
    let gate = Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
    let mut tasks = tokio::task::JoinSet::new();
    for (at, address) in addresses.into_iter().enumerate() {
        let gate = Arc::clone(&gate);
        tasks.spawn(async move {
            let _turn = gate.acquire_owned().await.ok()?;
            Some((at, download(address).await?))
        });
    }
    let mut done = Vec::new();
    while let Ok(Some(one)) = tokio::time::timeout_at(deadline, tasks.join_next()).await {
        done.extend(one.ok().flatten());
    }
    done.sort_by_key(|(at, _)| *at);
    done.into_iter().map(|(_, bytes)| bytes).collect()
}

pub async fn pictured(english: &str) -> Vec<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + WHOLE;
    let bare = ["a ", "an ", "the "]
        .iter()
        .find_map(|article| english.strip_prefix(article))
        .unwrap_or(english);
    let search = format!("{bare} filetype:bitmap");
    match tokio::time::timeout_at(deadline, found_photographs(&search)).await {
        Ok(Ok(addresses)) => downloads(addresses, deadline).await,
        _ => Vec::new(),
    }
}

pub fn choose(anchor: Vec<f32>, others: &[Vec<Vec<f32>>]) -> Vec<Vec<f32>> {
    let dot = super::vision::dot;
    let mut centre = anchor.clone();
    let mut kept: Vec<&Vec<f32>> = Vec::new();
    for _ in 0..CENTRE_ROUNDS {
        let now: Vec<&Vec<f32>> = others
            .iter()
            .filter_map(|faces| {
                faces.iter().max_by(|a, b| dot(a, &centre).total_cmp(&dot(b, &centre)))
            })
            .filter(|face| dot(face, &centre) >= SAME)
            .collect();
        if now.len() == kept.len() {
            break;
        }
        kept = now;
        let mut sum = anchor.clone();
        for face in &kept {
            sum.iter_mut().zip(face.iter()).for_each(|(sum, x)| *sum += x);
        }
        centre = super::vision::unit(&sum);
    }
    let mut chosen = vec![anchor];
    while chosen.len() < super::facelock::MAX_REFERENCES {
        let nearest = |face: &Vec<f32>| chosen.iter().map(|c| dot(c, face)).fold(-1.0, f32::max);
        let Some((at, likeness)) = kept
            .iter()
            .map(|face| nearest(face))
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            break;
        };
        if likeness >= super::facelock::DUPLICATE {
            break;
        }
        chosen.push(kept.swap_remove(at).clone());
    }
    chosen
}

pub fn prints_of(bytes: &[u8]) -> Option<Vec<Vec<f32>>> {
    let image = super::nsfw::decode_still(bytes).ok()?;
    super::facelock::prints(&image).ok()?
}

pub async fn person(
    ctx: &Arc<Ctx>,
    name: &str,
    english: Option<&str>,
    sure: bool,
) -> Result<Found, Miss> {
    let Photographs { label, portrait, others } = photographs(name, english, sure).await?;
    let mut faces = Vec::with_capacity(1 + others.len());
    for bytes in std::iter::once(portrait).chain(others) {
        let slot = ctx.nsfw_slot().await;
        let prints = tokio::task::spawn_blocking(move || {
            let _slot = slot;
            prints_of(&bytes)
        })
        .await
        .ok()
        .flatten();
        faces.push(prints.unwrap_or_default());
    }
    let mut faces = faces.into_iter();
    let anchor = faces
        .next()
        .and_then(|portrait| portrait.into_iter().next())
        .ok_or(Miss::Faceless)?;
    let prints = choose(anchor, &faces.collect::<Vec<_>>());
    Ok(Found { label, prints })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found() -> Value {
        serde_json::json!({"search": [
            {"id": "Q1", "label": "مسی", "match": {"text": "مسی"}},
            {"id": "Q2", "label": "لیونل مسی", "match": {"text": "لیونل مسی"}},
            {"id": "Q3", "label": "Messi", "match": {"text": "مسی"}},
        ]})
    }

    #[test]
    fn an_unsure_name_is_only_its_first_exact_result() {
        assert_eq!(ranked(&found(), "مسی", false, vec![]), [("Q1".to_owned(), "مسی".to_owned())]);
        assert!(ranked(&found(), "لیونل مسی", false, vec![]).is_empty());
    }

    #[test]
    fn a_sure_name_tries_exact_results_first() {
        let pages = vec![("Q9".to_owned(), "page".to_owned()), ("Q1".to_owned(), "again".to_owned())];
        let ids: Vec<String> =
            ranked(&found(), "لیونل مسی", true, pages).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["Q2", "Q9", "Q1", "Q3"]);
    }

    #[test]
    fn the_first_human_with_a_portrait_is_the_person() {
        let claim = |value: Value| serde_json::json!([{"mainsnak": {"datavalue": {"value": value}}}]);
        let human = || claim(serde_json::json!({"id": "Q5"}));
        let answer = serde_json::json!({"entities": {
            "Q1": {"claims": {"P31": claim(serde_json::json!({"id": "Q515"})), "P18": claim("town.jpg".into())}},
            "Q2": {"claims": {"P31": human()}},
            "Q3": {"claims": {"P31": human(), "P18": claim("them.jpg".into()), "P373": claim("Them".into())}},
        }});
        let asked: Vec<(String, String)> =
            ["Q1", "Q2", "Q3"].iter().map(|id| ((*id).to_owned(), (*id).to_owned())).collect();
        let (id, _, file, category) = pick(&asked, &answer).unwrap();
        assert_eq!((id.as_str(), file.as_str(), category.as_deref()), ("Q3", "them.jpg", Some("Them")));
        assert_eq!(loosely("امانول مکرون از"), "امانول~ مکرون~ از");
    }

    fn face(x: f32, y: f32) -> Vec<f32> {
        let mut face = vec![0.0; super::super::facelock::WIDTH];
        (face[0], face[1], face[2]) = (x, y, (1.0 - x * x - y * y).max(0.0).sqrt());
        face
    }

    #[test]
    fn only_the_portraits_face_is_kept_and_not_twice() {
        let anchor = face(1.0, 0.0);
        let others = [
            vec![face(0.0, 1.0), face(0.6, 0.8)],
            vec![face(0.6, 0.8)],
            vec![face(-0.2, 0.9)],
            vec![face(0.7, -0.5)],
        ];
        let chosen = choose(anchor.clone(), &others);
        assert_eq!(chosen, [anchor, face(0.6, 0.8), face(0.7, -0.5)]);
    }

    #[tokio::test]
    #[ignore]
    async fn named_people_through_the_lookup() {
        use super::super::facelock::{CUT, photographs_likeness};
        simple_logger::SimpleLogger::new().with_level(log::LevelFilter::Warn).init().ok();
        let pause = std::env::var("FACELOOKUP_PAUSE").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let names = std::fs::read_to_string(std::env::var("FACELOOKUP_NAMES").unwrap()).unwrap();
        let mut locks = Vec::new();
        for name in names.lines().map(str::trim).filter(|name| !name.is_empty()) {
            tokio::time::sleep(Duration::from_secs(pause)).await;
            let photographs = match photographs(name, None, true).await {
                Ok(photographs) => photographs,
                Err(miss) => {
                    println!("{name}: {miss:?}");
                    continue;
                }
            };
            let Some(anchor) = prints_of(&photographs.portrait).and_then(|p| p.into_iter().next())
            else {
                println!("{name}: portrait without a face");
                continue;
            };
            let faces: Vec<Vec<Vec<f32>>> =
                photographs.others.iter().filter_map(|bytes| prints_of(bytes)).collect();
            let fit: Vec<Vec<Vec<f32>>> = faces.iter().step_by(2).cloned().collect();
            let references = choose(anchor.clone(), &fit);
            let alone: Vec<&Vec<Vec<f32>>> =
                faces.iter().skip(1).step_by(2).filter(|faces| faces.len() == 1).collect();
            let hit = |references: &[Vec<f32>]| {
                alone.iter().filter(|faces| photographs_likeness(faces, references) >= CUT).count()
            };
            println!(
                "{name} = {}: {} photographs, {} references; held alone {} taken {} (portrait only {})",
                photographs.label,
                faces.len(),
                references.len(),
                alone.len(),
                hit(&references),
                hit(std::slice::from_ref(&anchor)),
            );
            locks.push((name.to_owned(), references, faces));
        }
        let mut strays = Vec::new();
        if let Ok(directory) = std::env::var("FACELOOKUP_STRAYS") {
            let mut stack = vec![std::path::PathBuf::from(directory)];
            while let Some(path) = stack.pop() {
                if path.is_dir() {
                    stack.extend(std::fs::read_dir(path).unwrap().map(|entry| entry.unwrap().path()));
                } else if let Some(faces) = std::fs::read(&path).ok().and_then(|b| prints_of(&b)) {
                    strays.extend(faces.into_iter().map(|face| (path.clone(), face)));
                }
            }
        }
        let (mut cross, mut cross_taken, mut stray_taken) = (0, 0, 0);
        for (name, references, _) in &locks {
            for (other, _, faces) in &locks {
                if other == name {
                    continue;
                }
                for face in faces.iter().flatten() {
                    cross += 1;
                    if photographs_likeness(std::slice::from_ref(face), references) >= CUT {
                        cross_taken += 1;
                        println!("stray: {name} took a face from {other}'s photographs");
                    }
                }
            }
            for (path, face) in &strays {
                if photographs_likeness(std::slice::from_ref(face), references) >= CUT {
                    stray_taken += 1;
                    println!("stray: {name} took an everyday face in {}", path.display());
                }
            }
        }
        println!(
            "{} locks; other people's faces {cross_taken} of {cross}; everyday faces {stray_taken} of {} x {}",
            locks.len(),
            strays.len(),
            locks.len()
        );
    }
}
