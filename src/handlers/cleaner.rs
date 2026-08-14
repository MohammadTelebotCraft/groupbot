use std::time::Duration;

use grammers_client::Client;
use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerRef};

use super::Ctx;

const START: &[&str] = &["ورود کلینر", "لاگین کلینر", "کلینر"];
const ADD: &[&str] = &["افزودن کلینر", "اضافه کردن کلینر", "نصب کلینر"];
const WIPE: &[&str] = &["حذف پیام", "حذف پیام ها", "پاکسازی پیام"];
const SWEEP: &[&str] = &["پاکسازی", "پاک کردن همه"];

const SWEEP_MAX: usize = 3_000;

fn media_filter(name: &str) -> Option<(&'static str, grammers_client::tl::enums::MessagesFilter)> {
    use grammers_client::tl::types::*;
    let found = match name {
        "عکس" | "تصویر" => ("عکس", InputMessagesFilterPhotos {}.into()),
        "ویدیو" | "فیلم" | "ویدئو" => ("ویدیو", InputMessagesFilterVideo {}.into()),
        "مدیا" | "رسانه" => ("مدیا", InputMessagesFilterPhotoVideo {}.into()),
        "گیف" => ("گیف", InputMessagesFilterGif {}.into()),
        "ویس" | "صدا" => ("ویس", InputMessagesFilterVoice {}.into()),
        "موزیک" | "آهنگ" | "اهنگ" => ("موزیک", InputMessagesFilterMusic {}.into()),
        "فایل" | "سند" | "استیکر" => ("فایل", InputMessagesFilterDocument {}.into()),
        "لینک" => ("لینک", InputMessagesFilterUrl {}.into()),
        "مکان" | "لوکیشن" => ("مکان", InputMessagesFilterGeo {}.into()),
        "مخاطب" | "کانتکت" => ("مخاطب", InputMessagesFilterContacts {}.into()),
        "نظرسنجی" => ("نظرسنجی", InputMessagesFilterPoll {}.into()),
        "ویدیو پیام" | "ویدئو پیام" => ("ویدیو پیام", InputMessagesFilterRoundVideo {}.into()),
        _ => return None,
    };
    Some(found)
}

const SEARCH_LIMIT: usize = 5_000;

const DEADLINE: Duration = Duration::from_secs(300);

const CHUNK: usize = 100;

const RANGE_MAX: i32 = 10_000;

pub fn sudo() -> Option<i64> {
    std::env::var("SUDO_ID").ok()?.parse().ok()
}

pub async fn add(ctx: &Ctx, message: &Message) -> bool {
    if !ADD.contains(&message.text().trim()) {
        return false;
    }
    if !super::can_manage(ctx, message).await {
        return true;
    }
    let (Some(cleaner), Some(user)) = (ctx.cleaner_id(), ctx.user_client()) else {
        let _ = message
            .reply("کلینر وارد نشده است. مالک ربات باید در پیوی «ورود کلینر» را بفرستد.")
            .await;
        return true;
    };
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };
    let Some(target) = PeerId::user(cleaner).map(PeerId::to_ambient_ref) else {
        return true;
    };

    if let Ok(me) = ctx.client.get_me().await
        && let Some(me_ref) = PeerId::user(me.id().bare_id_unchecked()).map(PeerId::to_ambient_ref)
        && let Ok(mine) = ctx.client.get_permissions(chat_ref, me_ref).await
    {
        let missing = match (mine.is_admin(), mine.can_add_admins()) {
            (false, _) => Some("اول ربات را ادمین کنید، با دسترسی کامل."),
            (true, false) => Some("ربات دسترسی «افزودن ادمین» ندارد؛ آن را بدهید و دوباره بفرستید."),
            _ => None,
        };
        if let Some(missing) = missing {
            let _ = message.reply(missing).await;
            return true;
        }
    }

    let _ = message.reply("در حال افزودن کلینر...").await;

    if let Ok(state) = ctx.client.get_permissions(chat_ref, target).await
        && state.is_banned()
        && let Err(e) = super::restrict::apply(
            ctx,
            chat_ref,
            target,
            super::restrict::Action::Unban,
            None,
            super::restrict::By {
                reason: "افزودن کلینر",
                target_name: "کلینر",
                ..Default::default()
            },
        )
        .await
    {
        let _ = message.reply(format!("رفع بن کلینر انجام نشد · {e}")).await;
        return true;
    }

    let member = matches!(
        ctx.client.get_permissions(chat_ref, target).await,
        Ok(state) if !state.has_left() && !state.is_banned()
    );
    if !member && let Err(reason) = join(ctx, &user, chat_ref).await {
        let _ = message.reply(reason).await;
        return true;
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    if let Ok(state) = ctx.client.get_permissions(chat_ref, target).await
        && state.is_admin()
    {
        let _ = message
            .reply("✓ کلینر در گروه است و از قبل ادمین بود. «حذف 999» پیام های قدیمی را هم پاک می کند.")
            .await;
        ctx.forget_user_chats();
        return true;
    }

    match promote(ctx, chat_ref, target).await {
        Ok(()) => {
            let _ = message
                .reply("✓ کلینر اضافه و ادمین شد. حالا «حذف 999» پیام های قدیمی را هم پاک می کند.")
                .await;
        }
        Err(e) => {
            let _ = message
                .reply(format!(
                    "کلینر وارد گروه شد ولی ادمین نشد · {e}
ربات باید دسترسی «افزودن ادمین» داشته باشد."
                ))
                .await;
        }
    }

    ctx.forget_user_chats();
    true
}

async fn join(ctx: &Ctx, user: &Client, chat_ref: PeerRef) -> std::result::Result<(), String> {
    let exported = ctx
        .client
        .invoke(&grammers_client::tl::functions::messages::ExportChatInvite {
            legacy_revoke_permanent: false,
            request_needed: false,
            peer: chat_ref.into(),
            expire_date: None,
            usage_limit: Some(1),
            title: Some("cleaner".to_owned()),
            subscription_pricing: None,
        })
        .await
        .map_err(|e| match e.to_string().contains("CHAT_ADMIN_REQUIRED") {
            true => "ربات دسترسی «افزودن اعضا» ندارد؛ آن را بدهید و دوباره بفرستید.".to_owned(),
            false => format!("ساخت لینک دعوت انجام نشد · {e}"),
        })?;
    let link = match exported {
        grammers_client::tl::enums::ExportedChatInvite::ChatInviteExported(invite) => invite.link,
        _ => return Err("لینک دعوت ساخته نشد.".to_owned()),
    };
    match user.accept_invite_link(&link).await {
        Ok(Some(_)) => Ok(()),

        Ok(None) => Err("درخواست عضویت فرستاده شد؛ آن را در گروه تایید کنید.".to_owned()),
        Err(e) if e.to_string().contains("USER_ALREADY_PARTICIPANT") => Ok(()),
        Err(e) if e.to_string().contains("INVITE_REQUEST_SENT") => {
            Err("درخواست عضویت فرستاده شد؛ آن را در گروه تایید کنید.".to_owned())
        }
        Err(e) if e.to_string().contains("USER_BANNED_IN_CHANNEL") => {
            Err("حساب کلینر در این گروه بن است و رفع بن نشد.".to_owned())
        }
        Err(e) if e.to_string().contains("CHANNELS_TOO_MUCH") => {
            Err("حساب کلینر در گروه های زیادی است و جای خالی ندارد.".to_owned())
        }
        Err(e) if e.to_string().contains("FLOOD_WAIT") => {
            Err(format!("تلگرام فعلا اجازه نمی دهد · {e}"))
        }
        Err(e) => Err(format!("پیوستن کلینر انجام نشد · {e}")),
    }
}

pub async fn on_join(ctx: &Ctx, message: &Message) -> bool {
    let Some(cleaner) = ctx.cleaner_id() else {
        return false;
    };
    if !matches!(
        message.action(),
        Some(grammers_client::tl::enums::MessageAction::ChatAddUser(_))
            | Some(grammers_client::tl::enums::MessageAction::ChatJoinedByLink(_))
    ) {
        return false;
    }
    let Some(joined) = super::joined_users(ctx, message)
        .await
        .into_iter()
        .find(|joined| joined.id == cleaner)
    else {
        return false;
    };
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };
    if let Err(e) = promote(ctx, chat_ref, joined.peer).await {
        eprintln!("cleaner: could not promote in {}: {e}", chat_ref.id);
    }
    ctx.forget_user_chats();
    false
}

pub async fn purge_history(
    ctx: &Ctx,
    chat: i64,
    max_id: i32,
) -> std::result::Result<usize, String> {
    let mut refused = None;
    if let Some(user) = ctx.user_client() {
        match chat_ref(ctx, &user, chat).await {
            Some(chat_ref) => {
                match delete_history_call(&user, chat_ref, max_id).await {
                    Ok(()) => return Ok(max_id as usize),
                    Err(e) => {
                        let deleted = delete_range(&user, chat_ref, 1, max_id).await;
                        if deleted > 0 {
                            return Ok(deleted);
                        }
                        refused = Some(format!("کلینر · {e}"));
                    }
                }
            }
            None => refused = Some("کلینر در این گروه نیست".to_owned()),
        }
    }

    let Some(chat_ref) = ctx.chat_ref(chat) else {
        return Err(refused.unwrap_or_else(|| "گروه شناخته نشد".to_owned()));
    };
    match delete_range(&ctx.client, chat_ref, 1, max_id).await {
        0 => Err(match refused {
            Some(refused) => format!("{refused}\nربات هم نتوانست"),
            None => "ربات اجازه حذف ندارد".to_owned(),
        }),
        deleted => Ok(deleted),
    }
}

async fn delete_range(client: &Client, chat_ref: PeerRef, first: i32, last: i32) -> usize {
    let first = first.max(last - RANGE_MAX + 1).max(1);
    let ids: Vec<i32> = (first..=last).collect();
    let mut deleted = 0;
    for chunk in ids.chunks(CHUNK) {
        match client.delete_messages(chat_ref, chunk).await {
            Ok(n) => deleted += n,
            Err(e) => {
                eprintln!("cleaner: delete range stopped at {}: {e}", chunk[0]);
                break;
            }
        }
    }
    deleted
}

async fn delete_history_call(
    client: &Client,
    chat_ref: PeerRef,
    max_id: i32,
) -> std::result::Result<(), grammers_client::InvocationError> {
    client
        .invoke(&grammers_client::tl::functions::channels::DeleteHistory {
            for_everyone: true,
            channel: chat_ref.into(),
            max_id,
        })
        .await
        .map(|_| ())
}

pub async fn sweep(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(rest) = SWEEP.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        rest.starts_with(char::is_whitespace).then(|| rest.trim())
    }) else {
        return false;
    };
    let Some((label, filter)) = media_filter(rest) else {
        return false;
    };
    if !super::can_manage(ctx, message).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    let (Some(user), Some(_)) = (ctx.user_client(), ctx.cleaner_id()) else {
        let _ = message
            .reply("برای پاکسازی قدیمی ها کلینر لازم است. «افزودن کلینر» را بفرستید.")
            .await;
        return true;
    };
    let Some(chat_ref) = chat_ref(ctx, &user, chat).await else {
        let _ = message
            .reply("کلینر در این گروه نیست. «افزودن کلینر» را بفرستید.")
            .await;
        return true;
    };

    let _ = message.reply(format!("در حال پاکسازی {label}...")).await;
    let mut ids: Vec<i32> = Vec::new();
    let mut search = user.search_messages(chat_ref).filter(filter);
    let mut failed = None;
    while ids.len() < SWEEP_MAX {
        match search.next().await {
            Ok(Some(found)) => ids.push(found.id()),
            Ok(None) => break,
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    if let Some(e) = failed.filter(|_| ids.is_empty()) {
        let _ = message
            .reply(format!(
                "انجام نشد · {e}\nکلینر باید در گروه ادمین باشد."
            ))
            .await;
        return true;
    }
    if ids.is_empty() {
        let _ = message.reply(format!("چیزی از نوع {label} پیدا نشد.")).await;
        return true;
    }

    let mut deleted = 0;
    for chunk in ids.chunks(CHUNK) {
        match user.delete_messages(chat_ref, chunk).await {
            Ok(n) => deleted += n,
            Err(e) => {
                let _ = message
                    .reply(format!("{deleted} پیام پاک شد، بعد خطا داد · {e}"))
                    .await;
                return true;
            }
        }
    }
    let _ = message
        .reply(match ids.len() >= SWEEP_MAX {
            true => format!(
                "✓ {deleted} {label} پاک شد (سقف هر بار {SWEEP_MAX} تا). برای بقیه دوباره بفرستید."
            ),
            false => format!("✓ {deleted} {label} پاک شد."),
        })
        .await;
    true
}

pub async fn wipe(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    let Some(arg) = WIPE.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
    }) else {
        return false;
    };
    let arg = (!arg.is_empty()).then_some(arg);
    let Some(named) = super::named(message, arg) else {
        return false;
    };
    if !super::can_manage(ctx, message).await {
        return true;
    }
    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        let _ = message
            .reply("کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.")
            .await;
        return true;
    };
    let (Ok(Some(chat_ref)), Some(chat), Some(user_id)) = (
        message.peer_ref().await,
        message.peer_id().bot_api_dialog_id(),
        target.id.bare_id(),
    ) else {
        return false;
    };
    if super::owner(ctx, chat) == Some(user_id) {
        let _ = message.reply("پیام های مالک ربات پاک نمی شود.").await;
        return true;
    }

    let done = match wipe_as_cleaner(ctx, chat, user_id, arg).await {
        Some(result) => result,

        None => delete_history(&ctx.client, chat_ref, target).await,
    };
    let _ = match done {
        Ok(()) => {
            message
                .reply(format!("✓ پیام های {name} در این گروه پاک شد."))
                .await
        }
        Err(e) => {
            eprintln!("wipe: {chat}: {e}");
            message
                .reply(format!(
                    "انجام نشد · {e}\n\
                     برای پیام های قدیمی «افزودن کلینر» را بفرستید."
                ))
                .await
        }
    };
    true
}

async fn wipe_as_cleaner(
    ctx: &Ctx,
    chat: i64,
    user_id: i64,
    arg: Option<&str>,
) -> Option<std::result::Result<(), grammers_client::InvocationError>> {
    let user = ctx.user_client()?;
    let chat_ref = chat_ref(ctx, &user, chat).await?;
    let target = member_ref(&user, chat_ref, user_id, arg).await?;
    Some(delete_history(&user, chat_ref, target).await)
}

async fn member_ref(
    user: &Client,
    chat_ref: PeerRef,
    user_id: i64,
    arg: Option<&str>,
) -> Option<PeerRef> {
    if let Some(username) = arg.and_then(|arg| arg.strip_prefix('@'))
        && let Ok(Some(peer)) = user.resolve_username(username).await
        && let Ok(Some(peer_ref)) = peer.to_ref().await
    {
        return Some(peer_ref);
    }
    let mut participants = user.iter_participants(chat_ref);
    let mut seen = 0;
    while let Ok(Some(participant)) = participants.next().await {
        if participant.user.id().bare_id_unchecked() == user_id {
            return participant.user.to_ref().await.ok().flatten();
        }
        seen += 1;
        if seen >= SEARCH_LIMIT {
            break;
        }
    }
    None
}

async fn delete_history(
    client: &Client,
    chat_ref: PeerRef,
    target: PeerRef,
) -> std::result::Result<(), grammers_client::InvocationError> {
    client
        .invoke(
            &grammers_client::tl::functions::channels::DeleteParticipantHistory {
                channel: chat_ref.into(),
                participant: target.into(),
            },
        )
        .await
        .map(|_| ())
}

pub async fn purge(ctx: &Ctx, chat: i64, first: i32, last: i32) -> Option<usize> {
    let user = ctx.user_client()?;
    let chat_ref = chat_ref(ctx, &user, chat).await?;
    let ids: Vec<i32> = (first..=last).collect();
    let mut deleted = 0;
    for chunk in ids.chunks(CHUNK) {
        match user.delete_messages(chat_ref, chunk).await {
            Ok(n) => deleted += n,
            Err(e) => {
                eprintln!("cleaner: could not delete in {chat}: {e}");

                return (deleted > 0).then_some(deleted);
            }
        }
    }
    Some(deleted)
}

async fn chat_ref(ctx: &Ctx, user: &Client, chat: i64) -> Option<PeerRef> {
    if let Some(peer) = ctx.user_chat(chat) {
        return Some(peer);
    }
    let mut found = Vec::new();
    let mut dialogs = user.iter_dialogs();
    while let Ok(Some(dialog)) = dialogs.next().await {
        let peer = dialog.peer();
        if let Some(id) = peer.id().bot_api_dialog_id()
            && let Ok(Some(peer_ref)) = peer.to_ref().await
        {
            found.push((id, peer_ref));
        }
    }
    let peer = found
        .iter()
        .find(|(id, _)| *id == chat)
        .map(|(_, peer)| *peer);
    ctx.set_user_chats(found);
    peer
}

async fn promote(
    ctx: &Ctx,
    chat_ref: PeerRef,
    target: PeerRef,
) -> std::result::Result<(), grammers_client::InvocationError> {
    let mut last = None;
    for level in 0..3 {
        let mut rights = ctx
            .client
            .set_admin_rights(chat_ref, target)
            .delete_messages(true)
            .ban_users(true)
            .invite_users(true)
            .pin_messages(true);
        if level < 2 {
            rights = rights.change_info(true).manage_call(true);
        }
        if level < 1 {
            rights = rights.add_admins(true);
        }
        match rights.await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let narrower_might_work = e.to_string().contains("RIGHT_FORBIDDEN");
                last = Some(e);
                if !narrower_might_work {
                    break;
                }
            }
        }
    }
    Err(last.unwrap_or(grammers_client::InvocationError::Dropped))
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    if !START.contains(&text) {
        return false;
    }
    let sender = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id);
    if sender.is_none() || sender != sudo() {
        return true;
    }
    let Some(user) = ctx.user_client() else {
        let _ = message
            .reply("نشست کلینر ساخته نشد. مقدار TG_ID و TG_HASH را بررسی کنید.")
            .await;
        return true;
    };
    if user.is_authorized().await.unwrap_or(false) {
        let name = user
            .get_me()
            .await
            .map(|me| me.full_name())
            .unwrap_or_default();
        let _ = message
            .reply(format!("کلینر از قبل وارد شده است · {name}"))
            .await;
        return true;
    }

    let api_hash = match std::env::var("TG_HASH") {
        Ok(hash) => hash,
        Err(_) => return true,
    };
    login(ctx, &user, message, &api_hash).await;
    true
}

async fn login(ctx: &Ctx, user: &Client, message: &Message, api_hash: &str) {
    use base64::Engine;

    let started = std::time::Instant::now();
    let mut shown = Vec::new();
    while started.elapsed() < DEADLINE {
        match user.qr_login_step(api_hash).await {
            Ok(grammers_client::QrLogin::Waiting { token, expires: _ }) => {
                if token != shown {
                    let url = format!(
                        "tg://login?token={}",
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&token)
                    );
                    send_qr(ctx, message, &url).await;
                    shown = token;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(grammers_client::QrLogin::PasswordRequired(token)) => {
                let hint = match token.hint() {
                    Some(hint) => format!("\nراهنما · {}", super::esc(hint)),
                    None => String::new(),
                };
                ctx.expect_password();
                let _ = message
                    .reply(InputMessage::new().html(format!(
                        "<b>رمز دو مرحله ای</b>\n\n\
                         کد اسکن شد. رمز دو مرحله ای حساب را همینجا بفرستید.{hint}\n\n\
                         <i>پیام رمز بلافاصله پاک می شود.</i>"
                    )))
                    .await;
                let Some(password) = ctx.await_password(DEADLINE).await else {
                    let _ = message.reply("رمزی نیامد. دوباره «ورود کلینر» را بفرستید.").await;
                    return;
                };
                let _ = match user.check_password(token, password.trim()).await {
                    Ok(me) => {
                        ctx.set_cleaner_id(me.id().bare_id_unchecked());
                        message
                            .reply(format!("✓ کلینر وارد شد · {}", super::esc(&me.full_name())))
                            .await
                    }
                    Err(e) => message.reply(format!("انجام نشد · {e}")).await,
                };
                return;
            }
            Ok(grammers_client::QrLogin::Success(me)) => {
                ctx.set_cleaner_id(me.id().bare_id_unchecked());
                let _ = message
                    .reply(format!("✓ کلینر وارد شد · {}", super::esc(&me.full_name())))
                    .await;
                return;
            }
            Err(e) => {
                let _ = message.reply(format!("انجام نشد · {e}")).await;
                return;
            }
        }
    }
    let _ = message
        .reply("زمان ورود تمام شد. «ورود کلینر» را دوباره بفرستید.")
        .await;
}

pub async fn take_password(ctx: &Ctx, message: &Message) -> bool {
    let sender = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id);
    if sender.is_none() || sender != sudo() {
        return false;
    }
    let password = message.text().trim().to_owned();
    if password.is_empty() || !ctx.give_password(password) {
        return false;
    }
    let _ = message.delete().await;
    true
}

async fn send_qr(ctx: &Ctx, message: &Message, url: &str) {
    let mut card = InputMessage::new().html(
        "<b>ورود کلینر</b>\n\n\
         در تلگرام · Settings › Devices › Link Desktop Device\n\
         و این کد را اسکن کنید.\n\n\
         <i>کد هر چند ثانیه تازه می شود.</i>",
    );
    if let Some(png) = qr_png(url) {
        let size = png.len();
        if let Ok(uploaded) = ctx
            .client
            .upload_stream(
                &mut std::io::Cursor::new(png),
                size,
                "cleaner-qr.png".to_owned(),
            )
            .await
        {
            card = card.photo(uploaded);
        }
    }
    let _ = message.reply(card).await;
}

fn qr_png(url: &str) -> Option<Vec<u8>> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let image = code
        .render::<image::Luma<u8>>()
        .min_dimensions(512, 512)
        .build();
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}
