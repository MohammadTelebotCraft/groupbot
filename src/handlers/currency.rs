use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use grammers_client::message::{InputMessage, Message, ReplyMarkup};
use grammers_client::update::CallbackQuery;
use scraper::{ElementRef, Html, Selector};

use super::Ctx;

pub const COMMANDS: &[&str] = &["نرخ ارز", "قیمت ارز"];

const SOURCE_URL: &str = "https://alanchand.com/currencies-price";
const CACHE_TTL: Duration = Duration::from_secs(60);
const MAX_STALE: Duration = Duration::from_secs(15 * 60);
const PAGE_SIZE: usize = 10;
const MANUAL_REFRESH_COOLDOWN: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY_BYTES: u64 = 512 * 1024;
const MAX_MESSAGE_UNITS: usize = 3_500;
const MAX_NAME_CHARS: usize = 96;
const MAX_PRICE_CHARS: usize = 48;
const ERROR_MESSAGE: &str = "فعلاً دریافت نرخ ارز ممکن نیست؛ لطفاً چند دقیقه بعد دوباره تلاش کنید.";

#[derive(Clone, Debug, PartialEq, Eq)]
struct RateRow {
    name: String,
    buy: String,
    sell: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RateSnapshot {
    updated_at: Option<String>,
    rows: Vec<RateRow>,
}

struct CachedSnapshot {
    snapshot: Arc<RateSnapshot>,
    fetched_at: Instant,
}

struct ServedSnapshot {
    snapshot: Arc<RateSnapshot>,
    stale: bool,
}

static CACHE: LazyLock<Mutex<Option<CachedSnapshot>>> = LazyLock::new(|| Mutex::new(None));
static REFRESH: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
static LAST_MANUAL_REFRESH: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));
static HTTP: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(format!("groupbot/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())
});

pub async fn handle(_ctx: &Ctx, message: &Message) -> bool {
    if !is_command(message.text().trim()) {
        return false;
    }

    let body = match snapshot(false).await {
        Ok(served) => {
            let page = 0;
            let total = page_count(served.snapshot.rows.len());
            Some((
                render_page(&served.snapshot, served.stale, page),
                page,
                total,
            ))
        }
        Err(error) => {
            log::warn!("currency: AlanChand fetch failed: {error}");
            None
        }
    };

    match body {
        Some((body, page, total)) => {
            let _ = message
                .reply(
                    InputMessage::new()
                        .html(body)
                        .reply_markup(markup(page, total)),
                )
                .await;
        }
        None => {
            let _ = message.reply(ERROR_MESSAGE).await;
        }
    }
    true
}

fn is_command(text: &str) -> bool {
    COMMANDS.contains(&text)
}

async fn snapshot(force_refresh: bool) -> Result<ServedSnapshot, String> {
    if !force_refresh && let Some(snapshot) = cached_for(CACHE_TTL) {
        return Ok(ServedSnapshot {
            snapshot,
            stale: false,
        });
    }

    let _refresh = REFRESH.lock().await;
    if !force_refresh && let Some(snapshot) = cached_for(CACHE_TTL) {
        return Ok(ServedSnapshot {
            snapshot,
            stale: false,
        });
    }

    if force_refresh
        && !manual_refresh_allowed()
        && let Some((snapshot, age)) = cached_snapshot()
    {
        return Ok(ServedSnapshot {
            snapshot,
            stale: age >= CACHE_TTL,
        });
    }
    if force_refresh {
        *LAST_MANUAL_REFRESH.lock().unwrap() = Some(Instant::now());
    }

    match fetch_snapshot().await {
        Ok(snapshot) => {
            let snapshot = Arc::new(snapshot);
            *CACHE.lock().unwrap() = Some(CachedSnapshot {
                snapshot: Arc::clone(&snapshot),
                fetched_at: Instant::now(),
            });
            Ok(ServedSnapshot {
                snapshot,
                stale: false,
            })
        }
        Err(error) => {
            if let Some(snapshot) = cached_for(MAX_STALE) {
                log::warn!(
                    "currency: serving the last AlanChand snapshot after refresh failure: {error}"
                );
                Ok(ServedSnapshot {
                    snapshot,
                    stale: true,
                })
            } else {
                Err(error)
            }
        }
    }
}

fn cached_for(max_age: Duration) -> Option<Arc<RateSnapshot>> {
    cached_snapshot()
        .filter(|(_, age)| *age < max_age)
        .map(|(snapshot, _)| snapshot)
}

fn cached_snapshot() -> Option<(Arc<RateSnapshot>, Duration)> {
    CACHE
        .lock()
        .unwrap()
        .as_ref()
        .map(|cached| (Arc::clone(&cached.snapshot), cached.fetched_at.elapsed()))
}

fn manual_refresh_allowed() -> bool {
    LAST_MANUAL_REFRESH
        .lock()
        .unwrap()
        .is_none_or(|last| last.elapsed() >= MANUAL_REFRESH_COOLDOWN)
}

async fn fetch_snapshot() -> Result<RateSnapshot, String> {
    let client = HTTP.as_ref().map_err(Clone::clone)?;
    let response = client
        .get(SOURCE_URL)
        .header(reqwest::header::ACCEPT, "text/html")
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("source returned HTTP {status}"));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES)
    {
        return Err(format!("source response exceeds {MAX_BODY_BYTES} bytes"));
    }

    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("reading response failed: {error}"))?
    {
        if body.len() as u64 + chunk.len() as u64 > MAX_BODY_BYTES {
            return Err(format!("source response exceeds {MAX_BODY_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(body)
        .map_err(|error| format!("source response is not UTF-8: {error}"))?;
    parse_page(&body)
}

fn parse_page(body: &str) -> Result<RateSnapshot, String> {
    let document = Html::parse_document(body);
    let rows_selector = selector("table.CurrencyTbl tbody tr");
    let name_selector = selector("td.currName");
    let buy_selector = selector("td.buyPrice");
    let sell_selector = selector("td.sellPrice");

    let rows = document
        .select(&rows_selector)
        .filter_map(|row| {
            Some(RateRow {
                name: cell_text(&row, &name_selector)?,
                buy: cell_text(&row, &buy_selector)?,
                sell: cell_text(&row, &sell_selector)?,
            })
        })
        .collect::<Vec<_>>();

    if rows.is_empty() {
        return Err("source contained no complete currency rows".to_owned());
    }

    let update_selector = selector("section.container-sm p.text-center");
    let updated_at = document
        .select(&update_selector)
        .map(|element| collapse_text(element.text()))
        .find(|text| !text.is_empty());

    Ok(RateSnapshot { updated_at, rows })
}

fn selector(query: &str) -> Selector {
    Selector::parse(query).expect("currency selectors are valid CSS")
}

fn cell_text(row: &ElementRef<'_>, selector: &Selector) -> Option<String> {
    row.select(selector)
        .next()
        .map(|element| collapse_text(element.text()))
        .filter(|text| !text.is_empty())
}

fn collapse_text<I>(parts: I) -> String
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut result = String::new();
    for part in parts {
        for word in part.as_ref().split_whitespace() {
            if !result.is_empty() {
                result.push(' ');
            }
            result.push_str(word);
        }
    }
    result
}

pub async fn on_callback(_ctx: &Ctx, query: &CallbackQuery, payload: &str) {
    let Some((action, page)) = payload.split_once(':') else {
        let _ = query.answer().send().await;
        return;
    };
    let Ok(page) = page.parse::<usize>() else {
        let _ = query.answer().send().await;
        return;
    };

    let served = match action {
        "p" => snapshot(false).await,
        "r" => snapshot(true).await,
        _ => {
            let _ = query.answer().send().await;
            return;
        }
    };
    let served = match served {
        Ok(served) => served,
        Err(error) => {
            log::warn!("currency: AlanChand callback refresh failed: {error}");
            let _ = query.answer().alert(ERROR_MESSAGE).send().await;
            return;
        }
    };

    let total = page_count(served.snapshot.rows.len());
    let page = page.min(total.saturating_sub(1));
    let body = render_page(&served.snapshot, served.stale, page);
    let _ = query
        .answer()
        .edit(
            InputMessage::new()
                .html(body)
                .reply_markup(markup(page, total)),
        )
        .await;
}

fn page_count(rows: usize) -> usize {
    rows.div_ceil(PAGE_SIZE).max(1)
}

fn markup(page: usize, total: usize) -> ReplyMarkup {
    let mut navigation = Vec::new();
    if page > 0 {
        navigation.push(super::style::data(
            "‹ قبلی",
            format!("fx:p:{}", page - 1),
            super::style::Colour::Primary,
        ));
    }
    if page + 1 < total {
        navigation.push(super::style::data(
            "بعدی ›",
            format!("fx:p:{}", page + 1),
            super::style::Colour::Primary,
        ));
    }

    let mut rows = Vec::new();
    if !navigation.is_empty() {
        rows.push(navigation);
    }
    rows.push(vec![super::style::data(
        "🔄 بروزرسانی",
        format!("fx:r:{page}").into_bytes(),
        super::style::Colour::Success,
    )]);
    ReplyMarkup::from_buttons(&rows)
}

fn render_page(snapshot: &RateSnapshot, stale: bool, page: usize) -> String {
    let total = page_count(snapshot.rows.len());
    let page = page.min(total.saturating_sub(1));
    let start = page * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(snapshot.rows.len());

    let mut out = String::from("💱 <b>نرخ ارز بازار آزاد</b>\n");
    if stale {
        out.push_str("⚠️ <i>بروزرسانی جدید دریافت نشد؛ آخرین داده موفق نمایش داده خواهد شد.</i>\n");
    }
    let updated = snapshot
        .updated_at
        .as_deref()
        .map(update_value)
        .unwrap_or_else(|| "زمان بروزرسانی در منبع اعلام نشده".to_owned());
    out.push_str(&format!("<i>آخرین بروزرسانی: {updated}</i>\n"));
    out.push_str(&format!(
        "<b>صفحه {} از {}</b>\n\n",
        fa_number(page + 1),
        fa_number(total)
    ));

    for (index, row) in snapshot.rows[start..end].iter().enumerate() {
        out.push_str(&format!(
            "🔹 <b>{}</b>\nخرید: <code>{}</code> تومان\nفروش: <code>{}</code> تومان\n",
            display_cell(&row.name, MAX_NAME_CHARS),
            display_cell(&row.buy, MAX_PRICE_CHARS),
            display_cell(&row.sell, MAX_PRICE_CHARS),
        ));
        if index + 1 < end - start {
            out.push('\n');
        }
    }

    out.push_str(&format!("\n<a href=\"{SOURCE_URL}\">منبع: AlanChand</a>"));
    debug_assert!(message_width(&out) <= MAX_MESSAGE_UNITS);
    out
}

fn update_value(value: &str) -> String {
    let value = value
        .strip_prefix("آخرین بروز رسانی :")
        .or_else(|| value.strip_prefix("آخرین بروزرسانی :"))
        .unwrap_or(value)
        .trim();
    super::esc(value)
}

fn display_cell(value: &str, max_chars: usize) -> String {
    let mut clipped = value.chars().take(max_chars).collect::<String>();
    if value.chars().nth(max_chars).is_some() {
        clipped.push('…');
    }
    super::esc(&clipped)
}

fn fa_number(number: usize) -> String {
    number
        .to_string()
        .chars()
        .map(|digit| match digit {
            '0'..='9' => char::from_u32('۰' as u32 + (digit as u32 - '0' as u32)).unwrap_or(digit),
            other => other,
        })
        .collect()
}

fn message_width(text: &str) -> usize {
    text.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
        <section class="container-sm">
            <p class="text-center">آخرین بروز رسانی : ۲۱:۱۰ سه شنبه ۱۰ شهریور ۱۴۰۵</p>
        </section>
        <table class="CurrencyTbl"><tbody>
            <tr>
                <td class="currName">دلار آمریکا</td>
                <td class="buyPrice">۲۱۲,۹۵۰</td>
                <td class="sellPrice">۲۱۵,۱۰۰<span class="priceSymbol up"></span></td>
            </tr>
            <tr>
                <td class="currName">یورو</td>
                <td class="buyPrice">۲۴۶,۸۰۰</td>
                <td class="sellPrice">۲۴۹,۳۰۰</td>
            </tr>
        </tbody></table>
    "#;

    #[test]
    fn parses_rows_and_update_time_from_the_public_table() {
        let snapshot = parse_page(PAGE).unwrap();
        assert_eq!(snapshot.rows.len(), 2);
        assert_eq!(snapshot.rows[0].name, "دلار آمریکا");
        assert_eq!(snapshot.rows[0].buy, "۲۱۲,۹۵۰");
        assert_eq!(snapshot.rows[0].sell, "۲۱۵,۱۰۰");
        assert_eq!(
            snapshot.updated_at.as_deref(),
            Some("آخرین بروز رسانی : ۲۱:۱۰ سه شنبه ۱۰ شهریور ۱۴۰۵")
        );
    }

    #[test]
    fn rejects_a_page_without_complete_rows() {
        assert!(parse_page("<table class=\"CurrencyTbl\"><tbody></tbody></table>").is_err());
        assert!(parse_page(
            "<table class=\"CurrencyTbl\"><tbody><tr><td class=\"currName\">دلار</td></tr></tbody></table>"
        )
        .is_err());
    }

    #[test]
    fn commands_are_exact_after_trimming() {
        assert!(is_command("نرخ ارز"));
        assert!(is_command(" قیمت ارز ".trim()));
        assert!(!is_command("نرخ ارز امروز"));
        assert!(!is_command("ارز"));
    }

    #[test]
    fn render_escapes_source_text_and_marks_stale_data() {
        let snapshot = RateSnapshot {
            updated_at: Some("آخرین بروزرسانی : <امروز> & اکنون".to_owned()),
            rows: vec![RateRow {
                name: "A & <B>".to_owned(),
                buy: "۱۰۰".to_owned(),
                sell: "۲۰۰".to_owned(),
            }],
        };
        let rendered = render_page(&snapshot, true, 0);
        assert!(rendered.contains("A &amp; &lt;B&gt;"));
        assert!(rendered.contains("آخرین بروزرسانی: &lt;امروز&gt; &amp; اکنون"));
        assert!(rendered.contains("آخرین داده موفق"));
        assert!(rendered.contains("خرید: <code>۱۰۰</code> تومان"));
        assert!(rendered.contains("فروش: <code>۲۰۰</code> تومان"));
        assert!(!rendered.contains(" | "));
        assert!(!rendered.contains("A & <B>"));
    }

    #[test]
    fn render_paginates_in_source_order_and_respects_telegram_room() {
        let snapshot = RateSnapshot {
            updated_at: None,
            rows: (0..25)
                .map(|index| RateRow {
                    name: format!("ارز شماره {index}"),
                    buy: "۱۲۳,۴۵۶".to_owned(),
                    sell: "۱۲۳,۴۵۷".to_owned(),
                })
                .collect(),
        };
        assert_eq!(page_count(snapshot.rows.len()), 3);
        let first = render_page(&snapshot, false, 0);
        let middle = render_page(&snapshot, false, 1);
        let last = render_page(&snapshot, false, 2);
        assert!(first.contains("صفحه ۱ از ۳"));
        assert!(first.contains("ارز شماره 0"));
        assert!(first.contains("ارز شماره 9"));
        assert!(!first.contains("ارز شماره 10"));
        assert!(middle.contains("ارز شماره 10"));
        assert!(middle.contains("ارز شماره 19"));
        assert!(!middle.contains("ارز شماره 20"));
        assert!(last.contains("ارز شماره 20"));
        assert!(last.contains("ارز شماره 24"));
        assert!(!last.contains("ارز شماره 19"));
        assert!(message_width(&first) <= MAX_MESSAGE_UNITS);
        assert!(message_width(&middle) <= MAX_MESSAGE_UNITS);
        assert!(message_width(&last) <= MAX_MESSAGE_UNITS);
    }

    #[test]
    fn markup_has_navigation_at_the_correct_boundaries() {
        fn payloads(markup: &ReplyMarkup) -> Vec<String> {
            use grammers_client::tl;

            let tl::enums::ReplyMarkup::ReplyInlineMarkup(inline) = &markup.raw else {
                return Vec::new();
            };
            inline
                .rows
                .iter()
                .flat_map(|row| match row {
                    tl::enums::KeyboardButtonRow::Row(row) => row.buttons.iter(),
                })
                .filter_map(|button| match button {
                    tl::enums::KeyboardButton::Callback(button) => {
                        Some(String::from_utf8_lossy(&button.data).into_owned())
                    }
                    _ => None,
                })
                .collect()
        }

        let first = payloads(&markup(0, 3));
        let middle = payloads(&markup(1, 3));
        let last = payloads(&markup(2, 3));
        assert!(first.contains(&"fx:p:1".to_owned()));
        assert!(!first.contains(&"fx:p:18446744073709551615".to_owned()));
        assert!(middle.contains(&"fx:p:0".to_owned()));
        assert!(middle.contains(&"fx:p:2".to_owned()));
        assert!(last.contains(&"fx:p:1".to_owned()));
        assert!(!last.contains(&"fx:p:2".to_owned()));
        assert!(first.contains(&"fx:r:0".to_owned()));
        assert!(middle.contains(&"fx:r:1".to_owned()));
        assert!(last.contains(&"fx:r:2".to_owned()));
    }

    #[test]
    fn display_cells_are_clipped_before_html_escaping() {
        let name = "<".repeat(MAX_NAME_CHARS + 10);
        let snapshot = RateSnapshot {
            updated_at: None,
            rows: vec![RateRow {
                name,
                buy: "<".repeat(MAX_PRICE_CHARS + 10),
                sell: "۲۰۰".to_owned(),
            }],
        };
        let rendered = render_page(&snapshot, false, 0);
        assert!(message_width(&rendered) <= MAX_MESSAGE_UNITS);
        assert!(rendered.contains("&lt;&lt;"));
    }
}
