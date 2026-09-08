use super::{Confidence, Icon};

#[derive(Clone, Copy, Debug, Default)]
pub struct Context<'a> {
    pub action: &'a str,
    pub object: &'a str,
    pub state: &'a str,
    pub severity: &'a str,
    pub scope: &'a str,
    pub duration: Option<std::time::Duration>,
}

pub fn icon_for(context: Context<'_>) -> Option<Icon> {
    use Icon::*;
    let Context {
        action,
        object,
        state,
        severity,
        scope,
        duration,
    } = context;
    let icon = if matches!(state, "failed" | "error" | "rejected") {
        ErrorRed
    } else if matches!(state, "waiting" | "pending") {
        Hourglass
    } else if state == "paused" || action == "pause" {
        Pause
    } else if let Some(icon) = match action {
        "ban" | "kick" | "punish" => Some(ModerationHammer),
        "mute" => Some(Muted),
        "unmute" | "unban" | "unlock" | "restore_permission" => Some(Unlocked),
        "warn" => Some(Warning),
        "pin" | "unpin" => Some(Pin),
        "cancel" | "close" | "dismiss" => Some(Close),
        "back" | "previous" | "next" => Some(Back),
        "send" => Some(TelegramSend),
        "delete" | "purge" => Some(Delete),
        _ => None,
    } {
        icon
    } else if scope == "permission" || action == "restrict" {
        permission(object, matches!(state, "allowed" | "restored"))
    } else {
        match object {
            "images" | "image" | "photos" | "media" if state == "blocked" => ImageDisabled,
            "images" | "image" | "photos" | "media" => Image,
            "audio" | "audios" | "music" => {
                if state == "blocked" {
                    SpeakerMuted
                } else {
                    Music
                }
            }
            "voice" | "voices" | "microphone" => {
                if state == "blocked" {
                    MicMuted
                } else {
                    Voice
                }
            }
            "user" | "profile" => User,
            "group" | "members" => Group,
            "logs" | "reports" | "cases" | "recent_actions" => DocumentActivity,
            "history" | "archive" => ArchiveHistory,
            "message" | "text" | "message_filter" => Chat,
            "toxicity" | "insult_filter" => Angry,
            "anti_flood" | "raid"
                if state == "triggered" && matches!(severity, "high" | "critical") =>
            {
                Fire
            }
            "anti_flood" | "cooldown" | "rate_limit" | "temporary_restriction" | "timeout" => Timer,
            "anti_sell_trade" | "anti_trade" | "anti_link" | "anti_spam" | "filter"
            | "protection" | "raid" => protection(state != "disabled"),
            "notifications" if state == "disabled" => Muted,
            "settings" => Settings,
            "help" | "onboarding" => Help,
            "date" | "expiry" => Calendar,
            "premium" | "vip" => PremiumStar,
            "automod" => Bot,
            _ => match state {
                "locked" | "restricted" | "blocked" => Locked,
                "unlocked" | "restored" => Unlocked,
                "success" | "saved" | "enabled" => Success,
                _ if action == "confirm" => Success,
                _ if duration.is_some() => Timer,
                _ => return None,
            },
        }
    };
    (icon.entry().confidence == Confidence::High).then_some(icon)
}

pub fn protection(on: bool) -> Icon {
    if on { Icon::Locked } else { Icon::Unlocked }
}

pub fn lock_icon(key: &str, on: bool) -> Icon {
    match key {
        "photo" | "photos" => permission("photos", !on),
        "voice" | "voices" => permission("voices", !on),
        "audio" | "audios" | "music" => permission("audios", !on),
        _ => protection(on),
    }
}

pub fn permission(object: &str, allowed: bool) -> Icon {
    use Icon::*;
    match (object, allowed) {
        ("image" | "images" | "photos" | "media", false) => ImageDisabled,
        ("image" | "images" | "photos" | "media", true) => Image,
        ("voice" | "voices" | "microphone", false) => MicMuted,
        ("voice" | "voices" | "microphone", true) => Voice,
        ("audio" | "audios" | "music", false) => SpeakerMuted,
        ("audio" | "audios" | "music", true) => Speaker,
        (_, false) => Locked,
        (_, true) => Unlocked,
    }
}

pub fn section_icon(section: &str) -> Option<Icon> {
    use Icon::*;
    Some(match section {
        "locks" | "sec" | "bt" | "cp" | "jn" | "bl" | "gr" | "lim" => Locked,
        "s" | "strict" | "ban" => ModerationHammer,
        "mute" => Muted,
        "wn" | "sp" => Warning,
        "fl" | "tm" | "sl" | "tmed" => Timer,
        "rd" => Locked,
        "usr" => User,
        "adm" | "ad" => Group,
        "lg" | "dr" | "cases" => DocumentActivity,
        "ls" => ArchiveHistory,
        "filter" | "an" | "msg" | "answer" | "gp" | "nt" => Chat,
        "imf" | "imgf" | "cq" => Image,
        "nsw" => ImageDisabled,
        "vw" | "vm" => Voice,
        "wc" => Welcome,
        "ng" => Night,
        "cl" | "ap" | "wp" => Delete,
        "st" | "sum" => Stats,
        "ai" => Bot,
        "pin" => Pin,
        "vip" => PremiumStar,
        "close" | "x" => Close,
        "i" | "help" => Help,
        "adv" | "etc" | "root" => Settings,
        _ => return None,
    })
}
