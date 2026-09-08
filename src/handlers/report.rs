use grammers_client::message::{Button, Message};
use grammers_client::session::types::PeerId;
use grammers_client::update::CallbackQuery;

use super::{Ctx, esc, name_of};
use crate::response::ResponseKind;

pub const COMMANDS: &[&str] = &["گزارش", "ریپورت", "report", "!report"];

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
        return false;
    };

    if !ctx.may_report(chat, user) {
        let _ = message.delete().await;
        return true;
    }

    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        let _ = message.delete().await;
        return true;
    };
    ctx.bump(chat, super::stats::REPORTED);
    let Some(admins) = super::chat_admins(ctx, chat_ref, chat).await else {
        log::warn!("report: {chat}: could not obtain a complete administrator list");
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "گزارش ثبت نشد؛ فهرست مدیران گروه در دسترس نبود. دوباره تلاش کنید.",
        )
        .await;
        let _ = message.delete().await;
        return true;
    };
    let pings: String = admins
        .iter()
        .take(10)
        .map(|id| format!("<a href=\"tg://user?id={id}\">{ANCHOR}</a>"))
        .collect();
    let Some(case) = super::cases::create_report(ctx, message, &reported).await else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "گزارش ثبت نشد؛ ذخیره سازی پرونده در دسترس نبود. دوباره تلاش کنید.",
        )
        .await;
        let _ = message.delete().await;
        return true;
    };
    let delete_data = format!("mc:d:{case}");
    let keep_data = format!("mc:k:{case}");
    let _ = message.delete().await;

    let _ = reported
        .reply(
            super::premium::icon_html(
                Some(super::premium::Icon::DocumentActivity),
                format!(
                    "‹ گزارش {} برای مدیران گروه ارسال شد.{pings}",
                    esc(&name_of(message))
                ),
            )
            .reply_markup(super::premium::buttons(&[vec![
                super::premium::decorate(
                    super::style::data(
                        "حذف پیام",
                        delete_data.into_bytes(),
                        super::style::Colour::Danger,
                    ),
                    Some(super::premium::Icon::Delete),
                ),
                super::premium::decorate(
                    Button::data("بررسی شد", keep_data.into_bytes()),
                    Some(super::premium::Icon::Success),
                ),
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

    if what == "d" {
        let Some(actor) = query.sender_id().bare_id() else {
            return;
        };
        if !super::limits::permits(ctx, chat, actor, super::limits::CLEAN) {
            super::limits::refuse(query, super::limits::CLEAN).await;
            return;
        }
    }

    let text = match what {
        "d" => {
            let Ok(Some(chat_ref)) = query.peer_ref().await else {
                return;
            };
            match ctx.client.delete_messages_critical(chat_ref, &[id]).await {
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

    let _ = query
        .answer()
        .edit(super::premium::icon_html(
            Some(super::premium::Icon::DocumentActivity),
            text,
        ))
        .await;
}
