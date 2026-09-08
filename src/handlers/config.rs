use grammers_client::message::{Button, InputMessage, Message};
use grammers_client::session::types::PeerId;

use super::{Ctx, can_manage, esc, is_owner, name_of, owner, sender_is_creator};

pub const OWNER: &str = "owner";

pub const CONFIG: &[&str] = &["کانفیگ", "پیکربندی"];
pub const ADMIN_LIST: &[&str] = &["لیست ادمین", "لیست ادمین ها", "ادمین ها"];
pub const PROMOTE: &[&str] = &["ترفیع"];
pub const DEMOTE: &[&str] = &["تنزل"];
pub const HELP: &[&str] = &["راهنما", "دستورها", "دستورات"];

const START: &[&str] = &["/start", "شروع"];

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
    let started = START.iter().any(|command| {
        text == *command
            || text
                .strip_prefix(command)
                .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    });
    if !started {
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

pub async fn help(ctx: &Ctx, message: &Message) -> bool {
    if !HELP.contains(&message.text().trim()) {
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

    if ADMIN_LIST.contains(&text) {
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

    if CONFIG.contains(&text) {
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

    let mut words = text.split_whitespace();
    let (Some(cmd), arg) = (words.next(), words.next()) else {
        return false;
    };
    let promote = PROMOTE.contains(&cmd);
    if (!promote && !DEMOTE.contains(&cmd)) || words.next().is_some() {
        return false;
    }

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

#[cfg(test)]
mod tests {
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
