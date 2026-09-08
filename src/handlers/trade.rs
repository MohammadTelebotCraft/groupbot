
use std::sync::Arc;

use grammers_client::message::Message;
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;

use super::locks::View;
use super::{Ctx, intent, nsfw};

#[path = "trade_context.rs"]
pub mod context;
#[path = "trade_language.rs"]
mod language;

pub const LOCK: &str = "trade";

pub const SHADOW: &str = "trade_shadow";

pub const LIMIT: &str = "trade_lim";

pub const LIMIT_RANGE: (u32, u32) = (30, 60);
pub const LIMIT_PRESETS: &[u32] = &[30, 35, 40, 50, 60];
pub const DEFAULT_LIMIT: u32 = 30;

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

fn deletes(margin: f32, limit: u32, _marked: bool, terse: bool) -> bool {
    !terse && margin.is_finite() && over(margin, limit)
}

const ESCALATE_FROM: f32 = -20.0;
const ESCALATE_UNDER: u32 = 60;

fn undecided(margin: f32) -> bool {
    margin * 1000.0 >= ESCALATE_FROM && !over(margin, ESCALATE_UNDER)
}

fn transactional_frame(frame: &str) -> bool {
    matches!(
        frame,
        "offer" | "want" | "exchange" | "service" | "rental" | "payment" | "indirect"
    )
}

fn authored(text: &str) -> String {
    let mut out = String::new();
    let mut closing = None;
    for line in text.lines() {
        if line.trim_start().starts_with('>') {
            continue;
        }
        for (offset, c) in line.char_indices() {
            let contraction = c == '\''
                && line[..offset]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric)
                && line[offset + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric);
            if let Some(end) = closing {
                if c == end && !contraction {
                    closing = None;
                    out.push(' ');
                }
                continue;
            }
            closing = match c {
                '«' => Some('»'),
                '“' => Some('”'),
                '‘' => Some('’'),
                '"' => Some('"'),
                '\'' if !contraction => Some('\''),
                '`' => Some('`'),
                _ => None,
            };
            if closing.is_none() {
                out.push(c);
            }
        }
        out.push(' ');
    }
    out.trim().to_owned()
}

fn text_key(text: &str) -> u64 {
    intent::text_key(&format!("trade-v3\0{text}\0authored\0{}", authored(text)))
}

fn outcome_score(outcome: intent::ScoreOutcome) -> Option<intent::Scored> {
    match outcome {
        intent::ScoreOutcome::Scored(scored) => Some(scored),
        intent::ScoreOutcome::Unavailable | intent::ScoreOutcome::Abstained(_) => None,
    }
}

fn try_judge(text: &str) -> Result<Option<intent::Scored>, super::nsfw::ModelError> {
    if authored(text).is_empty() {
        return Ok(None);
    }
    let clean = intent::descramble(text).0;
    let expanded = language::proposition(&clean);
    let input = expanded.as_deref().unwrap_or(&clean);
    let Some(small) = outcome_score(intent::score(input)?) else {
        return Ok(None);
    };
    let mut scored = small;
    if undecided(scored.margin) {
        if !intent::big_available() {
            return Ok(None);
        }
        let Some(escalated) = outcome_score(intent::score_big(input)?) else {
            return Ok(None);
        };
        scored = escalated;
        if language::corroborated(input)
            && transactional_frame(intent::frame_name(small.frame))
            && transactional_frame(intent::frame_name(scored.frame))
            && small.margin >= 0.020
            && scored.margin >= 0.020
            && small.margin > scored.margin
        {
            scored = small;
        }
    }
    if !transactional_frame(intent::frame_name(scored.frame)) {
        scored.margin = scored.margin.min(0.0);
    }
    Ok(Some(scored))
}

#[cfg(test)]
pub fn judge(text: &str) -> Option<intent::Scored> {
    match try_judge(text) {
        Ok(scored) => scored,
        Err(error) => {
            log::error!("trade: intent inference failed: {error}");
            None
        }
    }
}

fn try_judge_followup(
    current: &str,
    parent: &str,
) -> Result<Option<(intent::Scored, Reading)>, super::nsfw::ModelError> {
    let clean = intent::descramble(&authored(current)).0;
    if clean != intent::descramble(current).0 {
        return Ok(None);
    }
    let Some(kind) = context::followup(&clean) else {
        return Ok(None);
    };
    let Some(anchor) = try_judge(parent)? else {
        return Ok(None);
    };
    let parent = intent::descramble(&authored(parent)).0;
    let Some(object) = context::object(&parent) else {
        return Ok(None);
    };
    let inquiry = intent::frame_name(anchor.frame) == "inquiry";
    let direct_question = inquiry
        && match kind {
            context::Followup::Sell => parent.contains("میفروشی") || parent.contains("for sale"),
            context::Followup::Buy => parent.contains("میخری") || parent.contains("want to buy"),
            context::Followup::Amount => {
                parent.contains("چقدر میدی")
                    || parent.contains("چند میدی")
                    || parent.contains("your offer")
            }
            context::Followup::Exchange => parent.contains("معاوضه") || parent.contains("تعویض"),
            context::Followup::Contact => false,
        };
    let reading = read(&parent, &super::digits(&parent));
    if !direct_question && !deletes(anchor.margin, DEFAULT_LIMIT, reading.listing, reading.terse) {
        return Ok(None);
    }
    let resolved = context::resolve(kind, &clean, object);
    let Some(scored) = try_judge(&resolved)? else {
        return Ok(None);
    };
    Ok(Some((scored, read(&resolved, &super::digits(&resolved)))))
}

#[cfg(test)]
fn judge_followup(current: &str, parent: &str) -> Option<(intent::Scored, Reading)> {
    match try_judge_followup(current, parent) {
        Ok(scored) => scored,
        Err(error) => {
            log::error!("trade: contextual intent inference failed: {error}");
            None
        }
    }
}

pub struct Reading {
    pub suspicious: bool,
    pub listing: bool,
    pub terse: bool,
}

pub fn read(lowered: &str, digits: &str) -> Reading {
    let own = authored(lowered);
    let own_digits;
    let digits = if own == lowered.trim() {
        digits
    } else {
        own_digits = super::digits(&own);
        &own_digits
    };
    let (scrubbed, tampered) = intent::descramble(&own);
    let suspicious = tampered || suspicious_in(&scrubbed, digits);
    let listing = listing_in(&scrubbed, digits);
    let terse = is_terse(&scrubbed, listing)
        || context::followup(&scrubbed).is_some()
        || (context::object(&scrubbed).is_none()
            && ["نازتو", "نازت", "رفاقت", "friendship"]
                .iter()
                .any(|word| scrubbed.contains(word)));
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
    "تعویض",
    "عوض میکنم",
    "پیشنهاد",
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

#[cfg(test)]
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
                let free_right = !text[end..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric);
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


fn message_text(message: &Message) -> std::borrow::Cow<'_, str> {
    let tl::enums::Message::Message(raw) = &message.raw else {
        return message.text().into();
    };
    let ranges: Vec<_> = raw
        .entities
        .iter()
        .flatten()
        .filter_map(|entity| match entity {
            tl::enums::MessageEntity::Blockquote(e) => Some((e.offset, e.length)),
            tl::enums::MessageEntity::Pre(e) => Some((e.offset, e.length)),
            tl::enums::MessageEntity::Code(e) => Some((e.offset, e.length)),
            _ => None,
        })
        .collect();
    if ranges.is_empty() {
        return message.text().into();
    }
    quote_entities(message.text(), &ranges).into()
}

fn quote_entities(text: &str, ranges: &[(i32, i32)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut offset = 0;
    let mut quoting = false;
    for c in text.chars() {
        let covered = ranges.iter().any(|(start, len)| {
            *start >= 0 && *len > 0 && offset >= *start && offset < start.saturating_add(*len)
        });
        if covered != quoting {
            out.push(if covered { '«' } else { '»' });
        }
        out.push(c);
        quoting = covered;
        offset += c.len_utf16() as i32;
    }
    if quoting {
        out.push('»');
    }
    out
}

fn local_reply(message: &Message) -> (Option<i32>, bool) {
    let tl::enums::Message::Message(raw) = &message.raw else {
        return (None, false);
    };
    let Some(header) = raw.reply_to.as_ref() else {
        return (None, true);
    };
    let tl::enums::MessageReplyHeader::Header(header) = header else {
        return (None, false);
    };
    (local_reply_id(header), false)
}

fn local_reply_id(header: &tl::types::MessageReplyHeader) -> Option<i32> {
    if header.reply_to_peer_id.is_some()
        || (header.forum_topic
            && (header.reply_to_top_id.is_none()
                || header.reply_to_top_id == header.reply_to_msg_id))
    {
        return None;
    }
    header.reply_to_msg_id
}

pub async fn watch(ctx: &Arc<Ctx>, message: &Message, chat: i64, view: &View<'_>) {
    let Some(_) = ctx
        .settings
        .with_chat(chat, |settings| armed_under(&settings))
    else {
        return;
    };
    let sender = message.sender_id().and_then(PeerId::bare_id);
    let (reply_id, nearby) = local_reply(message);
    let text = message_text(message);
    let parent = ctx.trade_history.lock().unwrap().observe(
        chat,
        message.id(),
        nearby.then_some(sender).flatten(),
        reply_id,
        &text,
    );
    if text.is_empty() {
        return;
    }
    let followup = context::followup(&intent::descramble(&authored(&text)).0).is_some();
    let contextual = followup && (parent.is_some() || reply_id.is_some());
    let reading = read(&text, view.digits());
    if !reading.suspicious && !contextual {
        return;
    }
    let key = text_key(&text);
    let known = (!contextual).then(|| ctx.known_intent(key)).flatten();
    if known.is_none() && !intent::available() {
        return;
    }
    if super::is_exempt(ctx, message).await {
        return;
    }
    let Reading { listing, terse, .. } = reading;
    if let Some(margin) = known {
        if !ctx
            .trade_history
            .lock()
            .unwrap()
            .is_current(chat, message.id(), &text)
        {
            return;
        }
        let Some(armed) = ctx
            .settings
            .with_chat(chat, |settings| armed_under(&settings))
        else {
            return;
        };
        act_known(
            ctx,
            message,
            TradeVerdict {
                chat,
                key,
                margin,
                frame: None,
                listing,
                terse,
                armed: &armed,
            },
        )
        .await;
        return;
    }
    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return;
    };
    let message_id = message.id();
    let name = super::name_of(message);
    let text = text.into_owned();
    let reply_message = contextual.then(|| message.clone());
    let ctx = Arc::clone(ctx);
    let task_slot = ctx.intent_task_slot().await;
    Arc::clone(&ctx).spawn_owned(async move {
        let _task_slot = task_slot;
        let parent = if contextual && parent.is_none() {
            if let Some(message) = reply_message {
                tokio::time::timeout(std::time::Duration::from_secs(3), message.get_reply())
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .flatten()
                    .filter(|reply| {
                        reply.peer_id() == message.peer_id()
                            && (0..180)
                                .contains(&(message.date().as_second() - reply.date().as_second()))
                            && reply.text().len() <= 16_384
                    })
                    .map(|reply| message_text(&reply).into_owned())
            } else {
                None
            }
        } else {
            parent
        };
        let _slot = ctx.nsfw_slot().await;
        let started = std::time::Instant::now();
        let original = text.clone();
        let judged = tokio::task::spawn_blocking(
            move || -> Result<Option<(intent::Scored, bool, bool)>, super::nsfw::ModelError> {
                if contextual {
                    let Some(parent) = parent.as_deref() else {
                        return Ok(None);
                    };
                    Ok(try_judge_followup(&text, parent)?
                        .map(|(scored, reading)| (scored, reading.listing, reading.terse)))
                } else {
                    Ok(try_judge(&text)?.map(|scored| (scored, listing, terse)))
                }
            },
        )
        .await;
        let judged = match judged {
            Ok(Ok(judged)) => judged,
            Ok(Err(error)) => {
                log::error!(
                    "trade: chat {chat} text {key:016x} model failure: {error}; result not cached"
                );
                return;
            }
            Err(error) => {
                log::error!(
                    "trade: chat {chat} text {key:016x} worker failed: {error}; result not cached"
                );
                return;
            }
        };
        let Some((scored, listing, terse)) = judged else {
            log::info!("trade: chat {chat} text {key:016x} no verdict");
            return;
        };
        let margin = scored.margin;
        if !contextual {
            ctx.remember_intent(key, margin);
        }
        if !ctx
            .trade_history
            .lock()
            .unwrap()
            .is_current(chat, message_id, &original)
        {
            return;
        }
        let Some(armed) = ctx
            .settings
            .with_chat(chat, |settings| armed_under(&settings))
        else {
            return;
        };
        act_detached(
            &ctx,
            DetachedTrade {
                chat_ref,
                message_id,
                verdict: TradeVerdict {
                    chat,
                    key,
                    margin,
                    frame: Some(scored.frame),
                    listing,
                    terse,
                    armed: &armed,
                },
                sender,
                name: &name,
                millis: started.elapsed().as_millis(),
            },
        )
        .await;
    });
}

#[derive(Clone, Copy)]
struct TradeVerdict<'a> {
    chat: i64,
    key: u64,
    margin: f32,
    frame: Option<u8>,
    listing: bool,
    terse: bool,
    armed: &'a Armed,
}

fn report(verdict: &TradeVerdict<'_>, cached: bool, millis: Option<u128>) {
    let TradeVerdict {
        chat,
        key,
        margin,
        frame,
        listing,
        terse,
        armed,
    } = *verdict;
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

async fn act_known(ctx: &Arc<Ctx>, message: &Message, verdict: TradeVerdict<'_>) {
    report(&verdict, true, None);
    let TradeVerdict {
        chat,
        margin,
        listing,
        terse,
        armed,
        ..
    } = verdict;
    if !deletes(margin, armed.limit, listing, terse) || !armed.live {
        return;
    }
    if let Err(e) = message.delete_critical().await {
        eprintln!("trade: could not delete in {chat}: {e}");
        return;
    }
    ctx.bump(chat, super::stats::DELETED);
    let chances = match super::strict::punish(ctx, message, chat, LOCK).await {
        super::strict::Outcome::Announced => {
            let action = super::cases::action_key(super::strict::action_of(ctx, chat));
            super::cases::record_delete(ctx, message, LOCK, "خرید و فروش", action).await;
            return;
        }
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::cases::record_delete(ctx, message, LOCK, "خرید و فروش", "delete").await;
    super::notice::send(ctx, message, chat, "خرید و فروش", chances).await;
}

struct DetachedTrade<'a> {
    chat_ref: PeerRef,
    message_id: i32,
    verdict: TradeVerdict<'a>,
    sender: Option<i64>,
    name: &'a str,
    millis: u128,
}

async fn act_detached(ctx: &Arc<Ctx>, detached: DetachedTrade<'_>) {
    let DetachedTrade {
        chat_ref,
        message_id,
        verdict,
        sender,
        name,
        millis,
    } = detached;
    report(&verdict, false, Some(millis));
    let TradeVerdict {
        chat,
        margin,
        listing,
        terse,
        armed,
        ..
    } = verdict;
    if !deletes(margin, armed.limit, listing, terse) || !armed.live {
        return;
    }
    match ctx
        .client
        .delete_messages_critical(chat_ref, &[message_id])
        .await
    {
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
    nsfw::punish_and_notify(
        ctx,
        nsfw::DetachedModeration {
            chat,
            chat_ref,
            message_id,
            sender,
            name,
            cause: LOCK,
            reason: "خرید و فروش",
        },
    )
    .await;
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
        assert!(
            !has_card_number("6037--9918--1234--5678"),
            "double separators"
        );
        assert!(!has_card_number(
            "تولد من 1370/05/21 بود و شماره خونه 22334455"
        ));
        assert!(!has_card_number(""));
    }

    #[test]
    fn the_listing_marker_reads_the_register() {
        let mark = |t: &str| {
            let (scrubbed, _) = intent::descramble(&without_joiners(&t.to_lowercase()));
            listing_in(&scrubbed, &super::super::digits(t))
        };
        assert!(mark("ماشین صفر فروشی"));
        assert!(
            mark("ف.روشی گوشی سالم"),
            "the dotted listing word is still the word"
        );
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
        assert!(
            !mark("حوصلم دیگه طاق شده از این وضع"),
            "the idiom, not the barter"
        );
        assert!(
            !mark("من طاق نمیزنم باهات"),
            "the negated verb breaks the phrase"
        );
        assert!(
            !mark("مگه من طاق میزنم؟ عمرا"),
            "a rhetorical denial, not a listing"
        );
        assert!(!mark("مگه ماشینتو میفروشی که قیمت میپرسی؟"));
        assert!(
            !mark("در روایات از کم فروشی نهی شده"),
            "«کم فروشی» is the practice noun, not a listing"
        );
        assert!(!mark("گران فروشی جریمه داره"));
        assert!(
            mark("این گوشی فروشیه، ۱۵ تومن"),
            "the suffixed statement is a listing"
        );
        assert!(
            !mark("این فروشیه؟"),
            "the suffixed question is a buyer asking"
        );
        assert!(
            !mark("ماشینت فروشی؟ چند؟"),
            "the bare word with a question mark too"
        );
        assert!(!mark("فروشی?"));
        assert!(
            mark("ماشین فروشی، ۲۰۰ تومن؟ نه، ۱۸۰"),
            "a question later on is not about the word"
        );
        assert!(!mark("مگه فروشیه که قیمت میپرسی؟"));
        assert!(
            mark("seke kharidaram naghdi"),
            "the transliterated register marks"
        );
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
        assert!(
            mark("موتورمیفروشم داداش پیامبده"),
            "the glued first person is a listing"
        );
        assert!(
            !mark("ماشینمو نمیفروشمش عمرا"),
            "the lone negation prefix stays out"
        );
        assert!(
            !mark("ماشینمونمیفروشم به هیچ قیمتی"),
            "a glued «ن» may be the negation"
        );
        assert!(
            mark("ماشینمومیفروشم ۲۰۰ تومن"),
            "glued without the «ن» is the verb"
        );
        assert!(!mark("سلام بچه ها، خوبید؟"));
    }

    #[test]
    fn deletion_requires_the_full_threshold_and_finite_evidence() {
        assert!(deletes(0.031, DEFAULT_LIMIT, false, false));
        assert!(!deletes(0.022, DEFAULT_LIMIT, true, false));
        assert!(!deletes(0.040, 50, true, false));
        assert!(!deletes(0.099, DEFAULT_LIMIT, true, true));
        assert!(!deletes(f32::NAN, DEFAULT_LIMIT, true, false));
        assert!(!deletes(f32::INFINITY, DEFAULT_LIMIT, true, false));
        assert!(!deletes(-0.010, LIMIT_RANGE.0, true, false));
    }

    #[test]
    fn quotations_cannot_supply_the_speakers_intent() {
        for text in [
            "«اکانت رو میفروشم ۵۰۰»",
            "\"selling my account for $500\"",
            "> اکانت فروشی",
        ] {
            assert!(authored(text).is_empty(), "{text}");
            assert!(!read(text, &super::super::digits(text)).suspicious);
        }
        assert_eq!(
            authored("گفت «میفروشم» ادمین بررسی کن"),
            "گفت   ادمین بررسی کن"
        );
        assert!(authored("«سلام» اکانتم رو میفروشم").contains("میفروشم"));
        assert_eq!(authored("don't sell my account"), "don't sell my account");
        assert_eq!(authored("'selling my account for $500'"), "");
        assert_eq!(authored("'don't sell my account'"), "");
        assert_eq!(authored("‘اکانت فروشی’"), "");
        assert!(
            read(
                "این شماره «6037991812345670»",
                "این شماره «6037991812345670»"
            )
            .terse
        );
        assert_eq!(
            quote_entities("😀 اکانت فروشی", &[(3, 11)]),
            "😀 «اکانت فروشی»"
        );
        assert_eq!(authored(&quote_entities("اکانت فروشی", &[(0, 11)])), "");
        assert_ne!(
            text_key("> سلام\nاکانت فروشی"),
            text_key("> سلام اکانت فروشی")
        );
        assert_eq!(
            text_key("اکانت میفروشم"),
            text_key("ا.ک.ا.ن.ت می\u{200c}فروشم")
        );
    }

    #[test]
    fn only_direct_local_replies_can_supply_an_anchor() {
        let mut header = tl::types::MessageReplyHeader {
            reply_to_scheduled: false,
            forum_topic: false,
            quote: false,
            reply_to_ephemeral: false,
            reply_to_msg_id: Some(42),
            reply_to_peer_id: None,
            reply_from: None,
            reply_media: None,
            reply_to_top_id: None,
            quote_text: None,
            quote_entities: None,
            quote_offset: None,
            todo_item_id: None,
            poll_option: None,
        };
        assert_eq!(local_reply_id(&header), Some(42));
        header.forum_topic = true;
        assert_eq!(local_reply_id(&header), None);
        header.reply_to_top_id = Some(42);
        assert_eq!(local_reply_id(&header), None);
        header.reply_to_top_id = Some(7);
        assert_eq!(local_reply_id(&header), Some(42));
        header.reply_to_peer_id = Some(tl::types::PeerChannel { channel_id: 99 }.into());
        assert_eq!(local_reply_id(&header), None);
    }

    #[test]
    #[ignore = "needs intent.onnx, intent_big.onnx, intent_vocab.txt and intent_frames.txt"]
    fn the_battery_holds_in_the_real_pipeline() {
        assert!(
            intent::available() && intent::big_available(),
            "both shipped models are required"
        );
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/data/intent_battery.tsv");
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
            let verdict = reading.suspicious.then(|| judge(body)).flatten();
            for (index, limit) in [DEFAULT_LIMIT, LIMIT_RANGE.0].into_iter().enumerate() {
                let deleted = verdict.is_some_and(|scored| {
                    deletes(scored.margin, limit, reading.listing, reading.terse)
                });
                let entry = per_kind.entry(kind).or_default();
                if index == 0 {
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
            caught * 100 >= sells * 80,
            "battery recall regressed below 80%"
        );
        assert!(
            false_positives.is_empty(),
            "the battery produced false positives:\n{}",
            false_positives.join("\n")
        );
    }

    #[test]
    #[ignore = "needs shipped intent models; evaluates standalone and contextual regression corpus"]
    fn contextual_regression_corpus() {
        assert!(intent::available(), "small intent model must be installed");
        assert!(
            intent::big_available(),
            "escalation model must be installed"
        );
        let rows = include_str!("../../tools/data/intent_context.tsv");
        let mut false_positives = Vec::new();
        let mut missed = Vec::new();
        let mut positives = 0;
        let mut negatives = 0;
        for line in rows.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            let (label, kind, text) = (fields[0], fields[1], fields[2]);
            let parent = fields.get(3).copied().unwrap_or("");
            let reading = read(text, &super::super::digits(text));
            let result = if parent.is_empty() {
                reading
                    .suspicious
                    .then(|| judge(text))
                    .flatten()
                    .map(|score| (score, reading))
            } else {
                judge_followup(text, parent)
            };
            let deleted = result
                .as_ref()
                .is_some_and(|(s, r)| deletes(s.margin, DEFAULT_LIMIT, r.listing, r.terse));
            let description = format!(
                "{kind} {} {:+.3} {}: {text} [reply={parent}]",
                if deleted { "DELETE" } else { "KEEP" },
                result.as_ref().map_or(0.0, |(s, _)| s.margin),
                result
                    .as_ref()
                    .map_or("abstain", |(s, _)| intent::frame_name(s.frame))
            );
            println!("{description}");
            if label == "sell" {
                positives += 1;
                if !deleted {
                    missed.push(description);
                }
            } else {
                negatives += 1;
                if deleted {
                    false_positives.push(description);
                }
            }
        }
        println!(
            "context corpus: caught {}/{positives}; false positives {}/{negatives}",
            positives - missed.len(),
            false_positives.len()
        );
        println!("uncertain misses:\n{}", missed.join("\n"));
        assert!(
            false_positives.is_empty(),
            "false positives:\n{}",
            false_positives.join("\n")
        );
        assert!(
            (positives - missed.len()) * 100 >= positives * 95,
            "recall below 95%"
        );
    }

    #[test]
    #[ignore = "invoked by tools/evaluate_intent.py with explicit input and output files"]
    fn export_runtime_evaluation() {
        let Ok(input) = std::env::var("TRADE_EVAL_INPUT") else {
            return;
        };
        let output = std::env::var("TRADE_EVAL_OUTPUT").expect("output path");
        assert!(
            intent::available() && intent::big_available(),
            "both shipped models are required"
        );
        let data = std::fs::read_to_string(input).expect("evaluation corpus");
        let mut rows = Vec::new();
        for line in data.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            let (label, kind, body, parent) = match fields.as_slice() {
                [label, body] => (*label, "-", *body, ""),
                [label, kind, body] => (*label, *kind, *body, ""),
                [label, kind, body, parent] => (*label, *kind, *body, *parent),
                _ => panic!("malformed evaluation row"),
            };
            let reading = read(body, &super::super::digits(body));
            let result = if parent.is_empty() {
                reading
                    .suspicious
                    .then(|| judge(body))
                    .flatten()
                    .map(|s| (s, reading))
            } else {
                judge_followup(body, parent)
            };
            let deleted = |limit| {
                result
                    .as_ref()
                    .is_some_and(|(s, r)| deletes(s.margin, limit, r.listing, r.terse))
            };
            rows.push(serde_json::json!({
                "label": label, "kind": kind, "text": body, "parent": parent,
                "normalized": intent::descramble(body).0,
                "margin": result.as_ref().map(|(s, _)| s.margin),
                "frame": result.as_ref().map(|(s, _)| intent::frame_name(s.frame)),
                "deleted": deleted(DEFAULT_LIMIT), "deleted_at_minimum": deleted(LIMIT_RANGE.0),
            }));
        }
        std::fs::write(output, serde_json::to_vec_pretty(&rows).unwrap()).expect("write report");
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
