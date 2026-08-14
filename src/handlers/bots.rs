use grammers_client::message::{InputMessage, Message};
use grammers_client::peer::Role;
use grammers_client::session::types::{PeerId, PeerKind, PeerRef};
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

    let full_name = joined.full_name();
    if !remove(ctx, chat, chat_ref, peer, &full_name).await {
        return;
    }
    super::cleaner::wipe_user(ctx, chat, user).await;

    let removed_adder = ctx.settings.is_locked(chat, KICK_ADDER)
        && actor != user
        && kick(ctx, chat, chat_ref, actor).await;

    let name = esc(&full_name);
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

async fn remove(ctx: &Ctx, chat: i64, chat_ref: PeerRef, target: PeerRef, name: &str) -> bool {
    let done = super::restrict::apply(
        ctx,
        chat_ref,
        target,
        super::restrict::Action::Ban,
        None,
        super::restrict::By {
            reason: "قفل ربات",
            target_name: name,
            ..Default::default()
        },
    )
    .await;
    match done {
        Ok(()) => true,
        Err(e) => {
            eprintln!("bot lock: {chat}: could not ban bot {}: {e}", target.id);
            false
        }
    }
}

pub async fn on_lock_set(ctx: &Ctx, chat: i64, key: &str, on: bool) {
    if key == LOCK && on {
        sweep(ctx, chat).await;
    }
}

pub fn cleaner_candidate(ctx: &Ctx, message: &Message) -> bool {
    if message.outgoing() || message.peer_id().kind() != PeerKind::Channel {
        return false;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    if !ctx.settings.is_locked(chat, LOCK) {
        return false;
    }
    if !super::bot_authored(message) {
        return false;
    }

    match message.sender_id().and_then(PeerId::bare_id) {
        Some(user) => user != ctx.me_id() && !ctx.is_cleaner(user),
        None => false,
    }
}

pub async fn on_cleaner_message(ctx: &Ctx, message: &Message) {
    if !cleaner_candidate(ctx, message) {
        return;
    }
    let (Some(chat), Some(user)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_id().and_then(PeerId::bare_id),
    ) else {
        return;
    };

    if !ctx.settings.is_locked(chat, EVEN_ADMINS) && super::is_exempt(ctx, message).await {
        return;
    }

    let _ = message.delete().await;

    let Some(chat_ref) = ctx.chat_ref(chat) else {
        eprintln!("bot lock: {chat}: saw a bot post but have no ref to ban with");
        return;
    };
    let Some(target) = PeerId::user(user).map(PeerId::to_ambient_ref) else {
        return;
    };
    let name = super::name_of(message);
    if remove(ctx, chat, chat_ref, target, &name).await {
        super::cleaner::wipe_user(ctx, chat, user).await;
        println!("bot lock: {chat}: banned {user} for posting");
    }
}

fn spared_because(
    is_bot: bool,
    is_me: bool,
    is_cleaner: bool,
    is_admin: bool,
    even_admins: bool,
    vip: bool,
) -> Option<&'static str> {
    match () {
        () if !is_bot => Some("not a bot"),
        () if is_me => Some("myself"),
        () if is_cleaner => Some("the cleaner"),
        () if vip => Some("vip"),
        () if is_admin && !even_admins => Some("admin"),
        () => None,
    }
}

pub async fn sweep(ctx: &Ctx, chat: i64) -> usize {
    if !ctx.settings.is_locked(chat, LOCK) {
        return 0;
    }

    if ctx.me_id() == 0 {
        eprintln!("bot lock: {chat}: skipping the sweep, own id still unknown");
        return 0;
    }

    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return 0;
    };

    if chat_ref.id.kind() != PeerKind::Channel {
        return 0;
    }

    let even_admins = ctx.settings.is_locked(chat, EVEN_ADMINS);
    let mut participants = ctx
        .client
        .iter_participants(chat_ref)
        .filter(tl::enums::ChannelParticipantsFilter::ChannelParticipantsBots);
    let mut removed = 0;
    let mut seen = 0;

    let mut spared: Vec<String> = Vec::new();
    loop {
        match participants.next().await {
            Ok(Some(participant)) => {
                seen += 1;
                let user = participant.user.id().bare_id_unchecked();
                let is_admin = matches!(participant.role, Role::Admin(_) | Role::Creator(_));

                let why = spared_because(
                    participant.user.is_bot(),
                    user == ctx.me_id(),
                    ctx.is_cleaner(user),
                    is_admin,
                    even_admins,
                    super::vip::is_vip(ctx, chat, user),
                );
                if let Some(why) = why {
                    spared.push(format!("{user} ({why})"));
                    continue;
                }
                let Some(target) = PeerId::user(user).map(PeerId::to_ambient_ref) else {
                    spared.push(format!("{user} (no ref)"));
                    continue;
                };
                if remove(ctx, chat, chat_ref, target, &participant.user.full_name()).await {
                    super::cleaner::wipe_user(ctx, chat, user).await;
                    removed += 1;
                }
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("bot lock: {chat}: could not list bots: {e}");
                break;
            }
        }
    }
    println!(
        "bot lock: {chat}: {seen} bot(s) listed, {removed} removed{}",
        if spared.is_empty() {
            String::new()
        } else {
            format!(", spared {}", spared.join(", "))
        }
    );
    removed
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
    for joined in &bots {
        if remove(ctx, chat, chat_ref, joined.peer, &joined.name).await {
            super::cleaner::wipe_user(ctx, chat, joined.id).await;
            kicked.push(joined.id);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_never_takes_its_own_side() {
        let plain = spared_because(true, false, false, false, false, false);
        assert_eq!(plain, None, "a plain bot goes");

        assert_eq!(spared_because(false, false, false, false, false, false), Some("not a bot"));
        assert_eq!(spared_because(true, true, false, false, false, false), Some("myself"));
        assert_eq!(spared_because(true, false, true, false, false, false), Some("the cleaner"));
        assert_eq!(spared_because(true, false, false, false, false, true), Some("vip"));
        assert_eq!(
            spared_because(true, false, false, true, false, false),
            Some("admin"),
            "an admin bot is spared unless the lock says otherwise"
        );
        assert_eq!(
            spared_because(true, false, false, true, true, false),
            None,
            "with «اعمال روی ادمین ها هم» an admin bot goes too"
        );

        assert_eq!(spared_because(true, true, false, true, true, false), Some("myself"));
    }
}
