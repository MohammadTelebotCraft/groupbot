use grammers_client::message::{Button, InputMessage, Message};
use grammers_client::session::types::PeerId;

use super::{Ctx, can_manage, esc, is_owner, name_of, owner, sender_is_creator};

pub const OWNER: &str = "owner";

pub const CONFIG: &[&str] = &[
    "کانفیگ",
    "پیکربندی",
    "راه اندازی",
    "/config",
    "/setup",
    "/configure",
];
pub const ADMIN_LIST: &[&str] = &[
    "لیست ادمین",
    "لیست ادمین ها",
    "ادمین ها",
    "فهرست ادمین ها",
    "لیست مدیران",
    "/admins",
    "مدیران گروه",
    "/adminlist",
];
pub const ADMIN_CLEAR: &[&str] = &[
    "پاکسازی لیست ادمین",
    "پاکسازی لیست ادمین ها",
    "پاکسازی لیست مدیران",
    "خالی کردن لیست ادمین ها",
    "پاکسازی ادمین های ربات",
    "/clearbotadmins",
    "پاک کردن ادمین های ربات",
];
pub const PROMOTE: &[&str] = &[
    "افزودن ادمین ربات",
    "ترفیع ربات",
    "ترفیع",
    "/botpromote",
    "/addbotadmin",
];
pub const DEMOTE: &[&str] = &[
    "حذف ادمین ربات",
    "عزل ادمین ربات",
    "تنزل ربات",
    "تنزل",
    "/botdemote",
    "/delbotadmin",
];
pub const HELP: &[&str] = &[
    "راهنما",
    "راهنمای ربات",
    "دستورها",
    "دستورات",
    "لیست دستورات",
    "help",
    "/help",
    "/commands",
    "راهنمای دستورات",
    "/guide",
];

const START: &[&str] = &["/start", "شروع", "استارت"];

const LINKS: &[(&str, &str, Option<super::premium::Icon>)] = &[
    ("CHANNEL", "کانال", Some(super::premium::Icon::TelegramSend)),
    ("SUPPORT", "پشتیبانی", Some(super::premium::Icon::Chat)),
    ("SOURCE", "🖥  سورس", None),
];

pub(super) struct ConfiguredLink {
    label: &'static str,
    url: String,
    icon: Option<super::premium::Icon>,
}

#[derive(Default)]
pub(super) struct ConfiguredLinks(Box<[ConfiguredLink]>);

impl ConfiguredLinks {
    pub(super) fn from_environment() -> Result<Self, String> {
        let mut links = Vec::with_capacity(LINKS.len());
        for (name, label, icon) in LINKS {
            let value = match std::env::var(name) {
                Ok(value) => value,
                Err(std::env::VarError::NotPresent) => continue,
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(format!("{name} is not valid Unicode"));
                }
            };
            let url = normalize_link(&value)
                .ok_or_else(|| format!("{name} is not a Telegram handle or HTTP(S) URL"))?;
            links.push(ConfiguredLink {
                label,
                url,
                icon: *icon,
            });
        }
        Ok(Self(links.into_boxed_slice()))
    }

    pub(super) fn as_slice(&self) -> &[ConfiguredLink] {
        &self.0
    }
}

fn normalize_link(value: &str) -> Option<String> {
    if value.is_empty() || value != value.trim() || value.chars().any(char::is_whitespace) {
        return None;
    }
    if value.starts_with("https://") || value.starts_with("http://") {
        let parsed = reqwest::Url::parse(value).ok()?;
        let host = parsed.host_str()?;
        let address = host.trim_start_matches('[').trim_end_matches(']');
        let externally_addressable =
            host.contains('.') || address.parse::<std::net::IpAddr>().is_ok();
        let valid = matches!(parsed.scheme(), "http" | "https")
            && externally_addressable
            && parsed.username().is_empty()
            && parsed.password().is_none();
        return valid.then(|| parsed.to_string());
    }
    let handle = value.trim_start_matches('@');
    let usable = handle.len() >= 5
        && handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    usable.then(|| format!("https://t.me/{handle}"))
}

pub async fn start(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    if !is_start(text) {
        return false;
    }
    if let Some(user) = message.sender_id().and_then(PeerId::bare_id) {
        let access_hash = message
            .sender_ref()
            .await
            .ok()
            .flatten()
            .map(|peer| peer.auth.hash())
            .unwrap_or_default();
        if let Err(error) = ctx.settings.remember_started_user(user, access_hash).await {
            log::warn!(
                "start: could not remember user {user}; left-back DM remains disabled: {error}"
            );
        }
    }
    let mut card = super::premium::icon_html_on(
        super::premium::Surface::Message,
        Some(super::premium::Icon::Bot),
        format!(
            "<b>سلام {} عزیز</b>\n\n\
         من ربات مدیریت گروه هستم؛ نظم، امنیت و آمار گروه شما با من.\n\n\
         <b>چه کاری از من بر می آید</b>\n\
         ✓ پاسخ فوری به دستورها، حتی در گروه های پر رفت و آمد\n\
         ✓ جلوگیری از اسپم، رگبار پیام و تبلیغ\n\
         ✓ قفل روی هر نوع محتوا و فیلتر کلمه\n\
         ✓ مدیریت دسترسی اعضا و ادمین ها\n\
         ✓ خوشامد، اخطار، سکوت، بن و گزارش\n\
         ✓ آمار روزانه و رتبه بندی اعضا\n\n\
         <b>راه اندازی</b>\n\
         ۱ · ربات را به گروه اضافه کنید\n\
         ۲ · او را ادمین کنید و همه دسترسی ها را بدهید، «افزودن ادمین جدید» هم لازم است\n\
         ۳ · همین که دسترسی ها کامل شد خودش فعال می شود و کلینر را هم می آورد\n\n\
         <i>گروه باید سوپرگروه باشد · «وضعیت نصب» می گوید چه چیزی کم است \
         · «راهنما» برای دستورها، «پنل» برای تنظیمات</i>",
            esc(&name_of(message)),
        ),
    );

    let mut rows: Vec<Vec<Button>> = Vec::new();
    if let Some(username) = ctx.bot_username() {
        rows.push(vec![Button::url(
            "➕  افزودن ربات به گروه",
            format!("https://t.me/{username}?startgroup=new"),
        )]);
    }
    for pair in ctx.start_links().chunks(2) {
        rows.push(
            pair.iter()
                .map(|link| {
                    super::premium::decorate(Button::url(link.label, link.url.clone()), link.icon)
                })
                .collect(),
        );
    }
    if let Some(user) = message.sender_id().and_then(PeerId::bare_id) {
        let chat = message.peer_id().bot_api_dialog_id().unwrap_or(0);
        rows.push(vec![super::panel::help_button(user, chat)]);
    }
    if !rows.is_empty() {
        card = card.reply_markup(super::premium::buttons(&rows));
    }
    let _ = message.reply(card).await;
    true
}

fn is_start(text: &str) -> bool {
    super::commands::strip_prefix(text, "/start").is_some()
        || START[1..]
            .iter()
            .any(|command| super::commands::matches(text, command))
}

pub async fn help(ctx: &Ctx, message: &Message) -> bool {
    if !super::commands::contains(HELP, message.text()) {
        return false;
    }
    let Some(user) = message.sender_id().and_then(PeerId::bare_id) else {
        return false;
    };
    let chat = message.peer_id().bot_api_dialog_id().unwrap_or(0);
    let _ = ctx;
    super::respond(
        ctx,
        message,
        crate::response::ResponseKind::Help,
        super::premium::html(super::help::index())
            .reply_markup(super::panel::index_markup(user, chat)),
    )
    .await;
    true
}

pub async fn handle(ctx: &std::sync::Arc<Ctx>, message: &Message) -> bool {
    let text = message.text().trim();
    let (Some(chat), Some(sender)) = (
        message.peer_id().bot_api_dialog_id(),
        message.sender_id().and_then(PeerId::bare_id),
    ) else {
        return false;
    };

    if super::commands::contains(ADMIN_LIST, text) {
        let (creator, admins) = match message.peer_ref().await {
            Ok(Some(chat_ref)) => super::admins(ctx, chat_ref).await,
            _ => (None, Vec::new()),
        };

        let bot_admins: Vec<String> = ctx
            .settings
            .flags_with_prefix(chat, "admin:")
            .into_iter()
            .map(|id| format!("‹ <code>{id}</code>"))
            .collect();
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::AdminTool,
            super::premium::icon_html(
                Some(super::premium::Icon::Group),
                format!(
                    "<b>ادمین های گروه</b> ({})\n{}\n\n\
                 <b>ادمین های ربات</b> ({})\n{}\n\n\
                 مالک ربات · {}",
                    admins.len(),
                    if admins.is_empty() {
                        "‹ کسی نیست".to_owned()
                    } else {
                        admins.join("\n")
                    },
                    bot_admins.len(),
                    if bot_admins.is_empty() {
                        "‹ کسی نیست".to_owned()
                    } else {
                        bot_admins.join("\n")
                    },
                    match owner(ctx, chat) {
                        Some(id) => format!("<code>{id}</code>"),
                        None => "ثبت نشده".to_owned(),
                    },
                ),
            ),
        )
        .await;
        let _ = creator;
        return true;
    }

    if super::commands::contains(ADMIN_CLEAR, text) {
        if !is_owner(ctx, message) {
            if owner(ctx, chat).is_none() && can_manage(ctx, message).await {
                super::respond(
                    ctx,
                    message,
                    crate::response::ResponseKind::PermissionDenied,
                    "ابتدا سازنده گروه باید دستور «کانفیگ» را بفرستد.",
                )
                .await;
            }
            return true;
        }
        let keys: Vec<String> = ctx
            .settings
            .flags_with_prefix(chat, "admin:")
            .into_iter()
            .filter_map(|id| id.parse::<i64>().ok())
            .map(super::bot_admin_key)
            .collect();
        let mutations: Vec<_> = keys
            .iter()
            .map(|key| crate::state::SettingMutation::Delete { key })
            .collect();
        let body = match ctx.settings.try_apply_batch(chat, &mutations).await {
            Ok(_) => format!(
                "✓ {} ادمین ربات حذف شد. ادمین های تلگرام دست نخورده اند.",
                keys.len()
            ),
            Err(error) => {
                ::log::warn!("bot admin: clear for {chat} failed: {error}");
                if error.commit_outcome_unknown() {
                    "نتیجه پاکسازی ادمین های ربات نامشخص است؛ «لیست ادمین» را بررسی کنید."
                        .to_owned()
                } else {
                    "ادمین های ربات حذف نشدند؛ دوباره تلاش کنید.".to_owned()
                }
            }
        };
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::SettingsChanged,
            super::premium::icon_text(Some(super::premium::Icon::Delete), body),
        )
        .await;
        return true;
    }

    if super::commands::contains(CONFIG, text) {
        if !sender_is_creator(ctx, message).await {
            if super::can_manage(ctx, message).await {
                super::respond(
                    ctx,
                    message,
                    crate::response::ResponseKind::PermissionDenied,
                    super::premium::icon_text(
                        Some(super::premium::Icon::Locked),
                        "فقط سازنده گروه می تواند «کانفیگ» را بفرستد.",
                    ),
                )
                .await;
            }
            return true;
        }
        let Ok(Some(chat_ref)) = message.peer_ref().await else {
            return false;
        };

        let standing = super::install::standing(ctx, chat_ref).await;
        if !super::install::ready(&standing) {
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::CommandError,
                match standing {
                    super::install::Standing::Unknown => InputMessage::new()
                        .text("نتوانستم دسترسی های خودم را بخوانم. چند لحظه بعد دوباره بفرستید."),
                    standing => super::premium::html(super::install::card(
                        &standing,
                        owner(ctx, chat).is_some(),
                    )),
                },
            )
            .await;
            return true;
        }

        let admin_names = super::admins(ctx, chat_ref).await.1;

        let locked = match super::autoconfig::apply_defaults(ctx, chat, sender).await {
            Ok(locked) => locked,
            Err(error) => {
                ::log::warn!("config: could not persist setup for {chat}: {error}");
                let text = match error {
                    crate::state::SettingsWriteError::CommitUncertain(_) => {
                        "نتیجه ذخیره سازی پیکربندی نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید و ربات را دوباره راه اندازی کنید."
                    }
                    _ => "پیکربندی ذخیره نشد؛ دوباره تلاش کنید.",
                };
                super::respond(
                    ctx,
                    message,
                    crate::response::ResponseKind::CommandError,
                    super::premium::icon_text(Some(super::premium::Icon::ErrorRed), text),
                )
                .await;
                return true;
            }
        };
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::SettingsChanged,
            super::premium::html(super::autoconfig::summary(
                "پیکربندی انجام شد",
                &esc(&name_of(message)),
                sender,
                &admin_names,
                &locked,
            )),
        )
        .await;
        super::install::ensure_cleaner(ctx, chat_ref, chat).await;
        return true;
    }

    if help(ctx, message).await {
        return true;
    }

    let Some((promote, arg)) = admin_command(text) else {
        return false;
    };

    let Some(named) = super::named(message, arg) else {
        return false;
    };
    if !is_owner(ctx, message) {
        if owner(ctx, chat).is_none() && can_manage(ctx, message).await {
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::PermissionDenied,
                "ابتدا سازنده گروه باید دستور «کانفیگ» را بفرستد.",
            )
            .await;
        }
        return true;
    }

    let Some((target, target_name)) = super::resolve(ctx, message, named).await else {
        super::respond(ctx, message, crate::response::ResponseKind::CommandError, super::premium::icon_text(Some(super::premium::Icon::ErrorRed), "کاربر پیدا نشد. روی پیام او ریپلای کنید یا «ترفیع @username» / «ترفیع 123456789» بفرستید.")).await;
        return true;
    };

    let Some(target_id) = target.id.bare_id() else {
        super::respond(
            ctx,
            message,
            crate::response::ResponseKind::CommandError,
            super::premium::icon_text(Some(super::premium::Icon::ErrorRed), "کاربر پیدا نشد."),
        )
        .await;
        return true;
    };
    let changed = ctx
        .settings
        .try_set(chat, &super::bot_admin_key(target_id), promote)
        .await;
    let changed = match changed {
        Ok(changed) => changed,
        Err(error) => {
            ::log::warn!("bot admin: write for {chat}/{target_id} failed: {error}");
            super::respond(
                ctx,
                message,
                crate::response::ResponseKind::CommandError,
                if error.commit_outcome_unknown() {
                    "نتیجه ذخیره ادمین ربات نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                } else {
                    "ادمین ربات ذخیره نشد؛ دوباره تلاش کنید."
                },
            )
            .await;
            return true;
        }
    };

    let by = name_of(message);
    let mark = if promote { "✓" } else { "✗" };
    let what = match (promote, changed) {
        (true, true) => "ادمین ربات شد",
        (true, false) => "از قبل ادمین ربات بود",
        (false, true) => "از ادمینی ربات عزل شد",
        (false, false) => "ادمین ربات نبود",
    };
    super::respond(
        ctx,
        message,
        crate::response::ResponseKind::SettingsChanged,
        super::premium::icon_text(
            Some(super::premium::Icon::User),
            format!("{mark} {target_name} {what}.\nتوسط مالک: {by}"),
        ),
    )
    .await;
    true
}

fn admin_command(text: &str) -> Option<(bool, Option<&str>)> {
    [(PROMOTE, true), (DEMOTE, false)]
        .into_iter()
        .find_map(|(commands, promote)| {
            commands.iter().find_map(|command| {
                let tail = super::commands::strip_prefix(text, command)?;
                if tail.is_empty() {
                    Some((promote, None))
                } else if tail.split_whitespace().count() == 1 && super::arg_names_a_user(tail) {
                    Some((promote, Some(tail)))
                } else {
                    None
                }
            })
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn persian_start_aliases_do_not_claim_conversation() {
        assert!(is_start("شروع"));
        assert!(is_start("استارت"));
        assert!(is_start("/START campaign"));
        assert!(!is_start("شروع کار"));
        assert!(!is_start("استارت پروژه"));
        assert!(!is_start("/start@otherbot campaign"));
    }

    #[test]
    fn admin_aliases_preserve_targets_and_require_a_complete_command() {
        for text in [
            "افزودن ادمین ربات @Some_User",
            "افزودن\tادمين\u{200c}ربات @Some_User",
            "/BoTPromote @Some_User",
        ] {
            assert_eq!(
                admin_command(text),
                Some((true, Some("@Some_User"))),
                "{text}"
            );
        }
        assert_eq!(admin_command("عزل ادمین ربات"), Some((false, None)));
        assert_eq!(admin_command("/botdemote ۱۲۳"), Some((false, Some("۱۲۳"))));
        for text in [
            "ترفیعش بده",
            "ترفیع خوب است",
            "/botpromote@otherbot @someone",
            "ترفیع @someone extra",
        ] {
            assert_eq!(admin_command(text), None, "{text}");
        }
        assert!(super::super::commands::contains(HELP, "/HELP"));
        assert!(!super::super::commands::contains(HELP, "/help@otherbot"));
    }
    use super::*;

    #[test]
    fn a_link_is_taken_in_whatever_shape_it_was_written() {
        assert_eq!(
            normalize_link("@mychannel").as_deref(),
            Some("https://t.me/mychannel")
        );
        assert_eq!(
            normalize_link("mychannel").as_deref(),
            Some("https://t.me/mychannel")
        );
        assert_eq!(
            normalize_link("https://t.me/joinchat/AAA").as_deref(),
            Some("https://t.me/joinchat/AAA")
        );

        assert_eq!(normalize_link(""), None);
        assert_eq!(normalize_link("@ab"), None);
        assert_eq!(normalize_link("my channel"), None);
        assert_eq!(normalize_link("https://nodot"), None);
        assert_eq!(normalize_link(" https://t.me/joinchat/AAA"), None);
        assert_eq!(normalize_link("https://example.com/a b"), None);
        assert_eq!(normalize_link("https://user:pass@example.com"), None);
    }
}
