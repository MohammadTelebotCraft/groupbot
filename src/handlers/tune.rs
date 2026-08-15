use grammers_client::message::Message;

use super::{Ctx, betrayal, captcha, flood, numbers_in, warns};

const SETTINGS: &[(&str, &str)] = &[
    ("اخطار", "warns"),
    ("احراز", "captcha"),
    ("احراز هویت", "captcha"),
    ("خیانت", "betrayal"),
    ("رگبار", "flood"),
    ("اعلان", "notice"),
];

const COMMANDS: &[&str] = &["تنظیم", "ست"];

pub async fn handle(ctx: &Ctx, message: &Message, view: &super::locks::View<'_>) -> bool {
    let text = view.digits();
    let Some(rest) = COMMANDS.iter().find_map(|command| {
        let rest = text.strip_prefix(command)?;
        rest.starts_with(char::is_whitespace)
            .then(|| rest.trim().to_owned())
    }) else {
        return false;
    };
    let Some((name, what)) = SETTINGS
        .iter()

        .max_by_key(|(name, _)| if rest.starts_with(*name) { name.len() } else { 0 })
        .filter(|(name, _)| rest.starts_with(*name))
    else {
        return false;
    };
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };

    let Some(numbers) = numbers_in(&rest[name.len()..]) else {
        return false;
    };

    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    if numbers.is_empty() {
        let _ = message.reply(usage(what)).await;
        return true;
    }

    let reply = match *what {
        "warns" => {
            warns::set_limit(ctx, chat, numbers[0]).await;
            format!("✓ سقف اخطار روی {} تنظیم شد.", warns::limit(ctx, chat))
        }
        "notice" => {
            super::notice::set_ttl(ctx, chat, numbers[0]).await;
            let ttl = super::notice::ttl(ctx, chat);
            if ttl == 0 {
                "✓ اعلان حذف پاک نمی شود.".to_owned()
            } else {
                format!("✓ اعلان حذف پس از {ttl} ثانیه پاک می شود.")
            }
        }
        "captcha" => {
            captcha::set_timeout(ctx, chat, numbers[0]).await;
            format!(
                "✓ مهلت احراز هویت روی {} ثانیه تنظیم شد.",
                captcha::timeout(ctx, chat)
            )
        }
        "betrayal" => {
            betrayal::set(ctx, chat, betrayal::LIMIT, numbers[0]).await;
            if let Some(&minutes) = numbers.get(1) {
                betrayal::set(ctx, chat, betrayal::WINDOW, minutes).await;
            }
            format!(
                "✓ ضد خیانت: بیش از {} حذف در {} دقیقه.",
                betrayal::limit(ctx, chat),
                betrayal::window(ctx, chat)
            )
        }
        _ => {
            flood::set(ctx, chat, flood::LIMIT, numbers[0]).await;
            if let Some(&seconds) = numbers.get(1) {
                flood::set(ctx, chat, flood::WINDOW, seconds).await;
            }
            format!(
                "✓ ضد رگبار: بیش از {} پیام در {} ثانیه.",
                flood::limit(ctx, chat),
                flood::window(ctx, chat)
            )
        }
    };
    let _ = message.reply(reply).await;
    true
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
            let picked = SETTINGS
                .iter()
                .max_by_key(|(name, _)| if alias.starts_with(*name) { name.len() } else { 0 })
                .filter(|(name, _)| alias.starts_with(*name))
                .expect("an alias must match itself");
            assert_eq!(picked.1, *what, "«{alias}» resolved to the wrong setting");
        }
    }
}
