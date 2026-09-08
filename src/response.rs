
use std::fmt;
use std::str::FromStr;

use grammers_client::message::{InputMessage, Message};
use grammers_client::peer::Peer;
use grammers_client::session::types::{PeerKind, PeerRef};

use crate::state::{ChatSettings, SettingMutation, Settings, SettingsWriteError};

pub const MODE_KEY: &str = "response:mode";
const CATEGORY_PREFIX: &str = "response:category:";
const KIND_PREFIX: &str = "response:kind:";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseMode {
    #[default]
    Off,
    Balanced,
    PrivateFirst,
    Custom,
}

impl NoiseMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Balanced => "balanced",
            Self::PrivateFirst => "private_first",
            Self::Custom => "custom",
        }
    }
}

impl fmt::Display for NoiseMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for NoiseMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "balanced" => Ok(Self::Balanced),
            "private_first" => Ok(Self::PrivateFirst),
            "custom" => Ok(Self::Custom),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityOverride {
    Default,
    Public,
    Private,
}

impl VisibilityOverride {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

impl FromStr for VisibilityOverride {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(Self::Default),
            "public" => Ok(Self::Public),
            "private" => Ok(Self::Private),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseCategory {
    CommandErrors,
    PermissionDenied,
    Settings,
    Help,
    AdminTools,
    MemberLookups,
    Statistics,
    Warnings,
    Moderation,
    FilterManagement,
    LockManagement,
    AntiSpam,
    Welcome,
    Verification,
    Utilities,
    PersonalInformation,
    CallbackMenus,
}

impl ResponseCategory {
    pub const ALL: [Self; 17] = [
        Self::CommandErrors,
        Self::PermissionDenied,
        Self::Settings,
        Self::Help,
        Self::AdminTools,
        Self::MemberLookups,
        Self::Statistics,
        Self::Warnings,
        Self::Moderation,
        Self::FilterManagement,
        Self::LockManagement,
        Self::AntiSpam,
        Self::Welcome,
        Self::Verification,
        Self::Utilities,
        Self::PersonalInformation,
        Self::CallbackMenus,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CommandErrors => "command_errors",
            Self::PermissionDenied => "permission_denied",
            Self::Settings => "settings",
            Self::Help => "help",
            Self::AdminTools => "admin_tools",
            Self::MemberLookups => "member_lookups",
            Self::Statistics => "statistics",
            Self::Warnings => "warnings",
            Self::Moderation => "moderation",
            Self::FilterManagement => "filter_management",
            Self::LockManagement => "lock_management",
            Self::AntiSpam => "anti_spam",
            Self::Welcome => "welcome",
            Self::Verification => "verification",
            Self::Utilities => "utilities",
            Self::PersonalInformation => "personal_information",
            Self::CallbackMenus => "callback_menus",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
    }

    fn key(self) -> String {
        format!("{CATEGORY_PREFIX}{}", self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisibilityClass {
    PublicRequired,
    PrivatePreferred,
    Configurable,
    ContextDependent,
    PrivacyRequired,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseKind {
    CommandError,
    PermissionDenied,
    SettingsView,
    SettingsChanged,
    Help,
    AdminTool,
    MemberLookup,
    Statistics,
    WarningLookup,
    ModerationConfirmation,
    ModerationAnnouncement,
    WarningNotice,
    FilterManagement,
    LockManagement,
    AntiSpamControl,
    WelcomeSetup,
    WelcomeNotice,
    VerificationSetup,
    VerificationChallenge,
    UtilityResult,
    PersonalInformation,
    CleanerAuthentication,
    ModerationCaseDetails,
    CallbackFeedback,
    GroupAnnouncement,
    SecurityAlert,
    AuditLog,
    ScheduledReport,
    AutoAnswer,
    ContentRemovalNotice,
    ContextualAiResponse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseSemantics {
    pub class: VisibilityClass,
    pub category: ResponseCategory,
    pub balanced_private: bool,
}

impl ResponseKind {
    pub const OVERRIDABLE: [Self; 2] = [Self::ContentRemovalNotice, Self::WelcomeNotice];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CommandError => "command_error",
            Self::PermissionDenied => "permission_denied",
            Self::SettingsView => "settings_view",
            Self::SettingsChanged => "settings_changed",
            Self::Help => "help",
            Self::AdminTool => "admin_tool",
            Self::MemberLookup => "member_lookup",
            Self::Statistics => "statistics",
            Self::WarningLookup => "warning_lookup",
            Self::ModerationConfirmation => "moderation_confirmation",
            Self::ModerationAnnouncement => "moderation_announcement",
            Self::WarningNotice => "warning_notice",
            Self::FilterManagement => "filter_management",
            Self::LockManagement => "lock_management",
            Self::AntiSpamControl => "anti_spam_control",
            Self::WelcomeSetup => "welcome_setup",
            Self::WelcomeNotice => "welcome_notice",
            Self::VerificationSetup => "verification_setup",
            Self::VerificationChallenge => "verification_challenge",
            Self::UtilityResult => "utility_result",
            Self::PersonalInformation => "personal_information",
            Self::CleanerAuthentication => "cleaner_authentication",
            Self::ModerationCaseDetails => "moderation_case_details",
            Self::CallbackFeedback => "callback_feedback",
            Self::GroupAnnouncement => "group_announcement",
            Self::SecurityAlert => "security_alert",
            Self::AuditLog => "audit_log",
            Self::ScheduledReport => "scheduled_report",
            Self::AutoAnswer => "auto_answer",
            Self::ContentRemovalNotice => "content_removal_notice",
            Self::ContextualAiResponse => "contextual_ai_response",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ContentRemovalNotice => "اعلان ها",
            Self::WelcomeNotice => "خوشامد",
            Self::SettingsChanged => "تأیید تغییر تنظیم",
            Self::Help => "پاسخ راهنما",
            Self::AdminTool => "نتیجه ابزار مدیر",
            Self::MemberLookup => "جستجوی عضو",
            Self::Statistics => "پاسخ آمار",
            Self::WarningLookup => "نمایش اخطارها",
            Self::ModerationConfirmation => "تأیید اجرای مجازات",
            Self::FilterManagement => "تأیید مدیریت فیلتر",
            Self::LockManagement => "تأیید مدیریت قفل",
            Self::AntiSpamControl => "تأیید ضد اسپم",
            Self::WelcomeSetup => "تأیید تنظیم خوش آمد",
            Self::UtilityResult => "پاسخ ابزار عمومی",
            _ => self.as_str(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::OVERRIDABLE
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }

    fn key(self) -> String {
        format!("{KIND_PREFIX}{}", self.as_str())
    }

    pub const fn semantics(self) -> ResponseSemantics {
        use ResponseCategory as C;
        use VisibilityClass as V;
        match self {
            Self::CommandError => semantics(V::PrivatePreferred, C::CommandErrors, true),
            Self::PermissionDenied => semantics(V::PrivatePreferred, C::PermissionDenied, true),
            Self::SettingsView | Self::SettingsChanged => {
                semantics(V::Configurable, C::Settings, true)
            }
            Self::Help => semantics(V::Configurable, C::Help, true),
            Self::AdminTool => semantics(V::Configurable, C::AdminTools, true),
            Self::MemberLookup => semantics(V::Configurable, C::MemberLookups, true),
            Self::Statistics => semantics(V::Configurable, C::Statistics, true),
            Self::WarningLookup => semantics(V::Configurable, C::Warnings, true),
            Self::ModerationConfirmation => semantics(V::Configurable, C::Moderation, true),
            Self::ModerationAnnouncement | Self::WarningNotice => {
                semantics(V::PublicRequired, C::Moderation, false)
            }
            Self::FilterManagement => semantics(V::Configurable, C::FilterManagement, true),
            Self::LockManagement => semantics(V::Configurable, C::LockManagement, true),
            Self::AntiSpamControl => semantics(V::Configurable, C::AntiSpam, true),
            Self::WelcomeSetup => semantics(V::Configurable, C::Welcome, true),
            Self::WelcomeNotice => semantics(V::ContextDependent, C::Welcome, false),
            Self::VerificationSetup => semantics(V::Configurable, C::Verification, true),
            Self::VerificationChallenge => semantics(V::PublicRequired, C::Verification, false),
            Self::UtilityResult => semantics(V::Configurable, C::Utilities, true),
            Self::PersonalInformation
            | Self::CleanerAuthentication
            | Self::ModerationCaseDetails => {
                semantics(V::PrivacyRequired, C::PersonalInformation, true)
            }
            Self::CallbackFeedback => semantics(V::PrivatePreferred, C::CallbackMenus, true),
            Self::GroupAnnouncement
            | Self::SecurityAlert
            | Self::AuditLog
            | Self::ScheduledReport => semantics(V::PublicRequired, C::AdminTools, false),
            Self::AutoAnswer | Self::ContextualAiResponse => {
                semantics(V::ContextDependent, C::Utilities, false)
            }
            Self::ContentRemovalNotice => semantics(V::PrivatePreferred, C::LockManagement, true),
        }
    }
}

const fn semantics(
    class: VisibilityClass,
    category: ResponseCategory,
    balanced_private: bool,
) -> ResponseSemantics {
    ResponseSemantics {
        class,
        category,
        balanced_private,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum IntendedAudience {
    RequesterOnly,
    WholeGroup,
    SharedWorkflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryCapabilities {
    pub is_group: bool,
    pub has_target_user: bool,
    pub callback_overlay: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryDecision {
    Regular,
    Ephemeral { public_fallback: bool },
    CallbackOverlay,
    DirectPrivate,
    Suppress,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PolicyKindView {
    pub id: &'static str,
    pub label: &'static str,
    pub visibility: VisibilityOverride,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PolicyView {
    pub overrides: Vec<PolicyKindView>,
    pub private_preview: Vec<&'static str>,
    pub public_preview: Vec<&'static str>,
}

pub fn policy_view(settings: &Settings, chat: i64) -> PolicyView {
    settings.with_chat(chat, |chat_settings| {
        let overrides = ResponseKind::OVERRIDABLE
            .into_iter()
            .map(|kind| PolicyKindView {
                id: kind.as_str(),
                label: kind.label(),
                visibility: effective_visibility(&chat_settings, kind),
            })
            .collect();
        let (private_preview, public_preview) = preview(&chat_settings);
        PolicyView {
            overrides,
            private_preview,
            public_preview,
        }
    })
}

fn preview(settings: &ChatSettings<'_>) -> (Vec<&'static str>, Vec<&'static str>) {
    let sample = [
        (ResponseKind::ContentRemovalNotice, "اعلان ها"),
        (ResponseKind::WelcomeNotice, "خوشامد"),
    ];
    let capabilities = DeliveryCapabilities {
        is_group: true,
        has_target_user: true,
        callback_overlay: false,
    };
    let mut private = Vec::new();
    let mut public = Vec::new();
    for (kind, label) in sample {
        match decide_with_settings(
            NoiseMode::Off,
            settings,
            kind,
            IntendedAudience::RequesterOnly,
            capabilities,
        ) {
            DeliveryDecision::Regular => public.push(label),
            _ => private.push(label),
        }
    }
    (private, public)
}

pub fn decision(
    settings: &Settings,
    chat: i64,
    kind: ResponseKind,
    audience: IntendedAudience,
    capabilities: DeliveryCapabilities,
) -> DeliveryDecision {
    settings.with_chat(chat, |chat_settings| {
        decide_with_settings(NoiseMode::Off, &chat_settings, kind, audience, capabilities)
    })
}

fn decide_with_settings(
    _mode: NoiseMode,
    settings: &ChatSettings<'_>,
    kind: ResponseKind,
    audience: IntendedAudience,
    capabilities: DeliveryCapabilities,
) -> DeliveryDecision {
    let semantics = kind.semantics();

    if semantics.class == VisibilityClass::PublicRequired {
        return DeliveryDecision::Regular;
    }
    if !capabilities.is_group {
        return DeliveryDecision::DirectPrivate;
    }
    if semantics.class == VisibilityClass::PrivacyRequired {
        return if capabilities.has_target_user {
            DeliveryDecision::Ephemeral {
                public_fallback: false,
            }
        } else {
            DeliveryDecision::Suppress
        };
    }
    if audience != IntendedAudience::RequesterOnly {
        return DeliveryDecision::Regular;
    }
    if kind == ResponseKind::CallbackFeedback && capabilities.callback_overlay {
        return DeliveryDecision::CallbackOverlay;
    }

    let wants_private = ResponseKind::OVERRIDABLE.contains(&kind)
        && effective_visibility(settings, kind) == VisibilityOverride::Private;
    if wants_private {
        if capabilities.has_target_user {
            DeliveryDecision::Ephemeral {
                public_fallback: false,
            }
        } else {
            DeliveryDecision::Suppress
        }
    } else {
        DeliveryDecision::Regular
    }
}

fn read_override(settings: &ChatSettings<'_>, key: &str) -> VisibilityOverride {
    settings
        .value(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(VisibilityOverride::Default)
}

fn effective_visibility(settings: &ChatSettings<'_>, kind: ResponseKind) -> VisibilityOverride {
    match read_override(settings, &kind.key()) {
        VisibilityOverride::Private => VisibilityOverride::Private,
        VisibilityOverride::Default | VisibilityOverride::Public => VisibilityOverride::Public,
    }
}

pub async fn set_kind(
    settings: &Settings,
    chat: i64,
    kind: ResponseKind,
    visibility: VisibilityOverride,
) -> Result<bool, SettingsWriteError> {
    if !ResponseKind::OVERRIDABLE.contains(&kind) {
        return Ok(false);
    }
    set_override(settings, chat, kind.key(), visibility).await
}

async fn set_override(
    settings: &Settings,
    chat: i64,
    key: String,
    visibility: VisibilityOverride,
) -> Result<bool, SettingsWriteError> {
    match visibility {
        VisibilityOverride::Default => Ok(settings
            .try_apply_batch(chat, &[SettingMutation::Delete { key: &key }])
            .await?
            != 0),
        value => settings.try_set_value(chat, &key, value.as_str()).await,
    }
}

pub async fn reset(settings: &Settings, chat: i64) -> Result<bool, SettingsWriteError> {
    const LEGACY_KINDS: [ResponseKind; 12] = [
        ResponseKind::SettingsChanged,
        ResponseKind::Help,
        ResponseKind::AdminTool,
        ResponseKind::MemberLookup,
        ResponseKind::Statistics,
        ResponseKind::WarningLookup,
        ResponseKind::ModerationConfirmation,
        ResponseKind::FilterManagement,
        ResponseKind::LockManagement,
        ResponseKind::AntiSpamControl,
        ResponseKind::WelcomeSetup,
        ResponseKind::UtilityResult,
    ];
    let mut keys = Vec::with_capacity(
        1 + ResponseCategory::ALL.len() + ResponseKind::OVERRIDABLE.len() + LEGACY_KINDS.len(),
    );
    keys.push(MODE_KEY.to_owned());
    keys.extend(ResponseCategory::ALL.into_iter().map(ResponseCategory::key));
    keys.extend(ResponseKind::OVERRIDABLE.into_iter().map(ResponseKind::key));
    keys.extend(LEGACY_KINDS.into_iter().map(ResponseKind::key));
    let mutations: Vec<_> = keys
        .iter()
        .map(|key| SettingMutation::Delete { key })
        .collect();
    Ok(settings.try_apply_batch(chat, &mutations).await? != 0)
}

pub fn validate_setting(key: &str, value: &str) -> Result<(), &'static str> {
    if key == MODE_KEY {
        return value
            .parse::<NoiseMode>()
            .map(drop)
            .map_err(|()| "off, balanced, private_first, or custom");
    }
    if let Some(category) = key.strip_prefix(CATEGORY_PREFIX) {
        if ResponseCategory::parse(category).is_none() {
            return Err("a known response category");
        }
        return value
            .parse::<VisibilityOverride>()
            .map(drop)
            .map_err(|()| "default, public, or private");
    }
    if let Some(kind) = key.strip_prefix(KIND_PREFIX) {
        let legacy = [
            "settings_changed",
            "help",
            "admin_tool",
            "member_lookup",
            "statistics",
            "warning_lookup",
            "moderation_confirmation",
            "filter_management",
            "lock_management",
            "anti_spam_control",
            "welcome_setup",
            "utility_result",
        ];
        if ResponseKind::parse(kind).is_none() && !legacy.contains(&kind) {
            return Err("a known overridable response kind");
        }
        return value
            .parse::<VisibilityOverride>()
            .map(drop)
            .map_err(|()| "default, public, or private");
    }
    Err("a known response-policy key")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryOutcome {
    Regular,
    Ephemeral,
    DirectPrivate,
    Suppressed,
    PublicFallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryReceipt {
    pub outcome: DeliveryOutcome,
    pub group_message_id: Option<i32>,
}

#[derive(Clone, Copy, Debug, Default)]
struct SendOptions {
    receiver_override: Option<PeerRef>,
    only_if_private: bool,
}

fn observed(kind: ResponseKind, group: Option<i64>, outcome: DeliveryOutcome) -> DeliveryOutcome {
    log::debug!(
        "response_delivery kind={} group={group:?} delivery={outcome:?} success=true",
        kind.as_str()
    );
    outcome
}

fn mark_ephemeral_panel_callbacks(message: InputMessage) -> InputMessage {
    message
        .rewrite_callback_prefix(b'p', b'q')
        .rewrite_callback_prefix(b'h', b'k')
}

pub async fn send(
    client: &grammers_client::Client,
    settings: &Settings,
    trigger: &Message,
    kind: ResponseKind,
    audience: IntendedAudience,
    message: InputMessage,
) -> Result<DeliveryOutcome, Box<dyn std::error::Error + Send + Sync>> {
    Ok(send_inner(
        client,
        settings,
        trigger,
        kind,
        audience,
        message,
        SendOptions::default(),
    )
    .await?
    .outcome)
}

pub async fn send_tracked(
    client: &grammers_client::Client,
    settings: &Settings,
    trigger: &Message,
    kind: ResponseKind,
    audience: IntendedAudience,
    message: InputMessage,
) -> Result<DeliveryReceipt, Box<dyn std::error::Error + Send + Sync>> {
    send_inner(
        client,
        settings,
        trigger,
        kind,
        audience,
        message,
        SendOptions::default(),
    )
    .await
}

pub async fn send_tracked_to_user(
    client: &grammers_client::Client,
    settings: &Settings,
    trigger: &Message,
    receiver: PeerRef,
    kind: ResponseKind,
    audience: IntendedAudience,
    message: InputMessage,
) -> Result<DeliveryReceipt, Box<dyn std::error::Error + Send + Sync>> {
    if receiver.id.kind() != PeerKind::User {
        return Err("explicit ephemeral receiver is not a Telegram user".into());
    }
    send_inner(
        client,
        settings,
        trigger,
        kind,
        audience,
        message,
        SendOptions {
            receiver_override: Some(receiver),
            only_if_private: false,
        },
    )
    .await
}

pub async fn send_if_private(
    client: &grammers_client::Client,
    settings: &Settings,
    trigger: &Message,
    kind: ResponseKind,
    message: InputMessage,
) -> Result<DeliveryOutcome, Box<dyn std::error::Error + Send + Sync>> {
    Ok(send_inner(
        client,
        settings,
        trigger,
        kind,
        IntendedAudience::RequesterOnly,
        message,
        SendOptions {
            receiver_override: None,
            only_if_private: true,
        },
    )
    .await?
    .outcome)
}

async fn send_inner(
    client: &grammers_client::Client,
    settings: &Settings,
    trigger: &Message,
    kind: ResponseKind,
    audience: IntendedAudience,
    message: InputMessage,
    options: SendOptions,
) -> Result<DeliveryReceipt, Box<dyn std::error::Error + Send + Sync>> {
    let peer = trigger
        .peer_ref()
        .await?
        .ok_or("response peer is unavailable")?;
    let chat = peer.id.bot_api_dialog_id().unwrap_or_default();
    let receiver = if options.receiver_override.is_some() {
        options.receiver_override
    } else if trigger
        .sender_id()
        .is_some_and(|sender| sender.kind() == PeerKind::User)
        && !matches!(trigger.sender(), Some(Peer::User(user)) if user.is_bot())
    {
        trigger.sender_ref().await?
    } else {
        None
    };
    let is_group = matches!(peer.id.kind(), PeerKind::Chat | PeerKind::Channel);
    let group = is_group.then_some(chat);
    let choice = decision(
        settings,
        chat,
        kind,
        audience,
        DeliveryCapabilities {
            is_group,
            has_target_user: receiver.is_some(),
            callback_overlay: false,
        },
    );
    if options.only_if_private && matches!(choice, DeliveryDecision::Regular) {
        return Ok(receipt(kind, group, DeliveryOutcome::Suppressed, None));
    }

    match choice {
        DeliveryDecision::Regular => {
            let sent = client
                .send_message(peer, message.reply_to(Some(trigger.id())))
                .await?;
            Ok(receipt(
                kind,
                group,
                DeliveryOutcome::Regular,
                group.map(|_| sent.id()),
            ))
        }
        DeliveryDecision::DirectPrivate => {
            client
                .send_message(peer, message.reply_to(Some(trigger.id())))
                .await?;
            Ok(receipt(kind, group, DeliveryOutcome::DirectPrivate, None))
        }
        DeliveryDecision::Ephemeral { public_fallback } => {
            let public_fallback = public_fallback && !options.only_if_private;
            let Some(receiver) = receiver else {
                if public_fallback {
                    let sent = client
                        .send_message(peer, message.reply_to(Some(trigger.id())))
                        .await?;
                    return Ok(receipt(
                        kind,
                        group,
                        DeliveryOutcome::PublicFallback,
                        group.map(|_| sent.id()),
                    ));
                }
                return Ok(receipt(kind, group, DeliveryOutcome::Suppressed, None));
            };
            match client
                .send_ephemeral_message(
                    peer,
                    receiver,
                    None,
                    ephemeral_input(kind, trigger, message.clone()),
                )
                .await
            {
                Ok(()) => Ok(receipt(kind, group, DeliveryOutcome::Ephemeral, None)),
                Err(error) if public_fallback => {
                    log::warn!(
                        "response_delivery kind={} group={group:?} delivery=public_fallback reason=ephemeral_api error={error}",
                        kind.as_str(),
                    );
                    let sent = client
                        .send_message(peer, message.reply_to(Some(trigger.id())))
                        .await?;
                    Ok(receipt(
                        kind,
                        group,
                        DeliveryOutcome::PublicFallback,
                        group.map(|_| sent.id()),
                    ))
                }
                Err(error) => {
                    log::warn!(
                        "response_delivery kind={} group={group:?} delivery=suppressed reason=privacy_required_ephemeral_failed error={error}",
                        kind.as_str(),
                    );
                    match client.send_message(receiver, message).await {
                        Ok(_) => Ok(receipt(kind, group, DeliveryOutcome::DirectPrivate, None)),
                        Err(dm_error) => {
                            log::warn!(
                                "response_delivery kind={} group={group:?} delivery=suppressed reason=private_chat_failed error={dm_error}",
                                kind.as_str(),
                            );
                            Ok(receipt(kind, group, DeliveryOutcome::Suppressed, None))
                        }
                    }
                }
            }
        }
        DeliveryDecision::CallbackOverlay | DeliveryDecision::Suppress => {
            Ok(receipt(kind, group, DeliveryOutcome::Suppressed, None))
        }
    }
}

fn ephemeral_input(kind: ResponseKind, trigger: &Message, message: InputMessage) -> InputMessage {
    let message = mark_ephemeral_panel_callbacks(message);
    if kind == ResponseKind::ContentRemovalNotice {
        message
    } else {
        message.reply_to(Some(trigger.id()))
    }
}

fn receipt(
    kind: ResponseKind,
    group: Option<i64>,
    outcome: DeliveryOutcome,
    group_message_id: Option<i32>,
) -> DeliveryReceipt {
    DeliveryReceipt {
        outcome: observed(kind, group, outcome),
        group_message_id,
    }
}

pub async fn send_callback(
    client: &grammers_client::Client,
    settings: &Settings,
    query: &grammers_client::update::CallbackQuery,
    chat: i64,
    kind: ResponseKind,
    message: InputMessage,
) -> Result<DeliveryOutcome, Box<dyn std::error::Error + Send + Sync>> {
    send_callback_with_peer(client, settings, query, chat, kind, message, None).await
}

pub async fn send_callback_with_peer(
    client: &grammers_client::Client,
    settings: &Settings,
    query: &grammers_client::update::CallbackQuery,
    chat: i64,
    kind: ResponseKind,
    message: InputMessage,
    fallback_peer: Option<PeerRef>,
) -> Result<DeliveryOutcome, Box<dyn std::error::Error + Send + Sync>> {
    let peer = query
        .peer_ref()
        .await?
        .or(fallback_peer)
        .ok_or("callback peer is unavailable")?;
    let receiver = if query.sender_id().kind() == PeerKind::User {
        query.sender_ref().await?
    } else {
        None
    };
    let is_group = matches!(peer.id.kind(), PeerKind::Chat | PeerKind::Channel);
    let is_inline = matches!(
        query.raw(),
        grammers_client::tl::enums::Update::InlineBotCallbackQuery(_)
    );
    if is_group && peer.id.bot_api_dialog_id() != Some(chat) {
        return Err("callback policy chat does not match Telegram callback peer".into());
    }
    let group = is_group.then_some(chat);
    let choice = decision(
        settings,
        chat,
        kind,
        IntendedAudience::RequesterOnly,
        DeliveryCapabilities {
            is_group,
            has_target_user: receiver.is_some(),
            callback_overlay: false,
        },
    );
    match choice {
        DeliveryDecision::Regular => {
            edit_callback_message(client, query, &peer, message).await?;
            Ok(observed(kind, group, DeliveryOutcome::Regular))
        }
        DeliveryDecision::DirectPrivate => {
            if is_inline {
                query.answer().send().await?;
                let Some(receiver) = receiver else {
                    return Ok(observed(kind, group, DeliveryOutcome::Suppressed));
                };
                match client.send_message(receiver, message).await {
                    Ok(_) => Ok(observed(kind, group, DeliveryOutcome::DirectPrivate)),
                    Err(error) => {
                        log::warn!(
                            "response_delivery kind={} group={group:?} delivery=suppressed reason=inline_private_chat_failed error={error}",
                            kind.as_str(),
                        );
                        Ok(observed(kind, group, DeliveryOutcome::Suppressed))
                    }
                }
            } else {
                edit_callback_message(client, query, &peer, message).await?;
                Ok(observed(kind, group, DeliveryOutcome::DirectPrivate))
            }
        }
        DeliveryDecision::Ephemeral { public_fallback } => {
            let Some(receiver) = receiver else {
                let _ = query
                    .answer()
                    .alert("پاسخ خصوصی برای این نوع فرستنده در دسترس نیست.")
                    .send()
                    .await;
                return Ok(observed(kind, group, DeliveryOutcome::Suppressed));
            };
            match client
                .send_ephemeral_message(
                    peer,
                    receiver,
                    Some(query.query_id()),
                    mark_ephemeral_panel_callbacks(message.clone()),
                )
                .await
            {
                Ok(()) => Ok(observed(kind, group, DeliveryOutcome::Ephemeral)),
                Err(error) if public_fallback => {
                    log::warn!(
                        "response_delivery kind={} group={group:?} delivery=callback_edit_fallback reason=ephemeral_api error={error}",
                        kind.as_str(),
                    );
                    edit_callback_message(client, query, &peer, message).await?;
                    Ok(observed(kind, group, DeliveryOutcome::PublicFallback))
                }
                Err(error) => {
                    log::warn!(
                        "response_delivery kind={} group={group:?} delivery=suppressed reason=privacy_required_callback_failed error={error}",
                        kind.as_str(),
                    );
                    match client.send_message(receiver, message).await {
                        Ok(_) => {
                            let _ = query.answer().send().await;
                            Ok(observed(kind, group, DeliveryOutcome::DirectPrivate))
                        }
                        Err(dm_error) => {
                            log::warn!(
                                "response_delivery kind={} group={group:?} delivery=suppressed reason=private_chat_failed error={dm_error}",
                                kind.as_str(),
                            );
                            let _ = query
                                .answer()
                                .alert("پاسخ خصوصی در دسترس نیست؛ ابتدا ربات را در پیوی شروع کنید.")
                                .send()
                                .await;
                            Ok(observed(kind, group, DeliveryOutcome::Suppressed))
                        }
                    }
                }
            }
        }
        DeliveryDecision::CallbackOverlay | DeliveryDecision::Suppress => {
            Ok(observed(kind, group, DeliveryOutcome::Suppressed))
        }
    }
}

pub async fn redraw_panel_callback(
    client: &grammers_client::Client,
    query: &grammers_client::update::CallbackQuery,
    chat: i64,
    kind: ResponseKind,
    message: InputMessage,
    fallback_peer: Option<PeerRef>,
    ephemeral_origin: bool,
) -> Result<DeliveryOutcome, Box<dyn std::error::Error + Send + Sync>> {
    let peer = query
        .peer_ref()
        .await?
        .or(fallback_peer)
        .ok_or("callback peer is unavailable")?;
    let is_group = matches!(peer.id.kind(), PeerKind::Chat | PeerKind::Channel);
    if is_group && peer.id.bot_api_dialog_id() != Some(chat) {
        return Err("callback panel chat does not match Telegram callback peer".into());
    }
    let group = is_group.then_some(chat);
    if !ephemeral_origin {
        edit_callback_message(client, query, &peer, message).await?;
        return Ok(observed(kind, group, DeliveryOutcome::Regular));
    }

    let receiver = if query.sender_id().kind() == PeerKind::User {
        query.sender_ref().await?
    } else {
        None
    };
    let Some(receiver) = receiver else {
        let _ = query
            .answer()
            .alert("پاسخ خصوصی برای این نوع فرستنده در دسترس نیست.")
            .send()
            .await;
        return Ok(observed(kind, group, DeliveryOutcome::Suppressed));
    };
    let Some(message_id) = query.ephemeral_message_id() else {
        let _ = query
            .answer()
            .alert("شناسه پنل خصوصی در دسترس نیست؛ پنل را دوباره باز کنید.")
            .send()
            .await;
        return Ok(observed(kind, group, DeliveryOutcome::Suppressed));
    };
    let message = mark_ephemeral_panel_callbacks(message);
    match client
        .edit_ephemeral_message(peer, receiver, message_id, message.clone())
        .await
    {
        Ok(()) => {
            if let Err(error) = query.answer().send().await {
                log::warn!(
                    "response_delivery kind={} group={group:?} delivery=ephemeral_edit reason=callback_ack_failed error={error}",
                    kind.as_str(),
                );
            }
            Ok(observed(kind, group, DeliveryOutcome::Ephemeral))
        }
        Err(error) => {
            log::warn!(
                "response_delivery kind={} group={group:?} delivery=ephemeral_replacement reason=ephemeral_panel_edit_failed error={error}",
                kind.as_str(),
            );
            match client
                .send_ephemeral_message(peer, receiver, Some(query.query_id()), message)
                .await
            {
                Ok(()) => Ok(observed(kind, group, DeliveryOutcome::Ephemeral)),
                Err(replacement_error) => {
                    log::warn!(
                        "response_delivery kind={} group={group:?} delivery=suppressed reason=ephemeral_panel_edit_and_replacement_failed edit_error={error} replacement_error={replacement_error}",
                        kind.as_str(),
                    );
                    let _ = query
                        .answer()
                        .alert("پنل خصوصی تازه نشد؛ دوباره پنل را باز کنید.")
                        .send()
                        .await;
                    Ok(observed(kind, group, DeliveryOutcome::Suppressed))
                }
            }
        }
    }
}

async fn edit_callback_message(
    client: &grammers_client::Client,
    query: &grammers_client::update::CallbackQuery,
    peer: &PeerRef,
    message: InputMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let grammers_client::tl::enums::Update::BotCallbackQuery(update) = query.raw() {
        query.answer().send().await?;
        client.edit_message(*peer, update.msg_id, message).await?;
    } else {
        query.answer().edit(message).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(values: &[(&str, &str)]) -> HashMap<String, String> {
        values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn decide(values: &[(&str, &str)], kind: ResponseKind, target: bool) -> DeliveryDecision {
        let values = settings(values);
        decide_with_settings(
            values
                .get(MODE_KEY)
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            &ChatSettings(Some(&values)),
            kind,
            IntendedAudience::RequesterOnly,
            DeliveryCapabilities {
                is_group: true,
                has_target_user: target,
                callback_overlay: false,
            },
        )
    }

    #[test]
    fn only_notice_and_welcome_can_be_selected_private() {
        for kind in [
            ResponseKind::CommandError,
            ResponseKind::PermissionDenied,
            ResponseKind::SettingsView,
            ResponseKind::Help,
            ResponseKind::AdminTool,
            ResponseKind::Statistics,
            ResponseKind::ModerationConfirmation,
            ResponseKind::UtilityResult,
        ] {
            assert_eq!(
                decide(
                    &[
                        (MODE_KEY, "private_first"),
                        ("response:category:help", "private"),
                        ("response:kind:help", "private"),
                    ],
                    kind,
                    true,
                ),
                DeliveryDecision::Regular,
                "{} must retain normal delivery",
                kind.as_str()
            );
        }
        for kind in [
            ResponseKind::ContentRemovalNotice,
            ResponseKind::WelcomeNotice,
        ] {
            let key = format!("response:kind:{}", kind.as_str());
            assert_eq!(
                decide(&[(key.as_str(), "private")], kind, true),
                DeliveryDecision::Ephemeral {
                    public_fallback: false
                }
            );
            assert_eq!(decide(&[], kind, true), DeliveryDecision::Regular);
        }
    }

    #[test]
    fn public_and_privacy_constraints_still_win() {
        assert_eq!(
            decide(
                &[("response:kind:welcome_notice", "private")],
                ResponseKind::WelcomeNotice,
                false,
            ),
            DeliveryDecision::Suppress
        );
        assert_eq!(
            decide(&[], ResponseKind::ModerationAnnouncement, true),
            DeliveryDecision::Regular
        );
        assert_eq!(
            decide(&[], ResponseKind::PersonalInformation, true,),
            DeliveryDecision::Ephemeral {
                public_fallback: false
            }
        );
    }

    #[test]
    fn missing_or_anonymous_target_never_leaks_privacy_required_content() {
        assert_eq!(
            decide(
                &[(MODE_KEY, "private_first")],
                ResponseKind::ModerationCaseDetails,
                false,
            ),
            DeliveryDecision::Suppress
        );
        assert_eq!(
            decide(&[(MODE_KEY, "private_first")], ResponseKind::Help, false,),
            DeliveryDecision::Regular
        );
    }

    #[test]
    fn group_policies_are_isolated_and_shared_messages_stay_public() {
        let group_a = settings(&[("response:kind:content_removal_notice", "private")]);
        let group_b = settings(&[]);
        let capabilities = DeliveryCapabilities {
            is_group: true,
            has_target_user: true,
            callback_overlay: false,
        };
        assert_eq!(
            decide_with_settings(
                NoiseMode::Off,
                &ChatSettings(Some(&group_a)),
                ResponseKind::ContentRemovalNotice,
                IntendedAudience::RequesterOnly,
                capabilities,
            ),
            DeliveryDecision::Ephemeral {
                public_fallback: false
            }
        );
        assert_eq!(
            decide_with_settings(
                NoiseMode::Off,
                &ChatSettings(Some(&group_b)),
                ResponseKind::ContentRemovalNotice,
                IntendedAudience::RequesterOnly,
                capabilities,
            ),
            DeliveryDecision::Regular
        );
        assert_eq!(
            decide_with_settings(
                NoiseMode::Off,
                &ChatSettings(Some(&group_a)),
                ResponseKind::ContentRemovalNotice,
                IntendedAudience::SharedWorkflow,
                capabilities,
            ),
            DeliveryDecision::Regular
        );
    }

    #[test]
    fn callback_feedback_prefers_existing_private_overlay() {
        let values = settings(&[]);
        assert_eq!(
            decide_with_settings(
                NoiseMode::Off,
                &ChatSettings(Some(&values)),
                ResponseKind::CallbackFeedback,
                IntendedAudience::RequesterOnly,
                DeliveryCapabilities {
                    is_group: true,
                    has_target_user: true,
                    callback_overlay: true,
                },
            ),
            DeliveryDecision::CallbackOverlay
        );
    }

    #[test]
    fn policy_keys_are_typed_and_allowlisted() {
        assert_eq!(ResponseKind::OVERRIDABLE.len(), 2);
        assert!(ResponseKind::OVERRIDABLE.contains(&ResponseKind::ContentRemovalNotice));
        assert!(ResponseKind::OVERRIDABLE.contains(&ResponseKind::WelcomeNotice));
        assert!(validate_setting(MODE_KEY, "balanced").is_ok());
        assert!(validate_setting("response:category:help", "private").is_ok());
        assert!(validate_setting("response:kind:help", "public").is_ok());
        assert!(validate_setting("response:kind:welcome_notice", "private").is_ok());
        assert!(validate_setting("response:category:rust_module", "private").is_err());
        assert!(validate_setting("response:kind:audit_log", "private").is_err());
        assert!(validate_setting(MODE_KEY, "everything_private").is_err());
    }
}
