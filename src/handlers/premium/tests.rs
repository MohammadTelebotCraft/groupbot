use super::telegram::Renderer;
use super::*;
use grammers_client::message::{Button, InputMessage};
use grammers_client::tl;

fn custom_renderer() -> Renderer {
    Renderer {
        messages: true,
        native_buttons: true,
        channels: false,
        alts: Icon::ALL
            .iter()
            .filter_map(|icon| Some((icon.document_id()?, icon.entry().placeholder.clone())))
            .collect(),
    }
}

fn spans(entities: &[tl::enums::MessageEntity]) -> Vec<(i32, i32, i64)> {
    entities
        .iter()
        .filter_map(|e| match e {
            tl::enums::MessageEntity::CustomEmoji(e) => Some((e.offset, e.length, e.document_id)),
            _ => None,
        })
        .collect()
}

#[test]
fn registry_is_complete_unique_and_lossless() {
    use std::collections::BTreeSet;
    let mut keys = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for entry in registry() {
        assert!(keys.insert(&entry.key));
        assert!(!entry.fallback.is_empty());
        assert!(!entry.source.is_empty());
        if let Some(id) = &entry.custom_emoji_id {
            assert_eq!(id.parse::<i64>().unwrap().to_string(), *id);
            assert!(ids.insert(id));
        }
        if entry.usage == "ACTIVE" {
            assert_eq!(entry.confidence, Confidence::High);
            assert!(!entry.semantic_tags.is_empty());
        }
    }
    assert_eq!(ids.len(), 90);
    for icon in Icon::ALL {
        assert_eq!(Icon::from_key(icon.key()), Some(*icon));
    }
}

#[test]
fn supplied_visual_corrections_are_not_legacy_guesses() {
    assert_eq!(Icon::Unlocked.document_id(), Some(5258476306152038031));
    assert_eq!(Icon::TelegramSend.document_id(), Some(5981194327110456280));
    assert_eq!(Icon::ImageDisabled.document_id(), Some(5776239565782126896));
    assert_eq!(Icon::ErrorRed.document_id(), Some(5819154526816444042));
    assert_eq!(Icon::Close.document_id(), Some(5219776129669276751));
    assert_ne!(Icon::ErrorRed.document_id(), Icon::Close.document_id());
    for icon in [
        Icon::Members,
        Icon::ImagePrivate,
        Icon::Invisible,
        Icon::Network,
        Icon::Briefcase,
        Icon::Diamond,
    ] {
        assert_eq!(icon.document_id(), None);
    }
}

#[test]
fn semantic_selection_covers_moderation_actions_and_state() {
    use Icon::*;
    let cases = [
        ("ban", "user", "", ModerationHammer),
        ("kick", "user", "", ModerationHammer),
        ("mute", "user", "active", Muted),
        ("warn", "user", "", Warning),
        ("unmute", "user", "", Unlocked),
        ("unban", "user", "", Unlocked),
        ("restrict", "images", "", ImageDisabled),
        ("view", "logs", "", DocumentActivity),
        ("view", "history", "", ArchiveHistory),
        ("view", "user", "", User),
        ("pin", "message", "", Pin),
        ("", "anti_sell_trade", "enabled", Locked),
        ("", "anti_sell_trade", "disabled", Unlocked),
        ("", "cooldown", "", Timer),
        ("", "temporary_restriction", "active", Timer),
        ("", "", "waiting", Hourglass),
        ("", "automod", "paused", Pause),
        ("cancel", "", "", Close),
        ("mute", "user", "failed", ErrorRed),
        ("ban", "user", "pending", Hourglass),
        ("save", "", "success", Success),
    ];
    for (action, object, state, expected) in cases {
        assert_eq!(
            icon_for(Context {
                action,
                object,
                state,
                ..Default::default()
            }),
            Some(expected)
        );
    }
    assert_eq!(icon_for(Context::default()), None);
    assert_eq!(
        icon_for(Context {
            object: "notifications",
            state: "off",
            ..Default::default()
        }),
        None
    );
}

#[test]
fn severity_is_relevant_only_to_an_actual_flood_or_raid() {
    for object in ["anti_flood", "raid"] {
        assert_eq!(
            icon_for(Context {
                object,
                state: "triggered",
                severity: "high",
                ..Default::default()
            }),
            Some(Icon::Fire)
        );
        assert_ne!(
            icon_for(Context {
                object,
                state: "enabled",
                severity: "high",
                ..Default::default()
            }),
            Some(Icon::Fire)
        );
    }
    assert_eq!(section_icon("s"), Some(Icon::ModerationHammer));
    assert_eq!(section_icon("fl"), Some(Icon::Timer));
}

#[test]
fn media_permissions_have_distinct_states() {
    for (object, off, on) in [
        ("photos", Icon::ImageDisabled, Icon::Image),
        ("voices", Icon::MicMuted, Icon::Voice),
        ("audios", Icon::SpeakerMuted, Icon::Speaker),
        ("plain", Icon::Locked, Icon::Unlocked),
    ] {
        assert_eq!(permission(object, false), off);
        assert_eq!(permission(object, true), on);
        assert_eq!(
            icon_for(Context {
                object,
                scope: "permission",
                state: "blocked",
                ..Default::default()
            }),
            Some(off)
        );
    }
    assert_eq!(lock_icon("trade", true), Icon::Locked);
    assert_eq!(lock_icon("trade", false), Icon::Unlocked);
}

#[test]
fn english_persian_and_mixed_text_keep_the_same_semantics() {
    let renderer = custom_renderer();
    for body in [
        "Mute",
        "سکوت کاربر",
        "سکوت user42 برای ۳۰ دقیقه",
        "🔕 Alice <&> 🐳",
    ] {
        let markup = format!(
            "{} {}",
            renderer.badge(Some(Icon::Muted)),
            super::super::esc(body)
        );
        let (text, entities) = renderer.parse(&markup);
        assert_eq!(text, format!("{} {body}", Icon::Muted.entry().placeholder));
        assert_eq!(
            spans(&entities),
            vec![(0, 2, Icon::Muted.document_id().unwrap())]
        );
    }
}

#[test]
fn utf16_offsets_survive_astral_emoji_and_rtl_with_multiple_icons() {
    let renderer = custom_renderer();
    let prefix = "a😀 فارسی ";
    let source = format!(
        "{prefix}{} <b>کاربر</b> {}",
        renderer.badge(Some(Icon::Muted)),
        renderer.badge(Some(Icon::Pin))
    );
    let (text, entities) = renderer.parse(&source);
    let expected_first = prefix.encode_utf16().count() as i32;
    let expected_last = text[..text.rfind('📌').unwrap()].encode_utf16().count() as i32;
    assert_eq!(
        spans(&entities),
        vec![
            (expected_first, 2, Icon::Muted.document_id().unwrap()),
            (expected_last, 2, Icon::Pin.document_id().unwrap())
        ]
    );
    assert!(
        entities
            .iter()
            .any(|e| matches!(e, tl::enums::MessageEntity::Bold(_)))
    );
}

#[test]
fn actual_document_alt_is_distinct_from_semantic_fallback() {
    let mut renderer = custom_renderer();
    renderer
        .alts
        .insert(Icon::TelegramSend.document_id().unwrap(), "🚀".to_owned());
    let (text, entities) = renderer.parse(&renderer.badge(Some(Icon::TelegramSend)));
    assert_eq!(text, "🚀");
    assert_eq!(spans(&entities)[0].1, 2);
    assert_eq!(Renderer::default().badge(Some(Icon::TelegramSend)), "📨");
    assert!(spans(&renderer.parse("<a href=\"tg-icon:TELEGRAM_SEND\">x</a>").1).is_empty());
}

#[test]
fn no_rewriting_of_names_quotes_patterns_or_plain_emoji() {
    let renderer = custom_renderer();
    let user = "🔒 😡 نام <a href=\"tg-icon:MUTED\">🔕</a> ✅ [🔥.*]";
    let source = format!(
        "<blockquote>{}</blockquote><code>^🔒.*$</code>",
        super::super::esc(user)
    );
    let (text, entities) = renderer.parse(&source);
    assert_eq!(text, format!("{user}^🔒.*$"));
    assert!(spans(&entities).is_empty());
    assert_eq!(entities.len(), 2);
}

#[test]
fn missing_metadata_and_disabled_support_have_readable_fallbacks() {
    let fallback = Renderer::default();
    assert_eq!(fallback.badge(Some(Icon::Muted)), "🔕");
    assert_eq!(fallback.badge(None), "");
    let no_metadata = Renderer {
        messages: true,
        native_buttons: true,
        ..Default::default()
    };
    assert_eq!(no_metadata.badge(Some(Icon::Locked)), "🔒");
    assert_eq!(custom_renderer().badge(Some(Icon::Members)), "👥");
}

#[test]
fn channel_entities_are_independent_of_message_entities() {
    let mut renderer = custom_renderer();
    renderer.messages = false;
    renderer.channels = true;
    let markup = renderer.badge_with(Some(Icon::Muted), renderer.channels);
    let (text, entities) = renderer.parse_with(&markup, renderer.channels);
    assert_eq!(text, Icon::Muted.entry().placeholder);
    assert_eq!(
        spans(&entities),
        vec![(0, 2, Icon::Muted.document_id().unwrap())]
    );
    assert_eq!(renderer.badge(Some(Icon::Muted)), Icon::Muted.fallback());
}

#[test]
fn native_buttons_preserve_payload_text_and_colour() {
    let renderer = custom_renderer();
    let payload = vec![0, 255, 58, 1];
    let button = super::super::style::paint(
        Button::data("سکوت Alice 🔕", payload.clone()),
        super::super::style::Colour::Danger,
    );
    let button = renderer.decorate(button, Some(Icon::Muted));
    let button = super::super::style::paint(button, super::super::style::Colour::Success);
    let tl::enums::KeyboardButton::Callback(b) = button.raw else {
        panic!("callback")
    };
    assert_eq!(b.data, payload);
    assert_eq!(b.text, "سکوت Alice 🔕");
    let Some(tl::enums::KeyboardButtonStyle::Style(style)) = b.style else {
        panic!("style")
    };
    assert_eq!(style.icon, Icon::Muted.document_id());
    assert!(style.bg_success);
    assert!(!style.bg_danger);
}

#[test]
fn url_and_fallback_buttons_preserve_destinations() {
    for renderer in [custom_renderer(), Renderer::default()] {
        let url = "https://t.me/example?startgroup=new";
        let b = renderer.decorate(Button::url("ارسال", url), Some(Icon::TelegramSend));
        let tl::enums::KeyboardButton::Url(b) = b.raw else {
            panic!("URL")
        };
        assert_eq!(b.url, url);
        assert_eq!(
            b.text,
            if renderer.native_buttons {
                "ارسال"
            } else {
                "📨 ارسال"
            }
        );
    }
}

#[test]
fn captcha_presets_and_unknown_context_are_unchanged() {
    let renderer = custom_renderer();
    for label in ["🐳", "۱۲", "🔒 user's filter"] {
        let b = renderer.decorate(Button::data(label, b"c:7:2"), None);
        let tl::enums::KeyboardButton::Callback(b) = b.raw else {
            panic!("callback")
        };
        assert_eq!(b.text, label);
        assert_eq!(b.data, b"c:7:2");
        assert!(b.style.is_none());
    }
}

#[test]
fn edited_messages_and_captions_use_the_same_entity_builder() {
    let renderer = custom_renderer();
    let markup = format!("{} <b>عکس</b> 🐳", renderer.badge(Some(Icon::Image)));
    let (text, entities) = renderer.parse(&markup);
    let original = InputMessage::new()
        .text(&text)
        .fmt_entities(entities.clone());
    let _caption = original.clone().reply_to(Some(42));
    let _edited = InputMessage::new()
        .text(&text)
        .fmt_entities(entities.clone());
    assert_eq!(renderer.parse(&markup).0, text);
    assert_eq!(spans(&renderer.parse(&markup).1), spans(&entities));
}

#[test]
fn browser_fallback_registry_contains_no_document_ids() {
    let exported = web_fallbacks();
    let map: std::collections::BTreeMap<String, String> = serde_json::from_str(&exported).unwrap();
    assert_eq!(map["MUTED"], "🔕");
    assert_eq!(map["CLOSE"], "✕");
    for row in registry() {
        if let Some(id) = &row.custom_emoji_id {
            assert!(!exported.contains(id));
        }
    }
}
