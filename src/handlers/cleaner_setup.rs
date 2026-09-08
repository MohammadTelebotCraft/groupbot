
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use grammers_client::message::{InputMessage, ReplyMarkup};
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;
use grammers_client::update::CallbackQuery;

use super::{Ctx, cleaner, install, premium};

const EVERY_MINUTES: u64 = 360;
const CHECKED_SLOT: &str = "cln_checked_slot";
const ACCESS_ALERT: &str = "برای افزودن کلینر، به ربات دسترسی کامل بدهید؛ به ویژه «افزودن کاربران» و «افزودن ادمین جدید». سپس همین دکمه را دوباره بزنید.";
const UNKNOWN_ACCESS: &str = "دسترسی های ربات خوانده نشد. چند لحظه بعد دوباره دکمه را بزنید.";

pub fn available(ctx: &Ctx) -> bool {
    ctx.cleaner_id().is_some() && ctx.user_client().is_some()
}

pub fn with_button(message: InputMessage, chat: i64, retry: bool) -> InputMessage {
    message.reply_markup(markup(chat, retry))
}

fn markup(chat: i64, retry: bool) -> ReplyMarkup {
    ReplyMarkup::from_buttons(&[vec![super::style::choice(
        if retry {
            "بررسی دسترسی و افزودن کلینر"
        } else {
            "افزودن کلینر"
        },
        format!("cln:{chat}"),
        false,
    )]])
}

fn recommendation(chat: i64, presence: Presence) -> Option<InputMessage> {
    let status = match presence {
        Presence::Member => {
            "کلینر در گروه است ولی ادمین نیست. برای فعال شدن پاکسازی، دسترسی آن را کامل کنید."
        }
        Presence::Missing => {
            "کلینر در این گروه نیست. پیشنهاد می کنیم همین حالا آن را اضافه کنید تا پاکسازی کامل پیام ها و کنترل پیام ربات های دیگر در دسترس باشد."
        }
        Presence::Admin | Presence::Unknown => return None,
    };
    Some(with_button(
        premium::html(format!(
            "<b>پیشنهاد فعال سازی کلینر</b>\n\n{status}\n\n\
             <i>یکی از ادمین ها دکمه زیر را بزند. ربات باید همه دسترسی های لازم، به ویژه «افزودن ادمین جدید» را داشته باشد.</i>"
        )),
        chat,
        presence == Presence::Member,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Presence {
    Admin,
    Member,
    Missing,
    Unknown,
}

async fn presence(ctx: &Ctx, peer: PeerRef) -> Presence {
    let Some(target) = ctx
        .cleaner_id()
        .and_then(PeerId::user)
        .map(PeerId::to_ambient_ref)
    else {
        return Presence::Unknown;
    };
    match ctx.client.get_permissions(peer, target).await {
        Ok(state) if state.has_left() || state.is_banned() => Presence::Missing,
        Ok(state) if state.is_admin() => Presence::Admin,
        Ok(_) => Presence::Member,
        Err(grammers_client::InvocationError::Rpc(rpc)) if rpc.name == "USER_NOT_PARTICIPANT" => {
            Presence::Missing
        }
        Err(error) => {
            log::warn!(
                "cleaner recommendation: cannot read membership in {}: {error}",
                peer.id
            );
            Presence::Unknown
        }
    }
}

fn due_minutes(now: u64) -> [i64; 3] {
    std::array::from_fn(|back| ((now + EVERY_MINUTES - back as u64) % EVERY_MINUTES) as i64)
}

fn scheduled_slot(now: u64, chat: i64) -> u64 {
    let offset = chat.unsigned_abs() % EVERY_MINUTES;
    now.saturating_sub((now + EVERY_MINUTES - offset) % EVERY_MINUTES)
}

pub async fn run_recommendations(ctx: &Arc<Ctx>) {
    if !available(ctx) {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 60;
    let chats = match ctx
        .settings
        .cleaner_recommendations_due(&due_minutes(now))
        .await
    {
        Ok(chats) => chats,
        Err(error) => {
            log::warn!(
                "cleaner recommendation: due-chat query failed; retrying next tick: {error}"
            );
            return;
        }
    };
    let owner = Arc::clone(ctx);
    super::bounded(chats, super::FLEET_CONCURRENCY, move |chat| {
        let ctx = Arc::clone(&owner);
        async move {
            if !ctx.owns_chat(chat) {
                return;
            }
            let slot = scheduled_slot(now, chat);
            if ctx
                .settings
                .value_parsed::<u64>(chat, CHECKED_SLOT)
                .is_some_and(|last| last >= slot)
            {
                return;
            }
            let Some(peer) = ctx.resolve_group(chat).await else {
                return;
            };
            let state = presence(&ctx, peer).await;
            if state == Presence::Unknown {
                return;
            }
            if let Some(body) = recommendation(chat, state)
                && let Err(error) = ctx.client.send_message(peer, body).await
            {
                log::warn!("cleaner recommendation: cannot send to {chat}: {error}");
                return;
            }
            if let Err(error) = ctx
                .settings
                .try_set_value(chat, CHECKED_SLOT, &slot.to_string())
                .await
            {
                log::warn!("cleaner recommendation: could not store slot for {chat}: {error}");
            }
        }
    })
    .await;
}

fn access_problem(
    standing: &install::Standing,
    chat: i64,
    installed: bool,
) -> Option<(&'static str, InputMessage)> {
    if install::ready(standing) {
        return None;
    }
    if matches!(
        standing,
        install::Standing::Unknown | install::Standing::Gone
    ) {
        return Some((
            UNKNOWN_ACCESS,
            with_button(InputMessage::new().text(UNKNOWN_ACCESS), chat, true),
        ));
    }
    Some((
        ACCESS_ALERT,
        with_button(
            premium::html(format!(
                "{}\n\n<b>افزودن کلینر</b>\n\
         به ربات دسترسی کامل بدهید. مسیر · اطلاعات گروه ← ویرایش ← مدیران ← این ربات\n\n\
         <i>دسترسی ها را ذخیره کنید و دکمه زیر را دوباره بزنید.</i>",
                install::card(standing, installed),
            )),
            chat,
            true,
        ),
    ))
}

pub async fn check_access(
    ctx: &Ctx,
    peer: PeerRef,
    chat: i64,
) -> Option<(&'static str, InputMessage)> {
    access_problem(
        &install::standing(ctx, peer).await,
        chat,
        super::owner(ctx, chat).is_some(),
    )
}

pub async fn add(ctx: &Ctx, peer: PeerRef, chat: i64) -> InputMessage {
    super::autoconfig::configure(ctx, peer).await;
    match presence(ctx, peer).await {
        Presence::Admin => {
            return match ctx
                .settings
                .try_set(chat, install::CLEANER_ADDED, true)
                .await
            {
                Ok(_) => {
                    InputMessage::new().text("✓ کلینر در گروه است و ادمین است. راه اندازی کامل شد.")
                }
                Err(error) => {
                    log::warn!(
                        "cleaner setup: could not record installed cleaner for {chat}: {error}"
                    );
                    persistence_failure(&error)
                }
            };
        }
        Presence::Unknown => {
            return with_button(
                InputMessage::new().text("وضعیت کلینر خوانده نشد. چند لحظه بعد دوباره بررسی کنید."),
                chat,
                true,
            );
        }
        _ => {}
    }
    let Some(_permit) = ctx.try_cleaner_slot() else {
        return with_button(
            InputMessage::new()
                .text("کلینر مشغول راه اندازی گروه دیگری است. کمی بعد دوباره بزنید."),
            chat,
            true,
        );
    };
    if !ctx.claim_cleaner_install(chat) {
        return with_button(InputMessage::new().text("افزودن کلینر در حال انجام است یا همین حالا امتحان شد. چند دقیقه بعد دوباره بررسی کنید."), chat, true);
    }
    let result = cleaner::install(ctx, peer).await;
    ctx.forget_user_chats();
    if result.is_ok()
        && let Err(error) = ctx
            .settings
            .try_set(chat, install::CLEANER_ADDED, true)
            .await
    {
        log::warn!("cleaner setup: could not record installation for {chat}: {error}");
        return persistence_failure(&error);
    }
    match result {
        Ok(cleaner::Installed::AlreadyAdmin) => InputMessage::new()
            .text("✓ کلینر در گروه است و از قبل ادمین بود. «حذف همه» و «حذف پیام» در دسترس هستند."),
        Ok(cleaner::Installed::Promoted) => InputMessage::new()
            .text("✓ کلینر اضافه و ادمین شد. حالا «حذف همه» و «حذف پیام» هم کار می کنند."),
        Ok(cleaner::Installed::JoinedNotAdmin(reason)) => with_button(
            premium::html(format!(
                "<b>تکمیل دسترسی کلینر</b>\n\nکلینر وارد گروه شد ولی ادمین نشد · {}\n\n\
             <i>به ربات دسترسی کامل، به ویژه «افزودن ادمین جدید» بدهید یا کلینر را در تنظیمات گروه ادمین کنید. سپس دوباره بررسی کنید.</i>",
                super::esc(&reason),
            )),
            chat,
            true,
        ),
        Err(reason) => with_button(
            premium::html(format!(
                "<b>افزودن کلینر</b>\n\nانجام نشد · {}\n\n<i>پس از رفع مشکل دوباره دکمه را بزنید.</i>",
                super::esc(&reason),
            )),
            chat,
            true,
        ),
    }
}

fn persistence_failure(error: &crate::state::SettingsWriteError) -> InputMessage {
    InputMessage::new().text(if error.commit_outcome_unknown() {
        "کلینر در گروه است، اما نتیجه ثبت راه اندازی نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
    } else {
        "کلینر در گروه است، اما راه اندازی در پایگاه داده ثبت نشد؛ کمی بعد دوباره بررسی کنید."
    })
}

fn callback_chat(payload: &str, here: i64) -> Option<i64> {
    payload
        .parse::<i64>()
        .ok()
        .filter(|chat| *chat < 0 && *chat == here)
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, here: i64) {
    let Some(chat) = callback_chat(payload, here).filter(|chat| ctx.owns_chat(*chat)) else {
        let _ = query
            .answer()
            .alert("این دکمه را در گروه اصلی بزنید.")
            .send()
            .await;
        return;
    };
    let tl::enums::Update::BotCallbackQuery(update) = query.raw() else {
        return;
    };
    if !available(ctx) {
        let _ = query
            .answer()
            .alert("کلینر در حال حاضر در دسترس نیست. مالک ربات باید کلینر را وارد کند.")
            .send()
            .await;
        return;
    }
    let Some(peer) = ctx.resolve_group(chat).await else {
        let _ = query
            .answer()
            .alert("این دکمه فقط در گروه قابل استفاده است. اطلاعات گروه باید در دسترس باشد.")
            .send()
            .await;
        return;
    };
    let body = if let Some((alert, body)) = check_access(ctx, peer, chat).await {
        let _ = query.answer().alert(alert).send().await;
        body
    } else {
        let _ = query
            .answer()
            .text("در حال بررسی و افزودن کلینر...")
            .send()
            .await;
        add(ctx, peer, chat).await
    };
    if let Err(error) = ctx
        .client
        .edit_message(peer, update.msg_id, body.clone())
        .await
    {
        if matches!(&error, grammers_client::InvocationError::Rpc(rpc) if rpc.name == "MESSAGE_NOT_MODIFIED")
        {
            return;
        }
        log::warn!("cleaner setup: cannot update button message in {chat}: {error}");
        if let Err(error) = ctx.client.send_message(peer, body).await {
            log::warn!("cleaner setup: cannot send result to {chat}: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_cannot_target_another_group_or_private_chat() {
        assert_eq!(callback_chat("-100123", -100123), Some(-100123));
        for (payload, here) in [
            ("-100123", -100124),
            ("-100123", 123),
            ("123", 123),
            ("-1:extra", -1),
            ("bad", -1),
        ] {
            assert_eq!(callback_chat(payload, here), None);
        }
    }

    #[test]
    fn schedule_wraps_and_keeps_one_slot_per_six_hours() {
        assert_eq!(due_minutes(720), [0, 359, 358]);
        let chat = -719;
        assert_eq!(scheduled_slot(719, chat), 719);
        assert_eq!(scheduled_slot(720, chat), 719);
        assert_eq!(scheduled_slot(721, chat), 719);
        assert_eq!(scheduled_slot(1079, chat), 1079);
    }

    #[test]
    fn incomplete_and_unknown_access_never_allow_installation() {
        for state in [
            install::Standing::Member,
            install::Standing::Basic { admin: true },
            install::Standing::Unknown,
            install::Standing::Gone,
        ] {
            assert!(access_problem(&state, -1, true).is_some());
        }
        assert!(access_problem(&install::Standing::Creator, -1, true).is_none());
        assert!(ACCESS_ALERT.chars().count() <= 200);
    }

    #[test]
    fn recommendations_only_offer_missing_or_unpromoted_cleaners() {
        assert!(recommendation(-1, Presence::Missing).is_some());
        assert!(recommendation(-1, Presence::Member).is_some());
        assert!(recommendation(-1, Presence::Admin).is_none());
        assert!(recommendation(-1, Presence::Unknown).is_none());
    }

    #[test]
    fn add_and_retry_buttons_are_callbacks_bound_to_the_group() {
        for retry in [false, true] {
            let tl::enums::ReplyMarkup::ReplyInlineMarkup(markup) = markup(i64::MIN, retry).raw
            else {
                panic!("inline keyboard")
            };
            let tl::enums::KeyboardButtonRow::Row(row) = &markup.rows[0];
            let tl::enums::KeyboardButton::Callback(button) = &row.buttons[0] else {
                panic!("callback button")
            };
            let payload = std::str::from_utf8(&button.data).unwrap();
            assert!(button.data.len() <= 64);
            assert_eq!(
                callback_chat(payload.strip_prefix("cln:").unwrap(), i64::MIN),
                Some(i64::MIN)
            );
        }
    }
}
