use std::sync::Arc;

use grammers_client::message::{InputMessage, Message};
use grammers_client::session::types::{PeerId, PeerKind, PeerRef};
use grammers_client::tl;

use super::Ctx;

pub const CLEANER_ADDED: &str = "cln_added";

pub const STATUS: &[&str] = &["وضعیت نصب", "بررسی نصب"];

pub struct Need {
    pub label: &'static str,
    pub has: fn(&tl::types::ChatAdminRights) -> bool,

    pub for_what: &'static str,
}

pub const NEEDED: &[Need] = &[
    Need {
        label: "حذف پیام ها",
        has: |rights| rights.delete_messages,
        for_what: "قفل ها و فیلترها",
    },
    Need {
        label: "مسدود کردن کاربران",
        has: |rights| rights.ban_users,
        for_what: "سکوت، بن و ضد رگبار",
    },
    Need {
        label: "افزودن کاربران",
        has: |rights| rights.invite_users,
        for_what: "لینک دعوت و افزودن کلینر",
    },
    Need {
        label: "سنجاق کردن پیام ها",
        has: |rights| rights.pin_messages,
        for_what: "سنجاق و قفل سنجاق",
    },
    Need {
        label: "تغییر اطلاعات گروه",
        has: |rights| rights.change_info,
        for_what: "اختیارات گروه و قفل گروه",
    },
    Need {
        label: "افزودن ادمین جدید",
        has: |rights| rights.add_admins,
        for_what: "کلینر",
    },
];

const OPTIONAL: &[Need] = &[Need {
    label: "تنظیم مقام اعضا",
    has: |rights| rights.manage_ranks,
    for_what: "«تنظیم تگ» و مقام های خودکار",
}];

const ALL_MISSING: u32 = (1u32 << NEEDED.len()) - 1;

const MEMBER_MARK: i64 = 1 << 40;
const BASIC_MARK: i64 = 2 << 40;

const COMPLETE_MARK: i64 = 4 << 40;

pub enum Standing {
    Admin(tl::types::ChatAdminRights),

    Creator,

    Member,

    Basic { admin: bool },

    Gone,

    Unknown,
}

pub fn missing(standing: &Standing) -> u32 {
    match standing {
        Standing::Creator => 0,
        Standing::Admin(rights) => NEEDED.iter().enumerate().fold(0, |mask, (bit, need)| {
            match (need.has)(rights) {
                true => mask,
                false => mask | 1 << bit,
            }
        }),
        _ => ALL_MISSING,
    }
}

pub fn ready(standing: &Standing) -> bool {
    matches!(standing, Standing::Creator | Standing::Admin(_)) && missing(standing) == 0
}

pub async fn standing(ctx: &Ctx, chat_ref: PeerRef) -> Standing {
    let me_id = ctx.me_id();
    let Some(me) = PeerId::user(me_id).map(PeerId::to_ambient_ref) else {
        return Standing::Unknown;
    };
    if chat_ref.id.kind() != PeerKind::Channel {
        return match ctx.client.get_permissions(chat_ref, me).await {
            Ok(permissions) => Standing::Basic {
                admin: permissions.is_admin(),
            },
            Err(_) => Standing::Unknown,
        };
    }
    match ctx
        .client
        .invoke(&tl::functions::channels::GetParticipant {
            channel: chat_ref.into(),
            participant: me.into(),
        })
        .await
    {
        Ok(tl::enums::channels::ChannelParticipant::Participant(found)) => {
            from_participant(Some(&found.participant))
        }
        Err(grammers_client::InvocationError::Rpc(rpc))
            if rpc.name.contains("USER_NOT_PARTICIPANT") =>
        {
            Standing::Gone
        }
        Err(e) => {
            eprintln!("install: cannot read own rights in {}: {e}", chat_ref.id);
            Standing::Unknown
        }
    }
}

pub fn from_participant(participant: Option<&tl::enums::ChannelParticipant>) -> Standing {
    match participant {
        Some(tl::enums::ChannelParticipant::Creator(_)) => Standing::Creator,
        Some(tl::enums::ChannelParticipant::Admin(admin)) => {
            let tl::enums::ChatAdminRights::Rights(rights) = &admin.admin_rights;
            Standing::Admin(rights.clone())
        }
        Some(
            tl::enums::ChannelParticipant::Participant(_)
            | tl::enums::ChannelParticipant::ParticipantSelf(_),
        ) => Standing::Member,
        _ => Standing::Gone,
    }
}

fn tick(has: bool) -> &'static str {
    match has {
        true => "✓",
        false => "✗",
    }
}

fn checklist(standing: &Standing) -> String {
    let mask = missing(standing);
    let mut lines: Vec<String> = NEEDED
        .iter()
        .enumerate()
        .map(|(bit, need)| {
            format!(
                "{} {} · <i>{}</i>",
                tick(mask & 1 << bit == 0),
                need.label,
                need.for_what
            )
        })
        .collect();
    if let Standing::Admin(rights) = standing {
        lines.push(String::new());
        lines.push("<b>اختیاری</b>".to_owned());
        lines.extend(OPTIONAL.iter().map(|need| {
            format!(
                "{} {} · <i>{}</i>",
                tick((need.has)(rights)),
                need.label,
                need.for_what
            )
        }));
    }
    lines.join("\n")
}

fn lacking(standing: &Standing) -> String {
    let mask = missing(standing);
    let mut lines: Vec<String> = NEEDED
        .iter()
        .enumerate()
        .filter(|(bit, _)| mask & 1 << bit != 0)
        .map(|(_, need)| format!("✗ {} · <i>{}</i>", need.label, need.for_what))
        .collect();
    if let Standing::Admin(rights) = standing {
        let short: Vec<String> = OPTIONAL
            .iter()
            .filter(|need| !(need.has)(rights))
            .map(|need| format!("✗ {} · <i>{}</i>", need.label, need.for_what))
            .collect();
        if !short.is_empty() {
            lines.push(String::new());
            lines.push("<b>اختیاری</b> · <i>جلوی فعال شدن را نمی گیرد</i>".to_owned());
            lines.extend(short);
        }
    }
    lines.join("\n")
}

fn how_many(count: u32) -> &'static str {
    match count {
        1 => "یک",
        2 => "دو",
        3 => "سه",
        4 => "چهار",
        5 => "پنج",
        _ => "شش",
    }
}

pub fn card(standing: &Standing, installed: bool) -> String {
    let short = missing(standing).count_ones();
    let head = match standing {
        Standing::Basic { admin: true } => {
            "ربات ادمین است، ولی این گروه هنوز سوپرگروه نیست و در گروه معمولی دسترسی جدا وجود ندارد. \
             در تنظیمات ادمینِ ربات همه دسترسی ها را بدهید؛ تلگرام همانجا گروه را ارتقا می دهد."
                .to_owned()
        }
        Standing::Basic { admin: false } => {
            "این گروه هنوز سوپرگروه نیست و ربات در آن ادمین هم نیست، پس هیچ کاری از او بر نمی آید. \
             ربات را با همه دسترسی ها ادمین کنید؛ تلگرام خودش گروه را ارتقا می دهد."
                .to_owned()
        }
        Standing::Member => {
            "ربات در گروه هست ولی ادمین نیست، پس هیچ کاری از او بر نمی آید.".to_owned()
        }
        _ if installed => format!(
            "ربات {} دسترسی لازم را ندارد و آن بخش کار نمی کند.",
            how_many(short)
        ),
        _ => format!(
            "ربات ادمین است ولی {} دسترسی لازم را ندارد، پس هنوز فعال نشده است.",
            how_many(short)
        ),
    };
    let tail = match (installed, short) {
        (true, 1) => "همین یکی را در تنظیمات ادمینِ ربات بدهید. «وضعیت نصب» دوباره بررسی می کند.",
        (true, _) => "این ها را در تنظیمات ادمینِ ربات بدهید. «وضعیت نصب» دوباره بررسی می کند.",
        (false, 1) => "همین یکی مانده. به محض دادنش ربات خودش فعال می شود و کلینر را هم می آورد.",
        (false, _) => "تا کامل شدن این ها ربات فعال نمی شود. به محض دادن آخرین دسترسی، \
                       خودش فعال می شود و کلینر را هم می آورد.",
    };
    format!(
        "<b>نصب ربات</b>\n\n\
         {head}\n\n\
         <b>باقی مانده</b>\n\
         {}\n\n\
         <i>{tail}</i>",
        lacking(standing)
    )
}

fn notice_key(standing: &Standing) -> i64 {
    i64::from(missing(standing))
        + match standing {
            Standing::Member => MEMBER_MARK,
            Standing::Basic { admin } => BASIC_MARK + i64::from(*admin),
            _ => 0,
        }
}

pub async fn announce(ctx: &Ctx, chat_ref: PeerRef, chat: i64, standing: &Standing) {
    if matches!(standing, Standing::Gone | Standing::Unknown) {
        return;
    }
    if !ctx.claim_install_notice(chat, notice_key(standing)) {
        return;
    }
    let installed = super::owner(ctx, chat).is_some();
    let _ = ctx
        .client
        .send_message(chat_ref, InputMessage::new().html(card(standing, installed)))
        .await;
}

pub async fn announce_complete(ctx: &Ctx, chat_ref: PeerRef, chat: i64) {
    if !ctx.claim_install_notice(chat, COMPLETE_MARK) {
        return;
    }
    let _ = ctx
        .client
        .send_message(
            chat_ref,
            InputMessage::new().html(
                "<b>نصب ربات</b>\n\n\
                 ✓ همه دسترسی های لازم داده شد.\n\n\
                 <i>«وضعیت نصب» وضعیت کامل را نشان می دهد.</i>",
            ),
        )
        .await;
}

pub async fn ensure_cleaner(ctx: &Arc<Ctx>, chat_ref: PeerRef, chat: i64) -> bool {
    if ctx.cleaner_id().is_none() || ctx.user_client().is_none() {
        return false;
    }
    if ctx.settings.is_locked(chat, CLEANER_ADDED) {
        return false;
    }

    if !ctx.claim_cleaner_install(chat) {
        return false;
    }
    let permit = ctx.cleaner_slot().await;
    let ctx = Arc::clone(ctx);
    tokio::spawn(async move {
        let _permit = permit;
        let told = match super::cleaner::install(&ctx, chat_ref).await {
            Ok(super::cleaner::Installed::AlreadyAdmin) => {
                ctx.settings.set(chat, CLEANER_ADDED, true).await;
                None
            }
            Ok(super::cleaner::Installed::Promoted) => {
                ctx.settings.set(chat, CLEANER_ADDED, true).await;
                Some("✓ کلینر اضافه و ادمین شد. حالا «حذف همه» و «حذف پیام» هم کار می کنند.".to_owned())
            }

            Ok(super::cleaner::Installed::JoinedNotAdmin(reason)) => {
                ctx.settings.set(chat, CLEANER_ADDED, true).await;
                Some(format!("کلینر وارد گروه شد ولی ادمین نشد · {reason}"))
            }
            Err(reason) => {
                eprintln!("install: cleaner not added to {chat}: {reason}");
                Some(format!(
                    "کلینر اضافه نشد · {reason}\nبعدا «افزودن کلینر» را بفرستید."
                ))
            }
        };
        if let Some(told) = told {
            let _ = ctx.client.send_message(chat_ref, told).await;
        }
        ctx.forget_user_chats();
    });
    true
}

async fn cleaner_line(ctx: &Ctx, chat_ref: PeerRef, joining: bool) -> &'static str {
    let Some(cleaner) = ctx.cleaner_id() else {
        return "کلینر · روی این ربات وارد نشده است";
    };
    if joining {
        return "کلینر · در حال افزوده شدن";
    }
    let Some(target) = PeerId::user(cleaner).map(PeerId::to_ambient_ref) else {
        return "کلینر · وضعیتش خوانده نشد";
    };
    match ctx.client.get_permissions(chat_ref, target).await {
        Ok(state) if state.is_admin() => "✓ کلینر · ادمین این گروه",
        Ok(state) if state.has_left() || state.is_banned() => {
            "✗ کلینر · در گروه نیست · «افزودن کلینر» را بفرستید"
        }
        Ok(_) => "✗ کلینر · در گروه هست ولی ادمین نیست · «افزودن کلینر» را بفرستید",
        Err(grammers_client::InvocationError::Rpc(rpc))
            if rpc.name.contains("USER_NOT_PARTICIPANT") =>
        {
            "✗ کلینر · در گروه نیست · «افزودن کلینر» را بفرستید"
        }
        Err(_) => "کلینر · وضعیتش خوانده نشد",
    }
}

pub async fn handle(ctx: &Arc<Ctx>, message: &Message) -> bool {
    if !STATUS.contains(&message.text().trim()) {
        return false;
    }
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let (Ok(Some(chat_ref)), Some(chat)) = (
        message.peer_ref().await,
        message.peer_id().bot_api_dialog_id(),
    ) else {
        return false;
    };

    let standing = standing(ctx, chat_ref).await;
    if matches!(standing, Standing::Unknown | Standing::Gone) {
        let _ = message
            .reply("نتوانستم دسترسی های خودم را بخوانم. چند لحظه بعد دوباره بفرستید.")
            .await;
        return true;
    }
    if !ready(&standing) {
        let _ = message
            .reply(InputMessage::new().html(card(&standing, super::owner(ctx, chat).is_some())))
            .await;
        return true;
    }

    if super::owner(ctx, chat).is_none() {
        if super::autoconfig::configure(ctx, chat_ref).await {
            ensure_cleaner(ctx, chat_ref, chat).await;
            return true;
        }

        if super::owner(ctx, chat).is_none() {
            let _ = message
                .reply(
                    "دسترسی ها کامل است ولی فعال سازی انجام نشد؛ سازنده گروه خوانده نشد. \
                     اگر گروه سازنده دارد، چند دقیقه بعد دوباره بفرستید یا سازنده «کانفیگ» را بفرستد.",
                )
                .await;
            return true;
        }
    }

    let joining = ensure_cleaner(ctx, chat_ref, chat).await;
    let _ = message
        .reply(InputMessage::new().html(format!(
            "<b>نصب ربات</b>\n\n\
             ✓ نصب کامل است.\n\n\
             <b>دسترسی ها</b>\n\
             {}\n\n\
             {}\n\n\
             <i>«پنل» برای تنظیمات، «راهنما» برای دستورها</i>",
            checklist(&standing),
            cleaner_line(ctx, chat_ref, joining).await,
        )))
        .await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rights(all: bool) -> tl::types::ChatAdminRights {
        tl::types::ChatAdminRights {
            change_info: all,
            post_messages: all,
            edit_messages: all,
            delete_messages: all,
            ban_users: all,
            invite_users: all,
            pin_messages: all,
            add_admins: all,
            anonymous: false,
            manage_call: all,
            other: all,
            manage_topics: all,
            post_stories: all,
            edit_stories: all,
            delete_stories: all,
            manage_direct_messages: all,
            manage_ranks: all,
            manage_linked_peers: all,
        }
    }

    #[test]
    fn only_a_complete_administrator_is_ready() {
        assert!(ready(&Standing::Admin(rights(true))));
        assert!(ready(&Standing::Creator));
        for standing in [
            Standing::Admin(rights(false)),
            Standing::Member,
            Standing::Basic { admin: true },
            Standing::Basic { admin: false },
            Standing::Gone,
            Standing::Unknown,
        ] {
            assert!(!ready(&standing), "readiness leaked");
            assert_ne!(missing(&standing), 0, "a standing with nothing missing");
        }
    }

    #[test]
    fn the_card_names_only_what_is_missing() {
        for (bit, need) in NEEDED.iter().enumerate() {
            let mut only_this = rights(true);
            match need.label {
                "حذف پیام ها" => only_this.delete_messages = false,
                "مسدود کردن کاربران" => only_this.ban_users = false,
                "افزودن کاربران" => only_this.invite_users = false,
                "سنجاق کردن پیام ها" => only_this.pin_messages = false,
                "تغییر اطلاعات گروه" => only_this.change_info = false,
                "افزودن ادمین جدید" => only_this.add_admins = false,
                other => panic!("{other} has no test"),
            }
            let standing = Standing::Admin(only_this);
            assert_eq!(missing(&standing), 1 << bit, "{}", need.label);

            let short = lacking(&standing);
            assert_eq!(
                short.lines().filter(|line| !line.is_empty()).count(),
                1,
                "«{}» drew more than the one line that is missing",
                need.label
            );
            assert!(short.contains(need.label), "«{}» is not named", need.label);
            assert!(
                !short.contains('✓'),
                "«{}» drew a right that is already granted",
                need.label
            );

            for installed in [true, false] {
                let rendered = card(&standing, installed);
                assert_eq!(rendered.matches('✗').count(), 1, "{}", need.label);
                assert!(!rendered.contains('✓'), "{}", need.label);
                assert!(rendered.contains("یک دسترسی"), "{}", need.label);
            }
        }

        let mut no_ranks = rights(true);
        no_ranks.manage_ranks = false;
        let standing = Standing::Admin(no_ranks);
        assert_eq!(missing(&standing), 0, "an optional right is not a blocker");
        assert!(ready(&standing), "the optional right blocked installation");
        let short = lacking(&standing);
        assert!(short.contains("تنظیم مقام اعضا"));
        assert!(short.contains("اختیاری"));
    }

    #[test]
    fn the_full_mask_covers_the_whole_table() {
        assert_eq!(missing(&Standing::Admin(rights(false))), ALL_MISSING);
        assert_eq!(missing(&Standing::Member), ALL_MISSING);
        assert!(NEEDED.len() < 32, "the mask cannot hold the table");

        assert!(
            NEEDED.len() <= 6,
            "how_many has no word for {} rights",
            NEEDED.len()
        );
        for count in 1..=NEEDED.len() as u32 {
            assert!(!how_many(count).is_empty());
        }
    }

    #[test]
    fn no_two_cards_share_a_notice_key() {
        let mut keys = vec![COMPLETE_MARK];
        for mask in 1..=ALL_MISSING {
            let mut some = rights(true);
            for (bit, need) in NEEDED.iter().enumerate() {
                if mask & 1 << bit == 0 {
                    continue;
                }
                match need.label {
                    "حذف پیام ها" => some.delete_messages = false,
                    "مسدود کردن کاربران" => some.ban_users = false,
                    "افزودن کاربران" => some.invite_users = false,
                    "سنجاق کردن پیام ها" => some.pin_messages = false,
                    "تغییر اطلاعات گروه" => some.change_info = false,
                    "افزودن ادمین جدید" => some.add_admins = false,
                    other => panic!("{other} has no test"),
                }
            }
            let standing = Standing::Admin(some);
            assert_eq!(missing(&standing), mask, "the mask did not round trip");
            keys.push(notice_key(&standing));
        }
        for standing in [
            Standing::Member,
            Standing::Basic { admin: true },
            Standing::Basic { admin: false },
        ] {
            assert!(!ready(&standing), "a ready standing does not draw a card");
            keys.push(notice_key(&standing));
        }

        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two cards share a notice key");
    }

    #[test]
    fn nothing_is_both_needed_and_optional() {
        for optional in OPTIONAL {
            assert!(
                !NEEDED.iter().any(|need| need.label == optional.label),
                "«{}» is in both tables",
                optional.label
            );
        }
    }

    #[test]
    fn the_card_carries_only_the_markup_it_means() {
        for standing in [
            Standing::Admin(rights(false)),
            Standing::Admin(rights(true)),
            Standing::Member,
            Standing::Basic { admin: false },
        ] {
            for installed in [true, false] {
                let rendered = card(&standing, installed);
                assert!(rendered.starts_with("<b>نصب ربات</b>"));
                assert!(!rendered.contains('\u{200c}'), "found U+200C in the card");
                assert!(rendered.chars().count() < 3_500);
            }
        }
    }
}
