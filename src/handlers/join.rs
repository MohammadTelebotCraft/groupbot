use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::update::CallbackQuery;

use super::{Ctx, esc};

pub const CHANNEL: &str = "join_channel";

pub const GATE: &str = "gate_on";

pub const EXEMPT: &str = "gate_free:";

pub const PROMPT_EVERY: &str = "gate_every";

pub const PROMPT_TTL: &str = "gate_ttl";
pub const EVERY_PRESETS: &[u32] = &[0, 30, 60, 120, 300, 900];
pub const EVERY_RANGE: (u32, u32) = (0, 3600);
pub const TTL_PRESETS: &[u32] = &[0, 10, 30, 60, 300];
pub const TTL_RANGE: (u32, u32) = (0, 3600);
const DEFAULT_EVERY: u32 = 120;
const DEFAULT_TTL: u32 = 30;

pub const ADD_REQUIRED: &str = "add_required";
pub const ADD_PRESETS: &[u32] = &[0, 1, 3, 5, 10, 20];
pub const ADD_RANGE: (u32, u32) = (0, 1000);

pub const SET: &[&str] = &["تنظیم عضویت اجباری", "عضویت اجباری"];
pub const CLEAR: &[&str] = &["حذف عضویت اجباری", "خاموش عضویت اجباری"];
pub const SET_ADD: &[&str] = &["تنظیم اد اجباری", "اد اجباری", "عضوگیری اجباری"];
pub const SET_PROMPT: &[&str] = &["تنظیم اعلان شرط", "اعلان شرط"];
pub const FREE_ADD: &[&str] = &["معاف", "تنظیم معاف", "افزودن معاف"];
pub const FREE_REMOVE: &[&str] = &["حذف معاف", "لغو معاف"];
pub const CLEAR_ADD: &[&str] = &["حذف اد اجباری", "خاموش اد اجباری"];

pub fn channel(ctx: &Ctx, chat: i64) -> Option<String> {
    ctx.settings
        .value(chat, CHANNEL)
        .filter(|name| !name.is_empty())
}

pub fn required_adds(ctx: &Ctx, chat: i64) -> u64 {
    ctx.settings.value_parsed(chat, ADD_REQUIRED).unwrap_or(0)
}

pub fn exempt_key(user: i64) -> String {
    format!("{EXEMPT}{user}")
}

pub fn is_free(ctx: &Ctx, chat: i64, user: i64) -> bool {
    ctx.settings.is_locked(chat, &exempt_key(user))
}

pub async fn set_free(ctx: &Ctx, chat: i64, user: i64, on: bool) -> bool {
    ctx.settings.set(chat, &exempt_key(user), on).await
}

pub fn prompt_every(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value_parsed(chat, PROMPT_EVERY)
        .unwrap_or(DEFAULT_EVERY)
}

pub fn prompt_ttl(ctx: &Ctx, chat: i64) -> u32 {
    ctx.settings
        .value_parsed(chat, PROMPT_TTL)
        .unwrap_or(DEFAULT_TTL)
}

pub async fn set_prompt(ctx: &Ctx, chat: i64, key: &str, seconds: u32) {
    ctx.settings
        .set_value(chat, key, &seconds.to_string())
        .await;
}

pub fn seconds_label(seconds: u32, off: &'static str) -> String {
    match seconds {
        0 => off.to_owned(),
        s if s < 60 => format!("{s} ثانیه"),
        s => format!("{} دقیقه", s / 60),
    }
}

pub async fn set_required_adds(ctx: &Ctx, chat: i64, count: u64) {
    ctx.settings
        .set_value(chat, ADD_REQUIRED, &count.to_string())
        .await;
    sync_gate(ctx, chat).await;
}

pub async fn set_channel(ctx: &Ctx, chat: i64, name: &str) {
    if name.is_empty() {
        ctx.settings.set(chat, CHANNEL, false).await;
    } else {
        ctx.settings.set_value(chat, CHANNEL, name).await;
    }
    sync_gate(ctx, chat).await;
}

async fn sync_gate(ctx: &Ctx, chat: i64) {
    let on = channel(ctx, chat).is_some() || required_adds(ctx, chat) > 0;
    ctx.settings.set(chat, GATE, on).await;
}

pub async fn prime(ctx: &Ctx) {
    let mut gated = ctx.settings.chats_with(CHANNEL).await;
    gated.extend(ctx.settings.chats_with(ADD_REQUIRED).await);
    gated.extend(ctx.settings.flagged_with(GATE).await);
    gated.sort_unstable();
    gated.dedup();
    for chat in gated {
        sync_gate(ctx, chat).await;
    }
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if CLEAR.contains(&text) {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        set_channel(ctx, chat, "").await;
        let _ = message.reply("✗ عضویت اجباری برداشته شد.").await;
        return true;
    }

    if CLEAR_ADD.contains(&text) {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        set_required_adds(ctx, chat, 0).await;
        let _ = message.reply("✗ اد اجباری برداشته شد.").await;
        return true;
    }

    if let Some((add, arg)) = FREE_ADD
        .iter()
        .map(|c| (true, *c))
        .chain(FREE_REMOVE.iter().map(|c| (false, *c)))
        .find_map(|(add, command)| {
            let rest = text.strip_prefix(command)?;
            match rest.is_empty() {
                true => Some((add, None)),
                false => rest
                    .starts_with(char::is_whitespace)
                    .then(|| (add, Some(rest.trim())))
                    .filter(|(_, arg)| arg.is_none_or(|a| !a.contains(' '))),
            }
        })
    {
        let Some(named) = super::named(message, arg) else {
            return false;
        };
        if !super::limits::allows(ctx, message, super::limits::EXEMPT).await {
            return true;
        }
        let Some((target, target_name)) = super::resolve(ctx, message, named).await else {
            let _ = message
                .reply("کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.")
                .await;
            return true;
        };
        let Some(target_id) = target.id.bare_id() else {
            let _ = message.reply("کاربر پیدا نشد.").await;
            return true;
        };
        let changed = set_free(ctx, chat, target_id, add).await;
        let mark = if add { "✓" } else { "✗" };
        let what = match (add, changed) {
            (true, true) => "از شرط های ورود معاف شد",
            (true, false) => "از قبل معاف بود",
            (false, true) => "از لیست معاف حذف شد",
            (false, false) => "در لیست معاف نبود",
        };
        let _ = message.reply(format!("{mark} {target_name} {what}.")).await;
        return true;
    }

    if let Some(rest) = SET_PROMPT.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
    }) {
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        let Some(numbers) = super::numbers_in(rest) else {
            return false;
        };
        if let [every, ttl] = numbers[..] {
            set_prompt(ctx, chat, PROMPT_EVERY, every).await;
            set_prompt(ctx, chat, PROMPT_TTL, ttl).await;
        }
        let _ = message
            .reply(InputMessage::new().html(format!(
                "<b>اعلان شرط</b>\n\n\
                 فاصله بین دو اعلان · <b>{}</b>\n\
                 حذف خودکار اعلان · <b>{}</b>\n\n\
                 <i>تنظیم: «تنظیم اعلان شرط 120 30» یعنی هر ۱۲۰ ثانیه یک بار، حذف بعد از ۳۰ ثانیه.</i>",
                seconds_label(prompt_every(ctx, chat), "هر بار"),
                seconds_label(prompt_ttl(ctx, chat), "بدون حذف"),
            )))
            .await;
        return true;
    }

    if let Some(rest) = SET_ADD.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
    }) {
        let count = match super::numbers_in(rest).as_deref() {
            Some(&[count]) => Some(u64::from(count)),
            Some([]) if rest.is_empty() => None,
            _ => return false,
        };
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        let _ = match count {
            Some(count) => {
                set_required_adds(ctx, chat, count).await;
                message
                    .reply(match count {
                        0 => "✗ اد اجباری برداشته شد.".to_owned(),
                        n => format!("✓ اد اجباری روی {n} نفر تنظیم شد."),
                    })
                    .await
            }
            None => {
                message
                    .reply(InputMessage::new().html(format!(
                        "<b>اد اجباری</b>\n\n\
                         {}\n\n\
                         <i>تنظیم: «تنظیم اد اجباری 5» · برداشتن: «حذف اد اجباری»</i>",
                        match required_adds(ctx, chat) {
                            0 => "خاموش است.".to_owned(),
                            n => format!("هر عضو باید <b>{n}</b> نفر اضافه کند."),
                        }
                    )))
                    .await
            }
        };
        return true;
    }

    let Some(rest) = SET.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
    }) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }

    if rest.is_empty() {
        let _ = message
            .reply(InputMessage::new().html(match channel(ctx, chat) {
                Some(name) => format!(
                    "<b>عضویت اجباری</b>\n\n\
                     کانال · @{}\n\n\
                     <i>برداشتن: «حذف عضویت اجباری»</i>",
                    esc(&name)
                ),
                None => "<b>عضویت اجباری</b>\n\n\
                     خاموش است. «تنظیم عضویت اجباری @channel» را بفرستید.\n\n\
                     <i>ربات باید در آن کانال ادمین باشد تا بتواند عضویت را ببیند.</i>"
                    .to_owned(),
            }))
            .await;
        return true;
    }

    let Some(name) = rest.strip_prefix('@').map(|n| n.trim().to_lowercase()) else {
        return false;
    };
    if name.is_empty() {
        return false;
    }

    let Ok(Some(_)) = ctx.client.resolve_username(&name).await else {
        let _ = message
            .reply("کانال پیدا نشد. یوزرنیم کانال را مثل «تنظیم عضویت اجباری @channel» بفرستید.")
            .await;
        return true;
    };
    set_channel(ctx, chat, &name).await;
    let _ = message
        .reply(InputMessage::new().html(format!(
            "✓ عضویت اجباری روی @{} فعال شد.\n\n\
             <i>ربات باید در کانال ادمین باشد، وگرنه عضویت کسی را نمی بیند.</i>",
            esc(&name)
        )))
        .await;
    true
}

pub async fn enforce(ctx: &std::sync::Arc<Ctx>, message: &Message) -> bool {
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if !ctx.settings.is_locked(chat, GATE) {
        return false;
    }
    let channel = channel(ctx, chat);
    let needed = required_adds(ctx, chat);
    if channel.is_none() && needed == 0 {
        return false;
    }
    let Some(user) = message.sender_id().and_then(PeerId::bare_id) else {
        return false;
    };

    if message.action().is_some() {
        return false;
    }

    if is_free(ctx, chat, user) {
        return false;
    }
    if super::is_exempt(ctx, message).await {
        return false;
    }
    let added = super::stats::adds(ctx, chat, user).await;
    let owes_adds = added < needed;
    let missing_channel = match &channel {
        Some(_) if ctx.channel_member(chat, user) => false,
        Some(name) => {
            let member = is_member(ctx, name, message.sender_ref().await.ok().flatten()).await;
            if member {
                ctx.remember_member(chat, user);
            }
            !member
        }
        None => false,
    };
    if !owes_adds && !missing_channel {
        return false;
    }

    let chat_ref = match message.peer_ref().await {
        Ok(Some(peer)) => Some(peer),
        _ => ctx.chat_ref(chat),
    };
    if let Err(e) = message.delete().await {
        eprintln!("join lock: {chat}: could not delete: {e}");
        return false;
    }
    let every = std::time::Duration::from_secs(u64::from(prompt_every(ctx, chat)));
    if let Some(chat_ref) = chat_ref.filter(|_| ctx.may_notify_every(chat, user, every)) {
        let mut conditions = Vec::new();
        if let Some(name) = channel.as_deref().filter(|_| missing_channel) {
            conditions.push(format!("‹ عضو @{} شوید", esc(name)));
        }
        if owes_adds {
            conditions.push(format!(
                "‹ {needed} نفر به گروه اضافه کنید · تا اینجا <b>{added}</b>"
            ));
        }
        let mut rows = Vec::new();
        if let Some(name) = channel.as_deref().filter(|_| missing_channel) {
            rows.push(vec![Button::url(
                "عضویت در کانال",
                format!("https://t.me/{name}"),
            )]);
        }
        rows.push(vec![
            Button::data("بررسی مجدد", format!("j:{chat}")),
            Button::data("معاف کن", format!("jx:{chat}:{user}")),
        ]);

        let sent = ctx
            .client
            .send_message(
                chat_ref,
                InputMessage::new()
                    .html(format!(
                        "<b>شرط نوشتن در گروه</b>\n\n\
                         <a href=\"tg://user?id={user}\">{}</a> عزیز، برای نوشتن باید:\n\
                         {}\n\n\
                         <i>پس از انجام، دکمه بررسی را بزنید.</i>",
                        esc(&super::name_of(message)),
                        conditions.join("\n"),
                    ))
                    .reply_markup(ReplyMarkup::from_buttons(&rows)),
            )
            .await;

        let ttl = prompt_ttl(ctx, chat);
        if let (Ok(sent), true) = (sent, ttl > 0) {
            let ctx = std::sync::Arc::clone(ctx);
            let id = sent.id();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(u64::from(ttl))).await;
                let _ = ctx.client.delete_messages(chat_ref, &[id]).await;
            });
        }
    }
    true
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str) {
    let Some(chat) = payload.parse::<i64>().ok() else {
        return;
    };
    let needed = required_adds(ctx, chat);
    let added = match query.sender_id().bare_id() {
        Some(user) => super::stats::adds(ctx, chat, user).await,
        None => 0,
    };

    if let Some(user) = query.sender_id().bare_id() {
        ctx.forget_member(chat, user);
    }
    let member = match channel(ctx, chat) {
        Some(name) => is_member(ctx, &name, query.sender_ref().await.ok().flatten()).await,
        None => true,
    };
    if member && let Some(user) = query.sender_id().bare_id() {
        ctx.remember_member(chat, user);
    }
    let _ = query
        .answer()
        .alert(match (member, added >= needed) {
            (true, true) => "✓ شرط ها انجام شد. حالا می توانید بنویسید.".to_owned(),
            (false, _) => "هنوز عضو کانال نیستید.".to_owned(),
            (true, false) => format!("هنوز {added} از {needed} نفر را اضافه کرده اید."),
        })
        .send()
        .await;
}

pub async fn on_exempt(ctx: &Ctx, query: &CallbackQuery, payload: &str) {
    let Some((chat, user)) = payload.split_once(':') else {
        return;
    };
    let (Ok(chat), Ok(user)) = (chat.parse::<i64>(), user.parse::<i64>()) else {
        return;
    };
    set_free(ctx, chat, user, true).await;
    let _ = query
        .answer()
        .edit(InputMessage::new().html(format!(
            "✓ <a href=\"tg://user?id={user}\">این کاربر</a> از شرط های ورود معاف شد."
        )))
        .await;
}

async fn is_member(ctx: &Ctx, name: &str, user: Option<PeerRef>) -> bool {
    let Some(user) = user else {
        return true;
    };
    let Some(channel) = channel_ref(ctx, name).await else {
        return true;
    };
    match ctx.client.get_permissions(channel, user).await {
        Ok(permissions) => !permissions.has_left() && !permissions.is_banned(),
        Err(grammers_client::InvocationError::Rpc(error))
            if error.name.contains("USER_NOT_PARTICIPANT") =>
        {
            false
        }
        Err(e) => {
            eprintln!("join lock: cannot read @{name}: {e}");
            true
        }
    }
}

async fn channel_ref(ctx: &Ctx, name: &str) -> Option<PeerRef> {
    if let Some(peer) = ctx.join_ref(name) {
        return Some(peer);
    }
    let peer = ctx.client.resolve_username(name).await.ok()??;
    let peer = peer.to_ref().await.ok()??;
    ctx.remember_join_ref(name, peer);
    Some(peer)
}
