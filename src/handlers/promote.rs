use std::time::{Duration, Instant};

use grammers_client::message::{Button, Message, ReplyMarkup};
use grammers_client::session::types::PeerRef;
use grammers_client::tl;
use grammers_client::update::CallbackQuery;

use super::{Ctx, bot_admin_key, esc, is_owner, sender_is_creator};
use crate::response::ResponseKind;

pub const COMMANDS: &[&str] = &[
    "افزودن ادمین",
    "ادمین کن",
    "اضافه کردن ادمین",
    "ارتقا ادمین",
    "ارتقای ادمین",
    "ترفیع ادمین",
    "ادمین کردن",
    "/promote",
];
pub const DEMOTE: &[&str] = &[
    "حذف ادمین",
    "عزل ادمین",
    "برکناری ادمین",
    "تنزل ادمین",
    "لغو ادمین",
    "/demote",
];
pub const TAG: &[&str] = &["تنظیم تگ", "تنظیم مقام", "تنظیم عنوان ادمین", "/settitle"];
pub const TAG_CLEAR: &[&str] = &["حذف تگ", "حذف مقام", "حذف عنوان ادمین", "/cleartitle"];

pub const TAG_MAX: usize = 16;

pub const PENDING_TTL: Duration = Duration::from_secs(600);

#[derive(Clone)]
pub struct Pending {
    pub target: PeerRef,
    pub name: String,

    pub rights: u32,
    pub started: Instant,
}

async fn tag(ctx: &Ctx, message: &Message, text: &str) -> bool {
    let Some((rest, clearing)) = TAG
        .iter()
        .map(|command| (command, false))
        .chain(TAG_CLEAR.iter().map(|command| (command, true)))
        .find_map(|(command, clearing)| {
            super::commands::strip_prefix(text, command).map(|rest| (rest, clearing))
        })
    else {
        return false;
    };
    let Some((arg, title)) = title_arguments(rest, clearing) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    let Some(named) = super::named(message, arg) else {
        return false;
    };
    if title.chars().count() > TAG_MAX {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            format!("تگ باید حداکثر {TAG_MAX} حرف باشد."),
        )
        .await;
        return true;
    }
    if title.is_empty() && !clearing {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            "متن تگ را بنویسید، مثل «تنظیم تگ مدیر ارشد».",
        )
        .await;
        return true;
    }

    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.",
            ),
        )
        .await;
        return true;
    };
    let Ok(Some(chat_ref)) = message.peer_ref().await else {
        return false;
    };

    let outcome = set_rank(ctx, chat_ref, target.id.bare_id(), target, title).await;

    match outcome {
        Ok(()) if clearing => {
            super::respond(
                ctx,
                message,
                ResponseKind::SettingsChanged,
                super::premium::text(format!("✗ تگ {name} برداشته شد.")),
            )
            .await
        }
        Ok(()) => {
            super::respond(
                ctx,
                message,
                ResponseKind::SettingsChanged,
                super::premium::text(format!("✓ تگ {name} · {title}")),
            )
            .await
        }
        Err(e) => {
            eprintln!("tag: {chat}: could not title {name}: {e}");
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    format!(
                        "انجام نشد · {e}\nربات باید ادمین باشد و اجازه «مدیریت مقام ها» داشته باشد."
                    ),
                ),
            )
            .await
        }
    };
    true
}

pub async fn set_rank(
    ctx: &Ctx,
    chat_ref: PeerRef,
    user: Option<i64>,
    target: PeerRef,
    rank: &str,
) -> Result<(), grammers_client::InvocationError> {
    let asked = ctx
        .client
        .invoke_outbound(&tl::functions::messages::EditChatParticipantRank {
            peer: chat_ref.into(),
            participant: target.into(),
            rank: rank.to_owned(),
        })
        .await;
    let failed = match asked {
        Ok(_) => return Ok(()),
        Err(e) => e,
    };
    let known_member = chat_ref
        .id
        .bot_api_dialog_id()
        .and_then(|chat| ctx.cached_admins(chat))
        .zip(user)
        .is_some_and(|(admins, user)| !admins.contains(&user));
    let flood = matches!(&failed, grammers_client::InvocationError::Rpc(rpc) if rpc.code == 420);
    if known_member || flood {
        return Err(failed);
    }

    let Some((peer, _)) = super::admin_ref(ctx, chat_ref, user.unwrap_or_default()).await else {
        return Err(failed);
    };
    match super::restrict::set_member_admin_rank(ctx, chat_ref, peer, rank).await {
        Ok(()) => Ok(()),
        Err(_) => Err(failed),
    }
}

const RIGHTS: &[(&str, u32)] = &[
    ("حذف پیام", 1 << 0),
    ("بن کاربران", 1 << 1),
    ("افزودن اعضا", 1 << 2),
    ("سنجاق پیام", 1 << 3),
    ("مدیریت تماس", 1 << 4),
    ("تغییر اطلاعات", 1 << 5),
    ("افزودن ادمین", 1 << 6),
];

const DEFAULT_RIGHTS: u32 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3);

fn direct_rights(rest: &str) -> Option<(Option<String>, u32)> {
    let normalized = super::commands::normalize(rest);
    let rest = normalized.as_ref();
    let (target, choices) = rest
        .split_once(" با ")
        .or_else(|| rest.strip_prefix("با ").map(|choices| ("", choices)))?;
    let target = if target.is_empty() {
        None
    } else if super::arg_names_a_user(target) {
        Some(target.to_owned())
    } else {
        return None;
    };
    let rights = match choices {
        "پیش فرض" | "default" => DEFAULT_RIGHTS,
        "بدون دسترسی" | "none" => 0,
        _ => {
            let mut bits = 0;
            for choice in choices.split([',', '،']) {
                let choice = choice.trim();
                let (_, bit) = RIGHTS
                    .iter()
                    .find(|(label, _)| super::commands::matches(choice, label))?;
                if bits & *bit != 0 {
                    return None;
                }
                bits |= *bit;
            }
            bits
        }
    };
    Some((target, rights))
}

fn title_target(rest: &str) -> (Option<&str>, &str) {
    match rest.split_once(super::commands::separator) {
        Some((first, tail)) if super::arg_names_a_user(first) => (Some(first), tail.trim()),
        _ if super::arg_names_a_user(rest) => (Some(rest), ""),
        _ => (None, rest),
    }
}

fn title_arguments(rest: &str, clearing: bool) -> Option<(Option<&str>, &str)> {
    let (arg, title) = title_target(rest);
    (!clearing || title.is_empty()).then_some((arg, title))
}

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let text = view.digits();
    if tag(ctx, message, view.text()).await {
        return true;
    }
    let matched = COMMANDS
        .iter()
        .map(|command| (command, true))
        .chain(DEMOTE.iter().map(|command| (command, false)))
        .find_map(|(command, adding)| {
            super::commands::strip_prefix(text, command).map(|rest| (adding, rest.to_owned()))
        });
    let Some((adding, arg)) = matched else {
        return false;
    };

    let direct = adding.then(|| direct_rights(&arg)).flatten();
    let target_arg = match &direct {
        Some((target, _)) => target.as_deref(),
        None => (!arg.is_empty()).then_some(arg.as_str()),
    };
    let Some(named) = super::named(message, target_arg) else {
        return false;
    };
    if !is_owner(ctx, message) && !sender_is_creator(ctx, message).await {
        return true;
    }

    let Some((target, name)) = super::resolve(ctx, message, named).await else {
        super::respond(
            ctx,
            message,
            ResponseKind::CommandError,
            super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "کاربر پیدا نشد. روی پیام او ریپلای کنید یا @username / آیدی عددی بفرستید.",
            ),
        )
        .await;
        return true;
    };

    if !adding {
        demote(ctx, message, target, &name).await;
        return true;
    }

    if let Some((_, rights)) = direct {
        let (Some(chat), Ok(Some(chat_ref))) = (
            message.peer_id().bot_api_dialog_id(),
            message.peer_ref().await,
        ) else {
            super::respond(
                ctx,
                message,
                ResponseKind::CommandError,
                "گروه پیدا نشد؛ دوباره تلاش کنید.",
            )
            .await;
            return true;
        };
        let pending = Pending {
            target,
            name: esc(&name),
            rights,
            started: Instant::now(),
        };
        let body = match apply_promotion(
            ctx,
            chat_ref,
            chat,
            &pending,
            message
                .sender_id()
                .and_then(grammers_client::session::types::PeerId::bare_id),
        )
        .await
        {
            Ok(body) => super::premium::icon_html(Some(super::premium::Icon::User), body),
            Err(error) => {
                ::log::error!(
                    "promote: {chat}: could not promote {}: {error}",
                    pending.name
                );
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    promote_error_text(&error),
                )
            }
        };
        super::respond(ctx, message, ResponseKind::AdminTool, body).await;
        return true;
    }

    let Some(opener) = message
        .sender_id()
        .and_then(grammers_client::session::types::PeerId::bare_id)
    else {
        return false;
    };
    let key = ctx.pending_admin_new(Pending {
        target,
        name: esc(&name),
        rights: DEFAULT_RIGHTS,
        started: Instant::now(),
    });
    let Some(pending) = ctx.pending_admin(key) else {
        return true;
    };

    super::respond_shared(
        ctx,
        message,
        ResponseKind::AdminTool,
        super::premium::html(title(&pending)).reply_markup(markup(&pending, opener, key)),
    )
    .await;
    true
}

pub enum DemoteError {
    NotFound,
    Rpc(super::restrict::MemberMutationError),
    State(crate::state::SettingsWriteError),
}

impl From<super::restrict::MemberMutationError> for DemoteError {
    fn from(error: super::restrict::MemberMutationError) -> Self {
        Self::Rpc(error)
    }
}

impl From<crate::state::SettingsWriteError> for DemoteError {
    fn from(error: crate::state::SettingsWriteError) -> Self {
        Self::State(error)
    }
}

pub async fn demote_by_id(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    user_id: i64,
    name: &str,
    actor: Option<(i64, &str)>,
) -> Result<(), DemoteError> {
    let Some((peer, _)) = super::admin_ref(ctx, chat_ref, user_id).await else {
        return Err(DemoteError::NotFound);
    };
    super::restrict::set_member_admin_rights(
        ctx,
        chat_ref,
        peer,
        super::restrict::AdminRightsSpec::default(),
    )
    .await?;
    let admin_key = bot_admin_key(user_id);
    let badge_key = super::stats::badge_key(user_id);
    ctx.settings
        .try_apply_batch(
            chat,
            &[
                crate::state::SettingMutation::Delete { key: &admin_key },
                crate::state::SettingMutation::Delete { key: &badge_key },
            ],
        )
        .await?;
    ctx.forget_admins(chat);
    super::log::write(
        ctx,
        chat,
        "log_admin",
        super::log::Entry {
            title: "عزل ادمین",
            target: Some((user_id, name)),
            actor,
            ..Default::default()
        },
    )
    .await;
    Ok(())
}

async fn demote(ctx: &Ctx, message: &Message, target: PeerRef, name: &str) {
    let (Ok(Some(chat_ref)), Some(chat), Some(user_id)) = (
        message.peer_ref().await,
        message.peer_id().bot_api_dialog_id(),
        target.id.bare_id(),
    ) else {
        return;
    };

    let by = super::sender_of(message);
    let outcome = demote_by_id(
        ctx,
        chat_ref,
        chat,
        user_id,
        name,
        by.as_ref().map(|(id, name)| (*id, name.as_str())),
    )
    .await;

    match outcome {
        Ok(()) => {
            super::announce(
                ctx,
                message,
                ResponseKind::ModerationAnnouncement,
                super::premium::html(format!(
                    "✗ <b>{}</b> از ادمینی گروه و ربات عزل شد.",
                    esc(name)
                )),
            )
            .await;
            super::respond_if_private(
                ctx,
                message,
                ResponseKind::ModerationConfirmation,
                "عزل از دسترسی تلگرام و فهرست ادمین های ربات تکمیل شد.",
            )
            .await
        }
        Err(e) => {
            match &e {
                DemoteError::NotFound => {
                    eprintln!("promote: {chat}: could not demote {user_id}: not a listed admin")
                }
                DemoteError::Rpc(e) => {
                    eprintln!("promote: {chat}: could not demote {user_id}: {e}")
                }
                DemoteError::State(e) => {
                    eprintln!("promote: {chat}: demoted {user_id}, but state write failed: {e}")
                }
            }
            super::respond(ctx, message, ResponseKind::CommandError, super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    match e {
                        DemoteError::State(error) if error.commit_outcome_unknown() => {
                            "کاربر در تلگرام عزل شد، اما نتیجه ثبت آن نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
                        }
                        DemoteError::State(_) => {
                            "کاربر در تلگرام عزل شد، اما وضعیت ربات ذخیره نشد؛ دوباره تلاش نکنید و گزارش دهید."
                        }
                        _ => "انجام نشد. ربات فقط می تواند ادمین هایی را عزل کند که خودش اضافه کرده است.",
                    },
                )).await
        }
    };
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, chat: i64) {
    let mut parts = payload.splitn(3, ':');
    let (Some(opener), Some(key), Some(what)) = (parts.next(), parts.next(), parts.next()) else {
        return;
    };
    let (Ok(opener), Ok(key)) = (opener.parse::<i64>(), key.parse::<u64>()) else {
        return;
    };
    if query.sender_id().bare_id() != Some(opener) {
        let _ = query
            .answer()
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Locked),
                "این صفحه را شخص دیگری باز کرده است.",
            ))
            .send()
            .await;
        return;
    }
    let Some(mut pending) = ctx.pending_admin(key) else {
        let _ = query
            .answer()
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Timer),
                "این درخواست منقضی شده است. دوباره «افزودن ادمین» را بفرستید.",
            ))
            .send()
            .await;
        return;
    };

    match what {
        "no" => {
            ctx.pending_admin_done(key);
            let _ = query
                .answer()
                .edit(super::premium::icon_html(
                    Some(super::premium::Icon::Close),
                    "افزودن ادمین لغو شد.",
                ))
                .await;
        }
        "ok" => {
            ctx.pending_admin_done(key);
            confirm(ctx, query, chat, &pending).await;
        }
        index => {
            let Some(bit) = index.parse::<usize>().ok().and_then(|i| RIGHTS.get(i)) else {
                return;
            };
            pending.rights ^= bit.1;
            ctx.pending_admin_set_rights(key, pending.rights);
            let _ = query
                .answer()
                .edit(
                    super::premium::html(title(&pending))
                        .reply_markup(markup(&pending, opener, key)),
                )
                .await;
        }
    }
}

enum PromoteError {
    Rpc(super::restrict::MemberMutationError),
    State(crate::state::SettingsWriteError),
}

impl std::fmt::Display for PromoteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rpc(error) => write!(formatter, "{error}"),
            Self::State(error) => write!(formatter, "{error}"),
        }
    }
}

fn promote_error_text(error: &PromoteError) -> &'static str {
    match error {
        PromoteError::Rpc(_) => "انجام نشد. مطمئن شوید ربات ادمین است و اجازه «افزودن ادمین» دارد.",
        PromoteError::State(error) if error.commit_outcome_unknown() => {
            "کاربر در تلگرام ادمین شد، اما نتیجه ثبت آن نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید."
        }
        PromoteError::State(_) => {
            "کاربر در تلگرام ادمین شد، اما وضعیت ربات ذخیره نشد؛ دوباره تلاش نکنید و گزارش دهید."
        }
    }
}

async fn apply_promotion(
    ctx: &Ctx,
    chat_ref: PeerRef,
    chat: i64,
    pending: &Pending,
    by: Option<i64>,
) -> Result<String, PromoteError> {
    let has = |bit: u32| pending.rights & bit != 0;
    super::restrict::set_member_admin_rights(
        ctx,
        chat_ref,
        pending.target,
        super::restrict::AdminRightsSpec {
            delete_messages: has(RIGHTS[0].1),
            ban_users: has(RIGHTS[1].1),
            invite_users: has(RIGHTS[2].1),
            pin_messages: has(RIGHTS[3].1),
            manage_call: has(RIGHTS[4].1),
            change_info: has(RIGHTS[5].1),
            add_admins: has(RIGHTS[6].1),
        },
    )
    .await
    .map_err(PromoteError::Rpc)?;

    if let Some(id) = pending.target.id.bare_id() {
        let admin_key = bot_admin_key(id);
        let badge_key = super::stats::badge_key(id);
        ctx.settings
            .try_apply_batch(
                chat,
                &[
                    crate::state::SettingMutation::Put {
                        key: &admin_key,
                        value: "",
                    },
                    crate::state::SettingMutation::Delete { key: &badge_key },
                ],
            )
            .await
            .map_err(PromoteError::State)?;
    }
    ctx.forget_admins(chat);

    let granted: Vec<&str> = RIGHTS
        .iter()
        .filter(|(_, bit)| has(*bit))
        .map(|(label, _)| *label)
        .collect();
    super::log::write(
        ctx,
        chat,
        "log_admin",
        super::log::Entry {
            title: "ادمین جدید",
            target: pending
                .target
                .id
                .bare_id()
                .map(|id| (id, pending.name.as_str())),
            actor: by.map(|id| (id, "")),
            extra: vec![(
                "دسترسی ها",
                match granted.is_empty() {
                    true => "بدون دسترسی خاص".to_owned(),
                    false => granted.join(" · "),
                },
            )],
            ..Default::default()
        },
    )
    .await;
    Ok(format!(
        "<b>ادمین جدید</b>\n\n<b>{}</b> ادمین گروه و ربات شد.\n\n<b>دسترسی ها</b>\n{}",
        pending.name,
        if granted.is_empty() {
            "‹ بدون دسترسی خاص".to_owned()
        } else {
            granted
                .iter()
                .map(|label| format!("✓ {label}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    ))
}

async fn confirm(ctx: &Ctx, query: &CallbackQuery, chat: i64, pending: &Pending) {
    let Ok(Some(chat_ref)) = query.peer_ref().await else {
        return;
    };
    let body =
        match apply_promotion(ctx, chat_ref, chat, pending, query.sender_id().bare_id()).await {
            Ok(body) => super::premium::icon_html(Some(super::premium::Icon::User), body),
            Err(error) => {
                ::log::error!(
                    "promote: {chat}: could not promote {}: {error}",
                    pending.name
                );
                super::premium::icon_text(
                    Some(super::premium::Icon::ErrorRed),
                    promote_error_text(&error),
                )
            }
        };
    let _ = query.answer().edit(body).await;
}

fn title(pending: &Pending) -> String {
    format!(
        "<b>افزودن ادمین</b>\n\nکاربر · <b>{}</b>\n\nدسترسی ها را انتخاب و تایید کنید.",
        pending.name
    )
}

fn markup(pending: &Pending, opener: i64, key: u64) -> ReplyMarkup {
    let mut rows: Vec<Vec<Button>> = RIGHTS
        .chunks(2)
        .enumerate()
        .map(|(row, pair)| {
            pair.iter()
                .enumerate()
                .map(|(column, (label, bit))| {
                    let index = row * 2 + column;
                    let mark = if pending.rights & bit != 0 {
                        "✓"
                    } else {
                        "✗"
                    };
                    Button::data(
                        format!("{mark}  {label}"),
                        format!("a:{opener}:{key}:{index}").into_bytes(),
                    )
                })
                .collect()
        })
        .collect();
    rows.push(vec![
        super::premium::decorate(
            super::style::data(
                "تایید",
                format!("a:{opener}:{key}:ok").into_bytes(),
                super::style::Colour::Success,
            ),
            Some(super::premium::Icon::Success),
        ),
        super::premium::decorate(
            super::style::data(
                "لغو",
                format!("a:{opener}:{key}:no").into_bytes(),
                super::style::Colour::Danger,
            ),
            Some(super::premium::Icon::Close),
        ),
    ]);
    super::premium::buttons(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_title_keeps_original_spelling_digits_case_and_spacing() {
        let text = "تنظيم\u{200c}تگ ۱۲۳ Admin  ۲\u{200c}ي";
        let rest = super::super::commands::strip_prefix(text, "تنظیم تگ").unwrap();
        assert_eq!(title_target(rest), (Some("۱۲۳"), "Admin  ۲\u{200c}ي"));
        assert_eq!(title_target("@Ali Team Lead"), (Some("@Ali"), "Team Lead"));
        assert_eq!(title_target("Team Lead"), (None, "Team Lead"));
        assert_eq!(title_arguments("", true), Some((None, "")));
        assert_eq!(title_arguments("@Ali", true), Some((Some("@Ali"), "")));
        assert_eq!(title_arguments("@Ali unexpected", true), None);
        assert_eq!(title_arguments("چطوریه", true), None);
    }

    #[test]
    fn rights_bits_are_distinct_and_indexable() {
        let mut seen = 0u32;
        for (index, (label, bit)) in RIGHTS.iter().enumerate() {
            assert!(!label.is_empty());
            assert_eq!(seen & bit, 0, "duplicate bit at index {index}");
            seen |= bit;

            let payload = format!("a:{}:{}:{index}", i64::MAX, u64::MAX);
            assert!(payload.len() <= 64, "payload too long: {payload}");
        }
        assert_eq!(DEFAULT_RIGHTS & !seen, 0, "default grants an unknown right");
    }

    #[test]
    fn direct_promotion_rights_are_a_closed_list() {
        assert_eq!(
            direct_rights("@Ali با حذف پیام، بن کاربران"),
            Some((Some("@ali".to_owned()), RIGHTS[0].1 | RIGHTS[1].1))
        );
        assert_eq!(direct_rights("با پیش فرض"), Some((None, DEFAULT_RIGHTS)));
        assert_eq!(
            direct_rights("123 با بدون دسترسی"),
            Some((Some("123".to_owned()), 0))
        );
        for (label, bit) in RIGHTS {
            assert_eq!(direct_rights(&format!("با {label}")), Some((None, *bit)));
        }
        assert_eq!(direct_rights("@ali با حذف پیام، حذف پیام"), None);
        assert_eq!(direct_rights("@ali با حذف پیام، ناشناخته"), None);
        assert_eq!(direct_rights("@ali با حذف پیام امروز"), None);
        assert_eq!(direct_rights("@ali با"), None);
    }
}
