use grammers_client::message::{Button, InputMessage, Message, ReplyMarkup};
use grammers_client::session::types::PeerId;

use super::{Ctx, can_manage, esc, is_owner, name_of, owner, sender_is_creator};

pub const OWNER: &str = "owner";

const CONFIG: &[&str] = &["کانفیگ", "پیکربندی"];
const ADMIN_LIST: &[&str] = &["لیست ادمین", "لیست ادمین ها", "ادمین ها"];
const PROMOTE: &[&str] = &["ترفیع"];
const DEMOTE: &[&str] = &["تنزل"];
const HELP: &[&str] = &["راهنما", "دستورها", "دستورات"];

const START: &[&str] = &["/start", "شروع"];

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
         ۲ · او را ادمین کنید تا فعال شود\n\
         ۳ · در گروه «کانفیگ» را بفرستید\n\n\
         <i>گروه باید سوپرگروه باشد · «راهنما» برای دستورها، «پنل» برای تنظیمات</i>",
        esc(&name_of(message)),
    ));

    if let Ok(me) = ctx.client.get_me().await
        && let Some(username) = me.username()
    {
        card = card.reply_markup(ReplyMarkup::from_buttons_row(&[Button::url(
            "افزودن ربات به گروه",
            format!("https://t.me/{username}?startgroup=new"),
        )]));
    }
    let _ = message.reply(card).await;
    true
}

pub async fn help(message: &Message) -> bool {
    if !HELP.contains(&message.text().trim()) {
        return false;
    }
    let _ = message.reply(InputMessage::new().html(COMMANDS)).await;
    true
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
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
            return true;
        }
        ctx.settings.set_value(chat, OWNER, &sender.to_string()).await;
        let admin_names = match message.peer_ref().await {
            Ok(Some(chat_ref)) => super::admins(ctx, chat_ref).await.1,
            _ => Vec::new(),
        };

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
        return true;
    }

    if help(message).await {
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

pub const COMMANDS: &str = "<b>دستورها</b>\n\
         <b>افزودن ادمین</b> · ادمین گروه و ربات با انتخاب دسترسی ها\n\
         <b>حذف ادمین</b> · عزل ادمین گروه و ربات\n\
         <b>ترفیع</b> · ادمین ربات کردن (معاف از قفل ها)\n\
         <b>تنظیم تگ مدیر ارشد</b> / <b>حذف تگ</b> · مقام نمایشی ادمین در تلگرام\n\
         <b>تنزل</b> · عزل ادمین ربات\n\
         <b>سکوت</b> یا <b>خفه</b> / <b>حذف سکوت</b> · بستن و باز کردن پیام ها\n\
         <b>بن</b> یا <b>سیک</b> / <b>حذف بن</b> · اخراج و بازگرداندن\n\
         <b>قفل …</b> / <b>بازکردن …</b> · قفل ها، مثل «قفل عکس» یا «قفل سرویس»\n\
         <b>پنل</b> · پنل دکمه ای قفل ها و تنظیمات پیشرفته\n\
         <b>پنل پیوی</b> · همان پنل، در پیوی شما\n\
         <b>خوشامد</b> / <b>حذف خوشامد</b> · پیام خوشامد با تگ زنده\n\
         <b>اخطار</b> / <b>حذف اخطار</b> · اخطار تا سقف تعیین شده\n\
         <b>قفل گروه</b> · بستن ارسال پیام برای همه\n\
         <b>اختیارات گروه</b> / <b>اختیار عکس بسته</b> · اختیارات اعضای عادی در تلگرام\n\
         <b>گزارش</b> · گزارش پیام به مدیران (با ریپلای)\n\
         <b>تنظیم پاسخ</b> / <b>حذف پاسخ</b> · پاسخ خودکار (ریپلای یا «سلام = درود»)\n\
         <b>لیست ادمین</b> · ادمین های گروه و ربات\n\
         <b>امار</b> · داشبورد آمار گروه\n\
         <b>تنظیم گزارش روزانه 21</b> / <b>حذف گزارش روزانه</b> · خلاصه هر روز در گروه\n\
         <b>قوانین</b> / <b>تنظیم قوانین</b> · قوانین گروه\n\
         <b>یادداشت</b> · یادداشت ادمین روی یک کاربر\n\
         <b>سنجاق</b> / <b>حذف سنجاق</b> · سنجاق پیام (بی صدا هم دارد)\n\
         <b>اسلوموشن 30</b> · کندی ارسال پیام\n\
         <b>قفل شب 23 تا 7</b> · بستن خودکار گروه در شب\n\
         <b>تنظیم ویژه</b> / <b>حذف ویژه</b> · معافیت از قفل ها بدون دسترسی\n\
         <b>فیلتر کلمه</b> / <b>حذف فیلتر</b> · حذف پیام های حاوی کلمه\n\
         <b>معاف</b> / <b>حذف معاف</b> · معاف کردن از عضویت اجباری و اد اجباری\n\
         <b>لیست بن</b> / <b>لیست سکوت</b> / <b>لیست ویژه</b> / <b>لیست فیلتر</b> / <b>لیست معاف</b> · با حذف تکی\n\
         <b>پاکسازی لیست بن</b> · خالی کردن یک لیست\n\
         <b>حذف 99</b> · پاک کردن پیام های اخیر (با کلینر، قدیمی ها هم)\n\
         <b>حذف همه</b> · پاک کردن کل پیام های گروه، با تایید\n\
         <b>افزودن کلینر</b> · آوردن حساب پاکسازی به گروه و ادمین کردنش\n\
         <b>حذف پیام</b> · پاک کردن همه پیام های یک کاربر در گروه\n\
         <b>پاکسازی عکس</b> / <b>ویدیو</b> / <b>ویس</b> / <b>فایل</b> / <b>لینک</b> · پاک کردن یک نوع محتوا از کل گروه\n\
         <b>تنظیم لاگ @channel</b> / <b>حذف لاگ</b> · نوشتن رویدادهای گروه در کانال\n\
         <b>تگ</b> / <b>تگ 30 متن</b> · فراخوان اعضا، با ریپلای روی همان پیام\n\
         <b>تنظیم عضویت اجباری @channel</b> / <b>حذف عضویت اجباری</b> · اجبار به عضویت کانال\n\
         <b>تنظیم اد اجباری 5</b> / <b>حذف اد اجباری</b> · تا نفر مشخص اضافه نکند، نمی نویسد\n\
         <b>تنظیم اعلان شرط 120 30</b> · فاصله و حذف خودکار اعلان شرط ورود\n\
         <b>قفل ها</b> · وضعیت قفل ها\n\
         <b>پینگ</b> · سرعت پاسخ ربات\n\
         <b>تنظیم اخطار 5</b> · تنظیم عددی هر بخش با دستور\n\n\
         <i>هدف هر دستور: ریپلای، @username یا آیدی عددی</i>";
