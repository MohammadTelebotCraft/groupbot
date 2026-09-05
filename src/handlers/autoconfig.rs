use std::sync::Arc;

use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;
use grammers_client::update::Raw;

use super::{Ctx, bots, config, install, locks, welcome};

const DEFAULTS: &[(&str, &str)] = &[
    ("links", "قفل لینک"),
    ("file", "قفل فایل"),
    (locks::SERVICE, "قفل سرویس تلگرام"),
    (bots::LOCK, "قفل ورود ربات"),
    (bots::KICK_ADDER, "اخراج اضافه کننده ربات"),
];

const DEFAULT_WELCOME: &str = "{منشن} به {گروه} خوش آمدی.";

pub async fn on_message(ctx: &Arc<Ctx>, message: &Message) -> bool {
    let Some(tl::enums::MessageAction::ChatAddUser(action)) = message.action() else {
        return false;
    };

    let me_id = ctx.me_id();
    if me_id == 0 || !action.users.contains(&me_id) {
        return false;
    }
    let (Ok(Some(chat)), Some(chat_id)) = (
        message.peer_ref().await,
        message.peer_id().bot_api_dialog_id(),
    ) else {
        return false;
    };

    let standing = install::standing(ctx, chat).await;
    if !install::ready(&standing) {
        install::announce(ctx, chat, chat_id, &standing).await;

        return false;
    }
    let configured = configure(ctx, chat).await;
    install::ensure_cleaner(ctx, chat, chat_id).await;
    configured
}

pub async fn on_raw(ctx: &Arc<Ctx>, raw: &Raw) -> bool {
    let my_id = ctx.me_id();
    if my_id == 0 {
        return false;
    }

    let (chat_id, standing) = match &raw.raw {
        tl::enums::Update::ChannelParticipant(update) if update.user_id == my_id => (
            PeerId::channel(update.channel_id),
            install::from_participant(update.new_participant.as_ref()),
        ),
        tl::enums::Update::ChatParticipantAdmin(update) if update.user_id == my_id => (
            PeerId::chat(update.chat_id),
            install::Standing::Basic {
                admin: update.is_admin,
            },
        ),
        _ => return false,
    };
    let Some(chat_id) = chat_id else {
        return false;
    };
    let Some(chat) = chat_id.bot_api_dialog_id() else {
        return false;
    };
    let chat_ref = ctx
        .chat_ref(chat)
        .unwrap_or_else(|| chat_id.to_ambient_ref());

    if !install::ready(&standing) {
        install::announce(ctx, chat_ref, chat, &standing).await;
        return false;
    }

    let configured = configure(ctx, chat_ref).await;
    if !configured {
        install::announce_complete(ctx, chat_ref, chat).await;
    }
    install::ensure_cleaner(ctx, chat_ref, chat).await;
    configured
}

pub async fn configure(ctx: &Ctx, chat: PeerRef) -> bool {
    let Some(chat_id) = chat.id.bot_api_dialog_id() else {
        return false;
    };
    if super::owner(ctx, chat_id).is_some() {
        return false;
    }

    if !ctx.claim_autoconfig(chat_id) {
        return false;
    }
    let (creator, admin_names) = super::admins(ctx, chat).await;
    let Some((creator_id, creator_name)) = creator else {
        eprintln!("auto-config: no creator found for {chat_id}");
        return false;
    };

    ctx.settings
        .set_value(chat_id, config::OWNER, &creator_id.to_string())
        .await;
    let locked = apply_defaults(ctx, chat_id).await;
    let _ = ctx
        .client
        .send_message(
            chat,
            InputMessage::new().html(summary(
                "ربات فعال شد",
                &creator_name,
                creator_id,
                &admin_names,
                &locked,
            )),
        )
        .await;
    true
}

pub async fn apply_defaults(ctx: &Ctx, chat_id: i64) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (key, label) in DEFAULTS {
        ctx.settings.set(chat_id, key, true).await;
        lines.push(format!("✓ {label}"));
    }

    if ctx
        .settings
        .value(chat_id, welcome::TEXT)
        .unwrap_or_default()
        .is_empty()
    {
        ctx.settings
            .set_value(chat_id, welcome::TEXT, DEFAULT_WELCOME)
            .await;
        ctx.settings
            .set_value(chat_id, welcome::TTL, &welcome::INSTALL_TTL.to_string())
            .await;
        lines.push(format!(
            "✓ خوشامدگویی · حذف خودکار بعد از {} ثانیه",
            welcome::INSTALL_TTL
        ));
    }
    lines.join("\n")
}

pub fn summary(
    title: &str,
    owner_name: &str,
    owner_id: i64,
    admin_names: &[String],
    locked: &str,
) -> String {
    format!(
        "<b>{title}</b>\n\n\
         مالک ربات · <b>{owner_name}</b>\n\
         شناسه · <code>{owner_id}</code>\n\n\
         <b>ادمین ها</b> ({})\n\
         {}\n\
         {}\n\
         <i>راهنما برای دستورها، پنل برای تنظیمات</i>",
        admin_names.len(),
        admin_names.join("\n"),
        if locked.is_empty() {
            String::new()
        } else {
            format!("\n<b>پیش فرض ها</b>\n{locked}\n")
        },
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_keys_are_real() {
        for (key, _) in super::DEFAULTS {
            assert!(
                super::locks::LOCKS.iter().any(|lock| lock.key == *key)
                    || [super::bots::LOCK, super::bots::KICK_ADDER].contains(key),
                "unknown setting key {key}"
            );
        }
    }
}
