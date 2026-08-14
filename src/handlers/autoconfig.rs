use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;
use grammers_client::update::Raw;
use grammers_client::message::{InputMessage, Message};

use super::{Ctx, bots, config, locks, welcome};

const DEFAULTS: &[(&str, &str)] = &[
    ("links", "قفل لینک"),
    ("file", "قفل فایل"),
    (locks::SERVICE, "قفل سرویس تلگرام"),
    (bots::LOCK, "قفل ورود ربات"),
    (bots::KICK_ADDER, "اخراج اضافه کننده ربات"),
];

const DEFAULT_WELCOME: &str = "{منشن} به {گروه} خوش آمدی.";

pub async fn on_message(ctx: &Ctx, message: &Message) -> bool {
    let Some(tl::enums::MessageAction::ChatAddUser(action)) = message.action() else {
        return false;
    };

    let me_id = ctx.me_id();
    if me_id == 0 || !action.users.contains(&me_id) {
        return false;
    }
    let Ok(Some(chat)) = message.peer_ref().await else {
        return false;
    };

    let me_ref = match PeerId::user(me_id) {
        Some(id) => id.to_ambient_ref(),
        None => return false,
    };
    let is_admin = ctx
        .client
        .get_permissions(chat, me_ref)
        .await
        .is_ok_and(|p| p.is_admin());
    if !is_admin {
        return false;
    }
    configure(ctx, chat).await
}

pub async fn on_raw(ctx: &Ctx, raw: &Raw) -> bool {
    let my_id = ctx.me_id();
    if my_id == 0 {
        return false;
    }

    let chat_id = match &raw.raw {
        tl::enums::Update::ChannelParticipant(update) if update.user_id == my_id => {
            let is_admin = matches!(
                update.new_participant,
                Some(
                    tl::enums::ChannelParticipant::Admin(_)
                        | tl::enums::ChannelParticipant::Creator(_)
                )
            );
            if !is_admin {
                return false;
            }
            PeerId::channel(update.channel_id)
        }
        tl::enums::Update::ChatParticipantAdmin(update)
            if update.user_id == my_id && update.is_admin =>
        {
            PeerId::chat(update.chat_id)
        }
        _ => return false,
    };
    let Some(chat_id) = chat_id else {
        return false;
    };
    let chat_ref = chat_id
        .bot_api_dialog_id()
        .and_then(|id| ctx.chat_ref(id))
        .unwrap_or_else(|| chat_id.to_ambient_ref());
    configure(ctx, chat_ref).await
}

async fn configure(ctx: &Ctx, chat: PeerRef) -> bool {
    let Some(chat_id) = chat.id.bot_api_dialog_id() else {
        return false;
    };
    if super::owner(ctx, chat_id).is_some() {
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
        lines.push("✓ خوشامدگویی".to_owned());
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
