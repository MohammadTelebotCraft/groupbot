use grammers_client::message::Message;

use super::{Ctx, betrayal, captcha, flood, numbers_in, warns};
use crate::response::ResponseKind;
use crate::state::{SettingMutation, SettingsWriteError};

pub const SETTINGS: &[(&str, &str)] = &[
    ("اخطار", "warns"),
    ("سقف اخطار", "warns"),
    ("warns", "warns"),
    ("هشدار", "warns"),
    ("احراز", "captcha"),
    ("احراز هویت", "captcha"),
    ("کپچا", "captcha"),
    ("captcha", "captcha"),
    ("خیانت", "betrayal"),
    ("ضد خیانت", "betrayal"),
    ("betrayal", "betrayal"),
    ("رگبار", "flood"),
    ("ضد رگبار", "flood"),
    ("ضد اسپم", "flood"),
    ("flood", "flood"),
    ("اعلان", "notice"),
    ("اعلان حذف", "notice"),
    ("notice", "notice"),
];

pub const COMMANDS: &[&str] = &[
    "تنظیم",
    "ست",
    "/tune",
    "/adjust",
];

fn parse(text: &str) -> Option<(&'static str, Vec<u32>)> {
    let rest = COMMANDS
        .iter()
        .find_map(|command| super::commands::strip_prefix(text, command))?;
    let (tail, what) = SETTINGS
        .iter()
        .filter_map(|(name, what)| {
            super::commands::strip_prefix(rest, name).map(|tail| (tail, *what))
        })
        .min_by_key(|(tail, _)| tail.len())?;
    let numbers = numbers_in(tail)?;
    let max = if matches!(what, "betrayal" | "flood") {
        2
    } else {
        1
    };
    (numbers.len() <= max).then_some((what, numbers))
}

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let Some((what, numbers)) = parse(view.command()) else {
        return false;
    };
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    if numbers.is_empty() {
        super::respond(ctx, message, ResponseKind::CommandError, usage(what)).await;
        return true;
    }

    let mut kind = ResponseKind::SettingsChanged;
    let reply = match what {
        "warns" => match warns::set_limit(ctx, chat, numbers[0]).await {
            Ok(()) => format!("✓ سقف اخطار روی {} تنظیم شد.", warns::limit(ctx, chat)),
            Err(error) => {
                kind = ResponseKind::CommandError;
                log::warn!("warn limit for {chat} was not stored: {error}");
                failure_text(&error, "تنظیم سقف اخطار ذخیره نشد؛ دوباره تلاش کنید.")
            }
        },
        "notice" => {
            let value = numbers[0].clamp(super::notice::TTL_RANGE.0, super::notice::TTL_RANGE.1);
            match ctx
                .settings
                .try_set_value(chat, super::notice::TTL, &value.to_string())
                .await
            {
                Ok(_) if value == 0 => "✓ اعلان حذف پاک نمی شود.".to_owned(),
                Ok(_) => format!("✓ اعلان حذف پس از {value} ثانیه پاک می شود."),
                Err(error) => {
                    kind = ResponseKind::CommandError;
                    log::warn!("notice ttl for {chat} was not stored: {error}");
                    failure_text(&error, "تنظیم اعلان ذخیره نشد؛ دوباره تلاش کنید.")
                }
            }
        }
        "captcha" => {
            let value = numbers[0].clamp(captcha::TIMEOUT_RANGE.0, captcha::TIMEOUT_RANGE.1);
            match ctx
                .settings
                .try_set_value(chat, captcha::TIMEOUT, &value.to_string())
                .await
            {
                Ok(_) => format!("✓ مهلت احراز هویت روی {value} ثانیه تنظیم شد."),
                Err(error) => {
                    kind = ResponseKind::CommandError;
                    log::warn!("captcha timeout for {chat} was not stored: {error}");
                    failure_text(&error, "مهلت احراز هویت ذخیره نشد؛ دوباره تلاش کنید.")
                }
            }
        }
        "betrayal" => {
            let limit = numbers[0].clamp(betrayal::LIMIT_RANGE.0, betrayal::LIMIT_RANGE.1);
            let minutes = numbers
                .get(1)
                .copied()
                .map(|value| value.clamp(betrayal::WINDOW_RANGE.0, betrayal::WINDOW_RANGE.1));
            let limit_value = limit.to_string();
            let minutes_value = minutes.map(|value| value.to_string());
            let result = match minutes_value.as_deref() {
                Some(value) => {
                    ctx.settings
                        .try_apply_batch(
                            chat,
                            &[
                                SettingMutation::Put {
                                    key: betrayal::LIMIT,
                                    value: &limit_value,
                                },
                                SettingMutation::Put {
                                    key: betrayal::WINDOW,
                                    value,
                                },
                            ],
                        )
                        .await
                }
                None => ctx
                    .settings
                    .try_set_value(chat, betrayal::LIMIT, &limit_value)
                    .await
                    .map(usize::from),
            };
            match result {
                Ok(_) => format!(
                    "✓ ضد خیانت: بیش از {} حذف در {} دقیقه.",
                    betrayal::limit(ctx, chat),
                    betrayal::window(ctx, chat)
                ),
                Err(error) => {
                    kind = ResponseKind::CommandError;
                    log::warn!("betrayal limits for {chat} were not stored: {error}");
                    failure_text(&error, "تنظیم ضد خیانت ذخیره نشد؛ دوباره تلاش کنید.")
                }
            }
        }
        _ => {
            let limit = numbers[0].clamp(flood::LIMIT_RANGE.0, flood::LIMIT_RANGE.1);
            let seconds = numbers
                .get(1)
                .copied()
                .map(|value| value.clamp(flood::WINDOW_RANGE.0, flood::WINDOW_RANGE.1));
            let limit_value = limit.to_string();
            let seconds_value = seconds.map(|value| value.to_string());
            let result = match seconds_value.as_deref() {
                Some(value) => {
                    ctx.settings
                        .try_apply_batch(
                            chat,
                            &[
                                SettingMutation::Put {
                                    key: flood::LIMIT,
                                    value: &limit_value,
                                },
                                SettingMutation::Put {
                                    key: flood::WINDOW,
                                    value,
                                },
                            ],
                        )
                        .await
                }
                None => ctx
                    .settings
                    .try_set_value(chat, flood::LIMIT, &limit_value)
                    .await
                    .map(usize::from),
            };
            match result {
                Ok(_) => format!(
                    "✓ ضد رگبار: بیش از {} پیام در {} ثانیه.",
                    flood::limit(ctx, chat),
                    flood::window(ctx, chat)
                ),
                Err(error) => {
                    kind = ResponseKind::CommandError;
                    log::warn!("flood limits for {chat} were not stored: {error}");
                    failure_text(&error, "تنظیم ضد رگبار ذخیره نشد؛ دوباره تلاش کنید.")
                }
            }
        }
    };
    super::respond(
        ctx,
        message,
        kind,
        super::premium::icon_text(
            Some(match what {
                "warns" => super::premium::Icon::Warning,
                "captcha" | "flood" | "notice" => super::premium::Icon::Timer,
                _ => super::premium::Icon::Locked,
            }),
            reply,
        ),
    )
    .await;
    true
}

fn failure_text(error: &SettingsWriteError, rejected: &str) -> String {
    if error.commit_outcome_unknown() {
        "نتیجه ذخیره سازی نامشخص است؛ پیش از تلاش دوباره وضعیت را بررسی کنید.".to_owned()
    } else {
        rejected.to_owned()
    }
}

fn usage(what: &str) -> &'static str {
    match what {
        "warns" => "مثال: «تنظیم اخطار 5»",
        "captcha" => "مثال: «تنظیم احراز 120» (ثانیه)",
        "betrayal" => "مثال: «تنظیم خیانت 5 10» یعنی ۵ حذف در ۱۰ دقیقه",
        "notice" => "مثال: «تنظیم اعلان 15» (ثانیه، صفر یعنی پاک نشود)",
        _ => "مثال: «تنظیم رگبار 10 5» یعنی ۱۰ پیام در ۵ ثانیه",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_only_a_numeric_tail() {
        assert_eq!(numbers_in(""), Some(vec![]));
        assert_eq!(numbers_in(" 15"), Some(vec![15]));
        assert_eq!(numbers_in(" 10 5"), Some(vec![10, 5]));

        assert_eq!(numbers_in(" شرط 120 30"), None);
        assert_eq!(numbers_in(" abc"), None);
    }

    #[test]
    fn every_alias_is_matched_longest_first() {
        for (alias, what) in SETTINGS {
            let picked = parse(&format!("تنظیم {alias} 15")).expect("an alias must match itself");
            assert_eq!(
                picked,
                (*what, vec![15]),
                "«{alias}» resolved to the wrong setting"
            );
        }
    }

    #[test]
    fn settings_accept_keyboard_variants_but_not_extra_words_or_numbers() {
        assert_eq!(
            parse("تنظيم  احراز\u{200c}هويت ۱۲۰"),
            Some(("captcha", vec![120]))
        );
        assert_eq!(parse("/TUNE ضد اسپم ۱۰ ۵"), Some(("flood", vec![10, 5])));
        for text in [
            "تنظیم اخطار۵",
            "تنظیم اخطار 5 لطفا",
            "تنظیم اخطار 5 9",
            "تنظیم رگبار 5 9 2",
            "تنظیم اعلان شرط 120 30",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
