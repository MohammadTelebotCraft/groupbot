use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::PeerId;

use super::{Ctx, can_manage, esc, is_owner, name_of, owner, sender_is_creator};

pub const OWNER: &str = "owner";

pub const CONFIG: &[&str] = &["کانفیگ", "پیکربندی"];
pub const ADMIN_LIST: &[&str] = &["لیست ادمین", "لیست ادمین ها", "ادمین ها"];
pub const PROMOTE: &[&str] = &["ترفیع"];
pub const DEMOTE: &[&str] = &["تنزل"];
pub const HELP: &[&str] = &["راهنما", "دستورها", "دستورات"];

const START: &[&str] = &["/start", "شروع"];

const LINKS: &[(&str, &str)] = &[
    ("CHANNEL", "📢  کانال"),
    ("SUPPORT", "💬  پشتیبانی"),
    ("SOURCE", "🧩  سورس"),
];

fn link(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value
        .strip_prefix("https://")
        .or(value.strip_prefix("http://"))
    {
        return rest
            .contains('.')
            .then(|| value.split_whitespace().next().unwrap_or(value).to_owned());
    }
    let handle = value.trim_start_matches('@');
    let usable = handle.len() >= 5
        && handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    usable.then(|| format!("https://t.me/{handle}"))
}

async fn own_username(ctx: &Ctx) -> Option<&'static str> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    if let Some(known) = CACHE.get() {
        return known.as_deref();
    }
    let fetched = ctx
        .client
        .get_me()
        .await
        .ok()
        .and_then(|me| me.username().map(str::to_owned));

    CACHE.get_or_init(|| fetched).as_deref()
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
        ctx.settings.remember_started_user(user, access_hash).await;
    }
    let mut card = InputMessage::new().html(format!(
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
    ));

    let mut rows: Vec<Vec<Button>> = Vec::new();
    if let Some(username) = own_username(ctx).await {
        rows.push(vec![Button::url(
            "➕  افزودن ربات به گروه",
            format!("https://t.me/{username}?startgroup=new"),
        )]);
    }

    let links: Vec<(&str, String)> = LINKS
        .iter()
        .filter_map(|(name, label)| Some((*label, link(name)?)))
        .collect();
    for pair in links.chunks(2) {
        rows.push(
            pair.iter()
                .map(|(label, url)| Button::url(*label, url.clone()))
                .collect(),
        );
    }
    if let Some(user) = message.sender_id().and_then(PeerId::bare_id) {
        let chat = message.peer_id().bot_api_dialog_id().unwrap_or(0);
        rows.push(vec![super::panel::help_button(user, chat)]);
    }
    if !rows.is_empty() {
        card = card.reply_markup(ReplyMarkup::from_buttons(&rows));
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
    let _ = message
        .reply(
            InputMessage::new()
                .html(super::help::index())
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
        let _ = message
            .reply(InputMessage::new().html(format!(
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
            )))
            .await;
        let _ = creator;
        return true;
    }

    if CONFIG.contains(&text) {
        if !sender_is_creator(ctx, message).await {
            if super::can_manage(ctx, message).await {
                let _ = message
                    .reply("فقط سازنده گروه می تواند «کانفیگ» را بفرستد.")
                    .await;
            }
            return true;
        }
        let Ok(Some(chat_ref)) = message.peer_ref().await else {
            return false;
        };

        let standing = super::install::standing(ctx, chat_ref).await;
        if !super::install::ready(&standing) {
            let _ = message
                .reply(match standing {
                    super::install::Standing::Unknown => InputMessage::new()
                        .text("نتوانستم دسترسی های خودم را بخوانم. چند لحظه بعد دوباره بفرستید."),
                    standing => InputMessage::new()
                        .html(super::install::card(&standing, owner(ctx, chat).is_some())),
                })
                .await;
            return true;
        }

        ctx.settings
            .set_value(chat, OWNER, &sender.to_string())
            .await;
        let admin_names = super::admins(ctx, chat_ref).await.1;

        let locked = super::autoconfig::apply_defaults(ctx, chat).await;
        let _ = message
            .reply(InputMessage::new().html(super::autoconfig::summary(
                "پیکربندی انجام شد",
                &esc(&name_of(message)),
                sender,
                &admin_names,
                &locked,
            )))
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
            let _ = message
                .reply("ابتدا سازنده گروه باید دستور «کانفیگ» را بفرستد.")
                .await;
        }
        return true;
    }

    let Some((target, target_name)) = super::resolve(ctx, message, named).await else {
        let _ = message
            .reply("کاربر پیدا نشد. روی پیام او ریپلای کنید یا «ترفیع @username» / «ترفیع 123456789» بفرستید.")
            .await;
        return true;
    };

    let Some(target_id) = target.id.bare_id() else {
        let _ = message.reply("کاربر پیدا نشد.").await;
        return true;
    };
    let changed = ctx
        .settings
        .set(chat, &super::bot_admin_key(target_id), promote)
        .await;

    let by = name_of(message);
    let mark = if promote { "✓" } else { "✗" };
    let what = match (promote, changed) {
        (true, true) => "ادمین ربات شد",
        (true, false) => "از قبل ادمین ربات بود",
        (false, true) => "از ادمینی ربات عزل شد",
        (false, false) => "ادمین ربات نبود",
    };
    let _ = message
        .reply(format!("{mark} {target_name} {what}.\nتوسط مالک: {by}"))
        .await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_taken_in_whatever_shape_it_was_written() {
        unsafe {
            std::env::set_var("GB_TEST_LINK", "@mychannel");
            assert_eq!(
                link("GB_TEST_LINK").as_deref(),
                Some("https://t.me/mychannel")
            );

            std::env::set_var("GB_TEST_LINK", "mychannel");
            assert_eq!(
                link("GB_TEST_LINK").as_deref(),
                Some("https://t.me/mychannel")
            );

            std::env::set_var("GB_TEST_LINK", "  https://t.me/joinchat/AAA  ");
            assert_eq!(
                link("GB_TEST_LINK").as_deref(),
                Some("https://t.me/joinchat/AAA")
            );

            std::env::set_var("GB_TEST_LINK", "");
            assert_eq!(link("GB_TEST_LINK"), None);
            std::env::set_var("GB_TEST_LINK", "@ab");
            assert_eq!(link("GB_TEST_LINK"), None);
            std::env::set_var("GB_TEST_LINK", "my channel");
            assert_eq!(link("GB_TEST_LINK"), None);
            std::env::set_var("GB_TEST_LINK", "https://nodot");
            assert_eq!(link("GB_TEST_LINK"), None);
            std::env::remove_var("GB_TEST_LINK");
            assert_eq!(link("GB_TEST_LINK"), None);
        }
    }
}
