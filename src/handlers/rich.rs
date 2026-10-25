use grammers_client::tl;

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Media {
    pub photo: bool,
    pub video: bool,
    pub gif: bool,
    pub sticker: bool,
    pub animsticker: bool,
    pub voice: bool,
    pub music: bool,
    pub file: bool,
    pub location: bool,
    pub contact: bool,
    pub poll: bool,
    pub dice: bool,
    pub story: bool,
    pub other: bool,
    pub preview: bool,
    pub channel: bool,
    pub spoiler: bool,
}

impl Media {
    pub fn any(&self) -> bool {
        self.photo
            || self.video
            || self.gif
            || self.sticker
            || self.voice
            || self.music
            || self.file
            || self.location
            || self.contact
            || self.poll
            || self.dice
            || self.story
            || self.other
            || self.preview
    }

    pub fn merge(&mut self, other: Self) {
        self.photo |= other.photo;
        self.video |= other.video;
        self.gif |= other.gif;
        self.sticker |= other.sticker;
        self.animsticker |= other.animsticker;
        self.voice |= other.voice;
        self.music |= other.music;
        self.file |= other.file;
        self.location |= other.location;
        self.contact |= other.contact;
        self.poll |= other.poll;
        self.dice |= other.dice;
        self.story |= other.story;
        self.other |= other.other;
        self.preview |= other.preview;
        self.channel |= other.channel;
        self.spoiler |= other.spoiler;
    }

    pub fn of(media: &tl::enums::MessageMedia) -> Self {
        use tl::enums::MessageMedia as M;
        let mut shown = Self::default();
        match media {
            M::Empty | M::Unsupported => {}
            M::Photo(photo) => {
                shown.photo = true;
                shown.spoiler = photo.spoiler;
            }
            M::Document(document) => {
                shown.spoiler = document.spoiler;
                match &document.document {
                    Some(tl::enums::Document::Document(_)) if document.voice => shown.voice = true,
                    Some(tl::enums::Document::Document(inner)) => shown.add_document(inner),
                    _ => shown.other = true,
                }
            }
            M::Geo(_) | M::GeoLive(_) | M::Venue(_) => shown.location = true,
            M::Contact(_) => shown.contact = true,
            M::Poll(_) => shown.poll = true,
            M::Dice(_) => shown.dice = true,
            M::Story(_) => shown.story = true,
            M::WebPage(preview) => {
                shown.preview = match &preview.webpage {
                    tl::enums::WebPage::Page(page) => !page.url.trim().is_empty(),
                    tl::enums::WebPage::Pending(page) => page.url.is_some(),
                    tl::enums::WebPage::Empty(page) => page.url.is_some(),
                    tl::enums::WebPage::NotModified(_) => false,
                }
            }
            _ => shown.other = true,
        }
        shown
    }

    fn add_document(&mut self, document: &tl::types::Document) {
        let mime = document.mime_type.as_str();
        let mut animated = mime == "image/gif";
        let (mut sticker, mut video, mut voice) = (false, false, false);
        for attribute in &document.attributes {
            match attribute {
                tl::enums::DocumentAttribute::Animated => animated = true,
                tl::enums::DocumentAttribute::Sticker(_) => sticker = true,
                tl::enums::DocumentAttribute::Video(_) => video = true,
                tl::enums::DocumentAttribute::Audio(audio) => voice |= audio.voice,
                _ => {}
            }
        }
        if sticker {
            self.sticker = true;
            self.animsticker |= mime == "application/x-tgsticker" || mime.starts_with("video/");
        } else if animated {
            self.gif = true;
        } else if video || mime.starts_with("video/") {
            self.video = true;
        } else if voice {
            self.voice = true;
        } else if mime.starts_with("audio/") {
            self.music = true;
        } else if mime.starts_with("image/") {
            self.photo = true;
        } else {
            self.file = true;
        }
    }
}

pub struct Flat {
    pub text: String,
    pub entities: Vec<tl::enums::MessageEntity>,
    pub media: Media,
}

pub fn of(raw: &tl::enums::Message) -> Option<Flat> {
    let tl::enums::Message::Message(message) = raw else {
        return None;
    };
    let tl::enums::RichMessage::Message(rich) = message.rich_message.as_ref()?;
    Some(flatten(rich))
}

pub fn flatten(rich: &tl::types::RichMessage) -> Flat {
    let mut writer = Writer {
        documents: &rich.documents,
        text: String::new(),
        units: 0,
        entities: Vec::new(),
        media: Media::default(),
    };
    writer.blocks(&rich.blocks);
    Flat {
        text: writer.text,
        entities: writer.entities,
        media: writer.media,
    }
}

struct Writer<'r> {
    documents: &'r [tl::enums::Document],
    text: String,
    units: i32,
    entities: Vec<tl::enums::MessageEntity>,
    media: Media,
}

impl Writer<'_> {
    fn push(&mut self, text: &str) {
        self.text.push_str(text);
        self.units = self
            .units
            .saturating_add(i32::try_from(text.encode_utf16().count()).unwrap_or(i32::MAX));
    }

    fn span(
        &mut self,
        inner: &tl::enums::RichText,
        make: impl FnOnce(i32, i32) -> tl::enums::MessageEntity,
    ) {
        let start = self.units;
        self.rich(inner);
        if self.units > start {
            self.entities.push(make(start, self.units - start));
        }
    }

    fn line(&mut self, text: &tl::enums::RichText) {
        self.rich(text);
        self.push("\n");
    }

    fn link(&mut self, url: &str) {
        if url.trim().is_empty() {
            return;
        }
        let start = self.units;
        self.push(url);
        let length = self.units - start;
        self.entities.push(
            tl::types::MessageEntityUrl {
                offset: start,
                length,
            }
            .into(),
        );
        self.push("\n");
    }

    fn hidden(&mut self, url: &str) {
        let start = self.units;
        self.push(" ");
        self.entities.push(
            tl::types::MessageEntityTextUrl {
                offset: start,
                length: 1,
                url: url.to_owned(),
            }
            .into(),
        );
    }

    fn caption(&mut self, caption: &tl::enums::PageCaption) {
        let tl::enums::PageCaption::Caption(caption) = caption;
        self.line(&caption.text);
        self.line(&caption.credit);
    }

    fn rich(&mut self, text: &tl::enums::RichText) {
        use tl::enums::RichText as T;
        match text {
            T::TextEmpty => {}
            T::TextPlain(plain) => self.push(&plain.text),
            T::TextConcat(concat) => {
                for part in &concat.texts {
                    self.rich(part);
                }
            }
            T::TextBold(t) => self.rich(&t.text),
            T::TextItalic(t) => self.rich(&t.text),
            T::TextUnderline(t) => self.rich(&t.text),
            T::TextStrike(t) => self.rich(&t.text),
            T::TextSubscript(t) => self.rich(&t.text),
            T::TextSuperscript(t) => self.rich(&t.text),
            T::TextMarked(t) => self.rich(&t.text),
            T::TextAnchor(t) => self.rich(&t.text),
            T::TextPhone(t) => self.rich(&t.text),
            T::TextAutoPhone(t) => self.rich(&t.text),
            T::TextBankCard(t) => self.rich(&t.text),
            T::TextDate(t) => self.rich(&t.text),
            T::TextDiff(t) => {
                self.rich(&t.text);
                self.push(" ");
                self.rich(&t.old_text);
            }
            T::TextMath(math) => self.push(&math.source),
            T::TextFixed(t) => self.span(&t.text, |offset, length| {
                tl::types::MessageEntityCode { offset, length }.into()
            }),
            T::TextUrl(t) => {
                let url = t.url.clone();
                self.span(&t.text, |offset, length| {
                    tl::types::MessageEntityTextUrl {
                        offset,
                        length,
                        url,
                    }
                    .into()
                });
            }
            T::TextAutoUrl(t) => self.span(&t.text, |offset, length| {
                tl::types::MessageEntityUrl { offset, length }.into()
            }),
            T::TextEmail(t) => self.span(&t.text, |offset, length| {
                tl::types::MessageEntityEmail { offset, length }.into()
            }),
            T::TextAutoEmail(t) => self.span(&t.text, |offset, length| {
                tl::types::MessageEntityEmail { offset, length }.into()
            }),
            T::TextMention(t) => self.span(&t.text, |offset, length| {
                tl::types::MessageEntityMention { offset, length }.into()
            }),
            T::TextMentionName(t) => {
                let user_id = t.user_id;
                self.span(&t.text, |offset, length| {
                    tl::types::MessageEntityMentionName {
                        offset,
                        length,
                        user_id,
                    }
                    .into()
                });
            }
            T::TextHashtag(t) => self.span(&t.text, |offset, length| {
                tl::types::MessageEntityHashtag { offset, length }.into()
            }),
            T::TextCashtag(t) => self.span(&t.text, |offset, length| {
                tl::types::MessageEntityCashtag { offset, length }.into()
            }),
            T::TextBotCommand(t) => self.span(&t.text, |offset, length| {
                tl::types::MessageEntityBotCommand { offset, length }.into()
            }),
            T::TextSpoiler(t) => self.span(&t.text, |offset, length| {
                tl::types::MessageEntitySpoiler { offset, length }.into()
            }),
            T::TextCustomEmoji(emoji) => {
                let start = self.units;
                self.push(if emoji.alt.is_empty() {
                    " "
                } else {
                    &emoji.alt
                });
                self.entities.push(
                    tl::types::MessageEntityCustomEmoji {
                        offset: start,
                        length: self.units - start,
                        document_id: emoji.document_id,
                    }
                    .into(),
                );
            }
            T::TextImage(image) => self.document(image.document_id),
        }
    }

    fn blocks(&mut self, blocks: &[tl::enums::PageBlock]) {
        for block in blocks {
            self.block(block);
        }
    }

    fn block(&mut self, block: &tl::enums::PageBlock) {
        use tl::enums::PageBlock as B;
        match block {
            B::Unsupported | B::Divider | B::Anchor(_) => {}
            B::Title(b) => self.line(&b.text),
            B::Subtitle(b) => self.line(&b.text),
            B::AuthorDate(b) => self.line(&b.author),
            B::Header(b) => self.line(&b.text),
            B::Subheader(b) => self.line(&b.text),
            B::Paragraph(b) => self.line(&b.text),
            B::Footer(b) => self.line(&b.text),
            B::Kicker(b) => self.line(&b.text),
            B::Heading1(b) => self.line(&b.text),
            B::Heading2(b) => self.line(&b.text),
            B::Heading3(b) => self.line(&b.text),
            B::Heading4(b) => self.line(&b.text),
            B::Heading5(b) => self.line(&b.text),
            B::Heading6(b) => self.line(&b.text),
            B::Thinking(b) => self.line(&b.text),
            B::Math(b) => {
                self.push(&b.source);
                self.push("\n");
            }
            B::Preformatted(b) => {
                let language = b.language.clone();
                self.span(&b.text, |offset, length| {
                    tl::types::MessageEntityPre {
                        offset,
                        length,
                        language,
                    }
                    .into()
                });
                self.push("\n");
            }
            B::Blockquote(b) => {
                self.line(&b.text);
                self.line(&b.caption);
            }
            B::Pullquote(b) => {
                self.line(&b.text);
                self.line(&b.caption);
            }
            B::BlockquoteBlocks(b) => {
                self.blocks(&b.blocks);
                self.line(&b.caption);
            }
            B::List(b) => {
                for item in &b.items {
                    match item {
                        tl::enums::PageListItem::Text(item) => self.line(&item.text),
                        tl::enums::PageListItem::Blocks(item) => self.blocks(&item.blocks),
                    }
                }
            }
            B::OrderedList(b) => {
                for item in &b.items {
                    match item {
                        tl::enums::PageListOrderedItem::Text(item) => self.line(&item.text),
                        tl::enums::PageListOrderedItem::Blocks(item) => self.blocks(&item.blocks),
                    }
                }
            }
            B::Table(b) => {
                self.line(&b.title);
                for tl::enums::PageTableRow::Row(row) in &b.rows {
                    for tl::enums::PageTableCell::Cell(cell) in &row.cells {
                        if let Some(text) = &cell.text {
                            self.line(text);
                        }
                    }
                }
            }
            B::Details(b) => {
                self.line(&b.title);
                self.blocks(&b.blocks);
            }
            B::Photo(b) => {
                self.media.photo = true;
                self.media.spoiler |= b.spoiler;
                if let Some(url) = &b.url {
                    self.hidden(url);
                }
                self.caption(&b.caption);
            }
            B::Video(b) => {
                self.document(b.video_id);
                self.media.spoiler |= b.spoiler;
                self.caption(&b.caption);
            }
            B::Audio(b) => {
                self.document(b.audio_id);
                self.caption(&b.caption);
            }
            B::Cover(b) => self.block(&b.cover),
            B::Collage(b) => {
                self.blocks(&b.items);
                self.caption(&b.caption);
            }
            B::Slideshow(b) => {
                self.blocks(&b.items);
                self.caption(&b.caption);
            }
            B::Embed(b) => {
                if let Some(url) = &b.url {
                    self.link(url);
                }
                if b.poster_photo_id.is_some() {
                    self.media.photo = true;
                }
                self.caption(&b.caption);
            }
            B::EmbedPost(b) => {
                self.link(&b.url);
                self.push(&b.author);
                self.push("\n");
                self.blocks(&b.blocks);
                self.caption(&b.caption);
            }
            B::RelatedArticles(b) => {
                self.line(&b.title);
                for tl::enums::PageRelatedArticle::Article(article) in &b.articles {
                    self.link(&article.url);
                    for text in [&article.title, &article.description, &article.author]
                        .into_iter()
                        .flatten()
                    {
                        self.push(text);
                        self.push("\n");
                    }
                }
            }
            B::Channel(b) => {
                self.media.channel = true;
                if let tl::enums::Chat::Channel(channel) = &b.channel {
                    self.push(&channel.title);
                    self.push("\n");
                }
            }
            B::Map(b) => {
                self.media.location = true;
                self.caption(&b.caption);
            }
            B::InputPageBlockMap(b) => {
                self.media.location = true;
                self.caption(&b.caption);
            }
        }
    }

    fn document(&mut self, id: i64) {
        let found = self.documents.iter().find_map(|document| match document {
            tl::enums::Document::Document(document) if document.id == id => Some(document),
            _ => None,
        });
        match found {
            Some(document) => self.media.add_document(document),
            None => self.media.file = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> tl::enums::RichText {
        tl::types::TextPlain { text: text.into() }.into()
    }

    fn paragraph(text: tl::enums::RichText) -> tl::enums::PageBlock {
        tl::types::PageBlockParagraph { text }.into()
    }

    fn caption() -> tl::enums::PageCaption {
        tl::types::PageCaption {
            text: tl::enums::RichText::TextEmpty,
            credit: tl::enums::RichText::TextEmpty,
        }
        .into()
    }

    fn rich(blocks: Vec<tl::enums::PageBlock>, documents: Vec<tl::enums::Document>) -> Flat {
        flatten(&tl::types::RichMessage {
            rtl: false,
            part: false,
            blocks,
            photos: Vec::new(),
            documents,
        })
    }

    fn document(
        id: i64,
        mime: &str,
        attributes: Vec<tl::enums::DocumentAttribute>,
    ) -> tl::enums::Document {
        tl::types::Document {
            id,
            access_hash: 0,
            file_reference: Vec::new(),
            date: 0,
            mime_type: mime.into(),
            size: 1,
            thumbs: None,
            video_thumbs: None,
            dc_id: 1,
            attributes,
        }
        .into()
    }

    #[test]
    fn spans_become_entities_at_their_utf16_offsets() {
        let concat: tl::enums::RichText = tl::types::TextConcat {
            texts: vec![
                plain("😀 "),
                tl::types::TextUrl {
                    text: plain("here"),
                    url: "https://spam.example".into(),
                    webpage_id: 0,
                }
                .into(),
                plain(" "),
                tl::types::TextFixed {
                    text: plain("spam.com"),
                }
                .into(),
                plain(" "),
                tl::types::TextHashtag { text: plain("#ad") }.into(),
            ],
        }
        .into();
        let flat = rich(vec![paragraph(concat)], Vec::new());
        assert_eq!(flat.text, "😀 here spam.com #ad\n");
        let spans: Vec<_> = flat
            .entities
            .iter()
            .map(|entity| match entity {
                tl::enums::MessageEntity::TextUrl(e) => ("url", e.offset, e.length),
                tl::enums::MessageEntity::Code(e) => ("code", e.offset, e.length),
                tl::enums::MessageEntity::Hashtag(e) => ("tag", e.offset, e.length),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(spans, vec![("url", 3, 4), ("code", 8, 8), ("tag", 17, 3)]);
    }

    #[test]
    fn nested_blocks_are_written_out() {
        let flat = rich(
            vec![
                tl::types::PageBlockList {
                    items: vec![
                        tl::types::PageListItemText {
                            checkbox: false,
                            checked: false,
                            text: plain("one"),
                        }
                        .into(),
                    ],
                }
                .into(),
                tl::types::PageBlockDetails {
                    open: false,
                    blocks: vec![paragraph(plain("hidden t.me/x"))],
                    title: plain("more"),
                }
                .into(),
                tl::types::PageBlockBlockquote {
                    text: plain("quoted"),
                    caption: tl::enums::RichText::TextEmpty,
                }
                .into(),
            ],
            Vec::new(),
        );
        for part in ["one", "more", "hidden t.me/x", "quoted"] {
            assert!(
                flat.text.contains(part),
                "{part} missing from {:?}",
                flat.text
            );
        }
    }

    #[test]
    fn media_blocks_are_sorted_by_kind() {
        let flat = rich(
            vec![
                tl::types::PageBlockPhoto {
                    spoiler: true,
                    photo_id: 1,
                    caption: caption(),
                    url: Some("https://spam.example".into()),
                    webpage_id: Some(0),
                }
                .into(),
                tl::types::PageBlockVideo {
                    autoplay: false,
                    r#loop: false,
                    spoiler: false,
                    video_id: 2,
                    caption: caption(),
                }
                .into(),
                tl::types::PageBlockAudio {
                    audio_id: 3,
                    caption: caption(),
                }
                .into(),
            ],
            vec![
                document(2, "video/mp4", Vec::new()),
                document(
                    3,
                    "audio/ogg",
                    vec![
                        tl::types::DocumentAttributeAudio {
                            voice: true,
                            duration: 1,
                            title: None,
                            performer: None,
                            waveform: None,
                        }
                        .into(),
                    ],
                ),
            ],
        );
        assert_eq!(
            flat.media,
            Media {
                photo: true,
                video: true,
                voice: true,
                spoiler: true,
                ..Media::default()
            }
        );
        assert!(!flat.text.contains("spam.example"));
        assert!(flat.entities.iter().any(|e| matches!(
            e,
            tl::enums::MessageEntity::TextUrl(url) if url.url == "https://spam.example"
        )));
    }

    #[test]
    fn message_media_is_sorted_by_kind() {
        let doc = |mime: &str, attributes, voice| -> tl::enums::MessageMedia {
            tl::types::MessageMediaDocument {
                nopremium: false,
                spoiler: false,
                video: false,
                round: false,
                voice,
                document: Some(document(1, mime, attributes)),
                alt_documents: None,
                video_cover: None,
                video_timestamp: None,
                ttl_seconds: None,
            }
            .into()
        };
        let sticker: tl::enums::DocumentAttribute = tl::types::DocumentAttributeSticker {
            mask: false,
            alt: String::new(),
            stickerset: tl::enums::InputStickerSet::Empty,
            mask_coords: None,
        }
        .into();

        let animated = Media::of(&doc(
            "application/x-tgsticker",
            vec![sticker.clone()],
            false,
        ));
        assert!(animated.sticker && animated.animsticker);
        let still = Media::of(&doc("image/webp", vec![sticker], false));
        assert!(still.sticker && !still.animsticker);
        assert!(Media::of(&doc("video/mp4", Vec::new(), false)).video);
        assert!(
            Media::of(&doc(
                "video/mp4",
                vec![tl::enums::DocumentAttribute::Animated],
                false
            ))
            .gif
        );
        assert!(Media::of(&doc("audio/ogg", Vec::new(), true)).voice);
        assert!(Media::of(&doc("audio/mpeg", Vec::new(), false)).music);
        assert!(Media::of(&doc("application/zip", Vec::new(), false)).file);
        assert!(Media::of(&tl::enums::MessageMedia::Unsupported) == Media::default());
    }

    #[test]
    fn an_embed_and_a_related_article_are_links_on_the_screen() {
        let flat = rich(
            vec![
                tl::types::PageBlockEmbed {
                    full_width: false,
                    allow_scrolling: false,
                    url: Some("https://embed.example/x".into()),
                    html: None,
                    poster_photo_id: None,
                    w: None,
                    h: None,
                    caption: caption(),
                }
                .into(),
                tl::types::PageBlockRelatedArticles {
                    title: plain("see"),
                    articles: vec![
                        tl::types::PageRelatedArticle {
                            url: "https://t.me/spam".into(),
                            webpage_id: 0,
                            title: Some("join".into()),
                            description: None,
                            photo_id: None,
                            author: None,
                            published_date: None,
                        }
                        .into(),
                    ],
                }
                .into(),
            ],
            Vec::new(),
        );
        assert!(flat.text.contains("https://embed.example/x"));
        assert!(flat.text.contains("https://t.me/spam"));
        assert_eq!(
            flat.entities
                .iter()
                .filter(|e| matches!(e, tl::enums::MessageEntity::Url(_)))
                .count(),
            2
        );
        assert!(!flat.media.any());
    }
}
