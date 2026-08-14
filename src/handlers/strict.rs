use grammers_client::message::Message;

use super::restrict::{self, Action};
use super::{Ctx, esc, name_of};

pub const MODE: &str = "strict";

pub const ACTION: &str = "strict_action";

pub fn action_of(ctx: &Ctx, chat: i64) -> Action {
    match ctx.settings.value(chat, ACTION).as_deref() {
        Some("ban") => Action::Ban,
        _ => Action::Mute,
    }
}

pub fn is_ban(ctx: &Ctx, chat: i64) -> bool {
    action_of(ctx, chat) == Action::Ban
}

pub async fn punish(ctx: &Ctx, message: &Message, chat: i64) {
    if !ctx.settings.is_locked(chat, MODE) {
        return;
    }
    let (Ok(Some(chat_ref)), Ok(Some(target))) = (message.peer_ref().await, message.sender_ref().await)
    else {
        return;
    };

    let action = action_of(ctx, chat);
    if let Err(e) = restrict::apply(ctx, chat_ref, target, action, None, super::restrict::By { reason: "حالت سختگیرانه", target_name: &super::name_of(message), ..Default::default() }).await {
        eprintln!("strict mode: {chat}: could not restrict sender: {e}");
        return;
    }

    let what = if action == Action::Ban {
        "از گروه اخراج شد"
    } else {
        "سکوت شد"
    };
    let _ = ctx
        .client
        .send_message(
            chat_ref,
            grammers_client::message::InputMessage::new().html(format!(
                "<b>{}</b> به دلیل ارسال مورد قفل شده {what}.",
                esc(&name_of(message))
            )),
        )
        .await;
}
