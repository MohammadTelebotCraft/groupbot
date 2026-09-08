use std::sync::Arc;

use grammers_client::message::Message;
use grammers_client::session::types::{ChannelKind, PeerId, PeerInfo, PeerKind, PeerRef};
use grammers_client::tl;
use grammers_client::update::Raw;
use grammers_session::storages::ErasedSession;

use super::{Ctx, bots, config, install, locks, welcome};
use crate::state::{SettingMutation, SettingsWriteError};

const DEFAULTS: &[(&str, &str)] = &[
    ("links", "قفل لینک"),
    ("file", "قفل فایل"),
    (locks::SERVICE, "قفل سرویس تلگرام"),
    (bots::LOCK, "قفل ورود ربات"),
    (bots::KICK_ADDER, "اخراج اضافه کننده ربات"),
];

const DEFAULT_WELCOME: &str = "{منشن} به {گروه} خوش آمدی.";

pub(super) async fn resolve_peer(
    session: Option<&ErasedSession>,
    id: PeerId,
    stored: Option<PeerRef>,
) -> Option<PeerRef> {
    if id.kind() == PeerKind::User {
        return None;
    }
    let info = match session {
        Some(session) => session.peer(id).await.ok().flatten(),
        None => None,
    };
    if id.kind() == PeerKind::Channel
        && !matches!(
            info,
            Some(PeerInfo::Channel {
                kind: Some(ChannelKind::Megagroup | ChannelKind::Gigagroup),
                ..
            })
        )
    {
        return None;
    }
    let valid =
        |peer: &PeerRef| peer.id == id && (id.kind() == PeerKind::Chat || peer.auth.hash() != 0);
    info.and_then(|info| info.auth().map(|auth| PeerRef { id, auth }))
        .filter(valid)
        .or_else(|| stored.filter(valid))
        .or_else(|| (id.kind() == PeerKind::Chat).then(|| id.to_ambient_ref()))
}

fn setup_service(action: Option<&tl::enums::MessageAction>, me: i64) -> bool {
    if me == 0 {
        return false;
    }
    match action {
        Some(tl::enums::MessageAction::ChatAddUser(action)) => action.users.contains(&me),
        Some(tl::enums::MessageAction::ChatCreate(action)) => action.users.contains(&me),
        Some(tl::enums::MessageAction::ChannelMigrateFrom(_)) => true,
        _ => false,
    }
}

fn needs_setup(ctx: &Ctx, chat: i64) -> bool {
    super::owner(ctx, chat).is_none()
        || (super::cleaner_setup::available(ctx)
            && !ctx.settings.is_locked(chat, install::CLEANER_ADDED))
}

pub async fn on_message(ctx: &Arc<Ctx>, message: &Message, state: &super::ChatState) {
    if !super::is_group_message(ctx, message).await {
        return;
    }
    let explicit = setup_service(message.action(), ctx.me_id());
    if !ctx.claim_setup_probe(state, explicit) || (!explicit && !needs_setup(ctx, state.chat)) {
        return;
    }
    queue_recovery(ctx, state.chat, explicit).await;
}

async fn queue_recovery(ctx: &Arc<Ctx>, chat: i64, announce: bool) {
    let permit = ctx.sweep_slot().await;
    let ctx = Arc::clone(ctx);
    Arc::clone(&ctx).spawn_owned(async move {
        let _permit = permit;
        retry(&ctx, chat, announce).await;
    });
}

async fn retry(ctx: &Arc<Ctx>, chat: i64, announce: bool) {
    for (attempt, delay) in [0, 2, 8].into_iter().enumerate() {
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }
        let Some(peer) = ctx.resolve_group(chat).await else {
            continue;
        };
        let _state = match ctx.admit_chat(chat, peer).await {
            Ok(state) => state,
            Err(error) => {
                ctx.admission_failed(chat, error);
                return;
            }
        };
        if reconcile(ctx, peer, chat, None, announce && attempt == 0).await {
            return;
        }
    }
    log::warn!("auto-config: setup deferred for {chat}; retry on the next group activity");
    if announce && let Some(peer) = ctx.resolve_group(chat).await {
        let mut body = super::premium::html(
            "<b>راه اندازی گروه</b>\n\nاطلاعات لازم برای راه اندازی کامل خوانده نشد.\n\n\
             <i>ربات باید همه دسترسی های ادمین را داشته باشد. کمی بعد «وضعیت نصب» را بفرستید.</i>",
        );
        if super::cleaner_setup::available(ctx) {
            body = super::cleaner_setup::with_button(body, chat, true);
        }
        if let Err(error) = ctx.client.send_message(peer, body).await {
            log::warn!("auto-config: could not send setup retry to {chat}: {error}");
        }
    }
}

pub async fn recover_startup(ctx: Arc<Ctx>) {
    let chats = match ctx
        .settings
        .incomplete_setups(super::cleaner_setup::available(&ctx))
        .await
    {
        Ok(chats) => chats,
        Err(error) => {
            log::error!(
                "auto-config: startup recovery query failed; leaving work for retry: {error}"
            );
            return;
        }
    };
    super::bounded(chats, super::FLEET_CAMPAIGNS, move |chat| {
        let ctx = Arc::clone(&ctx);
        async move {
            if ctx.owns_chat(chat) {
                if ctx.resolve_group(chat).await.is_none() {
                    log::info!("auto-config: skipped non-group or unresolved peer {chat}");
                    return;
                }
                retry(&ctx, chat, false).await;
            }
        }
    })
    .await;
}

fn setup_update(
    raw: &tl::enums::Update,
    me: i64,
) -> Option<(PeerId, Option<install::Standing>, bool)> {
    if me == 0 {
        return None;
    }
    let (peer, standing, explicit) = match raw {
        tl::enums::Update::ChannelParticipant(update) if update.user_id == me => (
            PeerId::channel(update.channel_id),
            Some(install::from_participant(update.new_participant.as_ref())),
            true,
        ),
        tl::enums::Update::ChatParticipantAdmin(update) if update.user_id == me => (
            PeerId::chat(update.chat_id),
            Some(install::Standing::Basic {
                admin: update.is_admin,
            }),
            true,
        ),
        tl::enums::Update::ChatParticipantAdd(update) if update.user_id == me => {
            (PeerId::chat(update.chat_id), None, true)
        }
        tl::enums::Update::Channel(update) => (PeerId::channel(update.channel_id), None, false),
        tl::enums::Update::Chat(update) => (PeerId::chat(update.chat_id), None, false),
        _ => return None,
    };
    Some((peer?, standing, explicit))
}

pub async fn on_raw(ctx: &Arc<Ctx>, raw: &Raw) {
    let Some((id, standing, explicit)) = setup_update(&raw.raw, ctx.me_id()) else {
        return;
    };
    let Some(chat) = id.bot_api_dialog_id() else {
        return;
    };
    if !explicit && !needs_setup(ctx, chat) {
        return;
    }
    let Some(peer) = ctx.resolve_group(chat).await else {
        queue_recovery(ctx, chat, explicit).await;
        return;
    };
    let _state = match ctx.admit_chat(chat, peer).await {
        Ok(state) => state,
        Err(error) => {
            ctx.admission_failed(chat, error);
            return;
        }
    };
    if !reconcile(ctx, peer, chat, standing, explicit).await {
        queue_recovery(ctx, chat, false).await;
    }
}

async fn reconcile(
    ctx: &Arc<Ctx>,
    chat_ref: PeerRef,
    chat: i64,
    standing: Option<install::Standing>,
    announce: bool,
) -> bool {
    let standing = match standing {
        Some(standing) => standing,
        None => install::standing(ctx, chat_ref).await,
    };
    if matches!(standing, install::Standing::Unknown) {
        return false;
    }
    if matches!(standing, install::Standing::Gone) {
        return true;
    }
    if !install::ready(&standing) {
        if announce {
            install::announce(ctx, chat_ref, chat, &standing).await;
        }
        return true;
    }
    let was_installed = super::owner(ctx, chat).is_some();
    let configured = configure(ctx, chat_ref).await;
    let installed = super::owner(ctx, chat).is_some();
    if announce && was_installed && installed && !configured {
        install::announce_complete(ctx, chat_ref, chat).await;
    }
    install::ensure_cleaner(ctx, chat_ref, chat).await;
    installed
}

pub async fn configure(ctx: &Ctx, chat: PeerRef) -> bool {
    let Some(chat_id) = chat.id.bot_api_dialog_id() else {
        return false;
    };
    if super::owner(ctx, chat_id).is_some() {
        return false;
    }
    let Some(_configuring) = ctx.try_autoconfig(chat_id) else {
        return false;
    };
    if super::owner(ctx, chat_id).is_some() {
        return false;
    }
    let (creator, admin_names) = super::admins(ctx, chat).await;
    let Some((creator_id, creator_name)) = creator else {
        eprintln!("auto-config: no creator found for {chat_id}");
        return false;
    };

    let locked = match apply_defaults(ctx, chat_id, creator_id).await {
        Ok(locked) => locked,
        Err(error) => {
            log::warn!("auto-config: could not persist setup for {chat_id}: {error}");
            return false;
        }
    };
    if let Err(error) = ctx
        .client
        .send_message(
            chat,
            super::premium::html(summary(
                "ربات فعال شد",
                &creator_name,
                creator_id,
                &admin_names,
                &locked,
            )),
        )
        .await
    {
        log::warn!("auto-config: could not announce activation in {chat_id}: {error}");
    }
    log::info!("auto-config: activated {chat_id}");
    true
}

pub async fn apply_defaults(
    ctx: &Ctx,
    chat_id: i64,
    owner_id: i64,
) -> Result<String, SettingsWriteError> {
    let mut lines: Vec<String> = Vec::new();
    let owner = owner_id.to_string();
    let install_ttl = welcome::INSTALL_TTL.to_string();
    let mut mutations = Vec::with_capacity(DEFAULTS.len() + 3);
    for (key, label) in DEFAULTS {
        mutations.push(SettingMutation::Put { key, value: "" });
        lines.push(format!("✓ {label}"));
    }
    mutations.push(SettingMutation::Put {
        key: config::OWNER,
        value: &owner,
    });

    let welcome_was_empty = ctx
        .settings
        .value(chat_id, welcome::TEXT)
        .unwrap_or_default()
        .is_empty();
    mutations.push(SettingMutation::PutIfEmpty {
        key: welcome::TEXT,
        value: DEFAULT_WELCOME,
        condition_key: welcome::TEXT,
    });
    mutations.push(SettingMutation::PutIfEmpty {
        key: welcome::TTL,
        value: &install_ttl,
        condition_key: welcome::TEXT,
    });
    if welcome_was_empty {
        lines.push(format!(
            "✓ خوشامدگویی · حذف خودکار بعد از {} ثانیه",
            welcome::INSTALL_TTL
        ));
    }
    ctx.settings.try_apply_batch(chat_id, &mutations).await?;
    Ok(lines.join("\n"))
}

pub fn summary(
    title: &str,
    owner_name: &str,
    owner_id: i64,
    admin_names: &[String],
    locked: &str,
) -> String {
    format!(
        "<b>{title}</b>\n\n\
         مالک ربات · <b>{owner_name}</b>\n\
         شناسه · <code>{owner_id}</code>\n\n\
         <b>ادمین ها</b> ({})\n\
         {}\n\
         {}\n\
         <i>راهنما برای دستورها، پنل برای تنظیمات</i>",
        admin_names.len(),
        admin_names.join("\n"),
        if locked.is_empty() {
            String::new()
        } else {
            format!("\n<b>پیش فرض ها</b>\n{locked}\n")
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_client::session::types::PeerAuth;
    use grammers_session::storages::{MemorySession, erase};

    #[tokio::test]
    async fn first_promotion_uses_the_session_hash_before_any_group_message() {
        let session = erase(Arc::new(MemorySession::default()));
        let id = PeerId::channel(4451811811).unwrap();
        session
            .cache_peer(&PeerInfo::Channel {
                id: 4451811811,
                auth: Some(PeerAuth::from_hash(987654321)),
                kind: Some(ChannelKind::Megagroup),
            })
            .await
            .unwrap();
        let peer = resolve_peer(Some(session.as_ref()), id, None)
            .await
            .expect("hash from raw update envelope");
        let tl::enums::InputChannel::Channel(channel) = tl::enums::InputChannel::from(peer) else {
            panic!("channel")
        };
        assert_eq!(channel.channel_id, 4451811811);
        assert_eq!(channel.access_hash, 987654321);
        let stale = PeerRef {
            id,
            auth: PeerAuth::from_hash(123),
        };
        assert_eq!(
            resolve_peer(Some(session.as_ref()), id, Some(stale)).await,
            Some(peer)
        );
    }

    #[tokio::test]
    async fn unresolved_supergroups_never_fabricate_a_zero_hash() {
        let id = PeerId::channel(123).unwrap();
        assert!(resolve_peer(None, id, None).await.is_none());
        assert!(
            resolve_peer(None, id, Some(id.to_ambient_ref()))
                .await
                .is_none()
        );
        let stored = PeerRef {
            id,
            auth: PeerAuth::from_hash(42),
        };
        assert!(resolve_peer(None, id, Some(stored)).await.is_none());
        let basic = PeerId::chat(123).unwrap();
        assert_eq!(
            resolve_peer(None, basic, None).await,
            Some(basic.to_ambient_ref())
        );
        assert!(
            resolve_peer(None, PeerId::user(123).unwrap(), None)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn broadcast_updates_do_not_install_a_group_bot() {
        let session = erase(Arc::new(MemorySession::default()));
        let id = PeerId::channel(321).unwrap();
        session
            .cache_peer(&PeerInfo::Channel {
                id: 321,
                auth: Some(PeerAuth::from_hash(42)),
                kind: Some(ChannelKind::Broadcast),
            })
            .await
            .unwrap();
        let stored = PeerRef {
            id,
            auth: PeerAuth::from_hash(42),
        };
        assert!(
            resolve_peer(Some(session.as_ref()), id, Some(stored))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn only_confirmed_group_types_can_reuse_a_saved_hash() {
        let id = PeerId::channel(321).unwrap();
        let stored = PeerRef {
            id,
            auth: PeerAuth::from_hash(42),
        };
        for (kind, allowed) in [
            (None, false),
            (Some(ChannelKind::Broadcast), false),
            (Some(ChannelKind::Community), false),
            (Some(ChannelKind::Megagroup), true),
            (Some(ChannelKind::Gigagroup), true),
        ] {
            let session = erase(Arc::new(MemorySession::default()));
            session
                .cache_peer(&PeerInfo::Channel {
                    id: 321,
                    auth: None,
                    kind,
                })
                .await
                .unwrap();
            assert_eq!(
                resolve_peer(Some(session.as_ref()), id, Some(stored))
                    .await
                    .is_some(),
                allowed,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn additions_creations_and_migrations_trigger_setup_without_matching_other_members() {
        let added: tl::enums::MessageAction =
            tl::types::MessageActionChatAddUser { users: vec![7] }.into();
        let created: tl::enums::MessageAction = tl::types::MessageActionChatCreate {
            title: "group".into(),
            users: vec![7],
        }
        .into();
        let migrated: tl::enums::MessageAction = tl::types::MessageActionChannelMigrateFrom {
            title: "group".into(),
            chat_id: 10,
        }
        .into();
        for action in [&added, &created, &migrated] {
            assert!(setup_service(Some(action), 7));
            assert!(!setup_service(Some(action), 0));
        }
        assert!(!setup_service(Some(&added), 8));
        assert!(!setup_service(Some(&created), 8));
        assert!(!setup_service(None, 7));
    }

    #[test]
    fn channel_information_and_own_rights_updates_trigger_setup() {
        let channel: tl::enums::Update = tl::types::UpdateChannel { channel_id: 12 }.into();
        let (peer, standing, explicit) = setup_update(&channel, 7).unwrap();
        assert_eq!(peer, PeerId::channel(12).unwrap());
        assert!(standing.is_none());
        assert!(!explicit);
        let promoted: tl::enums::Update = tl::types::UpdateChatParticipantAdmin {
            chat_id: 12,
            user_id: 7,
            is_admin: true,
            version: 1,
        }
        .into();
        let (_, standing, explicit) = setup_update(&promoted, 7).unwrap();
        assert!(matches!(
            standing,
            Some(install::Standing::Basic { admin: true })
        ));
        assert!(explicit);
        assert!(setup_update(&promoted, 8).is_none());
        assert!(setup_update(&channel, 0).is_none());
    }

    #[test]
    fn default_keys_are_real() {
        for (key, _) in super::DEFAULTS {
            assert!(
                super::locks::LOCKS.iter().any(|lock| lock.key == *key)
                    || [super::bots::LOCK, super::bots::KICK_ADDER].contains(key),
                "unknown setting key {key}"
            );
        }
    }
}
