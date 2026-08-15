use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::update::CallbackQuery;

use super::{Ctx};

pub const ALL: &[&str] = &["حذف همه", "پاکسازی همه", "حذف کل پیام ها"];

pub const AUTO_AT: &str = "auto_purge_at";

pub const AUTO_COUNT: &str = "auto_purge_count";
pub const AUTO_AT_PRESETS: &[u32] = &[0, 3 * 60, 6 * 60, 12 * 60, 21 * 60, 23 * 60];

pub const AUTO_COUNT_PRESETS: &[u32] = &[100, 500, 1_000, 5_000, 20_000, 0];
pub const AUTO_COUNT_RANGE: (u32, u32) = (0, 100_000);
pub const AUTO_DEFAULT_AT: u32 = 4 * 60;
pub const AUTO_DEFAULT_COUNT: u32 = 1_000;

pub const COMMANDS: &[&str] = &["حذف", "پاکسازی", "پاک کن"];

const MAX: i32 = 1000;

const CHUNK: usize = 100;

pub fn auto_at(ctx: &Ctx, chat: i64) -> Option<u32> {
    ctx.settings
        .value_parsed::<u32>(chat, AUTO_AT)
        .filter(|at| *at < 1440)
}

pub fn auto_count(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value_parsed(chat, AUTO_COUNT)
        .unwrap_or(AUTO_DEFAULT_COUNT)
}

pub async fn set_auto_at(ctx: &Ctx, chat: i64, at: Option<u32>) {
    match at {
        Some(at) => {
            ctx.settings
                .set_value(chat, AUTO_AT, &(at % 1440).to_string())
                .await
        }
        None => ctx.settings.set_value(chat, AUTO_AT, "").await,
    }
}

pub async fn run_auto(ctx: &std::sync::Arc<Ctx>) {
    let now = ((super::stats::local_seconds() % 86_400) / 60) as u32;
    let day = super::stats::today();
    for chat in ctx.settings.chats() {
        if !auto_at(ctx, chat).is_some_and(|at| (0..=2).contains(&now.wrapping_sub(at))) {
            continue;
        }
        if ctx.settings.value_parsed::<u64>(chat, "auto_purge_day") == Some(day) {
            continue;
        }
        ctx.settings
            .set_value(chat, "auto_purge_day", &day.to_string())
            .await;
        let Some(chat_ref) = ctx.chat_ref(chat) else {
            continue;
        };

        let ctx = std::sync::Arc::clone(ctx);
        tokio::spawn(async move {
        let ctx = &*ctx;

        let Ok(marker) = ctx
            .client
            .send_message(
                chat_ref,
                InputMessage::new().html("<b>پاکسازی خودکار</b>\n\nدر حال پاک کردن..."),
            )
            .await
        else {
            return;
        };
        let last = marker.id() - 1;
        let count = auto_count(ctx, chat);
        let done = match count {
            0 => match super::cleaner::purge_history(ctx, chat, last).await {
                Ok(deleted) => format!("{deleted} پیام پاک شد."),
                Err(e) => format!("انجام نشد · {e}"),
            },
            count => {
                let first = (last - count as i32).max(1);
                format!(
                    "{} پیام پاک شد.",
                    wipe_range(ctx, chat, chat_ref, first, last).await
                )
            }
        };
        let _ = marker
            .edit(InputMessage::new().html(format!("<b>پاکسازی خودکار</b>\n\n{done}")))
            .await;
        });
    }
}

pub async fn handle_all(ctx: &Ctx, message: &Message) -> bool {
    if !ALL.contains(&message.text().trim()) {
        return false;
    }
    if !super::limits::allows(ctx, message, super::limits::CLEAN).await {
        return true;
    }
    let (Some(opener), Some(_)) = (
        message
            .sender_id()
            .and_then(grammers_client::session::types::PeerId::bare_id),
        message.peer_id().bot_api_dialog_id(),
    ) else {
        return false;
    };
    let last = message.id();
    let warning = match ctx.user_client().is_some() {
        true => "همه پیام های گروه برای همه پاک می شود. این کار برگشت ندارد.",

        false => "کلینر وارد نشده است؛ بدون آن هر بار تا ۱۰ هزار پیام آخر پاک می شود.\n\
                  برای پاک شدن کامل «افزودن کلینر» را بفرستید.",
    };
    let _ = message
        .reply(
            InputMessage::new()
                .html(format!("<b>حذف همه پیام ها</b>\n\n{warning}"))
                .reply_markup(ReplyMarkup::from_buttons(&[vec![
                    super::style::data(
                        "✅  تایید",
                        format!("pg:{opener}:{last}").into_bytes(),
                        super::style::Colour::Success,
                    ),
                    Button::data("❌  لغو", format!("pg:{opener}:0").into_bytes()),
                ]])),
        )
        .await;
    true
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str) {
    let Some((opener, last)) = payload.split_once(':') else {
        return;
    };
    let (Ok(opener), Ok(last)) = (opener.parse::<i64>(), last.parse::<i32>()) else {
        return;
    };
    if query.sender_id().bare_id() != Some(opener) {
        let _ = query
            .answer()
            .alert("این دکمه برای شخص دیگری است.")
            .send()
            .await;
        return;
    }
    if last == 0 {
        let _ = query
            .answer()
            .edit(InputMessage::new().html("✗ لغو شد."))
            .await;
        return;
    }
    let Some(chat) = query.peer_id().bot_api_dialog_id() else {
        return;
    };
    let _ = query.answer().send().await;

    let text = match super::cleaner::purge_history(ctx, chat, last).await {
        Ok(deleted) => format!(
            "✓ {deleted} پیام پاک شد.\n\
             <i>اگر گروه پیام قدیمی تر هم دارد، دوباره «حذف همه» را بفرستید.</i>"
        ),
        Err(e) => {
            eprintln!("purge all: {chat}: {e}");
            format!("انجام نشد · {e}")
        }
    };
    if let Some(chat_ref) = ctx.chat_ref(chat) {
        let _ = ctx
            .client
            .send_message(chat_ref, InputMessage::new().html(text))
            .await;
    }
}

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let Some(count) = parse(view.digits()) else {
        return false;
    };

    if !super::limits::allows(ctx, message, super::limits::CLEAN).await {
        return true;
    }
    let (Ok(Some(chat_ref)), Some(chat)) = (
        message.peer_ref().await,
        message.peer_id().bot_api_dialog_id(),
    ) else {
        return false;
    };

    let last = message.id();
    let first = (last - count).max(1);
    let deleted = wipe_range(ctx, chat, chat_ref, first, last).await;

    let _ = message
        .respond(format!("✓ {deleted} پیام حذف شد."))
        .await;
    true
}

async fn wipe_range(
    ctx: &Ctx,
    chat: i64,
    chat_ref: grammers_client::session::types::PeerRef,
    first: i32,
    last: i32,
) -> usize {
    if let Some(deleted) = super::cleaner::purge(ctx, chat, first, last).await {
        return deleted;
    }
    let ids: Vec<i32> = (first..=last).collect();
    let mut deleted = 0;
    for chunk in ids.chunks(CHUNK) {
        match ctx.client.delete_messages(chat_ref, chunk).await {
            Ok(n) => deleted += n,
            Err(e) => {
                eprintln!("purge: {chat}: could not delete messages: {e}");
                break;
            }
        }
    }
    deleted
}

fn parse(text: &str) -> Option<i32> {
    for command in COMMANDS {
        let Some(rest) = text.strip_prefix(command) else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim();
        if let Ok(count) = rest.parse::<i32>()
            && count > 0
        {
            return Some(count.min(MAX));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_counts() {
        assert_eq!(parse("حذف 99"), Some(99));
        assert_eq!(parse("پاکسازی 10"), Some(10));
        assert_eq!(parse("حذف 999"), Some(999));
        assert_eq!(parse("حذف 100000"), Some(MAX));

        assert_eq!(parse("حذف فیلتر تبلیغ"), None);
        assert_eq!(parse("حذف سکوت"), None);
        assert_eq!(parse("حذف ویژه 12345"), None);
        assert_eq!(parse("حذف 0"), None);
        assert_eq!(parse("حذف"), None);
    }

    #[test]
    fn the_count_must_be_the_whole_tail() {
        let word = "میکنمت";

        assert_eq!(parse(&format!("حذف {word}")), None);
        assert_eq!(parse(&format!("حذف {word} 50")), None);
        assert_eq!(parse(&format!("حذف 50 {word}")), None);
        assert_eq!(parse(&format!("حذف{word}")), None);

        assert_eq!(parse("حذف 50"), Some(50));
    }
}
