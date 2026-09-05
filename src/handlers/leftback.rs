use grammers_client::message::{Button, InputMessage, ReplyMarkup};
use grammers_client::session::types::{PeerAuth, PeerId, PeerRef};
use grammers_client::tl;

use super::{Ctx, esc};

pub const MODE: &str = "left_back";
const INVITE_KEY: &str = "left_back_invite";

async fn invite(ctx: &Ctx, chat: i64, chat_ref: PeerRef) -> Option<String> {
    if let Some(link) = ctx
        .settings
        .value(chat, INVITE_KEY)
        .filter(|link| link.starts_with("https://t.me/"))
    {
        return Some(link);
    }

    let exported = match ctx
        .client
        .invoke_outbound(&tl::functions::messages::ExportChatInvite {
            legacy_revoke_permanent: false,
            request_needed: false,
            peer: chat_ref.into(),
            expire_date: None,

            usage_limit: None,
            title: Some("بازگشت اعضا".to_owned()),
            subscription_pricing: None,
        })
        .await
    {
        Ok(exported) => exported,
        Err(e) => {
            eprintln!("left back: could not create invite for {chat}: {e}");
            return None;
        }
    };
    let tl::enums::ExportedChatInvite::ChatInviteExported(invite) = exported else {
        return None;
    };
    let link = invite.link;
    if !link.starts_with("https://t.me/") {
        return None;
    }
    let _ = ctx.settings.set_value(chat, INVITE_KEY, &link).await;
    Some(link)
}

pub async fn on_participant_update(ctx: &Ctx, update: &tl::types::UpdateChannelParticipant) {
    if update.prev_participant.is_none() || update.new_participant.is_some() {
        return;
    }

    let Some(chat) = PeerId::channel(update.channel_id).and_then(|id| id.bot_api_dialog_id())
    else {
        return;
    };
    if !ctx.settings.is_locked(chat, MODE) {
        return;
    }

    let Some(user) = PeerId::user(update.user_id) else {
        return;
    };
    if ctx.me_id() == update.user_id {
        return;
    }
    let Some(access_hash) = ctx.settings.started_user(update.user_id).await else {
        return;
    };
    let Some(chat_ref) = ctx
        .chat_ref(chat)
        .or_else(|| PeerId::channel(update.channel_id).map(PeerId::to_ambient_ref))
    else {
        return;
    };

    let Some(link) = invite(ctx, chat, chat_ref).await else {
        return;
    };

    let target = PeerRef {
        id: user,
        auth: PeerAuth::from_hash(access_hash),
    };
    let message = InputMessage::new()
        .html(format!(
            "شما از گروه خارج شدید. اگر می خواهید دوباره به گروه برگردید، از لینک زیر وارد شوید:\n\n{}",
            esc(&link)
        ))
        .reply_markup(ReplyMarkup::from_buttons(&[vec![Button::url(
            "➕ عضویت دوباره در گروه",
            link,
        )]]));
    if let Err(e) = ctx.client.send_message(target, message).await {
        eprintln!("left back: could not message {user} about {chat}: {e}");
    }
}
