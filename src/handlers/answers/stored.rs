use base64::{Engine, engine::general_purpose::STANDARD};
use grammers_client::message::InputMessage;
use grammers_client::tl::{self, Deserializable, Serializable};
use serde::{Deserialize, Serialize};

use super::super::welcome::{bounds, placed};

const VERSION: &str = "answer-v1|";

#[derive(Debug)]
pub(super) struct StoredAnswer {
    pub(super) media: String,
    pub(super) body: String,
    entities: Vec<tl::enums::MessageEntity>,
}

#[derive(Serialize, Deserialize)]
struct WireAnswer {
    media: String,
    body: String,
    entities: String,
}

impl StoredAnswer {
    pub(super) fn capture(
        media: String,
        source: &str,
        body: &str,
        entities: &[tl::enums::MessageEntity],
        inline_html: bool,
    ) -> Self {
        let start = body.as_ptr() as usize - source.as_ptr() as usize;
        let shift = source[..start].encode_utf16().count() as i32;
        let carried = slice_entities(entities, shift, body.encode_utf16().count() as i32);
        if inline_html && !carried.iter().any(explicit_formatting) {
            return Self::html(media, body);
        }
        Self {
            media,
            body: body.to_owned(),
            entities: carried,
        }
    }

    pub(super) fn media_only(media: String) -> Self {
        Self {
            media,
            body: String::new(),
            entities: Vec::new(),
        }
    }

    fn html(media: String, html: &str) -> Self {
        let (body, entities) = grammers_client::parsers::parse_html_message(html);
        Self {
            media,
            body,
            entities,
        }
    }

    pub(super) fn encode(&self) -> String {
        let wire = WireAnswer {
            media: self.media.clone(),
            body: self.body.clone(),
            entities: STANDARD.encode(self.entities.to_bytes()),
        };
        format!(
            "{VERSION}{}",
            serde_json::to_string(&wire).expect("answer fields are strings")
        )
    }

    pub(super) fn decode(stored: &str) -> Option<Self> {
        if let Some(json) = stored.strip_prefix(VERSION) {
            let wire: WireAnswer = serde_json::from_str(json).ok()?;
            let bytes = STANDARD.decode(&wire.entities).ok()?;
            let entities = Vec::<tl::enums::MessageEntity>::from_bytes(&bytes).ok()?;
            return Some(Self {
                media: wire.media,
                body: wire.body,
                entities,
            });
        }
        let (media, body) = stored.split_once('|').unwrap_or(("", stored));
        Some(Self::html(media.to_owned(), body))
    }

    pub(super) fn message(&self) -> InputMessage {
        InputMessage::new()
            .text(self.body.clone())
            .fmt_entities(self.entities.clone())
    }
}

fn explicit_formatting(entity: &tl::enums::MessageEntity) -> bool {
    use tl::enums::MessageEntity as E;
    matches!(
        entity,
        E::Bold(_)
            | E::Italic(_)
            | E::Underline(_)
            | E::Strike(_)
            | E::Code(_)
            | E::Pre(_)
            | E::Spoiler(_)
            | E::Blockquote(_)
            | E::CustomEmoji(_)
            | E::TextUrl(_)
            | E::MentionName(_)
            | E::InputMessageEntityMentionName(_)
            | E::FormattedDate(_)
            | E::DiffInsert(_)
            | E::DiffReplace(_)
            | E::DiffDelete(_)
    )
}

fn slice_entities(
    entities: &[tl::enums::MessageEntity],
    shift: i32,
    length: i32,
) -> Vec<tl::enums::MessageEntity> {
    use tl::enums::MessageEntity as E;
    let end = shift + length;
    entities
        .iter()
        .filter_map(|entity| {
            let (offset, count) = bounds(entity)?;
            if offset < 0 || count <= 0 {
                return None;
            }
            let entity_end = offset.checked_add(count)?;
            let clippable = matches!(
                entity,
                E::Bold(_)
                    | E::Italic(_)
                    | E::Underline(_)
                    | E::Strike(_)
                    | E::Code(_)
                    | E::Pre(_)
                    | E::Spoiler(_)
                    | E::Blockquote(_)
            );
            if !clippable && (offset < shift || entity_end > end) {
                return None;
            }
            let from = offset.max(shift);
            let to = entity_end.min(end);
            (from < to).then(|| placed(entity, from - shift, to - from))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl::enums::MessageEntity as E;
    use tl::types::*;

    fn styles(offset: i32, length: i32) -> Vec<E> {
        vec![
            MessageEntityBold { offset, length }.into(),
            MessageEntityItalic { offset, length }.into(),
            MessageEntityUnderline { offset, length }.into(),
            MessageEntityStrike { offset, length }.into(),
            MessageEntityCode { offset, length }.into(),
            MessageEntityPre {
                offset,
                length,
                language: "rust".to_owned(),
            }
            .into(),
            MessageEntitySpoiler { offset, length }.into(),
            MessageEntityBlockquote {
                offset,
                length,
                collapsed: true,
            }
            .into(),
        ]
    }

    #[test]
    fn all_text_styles_survive_storage_and_clip_to_the_inline_body() {
        let source = "  تنظيم\u{200c}پاسخ 😀 =  Hello 🦀 | <literal>  ";
        let body = source.split_once('=').unwrap().1.trim();
        let original = styles(0, source.encode_utf16().count() as i32);
        for entity in original {
            let stored =
                StoredAnswer::capture(String::new(), source, body, std::slice::from_ref(&entity), true);
            let loaded = StoredAnswer::decode(&stored.encode()).unwrap();
            assert_eq!(loaded.body, body);
            assert_eq!(loaded.entities.len(), 1);
            let expected = placed(&entity, 0, body.encode_utf16().count() as i32);
            assert_eq!(loaded.entities.to_bytes(), vec![expected].to_bytes());
        }
    }

    #[test]
    fn inline_offsets_use_utf16_and_atomic_entities_keep_their_metadata() {
        let source = "تنظیم پاسخ 😀 = 🦀 Link";
        let body = source.split_once('=').unwrap().1.trim();
        let start = source[..source.find('🦀').unwrap()].encode_utf16().count() as i32;
        let entities = vec![
            MessageEntityBold {
                offset: 0,
                length: 5,
            }
            .into(),
            MessageEntityCustomEmoji {
                offset: start,
                length: 2,
                document_id: 123,
            }
            .into(),
            MessageEntityTextUrl {
                offset: start + 3,
                length: 4,
                url: "https://Example.com/AbC".to_owned(),
            }
            .into(),
        ];
        let captured = StoredAnswer::capture(String::new(), source, body, &entities, true);
        let loaded = StoredAnswer::decode(&captured.encode()).unwrap();
        assert_eq!(loaded.entities.len(), 2);
        assert_eq!(bounds(&loaded.entities[0]), Some((0, 2)));
        assert_eq!(bounds(&loaded.entities[1]), Some((3, 4)));
        assert!(matches!(&loaded.entities[0], E::CustomEmoji(e) if e.document_id == 123));
        assert!(matches!(&loaded.entities[1], E::TextUrl(e) if e.url == "https://Example.com/AbC"));
        assert!(slice_entities(&entities, start + 1, 1).is_empty());
    }

    #[test]
    fn formatted_dates_and_diff_entities_retain_their_payloads() {
        let body = "2026 changed";
        let entities = vec![
            MessageEntityFormattedDate {
                offset: 0,
                length: 4,
                date: 1_800_000_000,
                relative: false,
                short_time: false,
                long_time: false,
                short_date: true,
                long_date: false,
                day_of_week: false,
            }
            .into(),
            MessageEntityDiffInsert {
                offset: 5,
                length: 7,
            }
            .into(),
            MessageEntityDiffReplace {
                offset: 5,
                length: 7,
                old_text: "Original".to_owned(),
            }
            .into(),
            MessageEntityDiffDelete {
                offset: 5,
                length: 7,
            }
            .into(),
        ];
        let captured = StoredAnswer::capture(String::new(), body, body, &entities, false);
        let loaded = StoredAnswer::decode(&captured.encode()).unwrap();
        assert_eq!(loaded.entities.to_bytes(), entities.to_bytes());
    }

    #[test]
    fn replied_captions_and_plain_replacements_survive_media_recovery() {
        let body = "🦀 Code <b>literal</b> | ۱۲";
        let entities = vec![
            MessageEntityCode {
                offset: 3,
                length: 4,
            }
            .into(),
        ];
        let captured =
            StoredAnswer::capture("media-reference".to_owned(), body, body, &entities, false);
        let mut loaded = StoredAnswer::decode(&captured.encode()).unwrap();
        assert_eq!(loaded.body, body);
        assert_eq!(loaded.entities.to_bytes(), entities.to_bytes());
        loaded.media.clear();
        let without_media = StoredAnswer::decode(&loaded.encode()).unwrap();
        assert!(without_media.media.is_empty());
        assert_eq!(without_media.entities.to_bytes(), entities.to_bytes());
        let plain = StoredAnswer::capture(String::new(), body, body, &[], false);
        let replaced = StoredAnswer::decode(&plain.encode()).unwrap();
        assert_eq!(replaced.body, body);
        assert!(replaced.entities.is_empty());
    }

    #[test]
    fn detected_links_do_not_disable_authored_inline_html() {
        let source = "تنظیم پاسخ لینک = <b>Help</b> https://example.com";
        let body = source.split_once('=').unwrap().1.trim();
        let offset = source[..source.find("https://").unwrap()]
            .encode_utf16()
            .count() as i32;
        let entities = vec![MessageEntityUrl { offset, length: 19 }.into()];
        let captured = StoredAnswer::capture(String::new(), source, body, &entities, true);
        assert_eq!(captured.body, "Help https://example.com");
        assert!(matches!(captured.entities[0], E::Bold(_)));
    }

    #[test]
    fn legacy_answers_and_unformatted_inline_html_keep_rendering() {
        for stored in ["|<b>Hi</b> | there", "<b>Hi</b> | there"] {
            let (media, html) = stored.split_once('|').unwrap_or(("", stored));
            let loaded = StoredAnswer::decode(stored).unwrap();
            assert_eq!(loaded.media, media);
            assert_eq!(
                loaded.body,
                grammers_client::parsers::parse_html_message(html).0
            );
        }
        let old = StoredAnswer::decode("<b>Hi</b>").unwrap();
        assert_eq!(old.body, "Hi");
        assert!(matches!(old.entities[0], E::Bold(_)));
        let html = "<code>Hi</code>";
        let inline = StoredAnswer::capture(String::new(), html, html, &[], true);
        assert_eq!(inline.body, "Hi");
        assert!(matches!(inline.entities[0], E::Code(_)));
        assert!(StoredAnswer::decode("answer-v1|not json").is_none());
    }
}
