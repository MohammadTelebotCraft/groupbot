use grammers_client::message::Message;
use grammers_client::session::types::PeerId;

use super::Ctx;

pub const MODE: &str = "lim";

pub struct Cap {
    pub name: &'static str,
    pub key: &'static str,
    pub label: &'static str,
}

pub const BAN: &Cap = &Cap { name: "ban", key: "lim_ban", label: "بن" };
pub const MUTE: &Cap = &Cap { name: "mute", key: "lim_mute", label: "سکوت" };
pub const WARN: &Cap = &Cap { name: "warn", key: "lim_warn", label: "اخطار" };
pub const SET: &Cap = &Cap { name: "set", key: "lim_set", label: "تنظیمات" };
pub const CLEAN: &Cap = &Cap { name: "clean", key: "lim_clean", label: "پاکسازی" };
pub const EXEMPT: &Cap = &Cap { name: "exempt", key: "lim_exempt", label: "معافیت" };
pub const PIN: &Cap = &Cap { name: "pin", key: "lim_pin", label: "سنجاق" };
pub const VIP: &Cap = &Cap { name: "vip", key: "lim_vip", label: "عضو ویژه" };

pub const CAPS: &[&Cap] = &[BAN, MUTE, WARN, SET, CLEAN, EXEMPT, PIN, VIP];

pub fn find(name: &str) -> Option<&'static Cap> {
    CAPS.iter().copied().find(|cap| cap.name == name)
}

fn decides(is_owner: bool, master_on: bool, denied: bool) -> bool {
    is_owner || !(master_on && denied)
}

pub fn permits(ctx: &Ctx, chat: i64, user: i64, cap: &Cap) -> bool {
    let is_owner = super::owner(ctx, chat) == Some(user);
    ctx.settings.with_chat(chat, |settings| {
        decides(is_owner, settings.is_locked(MODE), settings.is_locked(cap.key))
    })
}

pub fn allowed(ctx: &Ctx, message: &Message, cap: &Cap) -> bool {
    match (
        super::chat_id(message),
        message.sender_id().and_then(PeerId::bare_id),
    ) {
        (Some(chat), Some(user)) => permits(ctx, chat, user, cap),
        _ => true,
    }
}

pub async fn allows(ctx: &Ctx, message: &Message, cap: &Cap) -> bool {
    if !super::can_manage(ctx, message).await {
        return false;
    }
    if allowed(ctx, message, cap) {
        return true;
    }
    deny(message, cap).await;
    false
}

fn refusal(cap: &Cap) -> String {
    format!("✗ دسترسی {} برای شما بسته است.", cap.label)
}

pub async fn deny(message: &Message, cap: &Cap) {
    let _ = message.reply(refusal(cap)).await;
}

pub async fn refuse(query: &grammers_client::update::CallbackQuery, cap: &Cap) {
    let _ = query.answer().alert(refusal(cap)).send().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_is_never_bound_and_off_means_open() {
        assert!(decides(true, true, true), "the owner is never limited");
        assert!(decides(false, false, true), "a denial with the feature off does nothing");
        assert!(decides(false, true, false), "an undenied capability stays open");
        assert!(!decides(false, true, true), "an admin hitting a live denial is refused");
    }

    #[test]
    fn every_cap_is_named_once() {
        let count = CAPS.len();
        let mut names: Vec<&str> = CAPS.iter().map(|cap| cap.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "two capabilities answer to the same name");

        let mut keys: Vec<&str> = CAPS.iter().map(|cap| cap.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two capabilities share a settings key");
    }

    #[test]
    fn payloads_fit_telegram() {
        for cap in CAPS {
            let longest = format!("p:{}:{}:{MODE}:{}", i64::MAX, i64::MIN, cap.name);
            assert!(longest.len() <= 64, "payload too long for {}", cap.name);
            assert!(cap.key.starts_with(MODE), "{} is not a lim key", cap.key);
            assert!(!cap.name.contains(':'), "{} has a colon in its name", cap.name);
        }
    }
}
