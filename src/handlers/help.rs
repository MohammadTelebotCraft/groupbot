use super::{limits, locks, log, rights, tempmedia};

pub struct Cmd {
    pub form: &'static str,
    pub does: &'static str,
}

pub struct Topic {
    pub id: &'static str,

    pub icon: &'static str,
    pub title: &'static str,
    pub intro: &'static str,
    pub commands: &'static [Cmd],

    pub extra: Option<fn() -> String>,

    pub notes: &'static [&'static str],
}

const fn cmd(form: &'static str, does: &'static str) -> Cmd {
    Cmd { form, does }
}

pub const INDEX_ID: &str = "i";

pub const INDEX: &[&str] = &[
    "locks", "filter", "usr", "adm", "sec", "msg", "tm", "ls", "cl", "st", "gr", "lim",
    "etc",
];

pub const TOPICS: &[Topic] = &[
    Topic {
        id: "locks",
        icon: "🔒",
        title: "قفل ها",
        intro: "هر قفل یک نوع محتوا را می گیرد و پیامش را حذف می کند. ادمین ها و کاربران ویژه از قفل ها معاف اند.",
        commands: &[
            cmd("قفل عکس", "یک قفل را روشن می کند"),
            cmd("بازکردن عکس", "همان قفل را بر می دارد"),
            cmd("قفل همه", "هر قفل موجود را با هم روشن می کند"),
            cmd("بازکردن همه", "همه را با هم بر می دارد"),
            cmd("قفل ها", "فهرست قفل های روشن این گروه"),
            cmd("قفل گروه", "ارسال پیام را برای همه اعضا می بندد"),
            cmd("قفل فوروارد", "صفحه انتخاب فوروارد از کانال یا از کاربر"),
            cmd("قفل یوزرنیم", "صفحه انتخاب یوزرنیم، تگ و دستور ربات"),
            cmd("قفل ربات", "صفحه انتخاب قفل ربات و اخراج اضافه کننده"),
            cmd("قفل پک", "با ریپلای روی یک استیکر، کل پک آن را می بندد"),
            cmd("بازکردن پک", "قفل آن پک را بر می دارد"),
            cmd("قفل سنجاق", "سنجاق را روی همین پیام نگه می دارد"),
        ],
        extra: Some(lock_names),
        notes: &[
            "بازکردن، باز کردن، آنلاک و بازکن هر چهار به جای هم کار می کنند.",
            "قفل سنجاق فقط از مالک گروه پذیرفته می شود.",
            "«قفل ها» را همه می توانند بفرستند؛ باقی برای ادمین است.",
        ],
    },
    Topic {
        id: "usr",
        icon: "👤",
        title: "مدیریت کاربر",
        intro: "هدف هر کدام از این ها یک ریپلای است، یا یوزرنیم، یا آیدی عددی. مدت زمان اختیاری است و بدون آن دائمی می شود.",
        commands: &[
            cmd("سکوت", "کاربر را ساکت می کند"),
            cmd("سکوت 10 دقیقه", "سکوت با مدت مشخص"),
            cmd("حذف سکوت", "سکوت را بر می دارد"),
            cmd("بن", "کاربر را از گروه اخراج می کند"),
            cmd("بن @ali 1 روز", "بن با مدت مشخص"),
            cmd("حذف بن", "بن را بر می دارد"),
            cmd("اخطار", "یک اخطار ثبت می کند"),
            cmd("حذف اخطار", "یک اخطار کم می کند"),
            cmd("اخطارها", "اخطارهای یک کاربر"),
            cmd("ویژه", "کاربر ویژه؛ معاف از قفل ها بدون هیچ دسترسی"),
            cmd("حذف ویژه", "از لیست ویژه بر می دارد"),
            cmd("معاف", "از عضویت اجباری و اد اجباری معاف می کند"),
            cmd("حذف معاف", "معافیت را بر می دارد"),
        ],
        extra: None,
        notes: &[
            "واحدها: ثانیه، دقیقه، ساعت، روز، هفته، ماه.",
            "کمتر از ۳۰ ثانیه و بیشتر از ۳۶۶ روز را تلگرام دائمی حساب می کند، پس ربات آن را به ۳۰ ثانیه می رساند.",
            "خفه و سیک همان سکوت و بن هستند.",
            "سقف اخطار و کاری که در سقف انجام می شود در پنل › اخطار تنظیم می شود.",
        ],
    },
    Topic {
        id: "adm",
        icon: "👑",
        title: "ادمین ها",
        intro: "دو سطح ادمین هست: ادمین واقعی تلگرام، و ادمین ربات که فقط از قفل ها معاف است و به دستورها دسترسی دارد.",
        commands: &[
            cmd("افزودن ادمین", "ادمین تلگرام با انتخاب دسترسی ها روی صفحه شیشه ای"),
            cmd("حذف ادمین", "عزل ادمین تلگرام"),
            cmd("ترفیع", "ادمین ربات می کند، بدون دست زدن به دسترسی تلگرام"),
            cmd("تنزل", "ادمین ربات را عزل می کند"),
            cmd("تنظیم تگ مدیر ارشد", "مقام نمایشی ادمین در تلگرام، تا ۱۶ حرف"),
            cmd("حذف تگ", "مقام نمایشی را بر می دارد"),
            cmd("لیست ادمین", "ادمین های گروه و ادمین های ربات"),
            cmd("کانفیگ", "ثبت مالک و روشن کردن تنظیمات پیشنهادی"),
        ],
        extra: None,
        notes: &[
            "ترفیع، تنزل و کانفیگ فقط از مالک ربات یا سازنده گروه پذیرفته می شوند.",
            "کانفیگ را باید یک بار سازنده گروه بفرستد تا مالک ثبت شود.",
            "«لیست ادمین» را همه می توانند بفرستند.",
        ],
    },
    Topic {
        id: "sec",
        icon: "🛡",
        title: "امنیت و ورود",
        intro: "گاردهایی که خودشان کار می کنند و شرط هایی که پیش از نوشتن باید انجام شود. تنظیم دقیق هر کدام در صفحه خودش است.",
        commands: &[
            cmd("ضد رگبار 10 5", "بیش از ۱۰ پیام در ۵ ثانیه یعنی سکوت یا بن"),
            cmd("تنظیم خیانت 5 10", "ادمینی که ۵ نفر را در ۱۰ دقیقه حذف کند، عزل می شود"),
            cmd("تنظیم احراز 120", "مهلت احراز هویت عضو تازه، به ثانیه"),
            cmd("تنظیم اخطار 5", "سقف اخطار"),
            cmd("تنظیم عضویت اجباری @channel", "تا عضو کانال نشود پیامش پاک می شود"),
            cmd("حذف عضویت اجباری", "شرط عضویت را بر می دارد"),
            cmd("تنظیم اد اجباری 5", "تا ۵ نفر اضافه نکند اجازه نوشتن ندارد"),
            cmd("حذف اد اجباری", "شرط اد را بر می دارد"),
            cmd("تنظیم اعلان شرط 120 30", "فاصله اعلان شرط و حذف خودکارش، به ثانیه"),
            cmd("معاف", "یک کاربر را از شرط های ورود معاف می کند"),
            cmd("ربات مجاز", "یک ربات را از قفل ربات استثنا می کند"),
            cmd("حذف ربات مجاز", "آن استثنا را بر می دارد"),
        ],
        extra: None,
        notes: &[
            "ضد هجوم فقط از پنل روشن می شود.",
            "اعداد این دستورها بسته است: هر چیزی جز عدد بفرستید، دستور اجرا نمی شود.",
        ],
    },
    Topic {
        id: "msg",
        icon: "💬",
        title: "پیام و پاسخ",
        intro: "چیزهایی که ربات خودش می نویسد: خوشامد، پاسخ خودکار و اعلان حذف.",
        commands: &[
            cmd("تنظیم خوشامد", "با ریپلای روی یک پیام، آن را خوشامد می کند"),
            cmd("تنظیم خوشامد سلام {نام}", "خوشامد را از متن بعد از دستور می گیرد"),
            cmd("نمایش خوشامد", "خوشامد فعلی را نشان می دهد"),
            cmd("حذف خوشامد", "خوشامد را خاموش می کند"),
            cmd("تنظیم پاسخ سلام", "با ریپلای، پاسخ خودکار برای «سلام» می سازد"),
            cmd("تنظیم پاسخ سلام = درود", "همان، بدون ریپلای"),
            cmd("حذف پاسخ سلام", "آن پاسخ را بر می دارد"),
            cmd("تنظیم اعلان 15", "اعلان حذف پس از ۱۵ ثانیه پاک شود؛ صفر یعنی هرگز"),
        ],
        extra: None,
        notes: &[
            "تگ های خوشامد: {نام} · {منشن} · {آیدی} · {یوزرنیم} · {گروه}",
            "«خوشامد» و «پاسخ» به تنهایی متن نمی گیرند، چون از یک جمله معمولی قابل تشخیص نیستند. یا عبارت کامل را بنویسید یا ریپلای کنید.",
            "مخاطب پاسخ ها را در پنل می شود به ادمین ها یا کاربران ویژه محدود کرد.",
        ],
    },
    Topic {
        id: "tm",
        icon: "🧹",
        title: "پاکسازی و زمان",
        intro: "کارهایی که سر ساعت یا پس از مدتی خودشان انجام می شوند.",
        commands: &[
            cmd("قفل شب 23 تا 7", "گروه هر شب سر ساعت بسته و باز می شود"),
            cmd("قفل شب 23:30 تا 7:15", "همان، با دقیقه"),
            cmd("قفل شب خاموش", "قفل شب را بر می دارد"),
            cmd("اسلوموشن 30", "فاصله اجباری بین پیام های هر عضو، به ثانیه"),
            cmd("اسلوموشن 0", "اسلوموشن را بر می دارد"),
            cmd("تنظیم گزارش روزانه 21", "خلاصه هر روز سر این ساعت در گروه"),
            cmd("حذف گزارش روزانه", "گزارش روزانه را خاموش می کند"),
        ],
        extra: None,
        notes: &[
            "ساعت ها به وقت تهران است.",
            "اسلوموشن را فقط کلینر می تواند تنظیم کند؛ ربات ها به این بخش تلگرام دسترسی ندارند.",
            "رسانه موقت و پاکسازی خودکار فقط از پنل تنظیم می شوند.",
        ],
    },
    Topic {
        id: "tmed",
        icon: "⏳",
        title: "رسانه موقت",
        intro: "عکس و فیلم و استیکر و باقی رسانه ها پس از مدتی خودشان پاک می شوند. گروهی که رسانه انبار نمی کند، چیز کمتری برای گزارش شدن دارد.",
        commands: &[],
        extra: Some(temp_media_kinds),
        notes: &[
            "این بخش دستور متنی ندارد و فقط از پنل تنظیم می شود.",
            "می شود انتخاب کرد که شامل همه بشود یا فقط کسانی که مقامی ندارند.",
        ],
    },
    Topic {
        id: "ls",
        icon: "📋",
        title: "لیست ها",
        intro: "هر لیست را هم می شود دید و هم تک تک از داخلش حذف کرد. دکمه پاکسازی کل لیست را با یک تایید خالی می کند.",
        commands: &[
            cmd("لیست بن", "بن شده های گروه"),
            cmd("لیست سکوت", "سکوت شده ها"),
            cmd("لیست ویژه", "کاربران ویژه"),
            cmd("لیست فیلتر", "کلمه های فیلتر شده"),
            cmd("لیست پاسخ", "پاسخ های خودکار"),
            cmd("لیست معاف", "معاف های شرط ورود"),
            cmd("پاکسازی لیست بن", "کل آن لیست را خالی می کند"),
        ],
        extra: None,
        notes: &["لیست سیک و لیست خفه همان لیست بن و لیست سکوت هستند."],
    },
    Topic {
        id: "filter",
        icon: "🔤",
        title: "فیلتر کلمه",
        intro: "پیامی که یکی از این کلمه ها را داشته باشد حذف می شود. مقایسه بدون حساسیت به بزرگی و کوچکی حروف است.",
        commands: &[
            cmd("افزودن فیلتر ممد", "یک کلمه به لیست اضافه می کند"),
            cmd("فیلتر کلمه ممد", "همان"),
            cmd("حذف فیلتر ممد", "کلمه را بر می دارد"),
            cmd("لیست فیلتر", "کل لیست، با حذف تکی"),
        ],
        extra: None,
        notes: &[
            "«فیلتر» به تنهایی کلمه را از متن بعدش نمی گیرد، چون «فیلتر شکن» یک جمله است نه یک دستور. عبارت کامل بنویسید یا ریپلای کنید.",
            "هر کلمه تا ۶۴ حرف، و هر گروه تا ۲۰۰ کلمه.",
        ],
    },
    Topic {
        id: "cl",
        icon: "🧽",
        title: "کلینر",
        intro: "یک حساب کاربری که کارهایی را انجام می دهد که ربات ها اجازه اش را ندارند: پاک کردن تاریخچه قدیمی و همه پیام های یک نفر.",
        commands: &[
            cmd("حذف 99", "۹۹ پیام آخر را پاک می کند"),
            cmd("حذف همه", "کل پیام های گروه، با یک تایید"),
            cmd("حذف پیام", "همه پیام های یک کاربر در این گروه"),
            cmd("پاکسازی عکس", "همه عکس های گروه"),
            cmd("پاکسازی ویدیو", "همه ویدیوها"),
            cmd("پاکسازی ویس", "همه ویس ها"),
            cmd("پاکسازی فایل", "همه فایل ها و استیکرها"),
            cmd("پاکسازی لینک", "همه پیام های دارای لینک"),
            cmd("افزودن کلینر", "کلینر را به گروه می آورد و ادمین می کند"),
        ],
        extra: None,
        notes: &[
            "نوع های دیگر پاکسازی: مدیا · گیف · موزیک · مکان · مخاطب · نظرسنجی · ویدیو پیام",
            "بدون کلینر، «حذف همه» فقط تا ۱۰ هزار پیام آخر را می گیرد و «حذف پیام» کار نمی کند.",
            "ورود کلینر از پیوی ربات و فقط توسط مالک ربات انجام می شود.",
            "هر بار پاکسازی تا ۳۰۰۰ پیام؛ برای بقیه دوباره بفرستید.",
        ],
    },
    Topic {
        id: "st",
        icon: "📊",
        title: "آمار",
        intro: "شمارش پیام ها در حافظه انجام می شود و روی تایمر ذخیره می شود، پس هیچ کدام از این ها روی سرعت گروه اثر ندارد.",
        commands: &[
            cmd("امار", "داشبورد گروه: برترین ها، ساعت ها، نوع پیام، اعضا"),
            cmd("اطلاعات", "کارت یک کاربر، با ریپلای یا یوزرنیم"),
            cmd("آیدی @ali", "همان کارت"),
            cmd("تنظیم گزارش روزانه 21", "خلاصه هر روز سر ساعت ۲۱"),
            cmd("حذف گزارش روزانه", "گزارش روزانه را خاموش می کند"),
        ],
        extra: None,
        notes: &[
            "«امار» و «اطلاعات» را همه می توانند بفرستند.",
            "مقام خودکار در ۱۰۰، ۵۰۰، ۱۰۰۰، ۵۰۰۰ و ۱۰۰۰۰ پیام داده می شود و از پنل روشن می شود.",
        ],
    },
    Topic {
        id: "gr",
        icon: "⚙️",
        title: "اختیارات گروه",
        intro: "این ها اختیار اعضای عادی است و ادمین ها شامل آن نمی شوند. با هر تغییر، کل مجموعه دوباره روی گروه نوشته می شود.",
        commands: &[
            cmd("اختیارات گروه", "وضعیت همه اختیارات"),
            cmd("اختیار عکس بسته", "یک اختیار را می بندد"),
            cmd("اختیار عکس باز", "همان را باز می کند"),
        ],
        extra: Some(rights_names),
        notes: &[
            "به جای بسته و باز می شود قفل، خاموش، آزاد یا روشن هم نوشت.",
            "«قفل گروه» همه این ها را با هم می بندد.",
        ],
    },
    Topic {
        id: "lg",
        icon: "📝",
        title: "کانال لاگ",
        intro: "کارهایی که ربات انجام می دهد در یک کانال نوشته می شود. پیام ها دسته ای فرستاده می شوند، پس یک طوفان حذف یک پیام می شود نه صدتا.",
        commands: &[
            cmd("تنظیم لاگ @channel", "کانال لاگ را ثبت می کند"),
            cmd("لاگ", "وضعیت فعلی کانال لاگ"),
            cmd("حذف لاگ", "کانال لاگ را بر می دارد"),
        ],
        extra: Some(log_kinds),
        notes: &[
            "اول ربات را در کانال ادمین کنید.",
            "می شود یک پیام از کانال را در گروه فوروارد کرد و روی آن «تنظیم لاگ» زد.",
            "انتخاب اینکه چه رویدادهایی نوشته شود در پنل › کانال لاگ است.",
        ],
    },
    Topic {
        id: "lim",
        icon: "⚖️",
        title: "محدودیت مدیران",
        intro: "ادمین بودن یک چیز است و اینکه به کدام دستورها دسترسی داشته باشد چیز دیگری. مالک می تواند هر کدام را جدا ببندد.",
        commands: &[],
        extra: Some(capability_names),
        notes: &[
            "این بخش دستور متنی ندارد و فقط از پنل تنظیم می شود.",
            "روی مالک ربات اعمال نمی شود و تنظیمش هم فقط از خود اوست.",
            "دکمه های پنل هم مثل دستورها بسته می شوند، پس از راه پنل نمی شود دورش زد.",
        ],
    },
    Topic {
        id: "s",
        icon: "🔥",
        title: "حالت سختگیرانه",
        intro: "به طور عادی پیامی که قفل بگیرد فقط پاک می شود. با این حالت، فرستنده اش هم پس از چند تخلف تنبیه می شود.",
        commands: &[],
        extra: None,
        notes: &[
            "این بخش دستور متنی ندارد و فقط از پنل تنظیم می شود.",
            "در «موارد تخلف» انتخاب می کنید کدام قفل ها تخلف حساب شوند.",
            "شمارش تخلف پس از هفت روز بی تخلفی از نو شروع می شود.",
        ],
    },
    Topic {
        id: "fl",
        icon: "⚡",
        title: "ضد رگبار",
        intro: "کسی که در یک بازه کوتاه بیش از حد پیام بفرستد، سکوت یا بن می شود. شمارش برای هر کاربر جداست و ادمین ها شامل آن نمی شوند.",
        commands: &[
            cmd("ضد رگبار 10 5", "بیش از ۱۰ پیام در ۵ ثانیه"),
            cmd("تنظیم رگبار 10 5", "همان"),
        ],
        extra: None,
        notes: &["خاموش کردنش از دکمه بالای همین صفحه است."],
    },
    Topic {
        id: "rd",
        icon: "🚨",
        title: "ضد هجوم",
        intro: "ورود ناگهانی چند عضو در یک بازه کوتاه یعنی هجوم؛ تازه واردها تا مدتی سکوت می شوند تا فرصت بررسی باشد.",
        commands: &[],
        extra: None,
        notes: &[
            "این بخش دستور متنی ندارد و فقط از همین صفحه تنظیم می شود.",
            "ادمین می تواند با «حذف سکوت» زودتر آزادشان کند.",
        ],
    },
    Topic {
        id: "bt",
        icon: "🕵",
        title: "ضد خیانت ادمین",
        intro: "ادمینی که در مدت کوتاهی چند نفر را حذف کند، خودش عزل می شود. مالک ربات شامل آن نمی شود.",
        commands: &[cmd("تنظیم خیانت 5 10", "۵ حذف در ۱۰ دقیقه")],
        extra: None,
        notes: &["می شود انتخاب کرد که فقط عزل شود یا عزل و بن."],
    },
    Topic {
        id: "cp",
        icon: "🧩",
        title: "احراز هویت",
        intro: "عضو تازه تا وقتی ایموجی درست را از روی تصویر نزند ساکت می ماند. تصویر هر ایموجی یک بار ساخته می شود و بعد از آن از حافظه تلگرام می آید.",
        commands: &[cmd("تنظیم احراز 120", "مهلت پاسخ، به ثانیه")],
        extra: None,
        notes: &["پس از پایان مهلت یا سکوت می ماند یا اخراج می شود؛ انتخابش در همین صفحه است."],
    },
    Topic {
        id: "wn",
        icon: "⚠️",
        title: "اخطار",
        intro: "اخطارها روی هر کاربر جمع می شوند و در سقف تعیین شده کار مشخص شده انجام می شود.",
        commands: &[
            cmd("اخطار", "یک اخطار، با ریپلای یا یوزرنیم"),
            cmd("حذف اخطار", "یک اخطار کم می کند"),
            cmd("اخطارها", "اخطارهای یک کاربر"),
            cmd("تنظیم اخطار 5", "سقف اخطار"),
        ],
        extra: None,
        notes: &["«وارن» هم همان اخطار است."],
    },
    Topic {
        id: "jn",
        icon: "🚪",
        title: "عضویت اجباری",
        intro: "تا وقتی کاربر عضو کانال نشده باشد پیامش پاک می شود و یک اعلان با دکمه عضویت می گیرد.",
        commands: &[
            cmd("تنظیم عضویت اجباری @channel", "کانال شرط را ثبت می کند"),
            cmd("عضویت اجباری", "وضعیت فعلی"),
            cmd("حذف عضویت اجباری", "شرط را بر می دارد"),
            cmd("معاف", "یک کاربر را معاف می کند"),
        ],
        extra: None,
        notes: &["ربات باید در آن کانال ادمین باشد تا بتواند عضویت را ببیند."],
    },
    Topic {
        id: "ad",
        icon: "➕",
        title: "اد اجباری",
        intro: "تا وقتی کاربر تعداد مشخصی عضو اضافه نکرده باشد اجازه نوشتن ندارد. شمارش از روی همان کسانی است که خودش اضافه کرده.",
        commands: &[
            cmd("تنظیم اد اجباری 5", "پنج نفر"),
            cmd("اد اجباری", "وضعیت فعلی"),
            cmd("حذف اد اجباری", "شرط را بر می دارد"),
            cmd("معاف", "یک کاربر را معاف می کند"),
        ],
        extra: None,
        notes: &["عدد صفر یعنی خاموش."],
    },
    Topic {
        id: "gp",
        icon: "🔔",
        title: "اعلان شرط",
        intro: "اعلانی که به کسی که هنوز شرط ورود را انجام نداده نشان داده می شود. هم فاصله بین اعلان ها و هم حذف خودکارشان قابل تنظیم است.",
        commands: &[cmd("تنظیم اعلان شرط 120 30", "هر ۱۲۰ ثانیه یک بار، حذف پس از ۳۰ ثانیه")],
        extra: None,
        notes: &["فاصله صفر یعنی هر بار، و حذف صفر یعنی اعلان پاک نشود."],
    },
    Topic {
        id: "nt",
        icon: "🔕",
        title: "اعلان حذف",
        intro: "وقتی قفلی پیامی را حذف می کند، ربات یک اعلان کوتاه می گذارد تا فرستنده بداند چرا. اعلان خودش هم پس از مدتی پاک می شود.",
        commands: &[cmd("تنظیم اعلان 15", "پاک شدن پس از ۱۵ ثانیه")],
        extra: None,
        notes: &["صفر یعنی اعلان هرگز پاک نشود."],
    },
    Topic {
        id: "an",
        icon: "💡",
        title: "پاسخ خودکار",
        intro: "پیامی که متنش دقیقا برابر یک عبارت ذخیره شده باشد، پاسخ آماده اش را می گیرد. پاسخ می تواند رسانه هم داشته باشد.",
        commands: &[
            cmd("تنظیم پاسخ سلام", "با ریپلای روی پیام پاسخ"),
            cmd("تنظیم پاسخ سلام = درود", "بدون ریپلای"),
            cmd("حذف پاسخ سلام", "آن پاسخ را بر می دارد"),
            cmd("لیست پاسخ", "همه پاسخ ها، با حذف تکی"),
        ],
        extra: None,
        notes: &[
            "«پاسخ» به تنهایی متن نمی گیرد؛ عبارت کامل بنویسید یا ریپلای کنید.",
            "مخاطبش را می شود به ادمین ها یا کاربران ویژه محدود کرد.",
        ],
    },
    Topic {
        id: "wc",
        icon: "👋",
        title: "خوشامد",
        intro: "پیامی که برای هر عضو تازه فرستاده می شود. می تواند متن، رسانه یا هر دو باشد.",
        commands: &[
            cmd("تنظیم خوشامد", "با ریپلای روی پیام دلخواه"),
            cmd("تنظیم خوشامد سلام {نام}", "از متن بعد از دستور"),
            cmd("نمایش خوشامد", "خوشامد فعلی"),
            cmd("حذف خوشامد", "خاموشش می کند"),
        ],
        extra: None,
        notes: &[
            "تگ ها: {نام} · {منشن} · {آیدی} · {یوزرنیم} · {گروه}",
            "«خوشامد» به تنهایی متن نمی گیرد، چون «خوشامد گفتم» یک جمله است.",
        ],
    },
    Topic {
        id: "ng",
        icon: "🌙",
        title: "قفل شب",
        intro: "گروه هر شب سر ساعت مشخص بسته و صبح باز می شود. ساعت ها به وقت تهران است.",
        commands: &[
            cmd("قفل شب 23 تا 7", "از ۲۳ تا ۷"),
            cmd("قفل شب 23:30 تا 7:15", "با دقیقه"),
            cmd("قفل شب خاموش", "بر می دارد"),
        ],
        extra: None,
        notes: &["بستن و باز کردن از راه اختیارات گروه انجام می شود، پس ادمین ها همچنان می نویسند."],
    },
    Topic {
        id: "sl",
        icon: "🐢",
        title: "اسلوموشن",
        intro: "فاصله اجباری بین دو پیام هر عضو. این تنظیم خود تلگرام است و ادمین ها شامل آن نمی شوند.",
        commands: &[
            cmd("اسلوموشن 30", "هر ۳۰ ثانیه یک پیام"),
            cmd("اسلوموشن 0", "بر می دارد"),
        ],
        extra: None,
        notes: &[
            "تلگرام فقط ۱۰، ۳۰، ۶۰، ۳۰۰، ۹۰۰ و ۳۶۰۰ ثانیه را قبول می کند؛ عدد دیگر به نزدیک ترین پایین تر می رود.",
            "این کار را فقط کلینر می تواند انجام دهد؛ ربات ها به این بخش تلگرام دسترسی ندارند.",
        ],
    },
    Topic {
        id: "ap",
        icon: "🗑",
        title: "پاکسازی خودکار",
        intro: "هر روز سر ساعت مشخص، تعدادی از پیام های گروه پاک می شوند.",
        commands: &[],
        extra: None,
        notes: &[
            "این بخش دستور متنی ندارد و فقط از همین صفحه تنظیم می شود.",
            "تعداد صفر یعنی همه پیام ها؛ برای پاک شدن کامل تاریخچه کلینر لازم است.",
        ],
    },
    Topic {
        id: "dr",
        icon: "📅",
        title: "گزارش روزانه",
        intro: "خلاصه یک روز گروه، هر شب سر ساعت مشخص در همان گروه فرستاده می شود.",
        commands: &[
            cmd("تنظیم گزارش روزانه 21", "هر روز ساعت ۲۱"),
            cmd("گزارش روزانه 21:30", "با دقیقه"),
            cmd("حذف گزارش روزانه", "خاموشش می کند"),
        ],
        extra: None,
        notes: &["اگر فرستادن گزارش یک شب انجام نشود، تیک بعدی دوباره تلاش می کند."],
    },
    Topic {
        id: "sp",
        icon: "🎯",
        title: "موارد تخلف",
        intro: "انتخاب اینکه کدام قفل ها در حالت سختگیرانه تخلف حساب شوند. باقی قفل ها فقط پیام را پاک می کنند.",
        commands: &[],
        extra: None,
        notes: &[
            "این بخش دستور متنی ندارد و فقط از همین صفحه تنظیم می شود.",
            "فیلتر کلمه و پک استیکر هم می توانند تخلف حساب شوند.",
        ],
    },
    Topic {
        id: "etc",
        icon: "✨",
        title: "باقی دستورها",
        intro: "چیزهایی که جای دیگری نمی گنجند.",
        commands: &[
            cmd("پنل", "منوی شیشه ای همه تنظیمات"),
            cmd("پنل پیوی", "همان پنل، در پیوی خودتان"),
            cmd("راهنما", "همین فهرست"),
            cmd("پینگ", "سرعت پاسخ ربات و دیتابیس"),
            cmd("گزارش", "با ریپلای، پیام را به ادمین ها گزارش می کند"),
            cmd("قوانین", "قوانین گروه"),
            cmd("تنظیم قوانین", "با ریپلای یا متن بعد از دستور"),
            cmd("یادداشت", "با ریپلای، یادداشت ادمین ها روی یک کاربر"),
            cmd("حذف یادداشت", "یادداشت را بر می دارد"),
            cmd("سنجاق", "با ریپلای، پیام را سنجاق می کند"),
            cmd("سنجاق بی صدا", "سنجاق بدون اعلان"),
            cmd("حذف سنجاق", "سنجاق را بر می دارد"),
            cmd("تگ همه", "اعضا را دسته دسته صدا می زند"),
            cmd("تگ 30 بیاید", "۳۰ نفر، با این متن"),
        ],
        extra: None,
        notes: &[
            "«قوانین»، «گزارش» و «پینگ» را همه می توانند بفرستند.",
            "«یادداشت»، «سنجاق» و «گزارش» فقط با ریپلای کار می کنند و یوزرنیم قبول نمی کنند.",
            "«تگ» به تنهایی باید تعداد بگیرد؛ «تگ همه» خودش می داند.",
        ],
    },
];

fn lock_names() -> String {
    const GROUPS: &[(&str, &[&str])] = &[
        ("رسانه", &["photo", "video", "gif", "music", "voice", "file", "sticker", "animsticker", "media"]),
        ("متن", &["links", "hyperlink", "hashtag", "username", "mention", "english", "persian", "commands", "botcall"]),
        ("ظاهر", &["emoji", "premoji", "spoiler"]),
        ("فوروارد", &["forward_channel", "forward_user"]),
        ("تعاملی", &["poll", "dice", "contact", "location", "button", "story"]),
        ("هویت", &["anon", "bot", "promoter"]),
        ("رویداد", &["edit", "pin", "service"]),
    ];

    fn name_of(key: &'static str) -> &'static str {
        locks::LOCKS
            .iter()
            .find(|lock| lock.key == key)
            .map(|lock| lock.names[0])
            .unwrap_or(key)
    }
    let mut out = format!("<b>قفل ها</b> ({})\n", fa(locks::LOCKS.len()));
    for (group, keys) in GROUPS {
        let names: Vec<&str> = keys.iter().map(|key| name_of(key)).collect();
        out.push_str(&format!("‹ <b>{group}</b> · {}\n", names.join(" · ")));
    }
    out
}

fn listed(heading: &str, names: &[&str]) -> String {
    format!(
        "<b>{heading}</b> ({})\n‹ {}\n",
        fa(names.len()),
        names.join(" · ")
    )
}

fn rights_names() -> String {
    let names: Vec<&str> = rights::RIGHTS.iter().map(|right| right.label).collect();
    listed("اختیارها", &names)
}

fn capability_names() -> String {
    let names: Vec<&str> = limits::CAPS.iter().map(|cap| cap.label).collect();
    listed("دسترسی ها", &names)
}

fn temp_media_kinds() -> String {
    let names: Vec<&str> = tempmedia::KINDS.iter().map(|kind| kind.label).collect();
    listed("نوع ها", &names)
}

fn log_kinds() -> String {
    let names: Vec<&str> = log::KINDS.iter().map(|(_, label)| *label).collect();
    listed("رویدادها", &names)
}

pub fn find(id: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|topic| topic.id == id)
}

fn fa(number: usize) -> String {
    number
        .to_string()
        .chars()
        .map(|c| match c {
            '0'..='9' => char::from_u32('۰' as u32 + (c as u32 - '0' as u32)).unwrap_or(c),
            other => other,
        })
        .collect()
}

pub fn page(topic: &Topic) -> String {
    let mut out = format!(
        "{} <b>راهنما</b> › <b>{}</b>\n\n{}",
        topic.icon, topic.title, topic.intro
    );

    if !topic.commands.is_empty() {
        out.push_str(&format!("\n\n<b>دستورها</b> ({})\n", fa(topic.commands.len())));
        for command in topic.commands {
            out.push_str(&format!("<code>{}</code> · {}\n", command.form, command.does));
        }
        out.pop();
    }
    if let Some(extra) = topic.extra {
        out.push_str("\n\n");
        out.push_str(extra().trim_end());
    }
    if !topic.notes.is_empty() {
        out.push('\n');
        for note in topic.notes {
            out.push_str(&format!("\n<i>‹ {note}</i>"));
        }
    }
    out
}

pub fn index() -> String {
    let commands: usize = TOPICS.iter().map(|topic| topic.commands.len()).sum();
    format!(
        "<b>راهنما</b>\n\n\
         بخش ها · <b>{}</b>\n\
         دستورها · <b>{}</b>\n\
         قفل ها · <b>{}</b>\n\n\
         یک بخش را بزنید تا دستورهایش را ببینید.\n\
         هر دستور تک عرض نوشته شده و با یک تپ کپی می شود.\n\n\
         <i>هدف هر دستور: ریپلای، یوزرنیم یا آیدی عددی</i>",
        fa(INDEX.len()),
        fa(commands),
        fa(locks::LOCKS.len()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias_groups() -> Vec<(&'static str, Vec<&'static str>)> {
        let owned = |name: &'static str, aliases: &[&'static str]| (name, aliases.to_vec());
        vec![
            owned("locks::STATUS", locks::STATUS),
            (
                "restrict::COMMANDS",
                super::super::restrict::COMMANDS
                    .iter()
                    .map(|(alias, _)| *alias)
                    .collect(),
            ),
            owned("warns::WARN", super::super::warns::WARN),
            owned("warns::UNWARN", super::super::warns::UNWARN),
            owned("warns::SHOW", super::super::warns::SHOW),
            owned("filters::ADD", super::super::filters::ADD),
            owned("filters::REMOVE", super::super::filters::REMOVE),
            owned("packs::LOCK", super::super::packs::LOCK),
            owned("packs::UNLOCK", super::super::packs::UNLOCK),
            owned("answers::ADD", super::super::answers::ADD),
            owned("answers::REMOVE", super::super::answers::REMOVE),
            owned("welcome::SET", super::super::welcome::SET),
            owned("welcome::CLEAR", super::super::welcome::CLEAR),
            owned("welcome::SHOW", super::super::welcome::SHOW),
            owned("join::SET", super::super::join::SET),
            owned("join::CLEAR", super::super::join::CLEAR),
            owned("join::SET_ADD", super::super::join::SET_ADD),
            owned("join::CLEAR_ADD", super::super::join::CLEAR_ADD),
            owned("join::SET_PROMPT", super::super::join::SET_PROMPT),
            owned("join::FREE_ADD", super::super::join::FREE_ADD),
            owned("join::FREE_REMOVE", super::super::join::FREE_REMOVE),
            owned("flood::COMMANDS", super::super::flood::COMMANDS),
            owned("vip::ADD", super::super::vip::ADD),
            owned("vip::REMOVE", super::super::vip::REMOVE),
            owned("bots::ALLOW", super::super::bots::ALLOW),
            owned("bots::DISALLOW", super::super::bots::DISALLOW),
            owned("promote::COMMANDS", super::super::promote::COMMANDS),
            owned("promote::DEMOTE", super::super::promote::DEMOTE),
            owned("promote::TAG", super::super::promote::TAG),
            owned("promote::TAG_CLEAR", super::super::promote::TAG_CLEAR),
            owned("config::CONFIG", super::super::config::CONFIG),
            owned("config::ADMIN_LIST", super::super::config::ADMIN_LIST),
            owned("config::PROMOTE", super::super::config::PROMOTE),
            owned("config::DEMOTE", super::super::config::DEMOTE),
            owned("config::HELP", super::super::config::HELP),
            owned("panel::OPEN", super::super::panel::OPEN),
            owned("panel::TO_PRIVATE", super::super::panel::TO_PRIVATE),
            owned("ping::COMMANDS", super::super::ping::COMMANDS),
            owned("report::COMMANDS", super::super::report::COMMANDS),
            owned("purge::COMMANDS", super::super::purge::COMMANDS),
            owned("purge::ALL", super::super::purge::ALL),
            owned("cleaner::ADD", super::super::cleaner::ADD),
            owned("cleaner::WIPE", super::super::cleaner::WIPE),
            owned("cleaner::SWEEP", super::super::cleaner::SWEEP),
            owned("log::SET", super::super::log::SET),
            owned("log::CLEAR", super::super::log::CLEAR),
            owned("rights::SHOW", super::super::rights::SHOW),
            owned("rights::SET", super::super::rights::SET),
            (
                "lists::SHOW",
                super::super::lists::SHOW.iter().map(|(a, _)| *a).collect(),
            ),
            (
                "lists::CLEAR",
                super::super::lists::CLEAR.iter().map(|(a, _)| *a).collect(),
            ),
            owned("extras::SHOW_RULES", super::super::extras::SHOW_RULES),
            owned("extras::SET_RULES", super::super::extras::SET_RULES),
            owned("extras::NOTE_CMD", super::super::extras::NOTE_CMD),
            owned("extras::NOTE_CLEAR", super::super::extras::NOTE_CLEAR),
            owned("extras::PIN", super::super::extras::PIN),
            owned("extras::PIN_QUIET", super::super::extras::PIN_QUIET),
            owned("extras::UNPIN", super::super::extras::UNPIN),
            owned("extras::SLOW", super::super::extras::SLOW),
            owned("extras::NIGHT_CMD", super::super::extras::NIGHT_CMD),
            owned("extras::TAG_ALL", super::super::extras::TAG_ALL),
            owned("stats::STATS", super::super::stats::STATS),
            owned("stats::REPORT_SET", super::super::stats::REPORT_SET),
            owned("stats::REPORT_CLEAR", super::super::stats::REPORT_CLEAR),
            owned("stats::INFO", super::super::stats::INFO),
            owned("tune::COMMANDS", super::super::tune::COMMANDS),
        ]
    }

    fn every_form() -> Vec<&'static str> {
        TOPICS
            .iter()
            .flat_map(|topic| topic.commands.iter().map(|command| command.form))
            .collect()
    }

    #[test]
    fn every_documented_command_exists() {
        let lock_verbs = ["قفل ", "بازکردن "];
        for form in every_form() {
            let matched = alias_groups()
                .iter()
                .any(|(_, aliases)| aliases.iter().any(|alias| form.starts_with(alias)))
                || lock_verbs.iter().any(|verb| form.starts_with(verb))
                || super::super::tune::SETTINGS
                    .iter()
                    .any(|(name, _)| form.starts_with("تنظیم ") && form.contains(name));
            assert!(matched, "«{form}» is documented but no handler matches it");
        }
    }

    #[test]
    fn every_command_is_documented() {
        let forms = every_form();
        for (table, aliases) in alias_groups() {
            let covered = aliases
                .iter()
                .any(|alias| forms.iter().any(|form| form.starts_with(alias)));
            assert!(covered, "no help entry covers any alias of {table}");
        }
    }

    #[test]
    fn every_page_fits_one_message() {
        for topic in TOPICS {
            let rendered = page(topic);
            assert!(
                rendered.chars().count() < 3_500,
                "«{}» renders {} characters",
                topic.title,
                rendered.chars().count()
            );
        }
        assert!(index().chars().count() < 3_500);
    }

    #[test]
    fn the_content_carries_no_markup() {
        for topic in TOPICS {
            assert!(!topic.icon.is_empty(), "«{}» has no icon", topic.id);
            assert!(
                topic.icon.chars().count() <= 2,
                "«{}» carries more than one icon",
                topic.id
            );
            assert!(
                !topic.icon.is_ascii(),
                "«{}» has an ascii icon, not an emoji",
                topic.id
            );

            let mut all = vec![topic.title, topic.intro];
            all.extend(topic.notes);
            all.extend(topic.commands.iter().flat_map(|c| [c.form, c.does]));
            for text in all {
                assert!(!text.contains('<'), "markup in «{text}»");
                assert!(!text.contains('&'), "entity in «{text}»");
                assert!(!text.contains('\u{200c}'), "zero width non joiner in «{text}»");
            }
        }
    }

    #[test]
    fn every_topic_is_reachable_and_named_once() {
        let mut ids: Vec<&str> = TOPICS.iter().map(|topic| topic.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "two topics share an id");

        for id in INDEX {
            assert!(find(id).is_some(), "the index offers «{id}», which has no topic");
        }
    }

    #[test]
    fn every_topic_can_be_reached() {
        for topic in TOPICS {
            let reachable =
                INDEX.contains(&topic.id) || super::super::panel::is_page(topic.id);
            assert!(
                reachable,
                "«{}» is on no panel page and not in the index",
                topic.id
            );
        }
    }

    #[test]
    fn payloads_fit_telegram() {
        for topic in TOPICS {
            let longest = format!("h:{}:{}:{}", i64::MAX, i64::MIN, topic.id);
            assert!(longest.len() <= 64, "payload too long for {}", topic.id);
            assert!(!topic.id.contains(':'), "{} has a colon in its id", topic.id);
        }
    }

    #[test]
    fn the_locks_page_shows_every_lock() {
        let page = lock_names();
        for lock in locks::LOCKS {
            assert!(
                page.contains(lock.names[0]),
                "«{}» is in LOCKS but not on the help page",
                lock.names[0]
            );
        }
    }
}
