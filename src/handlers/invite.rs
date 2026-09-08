
use grammers_client::message::Message;
use grammers_client::session::types::PeerRef;
use grammers_client::tl;

use super::{Ctx, esc};
use crate::response::ResponseKind;

pub const COMMANDS: &[&str] = &["لینک", "لینک گروه"];

pub const RENEW: &[&str] = &["لینک جدید", "باطل کردن لینک", "ابطال لینک"];

async fn primary(ctx: &Ctx, chat_ref: PeerRef) -> Option<String> {
    let full = ctx
        .client
        .invoke(&tl::functions::channels::GetFullChannel {
            channel: chat_ref.into(),
        })
        .await
        .ok()?;
    let tl::enums::messages::ChatFull::Full(full) = full;
    let tl::enums::ChatFull::ChannelFull(channel) = full.full_chat else {
        return None;
    };
    link_of(channel.exported_invite?)
}

fn link_of(invite: tl::enums::ExportedChatInvite) -> Option<String> {
    match invite {
        tl::enums::ExportedChatInvite::ChatInviteExported(invite) => Some(invite.link),
        _ => None,
    }
}

async fn revoke(ctx: &Ctx, chat_ref: PeerRef, link: &str) -> Result<Option<String>, String> {
    let answer = ctx
        .client
        .invoke_outbound(&tl::functions::messages::EditExportedChatInvite {
            revoked: true,
            peer: chat_ref.into(),
            link: link.to_owned(),
            expire_date: None,
            usage_limit: None,
            request_needed: None,
            title: None,
        })
        .await
        .map_err(describe)?;
    Ok(match answer {
        tl::enums::messages::ExportedChatInvite::Replaced(replaced) => link_of(replaced.new_invite),
        tl::enums::messages::ExportedChatInvite::Invite(_) => None,
    })
}

async fn create(ctx: &Ctx, chat_ref: PeerRef) -> Result<Option<String>, String> {
    let exported = ctx
        .client
        .invoke_outbound(&tl::functions::messages::ExportChatInvite {
            legacy_revoke_permanent: false,
            request_needed: false,
            peer: chat_ref.into(),
            expire_date: None,
            usage_limit: None,
            title: None,
            subscription_pricing: None,
        })
        .await
        .map_err(describe)?;
    Ok(link_of(exported))
}

fn describe(e: grammers_client::InvocationError) -> String {
    match e.to_string().contains("CHAT_ADMIN_REQUIRED") {
        true => "ربات دسترسی «افزودن اعضا» ندارد؛ آن را بدهید و دوباره بفرستید.".to_owned(),
        false => format!("انجام نشد · {e}"),
    }
}

fn card(link: &str, fresh: bool) -> String {
    format!(
        "<b>لینک گروه</b>\n\n\
         لینک · <code>{}</code>\n\n\
         <i>{}</i>",
        esc(link),
        if fresh {
            "لینک قبلی از کار افتاد. لینک تازه: «لینک جدید»"
        } else {
            "برای باطل کردن این لینک: «لینک جدید»"
        }
    )
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let show = COMMANDS.contains(&text);
    let renew = RENEW.contains(&text);
    if !show && !renew {
        return false;
    }
    if message.peer_id().bot_api_dialog_id().is_none() {
        return false;
    }
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return true;
    };

    let current = primary(ctx, chat_ref).await;
    if show {
        match &current {
            Some(link) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::UtilityResult,
                    super::premium::html(card(link, false)),
                )
                .await
            }
            None => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::UtilityResult,
                    "هنوز لینکی ساخته نشده. «لینک جدید» بفرستید.",
                )
                .await
            }
        };
        return true;
    }

    let made = match &current {
        Some(link) => revoke(ctx, chat_ref, link).await,
        None => create(ctx, chat_ref).await,
    };
    match made {
        Ok(link) => match match link {
            Some(link) => Some(link),
            None => primary(ctx, chat_ref).await,
        } {
            Some(link) => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::UtilityResult,
                    super::premium::html(card(&link, true)),
                )
                .await
            }
            None => {
                super::respond(
                    ctx,
                    message,
                    ResponseKind::CommandError,
                    super::premium::icon_text(
                        Some(super::premium::Icon::ErrorRed),
                        "لینک تازه ساخته نشد.",
                    ),
                )
                .await
            }
        },
        Err(reason) => super::respond(ctx, message, ResponseKind::CommandError, reason).await,
    };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_command_sets_do_not_overlap() {
        for command in COMMANDS {
            assert!(
                !RENEW.contains(command),
                "«{command}» is in both tables and one of them would never run"
            );
        }
        assert!(RENEW.iter().any(|r| r.starts_with("لینک")));
        assert!(!COMMANDS.contains(&"لینک جدید"));
    }

    #[test]
    fn only_an_exported_invite_yields_a_link() {
        let exported: tl::enums::ExportedChatInvite = tl::types::ChatInviteExported {
            revoked: false,
            permanent: true,
            request_needed: false,
            link: "https://t.me/+abc".to_owned(),
            admin_id: 1,
            date: 0,
            start_date: None,
            expire_date: None,
            usage_limit: None,
            usage: None,
            requested: None,
            subscription_expired: None,
            title: None,
            subscription_pricing: None,
        }
        .into();
        assert_eq!(link_of(exported).as_deref(), Some("https://t.me/+abc"));

        assert_eq!(
            link_of(tl::enums::ExportedChatInvite::ChatInvitePublicJoinRequests),
            None
        );
    }
}
