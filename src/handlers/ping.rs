use std::time::{Duration, Instant};

use grammers_client::message::{InputMessage, Message};

use super::Ctx;

const COMMANDS: &[&str] = &["پینگ", "ping", "وضعیت ربات", "سرعت"];

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    if !COMMANDS.contains(&message.text().trim()) {
        return false;
    }

    let started = Instant::now();
    let Ok(sent) = message.reply("در حال اندازه گیری…").await else {
        return true;
    };
    let telegram = started.elapsed();
    let database = ctx.settings.ping().await;

    let _ = sent
        .edit(InputMessage::new().html(format!(
            "<b>وضعیت</b>\n\n\
             تلگرام · <b>{}</b>\n\
             دیتابیس · <b>{}</b>\n\
             روشن از · <b>{}</b>",
            millis(telegram),
            database.map_or("در دسترس نیست".to_owned(), millis),
            uptime(ctx.started.elapsed()),
        )))
        .await;
    true
}

fn millis(duration: Duration) -> String {
    format!("{} میلی ثانیه", duration.as_millis())
}

fn uptime(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let (days, hours, minutes) = (seconds / 86_400, (seconds % 86_400) / 3600, (seconds % 3600) / 60);
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{seconds} ثانیه"),
        (0, 0, m) => format!("{m} دقیقه"),
        (0, h, m) => format!("{h} ساعت و {m} دقیقه"),
        (d, h, _) => format!("{d} روز و {h} ساعت"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_uptime() {
        assert_eq!(uptime(Duration::from_secs(45)), "45 ثانیه");
        assert_eq!(uptime(Duration::from_secs(600)), "10 دقیقه");
        assert_eq!(uptime(Duration::from_secs(7_800)), "2 ساعت و 10 دقیقه");
        assert_eq!(uptime(Duration::from_secs(180_000)), "2 روز و 2 ساعت");
    }
}
