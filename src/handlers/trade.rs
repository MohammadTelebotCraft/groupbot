use std::sync::Arc;

use grammers_client::message::Message;
use grammers_client::session::types::{PeerId, PeerRef};

use super::locks::View;
use super::{Ctx, intent, nsfw};

pub const LOCK: &str = "trade";

pub const SHADOW: &str = "trade_shadow";

pub const LIMIT: &str = "trade_lim";

pub const LIMIT_RANGE: (u32, u32) = (25, 60);
pub const LIMIT_PRESETS: &[u32] = &[25, 28, 30, 35, 40, 50];
pub const DEFAULT_LIMIT: u32 = 30;

const BRIDGE_FLOOR: u32 = 20;

pub fn limit(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings.with_chat(chat, |settings| {
        settings.number(LIMIT, DEFAULT_LIMIT, LIMIT_RANGE)
    })
}

fn over(margin: f32, limit: u32) -> bool {
    margin * 1000.0 >= limit as f32
}

fn is_terse(scrubbed: &str, marked: bool) -> bool {
    let words = scrubbed.split_whitespace().count();
    words <= 1 || (!marked && words <= 2)
}

fn deletes(margin: f32, limit: u32, marked: bool, terse: bool) -> bool {
    if terse {
        return false;
    }
    over(margin, limit) || (marked && over(margin, BRIDGE_FLOOR))
}

const ESCALATE_FROM: f32 = -20.0;
const ESCALATE_UNDER: u32 = 60;

fn undecided(margin: f32) -> bool {
    margin * 1000.0 >= ESCALATE_FROM && !over(margin, ESCALATE_UNDER)
}

pub fn judge(text: &str) -> Option<intent::Scored> {
    let scored = intent::score(text)?;
    if !undecided(scored.margin) || !intent::big_available() {
        return Some(scored);
    }
    Some(intent::score_big(text).unwrap_or(scored))
}

pub struct Reading {
    pub suspicious: bool,
    pub listing: bool,
    pub terse: bool,
}

pub fn read(lowered: &str, digits: &str) -> Reading {
    let lowered = without_joiners(lowered);
    let (scrubbed, tampered) = intent::descramble(&lowered);
    let suspicious = tampered || suspicious_in(&scrubbed, digits);

    let listing = listing_in(&scrubbed, digits);
    let terse = is_terse(&scrubbed, listing);
    Reading {
        suspicious,
        listing,
        terse,
    }
}

pub struct Armed {
    pub limit: u32,
    pub live: bool,
}

fn armed_under(settings: &super::super::state::ChatSettings<'_>) -> Option<Armed> {
    settings.is_locked(LOCK).then(|| Armed {
        limit: settings.number(LIMIT, DEFAULT_LIMIT, LIMIT_RANGE),
        live: !settings.is_locked(SHADOW),
    })
}

const MARKS: &[&str] = &[
    "فروش",
    "خرید",
    "میخرم",
    "بخرم",
    "خریدار",
    "معاوض",
    "اجاره",
    "رهن",
    "کرایه",
    "سفارش",
    "حراج",
    "عمده",

    "طاق",
    "تبادل",
    "ترید",
    "سکه",

    "قیمت",
    "تومان",
    "تومن",
    "ریال",
    "میلیون",
    "دلار",
    "تخفیف",
    "واریز",
    "بیعانه",
    "پرداخت",
    "درگاه",
    "کارت به کارت",
    "شماره کارت",

    "قسط",
    "اقساط",
    "نقدی",
    "رزرو",
    "اکانت",
    "پرمیوم",
    "هزینه",
    "شهریه",
    "تعرفه",
    "کارمزد",
    "اجرت",
    "فاکتور",
    "وصول",
    "پی پال",
    "شارژ",
    "یوسی",

    "کیلویی",
    "دونه ای",
    "مثقالی",
    "جلسه ای",
    "صفحه ای",
    "ساعتی",

    "موجود",
    "ارسال",
    "پیش فروش",
    "مزایده",

    "پیوی",
    "پی وی",
    "دایرکت",
    "خصوصی",
    "نوبت",
    "تعمیر",
    "چند میدی",
    "گیفت",
    "احراز",
    "صراف",
    "پروکسی",
    "کانفیگ",
    "لایسنس",
    "ارزون",
    "طراحی",
    "کارتخوان",
    "خدمات",
    "واگذار",
    "تضمین",
    "ضمانت",
    "بشتابید",
    "پکیج",
    "کاشت",
    "تالار",

    "sale",
    "sell",
    "buy",
    "price",
    "order",
    "shipping",
    "discount",
    "service",
    "rent",
    "trade",
    "trading",
    "swap",
    "account",
    "usdt",
    "$",

    "فورش",
    "خربدار",
    "خریدرام",
    "میفرش",

    "forosh",
    "foroosh",
    "mifrosham",
    "miforosham",
    "kharid",
    "gheymat",
    "toman",
    "arzoon",
    "arzan",
    "naghdi",
    "ejare",
    "tadris",
    "narkh",
    "sefaresh",
    "pv",
    "dm",
];

fn has_card_number(digits: &str) -> bool {
    let mut run: Vec<u8> = Vec::with_capacity(20);
    let mut between = false;
    let flush = |run: &mut Vec<u8>| {
        let card = run.len() == 16 && luhn(run);
        run.clear();
        card
    };
    for c in digits.chars() {
        if let Some(d) = c.to_digit(10) {
            run.push(d as u8);
            between = false;
            continue;
        }

        if (c == ' ' || c == '-' || c == '_' || c == '.') && !between && !run.is_empty() {
            between = true;
            continue;
        }
        between = false;
        if flush(&mut run) {
            return true;
        }
    }
    flush(&mut run)
}

fn luhn(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    for (at, d) in digits.iter().rev().enumerate() {
        let mut d = u32::from(*d);
        if at % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum.is_multiple_of(10)
}

fn without_joiners(lowered: &str) -> std::borrow::Cow<'_, str> {
    match lowered.contains('\u{200c}') {
        true => lowered.replace('\u{200c}', "").into(),
        false => lowered.into(),
    }
}

fn suspicious_in(text: &str, digits: &str) -> bool {
    MARKS.iter().any(|mark| text.contains(mark)) || has_card_number(digits)
}

const LISTING_WORDS: &[&str] = &[
    "میفروشم",
    "می فروشم",
    "میفروشیم",
    "خریدارم",
    "خریداریم",

    "طاق میزنم",
    "معاوضه میکنم",
    "تبادل میکنم",

    "خربدارم",
    "خریدرام",
    "میفورشم",
    "میفرشم",
    "فورشی",

    "miforosham",
    "mifrosham",
    "forooshi",
    "foroshi",
    "kharidaram",
];

fn standalone(text: &str, word: &str) -> Option<usize> {
    let joins = |c: char| c.is_alphanumeric();
    let mut from = 0;
    while let Some(at) = text[from..].find(word) {
        let start = from + at;
        let end = start + word.len();
        if !text[..start].chars().next_back().is_some_and(joins)
            && !text[end..].chars().next().is_some_and(joins)
        {
            return Some(end);
        }
        from = start + word.chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn listing_in(text: &str, digits: &str) -> bool {
    let rhetorical = standalone(text, "مگه").is_some()
        || ["گزارش", "کلاهبردار", "اسکم", "بلاکش", "پولشویی"]
            .iter()
            .any(|word| text.contains(word));

    let reported = |upto: usize| {
        ["میگه", "گفت", "میگفت", "نوشته", "زده", "فرستاده", "گفته"]
            .iter()
            .any(|q| text[..upto].split_whitespace().any(|w| w == *q))
    };
    if !rhetorical {
        for word in LISTING_WORDS {
            if let Some(end) = standalone(text, word) {
                let before = text[..end - word.len()].trim_end();

                let opens = before.matches('«').count() > before.matches('»').count()
                    || before.matches('"').count() % 2 == 1;
                if opens {
                    continue;
                }
                let prev = before.rsplit(' ').next().unwrap_or("");

                let quoted = before.ends_with('«')
                    || before.ends_with('"')
                    || before.ends_with('\'')
                    || reported(end - word.len());
                if quoted || prev == "نه" || prev.contains("ناز") {
                    continue;
                }
                return true;
            }
        }

        for verb in ["میفروشم", "میفروشیم"] {
            let mut from = 0;
            while let Some(at) = text[from..].find(verb) {
                let start = from + at;
                let end = start + verb.len();
                let free_right = !text[end..].chars().next().is_some_and(char::is_alphanumeric);
                let glued: usize = text[..start]
                    .chars()
                    .rev()
                    .take_while(|c| c.is_alphanumeric())
                    .count();
                let negated = text[..start].ends_with('ن');
                if free_right && glued > 1 && !negated {
                    return true;
                }
                from = start + verb.chars().next().map_or(1, char::len_utf8);
            }
        }
        if let Some(end) = standalone(text, "فروشی") {
            let start = end - "فروشی".len();
            if reported(start) {
                return !rhetorical && has_card_number(digits);
            }
            let tail = text[end..].trim_start();

            let compound = ["کم", "گران", "ارزان", "تن", "وطن"]
                .iter()
                .any(|head| text[..start].trim_end().ends_with(head));

            let asking = tail.starts_with('؟') || tail.starts_with('?');
            if !compound && !asking && !tail.starts_with("نیست") && !tail.starts_with("ها") {
                return true;
            }
        }

        if let Some(end) = standalone(text, "فروشیه") {
            if reported(end - "فروشیه".len()) {
                return !rhetorical && has_card_number(digits);
            }
            let tail = text[end..].trim_start();
            if !tail.starts_with('؟') && !tail.starts_with('?') {
                return true;
            }
        }
    }
    !rhetorical && has_card_number(digits)
}

pub async fn watch(ctx: &Arc<Ctx>, message: &Message, chat: i64, view: &View<'_>) {
    if view.text().is_empty() {
        return;
    }
    let Some(armed) = ctx
        .settings
        .with_chat(chat, |settings| armed_under(&settings))
    else {
        return;
    };
    let reading = read(view.lower(), view.digits());
    if !reading.suspicious {
        return;
    }
    let key = intent::text_key(view.text());
    let known = ctx.known_intent(key);
    if known.is_none() && !intent::available() {
        return;
    }

    if super::is_exempt(ctx, message).await {
        return;
    }
    let Reading { listing, terse, .. } = reading;
    if let Some(margin) = known {
        act_known(ctx, message, chat, key, margin, None, listing, terse, &armed).await;
        return;
    }
    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return;
    };
    let message_id = message.id();
    let sender = message.sender_id().and_then(PeerId::bare_id);
    let name = super::name_of(message);
    let text = view.text().to_owned();
    let ctx = Arc::clone(ctx);

    let task_slot = ctx.intent_task_slot().await;
    tokio::spawn(async move {
        let _task_slot = task_slot;
        let _slot = ctx.nsfw_slot().await;
        let started = std::time::Instant::now();
        let judged = tokio::task::spawn_blocking(move || judge(&text)).await.ok().flatten();
        let Some(scored) = judged else {
            log::info!("trade: chat {chat} text {key:016x} no verdict");
            return;
        };
        let margin = scored.margin;

        ctx.remember_intent(key, margin);
        act_detached(
            &ctx,
            chat,
            chat_ref,
            message_id,
            key,
            margin,
            Some(scored.frame),
            listing,
            terse,
            &armed,
            sender,
            &name,
            started.elapsed().as_millis(),
        )
        .await;
    });
}

#[allow(clippy::too_many_arguments)]
fn report(
    chat: i64,
    key: u64,
    margin: f32,
    frame: Option<u8>,
    listing: bool,
    terse: bool,
    armed: &Armed,
    cached: bool,
    millis: Option<u128>,
) {
    log::info!(
        "trade[{}]: chat {chat} text {key:016x} margin {margin:+.3}{}{}{} limit {} {}{}{}",
        if armed.live { "live" } else { "shadow" },
        match frame {
            Some(frame) => format!(" frame={}", intent::frame_name(frame)),
            None => String::new(),
        },
        if listing { " marked" } else { "" },
        if terse { " terse" } else { "" },
        armed.limit,
        if deletes(margin, armed.limit, listing, terse) {
            "OVER"
        } else {
            "ok"
        },
        if cached { " cached" } else { "" },
        match millis {
            Some(ms) => format!(" {ms}ms"),
            None => String::new(),
        }
    );
}

#[allow(clippy::too_many_arguments)]
async fn act_known(
    ctx: &Arc<Ctx>,
    message: &Message,
    chat: i64,
    key: u64,
    margin: f32,
    frame: Option<u8>,
    listing: bool,
    terse: bool,
    armed: &Armed,
) {
    report(chat, key, margin, frame, listing, terse, armed, true, None);
    if !deletes(margin, armed.limit, listing, terse) || !armed.live {
        return;
    }
    if let Err(e) = message.delete().await {
        eprintln!("trade: could not delete in {chat}: {e}");
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    let chances = match super::strict::punish(ctx, message, chat, LOCK).await {
        super::strict::Outcome::Announced => return,
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::notice::send(ctx, message, chat, "خرید و فروش", chances).await;
}

#[allow(clippy::too_many_arguments)]
async fn act_detached(
    ctx: &Arc<Ctx>,
    chat: i64,
    chat_ref: PeerRef,
    message_id: i32,
    key: u64,
    margin: f32,
    frame: Option<u8>,
    listing: bool,
    terse: bool,
    armed: &Armed,
    sender: Option<i64>,
    name: &str,
    millis: u128,
) {
    report(chat, key, margin, frame, listing, terse, armed, false, Some(millis));
    if !deletes(margin, armed.limit, listing, terse) || !armed.live {
        return;
    }
    match ctx.client.delete_messages(chat_ref, &[message_id]).await {
        Ok(0) => {
            eprintln!("trade: delete affected nothing in {chat} msg {message_id}");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("trade: could not delete in {chat} msg {message_id}: {e}");
            return;
        }
    }
    ctx.bump(chat, super::stats::DELETED);
    nsfw::punish_and_notify(ctx, chat, chat_ref, sender, name, LOCK, "خرید و فروش").await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(text: &str) -> bool {
        let lowered = text.to_lowercase();
        let stripped: String = lowered.replace('\u{200c}', "");
        let (scrubbed, tampered) = intent::descramble(&stripped);
        tampered || suspicious_in(&scrubbed, &super::super::digits(text))
    }

    #[test]
    fn the_net_catches_every_selling_shape() {
        for text in [
            "گوشی آیفون فروشی، تماس بگیرید",
            "میفروشم پلی استیشن ۵ نو",
            "می\u{200c}فروشم گوشی سالم",
            "خریدارم گوشی تا ۱۰ میلیون",
            "قیمت بدید برای این کار",
            "ثبت سفارش فقط با واریز بیعانه",
            "شماره کارت 6037-9918-1234-5670 به نام محمدی",
            "شماره کارت ۶۰۳۷۹۹۱۸۱۲۳۴۵۶۷۰",
            "اجاره سوئیت مبله روزانه",
            "معاوضه با آیفون",
            "iPhone for sale, DM me",
            "selling my account, USDT accepted",
            "این جنس ۲۰۰ تومنه",

            "م.یو خربدارم",
            "فـروشی ماشین صفر",
            "ف ر و ش ی گوشی",
        ] {
            assert!(hits(text), "the net missed «{text}»");
        }
    }

    #[test]
    fn ordinary_chat_stays_outside_the_net() {
        for text in [
            "سلام بچه ها، خوبید؟",
            "فردا ساعت چند کلاس داریم؟",
            "دیشب بازی رو دیدید؟ چه گلی زد",
            "ممنون از راهنماییت",
        ] {
            assert!(!hits(text), "the net caught «{text}»");
        }
    }

    #[test]
    fn the_card_scan_is_exact() {
        assert!(has_card_number("6037991812345670"));
        assert!(has_card_number("6037-9918-1234-5670"));
        assert!(has_card_number("6037 9918 1234 5670"));
        assert!(!has_card_number("603799181234567"), "fifteen digits");
        assert!(!has_card_number("6037--9918--1234--5678"), "double separators");
        assert!(!has_card_number("تولد من 1370/05/21 بود و شماره خونه 22334455"));
        assert!(!has_card_number(""));
    }

    #[test]
    fn the_listing_marker_reads_the_register() {
        let mark = |t: &str| {
            let (scrubbed, _) = intent::descramble(&without_joiners(&t.to_lowercase()));
            listing_in(&scrubbed, &super::super::digits(t))
        };
        assert!(mark("ماشین صفر فروشی"));
        assert!(mark("ف.روشی گوشی سالم"), "the dotted listing word is still the word");
        assert!(mark("میو پوینت میلی ۲ میفروشم"));
        assert!(mark("می\u{200c}فروشم گوشی سالم"), "the half-space form");
        assert!(mark("می فروشم گوشی سالم"), "the spaced form");
        assert!(mark("طلا خریدارم"));
        assert!(mark("واریز به 6037-9918-1234-5670"));
        assert!(mark("سکه سلف طاق میزنم با میو پوینت"), "barter slang");
        assert!(mark("معاوضه میکنم گوشیمو با تبلت"));

        assert!(!mark("ماشینمو نمیفروشم عمرا"), "the ن joins");
        assert!(!mark("فکر کردی میفروشمش؟"), "the ش joins");
        assert!(!mark("این فروشی نیست داداش"), "an explicit refusal");
        assert!(!mark("خونه ما فروشی نیست، اشتباه زنگ زدن"));
        assert!(
            !mark("کتاب فروشی های انقلاب هنوز سرجاشونن؟"),
            "the plural is the shop noun, never a listing"
        );
        assert!(!mark("عمرا اکانتمو بفروشم"), "subjunctive, not a listing");
        assert!(!mark("حوصلم دیگه طاق شده از این وضع"), "the idiom, not the barter");
        assert!(!mark("من طاق نمیزنم باهات"), "the negated verb breaks the phrase");
        assert!(!mark("مگه من طاق میزنم؟ عمرا"), "a rhetorical denial, not a listing");
        assert!(!mark("مگه ماشینتو میفروشی که قیمت میپرسی؟"));
        assert!(
            !mark("در روایات از کم فروشی نهی شده"),
            "«کم فروشی» is the practice noun, not a listing"
        );
        assert!(!mark("گران فروشی جریمه داره"));
        assert!(mark("این گوشی فروشیه، ۱۵ تومن"), "the suffixed statement is a listing");
        assert!(!mark("این فروشیه؟"), "the suffixed question is a buyer asking");
        assert!(!mark("ماشینت فروشی؟ چند؟"), "the bare word with a question mark too");
        assert!(!mark("فروشی?"));
        assert!(mark("ماشین فروشی، ۲۰۰ تومن؟ نه، ۱۸۰"), "a question later on is not about the word");
        assert!(!mark("مگه فروشیه که قیمت میپرسی؟"));
        assert!(mark("seke kharidaram naghdi"), "the transliterated register marks");
        assert!(
            !mark("نه میخرم نه میفروشم، فقط اومدم حرف بزنم"),
            "«نه» before the verb is a refusal"
        );
        assert!(!mark("فقط نازتو خریدارم"), "affection, not commerce");
        assert!(
            !mark("طرف اومده پیوی میگه میفروشم ارزون، بلاکش کردم"),
            "reported speech is somebody warning, not selling"
        );
        assert!(!mark("نوشته بود «تتر میفروشم زیر قیمت» — گزارش بدید"));
        assert!(mark("موتورمیفروشم داداش پیامبده"), "the glued first person is a listing");
        assert!(!mark("ماشینمو نمیفروشمش عمرا"), "the lone negation prefix stays out");
        assert!(!mark("ماشینمونمیفروشم به هیچ قیمتی"), "a glued «ن» may be the negation");
        assert!(mark("ماشینمومیفروشم ۲۰۰ تومن"), "glued without the «ن» is the verb");
        assert!(!mark("سلام بچه ها، خوبید؟"));
    }

    #[test]
    fn the_bridge_only_lowers_the_bar_for_marked_messages() {
        assert!(deletes(0.031, DEFAULT_LIMIT, false, false), "over the limit needs no marker");
        assert!(!deletes(0.022, DEFAULT_LIMIT, false, false), "under the limit, unmarked: kept");
        assert!(deletes(0.022, DEFAULT_LIMIT, true, false), "a marked listing crosses at the floor");
        assert!(
            !deletes(0.099, DEFAULT_LIMIT, false, true),
            "a terse fragment is never deleted, whatever the margin — «موجودی» was real"
        );
        assert!(
            !deletes(0.015, DEFAULT_LIMIT, true, false),
            "the marker alone never deletes — the floor still holds"
        );
        assert!(!deletes(-0.010, LIMIT_RANGE.0, true, false), "a marked refusal stays");
        assert!(BRIDGE_FLOOR < LIMIT_RANGE.0, "the floor must undercut every limit");
    }

    #[test]
    #[ignore = "needs intent.onnx, intent_big.onnx, intent_vocab.txt and intent_frames.txt"]
    fn the_battery_holds_in_the_real_pipeline() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tools/data/intent_battery.tsv");
        let text = std::fs::read_to_string(path).expect("the battery file");
        let mut per_kind: std::collections::BTreeMap<&str, (usize, usize, usize)> =
            std::collections::BTreeMap::new();
        let mut false_positives = Vec::new();
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            let (label, kind, body) = (
                parts.next().unwrap(),
                parts.next().unwrap(),
                parts.next().unwrap().trim(),
            );
            let reading = read(&body.to_lowercase(), &super::super::digits(body));
            let verdict = reading
                .suspicious
                .then(|| judge(body))
                .flatten();
            for limit in [DEFAULT_LIMIT, LIMIT_RANGE.0] {
                let deleted = verdict.is_some_and(|scored| {
                    deletes(scored.margin, limit, reading.listing, reading.terse)
                });
                let entry = per_kind.entry(kind).or_default();
                if limit == DEFAULT_LIMIT {
                    if label == "sell" {
                        entry.0 += 1;
                        entry.1 += usize::from(deleted);
                    } else {
                        entry.2 += 1;
                    }
                }
                if deleted && label != "sell" {
                    false_positives.push(format!(
                        "limit {limit} {kind} {:+.3} {}: {body}",
                        verdict.map_or(0.0, |s| s.margin),
                        verdict.map_or("?", |s| intent::frame_name(s.frame)),
                    ));
                }
            }
        }
        let (mut sells, mut caught) = (0, 0);
        for (kind, (s, c, n)) in &per_kind {
            println!("{kind:12} sell {s:3} caught {c:3}  negatives {n:3}");
            sells += s;
            caught += c;
        }
        println!("recall at {DEFAULT_LIMIT}: {caught}/{sells}");
        assert!(
            false_positives.is_empty(),
            "the battery produced false positives:\n{}",
            false_positives.join("\n")
        );
    }

    #[test]
    fn the_presets_sit_inside_the_range() {
        assert!(LIMIT_PRESETS.contains(&DEFAULT_LIMIT));
        for preset in LIMIT_PRESETS {
            assert!(*preset >= LIMIT_RANGE.0 && *preset <= LIMIT_RANGE.1);
        }
    }

    #[test]
    fn no_mark_carries_a_zwnj() {
        for mark in MARKS {
            assert!(!mark.contains('\u{200c}'), "«{mark}» carries a ZWNJ");
            assert_eq!(*mark, mark.to_lowercase(), "«{mark}» is not lowercase");
        }
    }
}
