use grammers_client::tl;
use grammers_client::message::Message;

use super::{Ctx, name_of};

pub const LOCK: &str = "bot";

pub const KICK_ADDER: &str = "bot_kick_adder";

pub const EVEN_ADMINS: &str = "bot_even_admins";

pub async fn handle(ctx: &std::sync::Arc<Ctx>, message: &Message) -> bool {
    let joined_by_link = matches!(
        message.action(),
        Some(tl::enums::MessageAction::ChatJoinedByLink(_))
    );
    if !joined_by_link && !matches!(message.action(), Some(tl::enums::MessageAction::ChatAddUser(_)))
    {
        return false;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !ctx.settings.is_locked(chat, LOCK) {
        return false;
    }

    if !ctx.settings.is_locked(chat, EVEN_ADMINS) && super::is_exempt(ctx, message).await {
        return false;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    let bots: Vec<super::Joined> = super::joined_users(ctx, message)
        .await
        .into_iter()
        .filter(|joined| joined.is_bot)
        .collect();
    let mut kicked = Vec::new();
    for peer in bots.iter().map(|joined| joined.peer) {
        match ctx.client.kick_participant(chat_ref, peer).await {
            Ok(()) => kicked.push(peer.id),
            Err(e) => eprintln!("bot lock: {chat}: could not kick bot {}: {e}", peer.id),
        }
    }
    if kicked.is_empty() {
        return false;
    }

    let adder = name_of(message);
    let removed_adder = if ctx.settings.is_locked(chat, KICK_ADDER) {
        match message.sender_ref().await {
            Ok(Some(sender)) => match ctx.client.kick_participant(chat_ref, sender).await {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("bot lock: {chat}: could not kick adder: {e}");
                    false
                }
            },
            _ => false,
        }
    } else {
        false
    };

    let _ = message
        .respond(if removed_adder {
            format!("✗ افزودن ربات ممنوع است. ربات حذف شد و {adder} از گروه اخراج شد.")
        } else {
            format!("✗ افزودن ربات ممنوع است. ربات حذف شد. ({adder})")
        })
        .await;
    true
}
