mod selector;
mod telegram;

pub use selector::{Context, icon_for, lock_icon, permission, protection, section_icon};
pub use telegram::{
    Surface, badge, buttons, decorate, html, icon_html, icon_html_on, icon_text, initialize,
    plain_label, text,
};

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Confidence {
    High,
    Medium,
    Unclassified,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct PremiumEmoji {
    pub key: String,
    pub custom_emoji_id: Option<String>,
    pub placeholder: String,
    pub fallback: String,
    pub semantic_tags: Vec<String>,
    pub confidence: Confidence,
    pub usage: String,
    pub source: String,
}

pub const REGISTRY_JSON: &str = include_str!("premium/registry.json");

pub fn registry() -> &'static [PremiumEmoji] {
    static REGISTRY: OnceLock<Vec<PremiumEmoji>> = OnceLock::new();
    REGISTRY.get_or_init(|| serde_json::from_str(REGISTRY_JSON).expect("invalid emoji registry"))
}

macro_rules! icons {
    ($($name:ident => $key:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Icon { $($name),+ }
        impl Icon {
            pub const ALL: &'static [Self] = &[$(Self::$name),+];
            pub const fn key(self) -> &'static str { match self { $(Self::$name => $key),+ } }
            pub fn from_key(key: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|icon| icon.key() == key)
            }
            pub fn entry(self) -> &'static PremiumEmoji {
                registry().iter().find(|entry| entry.key == self.key()).expect("unregistered icon")
            }
            pub fn fallback(self) -> &'static str { &self.entry().fallback }
            pub fn document_id(self) -> Option<i64> {
                let entry = self.entry();
                (entry.confidence == Confidence::High && entry.usage == "ACTIVE")
                    .then(|| entry.custom_emoji_id.as_deref()?.parse().ok()).flatten()
            }
        }
    }
}

icons! {
    Locked => "LOCKED", Unlocked => "UNLOCKED", Muted => "MUTED",
    SpeakerMuted => "SPEAKER_MUTED", MicMuted => "MIC_MUTED", Speaker => "SPEAKER",
    ModerationHammer => "MODERATION_HAMMER", Warning => "WARNING", Success => "SUCCESS",
    ErrorRed => "ERROR_RED", Close => "CLOSE", User => "USER", Members => "MEMBERS",
    Chat => "CHAT", TelegramSend => "TELEGRAM_SEND", Pin => "PIN",
    DocumentActivity => "DOCUMENT_ACTIVITY", ArchiveHistory => "ARCHIVE_HISTORY",
    Timer => "TIMER", Hourglass => "HOURGLASS", Calendar => "CALENDAR", Fire => "FIRE",
    Lightning => "LIGHTNING", Image => "IMAGE", Camera => "CAMERA",
    ImagePrivate => "IMAGE_PRIVATE", ImageDisabled => "IMAGE_DISABLED", Music => "MUSIC",
    Voice => "VOICE", Angry => "ANGRY", Mask => "MASK", Invisible => "INVISIBLE",
    Pause => "PAUSE", Network => "NETWORK", PremiumStar => "PREMIUM_STAR",
    Diamond => "DIAMOND", Briefcase => "BRIEFCASE", Group => "GROUP", Help => "HELP",
    Settings => "SETTINGS", Back => "BACK", Delete => "DELETE", Video => "VIDEO",
    File => "FILE", Welcome => "WELCOME", Stats => "STATS", Link => "LINK",
    Night => "NIGHT", Bot => "BOT",
}

pub fn web_fallbacks() -> String {
    let entries: std::collections::BTreeMap<_, _> = Icon::ALL
        .iter()
        .map(|icon| (icon.key(), icon.fallback()))
        .collect();
    serde_json::to_string(&entries).expect("serializable icon strings")
}

#[cfg(test)]
mod tests;
