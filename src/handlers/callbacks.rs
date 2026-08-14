use grammers_client::update::CallbackQuery;

use super::{Ctx, chat_admins, is_bot_admin, owner};

pub async fn handle(ctx: &Ctx, query: &CallbackQuery) {
    let Ok(data) = std::str::from_utf8(query.data()) else {
        return;
    };
    let Some(here) = query.peer_id().bot_api_dialog_id() else {
        return;
    };

    let chat = data
        .strip_prefix("p:")
        .and_then(|rest| rest.split(':').nth(1))
        .and_then(|chat| chat.parse::<i64>().ok())
        .unwrap_or(here);

    if let Some(payload) = data.strip_prefix("c:") {
        super::captcha::on_callback(ctx, query, payload, chat).await;
        return;
    }

    if let Some(payload) = data.strip_prefix("j:") {
        super::join::on_callback(ctx, query, payload).await;
        return;
    }

    if let Some(payload) = data.strip_prefix("f:") {
        let is_admin = presser_can_manage(ctx, query, chat).await;
        super::filters::on_callback(ctx, query, payload, is_admin).await;
        return;
    }

    if !presser_can_manage(ctx, query, chat).await {
        let _ = query
            .answer()
            .alert("فقط ادمین ها می توانند این دکمه را بزنند.")
            .send()
            .await;
        return;
    }

    if let Some(payload) = data.strip_prefix("jx:") {
        super::join::on_exempt(ctx, query, payload).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("pg:") {
        super::purge::on_callback(ctx, query, payload).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("s:") {
        super::stats::on_callback(ctx, query, payload).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("r:") {
        super::report::on_callback(ctx, query, payload, chat).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("a:") {
        super::promote::on_callback(ctx, query, payload, chat).await;
        return;
    }
    if let Some(action) = data.strip_prefix("p:") {
        super::panel::on_callback(ctx, query, action).await;
        return;
    }
    if let Some(action) = data.strip_prefix("t:") {
        super::toggles::on_callback(ctx, query, action, chat).await;
    }
}

async fn presser_can_manage(ctx: &Ctx, query: &CallbackQuery, chat: i64) -> bool {
    let Some(presser) = query.sender_id().bare_id() else {
        return false;
    };
    if owner(ctx, chat) == Some(presser) || is_bot_admin(ctx, chat, presser) {
        return true;
    }

    let chat_ref = match ctx.chat_ref(chat) {
        Some(peer) => peer,
        None => match query.peer_ref().await {
            Ok(Some(peer)) => peer,
            _ => return false,
        },
    };
    if let Some(admins) = chat_admins(ctx, chat_ref, chat).await {
        return admins.contains(&presser);
    }

    let Ok(Some(sender)) = query.sender_ref().await else {
        return false;
    };
    ctx.client
        .get_permissions(chat_ref, sender)
        .await
        .is_ok_and(|p| p.is_admin())
}
