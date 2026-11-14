use std::collections::BTreeMap;

use grammers_client::message::{Button, InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerKind};
use grammers_client::update::CallbackQuery;

use crate::response::ResponseKind;
use crate::state::{SettingMutation, SettingsWriteError};

use super::premium::Icon;
use super::{Ctx, esc};

pub const COMMANDS: &[&str] = &["کپی تنظیمات", "/copysettings"];

const PICKER_MAX: usize = 20;

const KEPT: &[&str] = &[
    super::restrict::WIPE_BAN,
    super::restrict::WIPE_MUTE,
    super::purge::AUTO_AT,
    super::purge::AUTO_COUNT,
    "default_rights:night",
];

const KEYS: &[&str] = &[
    super::filters::EXACT,
    super::welcome::TEXT,
    super::welcome::ENTITIES,
    super::welcome::MEDIA,
    super::welcome::TTL,
    super::extras::RULES,
    super::bots::KICK_ADDER,
    super::bots::EVEN_ADMINS,
    crate::response::MODE_KEY,
];

const PREFIXES: &[&str] = &[
    super::filters::PREFIX,
    super::packs::PREFIX,
    super::restrict::CUSTOM_PREFIX,
    super::answers::PREFIX,
    super::voicemonitor::WORDS_PREFIX,
    super::voicemonitor::DISABLED_DEFAULT_PREFIX,
    super::strict::PICK,
    crate::response::CATEGORY_PREFIX,
    crate::response::KIND_PREFIX,
];

const IMAGE_PICK: &str = "strict:imgf";

pub(crate) fn copied(key: &str, limits: bool) -> bool {
    if key == super::limits::MODE || super::limits::CAPS.iter().any(|cap| cap.key == key) {
        return limits;
    }
    if KEPT.contains(&key) || key == IMAGE_PICK {
        return false;
    }
    super::locks::LOCKS.iter().any(|lock| lock.key == key)
        || super::setting::SETTINGS
            .iter()
            .any(|setting| setting.key == key)
        || super::tempmedia::KINDS.iter().any(|kind| kind.key == key)
        || KEYS.contains(&key)
        || PREFIXES.iter().any(|prefix| key.starts_with(prefix))
}

type Rows = BTreeMap<String, String>;

fn snapshot(ctx: &Ctx, chat: i64) -> Rows {
    ctx.settings.with_chat(chat, |settings| {
        settings
            .rows()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect()
    })
}

fn plan<'a>(source: &'a Rows, target: &'a Rows, limits: bool) -> Vec<SettingMutation<'a>> {
    let mut batch: Vec<SettingMutation<'a>> = source
        .iter()
        .filter(|(key, value)| copied(key, limits) && target.get(*key) != Some(*value))
        .map(|(key, value)| SettingMutation::Put { key, value })
        .collect();
    batch.extend(
        target
            .keys()
            .filter(|key| copied(key, limits) && !source.contains_key(*key))
            .map(|key| SettingMutation::Delete { key }),
    );

    let adds = source
        .get(super::join::ADD_REQUIRED)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let gated = target
        .get(super::join::CHANNEL)
        .is_some_and(|channel| !channel.is_empty())
        || adds > 0;
    match (gated, target.contains_key(super::join::GATE)) {
        (true, false) => batch.push(SettingMutation::Put {
            key: super::join::GATE,
            value: "",
        }),
        (false, true) => batch.push(SettingMutation::Delete {
            key: super::join::GATE,
        }),
        _ => {}
    }

    let picked = |rows: &Rows, other: bool| {
        rows.keys()
            .any(|key| key.starts_with(super::strict::PICK) && !(other && key == IMAGE_PICK))
    };
    let counted = !picked(target, false) || target.contains_key(IMAGE_PICK);
    let wanted = picked(source, false) && (counted || !picked(source, true));
    match (wanted, target.contains_key(IMAGE_PICK)) {
        (true, false) => batch.push(SettingMutation::Put {
            key: IMAGE_PICK,
            value: "",
        }),
        (false, true) => batch.push(SettingMutation::Delete { key: IMAGE_PICK }),
        _ => {}
    }
    batch
}

enum Ask {
    Picker,
    From(i64),
}

fn ask(tail: &str) -> Option<Ask> {
    let words: Vec<&str> = tail.split_whitespace().collect();
    match words.as_slice() {
        [] => Some(Ask::Picker),
        ["از", id] | [id] => group_id(id).map(Ask::From),
        _ => None,
    }
}

fn group_id(text: &str) -> Option<i64> {
    let id: i64 = text.parse().ok()?;
    match id {
        i64::MIN..=-1 => Some(id),
        1..=999_999_999_999 => Some(-1_000_000_000_000 - id),
        _ => None,
    }
}

fn title(ctx: &Ctx, chat: i64) -> String {
    ctx.settings
        .value(chat, super::TITLE)
        .unwrap_or_else(|| chat.to_string())
}

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let Some(tail) = COMMANDS
        .iter()
        .find_map(|command| super::commands::strip_prefix(view.digits(), command))
    else {
        return false;
    };
    let Some(ask) = ask(tail) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Some(target) = super::chat_id(message) else {
        return false;
    };
    let Some(user) = message
        .sender_id()
        .filter(|sender| sender.kind() == PeerKind::User)
        .and_then(PeerId::bare_id)
    else {
        refuse(
            ctx,
            message,
            "با حساب خودتان بفرستید؛ ادمین ناشناس در گروه دیگر شناخته نمی شود.",
        )
        .await;
        return true;
    };
    if super::owner(ctx, target).is_none() {
        refuse(
            ctx,
            message,
            "ابتدا سازنده گروه باید دستور «کانفیگ» را بفرستد.",
        )
        .await;
        return true;
    }

    let content = match ask {
        Ask::Picker => picker(ctx, target, user).await,
        Ask::From(source) => vet(ctx, target, source, user)
            .await
            .map(|()| card(ctx, target, source, user)),
    };
    let content = match content {
        Ok(content) => content,
        Err(why) => {
            refuse(ctx, message, why).await;
            return true;
        }
    };
    let sent = match message.sender_ref().await {
        Ok(Some(sender)) => ctx.client.send_message(sender, content).await.is_ok(),
        _ => false,
    };
    if sent {
        super::respond(
            ctx,
            message,
            ResponseKind::AdminTool,
            super::premium::icon_text(
                Some(Icon::TelegramSend),
                "کپی تنظیمات به پیوی شما فرستاده شد.",
            ),
        )
        .await;
    } else {
        refuse(
            ctx,
            message,
            "ابتدا ربات را در پیوی خود استارت کنید، سپس دوباره امتحان کنید.",
        )
        .await;
    }
    true
}

async fn refuse(ctx: &Ctx, message: &Message, why: &str) {
    super::respond(
        ctx,
        message,
        ResponseKind::CommandError,
        super::premium::icon_text(Some(Icon::ErrorRed), why),
    )
    .await;
}

async fn vet(ctx: &Ctx, target: i64, source: i64, user: i64) -> Result<(), &'static str> {
    if source == target {
        return Err("این همان گروه است؛ گروه دیگری را انتخاب کنید.");
    }
    let Some(chat_ref) = (source < 0).then(|| ctx.chat_ref(source)).flatten() else {
        return Err(
            "آن گروه در دسترس این ربات نیست. کپی فقط بین گروه هایی کار می کند که همین ربات در آن هاست.",
        );
    };
    if super::owner(ctx, source).is_none() || super::install::left_or_forgotten(ctx, source) {
        return Err("ربات در آن گروه نصب نیست.");
    }
    if !super::is_admin(ctx, chat_ref, source, user).await {
        return Err("ادمین بودن شما در آن گروه تایید نشد.");
    }
    if !super::limits::permits(ctx, source, user, super::limits::SET) {
        return Err("دسترسی تنظیمات شما در آن گروه بسته است.");
    }
    Ok(())
}

fn payload(prefix: &str, opener: i64, source: i64, target: i64) -> Vec<u8> {
    format!("{prefix}:{opener}:{source}:{target}").into_bytes()
}

async fn picker(ctx: &Ctx, target: i64, user: i64) -> Result<InputMessage, &'static str> {
    let chats = match ctx.settings.panels_for(user, 201).await {
        Ok(chats) => chats,
        Err(error) => {
            eprintln!("settings copy: {target}: groups of {user}: {error}");
            return Err("فهرست گروه ها خوانده نشد؛ دوباره تلاش کنید.");
        }
    };
    let mut groups: Vec<(String, i64)> = chats
        .into_iter()
        .filter(|&chat| {
            chat < 0
                && chat != target
                && super::owner(ctx, chat).is_some()
                && !super::install::left_or_forgotten(ctx, chat)
                && ctx.chat_ref(chat).is_some()
        })
        .map(|chat| (title(ctx, chat), chat))
        .collect();
    groups.sort();
    let more = groups.len() > PICKER_MAX;
    groups.truncate(PICKER_MAX);

    let head = format!(
        "<b>کپی تنظیمات</b>\n\n\
         مقصد · <b>{}</b>\n\
         شناسه · <code>{target}</code>",
        esc(&title(ctx, target))
    );
    if groups.is_empty() {
        return Ok(super::premium::icon_html(
            Some(Icon::Settings),
            format!(
                "{head}\n\n\
                 گروه دیگری پیدا نشد که مالک یا ادمین ربات آن باشید.\n\n\
                 <i>برای گروهی که فقط ادمین تلگرام آن هستید «کپی تنظیمات از شناسه گروه» را در گروه مقصد بفرستید.</i>"
            ),
        ));
    }

    let note = if more {
        format!(
            "\n\n<i>فقط {PICKER_MAX} گروه آمده است؛ برای بقیه «کپی تنظیمات از شناسه گروه» را بفرستید.</i>"
        )
    } else {
        String::new()
    };
    let mut rows: Vec<Vec<Button>> = groups
        .iter()
        .map(|(name, chat)| {
            vec![Button::data(
                name.as_str(),
                payload("cs", user, *chat, target),
            )]
        })
        .collect();
    rows.push(vec![cancel(user, target)]);
    Ok(super::premium::icon_html(
        Some(Icon::Settings),
        format!("{head}\n\nگروهی را انتخاب کنید که تنظیماتش در مقصد کپی شود.{note}"),
    )
    .reply_markup(super::premium::buttons(&rows)))
}

fn cancel(opener: i64, target: i64) -> Button {
    super::premium::decorate(
        super::style::data(
            "لغو",
            payload("cs", opener, 0, target),
            super::style::Colour::Danger,
        ),
        Some(Icon::Close),
    )
}

fn card(ctx: &Ctx, target: i64, source: i64, opener: i64) -> InputMessage {
    let limits = super::owner(ctx, target) == Some(opener);
    let changes = plan(&snapshot(ctx, source), &snapshot(ctx, target), limits).len();
    let text = preview(
        &esc(&title(ctx, source)),
        &esc(&title(ctx, target)),
        changes,
        limits,
    );
    let message = super::premium::icon_html(Some(Icon::Settings), text);
    if changes == 0 {
        return message;
    }
    message.reply_markup(super::premium::buttons(&[vec![
        super::premium::decorate(
            super::style::data(
                "تایید",
                payload("cy", opener, source, target),
                super::style::Colour::Primary,
            ),
            Some(Icon::Success),
        ),
        cancel(opener, target),
    ]]))
}

fn preview(source: &str, target: &str, changes: usize, limits: bool) -> String {
    let head = format!(
        "<b>کپی تنظیمات</b>\n\n\
         از · <b>{source}</b>\n\
         به · <b>{target}</b>"
    );
    if changes == 0 {
        return format!("{head}\n\n<i>تنظیمات دو گروه همین حالا یکی است.</i>");
    }
    let mut copied = vec![
        "قفل ها و تنظیمات پنل",
        "فیلتر کلمه، پک ها و دستورهای سفارشی",
        "پاسخ ها، خوشامد و قوانین",
    ];
    let mut kept = vec![
        "مالک، ادمین ها، کاربران ویژه، لقب ها و ربات های مجاز",
        "کانال لاگ، کانال عضویت اجباری و قفل سنجاق",
        "فیلتر تصویری و قفل چهره",
        "دسترسی های گروه، حالت شب و اسلوموشن",
        "پاکسازی خودکار و پاکسازی پیام ها با بن و سکوت",
    ];
    if limits {
        copied.push("محدودیت مدیران");
    } else {
        kept.push("محدودیت مدیران؛ فقط مالک گروه مقصد آن را کپی می کند");
    }
    let list = |items: &[&str]| {
        items
            .iter()
            .map(|item| format!("‹ {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "{head}\n\
         تغییر · <b>{changes}</b> مورد\n\n\
         <b>کپی می شود</b>\n{}\n\n\
         <b>کپی نمی شود</b>\n{}\n\n\
         <i>تنظیمات فعلی گروه مقصد جای خود را به تنظیمات گروه مبدا می دهد.</i>",
        list(&copied),
        list(&kept),
    )
}

pub async fn on_callback(
    ctx: &Ctx,
    query: &CallbackQuery,
    payload: &str,
    chat: i64,
    confirmed: bool,
) {
    let mut parts = payload.split(':');
    let (Some(Ok(opener)), Some(Ok(source)), Some(Ok(target)), None) = (
        parts.next().map(str::parse::<i64>),
        parts.next().map(str::parse::<i64>),
        parts.next().map(str::parse::<i64>),
        parts.next(),
    ) else {
        return;
    };
    if target != chat || chat >= 0 {
        return;
    }
    if query.sender_id().bare_id() != Some(opener) {
        alert(query, Icon::Locked, "این دکمه برای شخص دیگری است.").await;
        return;
    }
    if source == 0 {
        let _ = query
            .answer()
            .edit(super::premium::icon_html(Some(Icon::Close), "لغو شد."))
            .await;
        return;
    }
    if super::owner(ctx, chat).is_none() {
        alert(
            query,
            Icon::ErrorRed,
            "ابتدا سازنده گروه باید دستور «کانفیگ» را بفرستد.",
        )
        .await;
        return;
    }
    if let Err(why) = vet(ctx, chat, source, opener).await {
        alert(query, Icon::ErrorRed, why).await;
        return;
    }
    if !confirmed {
        let _ = query.answer().edit(card(ctx, chat, source, opener)).await;
        return;
    }
    if !ctx.claim_settings_copy(chat) {
        let _ = query.answer().send().await;
        return;
    }

    let limits = super::owner(ctx, chat) == Some(opener);
    let (from, into) = (snapshot(ctx, source), snapshot(ctx, chat));
    let sweep = into.get(super::bots::LOCK).map(String::as_str) != Some("")
        && from.get(super::bots::LOCK).map(String::as_str) == Some("");
    let batch = plan(&from, &into, limits);
    let changed = match ctx.settings.try_apply_batch(chat, &batch).await {
        Ok(changed) => changed,
        Err(error) => {
            eprintln!("settings copy: {source} -> {chat}: {error}");
            let _ = query
                .answer()
                .edit(super::premium::icon_text(
                    Some(Icon::ErrorRed),
                    failure(&error),
                ))
                .await;
            return;
        }
    };
    let _ = query
        .answer()
        .edit(super::premium::icon_html(
            Some(Icon::Success),
            format!(
                "<b>کپی تنظیمات</b>\n\n\
                 از · <b>{}</b>\n\
                 به · <b>{}</b>\n\
                 تغییر · <b>{changed}</b> مورد",
                esc(&title(ctx, source)),
                esc(&title(ctx, chat))
            ),
        ))
        .await;
    if sweep {
        super::bots::on_lock_set(ctx, chat, super::bots::LOCK, true).await;
    }
}

fn failure(error: &SettingsWriteError) -> &'static str {
    match error {
        error if error.commit_outcome_unknown() => {
            "نتیجه کپی نامشخص است؛ پیش از تلاش دوباره تنظیمات را بررسی کنید."
        }
        SettingsWriteError::CapacityReached(_) => {
            "جا نشد؛ تعداد تنظیمات گروه مقصد از سقف مجاز بیشتر می شد."
        }
        _ => "کپی ذخیره نشد؛ دوباره تلاش کنید.",
    }
}

async fn alert(query: &CallbackQuery, icon: Icon, text: &str) {
    let _ = query
        .answer()
        .alert(super::premium::plain_label(Some(icon), text))
        .send()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(pairs: &[(&str, &str)]) -> Rows {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn every_lock_and_declared_setting_travels_unless_kept() {
        for lock in super::super::locks::LOCKS {
            assert!(copied(lock.key, false), "lock {} is not copied", lock.key);
        }
        for setting in super::super::setting::SETTINGS {
            let limit = setting.key == super::super::limits::MODE;
            assert_eq!(
                copied(setting.key, false),
                !limit && !KEPT.contains(&setting.key),
                "{} is classified wrongly",
                setting.key
            );
        }
        for kept in KEPT {
            assert!(
                super::super::setting::SETTINGS
                    .iter()
                    .any(|setting| setting.key == *kept),
                "{kept} is kept but is no longer a declared setting"
            );
        }
    }

    #[test]
    fn identity_state_and_chat_bound_keys_stay_home() {
        for key in [
            super::super::config::OWNER,
            super::super::HASH,
            super::super::TITLE,
            "admin:42",
            "vip:42",
            "nick:42",
            "gate_free:42",
            "badge:42",
            "imgf:گربه",
            super::super::install::CLEANER_ADDED,
            super::super::install::LEFT_DAY,
            "checked_day",
            super::super::stats::REPORT_DAY,
            "auto_purge_day",
            "left_back_invite",
            super::super::pinlock::LOCK,
            super::super::pinlock::KEPT,
            super::super::extras::SLOW_STATE,
            super::super::log::CHANNEL,
            super::super::log::ON,
            super::super::join::CHANNEL,
            super::super::join::GATE,
            super::super::comment::GROUP,
            super::super::restrict::WIPE_BAN,
            super::super::purge::AUTO_AT,
            "night_state",
            "glock_until",
            "botok:42",
            "botname:42",
            "botby:42",
            IMAGE_PICK,
        ] {
            assert!(!copied(key, true), "{key} must not be copied");
        }
        assert_eq!(
            IMAGE_PICK,
            super::super::strict::pick_key(super::super::imgfilter::CAUSE)
        );
        assert!(super::super::bots::ALLOWED.starts_with("botok"));
        assert!(super::super::bots::NAMED.starts_with("botname"));
        assert!(super::super::bots::ADMITTED.starts_with("botby"));
    }

    #[test]
    fn the_target_keeps_its_own_image_filter_pick() {
        let pick = |causes: &[&str]| -> Rows {
            causes
                .iter()
                .map(|cause| (super::super::strict::pick_key(cause), String::new()))
                .collect()
        };
        let image = |source: &[&str], target: &[&str]| {
            let (source, target) = (pick(source), pick(target));
            plan(&source, &target, false)
                .iter()
                .find_map(|mutation| match mutation {
                    SettingMutation::Put { key, .. } if *key == IMAGE_PICK => Some("put"),
                    SettingMutation::Delete { key } if *key == IMAGE_PICK => Some("delete"),
                    _ => None,
                })
        };

        assert_eq!(image(&["links"], &[]), Some("put"));
        assert_eq!(image(&["links", "imgf"], &["photo"]), None);
        assert_eq!(image(&[], &["photo", "imgf"]), Some("delete"));
        assert_eq!(image(&["imgf"], &["photo"]), Some("put"));
    }

    #[test]
    fn the_target_keeps_its_allowed_bots() {
        let source = rows(&[("botok:1", "")]);
        let target = rows(&[("botok:2", ""), ("botname:2", "game")]);
        assert!(plan(&source, &target, true).is_empty());
    }

    #[test]
    fn limits_travel_only_for_the_owner() {
        for key in std::iter::once(super::super::limits::MODE)
            .chain(super::super::limits::CAPS.iter().map(|cap| cap.key))
        {
            assert!(copied(key, true));
            assert!(!copied(key, false));
        }
    }

    #[test]
    fn a_copy_replaces_and_leaves_the_rest_alone() {
        let source = rows(&[
            ("links", ""),
            ("flood_limit", "8"),
            ("filter:شکن", ""),
            ("owner", "1"),
            ("vip:5", ""),
            ("title", "منبع"),
        ]);
        let target = rows(&[
            ("photo", ""),
            ("flood_limit", "8"),
            ("owner", "2"),
            ("vip:9", ""),
            ("admin:9", ""),
            ("log_channel", "-100|1"),
        ]);
        let batch = plan(&source, &target, false);
        assert_eq!(
            batch,
            vec![
                SettingMutation::Put {
                    key: "filter:شکن",
                    value: ""
                },
                SettingMutation::Put {
                    key: "links",
                    value: ""
                },
                SettingMutation::Delete { key: "photo" },
            ]
        );
    }

    #[test]
    fn the_join_gate_follows_the_final_state() {
        let adds = rows(&[(super::super::join::ADD_REQUIRED, "3")]);
        let none = rows(&[]);
        assert!(plan(&adds, &none, false).contains(&SettingMutation::Put {
            key: super::super::join::GATE,
            value: ""
        }));

        let gated = rows(&[
            (super::super::join::ADD_REQUIRED, "3"),
            (super::super::join::GATE, ""),
        ]);
        assert!(plan(&none, &gated, false).contains(&SettingMutation::Delete {
            key: super::super::join::GATE
        }));

        let channel = rows(&[
            (super::super::join::CHANNEL, "-100|1"),
            (super::super::join::GATE, ""),
        ]);
        let batch = plan(&none, &channel, false);
        assert!(batch.is_empty(), "{batch:?}");
    }

    #[test]
    fn the_tail_is_an_id_or_nothing() {
        assert!(matches!(ask(""), Some(Ask::Picker)));
        assert!(matches!(ask("-1001234567890"), Some(Ask::From(-1001234567890))));
        assert!(matches!(ask("از -1001234567890"), Some(Ask::From(-1001234567890))));
        assert!(matches!(ask("1234567890"), Some(Ask::From(-1001234567890))));
        assert!(ask("رو بفرست").is_none());
        assert!(ask("از گروه").is_none());
        assert!(ask("0").is_none());
        assert!(ask("-100 -200").is_none());
    }

    #[test]
    fn payloads_fit_telegram() {
        for prefix in ["cs", "cy"] {
            let longest = payload(prefix, i64::MAX, i64::MIN, i64::MIN);
            assert!(longest.len() <= 64, "{}", String::from_utf8_lossy(&longest));
        }
    }

    #[test]
    fn the_card_follows_the_layout() {
        for limits in [true, false] {
            let text = preview("الف", "ب", 7, limits);
            assert!(text.starts_with("<b>کپی تنظیمات</b>\n\n"));
            assert!(!text.contains("\n\n\n"), "two blank lines");
            assert!(!text.contains('\u{200c}'));
            assert_eq!(text.contains("‹ محدودیت مدیران\n"), limits);
        }
        assert!(!preview("الف", "ب", 0, true).contains("کپی می شود"));
    }
}
