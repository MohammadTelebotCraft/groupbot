<div align="center">

# groupbot

**ربات مدیریت گروه تلگرام — با Rust روی MTProto**

<br>

![Rust](https://img.shields.io/badge/Rust-2024%20edition-000000?style=flat-square&logo=rust&logoColor=white)
![MTProto](https://img.shields.io/badge/MTProto-grammers-2AABEE?style=flat-square&logo=telegram&logoColor=white)
![PostgreSQL](https://img.shields.io/badge/PostgreSQL-4169E1?style=flat-square&logo=postgresql&logoColor=white)
![Tokio](https://img.shields.io/badge/Tokio-multi--thread-1A1A1A?style=flat-square)

<br>

یه پروسه، هزارتا گروه، ۳۶ تا قفل و یه پنل شیشه ای کامل فارسی.

</div>


---

<div dir="rtl">

## این چیه

یه ربات مدیریت گروه تلگرامه که از اول واسه scale نوشته شده. فرضش اینه که ده ها هزار گروه شلوغ
داری و **یه پروسه** باید همه شون رو بچرخونه، نه یه گروه تستی ۲۰ نفره.

دستورا همه فارسین و prefix ندارن — `قفل عکس` می زنی، نه `/lock photo`. هر چیزی که با دستور
تنظیم می شه، با یه تپ رو پنل هم هست و برعکس.

سه تا تصمیم اصلی که فرقش رو با یه ربات معمولی می سازه:

| تصمیم | چی گیرت میاد |
|---|---|
| **MTProto به جای Bot API** | دکمه رنگی، حذف پیام قدیمی تر از ۴۸ ساعت، آپدیت خام participant |
| **کش کامل تنظیمات تو مموری** | چک کردن همه قفلا واسه هر پیام، بدون حتی یه کوئری |
| **دو تا session تو یه پروسه** | اکانت ربات + یه اکانت یوزر («کلینر») واسه کارایی که ربات اجازه نداره |

---

## چرا MTProto و نه Bot API

Bot API فقط یه wrapper ساده روی MTProto هست و دقیقا همون چیزایی رو حذف کرده که این ربات لازم داره:

- **حذف پیام قدیمی** — تلگرام نمی ذاره ربات پیام قدیمی تر از ۴۸ ساعت رو پاک کنه، ولی اکانت یوزر
  این محدودیتو نداره. `حذف 99` رو history قدیمی هم کار می کنه.
- **دکمه رنگی** — فیلد `keyboardButtonStyle` فقط تو MTProto هست. رنگ اینجا state رو نشون می ده،
  نه تزیین: سبز یعنی روشن، آبی یعنی انتخاب شده.
- **آپدیت خام** — ادمین شدن و عزل شدن اصلا به شکل پیام نمیاد، `ChannelParticipant` میاد. ضد
  خیانت ادمین و invalidate شدن کش ادمینا هر دو رو همینا سوارن.
- **کنترل رو شبکه** — سایز sender pool، ظرفیت channel آپدیتا و recover کردن gap. زیر بار سنگین
  فرق بین «کار می کنه» و «پیام گم می شه» همینه.

---

## امکانات

### قفل ها

۳۶ تا قفل، هر کدوم یه ردیف تو `LOCKS`. چک کردن همه شون واسه هر پیام فقط خوندن از مموریه.

| دسته | قفل ها |
|---|---|
| مدیا | عکس · ویدیو · گیف · موزیک · ویس · فایل · مدیا · استیکر · استیکر متحرک |
| متن | لینک · لینک مخفی · هشتگ · یوزرنیم · تگ · انگلیسی · فارسی · دستورات عمومی · دستور ربات |
| ایموجی | ایموجی · ایموجی پرمیوم · اسپویلر |
| فوروارد | فوروارد از کانال · فوروارد از کاربر |
| تعاملی | نظرسنجی · تاس · مخاطب · لوکیشن · دکمه شیشه ای · استوری |
| هویت | ناشناس · ربات · تبچی · لینک در بایو |
| ایونت | ویرایش · سنجاق · پیام سرویس |

جداشون: **فیلتر کلمه** و **قفل پک استیکر** — رو یه استیکر ریپلای می کنی، کل پک بن می شه.

### نگهبان تصویر

اینا از قفل ها جدان و تو پنل بخش خودشونو دارن. فرقشون اینه که یه قفل معمولی از رو خود
پیام تصمیم می گیره و مجانیه، ولی اینا تصویرو دانلود می کنن و می دن به مدل.

| مورد | مدل | کارش |
|---|---|---|
| **غیراخلاقی** | Marqo ViT-tiny + MobileNetV4 + SigLIP 2 | با یک کلید روشن/خاموش به عکس و گیف و استیکر و کاور ویدیو نگاه می کند و محتوای مستهجن را حذف می کند؛ تنظیم امتیاز یا برچسب لازم نیست. مدل عمومی برای کم کردن خطای عکس های معمولی حق وتو دارد |
| **موضوعی** | SigLIP 2 | سیگار · مشروب · اسلحه · قمار · مواد · خون |
| **تبلیغ در تصویر** | PaddleOCR | متنی که رو خود عکس نوشته شده رو می خونه و توش دنبال لینک تلگرام و آیدی کانال و آدرس سایت می گرده |

هر تصویر **یه بار** گرفته و باز می شه، هر چند تاشون که روشن باشه — هر سه مدل رو همون یه
`RgbImage` کار می کنن. جواب با file id کش می شه، پس همون عکس اگه دوباره یا تو یه گروه دیگه
فرستاده شه هزینه ای نداره. تا وقتی هیچ کدوم روشن نشده هیچ عکسی اصلا دانلود نمی شه.

خوندن متن یه cascade ئه: مدل detection رو هر عکس می ره ولی کوچیکه، مدل recognition فقط
رو جاهایی که detection متن پیدا کرده. یه عکس معمولی بدون نوشته فقط پول اولی رو می ده.

`قفل همه` اینا رو روشن نمی کنه، و عمدیه — کسی که همه قفل ها رو می خواد منظورش روشن کردن
شیش تا مدل نیست.

### گیت های ورود

| فیچر | کارش |
|---|---|
| **احراز هویت** | عضو جدید تا ایموجی درستو از رو عکس نزنه mute می مونه |
| **عضویت اجباری** | تا عضو کانال نشه پیامش پاک می شه |
| **اد اجباری** | تا چند نفر add نکنه اجازه نوشتن نداره |
| **قفل ربات** | ربات که اضافه شه، هم خودش هم اد کننده اش کیک می شن |

عکس کپچا **یه بار** واسه هر ایموجی render می شه، بعدش تلگرام با `file_id` می فرستدش. مسیر
جوین هیچ وقت چیزی render یا آپلود نمی کنه.

### نظارت ویس

از مسیر `پنل ← تنظیمات پیشرفته ← امنیت و ورود`، **نظارت واژه های نامناسب در ویس** را روشن
کنید. فقط voice note بررسی می شود؛ موزیک و فایل صوتی عادی وارد این مسیر نمی شوند.

لیست پیش فرض هر گروه فقط این هشت واژه را دارد و برای هر گروه کاملا قابل شخصی سازی است:

`مادرجنده` · `جنده` · `کونی` · `کس ننت` · `کیری` · `حرومزاده` · `خارکسه` · `کیرم`

مدیر می تواند واژه یا عبارت دلخواه اضافه کند، حتی واژه های پیش فرض را حذف کند، و بعدا با همان
دستور دوباره برگرداند:

- `فیلتر ویس کلمه ...` یا `افزودن کلمه ویس ...`
- `حذف کلمه ویس ...`
- `لیست کلمات ویس`

مدیر می تواند از صفحه **کلمات فیلتر ویس** در همان پنل هم موارد فعال را حذف و پیش فرض های حذف شده
را بازگرداند. این لیست جدا از فیلتر متن گروه است و تغییر آن، کش تشخیص همان voice را هم به شکل
درست invalid می کند. دکمه **متن تشخیص داده شده ویس** متن خام تشخیص داده شده را نشان می دهد و
واژه ها را با `...` یا `•••` پنهان نمی کند.

تشخیص ویس با چند worker پایدار انجام می شود و برای voiceهای کوتاه، FFmpeg فقط یک بار صدا را
به PCM تبدیل می کند؛ تعداد workerها با `VOICE_WORKERS` تنظیم می شود. برای `faster-whisper` مقدار
پیش فرض یک و سقف چهار است، چون هر worker می تواند مدل محلی خودش را load کند. هر مدل ONNX استخر خودش را
دارد، پس مقدار پیش فرض fleet-safe برای تصویر دو session و دو slot است؛ `NSFW_INFER_SESSIONS`،
`NSFW_SLOTS` و `NSFW_INFER_THREADS` را فقط بعد از اندازه گیری RSS کل مدل های همان worker افزایش دهید.
برای حذف سقف سرویس Google، روی worker دارای مدل محلی می توانید `VOICE_BACKEND=faster-whisper`
بگذارید؛ در حالت پیش فرض همان SpeechRecognition استفاده می شود.

### مدیریت کاربر

سکوت و بن (با ریپلای، `@username` یا آیدی) · افزودن ادمین با انتخاب دسترسیا رو صفحه شیشه ای ·
ترفیع و تنزل (فقط ادمین سطح ربات، بدون دست زدن به دسترسی واقعی تلگرام) · اخطار تا سقف ·
کاربر ویژه (معاف از قفلا بدون هیچ قدرتی) · اختیارات گروه · لیست بن و سکوت و ویژه و فیلتر و
معاف و پک های استیکر، همه تو پنل با حذف تکی.

**محدودیت مدیران** هم هست: مالک می تونه هشت تا دسترسی رو جدا جدا ببنده — بن، سکوت، اخطار،
تنظیمات، پاکسازی، معافیت، سنجاق، عضو ویژه. دکمه هاش هم مثل دستوراش بسته می شه، پس از راه پنل
نمی شه دورش زد. رو خود مالک اعمال نمی شه و این بخش فقط واسه خودشه.

### گاردهای خودکار

| گارد | چیکار می کنه |
|---|---|
| **ضد رگبار** | پیام زیادی تو بازه کوتاه → mute یا بن |
| **ضد خیانت ادمین** | ادمینی که تو چند دقیقه چند نفرو کیک کنه، خودش دیموت می شه |
| **حالت سختگیرانه** | محتوای قفل شده بفرستی، غیر از پاک شدن پیام خودتم تنبیه می شی |
| **قفل شب** | گروه سر ساعت خودکار بسته و باز می شه |
| **رسانه موقت** | عکس و فیلم و استیکر و… بعد از یه مدت خودشون پاک می شن، دسته به دسته انتخابی |
| **قفل ربات** | رباتای داخل گروه هم پاک می شن، نه فقط اونایی که تازه اضافه می شن؛ اگه کلینر باشه تا اولین پیامی که یه ربات بفرسته بن می شه. با «ربات مجاز» می شه یکی رو استثنا کرد |

قفل لینک حالا لینک داخل دکمه های شیشه ای رو هم می گیره، نه فقط لینک تو متن. تو گروهی که قفل
ربات خاموشه ولی کلینر هست، پیام رباتا هم از قفلا رد می شه و اگه گیر کنه فقط پاک می شه، بدون بن.

### پنل

`پنل` یه منوی شیشه ایه: قفلا، حالت سختگیرانه، تنظیمات پیشرفته، ضد خیانت، ضد رگبار و لیستا.
`لیست لیست ها` مستقیما منوی لیست ها را باز می کند و لیست پک های استیکر هم از همان جا قابل حذف
تکی یا پاکسازی است.
`پنل پیوی` همینو تو پیوی باز می کنه. تنظیمای عددی preset یه تپی ان، و هر کدوم یه معادل دستوری
هم دارن: `تنظیم اخطار 5` · `ضد رگبار 10 5` · `قفل شب 23 تا 7` · `اسلوموشن 30`

### آمار و لاگ

شمارش پیاما تو مموری انجام می شه و رو تایمر تو Postgres فلاش می شه — داشبورد گروه، برترینا،
کارت کاربر و مقام گرفتن. `کانال لاگ` هم کارای ربات رو تو یه کانال می نویسه، **batch**: یه
طوفان حذف می شه یه پیام تو هر تیک، نه یه پیام واسه هر حذف.

### کلینر

یه اکانت یوزر تو همون پروسه، واسه کارایی که ربات اجازه نداره:

`حذف 99` رو history قدیمی · `حذف همه` (با تایید) · `حذف پیام` (همه پیامای یه نفر) ·
`پاکسازی عکس / ویدیو / ویس / فایل / لینک`

لاگینش از پیوی ربات و توسط sudo انجام می شه. بعد از اون، هر گروهی که «افزودن ادمین جدید» رو به
ربات بده کلینر خودکار میاد توش و ادمین می شه — یه بار، پس اگه ادمینی عمدا درش بیاره دیگه
برنمی گرده و باید `افزودن کلینر` فرستاده شه.

### باقیش

خوشامد با تگ زنده · پاسخ خودکار · قوانین · یادداشت رو کاربر · سنجاق · گزارش به ادمینا · تگ
کردن اعضا · پینگ · اعلان حذف · کانفیگ خودکار (تا دسترسیای ربات کامل شه، سازنده گروه مالک ثبت
می شه، پنج تا قفل پیش فرض روشن می شن و یه خوشامد با حذف خودکار ۱۰ ثانیه نوشته می شه — خوشامد
تنها چیزیه که اگه از قبل داشته باشی دست نمی خوره)

---

## معماری

```
                       ┌──────────────────────────────┐
                       │        Telegram / DC         │
                       └───────────────┬──────────────┘
                                       │ MTProto
                       ┌───────────────┴──────────────┐
                       │   SenderPool (bot + cleaner) │
                       └───────────────┬──────────────┘
                                       │ updates (channel: 4096)
                       ┌───────────────┴──────────────┐
                       │      main.rs — لوپ اصلی       │
                       │  semaphore: 512 آپدیت همزمان  │
                       └───────────────┬──────────────┘
                                       │ یه task واسه هر آپدیت
                       ┌───────────────┴──────────────┐
                       │   handlers::dispatch          │
                       │   زنجیره short-circuit (||)    │
                       └───┬────────────────────┬─────┘
              ┌────────────┴──────┐   ┌─────────┴──────────┐
              │  Ctx (مموری)      │   │  Settings (کش)      │
              │  کش ادمین، رگبار،  │   │  read:  sync        │
              │  کپچا، لاگ، شمارنده│   │  write: async       │
              └───────────────────┘   └─────────┬──────────┘
                                      ┌─────────┴──────────┐
                                      │     PostgreSQL     │
                                      └────────────────────┘
```

**`main.rs`** — session، sender pool و لوپ آپدیت. واسه هر آپدیت یه task جدا spawn می کنه تا یه
ریکوئست کند تو یه گروه، گروه بعدی رو بلاک نکنه. semaphore **قبل از spawn** گرفته می شه (۵۱۲)،
پس وقتی Postgres یا تلگرام کند می شه task بی نهایت تو مموری صف نمی شه و خود update buffer
backpressure می ده. stream کلینر هم همین admission control رو داره. تایمرهای بک گراند قفل شب و
گزارش روزانه رو هر دقیقه، لاگ رو هر ۳ ثانیه و شمارنده ها رو هر ۶۰ ثانیه flush می کنن.

**`state.rs`** — تنظیمات هر چت. Postgres همون source of truth هست ولی **هر ردیف تو مموری هم کش
شده**؛ read بدون `await`، write هم async و تک ردیفی.

**`handlers/`** — یه فایل واسه هر فیچر. هر هندلر `true` برمی گردونه اگه پیامو مصرف کرده باشه و
`dispatch` با `||` زنجیرشون می کنه؛ یعنی تا یکی پیامو برداشت، بقیه اصلا صدا زده نمی شن.

---

## مسیر یه پیام

ترتیب تو `dispatch` رندوم نیست:

```
آپدیت میاد
   ├─ edit؟      → دوباره با قفلا چک می شه
   ├─ callback؟   → یه بار authorize، بعد route با prefix پیلود
   ├─ raw؟        → invalidate کش ادمین، ضد خیانت، لاگ
   └─ پیام جدید
        │
        ├─ ۱. قدیمی تر از ۱۲۰ ثانیه؟ → drop
        │      (ری کانکت پیامای missed رو replay می کنه، یه دستور
        │       از قبل ریستارت نباید بار دوم اجرا شه)
        ├─ ۲. پیویه؟ → معرفی، لیست دستورا، پنل، لاگین کلینر
        ├─ ۳. یاد گرفتن ref چت (یه بار تو هر پروسه)
        ├─ ۴. شمارش آماری — قبل از هرچی که ممکنه پیامو بخوره
        ├─ ۵. ضد رگبار
        ├─ ۶. عددی که پنل منتظرش بود؟
        ├─ ۷. عضویت اجباری — پیام غریبه پاک می شه، دستور باشه یا نباشه
        ├─ ۸. زنجیره هندلرا: کلینر → کانفیگ → قفل ربات → کپچا → خوشامد
        │      → دستورا → قفل ها → پاسخ خودکار
        └─ ۹. قفل سرویس — آخر از همه، تا خوشامد و قفل ربات
               اعلانی که روش کار می کنن رو از دست ندن
```

یه نکته که راحت از قلم می افته: خروجی رباتای دیگه (`via_bot_id`) **به هندلرای دستور نمی رسه**.
جواب اینلاین یه ربات نجواگر با «کاربر عزیز …» شروع می شه که وگرنه مثل دستور `کاربر` پارس
می شد. قفلا ولی سر جاشونن — پیام ربات هم مثل بقیه پاک شدنیه.

---

## دیتابیس

کل state ماندگار **یه تیبل** بیشتر نیست:

```sql
CREATE TABLE settings (
    chat_id BIGINT NOT NULL,
    key     TEXT   NOT NULL,
    value   TEXT   NOT NULL DEFAULT '',
    PRIMARY KEY (chat_id, key)
);
```

| شکل | یعنی |
|---|---|
| ردیف با `value` خالی | یه **flag** — قفل روشنه، کاربر تو لیسته |
| ردیف با مقدار | یه **setting** — `owner`، سقف اخطار، ساعت قفل شب |
| کلید prefix دار | یه **set** — `admin:<id>` · `filter:<word>` · `pack:<id>` · `answer:<key>` |

نبودن ردیف یعنی خاموش، پس «خاموش کردن» فقط یه `DELETE` می شه و تیبل فقط به اندازه چیزایی که واقعا
روشنن بزرگ می شه. سه تا prefix آخری یه ایندکس جدا تو مموری هم دارن، تا گروهی که فیلتر نداره
بابت فیلترا هیچی نده.

---

## چند تا قاعده که نباید بشکنه

- **کار هر پیام باید O(1) و لوکال باشه و allocate نکنه.** هرچی matcher بیشتر از یه بار لازم
  داره، یه بار تو `locks::View` حساب می شه.
- **هیچی که per-message اجرا می شه حق نداره رو تلگرام block شه.** تاخیر قابل دیدن قبل از یه
  delete یعنی باگ.
- **هر کشی باید قاعده purge داشته باشه.** مپی که فقط رشد می کنه memory leakه.
- **«نمی دونم» یعنی «نه» نیست.** لیست ادمینا که نیومد یعنی معلوم نیست، نه اینکه ادمین نیست —
  و ست ادمین خالی **هیچ وقت** کش نمی شه (هر گروه یه سازنده داره، خالی یعنی کوئری fail شده).
- **فرستنده نداشتن یعنی ادمین ناشناس** — پس معافیتش سر جاشه.
- **soft fail.** یه delete ناموفق یه خط لاگ می شه و رد می شه. تو این مقیاس پیام ارور واسه هر
  اتفاق یعنی flood.
- **هر shard یک process است.** کش تنظیمات per-processه؛ برای fleet هر process باید bot token،
  directory و database جدا داشته باشد و هیچ `chat_id` بین shardها مشترک نباشد. دو process روی
  یک token یا یک database بدون `LISTEN/NOTIFY` ممنوع است، چون cache تنظیماتشان invalid نمی شود.

---

## راه اندازی

لازم داری: Rust (edition 2024)، PostgreSQL، `api_id` و `api_hash` از
[my.telegram.org](https://my.telegram.org) و توکن از [@BotFather](https://t.me/BotFather).

`Cargo.toml` با path نسبی به grammers وصله، پس ساختار باید این شکلی باشه:

```
GroupManagement/
├── grammers/
└── groupbot/
```

```sh
# کتابخونه، رو همون کامیتی که باهاش بیلد شده
git clone https://codeberg.org/Lonami/grammers
cd grammers && git checkout 9fef0bae1e59b6138ae7777c783983934a80e129
git apply ../groupbot/patches/grammers.patch
cd ../groupbot

cp .env.example .env      # مقدارای واقعیو بذار
createdb groupbot         # تیبل خودش موقع اولین ران ساخته می شه
./run.sh
```

مدل عمومی (`vision.onnx`، حدود ۳۷۲ مگ) تو ریپو نیست. نسخه پیش فرض `siglip2-base-patch16-384`
است — همه چیز (دو تا tower، بردارهای `concept_vectors` و ثابت های وتو) باید از **همین یه
checkpoint** بیاد؛ یه checkpoint دیگه با همین عرض، فضای دیگه ایه و بی خطا خراب می کنه.
بقیه مدل ها و خود ONNX Runtime با
`include_bytes!` تو باینری ان، ولی این یکی انقدر بزرگه که rustc موقع لینک چند گیگ رم می خواد،
واسه همین از رو دیسک و از **کنار فایل اجرایی** لود می شه. با اسکریپت خودش ساخته می شه:

```sh
pip install torch transformers onnx onnxruntime pillow numpy
python3 tools/export_vision.py --out ./out --check
cp out/vision.onnx target/release/
```

این یه مدل، هر سه کارو می کنه: قفل های موضوعی، «فیلتر تصویری» و وتوی قفل غیراخلاقی. نباشه
هر سه شون خاموش می شن، یه خط تو لاگ می گه و بقیه ربات سر جاشه — **قفل غیراخلاقی هم بدون این
مدل دقیقا مثل قبل کار می کنه**، فقط اون وتو رو نداره.

جاش `clip.onnx` قبلی بود، CLIP ViT-B/32. عوض شد چون هر چیزی که این سه تا فیچر تصمیم می گیرن
یه اختلاف cosine تو همین فضاست، پس دقت خود فضا سقف دقت هر سه تاست. اندازه گیری شده روی همین
باکس: CLIP ViT-B/32 صد و سی و یک میلی ثانیه برای هر عکس، SigLIP 2 صد و سی و هفت — یعنی
عملا هم قیمت، ولی روی ۱۲۰ تصویر تبلیغ شرط بندی و ۵۵۰ عکس معمولی، نرخ خطای قفل قمار در recall
نود درصد از ۷.۱٪ به ۱.۱٪ رسید.

**حساسیت قفل موضوعی هم عوض شد** (پیش فرض ۳۰ به ۲۰)، چون margin تو فضای جدید حدود بیست
هزارم پایین تره. عدد قدیمی رو نگه داشتن یعنی قفل تقریبا هیچی نگیره.

«فیلتر تصویری» — همون قفلی که خود گروه تعریف می کنه — از همین `vision.onnx` استفاده می کنه، پس
راه نمونه («فیلتر این ‹نام›» با ریپلای) با همین یه فایل کار می کنه. فیلتر **متنی** سه تا فایل
دیگه می خواد، که همون اسکریپت با هم می سازتشون:

```sh
cp out/vision_text.onnx out/vision_text_vocab.txt out/vision_text_merges.txt target/release/
```

برج متنی ۴۹۰ مگه. جدول واژه ها int8 ذخیره می شه که مجانیه — جهت یه عبارت حداکثر
۰.۹۹۸۹ cosine جابه جا می شه — ولی `--int8` که matmul ها رو هم int8 می کنه فایلو می رسونه به
۲۲۷ مگ و گرون تره: «سیگار» با cosine ۰.۹۱۵ در می آد، یعنی فیلتر یه جای دیگه رو نشون می ده.
اگه رم سرور کم بود بزنش، وگرنه نه. نباشه فقط «فیلتر متنی» جواب نمی ده و همون یه خط لاگ در می آد.

**فایل های قدیمی (`clip.onnx` و `clip_text*`) دیگه خونده نمی شن.** یه deploy که مدل جدیدو
کنار باینری نذاره، قفل های موضوعی و فیلتر تصویریش خاموش می شن.

**سرِ قفل غیراخلاقی (`nsfw_head`) فایل جدا نداره** — یه بردار ۷۶۸ تایی تو
`src/handlers/nsfw_head_vectors.rs` که با همین embedding کار می کنه و فقط می تونه *اضافه* حذف کنه،
هیچ وقت جلوی حذف رو نمی گیره. از یه corpus روی همین checkpoint یاد گرفته شده، پس با عوض شدن
`vision.onnx` باید دوباره ساخته بشه، همراه بقیه مجموعه:

```sh
../bin/python tools/nsfw_data.py sample                       # corpus (عکس های ترسیمی از Danbooru)
../bin/python tools/vision_embed.py --manifest ~/.cache/nsfw-train/corpus/manifest.tsv \
    --onnx target/release/vision.onnx --out ~/.cache/nsfw-train/emb/corpus_base224.npz
../bin/python tools/train_nsfw_head.py --tower base224 --write   # nsfw_head_vectors.rs + عدد pin
../bin/python tools/nsfw_bench.py --set test --tower base224     # کل cascade، گروه به گروه
```

`train_nsfw_head.py` دو تا عدد `HEAD_SURE` و `HEAD_DELETE` رو هم چاپ می کنه که باید تو
`nsfw.rs` نوشته بشن — روی held-out با صفر خطای مثبت اندازه گرفته شدن و `nsfw_bench.py` روی
مجموعه تست دست نخورده تاییدشون می کنه.

«قفل خرید و فروش» یه مدل جدا داره — `intent.onnx` (حدود ۱۳۰ مگ)، `intent_vocab.txt` و
`intent_frames.txt` — چون برج متنی SigLIP جمله ها رو تو فضای *تصویر* می ذاره و «دیروز گوشی
خریدم» و «گوشی فروشی» رو کنار هم می بینه. این یکی `multilingual-e5-small` است که روی
corpus خود پروژه fine-tune شده تا «قاب» پیام رو بفهمه: آگهی، درخواست خرید، معاوضه، خدمات …
در برابر خرید دیروز، نقل قول یه آگهی تو یه هشدار، انکار، فرض، سوال، خبر، شوخی، حرف درباره
خود قفل. خروجی مدل خود نمره قفله به علاوه قابی که پیام رو توش خونده، و همون قاب تو journal
کنار عدد نوشته می شه. مثل بقیه از کنار فایل اجرایی لود می شه و نباشه فقط همین قفل خاموشه:

```sh
python3 tools/gen_intent_corpus.py                              # corpus با برچسب قاب
python3 tools/finetune_intent.py --model small --out ./out       # حدود ۱۵ دقیقه روی CPU
python3 tools/finetune_intent.py --model base  --out ./out       # لایه دوم، حدود یه ساعت
python3 tools/export_intent.py --finetuned out/small --out ./out --check
python3 tools/export_intent.py --finetuned out/base  --out ./out --check
python3 tools/evaluate_intent.py --files ./out                   # sweep روی eval
python3 tools/intent_battery.py  --files ./out                   # battery، باید بدون FP باشه
cp out/intent.onnx out/intent_big.onnx out/intent_vocab.txt out/intent_frames.txt target/release/
```

`finetune_intent.py` بعد هر epoch مجموعه eval رو (که هیچ وقت train نمی شه) قضاوت می کنه و
epoch با بیشترین recall در صفر خطای مثبت رو نگه می داره؛ دماش (temperature) رو هم طوری
انتخاب می کنه که مرز صفر خطا روی همون عدد پیش فرض حساسیت بیفته، پس عددی که یه گروه ذخیره
کرده همون معنی رو می ده. `intent_battery.tsv` قاضی دومه: چند صد پیام دست نویس از هر شکلی
که یه گروه می فرسته، با نوعش، که نه train می شه نه برای انتخاب استفاده می شه — فقط گزارش.

`intent_big.onnx` (حدود ۴۲۰ مگ) لایه دومه: فقط پیام هایی که مدل کوچیک درباره شون مطمئن نیست
می رن سراغش، پس هزینه ش فقط رو همون باند مبهمه. نباشه فقط همون لایه خاموشه و جواب مدل کوچیک
می مونه. هر دو مدل از یه tokenizer استفاده می کنن، پس `intent_vocab.txt` یکیه؛ فایل قاب ها
هم یکیه چون هر دو روی همون corpus train شدن.

corpus با قالب ها ساخته می شه (هر register یه خانواده قالبه با برچسب قاب) و داور همیشه
`tools/data/intent_eval.tsv` می مونه که هیچوقت تو train نمی ره. یه اشتباه جدید پیدا شد؟ یه
خانواده قالب تو همون قاب و یه خط eval (یا battery) اضافه می شه و دوباره fine-tune — نه جمله
جمله دست نویسی، و نه یه کلمه به یه لیست سیاه.
دستور «فیلتر دقیق» هم به فیلتر کلمه اضافه شده که کلمه رو فقط جدا بشمره، نه داخل کلمه دیگه —
«بت» دیگه «صحبت» رو نمی گیره.

`.env` اینا رو می خواد: `TG_ID` · `TG_HASH` · `TG_BOT_TOKEN` · `DATABASE_URL`، و هیچ کدام
نمی توانند خالی باشند (`TG_ID` هم باید عدد مثبت ۳۲ بیتی باشد). گزینه های اختیاری:
`SUDO_ID` · `LOG` · `CHANNEL` · `SUPPORT` · `SOURCE`.

اون سه تای آخر فقط دکمه های کارت `/start` ان. هر کدوم که ست نشه دکمه اش هم اصلا کشیده
نمی شه، پس یه deploy که هیچ کدومو ست نکنه کارتش دکمه مرده نداره. هم `@name` قبوله هم
`name` هم یک URL کامل HTTP(S). اگر متغیر حاضر ولی خالی/خراب باشد startup متوقف می شود؛ مقدار
خراب دیگر بی صدا دکمه را حذف نمی کند. تو `.gitignore` هست و **هیچ وقت نباید کامیت شه**.

بعدش: ربات رو ادمین گروه کن و **همه دسترسیا رو بده** — حذف پیام، مسدود کردن کاربران، افزودن
کاربران، سنجاق، تغییر اطلاعات گروه و «افزودن ادمین جدید». تا این شیش تا کامل نشه ربات فعال
نمی شه و `کانفیگ` هم کار نمی کنه؛ به جاش یه کارت می فرسته که فقط **همونایی که کم اند** توش
لیست شده، نه کل شیش تا. هر بار که دسترسیای ربات رو عوض کنی دوباره چک می کنه و می گه چی مونده،
و همین که آخریش تیک خورد خودش فعال می شه و کلینر رو هم میاره. `وضعیت نصب` هر وقتی وضع فعلی رو
می گه. بعدش `پنل` واسه تنظیمات و `دستورات` واسه لیست کامل.

**داشبورد Mini App** اختیاریه و با `MINIAPP_LINK` خاموش/روشن می شه — نبودش یعنی نه سروری بالا
میاد نه دکمه ای تو `پنل` کشیده می شه. ست کردنش دقیقا یک لینک HTTPS به شکل
`https://t.me/<bot_username>/<short_name>` بدون query/fragment می خواد که با
`/newapp` تو BotFather ثبت می شه؛ دکمه پنل خودش `?startapp=<chat_id>` رو بهش اضافه می کنه.
`MINIAPP_BIND` (پیش فرض `127.0.0.1:8787`) فقط روی loopback گوش می ده — یه ریورس پروکسی
(nginx یا Caddy) باید جلوش TLS بزنه. `MINIAPP_CONCURRENCY` همون کاری که `UPDATE_CONCURRENCY`
با استریم آپدیت می کنه رو با درخواست های Mini App می کنه.

---

## سرور

برای اجرای مستقیم یک release آماده، بعد از قرار دادن مدل های عمومی کنار باینری:

```sh
cargo build --release
./serve.sh --check
./serve.sh
```

`serve.sh --check` وجود باینری، مدل تصویری، مدل متنی، واژه نامه، mergeها و `.env` را
قبل از اجرا بررسی می کند؛ در نتیجه هر دو مسیر فیلتر عمومی و قفل غیراخلاقی آماده اند.

```sh
sudo cp groupbot.service /etc/systemd/system/
sudo systemctl enable --now groupbot
journalctl -u groupbot -f
```

`backup.sh` هر شب از دیتابیس dump و از session ها آرشیو می گیره:

```
30 3 * * * /home/ubuntu/GroupManagement/groupbot/backup.sh
```

دیتابیس تنها چیزیه که از دست دادن دیسکو زنده رد می کنه — مالک و تنظیمات و لیستای همه گروها
فقط اونجان.

### مقیاس ۵۰۰ هزار گروه

۵۰۰ هزار گروه هدف fleetه، نه یک process یا یک bot account. برای هر shard یک bot account، یک
process و یک directory جدا (`groupbot@alpha`، `groupbot@beta` و …) داشته باش؛ Telegram ظرفیت
ارسال را به account می دهد، پس چند thread یا چند process با یک token ظرفیت جدید نمی سازد.

هر shard فقط state همان گروه هایی را نگه می دارد که آن bot داخلشان است. `DB_POOL` را از مقدار
پیش فرض ۸ فقط با اندازه گیری بالا ببر؛ زیاد کردنش برای همه shardها می تواند connectionهای
Postgres را منفجر کند. تعداد shardها را از روی سه عدد انتخاب کن: message rate، rows/sec جدول
`counters` و بعد memory. انتقال گروه بین shardها با اضافه کردن bot جدید، برداشتن bot قدیمی و
انتقال ردیف های همان `chat_id` انجام می شود.

خود process هم این مرز را enforce می کند: `MAX_SHARD_CHATS=50000` پیش فرض است و اگر database یک
shard بیشتر از این گروه تنظیم شده داشته باشد، بعد از migration و قبل از materialize کردن mirror
و باز کردن stream آپدیت fail-fast می شود.
این guard جلوی حالتی را می گیرد که اشتباها یک database کل fleet به یک process داده شود؛ برای
افزایش عمدی سقف باید مقدار را همراه با نتیجه probe و بودجه host تغییر بدهی.
این سقف بعد از startup هم فعال است: اولین پیام هر گروه باید hash همان shard را در mirror ثبت کند؛
اگر سقف پر شده باشد، آن گروه قبل از ساختن state و اجرای handler پذیرفته نمی شود. raw updateهایی
که از گروه ناشناخته بعد از پر شدن سقف برسند هم state جدید نمی سازند.

ردیف های تنظیمات هم مستقل از تعداد گروه ها سقف دارند: هر گروه حداکثر ۵۱۲ ردیف در mirror دارد و
هر shard به طور پیش فرض حداکثر ۵٬۰۰۰٬۰۰۰ ردیف (`MAX_SHARD_SETTINGS_ROWS`) می پذیرد. این دو
guard جلوی آن را می گیرند که پاسخ های خودکار، لیست کاربران یا کلمات سفارشی یک گروه، ظرفیت کل
shard را پنهانی مصرف کند؛ مقدار shard را فقط همراه با `--capacity-probe` و اندازه گیری RSS
افزایش بده. علاوه بر تعداد، مجموع اندازه UTF-8 کلید و مقدارها هم به طور پیش فرض روی ۵۱۲ مگابایت
(`MAX_SHARD_SETTINGS_BYTES`) بسته است؛ اگر مقدارهای سفارشی بزرگ ترند، این سقف را فقط همراه با
نتیجه probe و بودجه واقعی حافظه افزایش بده.

همین process قبل از migration یک advisory lock دائمی روی database می گیرد و تا زمان خاموش شدن
نگه می دارد؛ بنابراین دو `groupbot@...` روی یک database هم زمان بالا نمی آیند. این lock فقط
database را محافظت می کند؛ یکتا بودن token بین databaseهای جدا همچنان باید با manifest کنترل شود.

برای admission هر shard، `UPDATE_CONCURRENCY` و `CLEANER_CONCURRENCY` قابل تنظیم هستند؛ هر دو
در خود برنامه سقف دارند و فقط بعد از اندازه گیری latency تلگرام و Postgres تغییرشان بده. زیاد
کردن taskها ظرفیت ارسال Telegram را بیشتر نمی کند و فقط صف حافظه و اثر flood-wait را بزرگ تر
می کند.

صف حذف media نیز دو لایه محدود دارد: حداکثر ۵۰۰۰ پیام در هر گروه و حداکثر ۲۰۰٬۰۰۰ ردیف در
صف‌های در انتظار flush. وقتی صف یک گروه پر شود، شناسه‌ای که از حافظه بیرون می افتد در sweep
بعدی از `pending_deletes` هم پاک می‌شود؛ بنابراین سقف حافظه فقط ظاهر کار نیست و جدول durable هم
با فشار یک گروه بی‌نهایت رشد نمی‌کند. restore شروع کار هم ردیف‌های قدیمی و overflow را batch می‌کند.
خود sweep هم بیش از ۵۱۲ گروه و ۵۰٬۰۰۰ ردیف در یک flush materialize نمی‌کند؛ اگر فشار بیشتر
باشد، گروه‌های باقی‌مانده در dirty set می‌مانند و در tick بعدی ادامه پیدا می‌کنند. این سقف
برای شمارنده‌ها، لاگ‌ها و حذف media جداگانه رعایت می‌شود تا burst هم حافظه را یک‌باره منفجر
نکند و هم work را silently drop نکند.

ردیف‌های durable مربوط به عضوها هم مرز دارند: `counters` برای هر گروه حداکثر ۲۰٬۰۰۰ عضو را
نگه می‌دارد و `notes` نیز حداکثر ۲۰٬۰۰۰ یادداشت دارد؛ به‌روزرسانی عضوهای موجود ادامه پیدا می‌کند
اما عضو یا یادداشت تازه بعد از سقف ساخته نمی‌شود. فیلتر تصویری در SQL حداکثر ۸ ردیف برای هر گروه
دارد و نوشتن هم زیر همان قفل per-chat کنترل می‌شود، نه فقط خواندن. این مهم است چون محدود کردن
صف‌های RAM بدون محدود کردن insertهای PostgreSQL، بعد از مدتی همان فشار را به دیسک و index منتقل
می‌کرد.

علاوه بر سقف هر گروه، شمارنده‌های هر shard به طور پیش‌فرض حداکثر ۵٬۰۰۰٬۰۰۰ ردیف دارند
(`MAX_SHARD_COUNTER_ROWS`). این عدد در جدول ظرفیت خود دیتابیس به شکل تراکنشی رزرو می‌شود؛ بنابراین
دو flush هم‌زمان یا چند مسیر moderation نمی‌توانند با هم از سقف عبور کنند. شروع سرویس هم اگر
دیتابیس قدیمی از سقف بزرگ‌تر باشد fail-fast می‌کند. `tools/fleet_capacity.py` و route validator
همین عدد را با `--max-counter-rows-per-shard` مدل می‌کنند.

یادداشت‌ها نیز همین guard را با `MAX_SHARD_NOTE_ROWS=5000000` دارند؛ نوشتن جدید باید هم سقف هر
گروه (۲۰٬۰۰۰) و هم سقف shard را جا داشته باشد و حذف، ظرفیت رزروشده را آزاد می‌کند.

left-back فقط برای گروهی که این گزینه روشن است و کاربری که قبلا `/start` کرده اجرا می‌شود. لینک
دعوت یک‌بار برای هر گروه ساخته و در settings cache می‌شود؛ برای هر خروج فقط lookup محلی و DM
می‌ماند، نه یک `ExportChatInvite` تازه و یک لینک یک‌بارمصرف. فهرست کاربران `/start` شده در هر
shard یک cache durable با سقف ۵٬۰۰۰٬۰۰۰ ردیف است و وقتی پر شود قدیمی‌ترین activationها به شکل
تراکنشی بیرون می‌روند؛ بنابراین این قابلیت با churn کاربران جدول بی‌نهایت نمی‌سازد.

با `LOG=info` خط `capacity` هر دقیقه علاوه بر صف ها و cacheها، تعداد update/cleanerهای فعال،
`updates_received` و `updates_completed` در پنجره آخر، تعداد connectionهای باز و idle، صف مشترک
outbound و تعداد durable `counter_rows` و `note_rows` همین shard را نشان می دهد؛ `update_active`،
connectionهای نزدیک سقف یا ردیف های durable نزدیک limit باید با telemetry واقعی بررسی شوند، نه با
زیاد کردن کورکورانه concurrency.
واحد systemd هر shard نیز `MemoryHigh=2500M` و `MemoryMax=3G` دارد؛ اگر مدل های محلی بیشتری
نصب می کنی، قبل از افزایش pool این سقف را با نتیجه `--model-probe` هماهنگ کن.

برای اندازه گیری واقعی mirror، اول روی یک دیتابیس موقت `tools/capacity_probe.sql` را اجرا کن، بعد
همان binary را با `DATABASE_URL` همان دیتابیس و فلگ `--capacity-probe` اجرا کن. این حالت هیچ session
تلگرامی را باز نمی کند و زمان load آینه، تعداد گروه ها، تعداد ردیف ها و یک scan از چند setting را
چاپ می کند. دیتابیس probe را هرگز روی دیتابیس production اجرا نکن.

برای اندازه گیری RSS بدترین حالت مدل ها، همان binary را بدون `DATABASE_URL` با `--model-probe`
اجرا کن. این حالت هیچ Telegram session یا دیتابیسی باز نمی کند، اما استخرهای NSFW، grading، OCR
و مدل مفهومی را load می کند؛ خروجی `/usr/bin/time -v` را برای انتخاب `NSFW_INFER_SESSIONS` و
`NSFW_SLOTS` ثبت کن.

یک اجرای واقعی روی همین سرور با ۵۰۰٬۰۰۰ گروه، شش ردیف تنظیم و marker قفل شب برای هر گروه، warm load
حدود ۶٫۷ ثانیه، سه scan متوالی حدود ۲٫۰ ثانیه و اوج RSS حدود ۶۶۵ مگابایت داشت. اولین اجرا به خاطر
migrationهای قدیمی حدود ۲۹ ثانیه طول کشید؛ بعد از ثبت markerها این هزینه تکرار نمی شود. این اعداد
هزینه mirror را ثابت می کنند، نه ظرفیت ارسال تلگرام؛ تعداد shardها همچنان باید از message rate،
rows/sec و memory انتخاب شود.

در `--model-probe` فعلی، با دو session و دو slot و با فعال بودن NSFW، grading، هر دو OCR و مدل
مفهومی، اوج RSS برابر ۱٬۶۲۷٬۵۰۴ kB بود؛ نسخه هشت-session در همین worker به ۶٫۸۷ GiB رسید. به
همین دلیل estimator برای هر shard ۲۰۴۸ MB را محافظه کارانه می گیرد؛ این عدد شامل mirror کوچک
۵۰ هزار گروه و حاشیه عملیاتی است، نه یک تضمین برای workerهایی که مدل یا voice بیشتری فعال می کنند.

تمام writeهای اصلی Telegram در هر client از یک token bucket مشترک عبور می کنند: پیش فرض ۲۵ action
در ثانیه، burst برابر ۲۵ و حداکثر ۳۲ درخواست هم زمان. این queue بین `Message::reply`، jobهای زمان بندی
شده و moderation مشترک است، پس چند مسیر داخلی نمی توانند سقف account را جداگانه دور بزنند. علاوه بر
آن، messageهای مقصد یک group از pacing جداگانه پیش فرض ۲۰ پیام در دقیقه عبور می کنند؛ این خواب گروه
شلوغ، slotهای account را نگه نمی دارد و گروه های دیگر می توانند کار کنند. مقدارها با
`TELEGRAM_ACTIONS_PER_SECOND`، `TELEGRAM_ACTION_BURST`، `TELEGRAM_OUTBOUND_CONCURRENCY` و
`TELEGRAM_GROUP_MESSAGES_PER_MINUTE` قابل تغییرند؛ فقط بعد از دیدن `outbound=active/waiting` و
429های واقعی آن ها را بالا ببر. metadata این pacing با `TELEGRAM_CHAT_RATE_CACHE_MAX` سقف دارد.

برای اینکه admission کل fleet حدسی نباشد، قبل از ساختن shardها estimator بدون dependency را اجرا کن:

```bash
python3 tools/fleet_capacity.py \
  --groups 500000 \
  --groups-per-shard 50000 \
  --updates-per-second 0 \
  --updates-per-shard-per-second 500 \
  --actions-per-second 250 \
  --telegram-actions-per-second 25 \
  --counter-rows-per-second 8000 \
  --counter-rows-per-shard-per-second 1000 \
  --counter-rows-per-group 100 \
  --max-counter-rows-per-shard 5000000 \
  --note-rows-per-group 100 \
  --max-note-rows-per-shard 5000000 \
  --settings-rows-per-group 100 \
  --max-settings-rows-per-shard 5000000 \
  --settings-bytes-per-group 10240 \
  --max-settings-bytes-per-shard 536870912 \
  --db-pool 8 \
  --db-max-connections 100 \
  --memory-per-shard-mb 2048 \
  --host-memory-mb 22000 \
  --host-cpu-cores 8 \
  --cpu-cores-per-shard 2 \
  --hosts-available 3 \
  --json
```

در این مثال ۵۰۰ هزار گروه حداقل ۱۰ shard می خواهد؛ نرخ ۲۵۰ action در ثانیه با سقف ۲۵ هم ۱۰ shard و نرخ
۸۰۰۰ ردیف counter در ثانیه با ظرفیت اندازه گیری شده ۱۰۰۰ ردیف برای هر shard، ۸ shard می خواهد،
و میانگین ۱۰۰ ردیف ساکن counter و ۱۰۰ یادداشت برای هر گروه، با سقف ۵٬۰۰۰٬۰۰۰ ردیف از هر نوع در هر
shard، ۱۰ shard می خواهد؛ این اعداد را از inventory واقعی اعضا تنظیم کن، چون سقف ۲۰٬۰۰۰ عضو برای هر گروه می تواند خیلی بیشتر
از این مثال فضا بخواهد.
و با فرض میانگین ۱۰۰ ردیف تنظیمات برای هر گروه، سقف ۵٬۰۰۰٬۰۰۰ ردیف تنظیمات هم ده shard می خواهد،
پس سقف گروه تعیین کننده است. با سقف عملیاتی ۲۵ action در ثانیه، ده shard ۲۵۰ action را پوشش
می دهند. با `DB_POOL=8`، ده shard ۸۰ connection می خواهند و با ۲۰٪ headroom عدد محاسبه شده
۱۰۰ است. مقدار پیش فرض `DB_POOL=8` برای یک shard و fleet ده-shard مناسب است. پارامترهای CPU در
این مثال نشان می دهند که ده shard با دو inference slot و رزرو تقریبی دو core برای هر shard روی
host هشت-core به سه host نیاز دارد؛ چهار، سه و سه shard روی hostها قرار گرفته اند. برای workload
متن-only که مدل ها خاموشند می توانی `--cpu-cores-per-shard 1 --hosts-available 2` بگذاری، اما
برای فعال بودن مدل های تصویر، مقدار دو را پایین نیاور مگر بعد از benchmark واقعی. اگر تمام shardها
را روی یک host می گذاری، `--hosts-available 1` را بزن تا estimator در صورت overcommit fail شود.
این ابزار وقتی بودجه database یا memory جا نشود با exit code 2 خارج می شود. مقدارهای
`--updates-per-second`، `--actions-per-second` و `--counter-rows-per-second` باید از telemetry
واقعی بیایند؛ `--counter-rows-per-group` ظرفیت ساکن را مدل می کند و `--updates-per-shard-per-second`
ظرفیت اندازه گیری شده dispatch همین binary است.
صفر در مثال یعنی نرخ ورودی هنوز اندازه گیری نشده و نباید به عنوان ظرفیت ثابت fleet تفسیر شود.
عدد ۳۰ فقط سقف تقریبی account تلگرام در حالت عادی است و افزایش process یا thread آن سقف account
را زیاد نمی کند.

قبل از فعال کردن واحدهای systemd، manifest بدون secret را هم بررسی کن. فایل نمونه فقط اسم
referenceهای token و database را دارد؛ مقدار واقعی توکن و password باید در `.env` همان shard
بماند و هیچ وقت داخل manifest یا git نیاید:

```bash
python3 tools/fleet_manifest.py tools/fleet.example.json --json
```

validator جمع گروه ها، حداقل shard لازم، ظرفیت ساکن counterها و noteها، یکتا بودن token reference و database
reference و بودجه واقعی connection و memory همان تعداد shard اعلام شده را چک می کند. بعد از ساختن `.env`های واقعی
و انتقال گروه ها، هر shard را با `groupbot@<name>` جداگانه فعال کن؛ اجرای چند process با یک token
یا یک database عمدی رد می شود چون cache تنظیمات بین processها هماهنگ نمی شود.

برای ساختن خروجی قابل انتقال به هر host، `fleet_prepare.py` بعد از همین validation routeها را
می سازد و برای هر shard فقط `chat_ids.txt` و `.env.example` بدون secret می نویسد. با `--host` می
توانی فقط directoryهای یک host را آماده کنی؛ مقدارهای واقعی `TG_BOT_TOKEN` و `DATABASE_URL` را
بعدا از secret manager داخل همان `.env`ها پر کن:

```bash
python3 tools/fleet_prepare.py tools/fleet.example.json \
  --input chat_ids.txt --output-dir /home/ubuntu/shards --host host-a --json
```

این command سرویس را start نمی کند و فایل موجود را بدون `--force` overwrite نمی کند؛ بعد از review
می توانی `systemctl enable --now groupbot@alpha` و بقیه shardهای همان host را اجرا کنی.

برای اینکه مالکیت گروه ها بین deployها حدسی نشود، manifest از اولویت بندی rendezvous استفاده
می کند و در عین حال سقف واقعی `groups_per_shard` را روی یک فهرست کامل enforce می کند؛ pure hash
به تنهایی به خاطر uneven بودن می تواند یک shard را چند صد گروه از سقف بالاتر ببرد. خروجی فقط
برنامه ownership است و خودش bot را add/remove نمی کند:

```bash
python3 tools/shard_route.py tools/fleet.example.json \
  --input chat_ids.txt > ownership.tsv
```

اگر اندازه تنظیمات گروه ها خیلی متفاوت است، قبل از route یک inventory با سه ستون
`chat_id settings_rows settings_bytes` بساز و همان را با `--weights weights.txt` به ابزار بده.
در این حالت علاوه بر تعداد گروه، مجموع ردیف و byte هر shard هم زیر سقف manifest نگه داشته می شود؛
`fleet_routes.py` نیز با همین فایل routeهای ساخته شده را دوباره بررسی می کند.
برای اندازه واقعی جدول `counters` و `notes` هم فایل های دو ستونه `chat_id counter_rows` و
`chat_id note_rows` بساز و با `--counter-weights` و `--note-weights` بده؛ این وزن ها جلوی جمع شدن گروه های پرعضو روی یک shard
را می گیرند.
همین validator برای هر route مجموع counterهای ساکن را هم از روی `counter_rows_per_group` با سقف
`max_counter_rows_per_shard` و مجموع noteها را با `max_note_rows_per_shard` تطبیق می دهد.

```bash
python3 tools/shard_route.py tools/fleet.example.json \
  --input chat_ids.txt --weights weights.txt --counter-weights counter_weights.txt \
  --note-weights note_weights.txt > ownership.tsv
python3 tools/fleet_routes.py tools/fleet.example.json \
  --input chat_ids.txt --weights weights.txt --counter-weights counter_weights.txt \
  --note-weights note_weights.txt \
  --routes-dir /home/ubuntu/shards --pattern '{shard}/chat_ids.txt'
```

با تغییر فهرست shardها، فهرست کامل `chat_ids` را دوباره route و diff کن؛ گروه های جابه جا شده را
قبل از برداشتن bot قدیمی به bot مالک جدید منتقل کن. این ترتیب cacheهای دو process را هم زمان روی
یک گروه فعال نگه نمی دارد و سقف هر shard هم در migration شکسته نمی شود. برای enforce کردن همین
تصمیم در خود process، برای هر shard خروجی را به یک فایل فقط شامل id تبدیل کن و در `.env` همان
shard بگذار:

```bash
python3 tools/shard_route.py tools/fleet.example.json \
  --input chat_ids.txt --shard alpha > /home/ubuntu/shards/alpha/chat_ids.txt
# /home/ubuntu/shards/alpha/.env
SHARD_CHAT_IDS_FILE=/home/ubuntu/shards/alpha/chat_ids.txt
```

قبل از هر rolling restart، کل routeها را هم یک بار با manifest تطبیق بده؛ این check جلوی missing
یا duplicate شدن chat بین دو shard را می گیرد:

```bash
python3 tools/fleet_routes.py tools/fleet.example.json \
  --input chat_ids.txt --routes-dir /home/ubuntu/shards --pattern '{shard}/chat_ids.txt' --json
```

وقتی `SHARD_CHAT_IDS_FILE` تنظیم باشد، startup اگر database row خارج از route باشد fail-fast می کند
و update، callback، edit و raw participant از گروهی که در فایل نیستند قبل از ساختن state یا write
رد می شوند. واحد `groupbot@<name>` هم به همین دلیل بدون این فایل اصلا اجرا نمی شود. فایل باید یک id در هر خط داشته باشد و حداکثر `MAX_SHARD_CHATS` id داشته باشد؛ تغییر
route یعنی ساختن فایل های جدید، انتقال durable rows، و سپس rolling restart هر دو shard.

برای انتقال خود ردیف های durable، `tools/shard_migrate.py` فقط یازده جدول chat-scoped را می بیند:
`settings`، `counters`، `notes`، `tallies`، `pending_deletes`، `pending_captchas` و
`pending_warn_actions`، `pending_rank_awards`، `moderation_cases`،
`moderation_case_events` و `image_filters`. جدول های
`started_users` و `calibration` سراسری هستند و عمداً منتقل نمی شوند. DSNها را در environment بگذار؛
ابزار secret را روی command line یا داخل manifest نمی گیرد. پیش فرض فقط preflight خواندنی است:

```bash
export SOURCE_DATABASE_URL='postgresql://...'
export TARGET_DATABASE_URL='postgresql://...'
python3 -m pip install 'psycopg[binary]>=3.1,<4'
python3 tools/shard_migrate.py \
  --input ownership.tsv --shard alpha \
  --source-stats-dir /home/ubuntu/shards/source/durable-work/stats \
  --target-stats-dir /home/ubuntu/shards/alpha/durable-work/stats --json
```

قبل از `--apply` هر دو bot process را متوقف کن. migrator ظرفیت `MAX_SHARD_CHATS`،
`MAX_SHARD_SETTINGS_ROWS`، `MAX_SHARD_SETTINGS_BYTES`، `MAX_SHARD_COUNTER_ROWS` و `MAX_SHARD_NOTE_ROWS` را چک می کند، هر batch را محدود نگه می دارد، مقصد را با ردیف های source
مقایسه می کند و فقط بعد از verification ردیف های source را حذف می کند. اگر بعد از commit مقصد قطع شد،
اجرای دوباره همان command فقط وقتی ادامه می دهد که ردیف های مقصد دقیقاً برابر source باشند؛ partial
یا conflicting row را overwrite نمی کند. پس از copy صریح idها، sequenceهای case/event و sequence
مشترک generation/lease نیز حداقل تا بیشترین مقدار منتقل شده جلو برده می شوند تا fencing token یا id
بعدی با durable work واردشده تکراری نشود:

خاموش کردن باید graceful باشد تا flush نهایی تمام شود. هر دو مسیر `durable-work/stats` باید وجود
داشته و کاملاً خالی باشند؛ migrator در صورت نبودن مسیر، فایل موقت/ناشناخته یا batch مانده fail-closed
می شود. ابتدا shutdown را کامل کن، خالی بودن هر دو مسیر را با preflight زیر ثابت کن، و فقط بعد همان
دو مسیر را به اجرای `--apply` بده. ردیف های outbox داخل PostgreSQL همراه گروه منتقل می شوند.

```bash
python3 tools/shard_migrate.py \
  --input ownership.tsv --shard alpha --batch-chats 100 \
  --source-stats-dir /home/ubuntu/shards/source/durable-work/stats \
  --target-stats-dir /home/ubuntu/shards/alpha/durable-work/stats --apply --json
```

این انتقال بین دو database یک تراکنش اتمیک PostgreSQL نیست؛ backup بگیر و تا پایان کار processهای
مبدأ و مقصد را پایین نگه دار. پس از انتقال، route را دوباره بررسی کن، bot جدید را اضافه کن، و بعد
bot قدیمی را از گروه های منتقل شده خارج کن.

---

## توسعه دادن

طوری چیده شده که اضافه کردن فیچر یعنی دست زدن به **یه جا**، نه شیش جا.

### یه قفل جدید

یه ردیف تو `locks::LOCKS` و یه تابع matcher. تموم — پنل، `قفل …`، `بازکردن …`، خروجی `قفل ها`
و حالت سختگیرانه همه خودکار می بیننش.

```rust
Lock {
    key: "webapp",
    names: &["وب اپ", "مینی اپ"],
    matches: is_webapp,
},

fn is_webapp(view: &View) -> bool {
    matches!(view.media, Some(Media::WebPage(_)))
}
```

matcher فقط از `View` می خونه و هیچ وقت `await` نداره. چیزی که بیشتر از یه بار لازمش داری رو
بذار تو `View` تا یه بار واسه کل پیام حساب شه، نه یه بار واسه هر قفل.

### یه فیچر جدید

یه ماژول تو `handlers/` و یه خط تو `dispatch`:

```rust
pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    if message.text().trim() != "دستور من" {
        return false;                       // مال من نبود
    }
    if !can_manage(ctx, message).await {
        return true;                        // مال من بود ولی دسترسی نداشت
    }
    true
}
```

جای خطی که تو `dispatch` می ذاری مهمه: هرچی بالاتر، زودتر پیامو برمی داره.

### یه تنظیم جدید

- **flag** → `settings.set(chat, "key", on)` و `settings.is_locked(chat, "key")`
- **مقدار** → `settings.set_value(...)` و `settings.value_parsed(...)`
- **set** → کلید prefix دار؛ اگه iterate لازم داره، prefix رو به `state::INDEXED` اضافه کن
- **ردیف پنل** → یه `Toggles` تو `toggles::GROUPS`، یا واسه عددیا یه ردیف تو `tune::SETTINGS`

### دست نزن

- ترتیب `can_manage` هست مالک ← ادمین ربات ← ادمین گروه، از ارزون به گرون.
- `ترفیع` و `تنزل` **هیچ وقت** نباید `set_admin_rights` صدا بزنن؛ دسترسی واقعی تلگرام تصمیم
  مالک گروهه.
- target هر دستور همیشه از `handlers::target` میاد تا رفتار همه دستورا یکی بمونه.

### پیاما

`✓` روشن · `✗` خاموش · `★` مالک · `‹` بولت لیست. هرچی بلندتر از یه خطه HTMLه و اسمی که از
کاربر اومده باید از `esc()` رد شه. رنگ دکمه هم state رو می گه: سبز روشن، آبی انتخاب شده، قرمز
خطرناک.

---

## تست

قبل از هر دیپلوی هر دوتا باید تمیز باشن:

```sh
cargo clippy --all-targets        # صفر warning
cargo test -- --include-ignored   # با roundtrip واقعی Postgres
```

تستا رو چیزایین که شکستنشون بی صداست: شکل پیلود دکمه های پنل، تبدیل ارقام فارسی، پارس دستورا
و منطق تصمیم قفلا.

برای سنجش جداگانه ی cascade تصویری، `tools/evaluate_nsfw.py` نمونه ها را فقط در حافظه از
منابع عمومی می خواند و precision، recall و false-positive rate را گزارش می کند. **مجموعه ی
منفی مهم تر از مثبت است**: با غذا به عنوان منفی، نرخ خطا صفر در می آید و همان cascade روی
عکس پرتره ی معمولی خطا می دهد؛ منفی واقعی یعنی عکس آدم — Flickr30k و CelebA. حالت
`--source pad3` فقط ردیف های صریحاً `pornography` را مثبت می گیرد؛ unsafe های مربوط به
بتینگ و خشونت را عمداً وارد برچسب NSFW نمی کند.

در مسیر محصول هیچ امتیاز، حالت آزمایشی، کالیبراسیون یا برچسب دستی وجود ندارد. برای تست،
قفل را روشن کنید و از لاگ های حذف/نگه داری و تست های خودکار استفاده کنید؛ تصمیم نهایی فقط
دو حالت دارد: نگه داشتن یا حذف کردن.

---

## ساختار پروژه

```
src/
├── main.rs              session، sender pool، لوپ آپدیت، تایمرا
├── state.rs             تنظیمات: Postgres + کش مموری
└── handlers/
    ├── mod.rs           dispatch، can_manage، target، admins، Ctx
    ├── locks.rs         جدول قفلا و hot path مچ کردن
    ├── panel.rs         پنل شیشه ای
    ├── toggles.rs       گزینه های چند حالته پنل
    ├── tune.rs          معادل دستوری تنظیمای عددی
    ├── style.rs         دکمه رنگی
    ├── callbacks.rs     روت کردن کلیک دکمه ها
    ├── config.rs        کانفیگ، مالک، لیست دستورا
    ├── autoconfig.rs    کانفیگ خودکار موقع ادمین شدن
    ├── install.rs       چک دسترسیای لازم و آوردن خودکار کلینر
    ├── captcha.rs       احراز هویت
    ├── emoji_image.rs   رندر ایموجی به PNG
    ├── join.rs          عضویت اجباری و اد اجباری
    ├── bots.rs          قفل ربات
    ├── welcome.rs       خوشامد
    ├── restrict.rs      سکوت و بن
    ├── promote.rs       افزودن ادمین
    ├── rights.rs        دسترسیای پیش فرض اعضا
    ├── warns.rs         اخطار
    ├── vip.rs           کاربر ویژه
    ├── lists.rs         لیستا تو پنل
    ├── filters.rs       فیلتر کلمه
    ├── packs.rs         قفل پک استیکر
    ├── flood.rs         ضد رگبار
    ├── betrayal.rs      ضد خیانت ادمین
    ├── strict.rs        حالت سختگیرانه
    ├── purge.rs         حذف batch
    ├── cleaner.rs       اکانت یوزر
    ├── stats.rs         شمارنده و داشبورد
    ├── log.rs           کانال لاگ
    ├── report.rs        گزارش به ادمینا
    ├── answers.rs       پاسخ خودکار
    ├── notice.rs        اعلان حذف
    ├── extras.rs        قوانین، یادداشت، سنجاق، اسلوموشن، قفل شب
    ├── currency.rs      نرخ ارز صفحه بندی شده از AlanChand
    └── ping.rs          سرعت پاسخ
```

</div>

## Semantic moderation icons

`src/handlers/premium/registry.json` is the only custom-emoji ID registry. IDs are decimal
strings in JSON and converted to `i64` only for MTProto. Each entry records its semantic key,
Unicode fallback, original placeholder, tags, confidence, usage and provenance. The registry
preserves 89 documents from the previous implementation and adds the red error document
specified in the emoji task. The original message dump and screenshot were not available
with that task; legacy-only mappings remain unclassified, and the written visual descriptions
are the evidence for the active mappings.

`premium::icon_for(Context { action, object, state, severity, scope, duration })` returns an
optional semantic `Icon`. Rules are deterministic and language independent: mute, ban and
warning have distinct icons; permission and anti-trade state select their allowed/blocked
icons; severe triggered raids use fire; cooldowns use the timer. Failed, pending or paused
operations are shown as such before selecting a completed-action icon. Unknown contexts can
have no icon. Help topics, panel sections and Mini App permissions share these rules.

```rust
let selected = premium::icon_for(premium::Context {
    action: "mute", object: "user", state: "active", ..Default::default()
});
let message = premium::icon_text(selected, "سکوت کاربر");
let button = premium::decorate(Button::data("سکوت", b"mute_user"), selected);
```

`premium::icon_html`, `icon_text` and `badge` mark only bot-owned presentation. Ordinary
`premium::text` is opaque. The HTML renderer converts explicit semantic badges into
`MessageEntityCustomEmoji` entities, preserving formatting and UTF-16 offsets. It never scans
translated labels or ordinary emoji for meaning. User-authored welcome/answer content uses
the library's content renderer directly; captions, names, quoted evidence, filter words and
command arguments are not rewritten.

At startup one bounded, read-only `messages.getCustomEmojiDocuments` request validates the
active documents and gets their exact `alt` strings. Missing or failed metadata falls back to
Unicode. This distinction matters when a document's Telegram placeholder differs from its
visual meaning, such as the send icon. Telegram requires custom entities to wrap that exact
`alt`, as described in [Custom emojis](https://core.telegram.org/api/custom-emoji).

| Surface | Rendering in this project |
| --- | --- |
| Direct bot messages, moderation replies, bot captions and message edits | Custom entities when enabled and metadata is available; semantic Unicode otherwise |
| Inline callback and URL buttons | Native MTProto `KeyboardButtonStyle.icon`; Unicode label prefix when disabled or unavailable; callback bytes, URLs and colours preserved |
| Channel moderation logs | Unicode by default; custom entities only with explicit channel eligibility enabled |
| Callback alerts/toasts | Unicode/plain text; no entity markup |
| Mini App | Central-registry Unicode fallbacks for matching concepts, existing SVGs for other concepts; receives semantic state keys, never document IDs |
| Captcha choices and numeric presets | Their exact symbols/numbers, with no added decoration |
| User-authored content and reply-keyboard command labels | Preserved without emoji substitution |

The local grammers schema supports native button styles. Telegram restricts custom button
icons to eligible bots (a purchased Fragment username or a Premium bot owner); see
[Bot buttons](https://core.telegram.org/api/bots/buttons). Reading a document does not prove
the bot's sending entitlement. Configure the deployment accordingly; no send attempt is
made to probe entitlement.

| Environment variable | Default | Effect |
| --- | --- | --- |
| `PREMIUM_EMOJI` | enabled | `0`, `false` or `off` forces Unicode for messages and buttons |
| `PREMIUM_EMOJI_BUTTONS` | enabled | Independently disables native button icons |
| `PREMIUM_EMOJI_CHANNELS` | disabled | `1` or `true` enables custom channel-log entities only for a bot eligible to use them there |

The audit covers start/help, setup/admin and owner panels, settings, member information,
warnings and enforcement, unmute/unban, locks, anti-trade/link/flood/raid, voice/photo/media
permissions, filters, logs/reports/cases, scheduled operations, confirmations, errors,
navigation, welcome/answer presentation and the Mini App. Plain instructions and unmatched
concepts retain their appropriate Unicode/SVG/text presentation. Moderation decisions,
database/configuration keys, commands and callback routing are unchanged.

Of the 90 custom documents, 31 are active, 12 are unused for moderation and 47 require review.
The medium-confidence visuals are `MEMBERS`, `IMAGE_PRIVATE`, `BRIEFCASE`, `NETWORK` and
`INVISIBLE`; they are not dynamically selected for premium rendering. The unrelated money,
shopping, gift, calculator, tag/swap and diamond assets are unused. The remaining abstract
legacy visuals stay unclassified. The registry records every individual classification.

Validation commands:

```sh
cargo test --locked handlers::premium
cargo test --locked
cargo clippy --all-targets --locked
python tools/audit_premium.py
node --check src/miniapp/assets/app.js
node tools/test_premium_ui.cjs
```

Observed during this migration: 15 emoji tests passed; the complete non-ignored Rust run had
305 passing tests, zero failures and 24 ignored database/external-model tests. The registry
audit found no raw IDs outside the registry/test fixtures. The Mini App renderer test covers
mixed Persian/English labels, permission states, fallback keys and unchanged action attributes.
These are local construction/rendering checks. Screenshot comparison, live Telegram appearance,
bot entitlement, sending/editing actual messages and client-specific RTL layout remain unverified.
Deployment follow-up (2026-09-06): panel and help close actions remove their inline keyboards;
the panel also displays a closed confirmation above its lock summary. The initial empty keyboard
was rejected by Telegram with `REPLY_MARKUP_INVALID`. Closing now omits `reply_markup`, following
[TDLib's edit implementation](https://github.com/tdlib/td/blob/master/td/telegram/MessagesManager.cpp)
and its conversion of an empty inline keyboard to null. All 329 Rust
tests passed with ignored tests enabled against an isolated local PostgreSQL database and the
production image and text models. Clippy, UI checks and the
locked release build passed. The binary was deployed to `groupbot-prod` with a recoverable backup,
its running checksum was verified, and `groupbot.service` remained active with zero restarts.
Startup validated 31 custom emoji documents and the Mini App served the new semantic registry.

### Cleaner recommendations

Groups with a missing or non-admin cleaner receive a Persian recommendation with an inline
«افزودن کلینر» / «بررسی دسترسی و افزودن کلینر» button about every six hours. Each known group has
a fixed minute within that interval; the indexed `hash` rows select only due groups, and
bounded workers check live Telegram membership before sending. `cln_checked_slot` remembers
completed checks across restarts. Failed membership checks or sends remain eligible in the
three-minute delivery window. No recommendations run while the shard's cleaner is signed out.

The `cln:<chat>` callback is bound to its original group and uses the same admin and `CLEAN`
capability checks as «افزودن کلینر». Every press checks the management bot's current required
permissions. Missing access produces an alert, a checklist, the path to Telegram's admin
settings, and a retry button. Unknown permissions never start a join. Successful setup replaces
the recommendation with confirmation and removes its button. The text command shares this flow;
manual joins also share the automatic flow's concurrency limit and per-group duplicate guard.
The durable `cln_added` flag continues to prevent automatic re-adding after deliberate removal;
an administrator can explicitly restore the cleaner with the recommendation button.

Deployed to `groupbot-prod` on 2026-09-06 at 19:29 UTC. All 335 Rust tests passed with
ignored tests enabled against an isolated local PostgreSQL database and verified production
models (`VISION_FILES=target/cleaner-deploy/models`); Clippy and the locked release build passed.
The older models in local `target/release` are not the production vision exports.
The running executable matched SHA256
`10ccbc8372c1504abfb1153b168076ae665c814e46cde554a6daa36fa9d965bc`;
`groupbot.service` stayed active with zero restarts, the cleaner signed in, and the reminder
index was present. Rollback binary on production:
`/home/ubuntu/GroupManagement/groupbot/target/release/groupbot.prev.20260906T192916Z`.

### Automatic group setup recovery

The first join or promotion now resolves the group's access hash from the bot's Telegram
session before listing administrators or inviting the cleaner. Previously this path could
fabricate a zero-hash supergroup reference, causing `CHANNEL_INVALID` and leaving setup
incomplete even with all required admin rights.

Setup handles additions, group creation, migration and permission updates before moderation.
Known incomplete groups recover at startup, including groups with an owner whose cleaner
has never finished joining; group activity also checks for incomplete setup
at most once per minute. Transient failures receive bounded retries, and a per-group mutex
prevents duplicate configuration without suppressing retries after a failed lookup. The owner
record is written only after the default locks are saved. Existing configured groups retain
their settings, and intentional cleaner removal still requires the explicit add button.
That button also finishes any missing group configuration after checking admin permissions.
Cleaner installations are serialized per account, and short Telegram join flood waits are
honored for up to two additional retries using the same invitation. Longer waits retain the
actionable failure card and retry button.

Validation: all 341 Rust tests passed, including database and external-model tests using an
isolated PostgreSQL database and the verified production models in `target/cleaner-deploy/models`.
Clippy passed with `--all-targets -- -D warnings`.

Deployed on 2026-09-06 at 20:15 UTC. The running executable matched SHA256
`fd0f412968a5e5f2b0af9af80430eac9127565285a8aa24d94e8e6b101bc76aa`;
`groupbot.service` remained active with zero restarts. Live Telegram membership checks
confirmed that both the management bot and cleaner were administrators with all six required
rights in the affected group. Its owner, five default locks and `cln_added` marker were present.
The earlier recovery pass also activated four other previously unconfigured groups.
Rollback binary on production:
`/home/ubuntu/GroupManagement/groupbot/target/release/groupbot.prev.20260906T201506Z`.

Broadcast channel exclusion: group setup, cleaner reminders and their callback handling now
require a confirmed basic group, supergroup or gigagroup. Saved access hashes alone cannot
identify a group. Unknown types wait for Telegram's group metadata; known broadcast channels
are skipped at startup. New and edited channel posts are excluded from group command and
moderation handlers before admission. The destination is checked, so linked channel posts
and anonymous administrators inside a discussion group continue through the group flow.
Old channel buttons answer only the presser with a group-only alert. This does not alter
explicitly configured log channel destinations.

Deployed on 2026-09-06 at 20:45 UTC after all 343 Rust tests passed (including isolated
PostgreSQL and production-model tests), warning-free Clippy and the locked release build.
Production startup logged all three saved broadcast channels as skipped while live group
handling continued. The service stayed active with zero restarts and its running checksum
matched `af34e79bf75a9731a4dc80b040dced8a6dab35568eff247edf940cefcbd03f93`.
Rollback binary: `/home/ubuntu/GroupManagement/groupbot/target/release/groupbot.prev.20260906T204505Z`.
