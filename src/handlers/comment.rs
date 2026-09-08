
use std::sync::Arc;

use grammers_client::message::Message;
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;

use super::{ChatState, Ctx, Queued, RootClaim};

pub const LOCK: &str = "comment";

pub const GROUP: &str = "cmt_group";

const REASON: &str = "کامنت";

const SIGN: &str = "کامنت قفل است کسی نمیتواند کامنت بزارد";

fn reply_of(message: &Message) -> Option<&tl::types::MessageReplyHeader> {
    let tl::enums::Message::Message(raw) = &message.raw else {
        return None;
    };
    match raw.reply_to.as_ref()? {
        tl::enums::MessageReplyHeader::Header(header) => Some(header),
        _ => None,
    }
}

fn root_of(header: &tl::types::MessageReplyHeader) -> Option<i32> {
    if header.forum_topic || header.reply_to_peer_id.is_some() {
        return None;
    }
    header.reply_to_top_id.or(header.reply_to_msg_id)
}

pub async fn on_post(ctx: &Ctx, state: &ChatState, message: &Message, chat: i64) {
    if !super::is_linked_post(message) {
        return;
    }
    state.remember_post(message.id());
    if !ctx.settings.is_locked(chat, GROUP)
        && let Err(error) = ctx.settings.try_set(chat, GROUP, true).await
    {
        ::log::warn!("comments: could not remember linked group {chat}: {error}");
    }
    if !ctx.settings.is_locked(chat, LOCK) {
        return;
    }
    if !ctx.claim_comment_sign(chat, message.id()) {
        return;
    }
    if let Err(e) = message.reply(SIGN).await {
        eprintln!("comment: {chat}: could not post the sign: {e}");
    }
}

pub async fn tripped(ctx: &Arc<Ctx>, chat: i64, message: &Message) -> bool {
    if !ctx.settings.is_locked(chat, LOCK) {
        return false;
    }
    let Some(root) = reply_of(message).and_then(root_of) else {
        return false;
    };
    let Some(state) = ctx.try_state(chat) else {
        return false;
    };
    if let Some(known) = state.root_known(root) {
        return known;
    }
    if !ctx.settings.is_locked(chat, GROUP) {
        return false;
    }
    if super::is_exempt(ctx, message).await {
        return false;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    let waiting = Queued {
        message: message.id(),
        sender: message.sender_id().and_then(PeerId::bare_id),
        name: super::name_of(message),
    };
    match state.claim_root(root, waiting) {
        RootClaim::Known(known) => known,
        RootClaim::Waiting => false,
        RootClaim::Mine => {
            let ctx = Arc::clone(ctx);
            let permit = ctx.comment_slot().await;
            Arc::clone(&ctx).spawn_owned(async move {
                let _permit = permit;
                resolve(&ctx, &state, chat, chat_ref, root).await;
            });
            false
        }
    }
}

async fn resolve(ctx: &Arc<Ctx>, state: &ChatState, chat: i64, chat_ref: PeerRef, root: i32) {
    let post = match ctx.client.get_messages_by_id(chat_ref, &[root]).await {
        Ok(found) => found
            .into_iter()
            .next()
            .flatten()
            .is_some_and(|found| super::is_linked_post(&found)),
        Err(e) => {
            eprintln!("comment: {chat}: could not look up {root}: {e}");
            state.forget_root(root);
            return;
        }
    };
    let queued = state.settle_root(root, post);
    if !post {
        return;
    }
    if !ctx.settings.is_locked(chat, LOCK) {
        return;
    }
    for entry in queued {
        act(ctx, chat, chat_ref, entry).await;
    }
}

async fn act(ctx: &Arc<Ctx>, chat: i64, chat_ref: PeerRef, entry: Queued) {
    if !ctx.claim_moderation(chat, entry.message) {
        return;
    }
    match ctx
        .client
        .delete_messages_critical(chat_ref, &[entry.message])
        .await
    {
        Ok(0) => {
            eprintln!(
                "comment: delete affected nothing in {chat} msg {}",
                entry.message
            );
            return;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "comment: could not delete in {chat} msg {}: {e}",
                entry.message
            );
            return;
        }
    }
    ctx.bump(chat, super::stats::DELETED);
    super::log::write(
        ctx,
        chat,
        "log_del",
        super::log::Entry {
            title: "حذف پیام",
            target: entry.sender.map(|id| (id, entry.name.as_str())),
            reason: Some(REASON),
            ..Default::default()
        },
    )
    .await;
    super::nsfw::punish_and_notify(
        ctx,
        super::nsfw::DetachedModeration {
            chat,
            chat_ref,
            message_id: entry.message,
            sender: entry.sender,
            name: &entry.name,
            cause: LOCK,
            reason: REASON,
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> tl::types::MessageReplyHeader {
        tl::types::MessageReplyHeader {
            reply_to_scheduled: false,
            forum_topic: false,
            quote: false,
            reply_to_ephemeral: false,
            reply_to_msg_id: None,
            reply_to_peer_id: None,
            reply_from: None,
            reply_media: None,
            reply_to_top_id: None,
            quote_text: None,
            quote_entities: None,
            quote_offset: None,
            todo_item_id: None,
            poll_option: None,
        }
    }

    #[test]
    fn the_root_is_the_top_of_the_thread_when_there_is_one() {
        let mut reply = header();
        reply.reply_to_msg_id = Some(42);
        reply.reply_to_top_id = Some(7);
        assert_eq!(root_of(&reply), Some(7));

        let mut direct = header();
        direct.reply_to_msg_id = Some(7);
        assert_eq!(root_of(&direct), Some(7));

        assert_eq!(root_of(&header()), None);
    }

    #[test]
    fn a_topic_and_a_reply_to_another_chat_name_no_root() {
        let mut topic = header();
        topic.reply_to_msg_id = Some(42);
        topic.reply_to_top_id = Some(7);
        topic.forum_topic = true;
        assert_eq!(root_of(&topic), None);

        let mut elsewhere = header();
        elsewhere.reply_to_msg_id = Some(7);
        elsewhere.reply_to_peer_id = Some(tl::types::PeerChannel { channel_id: 123 }.into());
        assert_eq!(root_of(&elsewhere), None);
    }

    #[test]
    fn the_sign_carries_no_zero_width_non_joiner() {
        assert!(!SIGN.contains('\u{200c}'));
        assert!(!SIGN.is_empty());
    }
}
