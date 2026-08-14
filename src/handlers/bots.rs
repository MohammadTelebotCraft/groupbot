use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;

use super::{Ctx, esc, name_of};

pub const LOCK: &str = "bot";

pub const KICK_ADDER: &str = "bot_kick_adder";

pub const EVEN_ADMINS: &str = "bot_even_admins";

pub async fn on_participant_update(
    ctx: &std::sync::Arc<Ctx>,
    update: &tl::types::UpdateChannelParticipant,
) {
    use tl::enums::ChannelParticipant as P;

    let Some(chat) = PeerId::channel(update.channel_id).and_then(PeerId::bot_api_dialog_id) else {
        return;
    };
    if !ctx.settings.is_locked(chat, LOCK) {
        return;
    }

    let inside = |participant: &Option<P>| {
        matches!(
            participant,
            Some(P::Participant(_) | P::ParticipantSelf(_) | P::Admin(_) | P::Creator(_))
        )
    };
    if !inside(&update.new_participant) || inside(&update.prev_participant) {
        return;
    }

    let (user, actor) = (update.user_id, update.actor_id);
    let Some(chat_ref) = ctx
        .chat_ref(chat)
        .or_else(|| PeerId::channel(update.channel_id).map(PeerId::to_ambient_ref))
    else {
        return;
    };
    let Some(peer) = PeerId::user(user).map(PeerId::to_ambient_ref) else {
        return;
    };

    let Ok(grammers_client::peer::Peer::User(joined)) = ctx.client.resolve_peer(peer).await else {
        return;
    };
    if !joined.is_bot() {
        return;
    }

    if !ctx.settings.is_locked(chat, EVEN_ADMINS)
        && actor != user
        && super::added_by_an_admin(ctx, chat_ref, chat, actor).await
    {
        return;
    }

    if let Err(e) = ctx.client.kick_participant(chat_ref, peer).await {
        eprintln!("bot lock: {chat}: could not kick bot {user}: {e}");
        return;
    }
    let removed_adder = ctx.settings.is_locked(chat, KICK_ADDER)
        && actor != user
        && kick(ctx, chat, chat_ref, actor).await;

    let name = esc(&joined.full_name());
    let _ = ctx
        .client
        .send_message(
            chat_ref,
            InputMessage::new().html(if removed_adder {
                format!("✗ افزودن ربات ممنوع است. {name} حذف شد و اضافه کننده اش از گروه اخراج شد.")
            } else {
                format!("✗ افزودن ربات ممنوع است. {name} حذف شد.")
            }),
        )
        .await;
}

async fn kick(ctx: &Ctx, chat: i64, chat_ref: PeerRef, user: i64) -> bool {
    let Some(peer) = PeerId::user(user).map(PeerId::to_ambient_ref) else {
        return false;
    };
    match ctx.client.kick_participant(chat_ref, peer).await {
        Ok(()) => true,
        Err(e) => {
            eprintln!("bot lock: {chat}: could not kick adder {user}: {e}");
            false
        }
    }
}

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
