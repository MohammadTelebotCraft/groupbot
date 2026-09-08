use grammers_client::message::Message;
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;

use super::Ctx;
use crate::response::ResponseKind;
use crate::state::{SettingMutation, SettingsWriteError};

pub const LOCK: &str = "pin_lock";

pub const KEPT: &str = "pin_kept";

pub const NAMES: &[&str] = &["سنجاق", "پین"];

pub fn kept(ctx: &Ctx, chat: i64) -> Option<i32> {
    ctx.settings.value_parsed(chat, KEPT)
}

pub async fn set(ctx: &Ctx, message: &Message, chat: i64, on: bool) -> bool {
    if !super::is_owner(ctx, message) && !super::sender_is_creator(ctx, message).await {
        super::respond(
            ctx,
            message,
            ResponseKind::PermissionDenied,
            super::premium::icon_text(
                Some(super::premium::Icon::Locked),
                "قفل سنجاق فقط از مالک گروه پذیرفته می شود.",
            ),
        )
        .await;
        return true;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return true;
    };

    if !on {
        let response = match clear(ctx, chat).await {
            Ok(()) => {
                super::premium::icon_text(Some(super::premium::Icon::Pin), "قفل سنجاق برداشته شد.")
            }
            Err(error) => {
                ::log::warn!("pin lock: clear for {chat} failed: {error}");
                setting_failure_text(&error, "قفل سنجاق برداشته نشد؛ دوباره تلاش کنید.")
            }
        };
        super::respond(ctx, message, ResponseKind::LockManagement, response).await;
        return true;
    }

    let id = match message.reply_to_message_id() {
        Some(id) => Some(id),
        None => ctx
            .client
            .get_pinned_message(chat_ref)
            .await
            .ok()
            .flatten()
            .map(|pinned| pinned.id()),
    };
    let Some(id) = id else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "هیچ پیامی سنجاق نشده. روی پیام موردنظر ریپلای کنید یا اول آن را سنجاق کنید.",
        )
        .await;
        return true;
    };

    if let Err(e) = ctx.client.pin_message(chat_ref, id).await {
        eprintln!("pin lock: {chat}: could not pin {id}: {e}");
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "انجام نشد. مطمئن شوید ربات ادمین است و اجازه سنجاق کردن دارد.",
            ),
        )
        .await;
        return true;
    }
    let kept = id.to_string();
    if let Err(error) = ctx
        .settings
        .try_apply_batch(
            chat,
            &[
                SettingMutation::Put {
                    key: KEPT,
                    value: &kept,
                },
                SettingMutation::Put {
                    key: LOCK,
                    value: "",
                },
            ],
        )
        .await
    {
        ::log::warn!("pin lock: setup for {chat} failed after pinning {id}: {error}");
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            setting_failure_text(
                &error,
                "پیام سنجاق شد، اما قفل سنجاق ذخیره نشد؛ دوباره تلاش کنید.",
            ),
        )
        .await;
        return true;
    }
    super::respond(
        ctx,
        message,
        ResponseKind::LockManagement,
        super::premium::icon_text(
            Some(super::premium::Icon::Pin),
            "قفل سنجاق فعال شد. تا وقتی مالک بازش نکند، سنجاق همین پیام می ماند.",
        ),
    )
    .await;
    true
}

pub async fn on_raw(ctx: &Ctx, raw: &grammers_client::update::Raw) {
    let (peer, changed, pinned) = match &raw.raw {
        tl::enums::Update::PinnedChannelMessages(update) => (
            PeerId::channel(update.channel_id),
            &update.messages,
            update.pinned,
        ),

        tl::enums::Update::PinnedMessages(update) => (
            PeerId::try_from(&update.peer).ok(),
            &update.messages,
            update.pinned,
        ),
        _ => return,
    };
    let Some(chat) = peer.and_then(PeerId::bot_api_dialog_id) else {
        return;
    };
    if !ctx.settings.is_locked(chat, LOCK) {
        return;
    }
    let Some(kept) = kept(ctx, chat) else {
        return;
    };

    let Some(chat_ref) = ctx
        .chat_ref(chat)
        .or_else(|| peer.map(PeerId::to_ambient_ref))
    else {
        return;
    };

    if pinned {
        let others: Vec<i32> = changed.iter().copied().filter(|id| *id != kept).collect();
        if others.is_empty() {
            return;
        }
        if gone(ctx, chat_ref, kept).await {
            release_because(ctx, chat_ref, chat, true).await;
            return;
        }
        for id in others {
            if let Err(e) = ctx.client.unpin_message(chat_ref, id).await {
                eprintln!("pin lock: {chat}: could not unpin {id}: {e}");
            }
        }
        return;
    }

    if changed.contains(&kept)
        && let Err(e) = ctx.client.pin_message(chat_ref, kept).await
    {
        eprintln!("pin lock: {chat}: could not restore pin {kept}: {e}");
        release_because(ctx, chat_ref, chat, gone(ctx, chat_ref, kept).await).await;
    }
}

async fn gone(ctx: &Ctx, chat_ref: PeerRef, kept: i32) -> bool {
    match ctx.client.get_messages_by_id(chat_ref, &[kept]).await {
        Ok(found) => found.first().is_none_or(Option::is_none),
        Err(e) => {
            eprintln!("pin lock: could not look up {kept}: {e}");
            false
        }
    }
}

async fn release_because(ctx: &Ctx, chat_ref: PeerRef, chat: i64, deleted: bool) {
    if let Err(error) = clear(ctx, chat).await {
        ::log::warn!("pin lock: automatic release for {chat} failed: {error}");
        return;
    }

    let why = if deleted {
        "پیام سنجاق شده حذف شده بود، پس قفل برداشته شد."
    } else {
        "ربات نتوانست سنجاق را برگرداند، پس قفل برداشته شد.\n\
         <i>مطمئن شوید ربات ادمین است و اجازه سنجاق کردن دارد.</i>"
    };
    let _ = ctx
        .client
        .send_message(
            chat_ref,
            super::premium::icon_html(
                Some(super::premium::Icon::Pin),
                format!("<b>قفل سنجاق</b>\n\n{why}"),
            ),
        )
        .await;
}

async fn clear(ctx: &Ctx, chat: i64) -> Result<(), SettingsWriteError> {
    ctx.settings
        .try_apply_batch(
            chat,
            &[
                SettingMutation::Delete { key: LOCK },
                SettingMutation::Delete { key: KEPT },
            ],
        )
        .await?;
    Ok(())
}

fn setting_failure_text(
    error: &SettingsWriteError,
    rejected: &str,
) -> grammers_client::message::InputMessage {
    let message = match error {
        SettingsWriteError::CommitUncertain(_) => {
            "نتیجه ذخیره سازی قفل نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید و ربات را دوباره راه اندازی کنید."
        }
        _ => rejected,
    };
    super::premium::icon_text(Some(super::premium::Icon::ErrorRed), message)
}
