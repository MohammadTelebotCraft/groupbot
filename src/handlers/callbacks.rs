use grammers_client::update::CallbackQuery;

use super::{Ctx, chat_admins, is_bot_admin, limits, owner};

pub async fn handle(ctx: &Ctx, query: &CallbackQuery) {
    let Ok(data) = std::str::from_utf8(query.data()) else {
        return;
    };
    let Some(here) = query.peer_id().bot_api_dialog_id() else {
        return;
    };

    let _admitted = if here < 0 {
        let Ok(Some(peer)) = query.peer_ref().await else {
            let _ = query
                .answer()
                .alert("این دکمه فقط در گروه قابل استفاده است.")
                .send()
                .await;
            return;
        };
        let state = match ctx.admit_chat(here, peer).await {
            Ok(state) => state,
            Err(error) => {
                ctx.admission_failed(here, error);
                return;
            }
        };
        Some(state)
    } else {
        None
    };

    let chat = callback_target_chat(data, here);

    let target_admitted = if here >= 0 && chat < 0 {
        let Some(peer) = ctx.resolve_group(chat).await else {
            return;
        };
        match ctx.admit_chat(chat, peer).await {
            Ok(state) => Some(state),
            Err(error) => {
                ctx.admission_failed(chat, error);
                return;
            }
        }
    } else {
        None
    };

    let _slot = if let Some(state) = target_admitted.as_ref().or(_admitted.as_ref()) {
        Some(state.slot().await)
    } else {
        None
    };

    if !callback_chat_matches(here, chat) {
        let _ = query
            .answer()
            .alert("این پنل متعلق به گروه دیگری است.")
            .send()
            .await;
        return;
    }

    if let Some(payload) = data.strip_prefix("c:") {
        super::captcha::on_callback(ctx, query, payload, chat).await;
        return;
    }

    if let Some(payload) = data.strip_prefix("j:") {
        super::join::on_callback(ctx, query, payload).await;
        return;
    }

    if let Some(payload) = data.strip_prefix("f:") {
        let is_admin = presser_can_manage(ctx, query, chat).await;
        super::filters::on_callback(ctx, query, payload, is_admin).await;
        return;
    }

    if let Some(payload) = data.strip_prefix("v:") {
        super::voicemonitor::on_callback(ctx, query, payload, chat).await;
        return;
    }

    if let Some(payload) = data.strip_prefix("fx:") {
        super::currency::on_callback(ctx, query, payload).await;
        return;
    }

    if let Some(payload) = data.strip_prefix("sd:") {
        super::sudo::on_callback(ctx, query, payload).await;
        return;
    }

    if let Some(action) = data.strip_prefix("h:") {
        super::panel::on_help(ctx, query, action, false).await;
        return;
    }
    if let Some(action) = data.strip_prefix("k:") {
        super::panel::on_help(ctx, query, action, true).await;
        return;
    }

    if !presser_can_manage(ctx, query, chat).await {
        let _ = query
            .answer()
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Locked),
                "فقط ادمین ها می توانند این دکمه را بزنند.",
            ))
            .send()
            .await;
        return;
    }

    if let Some(cap) = match data.split(':').next() {
        Some("jx") => Some(limits::EXEMPT),
        Some("pg" | "cln") => Some(limits::CLEAN),
        Some("r") => Some(limits::CASE),
        Some("mc") => Some(limits::CASE),
        Some("s" | "p" | "q" | "t") => Some(limits::SET),
        _ => None,
    } && let Some(presser) = query.sender_id().bare_id()
        && !limits::permits(ctx, chat, presser, cap)
    {
        limits::refuse(query, cap).await;
        return;
    }

    if let Some(payload) = data.strip_prefix("cln:") {
        super::cleaner_setup::on_callback(ctx, query, payload, chat).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("jx:") {
        super::join::on_exempt(ctx, query, payload).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("pg:") {
        super::purge::on_callback(ctx, query, payload).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("s:") {
        super::stats::on_callback(ctx, query, payload).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("r:") {
        super::report::on_callback(ctx, query, payload, chat).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("mc:") {
        super::cases::on_callback(ctx, query, payload, chat).await;
        return;
    }
    if let Some(payload) = data.strip_prefix("a:") {
        super::promote::on_callback(ctx, query, payload, chat).await;
        return;
    }
    if let Some(action) = data.strip_prefix("p:") {
        super::panel::on_callback(ctx, query, action, false).await;
        return;
    }
    if let Some(action) = data.strip_prefix("q:") {
        super::panel::on_callback(ctx, query, action, true).await;
        return;
    }
    if let Some(action) = data.strip_prefix("t:") {
        super::toggles::on_callback(ctx, query, action, chat).await;
    }
}

fn callback_target_chat(data: &str, here: i64) -> i64 {
    data.strip_prefix("p:")
        .or_else(|| data.strip_prefix("q:"))
        .or_else(|| data.strip_prefix("h:"))
        .or_else(|| data.strip_prefix("k:"))
        .and_then(|rest| rest.split(':').nth(1))
        .and_then(|chat| chat.parse::<i64>().ok())
        .unwrap_or(here)
}

pub(super) fn dispatch_chat(data: &[u8], here: i64) -> i64 {
    let Ok(data) = std::str::from_utf8(data) else {
        return here;
    };
    let target = callback_target_chat(data, here);
    if here >= 0 && target < 0 {
        target
    } else {
        here
    }
}

async fn presser_can_manage(ctx: &Ctx, query: &CallbackQuery, chat: i64) -> bool {
    let Some(presser) = query.sender_id().bare_id() else {
        return false;
    };
    if owner(ctx, chat) == Some(presser) || is_bot_admin(ctx, chat, presser) {
        return true;
    }

    let chat_ref = match ctx.chat_ref(chat) {
        Some(peer) => peer,
        None => match query.peer_ref().await {
            Ok(Some(peer)) => peer,
            _ => return false,
        },
    };
    if let Some(admins) = chat_admins(ctx, chat_ref, chat).await {
        return admins.contains(&presser);
    }

    let Ok(Some(sender)) = query.sender_ref().await else {
        return false;
    };
    ctx.client
        .get_permissions(chat_ref, sender)
        .await
        .is_ok_and(|p| p.is_admin())
}

fn callback_chat_matches(here: i64, target: i64) -> bool {
    !(here < 0 && target < 0 && here != target)
}

#[cfg(test)]
mod tests {
    use super::{callback_chat_matches, callback_target_chat, dispatch_chat};

    #[test]
    fn public_and_ephemeral_panels_keep_their_encoded_group() {
        assert_eq!(callback_target_chat("p:7:-1001:root", -9), -1001);
        assert_eq!(callback_target_chat("q:7:-1001:root", -9), -1001);
        assert_eq!(callback_target_chat("h:7:-1001:root", -9), -1001);
        assert_eq!(callback_target_chat("k:7:-1001:root", -9), -1001);
        assert_eq!(callback_target_chat("s:unrelated", -9), -9);
    }

    #[test]
    fn group_callbacks_cannot_cross_group_boundaries() {
        assert!(callback_chat_matches(-1001, -1001));
        assert!(!callback_chat_matches(-1001, -1002));
    }

    #[test]
    fn private_panels_can_target_their_group() {
        assert!(callback_chat_matches(42, -1001));
    }

    #[test]
    fn private_panel_callbacks_are_admitted_on_the_target_group_lane() {
        assert_eq!(dispatch_chat(b"p:7:-1001:root", 42), -1001);
        assert_eq!(dispatch_chat(b"q:7:-1001:root", 42), -1001);
        assert_eq!(dispatch_chat(b"s:ordinary", 42), 42);
        assert_eq!(dispatch_chat(&[0xff], 42), 42);
    }

    #[test]
    fn forwarded_group_panels_cannot_charge_another_groups_lane() {
        assert_eq!(dispatch_chat(b"p:7:-1002:root", -1001), -1001);
    }
}
