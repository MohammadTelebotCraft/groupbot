use grammers_client::message::Message;
use grammers_client::session::types::{PeerKind, PeerRef};
use grammers_client::tl;

use super::restrict::{self, Action};
use super::{Ctx, locks};

pub const LOCK: &str = "biolink";

pub const ACTION: &str = "biolink_action";

#[derive(Clone, Copy, PartialEq)]
pub enum Act {
    Delete,
    Mute,
    Kick,
    Ban,
}

pub fn action_of(ctx: &Ctx, chat: i64) -> Act {
    match ctx.settings.value(chat, ACTION).as_deref() {
        Some("mute") => Act::Mute,
        Some("kick") => Act::Kick,
        Some("ban") => Act::Ban,
        _ => Act::Delete,
    }
}

pub fn action_label(act: Act) -> &'static str {
    match act {
        Act::Delete => "فقط حذف",
        Act::Mute => "سکوت",
        Act::Kick => "اخراج",
        Act::Ban => "بن",
    }
}

pub fn has_link(lowercased: &str) -> bool {
    locks::text_has_link(lowercased) || mentions_a_handle(lowercased)
}

fn mentions_a_handle(text: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '@'))
        .filter_map(|word| word.strip_prefix('@'))
        .any(|handle| {
            (5..=32).contains(&handle.chars().count())
                && handle.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

pub async fn tripped(ctx: &std::sync::Arc<Ctx>, chat: i64, message: &Message) -> bool {
    if !ctx.settings.is_locked(chat, LOCK) {
        return false;
    }

    let Some(sender) = message.sender_id().filter(|id| id.kind() == PeerKind::User) else {
        return false;
    };
    let Some(user) = sender.bare_id() else {
        return false;
    };
    if let Some(verdict) = ctx.claim_bio(user) {
        return verdict;
    }

    let Ok(Some(target)) = message.sender_ref().await else {
        return false;
    };
    let ctx = std::sync::Arc::clone(ctx);
    tokio::spawn(async move {
        let _slot = ctx.bio_slot().await;
        if let Some(about) = fetch(&ctx, target).await {
            ctx.remember_bio(user, has_link(&about.to_lowercase()));
        }
    });
    false
}

async fn fetch(ctx: &Ctx, target: PeerRef) -> Option<String> {
    let full = ctx
        .client
        .invoke(&tl::functions::users::GetFullUser { id: target.into() })
        .await;
    match full {
        Ok(tl::enums::users::UserFull::Full(found)) => {
            let tl::enums::UserFull::Full(user) = found.full_user;
            user.about
        }
        Err(e) => {
            eprintln!("biolink: could not read a bio: {e}");
            None
        }
    }
}

pub async fn punish(ctx: &Ctx, message: &Message, chat: i64) {
    let act = action_of(ctx, chat);
    if act == Act::Delete {
        return;
    }
    let (Ok(Some(chat_ref)), Ok(Some(target))) =
        (message.peer_ref().await, message.sender_ref().await)
    else {
        return;
    };

    if act == Act::Kick {
        if let Err(e) = ctx.client.kick_participant(chat_ref, target).await {
            eprintln!("biolink: {chat}: could not kick: {e}");
        }
        return;
    }

    let action = if act == Act::Ban { Action::Ban } else { Action::Mute };
    let applied = restrict::apply(
        ctx,
        chat_ref,
        target,
        action,
        None,
        restrict::By {
            reason: "لینک در بایو",
            target_name: &super::name_of(message),
            ..Default::default()
        },
    )
    .await;
    match applied {
        Ok(()) => ctx.bump(
            chat,
            if action == Action::Ban {
                super::stats::BANNED
            } else {
                super::stats::MUTED
            },
        ),
        Err(e) => eprintln!("biolink: {chat}: could not restrict: {e}"),
    }
}

pub fn is_locked(ctx: &Ctx, chat: i64) -> bool {
    ctx.settings.is_locked(chat, LOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_what_a_bio_is_advertising() {
        assert!(has_link("https://example.com"));
        assert!(has_link("t.me/spam"));
        assert!(has_link("www.shop"));
        assert!(has_link("برای سفارش @my_channel"));
        assert!(has_link("@shop_here"));

        assert!(!has_link(""));
        assert!(!has_link("سلام، خوشحالم که اینجایید"));
        assert!(!has_link("just a normal bio"));

        assert!(!has_link("@ab"));

        assert!(!has_link("me@example"));

        assert!(!has_link("@ خانه"));
    }

    #[test]
    fn every_offered_action_maps_to_a_distinct_one() {
        let reading = |value: &str| match value {
            "mute" => Act::Mute,
            "kick" => Act::Kick,
            "ban" => Act::Ban,
            _ => Act::Delete,
        };
        let offered = super::super::setting::SETTINGS
            .iter()
            .find(|declared| declared.key == ACTION)
            .expect("the action is declared in the settings table");
        let super::super::setting::Kind::Pick { options, default } = &offered.kind else {
            panic!("the bio link action is a pick");
        };

        let mapped: Vec<Act> = options.iter().map(|pick| reading(pick.value)).collect();
        for (at, act) in mapped.iter().enumerate() {
            assert!(
                !mapped[..at].contains(act),
                "two options answer to the same action: {}",
                options[at].value
            );
        }
        assert_eq!(mapped.len(), 4, "every action is offered");

        assert!(reading(default) == Act::Delete, "the default is the gentlest action");
        assert!(reading("") == Act::Delete);
        assert!(reading("BAN") == Act::Delete);
    }
}
