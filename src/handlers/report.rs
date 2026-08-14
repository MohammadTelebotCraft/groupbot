use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::PeerId;
use grammers_client::update::CallbackQuery;

use super::{Ctx, esc, name_of};

const COMMANDS: &[&str] = &["گزارش", "ریپورت", "report", "!report"];

const ANCHOR: &str = "\u{2063}";

pub const EVERY: std::time::Duration = std::time::Duration::from_secs(60);

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    if !COMMANDS.contains(&message.text().trim()) {
        return false;
    }
    let (Some(chat), Some(user)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_id().and_then(PeerId::bare_id),
    ) else {
        return false;
    };
    let Ok(Some(reported)) = message.get_reply().await else {
        let _ = message
            .reply("روی پیامی که می خواهید گزارش کنید ریپلای کنید.")
            .await;
        return true;
    };

    let _ = message.delete().await;

    if !ctx.may_report(chat, user) {
        return true;
    }

    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };
    ctx.bump(chat, super::stats::REPORTED);
    let admins = super::chat_admins(ctx, chat_ref, chat)
        .await
        .unwrap_or_default();
    let pings: String = admins
        .iter()
        .take(10)
        .map(|id| format!("<a href=\"tg://user?id={id}\">{ANCHOR}</a>"))
        .collect();

    let _ = reported
        .reply(
            InputMessage::new()
                .html(format!(
                    "‹ گزارش {} برای مدیران گروه ارسال شد.{pings}",
                    esc(&name_of(message))
                ))
                .reply_markup(ReplyMarkup::from_buttons(&[vec![
                    super::style::data(
                        "حذف پیام",
                        format!("r:d:{}", reported.id()).into_bytes(),
                        super::style::Colour::Danger,
                    ),
                    Button::data("✓ بررسی شد", format!("r:k:{}", reported.id()).into_bytes()),
                ]])),
        )
        .await;
    true
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, chat: i64) {
    let Some((what, id)) = payload.split_once(':') else {
        return;
    };
    let Ok(id) = id.parse::<i32>() else {
        return;
    };
    let by = query
        .sender()
        .and_then(|peer| peer.name())
        .unwrap_or("ادمین")
        .to_owned();

    let text = match what {
        "d" => {
            let Ok(Some(chat_ref)) = query.peer_ref().await else {
                return;
            };
            match ctx.client.delete_messages(chat_ref, &[id]).await {
                Ok(_) => format!("‹ پیام گزارش شده حذف شد · {}", esc(&by)),
                Err(e) => {
                    eprintln!("report: {chat}: could not delete {id}: {e}");
                    "انجام نشد. مطمئن شوید ربات اجازه حذف پیام دارد.".to_owned()
                }
            }
        }
        "k" => format!("‹ گزارش بررسی شد · {}", esc(&by)),
        _ => return,
    };

    let _ = query.answer().edit(InputMessage::new().html(text)).await;
}
