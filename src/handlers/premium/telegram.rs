use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};

use grammers_client::message::{Button, InputMessage, ReplyMarkup};
use grammers_client::{Client, parsers, tl};

use super::Icon;

#[derive(Default)]
pub(super) struct Renderer {
    pub messages: bool,
    pub native_buttons: bool,
    pub channels: bool,
    pub alts: HashMap<i64, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Message,
    Caption,
    Edit,
    Channel,
}

static RENDERER: OnceLock<Renderer> = OnceLock::new();
static FALLBACK: LazyLock<Renderer> = LazyLock::new(Renderer::default);

fn renderer() -> &'static Renderer {
    RENDERER.get().unwrap_or(&FALLBACK)
}

fn parse_switch(name: &str, value: Option<&str>, default: bool) -> Result<bool, String> {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        None => Ok(default),
        Some("1" | "true" | "on") => Ok(true),
        Some("0" | "false" | "off") => Ok(false),
        Some(value) => Err(format!(
            "{name} must be one of 1/true/on or 0/false/off, got {value:?}"
        )),
    }
}

fn configured_switch(name: &str, default: bool) -> Result<bool, String> {
    match std::env::var(name) {
        Ok(value) => parse_switch(name, Some(&value), default),
        Err(std::env::VarError::NotPresent) => parse_switch(name, None, default),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not valid Unicode")),
    }
}

pub async fn initialize(client: &Client) -> Result<(), String> {
    let messages = configured_switch("PREMIUM_EMOJI", true)?;
    let mut renderer = Renderer {
        messages,
        native_buttons: messages && configured_switch("PREMIUM_EMOJI_BUTTONS", true)?,
        channels: configured_switch("PREMIUM_EMOJI_CHANNELS", false)?,
        ..Default::default()
    };
    if renderer.messages || renderer.native_buttons || renderer.channels {
        let document_id: Vec<i64> = Icon::ALL
            .iter()
            .filter_map(|icon| icon.document_id())
            .collect();
        let request = tl::functions::messages::GetCustomEmojiDocuments { document_id };
        match tokio::time::timeout(std::time::Duration::from_secs(5), client.invoke(&request)).await
        {
            Ok(Ok(documents)) => {
                for document in documents {
                    if let tl::enums::Document::Document(document) = document {
                        for attribute in document.attributes {
                            if let tl::enums::DocumentAttribute::CustomEmoji(attribute) = attribute
                                && !attribute.alt.is_empty()
                            {
                                renderer.alts.insert(document.id, attribute.alt);
                            }
                        }
                    }
                }
                log::info!("premium emoji: validated {} documents", renderer.alts.len());
            }
            _ => log::warn!("premium emoji metadata unavailable; using semantic Unicode fallbacks"),
        }
    }
    RENDERER
        .set(renderer)
        .map_err(|_| "premium emoji renderer was initialized more than once".to_owned())
}

impl Renderer {
    fn custom(&self, icon: Icon) -> Option<(i64, &str)> {
        let id = icon.document_id()?;
        Some((id, self.alts.get(&id)?.as_str()))
    }

    pub(super) fn badge(&self, icon: Option<Icon>) -> String {
        self.badge_with(icon, self.messages)
    }

    pub(super) fn badge_with(&self, icon: Option<Icon>, custom_enabled: bool) -> String {
        let Some(icon) = icon else {
            return String::new();
        };
        if custom_enabled && let Some((_, alt)) = self.custom(icon) {
            format!(
                "<a href=\"tg-icon:{}\">{}</a>",
                icon.key(),
                super::super::esc(alt)
            )
        } else {
            super::super::esc(icon.fallback())
        }
    }

    pub(super) fn parse(&self, message: &str) -> (String, Vec<tl::enums::MessageEntity>) {
        self.parse_with(message, self.messages)
    }

    pub(super) fn parse_with(
        &self,
        message: &str,
        custom_enabled: bool,
    ) -> (String, Vec<tl::enums::MessageEntity>) {
        let (text, entities) = parsers::parse_html_message(message);
        let mut entities: Vec<_> = entities
            .into_iter()
            .filter_map(|entity| {
                if let tl::enums::MessageEntity::TextUrl(link) = &entity
                    && let Some(key) = link.url.strip_prefix("tg-icon:")
                {
                    let icon = Icon::from_key(key)?;
                    let (document_id, alt) = self.custom(icon).filter(|_| custom_enabled)?;
                    let utf16: Vec<u16> = text.encode_utf16().collect();
                    let start = usize::try_from(link.offset).ok()?;
                    let end = start.checked_add(usize::try_from(link.length).ok()?)?;
                    if utf16.get(start..end)? != alt.encode_utf16().collect::<Vec<_>>() {
                        return None;
                    }
                    return Some(
                        tl::types::MessageEntityCustomEmoji {
                            offset: link.offset,
                            length: link.length,
                            document_id,
                        }
                        .into(),
                    );
                }
                Some(entity)
            })
            .collect();
        entities.sort_by_key(|entity| {
            bounds(entity).map(|(offset, length)| (offset, std::cmp::Reverse(length)))
        });
        (text, entities)
    }

    pub(super) fn decorate(&self, button: Button, icon: Option<Icon>) -> Button {
        let Some(icon) = icon else {
            return button;
        };
        let document = self
            .custom(icon)
            .filter(|_| self.native_buttons)
            .map(|(id, _)| id);
        let apply = |text: &mut String, style: &mut Option<tl::enums::KeyboardButtonStyle>| {
            if let Some(id) = document {
                let mut value = match style.take() {
                    Some(tl::enums::KeyboardButtonStyle::Style(style)) => style,
                    None => tl::types::KeyboardButtonStyle {
                        bg_primary: false,
                        bg_danger: false,
                        bg_success: false,
                        icon: None,
                    },
                };
                value.icon = Some(id);
                *style = Some(value.into());
            } else {
                *text = format!("{} {}", icon.fallback(), text);
            }
        };
        let raw = match button.raw {
            tl::enums::KeyboardButton::Callback(mut b) => {
                apply(&mut b.text, &mut b.style);
                b.into()
            }
            tl::enums::KeyboardButton::Url(mut b) => {
                apply(&mut b.text, &mut b.style);
                b.into()
            }
            other => other,
        };
        Button { raw }
    }
}

pub fn html(message: impl AsRef<str>) -> InputMessage {
    let renderer = renderer();
    let (text, entities) = renderer.parse(message.as_ref());
    InputMessage::new().text(text).fmt_entities(entities)
}

pub fn text(message: impl Into<String>) -> InputMessage {
    InputMessage::new().text(message)
}

pub fn badge(icon: Option<Icon>) -> String {
    let renderer = renderer();
    renderer.badge(icon)
}

pub fn icon_html(icon: Option<Icon>, message: impl AsRef<str>) -> InputMessage {
    let prefix = badge(icon);
    if prefix.is_empty() {
        html(message)
    } else {
        html(format!("{prefix} {}", message.as_ref()))
    }
}

pub fn icon_html_on(
    surface: Surface,
    icon: Option<Icon>,
    message: impl AsRef<str>,
) -> InputMessage {
    let renderer = renderer();
    let custom = match surface {
        Surface::Message | Surface::Caption | Surface::Edit => renderer.messages,
        Surface::Channel => renderer.channels,
    };
    let prefix = renderer.badge_with(icon, custom);
    let source = if prefix.is_empty() {
        message.as_ref().to_owned()
    } else {
        format!("{prefix} {}", message.as_ref())
    };
    let (text, entities) = renderer.parse_with(&source, custom);
    InputMessage::new().text(text).fmt_entities(entities)
}

pub fn plain_label(icon: Option<Icon>, text: impl AsRef<str>) -> String {
    icon.map_or_else(
        || text.as_ref().to_owned(),
        |icon| format!("{} {}", icon.fallback(), text.as_ref()),
    )
}

pub fn icon_text(icon: Option<Icon>, message: impl AsRef<str>) -> InputMessage {
    icon_html(icon, super::super::esc(message.as_ref()))
}

pub fn decorate(button: Button, icon: Option<Icon>) -> Button {
    renderer().decorate(button, icon)
}

pub fn buttons(rows: &[Vec<Button>]) -> ReplyMarkup {
    ReplyMarkup::from_buttons(rows)
}

fn bounds(entity: &tl::enums::MessageEntity) -> Option<(i32, i32)> {
    use tl::enums::MessageEntity as E;
    Some(match entity {
        E::Unknown(e) => (e.offset, e.length),
        E::Mention(e) => (e.offset, e.length),
        E::Hashtag(e) => (e.offset, e.length),
        E::BotCommand(e) => (e.offset, e.length),
        E::Url(e) => (e.offset, e.length),
        E::Email(e) => (e.offset, e.length),
        E::Bold(e) => (e.offset, e.length),
        E::Italic(e) => (e.offset, e.length),
        E::Code(e) => (e.offset, e.length),
        E::Pre(e) => (e.offset, e.length),
        E::TextUrl(e) => (e.offset, e.length),
        E::MentionName(e) => (e.offset, e.length),
        E::InputMessageEntityMentionName(e) => (e.offset, e.length),
        E::Phone(e) => (e.offset, e.length),
        E::Cashtag(e) => (e.offset, e.length),
        E::Underline(e) => (e.offset, e.length),
        E::Strike(e) => (e.offset, e.length),
        E::BankCard(e) => (e.offset, e.length),
        E::Spoiler(e) => (e.offset, e.length),
        E::CustomEmoji(e) => (e.offset, e.length),
        E::Blockquote(e) => (e.offset, e.length),
        _ => return None,
    })
}

#[cfg(test)]
mod config_tests {
    use super::parse_switch;

    #[test]
    fn switches_default_only_when_absent_and_reject_typos() {
        assert!(parse_switch("FEATURE", None, true).unwrap());
        assert!(!parse_switch("FEATURE", None, false).unwrap());
        assert!(parse_switch("FEATURE", Some("TRUE"), false).unwrap());
        assert!(!parse_switch("FEATURE", Some(" off "), true).unwrap());
        assert!(parse_switch("FEATURE", Some("enabled"), false).is_err());
        assert!(parse_switch("FEATURE", Some(""), false).is_err());
    }
}
