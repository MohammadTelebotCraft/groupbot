#!/usr/bin/env python3
"""Generates the training corpus «قفل خرید و فروش» learns from — labeled by *frame*.

Every production false positive had the same shape: a *register* the hand-written exemplars
never covered — a friend begging for game coins, the plural of the shop noun, economy news
with prices in it, an ad quoted inside a warning. Hand-writing a dozen sentences per incident
does not scale; this generator writes the registers themselves, as template families with slot
lists, and emits thousands of labeled sentences per run. A future incident becomes one new
template family — the family then covers the register, not the sentence.

Since the classifier is fine-tuned end to end, every row also carries the **frame** it belongs
to: not just «sell» or «safe» but *which kind* of selling or not-selling it is. The frames are
the failure classes the lock has to tell apart — a quoted ad, a past purchase, a hypothetical,
a refusal, a buyer's question — and the model learns them as classes, so the frame it reads a
message as is written into the journal beside the score.

Deterministic (seeded), so the corpus is reproducible and diffs are meaningful. Output:
`tools/data/intent_corpus.tsv` (`label<TAB>frame<TAB>text`).

**The eval set and the battery are the judges and must never leak into the corpus.** After
generation, any row whose normalised trigram overlap with a row of either is high is dropped —
the held-out sets stay held out, or their verdict means nothing.

    ../bin/python tools/gen_intent_corpus.py
"""

from __future__ import annotations

import pathlib
import random
import re

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE / "data" / "intent_corpus.tsv"
EVAL = HERE / "data" / "intent_eval.tsv"
BATTERY = HERE / "data" / "intent_battery.tsv"
FLEET = HERE / "data" / "intent_fleet.tsv"

SEED = 4711
# Per template family, how many slot combinations are sampled. Families multiply out to far
# more; sampling keeps the corpus balanced so no family dominates the head.
PER_FAMILY = 150

# The frames, in the order the export writes them into `intent_frames.txt`. The first group
# is what the lock deletes; the second is everything that merely sounds like it. Free-written
# rows (the fleet file, the hand exemplars) carry a label and no frame — `NO_FRAME` — and
# supervise the score alone. They used to have catch-all frames, and the catch-all became
# the bucket the model put every unusual negative into: the eval's boundary was set by
# ordinary discussion rows read as «other_sell».
SELL_FRAMES = ["offer", "want", "exchange", "service", "rental", "payment", "indirect"]
SAFE_FRAMES = [
    "past", "quote", "refusal", "hypothetical", "inquiry", "news", "joke", "meta", "advice",
    "tutorial", "third", "beg", "job", "charity", "lost", "complaint", "plain",
]
FRAMES = SELL_FRAMES + SAFE_FRAMES
NO_FRAME = "-"

# ---------------------------------------------------------------------------------------
# slots
# ---------------------------------------------------------------------------------------

GOODS = [
    "گوشی", "گوشی سامسونگ", "آیفون ۱۱", "آیفون ۱۳", "لپ تاپ", "لپ تاپ ایسوس", "تبلت", "مانیتور",
    "پلی استیشن", "پلی استیشن ۵", "ایکس باکس", "دسته بازی", "هدفون", "ساعت هوشمند", "دوچرخه", "موتور",
    "ماشین", "پراید", "پژو ۲۰۶", "یخچال", "ماشین لباسشویی", "مبل", "فرش", "میز تحریر",
    "کتاب های کنکور", "گیتار", "کیبورد", "دوربین", "کولر گازی", "بخاری", "کفش", "کتونی",
    "مانتو", "لباس بچگانه", "عطر", "ساعت مچی", "سرویس طلا", "پاوربانک", "مودم", "چادر مسافرتی",
    "کارت گرافیک", "سیم کارت رند", "بلیط کنسرت", "اسپیکر", "پیانو دیجیتال",
]
ENGLISH_GOODS = ["iphone 13", "ps5", "macbook", "airpods", "galaxy s23", "rtx 3060", "apple watch", "xbox series s"]
GAME = ["سکه", "جم", "یوسی", "سی پی", "وی بی", "الماس", "میو پوینت", "میو", "گلد", "اسکین", "اکانت کلش", "اکانت پابجی", "سکه سلف", "نیترو"]
DIGITAL = ["اکانت تلگرام پرمیوم", "اکانت اسپاتیفای", "نیترو", "گیفت کارت", "لایسنس ویندوز", "کانفیگ", "شماره مجازی", "تتر", "پی پال", "اکانت استیم"]
SERVICES = [
    "طراحی لوگو", "تدوین ویدیو", "تایپ و ترجمه", "تدریس ریاضی", "مشاوره کنکور",
    "تعمیر موبایل", "نظافت منزل", "اسباب کشی", "خیاطی", "کاشت ناخن", "عکاسی مراسم",
    "برنامه نویسی سایت", "پروژه دانشجویی", "نصب ویندوز", "ماساژ درمانی", "تدریس زبان", "رزومه نویسی",
]
PLACES = ["آپارتمان", "سوئیت", "خونه ویلایی", "مغازه", "زمین", "باغچه", "ویلا", "اتاق", "دفتر کار"]
PRICES = ["۲۰۰ تومن", "۵ میلیون", "۸۵۰ تومن", "۲ میلیون و نیم", "۱۲ تومن", "۹۹ هزار تومن", "۱۸ میلیون", "۳ تومن"]
PRICEY = ["قیمت توافقی", "زیر قیمت بازار", "نصف قیمت", "قیمتش مفته", "با تخفیف", "قیمت مناسب", "مقطوع", ""]
CTA = ["پیام بدید", "بیاید پیوی", "تماس بگیرید", "دایرکت بدید", "خصوصی پیام بده", "پی وی", "dm بدید", ""]
COND = ["در حد نو", "نو", "دست دوم تمیز", "کم کارکرد", "سالم", "اورجینال", "پلمپ", "بی رنگ", ""]
FRIEND = ["داداش", "رفیق", "عزیز", "خدایی", "دیگه", ""]
WHO = ["داداشم", "رفیقم", "همسایمون", "عموم", "یکی از بچه ها", "باباش", "همکارم", "خواهرم", "پسر همسایه"]
HANDLES = ["@ali_shop", "@seller_ir", "@uc_center", "@nail_home", "09121234567", "09351234567", "t.me/shop_x"]
CARDS = ["6037991812345670", "6104-3378-1234-5674", "6219861912345674", "۶۰۳۷۹۹۱۸۱۲۳۴۵۶۷۰"]

random_state = random.Random(SEED)


def fill(template: str, **slots: list[str]) -> list[str]:
    """Every combination of the slots, sampled down to PER_FAMILY."""
    keys = list(slots)
    combos: list[str] = []
    total = 1
    for key in keys:
        total *= len(slots[key])

    def render(values: dict[str, str]) -> str:
        text = template
        for key, value in values.items():
            text = text.replace("{" + key + "}", value)
        text = re.sub(r"\s+", " ", text.replace("، ،", "،").replace("،،", "،"))
        return text.strip(" ،").strip()

    if total <= PER_FAMILY:
        def walk(at: int, values: dict[str, str]):
            if at == len(keys):
                combos.append(render(values))
                return
            for value in slots[keys[at]]:
                walk(at + 1, {**values, keys[at]: value})

        walk(0, {})
        return list(dict.fromkeys(combos))
    for _ in range(PER_FAMILY):
        combos.append(render({key: random_state.choice(slots[key]) for key in keys}))
    return list(dict.fromkeys(combos))


Row = tuple[str, str]  # (frame, text)


def rows_of(frame: str, texts: list[str]) -> list[Row]:
    return [(frame, text) for text in texts]


# ---------------------------------------------------------------------------------------
# SELL families — every register a seller uses, by frame
# ---------------------------------------------------------------------------------------

def sell_rows() -> list[Row]:
    rows: list[Row] = []
    thing = GOODS + DIGITAL
    R = rows_of

    # offer — listings and advertisements
    rows += R("offer", fill("{t} فروشی {c}", t=thing, c=COND))
    rows += R("offer", fill("{t} فروشی، {p}", t=thing, p=PRICEY))
    rows += R("offer", fill("فروشی: {t} {c}", t=thing, c=COND))
    rows += R("offer", fill("{t} میفروشم {p}", t=thing, p=PRICEY))
    rows += R("offer", fill("می فروشم {t}، {cta}", t=thing, cta=CTA))
    rows += R("offer", fill("{g} میفروشم {p}", g=GAME, p=PRICEY + ["تحویل آنی", "ارزون تر از همه"]))
    rows += R("offer", fill("{t} {c} فروشی، {pr}، {cta}", t=thing, c=COND, pr=PRICES, cta=CTA))
    rows += R("offer", fill("فروش ویژه {t}، {p}، {cta}", t=thing, p=PRICEY, cta=CTA))
    rows += R("offer", fill("{t} {c}، {pr}، {h}", t=thing, c=COND, pr=PRICES, h=HANDLES))
    rows += R("offer", fill("{t} / {pr} / {h}", t=thing, pr=PRICES, h=HANDLES))
    rows += R("offer", fill("{g} {v}", g=GAME, v=["خربدارم", "خریدرام", "فورشی", "میفورشم", "میفرشم"]))
    rows += R("offer", fill("{v} {t}", t=thing, v=["فورشی", "میفورشم", "میفرشم"]))
    rows += R("offer", fill("این {t} فروشیه، {p}", t=thing, p=PRICEY))
    rows += R("offer", fill("{t} فروشیه ها، {cta}", t=thing, cta=["کسی خواست بگه", "پیام بدید", "جدی ها فقط"]))
    rows += R("offer", fill("{t}م فروشیه اگه {cond}", t=["اکانت", "گوشی", "ماشین", "دوچرخه"], cond=["قیمت خوب بدید", "خریدار باشه", "به قیمت بخرید"]))
    rows += R("offer", fill("{t} موجود شد، {cta}", t=thing, cta=CTA))
    rows += R("offer", fill("{t} رسید، تنوع کامل، {cta}", t=thing, cta=CTA))
    rows += R("offer", fill("شرایط اقساطی داریم برای {t}، {cta}", t=thing, cta=CTA))
    rows += R("offer", fill("موجودی {t} محدوده، {cta}", t=thing + GAME, cta=["برا رزرو پیام بدید", "سفارش تو پیوی"]))
    rows += R("offer", fill("{t} for sale, {p}, dm me", t=ENGLISH_GOODS + ["my account", "gaming pc"], p=["barely used", "best price", "cheap"]))
    rows += R("offer", fill("selling {t}, {pay}", t=["fresh accounts", "skins", "nitro", "usdt"], pay=["paypal accepted", "crypto ok", "good rate"]))
    rows += R("offer", fill("{t} forooshi, {p}", t=["goshi", "mashin", "laptop", "account", "seke"], p=["20 milion", "gheymat tavafoghi", "arzoon", "zire gheymat"]))
    rows += R("offer", fill("{t} miforosham {p}", t=["goshi", "account", "tether", "seke", "gem"], p=["arzoon", "naghdi", "payam bede", "nerkh khoob"]))
    # mixed script — the goods in English, the register in Persian and the other way round
    rows += R("offer", fill("{e} فروشی، {c}، {pr}، {cta}", e=ENGLISH_GOODS, c=COND, pr=PRICES, cta=CTA))
    rows += R("offer", fill("selling {t}، {pr}، پیام بدید", t=thing, pr=PRICES))
    rows += R("offer", fill("{e} میفروشم {p}", e=ENGLISH_GOODS, p=PRICEY))
    # slang shopfront
    rows += R("offer", fill("{t} رو دستم مونده، {price} بدید مال شما", t=thing, price=PRICES))
    rows += R("offer", fill("کی {t} میخواد؟ دارم میدم {pr}، {who}", t=thing + GAME, pr=PRICES, who=["جدی ها فقط", "بیاد پیوی", "خبر بده"]))
    rows += R("offer", fill("{t}م اضافیه، هرکی خواست {cta}، {p}", t=["گوشی", "دسته", "لپ تاپ", "بلیط"], cta=CTA, p=PRICEY))
    rows += R("offer", fill("بچه ها من میرم {where}، همه وسایلم فروشیه، {cta}", where=["خارج", "شهرستان", "خوابگاه"], cta=CTA))
    rows += R("offer", fill("{t} {c} میدم بره، {pr}", t=thing, c=COND, pr=PRICES))
    rows += R("offer", fill("سفارش {f} قبول میکنم، {p}", f=["کیک تولد", "غذای خونگی", "شیرینی عید", "ترشی خونگی", "لباس سفارشی"], p=PRICEY + ["ارسال رایگان"]))
    rows += R("offer", fill("{g} میزنم {p}، {cta}", g=["یوسی", "سی پی", "شارژ", "وی بی", "جم"], p=["ارزون", "زیر قیمت", "تحویل آنی"], cta=CTA))

    # want — committed buyers
    rows += R("want", fill("خریدارم {t}، {pay}", t=thing, pay=["نقدی", "پول آماده ست", "تا سقف ۲۰ میلیون", "فوری"]))
    rows += R("want", fill("{g} خریدارم {pay}", g=GAME, pay=["نقدی", "فوری", "به قیمت خوب", ""]))
    rows += R("want", fill("{t} خریداریم به بالاترین قیمت", t=["طلا", "ضایعات", "گوشی خراب", "کتاب دست دوم", "ماشین تصادفی", "سهام عدالت"]))
    rows += R("want", fill("نقدی میخرم {t}، {cta}", t=thing, cta=CTA))
    rows += R("want", fill("{t} میخرم {pay}، {cta}", t=thing + GAME, pay=["نقدی", "تا ۱۰ میلیون", "به قیمت روز"], cta=CTA))
    rows += R("want", fill("دنبال {t} {pay} ام، دارید خبر بدید", t=thing, pay=["نقدی", "زیر قیمت", "دست دوم تمیز"]))
    rows += R("want", fill("هرکی {t} داره خبرم کنه، {pay} میخرم", t=thing, pay=["نقدی", "فوری", "به قیمت خوب"]))
    rows += R("want", fill("kharidaram {t} {pay}", t=["goshi", "mashin", "tala", "seke", "laptop", "gem"], pay=["naghdi", "fori", "pool amadast", "ta 20 milion", ""]))
    rows += R("want", fill("{t} kharidaram {pay}", t=["seke", "goshi", "tala", "account"], pay=["naghdi", "fori", "gheymat khoob midam", ""]))
    rows += R("want", fill("خریدار جدی {t} هستم، {pay}", t=thing, pay=["سریع تسویه میکنم", "پولش آماده ست", "امروز فردا میخرم"]))
    rows += R("want", fill("{t} {pay} میخوام، {who} پیام بده", t=PLACES, pay=["رهن کامل", "اجاره", "برای خرید"], who=["مالک", "فقط مالک", "بنگاه نباشه"]))

    # exchange
    rows += R("exchange", fill("{g} طاق میزنم با {g2}", g=GAME, g2=GAME))
    rows += R("exchange", fill("معاوضه میکنم {t} با {t2}", t=thing, t2=thing))
    rows += R("exchange", fill("تبادل {g} با {g2} انجام میدم", g=GAME, g2=GAME))
    rows += R("exchange", fill("{t} میدم {t2} میگیرم، {extra}", t=thing + GAME, t2=thing + GAME, extra=["فرقشو میدم", "نرخ توافقی", "بدون پول"]))
    rows += R("exchange", fill("{t} رو با {t2} عوض میکنم، {cta}", t=thing, t2=thing, cta=CTA))
    rows += R("exchange", fill("حواله {cur} به {where} میزنم، نرخ توافقی", cur=["دلار", "یورو", "لیر"], where=["ترکیه", "دبی", "آلمان"]))
    rows += R("exchange", fill("tabadol mizanam {g} ba {g2}", g=["seke", "gem", "account clash", "skin"], g2=["uc", "gold", "account pubg", "tether"]))

    # service
    rows += R("service", fill("{s} انجام میدم، {p}، {cta}", s=SERVICES, p=PRICEY + ["هزینه توافقی", "تعرفه منصفانه"], cta=CTA))
    rows += R("service", fill("{s} قبول میکنم، {p}", s=SERVICES, p=PRICEY + ["اجرت جزئی"]))
    rows += R("service", fill("{s} با قیمت دانشجویی، {cta}", s=SERVICES, cta=CTA))
    rows += R("service", fill("{s}، {unit} {pr}، {cta}", s=SERVICES, unit=["جلسه ای", "ساعتی", "صفحه ای", "دقیقه ای"], pr=PRICES, cta=CTA))
    rows += R("service", fill("{s} {when}، وقت خالی دارم، {cta}", s=SERVICES, when=["این هفته", "آنلاین", "درب منزل"], cta=CTA))
    rows += R("service", fill("{s} می کنم، نمونه کار دارم، {cta}", s=SERVICES, cta=CTA))
    rows += R("service", fill("tadris {s} {how}, hamahangi to pv", s=["riazi", "zaban", "shatranj", "fizik"], how=["online", "hozoori", "jalase 300"]))

    # rental
    rows += R("rental", fill("{pl} اجاره داده میشود، {p}", pl=PLACES, p=PRICEY + ["ماهانه توافقی"]))
    rows += R("rental", fill("رهن و اجاره {pl}، {cta}", pl=PLACES, cta=CTA))
    rows += R("rental", fill("{pl} فروشی، {pr}", pl=PLACES, pr=["متری ۸ تومن", "سند تک برگ", "قیمت توافقی"]))
    rows += R("rental", fill("{t} اجاره میدم {unit} {pr}، {cta}", t=["دوربین", "پلی استیشن", "چادر", "لباس عروس", "ون", "اسکوتر"], unit=["روزانه", "هفته ای", "ساعتی"], pr=PRICES, cta=CTA))
    rows += R("rental", fill("اجاره {pl} {where}، {unit} {pr}", pl=["سوئیت مبله", "ویلا", "اتاق"], where=["مشهد", "شمال", "کیش", "تهران"], unit=["شبی", "روزانه", "ماهی"], pr=PRICES))

    # payment
    rows += R("payment", fill("بعد از واریز به کارت {card} رسید بفرستید", card=CARDS))
    rows += R("payment", fill("واریز فقط به {card} به نام {name}", card=CARDS, name=["محمدی", "رضایی", "احمدی"]))
    rows += R("payment", fill("{g} میزنم، پرداخت {pay}", g=["یوسی", "سی پی", "شارژ", "وی بی"], pay=["کارت به کارت", "با تتر", "درگاه"]))
    rows += R("payment", fill("{pr} کارت به کارت کنید {t} رو میفرستم، {card}", pr=PRICES, t=["اکانت", "کد", "فایل"], card=CARDS))
    rows += R("payment", fill("بیعانه {pr} به {card}، بعدش ارسال", pr=PRICES, card=CARDS))

    # indirect — the offer without the register
    rows += R("indirect", fill("{t} {who} دنبال صاحب جدیدشه، {cta}", t=thing, who=["م", " پسرم", " خودم"], cta=CTA))
    rows += R("indirect", fill("{t} {time} تو انباری خاک میخوره، یه پولی بدید مال خودتون", t=thing, time=["دو ساله", "یه ساله", "چند ماهه"]))
    rows += R("indirect", fill("{t} دارم، مفت نمیدم ولی باهاتون کنار میام", t=thing))
    rows += R("indirect", fill("{t} کادو گرفتم استفاده نکردم، {pr} بدید مال شما", t=thing, pr=PRICES))
    rows += R("indirect", fill("{t} رو رد میکنم، {p}، {cta}", t=thing + GAME, p=PRICEY, cta=CTA))
    rows += R("indirect", fill("{n} تا {t} رو دستم مونده، به قیمت خودش میدم", n=["دو", "سه", "چند"], t=["بلیط", "کد تخفیف", "کارت هدیه"]))
    rows += R("indirect", fill("{who} {t}شو گذاشته فروش، {pr}، بگید وصلتون کنم", who=WHO, t=["ماشین", "گوشی", "اکانت", "لپ تاپ"], pr=PRICES))
    rows += R("indirect", fill("{who} {t} {v}، {pr}، از طرف من پیام بدید بهش", who=WHO, t=thing, v=["میفروشه", "داره میفروشه"], pr=PRICES))
    rows += R("indirect", fill("{x} واگذار میشه، {cta}", x=["امتیاز وام", "وقت سفارت", "نوبت دکتر", "جای کلاس زبان", "سهمیه"], cta=CTA))
    rows += R("indirect", fill("هرکی {t} بخواد من دارم، {p}، {cta}", t=thing + GAME, p=PRICEY, cta=CTA))
    rows += R("indirect", fill("{t} {c} داریم چند دست، جمع کردیم، ارزون رد میکنیم", t=["میز و صندلی", "مبل", "قفسه", "لوازم مغازه"], c=["تمیز", "سالم"]))
    rows += R("indirect", fill("هر {what} که بخوای موجوده، لیست میدم، {cta}", what=["رنگ", "سایز", "مدل"], cta=CTA))
    rows += R("indirect", fill("{t} خودم رو {v}، عکسا تو پیوی، قیمتم مناسبه", t=["مانتوهای", "کیک های", "کارای رزینی", "زیورآلات"], v=["میدوزم", "میسازم", "درست میکنم"]))
    rows += R("indirect", fill("سلام بچه ها. راستی {t}م اضافیه، {pr}، {cta}", t=["گوشی", "لپ تاپ", "دوچرخه"], pr=PRICES, cta=CTA))
    rows += R("indirect", fill("نمیدونم اینجا مجازه یا نه ولی {t}م فروشیه، {pr}", t=["ماشین", "گوشی", "اکانت"], pr=PRICES))
    rows += R("indirect", fill("har ki {g} bekhad man daram, {p}, {cta}", g=["gem", "uc", "seke", "account", "goshi"], p=["arzoon tar az bazar", "nerkh khoob", "zire gheymat"], cta=["bia khosoosi", "pv bede", "dm"]))
    rows += R("indirect", fill("{g} {v} ارزون تر از همه، {cta}", g=["یوسی", "سی پی", "جم", "شارژ"], v=["میزنم", "میدم", "دارم"], cta=CTA))
    rows += R("indirect", fill("{who} {t} رو گذاشتیم فروش، {ask}", who=["خونه بابام", "ماشین داداشم", "مغازه رو"], t=["", "تو کرج", "تو شهرک"], ask=["کسی خریدار سراغ داره بگه", "خریدار جدی پیام بده", "قیمت توافقیه"]))
    rows += R("indirect", fill("{g} هامو رد میکنم {n} تا {pr} کی میخواد", g=["سکه", "جم", "یوسی"], n=["۱۰۰", "۵۰۰", "هزار"], pr=PRICES))
    rows += R("indirect", fill("{who} {t} دیگه، {cta} {what}", who=["بابا خودم", "من خودم"], t=["میفروشم", "دارم"], cta=["بیا پیوی", "پیام بده"], what=["نرخو بگم", "قیمت بدم", "عکس بفرستم"]))
    rows += R("offer", fill("{t} {pr}، پرداخت {pay} هم اوکیه", t=["نیترو discord یکساله", "gift card apple", "اکانت spotify", "vpn config"], pr=PRICES, pay=["crypto", "کارت به کارت", "paypal"]))
    return rows


# ---------------------------------------------------------------------------------------
# SAFE families — every register that merely *sounds* like commerce, by frame
# ---------------------------------------------------------------------------------------

def safe_rows() -> list[Row]:
    rows: list[Row] = []
    thing = GOODS + DIGITAL
    R = rows_of

    # beg / lend / request — the «سکه بده» incident's whole family
    rows += R("beg", fill("{f} {g} بده {f2}", f=["داداش", "جاسم", "علی", "یکی", ""], g=GAME + ["شارژ", "پول"], f2=FRIEND))
    rows += R("beg", fill("یکم {g} بده بازی کنیم", g=GAME))
    rows += R("beg", fill("{g} میخوام، کی میده؟", g=GAME))
    rows += R("beg", fill("{p} قرض بده تا {when}", p=["ده تومن", "صد تومن", "یکم پول"], when=["فردا", "شنبه", "آخر ماه"]))
    rows += R("beg", fill("{t} تو بده {do}", t=["گوشی", "دسته", "کارت", "جزوه", "شارژر"], do=["یه لحظه", "ببینم", "کارم تموم شه بدم"]))
    rows += R("beg", fill("یه {t} بده {do} فقط", t=["اکانت", "کد", "کانفیگ"], do=["تست کنم", "ببینم", "امتحان کنم"]))
    rows += R("beg", fill("torokhoda yeki ye {t} bede, pul nemidam ha, faghat az roo lotf", t=["pet", "seke", "gem", "account"]))

    # past transactions
    rows += R("past", fill("{t} مو {v}، {feel}", t=["گوشی", "ماشین", "لپ تاپ", "اکانت", "موتور", "دوچرخه"], v=["فروختم", "خریدم"], feel=["راضی ام", "پشیمونم", "راحت شدم", "دلم تنگ شده"]))
    rows += R("past", fill("{who} {t} شو فروخت", who=WHO, t=["ماشین", "خونه", "مغازه", "اکانت", "موتور"]))
    rows += R("past", fill("بالاخره {t} خریدیم بعد {time}", t=["خونه", "ماشین", "یخچال"], time=["سه سال", "کلی پس انداز", "عمری"]))
    rows += R("past", fill("{when} {t} خریدم {pr} دادم، {feel}", when=["دیروز", "هفته پیش", "پارسال"], t=thing, pr=PRICES, feel=["عالیه", "پشیمونم", "خیلی خوبه", "خراب شد"]))
    rows += R("past", fill("{t}مو {when} فروختم، الان قیمتش {x} شده", t=["موتور", "ماشین", "گوشی", "کارت گرافیک"], when=["پارسال", "دو سال پیش"], x=["سه برابر", "دو برابر", "نجومی"]))
    rows += R("past", fill("{g} خریدم واسه {who}، {what}", g=GAME, who=["پسرم", "داداشم", "خودم"], what=["دو روزه تموم کرد", "کلی ذوق کرد", "پولم حروم شد"]))
    rows += R("past", fill("dirooz {t} kharidam {pr} dadam, {feel}", t=["goshi", "headset", "laptop"], pr=["900", "5 milion", "2 toman"], feel=["aliye", "pashimoonam"]))
    rows += R("past", fill("i finally {v} my old {t}", v=["sold", "bought"], t=["bike", "phone", "laptop", "car"]))
    rows += R("past", fill("{t} {when} از {where} گرفتم، {feel}", t=thing, when=["دیروز", "عید"], where=["دیجی کالا", "بازار", "یه دوست"], feel=["راضی ام", "بد نبود", "گرون بود"]))
    rows += R("past", fill("{who} {t}شو داد به من، {how} 😁", who=WHO, t=["اکانت", "گوشی", "دوچرخه", "کتابا"], how=["مفتی افتاد دستم", "دیگه لازمش نداشت", "کادو داد"]))
    rows += R("past", fill("{pl} رو {v} و {then}", pl=["خونه", "مغازه", "زمین"], v=["رهن دادیم", "اجاره دادیم", "فروختیم"], then=["اومدیم شهرستان، راحت تریم", "پولشو خرج عمل بابام کردیم", "دیگه دردسر نداریم"]))

    # questions and appraisal — inquiry
    rows += R("inquiry", fill("{t} الان چند {v}؟", t=thing, v=["می ارزه", "قیمتشه", "در میاد"]))
    rows += R("inquiry", fill("به نظرت {t} رو بفروشم یا نه؟", t=["ماشینم", "گوشیم", "اکانتم", "خونه"]))
    rows += R("inquiry", fill("این {t} ارزش خرید داره؟", t=thing))
    rows += R("inquiry", fill("{t} از کجا خریدی؟ {like}", t=thing, like=["خیلی خوبه", "چقدر قشنگه", "منم میخوام"]))
    rows += R("inquiry", fill("کدوم {t} بخرم به نظرتون؟", t=["گوشی", "لپ تاپ", "ماشین", "بازی"]))
    rows += R("inquiry", fill("کسی {t} {v}؟", t=thing + GAME, v=["میفروشه", "دست دوم میفروشه", "سراغ داره", "داره بفروشه"]))
    rows += R("inquiry", fill("کجا میتونم {t} {how} بخرم؟", t=thing + GAME, how=["ارزون", "اورجینال", "مطمئن", ""]))
    rows += R("inquiry", fill("قیمت {t} الان چند شده؟ {ask}", t=thing + GAME, ask=["کسی خبر داره", "", "خیلی وقته نپرسیدم"]))
    rows += R("inquiry", fill("{t} از کجا میخرید که {bad}؟", t=["تتر", "یوسی", "گوشی", "بلیط"], bad=["کلاه سرتون نره", "تقلبی نباشه", "گرون نباشه"]))
    rows += R("inquiry", fill("کسی {shop} معتبر واسه {t} سراغ داره؟", shop=["فروشگاه", "سایت", "پیج"], t=thing))
    rows += R("inquiry", fill("{t} {v} چقدره این روزا؟", t=["اجاره خونه", "کاشت ناخن", "تدریس خصوصی", "بلیط هواپیما"], v=["قیمتش", "هزینه ش", "نرخش"]))
    rows += R("inquiry", fill("{t} بخرم یا {t2}؟ {ask}", t=["ماشین دست دوم", "آیفون", "لپ تاپ"], t2=["صفر قسطی", "سامسونگ", "تبلت"], ask=["نظرتون چیه", "گیج شدم", ""]))
    rows += R("inquiry", fill("چند میدی این {t} رو؟ {why}", t=["گوشی", "ساعت", "دوچرخه"], why=["فقط برا اطلاع میپرسم", "کنجکاوم", "میخوام بدونم قیمت روز چنده"]))
    rows += R("inquiry", fill("kasi midoone koja {t} arzoon peyda mishe?", t=["goshi", "laptop", "uc", "bilit"]))
    rows += R("inquiry", fill("{t} چطور {v}؟ از داخل ایران میشه؟", t=["اکانت پرمیوم", "بازی استیم", "گیفت کارت"], v=["بخرم", "تهیه کنم"]))
    rows += R("inquiry", fill("نرخ {x} امروز چنده؟", x=["دلار", "تتر", "سکه", "طلا", "یورو"]))
    rows += R("inquiry", fill("کسی تجربه خرید از {where} داره؟", where=["ترب", "دیجی کالا", "دیوار", "این پیج", "سایت خارجی"]))
    rows += R("inquiry", fill("کدوم {t} بخرم {why}؟ بودجه {pr}", t=["لپ تاپ", "گوشی", "تبلت", "دوربین", "موتور"], why=["واسه برنامه نویسی", "واسه بازی", "واسه دانشگاه", "واسه شهر", ""], pr=PRICES))
    rows += R("inquiry", fill("با {pr} چه {t}ی میشه خرید؟ {ask}", pr=PRICES, t=["گوشی", "لپ تاپ", "ماشین"], ask=["پیشنهاد بدید", "نظرتون چیه", ""]))
    rows += R("inquiry", fill("{t} خوب زیر {pr} چی هست؟ {ask}", t=["گوشی", "هدفون", "ساعت هوشمند", "کیبورد"], pr=PRICES, ask=["تجربه دارید؟", "معرفی کنید", ""]))
    rows += R("inquiry", fill("به نظرتون {t} چقد می ارزه؟ {dis}", t=["اکانت لول ۸۰ فری فایر", "گوشی دو ساله م", "ماشین مدل ۹۰", "این ساعت"], dis=["نمیفروشما، فقط سوالمه", "فقط کنجکاوم", "قصد فروش ندارم"]))
    rows += R("inquiry", fill("{state}، دنبال {t} ایم این روزا، {ask}", state=["اجاره خونمون تموم شده", "صاحبخونه جواب کرده", "جابجا شدیم"], t=["خونه", "یه جای ارزون", "خونه نزدیک مترو"], ask=["", "کسی بنگاه خوب سراغ داره؟", "خدا به دادمون برسه"]))
    rows += R("inquiry", fill("کجا میشه {t} {c} پیدا کرد؟ {bad}", t=["شارژر تایپ سی", "کیس کامپیوتر", "قطعه یدکی"], c=["اورجینال", "خوب", "ارزون"], bad=["هرچی گرفتم تقلبی دراومد", "همه جا گرونه", ""]))
    rows += [("inquiry", t) for t in [
        "موجودی", "موجودی؟", "موجودی داری؟", "موجوده؟", "قیمت؟", "قیمتش؟", "چند؟",
        "چنده؟", "هست هنوز؟", "ارسال داری؟", "تخفیف نداره؟", "نقدی چند میشه؟", "فروشی؟",
        "موجودی حسابم صفر شد", "موجودیمو چک کن برام", "این فروشیه؟", "اینا فروشیه یا نمایشی؟",
    ]]

    # economy news with prices — the +31.5 incident
    rows += R("news", fill("{x} رسید به {p}، {react}", x=["دلار", "سکه", "طلا", "یورو", "بنزین"], p=PRICES, react=["باورتون میشه؟", "عجب روزگاری", "کجا میریم آخه"]))
    rows += R("news", fill("قیمت {x} باز رفت بالا", x=["دلار", "سکه", "خونه", "گوشی", "اجاره ها"]))
    rows += R("news", fill("میگن {x} قراره {v}", x=["قیمت گوشی", "دلار", "اجاره ها"], v=["بیاد پایین", "گرون شه", "دو برابر شه"]))
    rows += R("news", fill("قبض {x} این ماه {p} اومده", x=["برق", "گاز", "آب"], p=PRICES))
    rows += R("news", fill("خبر: {who} {what}", who=["بانک مرکزی", "ایران خودرو", "وزارت صمت", "دیجی کالا", "مجلس"], what=["فروش ارز به مسافران را محدود کرد", "پیش فروش محصولات را متوقف کرد", "حراج بزرگ پاییزه را اعلام کرد", "طرح مالیات بر خرید و فروش سکه را تصویب کرد", "فروش اجباری کالا را تخلف اعلام کرد"]))
    rows += R("news", fill("طبق {src}، {what}", src=["آمار", "گزارش مرکز آمار", "خبرگزاری"], what=["معاملات مسکن ۴۰ درصد کاهش داشته", "۲۰ درصد خانوارها طلای خود را فروخته اند", "تورم خوراکی ها به ۵۰ درصد رسید", "کلاهبرداری با فروش اکانت افزایش یافته"]))
    rows += R("news", fill("بازار {x} {state} میگن، {who} {feel}", x=["موبایل", "خودرو", "طلا", "مسکن"], state=["خیلی کساده", "خوابیده", "قفل شده"], who=["فروشنده ها", "دلالا", "مغازه دارا"], feel=["مینالن", "بیکار نشستن", "دارن جمع میکنن"]))
    rows += R("news", fill("با این {x} مگه میشه چیزی خرید؟", x=["حقوق", "قیمتا", "تورم", "وضع"]))
    rows += R("news", fill("{x} {when} {v} درصد {dir}", x=["بورس", "طلا", "بیت کوین", "دلار"], when=["امروز", "این هفته"], v=["۲", "۵", "۱۰"], dir=["ریخت", "رشد کرد", "بالا رفت"]))
    rows += R("news", fill("{x} تو {where} {state}", x=["اجاره خونه", "قیمت مسکن", "کرایه ها", "اجاره مغازه"], where=["تهران", "کرج", "شمال"], state=["دیوونه کننده شده", "نجومیه", "دو برابر شده"]))
    rows += R("news", fill("{x} نجومی شده، یه {t} {pr} {v} میخواد", x=["اجاره ها", "رهن و اجاره", "قیمتا"], t=["اتاق", "سوئیت", "زیرزمین", "خونه ۵۰ متری"], pr=PRICES, v=["رهن", "پیش", "ودیعه"]))
    rows += R("news", fill("{who} {where} اصلا انصاف ندارن، {what}", who=["فروشنده های", "مغازه دارای", "دلالای"], where=["پاساژ علاءالدین", "بازار", "چهارراه"], what=["هر چی بپرسی سه برابر قیمت میگن", "قیمت ندارن", "با هم هماهنگن"]))
    rows += R("news", fill("gheymate {x} chand shod emrooz? {react}", x=["dolar", "tala", "seke", "tether"], react=["", "kheyli raft bala", "kasi midoone"]))

    # scam warnings and third-party ads — quote
    rows += R("quote", fill("اون {who2} که {g} میفروخت {bad}", who2=["یارو", "کاناله", "پیجه"], g=GAME + ["گوشی", "سیگنال"], bad=["کلاهبردار بود", "اسکم بود", "بلاک شد", "پولا رو خورد"]))
    rows += R("quote", fill("این کانالای فروش {t} همش {bad}", t=["اکانت", "گوشی", "ارز"], bad=["فیکن", "کلاهبرداریه", "دزدین"]))
    rows += R("quote", fill("تو {app} یه آگهی {t} دیدم، مشکوک بود", app=["دیوار", "شیپور", "گروه بغلی"], t=thing))
    rows += R("quote", fill(
        "{intro} «{ad}» {warn}",
        intro=["این پیامو ببینید:", "این اسکرینشاتو ببینید، نوشته", "طرف اومده پیوی میگه", "تو گروه بغلی زده", "کپی پیامش:", "ادمین این پیام"],
        ad=["گوشی میفروشم زیر قیمت", "تتر میفروشم نرخ عالی", "اکانت فروشی ارزون", "فالوور میفروشم", "سکه میفروشم تحویل آنی", "یوسی ۶۰ تایی ۲۰ تومن فقط امروز", "آیفون ۱۳ فقط ۱۰ میلیون پیام بدید", "واریز به کارت 6037991812345670"],
        warn=["گزارشش کنید، کلاهبرداره", "بلاکش کردم", "صددرصد اسکمه", "گولشو نخورید", "مواظب باشید", "رو پاک کن، تبلیغ کلاهبرداریه", "بعد از واریز جواب نداد"],
    ))
    rows += R("quote", fill(
        "{intro} {ad} {warn}",
        intro=["طرف اومده پیوی میگه", "یکی نوشته", "تو کانال زده بود", "یارو تو پیوی نوشته", "پیامش این بود:", "همون که میگفت"],
        ad=["میفروشم ارزون", "تتر میخرم بالاتر از بازار", "اکانت فروشی", "گیفت کارت ارزون دارم", "لپ تاپ گیمینگ زیر قیمت میفروشم", "ماشین فروشی ۱۰۰ تومن پراید صفر"],
        warn=["بلاکش کردم", "بعد بلاک کرد", "خب معلومه اسکمه", "رو دیشب از گروه انداختن بیرون", "و آیدیشم فیک بود", "همه رو پیوی زده، بنش کنید"],
    ))
    rows += R("quote", fill("{ex}: «{ad}» — {rule}", ex=["مثال آگهی کلاهبرداری", "نمونه پیام اسکم", "این شکلی رو رد کنید"], ad=["یوسی ۶۰ تایی ۲۰ تومن فقط امروز", "ارز دیجیتال با سود تضمینی", "کارت به کارت کن ارسال میکنم", "اکانت پرمیوم رایگان فقط شماره کارت بدید"], rule=["هر وقت خیلی ارزون بود شک کنید", "یعنی کلاهبرداری", "چقدر تابلو"]))
    rows += R("quote", fill("مدرک دارم که نوشته بود «{ad}» و بعد {bad}", ad=["واریز به کارت 6104-3378-1234-5674", "اول بیعانه بریز", "تحویل آنی"], bad=["بلاک کرد", "غیبش زد", "جواب نداد"]))
    rows += R("quote", fill("{who} {said} {t}شو {v}", who=WHO, said=["میگفت", "گفت", "میگه"], t=["گوشی", "ماشین", "مغازه", "اکانت"], v=["فروخته", "میفروشه", "خریدار پیدا شده"]))
    rows += R("quote", fill("شنیدم {who} {g} میفروشه، {then}", who=["یکی از بچه ها", "پسر همسایه", "یه نفر تو گروه"], g=GAME, then=["اگه ادمین ببینه بن میشه ها", "باباش خبر نداره", "به نظرتون واقعیه؟"]))
    rows += R("quote", fill("mavazebe oon {who} bashin, {g} miforookht, {bad}", who=["yaroo", "channel", "page"], g=["seke", "tether", "account"], bad=["scammere", "pul migire chizi nemide", "block konid"]))

    # negations, refusals, rhetorical
    rows += R("refusal", fill("{t} مو نمیفروشم {emph}", t=["ماشین", "گوشی", "اکانت", "خونه"], emph=["عمرا", "به هیچ قیمتی", "فعلا"]))
    rows += R("refusal", fill("این {t} فروشی نیست {emph}", t=["ماشین", "خونه", "وسیله", "گوشی", "سگ"], emph=["داداش", "گفتم که", "", "ا، پیوی ندین قیمت بپرسین", "صد دفعه گفتم"]))
    rows += R("refusal", fill("مگه من {v}؟ {emph}", v=["میفروشم", "طاق میزنم", "معامله میکنم"], emph=["عمرا", "کی گفته", ""]))
    rows += R("refusal", fill("عمرا {t} مو بفروشم", t=["اکانت", "ماشین", "کلکسیونم"]))
    rows += R("refusal", fill("نه میخرم نه میفروشم، {why}", why=["فقط اومدم حرف بزنم", "فقط راجع به گیم حرف میزنم", "چیزی ندارم", "پول ندارم"]))
    rows += R("refusal", fill("فکر کردی {t} رو میفروشمش؟ {why}", t=["گوشیم", "ماشینم", "این"], why=["یادگاری بابامه", "عمرا", "نه بابا"]))
    rows += R("refusal", fill("من چیزی نمیفروشم، {why}", why=["فقط عکسشو گذاشتم", "اشتباه شده", "قیمت نپرسید"]))
    rows += R("refusal", fill("{no} {who} نیستم، {why}", no=["اصلا", "من"], who=["خریدار", "فروشنده"], why=["فقط کنجکاو بودم", "فقط دارم نگاه میکنم", "قیمتشو پرسیدم همین"]))
    rows += R("refusal", fill("مگه {t} رو میفروشی که {ask}؟", t=["ماشینتو", "گوشیتو", "اکانتتو"], ask=["قیمت میپرسی", "عکس گذاشتی", "سوال میکنی"]))
    rows += R("refusal", fill("{t} {v}؟ نه بابا {j}", t=["گوشیم", "اکانتم"], v=["فروشیه", "میفروشم"], j=["شوخی کردم", "کی گفته", "عمرا"]))

    # jokes and idioms
    rows += R("joke", fill("خودمو میفروشم به {x}", x=["یه لیوان چای", "یه پیتزا", "تعطیلات", "یه روز خواب"]))
    rows += R("joke", fill("{who} ما رو فروخت رفت {where}", who=["رفیقمون", "داداشمون"], where=["با بقیه", "پی زندگیش", ""]))
    rows += R("joke", fill("حوصلم {v} از {x}", v=["طاق شده", "سر رفته"], x=["این وضع", "درس", "بیکاری"]))
    rows += R("joke", fill("{x} طاقت ندارم دیگه", x=["", "خدایی", "به خدا"]))
    rows += R("joke", fill("{x} مگه فروشیه که {y}؟ 😂", x=["عکس پروفایلم", "دلم", "قیافم"], y=["هی قیمت میپرسین", "همه میخوانش", "پیوی میدین"]))
    rows += R("joke", fill("{body} مو میفروشم واسه {t} :)) {j}", body=["کلیه", "نصف عمر", "روح"], t=["آیفون", "ps5", "یه سفر"], j=["شوخی کردم", "شوخی بود بابا", ""]))
    rows += R("joke", fill("کی {v} منو؟ {why} 😂", v=["میخره", "میبره"], why=["خسته ام", "بیکارم", "ارزونم"]))
    rows += R("joke", fill("{x}تو به {y} نفروش {who}", x=["آخرت", "رفاقت", "خود"], y=["دو تا لایک", "یه پست", "دنیا"], who=["داداچ", "داداش", ""]))
    rows += R("joke", fill("{p} — {who}", p=["مفت باشه کوفت باشه", "بهشت را به بها دهند نه به بهانه", "یوسفو به بهای اندک فروختن"], who=["شعار خاندان ما", "حکایت ماست", "همینو بچسب"]))
    rows += R("joke", fill("{t}مو تو بازی {v}، ورشکست شدم 😂", t=["سکه ها", "جم ها", "همه پولا"], v=["باختم", "حروم کردم"]))
    rows += R("joke", fill("تو دلمو {v}، دیگه چی میخوای بخری؟ 😍", v=["خریدی", "بردی"]))
    # The paradox: the register's own words, each cancelled in the same breath. It is a joke
    # about being broke, and the battery measured it at +34 as a wanted-post.
    rows += R("joke", fill("{r1} ولی {l1}، {r2} ولی {l2}، {end}", r1=["خریدارم", "مشتری ام", "دنبال خونه ام"], l1=["پول ندارم", "جیبم خالیه", "حقوق ندارم"], r2=["فروشنده ام", "میفروشم", "کاسبم"], l2=["جنس ندارم", "مشتری ندارم", "مغازه ندارم"], end=["اینه زندگی", "خلاصه وضع ما 😂", "همینه دیگه"]))
    rows += R("joke", fill("{v} ولی {lack} 😂 {end}", v=["خریدارم", "میخرم", "میفروشم", "معامله میکنم"], lack=["پول ندارم", "جنس ندارم", "حسش نیست", "کارت خالیه"], end=["", "شوخی کردم", "زندگی همینه"]))
    rows += R("joke", fill("من {v} {what}، فقط {but} 😂", v=["میخرم", "میفروشم"], what=["همه چیو", "هر چی بگی"], but=["پول نیست", "جنس نیست", "خریدار نیست"]))

    # football transfers
    rows += R("joke", fill("{team} {v} {pos}", team=["پرسپولیس", "استقلال", "بارسا", "رئال"], v=["دنبال خرید", "میخواد بفروشه", "فروخت"], pos=["مهاجم", "مدافع", "دروازه بان جدید"]))
    rows += R("joke", fill("این بازیکنو باید {v}", v=["بفروشن", "همین الان بفروشن", "نگه دارن"]))
    rows += R("joke", fill("{team} {who} رو {pr} خرید، {react}", team=["چلسی", "رئال", "سیتی"], who=["امباپه", "این مدافعو", "یه جوون"], pr=["۱۰۰ میلیون یورو", "گرون"], react=["دیوونه شدن", "نیمکت نشین شد"]))

    # group rules and admin talk — meta
    rows += R("meta", fill("قانون گروه: {x} ممنوع", x=["خرید و فروش", "تبلیغ و فروش", "آگهی", "هرگونه فروش"]))
    rows += R("meta", fill("اینجا جای {x} نیست، {warn}", x=["تبلیغ", "خرید و فروش", "آگهی فروش"], warn=["اخطار میدم", "بن میشید", "قوانین رو بخونید"]))
    rows += R("meta", fill("{ask} قفل خرید و فروش رو {v}، {why}", ask=["ادمین", "بچه ها", ""], v=["بردارید", "روشن کنید", "کم کنید", "چک کنید"], why=["پیامای عادی هم پاک میکنه", "گروه پر از آگهی شده", "زیادی حساسه", "ما که گروه بازی هستیم"]))
    rows += R("meta", fill("ربات {v} {why}", v=["پیاممو پاک کرد", "پیام منو حذف کرد"], why=["من که چیزی نمیفروختم", "فقط قیمت پرسیده بودم", "درباره بازار حرف میزدم"]))
    rows += R("meta", fill("{x} تو گروه {v}، {where}", x=["پیام فروش", "آگهی", "تبلیغ"], v=["نذارید", "ممنوعه"], where=["برای این کار گروه جدا داریم", "فقط تو کانال آگهی ها", "ادمین بن میکنه"]))
    rows += R("meta", fill("این قفل خرید و فروش {q}؟", q=["چطور کار میکنه", "با کلمه تشخیص میده یا معنا", "چرا فعاله", "حساسیتش چنده"]))
    rows += R("meta", fill("{who} واسه آگهی فروش {v}", who=["چند نفر", "یکی", "دو نفر"], v=["بن شدن", "اخطار گرفتن", "پاک شدن امروز"]))
    rows += R("meta", fill("ربات با کلمه «{w}» حساسه، {warn}", w=["فروشی", "قیمت", "میفروشم"], warn=["مواظب باشید چی مینویسید", "الکی نگید"]))
    rows += R("meta", fill("این گروه واسه {x} نیست، واسه {y}", x=["خرید و فروش", "آگهی", "تبلیغ"], y=["بحث فنیه", "بازیه", "درسه"]))
    rows += R("meta", fill("{who} نمیدونستم اینجا فروش ممنوعه، {sorry}", who=["من", "ببخشید"], sorry=["پاکش کردم", "دیگه نمیذارم", "ببخشید"]))
    rows += R("meta", fill("تست قفل: این پیام {v} چون فروش نیست", v=["نباید پاک بشه", "باید بمونه"]))
    rows += R("meta", fill("کلمات ممنوعه: {w} — {warn}", w=["فروشی، میفروشم، خریدارم", "قیمت، فروش، خرید"], warn=["اینا رو ننویسید ربات پاک میکنه", "حواستون باشه"]))
    rows += R("meta", fill("{words}: هر آگهی که {this} داره {lie}", words=["تحویل آنی، ارسال رایگان، تضمین کیفیت", "زیر قیمت بازار، فقط امروز", "پیام بدید، تماس بگیرید، دایرکت", "نقدی، فوری، پول آماده"], this=["این سه تا رو", "اینا رو", "این جمله رو"], lie=["دروغه", "کلاهبرداریه", "رو رد کنید", "مشکوکه"]))
    rows += R("meta", fill("{words} — {what}", words=["پیام بدید تماس بگیرید دایرکت بدید", "فروشی فروشی فروشی", "زیر قیمت، تخفیف، حراج"], what=["چقدر این جمله ها تو گروه تکرار میشه", "کلمات مورد علاقه اسپمرها", "هر روز همینو میبینیم"]))
    rows += R("meta", fill("نمونه یه آگهی {q}: {tip}", q=["خوب", "درست", "معتبر"], tip=["عکس واضح، قیمت مشخص، بدون اغراق", "شماره تماس واقعی و آدرس", "بدون تحویل آنی و تضمین الکی"]))

    # advice and imperatives to others
    rows += R("advice", fill("{t}تو بفروش {why}", t=["ماشین", "گوشی", "اکانت"], why=["تا قیمت نیومده پایین", "دیگه ارزش نگه داشتن نداره", "پولشو بذار طلا"]))
    rows += R("advice", fill("الان {v}، {why}", v=["نخر", "نفروش", "صبر کن"], why=["بعد عید ارزون میشه", "بازار خرابه", "خریدار خوب پیدا میشه"]))
    rows += R("advice", fill("{no} {v}، {why}", no=["زیر قیمت", "به این قیمت", "الکی", "عجله ای"], v=["نده", "نفروش", "نخر", "رد نکن"], why=["صبر کن خریدار خوب پیدا میشه", "حیفه", "بازار میاد بالا", "پشیمون میشی"]))
    rows += R("advice", fill("{t}تو {v} {tip}", t=["گوشی", "ماشین", "اکانت", "خونه"], v=["نفروش", "نگه دار", "دست نزن"], tip=["فعلا", "تا بازار خوب شه", "ارزشش بیشتر میشه", "یادگاریه"]))
    rows += R("advice", fill("اگه میخوای {t} بخری {tip}", t=thing, tip=["برو مرکز شهر، ارزون تره", "حتما گارانتی رو چک کن", "از مغازه معتبر بگیر"]))
    rows += R("advice", fill("از هیچکی {t} نخرید مگه {ok}", t=["تتر", "یوسی", "گوشی", "بلیط"], ok=["از صرافی معتبر", "از سایت رسمی", "حضوری"]))
    rows += R("advice", fill("هیچ وقت {no}، {rule}", no=["اول پول نده", "بیعانه نریز", "به آگهی ارزون اعتماد نکن"], rule=["بعد جنس بگیر", "کلاهبرداریه", "تجربه ست"]))
    rows += R("advice", fill("اگه میخوای بفروشی {where} آگهی بذار، {here}", where=["تو دیوار", "تو شیپور", "تو کانال آگهی"], here=["اینجا جاش نیست", "نه اینجا"]))

    # tutorials and how-to
    rows += R("tutorial", fill("چطور {where} آگهی فروش بذارم؟ {q}", where=["تو دیوار", "تو شیپور", "تو اینستا"], q=["گزینه ش رو پیدا نمیکنم", "راهنمایی کنید"]))
    rows += R("tutorial", fill("آموزش: برای {v} {t} اول {step}", v=["فروش", "خرید"], t=["اکانت", "ماشین", "خونه"], step=["ایمیل رو عوض کنید", "برگ سبز رو آماده کنید", "قیمت بازار رو در بیارید"]))
    rows += R("tutorial", fill("راهنمای خرید از {where} با {how}", where=["سایت های خارجی", "استیم", "آمازون"], how=["کارت ایرانی", "تتر", "گیفت کارت"]))
    rows += R("tutorial", fill("برای کارت به کارت {step}", step=["از اپ بانک وارد بخش انتقال وجه بشید", "رمز دوم لازمه", "سقف روزانه ۱۰ تومنه"]))
    rows += R("tutorial", fill("چطور بفهمم آگهی {t} کلاهبرداریه؟ {q}", t=["فروش", "اجاره", "اکانت"], q=["نشونه هاش چیه", "از کجا چک کنم"]))
    rows += R("tutorial", fill("مرحله {n}: {step}", n=["اول", "دوم", "سوم"], step=["قیمت بازار رو در بیار", "عکس خوب بگیر", "آگهی بزن", "با خریدار حضوری قرار بذار"]))

    # third-person narration
    rows += R("third", fill("{who} داره {t}شو میفروشه، {feel}", who=WHO, t=["ماشین", "خونه", "اکانت", "مغازه"], feel=["دلم گرفته", "ناراحتم", "مجبوره", "بازار خرابه"]))
    rows += R("third", fill("{who} {job}، {state}", who=WHO, job=["سمساره", "مشاور املاکه", "تو کار واردات لوازم یدکیه", "آنلاین شاپ داره", "مغازه لوازم خانگی داره"], state=["خونش پر از عتیقه ست", "میگه بازار خوابیده", "همیشه سفره", "شبا تا دیر وقت بسته بندی میکنه", "همش سر کاره"]))
    rows += R("third", fill("{who} {t} {v} به بچه ها، {then}", who=["پسر همسایه", "یکی تو مدرسه"], t=GAME, v=["میفروشه", "میده"], then=["باباش خبر نداره", "ولی من نخریدم"]))
    rows += R("third", fill("یه {who} سر کوچه {t} میفروشه، {feel}", who=["پیرمرد", "پسر بچه", "خانم"], t=["سبزی", "بلال", "گل"], feel=["همیشه ازش میخرم", "دلم میسوزه", "خدا خیرش بده"]))
    rows += R("third", fill("{who} {t}شو فروخت {pr}، الان {now}", who=WHO, t=["اکانت", "ماشین", "گوشی"], pr=PRICES, now=["دوباره میخواد بازی کنه", "پشیمونه", "پیاده ست"]))
    rows += R("third", fill("{corp} داره {what}، {then}", corp=["شرکت ما", "شرکت بابام", "کارخونه"], what=["یکی از دفتراشو میفروشه", "دستگاه ها رو میفروشه"], then=["شاید تعدیل کنن", "وضع خرابه"]))

    # jobs
    rows += R("job", fill("{who} دنبال کار میگرده، {ask}", who=WHO, ask=["جایی سراغ دارید؟", "رزومه شو بفرستم؟"]))
    rows += R("job", fill("{corp} {role} میخواد، {terms}", corp=["شرکت ما", "یه استارتاپ", "دفتر ما"], role=["برنامه نویس", "منشی خانم", "کارگر ساده", "نیروی فروش", "کارآموز طراحی"], terms=["حقوق توافقی، رزومه بفرستید", "با بیمه", "پورسانت عالی", "بدون حقوق، با گواهی", "ساعت کاری ۸ تا ۴"]))
    rows += R("job", fill("{what} فردا دارم، {ask}", what=["مصاحبه کاری", "جلسه با HR"], ask=["دعا کنید", "استرس دارم"]))
    rows += R("job", fill("حقوقمو {v}، {then}", v=["سه ماهه ندادن", "دو برابر کردن", "کم کردن"], then=["چیکار کنم؟", "خوشحالم", "استعفا میدم"]))
    rows += R("job", fill("کسی کار {kind} واسه {who} سراغ داره؟", kind=["پاره وقت", "دورکاری", "شبانه"], who=["دانشجو", "خانم", "بازنشسته"]))
    rows += R("job", fill("{role} نیازمندیم {terms}، {contact}", role=["نیروی فروش", "فروشنده خانم", "بازاریاب", "کارشناس فروش", "صندوقدار"], terms=["با پورسانت عالی", "با حقوق ثابت", "تمام وقت", "با بیمه"], contact=["تماس با دفتر", "رزومه بفرستید", "مراجعه حضوری"]))
    rows += R("job", fill("{role} {v}، {terms}", role=["کارآموز طراحی سایت", "کارآموز حسابداری", "منشی", "نیروی انبار"], v=["میپذیریم", "استخدام میکنیم", "نیاز داریم"], terms=["بدون حقوق، با گواهی", "حقوق اداره کار", "ساعت کاری ۸ تا ۴"]))

    # charity and donations, cards outside a trade
    rows += R("charity", fill("برای {x} کمک جمع میکنیم، {ask}", x=["سیل زده ها", "زلزله زده ها", "خیریه"], ask=["هر چقدر تونستید", "دستتون درد نکنه"]))
    rows += R("charity", fill("برای {x} کمک جمع میکنیم، شماره کارت {card} به نام {name}", x=["سیل زده ها", "عمل بچه همسایه", "خیریه مهر", "زلزله زدگان"], card=CARDS, name=["خیریه", "مسئول جمع آوری", "هلال احمر"]))
    rows += R("charity", fill("{what} رو {who} دادم، هرکی سهمشو میده کارتم اینه {card}", what=["قبض گروه", "پول شام", "هزینه اردو"], who=["من", "خودم"], card=CARDS))
    rows += R("charity", fill("{what} نفری {pr}، به کارت {who} واریز کنید", what=["هزینه سفر اردو", "شارژ ساختمون", "پول کادوی تولد"], pr=PRICES, who=["مسئول اردو", "مدیر ساختمون", "من"]))
    rows += R("charity", fill("شماره شبا {who}: IR{iban} {thanks}", who=["خیریه", "انجمن"], iban=["120170000000123456789012", "580120000000987654321012"], thanks=["دستتون درد نکنه", "خدا خیرتون بده"]))
    rows += R("charity", fill("{x} امسال، هرکی خواست شریک شه پیام بده، {pr}", x=["نذری", "قربونی", "افطاری"], pr=["پولش نفری ۱۰۰", "هر چقدر تونستید"]))

    # lost and found
    rows += R("lost", fill("{t} پیدا شده تو {where}، صاحبش پیام بده", t=["کیف پول", "سوییچ", "کارت بانکی", "گوشی", "مدارک"], where=["پارک", "تاکسی", "محوطه", "کلاس"]))
    rows += R("lost", fill("{t} گم کردم {where}", t=["کارتمو", "سوییچمو", "کیفمو", "گوشیمو"], where=["تو مترو", "دیروز", "خدا میدونه کجا", "تو تاکسی، مژدگانی میدم"]))
    rows += R("lost", fill("کارتمو گم کردم {card} اگه کسی پیدا کرد خبر بده", card=CARDS))
    rows += R("lost", fill("{t} گمشده پیدا شده به شرط شیرینی تحویل داده میشود", t=["مدارک", "کیف", "گوشی"]))

    # complaints about orders, delivery, payment
    rows += R("complaint", fill("همه {g} هامو {v}", g=GAME, v=["باختم", "حروم کردم", "از دست دادم"]))
    rows += R("complaint", fill("{x} تموم شد وسط {y}", x=["شارژم", "گیم تایمم", "اینترنتم"], y=["بازی", "حرف زدن", "کلاس"]))
    rows += R("complaint", fill("{x} باز {bad}", x=["درگاه بانک", "سامانه دولتی", "کارتخوان مغازه", "اپ بانک"], bad=["خطا میده", "قطعه", "بالا نمیاد", "خرابه"]))
    rows += R("complaint", fill("موقع پرداخت {x} ارور داد", x=["قبض", "قسط", "شهریه"]))
    rows += R("complaint", fill("سفارش {what} {bad}، دیگه از {where} سفارش نمیدم", what=["غذامون", "لباسم", "کتابم"], bad=["دو ساعته نیومده", "اشتباه اومد", "خراب رسید"], where=["این اپ", "این سایت", "این پیج"]))
    rows += R("complaint", fill("{promise} نوشته بود ولی {but}", promise=["ارسال رایگان", "تخفیف ۵۰ درصد", "تحویل فوری"], but=["موقع پرداخت هزینه اضافه کرد", "اول گرون کرده بود بعد تخفیف", "سه هفته طول کشید"]))
    rows += R("complaint", fill("{t} {bad} بعد {when}", t=["لایسنس ویندوزم", "اکانتم", "شارژم"], bad=["پرید", "بسته شد", "تموم شد"], when=["آپدیت", "یه هفته", "دو روز"]))
    rows += R("complaint", fill("پولمو از کارتم {v}، {then}", v=["کشیدن", "برداشتن"], then=["به بانک زنگ زدم", "شکایت کردم"]))

    # hypothetical, conditional, someday
    rows += R("hypothetical", fill(
        "{someday} {stuff} رو {v}، {excuse}",
        someday=["یه روز میشینم", "بالاخره یه روز", "قراره بعد عید", "شاید تابستون"],
        stuff=["کل وسایل اضافه خونه", "این خرت و پرتا", "نصف کمدمو", "کتابای قدیمیمو"],
        v=["جمع میکنم میفروشم", "میفروشم", "رد میکنم بره"],
        excuse=["اگه تنبلیم بذاره", "ولی حسش نیست", "خدا بخواد", "به شرطی که وقت شه"],
    ))
    rows += R("hypothetical", fill("اگه پول داشتم {t} میخریدم {but}", t=thing, but=["ولی ندارم", "همین الان", "ولی فعلا نه"]))
    rows += R("hypothetical", fill("اگه {cond} مجبورم {t} بخرم", cond=["گوشیم خراب شه", "ماشین بره", "لپ تاپم بسوزه"], t=["یه ارزونشو", "دست دوم", "قسطی"]))
    rows += R("hypothetical", fill("حاضرم {t}مو بدم فقط {wish} 😂", t=["اکانت", "گوشی", "ماشین"], wish=["یه شب راحت بخوابم", "امتحان پاس شم", "تعطیل شه"]))
    rows += R("hypothetical", fill("شاید {when} {t} {v}، هنوز معلوم نیست", when=["تابستون", "بعد عید", "سال دیگه"], t=["ماشینو", "خونه رو", "اکانتو"], v=["بفروشم", "عوض کنم"]))
    rows += R("hypothetical", fill("اگه بخوام {t} بفروشم {but}", t=["ماشینو", "گوشیمو", "اکانتمو"], but=["کسی نمیخره با این بازار", "فقط به بچه های خودمون میدم", "باید اول درستش کنم"]))
    rows += R("hypothetical", fill("دارم پول جمع میکنم {t} بخرم {when}", t=["یه دوچرخه", "یه گوشی", "ps5"], when=["واسه تابستون", "تا عید", "یه روزی"]))
    rows += R("hypothetical", fill("فرض کن {what}، {q}؟", what=["یه روز ۱۰۰ تا بیت کوین داشته باشی", "کسی گوشیتو دو برابر قیمت بخره", "لاتاری برنده شی"], q=["میفروشی یا نگه میداری", "چیکار میکنی", "ماشین میخری"]))
    rows += R("hypothetical", fill("inghad poolam nist ke hazeram {what} befrusham vase {t} :))", what=["kolyamo", "roohamo"], t=["iphone 17", "ps5"]))
    rows += R("hypothetical", fill("نمیدونم {t} بفروشم یا نه، {q}", t=["ماشینو", "گوشیمو"], q=["به نظرت چیکار کنم؟", "بازار خرابه", "بذارم بمونه؟"]))
    rows += R("hypothetical", fill("کاش {what}", what=["زودتر بیت کوین خریده بودم", "میشد وقتو خرید", "اکانت قدیمیمو پس میگرفتم", "اون ماشینو نفروخته بودم"]))
    rows += R("hypothetical", fill("اگه من جای تو بودم {t} رو {v}", t=["اون گوشی", "ماشین", "اکانت"], v=["نمیفروختم", "نمیخریدم", "نگه میداشتم"]))

    # shop-name compounds
    rows += R("plain", fill("{shop} های {where} {state}", shop=["کتاب فروشی", "طلا فروشی", "میوه فروشی"], where=["انقلاب", "بازار", "محلمون"], state=["هنوز سرجاشونن", "بسته بودن", "اعتصاب کردن"]))
    rows += R("plain", fill("{x} فروشی {bad}", x=["کم", "گران"], bad=["حرامه", "جریمه داره", "نهی شده"]))
    # finglish chat, so the transliterated marks alone never read as selling
    rows += [("plain", t) for t in [
        "salam chetori dadash", "gheymata kheyli raft bala", "toman dige arzesh nadare",
        "kharid raftim ba maman", "goshimo forookhtam rahat shodam", "chand kharidi ino?",
        "arzoon bood vase hamin gereftam", "naghdi dadam pool o", "mifrosham? na baba shookhi kardam",
        "forooshgah baste bood", "ghanoon group: kharid o foroosh mamnoo", "bazar mobile kheyli kasade migan",
    ]]
    # plain chat filler
    rows += [("plain", t) for t in [
        "سلام صبح بخیر همگی", "فردا کلاس داریم؟", "دیشب بازی رو دیدید چه گلی شد",
        "تولدت مبارک داداش گلم", "کسی جزوه داره بفرسته؟", "هوا امروز محشره بریم بیرون",
        "این سریاله رو از دست ندید", "چقدر این استیکر خنده داره", "شب بخیر تا فردا",
        "ممنون از راهنماییتون بچه ها", "کی میاد فوتبال جمعه؟", "عکس سفرو گذاشتم ببینید",
        "آیدیمو عوض کردم، سیو کنید جدیدو", "شماره ام عوض شد ذخیره کنید", "بیا پیوی کارت دارم",
        "هرکی جزوه میخواد بیاد خصوصی بفرستم", "لینک گروه جدید رو گذاشتم بیاید", "سیم کارتم سوخت رفتم عوضش کردم",
        "anyone up for a game tonight?", "امتحان فردا چی میاد به نظرتون؟", "ماشینم تو برف گیر کرده کسی نزدیکه کمک کنه؟",
    ]]
    rows += R("plain", fill("{x} {v}، {ask}", x=["آیدیم", "شماره م", "کانالم"], v=["عوض شد", "جدیده"], ask=["سیو کنید", "دنبال کنید", "ذخیره کنید"]))
    # The call-to-action without anything for sale — «بیا پیوی کارت دارم» is "I need a word
    # with you", and a free handout is a favour. The shape of an ad's last line, with none of
    # its substance; the battery measured both at +35..+40 before this family existed.
    rows += R("plain", fill("بیا {where} {why}", where=["پیوی", "خصوصی", "پی وی", "دایرکت"], why=["کارت دارم", "یه چیزی بگم", "سوال دارم", "حرف بزنیم", "یه کاری باهات دارم", "مهمه"]))
    rows += R("plain", fill("{who} {t} میخواد بیاد {where} {v}، {free}", who=["هرکی", "هر کی", "کسی"], t=["جزوه", "فایل", "لینک", "عکسا", "پاسخنامه", "نمونه سوال"], where=["خصوصی", "پیوی", "دایرکت"], v=["بفرستم", "بدم", "میفرستم"], free=["رایگانه", "", "پولی نیست", "خدا خیرتون بده"]))
    rows += R("plain", fill("{t} رو {where} {v}، {why}", t=["جزوه", "لینک", "عکس", "فایل"], where=["تو پیوی", "خصوصی", "دایرکت"], v=["فرستادم", "میفرستم", "بذارم"], why=["اینجا شلوغ میشه", "چک کن", "ببین"]))
    rows += R("plain", fill("{who} {q} بیاد پیوی {v}", who=["هرکی", "کسی"], q=["سوال داره", "مشکل داره", "کمک میخواد"], v=["جواب میدم", "کمکش میکنم", "راهنماییش میکنم"]))
    rows += R("plain", fill("{intro} {t}: {list} — {end}", intro=["لیست جهیزیه", "لیست خرید عید", "چیزایی که لازم داریم"], t=["خواهرم", "خونه", "سفر"], list=["یخچال، ماشین لباسشویی، تلویزیون", "کفش، مانتو، عطر", "چادر، کوله، کفش"], end=["باید بخریم", "خدا کمکمون کنه", "کلی خرج داره"]))
    rows += R("plain", fill("{t} {v} — {story}", t=["گوشی سامسونگ A52", "لپ تاپ ایسوس", "پراید ۹۶"], v=["۱۲۸ گیگ مشکی", "i5 و ۸ گیگ رم", "سفید"], story=["اینو خریدم بالاخره، نظرتون چیه؟", "همینو دارم، راضی ام", "مال داداشمه"]))
    return rows


# ---------------------------------------------------------------------------------------
# typo augmentation — what the runtime descrambler cannot undo
# ---------------------------------------------------------------------------------------
#
# Dots, tatweel and spaced-out letters are *reversed* at runtime (`intent::descramble`), so
# the model never sees them and they need no training. A deliberate misspelling —
# «خربدارم» for «خریدارم», the production bypass — cannot be reversed, so the model has to
# learn that a typo'd commerce word still means commerce. Three transforms, applied to both
# classes so a typo alone never reads as selling: swap two adjacent letters, replace one
# letter with its keyboard neighbour, drop one letter.

NEIGHBOURS = {
    "ض": "ص", "ص": "ضث", "ث": "صق", "ق": "ثف", "ف": "قغ", "غ": "فع", "ع": "غه",
    "ه": "عخ", "خ": "هح", "ح": "خج", "ج": "حچ", "چ": "ج",
    "ش": "س", "س": "شی", "ی": "سب", "ب": "یل", "ل": "با", "ا": "لت", "ت": "ان",
    "ن": "تم", "م": "نک", "ک": "مگ", "گ": "ک",
    "ظ": "ط", "ط": "ظز", "ز": "طر", "ر": "زذ", "ذ": "رد", "د": "ذپ", "پ": "دو",
    "و": "پ",
}


def typo(text: str, rng: random.Random) -> str | None:
    words = text.split(" ")
    targets = [at for at, w in enumerate(words) if len(w) >= 4 and any(c in NEIGHBOURS for c in w)]
    if not targets:
        return None
    at = rng.choice(targets)
    letters = list(words[at])
    spot = rng.randrange(1, len(letters) - 1)
    kind = rng.randrange(3)
    if kind == 0:
        letters[spot], letters[spot + 1] = letters[spot + 1], letters[spot]
    elif kind == 1 and letters[spot] in NEIGHBOURS:
        letters[spot] = rng.choice(NEIGHBOURS[letters[spot]])
    else:
        del letters[spot]
    words[at] = "".join(letters)
    mangled = " ".join(words)
    return mangled if mangled != text else None


# ---------------------------------------------------------------------------------------
# the leak guard
# ---------------------------------------------------------------------------------------

def normal(text: str) -> str:
    text = text.replace("‌", " ").lower()
    return re.sub(r"[\W\d_۰-۹]+", " ", text).strip()


def trigrams(text: str) -> set[str]:
    words = normal(text).split()
    if len(words) < 3:
        return {" ".join(words)} if words else set()
    return {" ".join(words[i : i + 3]) for i in range(len(words) - 2)}


def held_out_grams() -> set[str]:
    grams: set[str] = set()
    for path in (EVAL, BATTERY):
        if not path.exists():
            continue
        for line in path.read_text(encoding="utf-8").splitlines():
            grams |= trigrams(line.split("\t")[-1])
    return grams


def main() -> None:
    judge = held_out_grams()

    rows: list[tuple[str, str, str]] = []  # (label, frame, text)
    for label, produced in (("sell", sell_rows()), ("safe", safe_rows())):
        for frame, text in produced:
            assert frame in FRAMES, frame
            rows.append((label, frame, text))
    # Composition: a message is as long as its author, and the frame decides, not the length
    # or the goods it lists. A plain sentence or two in front of (and, for the safe side, after)
    # a row of either class teaches exactly that — the battery's long narratives with the
    # frame at the end were the measured hole.
    plain = [text for label, frame, text in rows if frame == "plain"]
    # A chat-shaped ad is a real register but a rare one; at one sell in five the model
    # learned that chatty text is an ad and the battery's discussion rows moved up. One in
    # twelve keeps the register without teaching the shape.
    for label, share in (("sell", 12), ("safe", 3)):
        originals = [(frame, text) for l, frame, text in rows if l == label and frame != NO_FRAME]
        for frame, text in originals[::share]:
            lead = " ".join(random_state.sample(plain, random_state.randint(1, 3)))
            if label == "safe" and random_state.random() < 0.5:
                composed = f"{text}. {lead}"
            else:
                composed = f"{lead}. {text}"
            rows.append((label, frame, composed))
    # Free-written material from the audit fleets — registers templates express poorly
    # (dialects, euphemism, testimonial ads). They carry a label and no frame, so they go
    # into the catch-alls; the leak guard applies below like every other source.
    if FLEET.exists():
        for line in FLEET.read_text(encoding="utf-8").splitlines():
            label, text = line.split("\t", 1)
            rows.append((label, NO_FRAME, text))
    # One typo'd variant for a sample of each class — the sell side so evasive spelling
    # still scores as selling, the safe side so a typo alone never does.
    for label, share in (("sell", 4), ("safe", 3)):
        originals = [(frame, text) for l, frame, text in rows if l == label]
        for frame, text in originals[::share]:
            mangled = typo(text, random_state)
            if mangled:
                rows.append((label, frame, mangled))
    # Emoji decoration on a sample of both classes — advertisements wear it, chat wears it,
    # and the model should read through it either way.
    glitter = ["🔥", "✅", "💰", "📱", "💎", "⭐", "🎁"]
    for label, share in (("sell", 6), ("safe", 6)):
        originals = [(frame, text) for l, frame, text in rows if l == label]
        for frame, text in originals[::share]:
            words = text.split(" ")
            if len(words) < 2:
                continue
            e = random_state.choice(glitter)
            words[0] = f"{e}{words[0]}{e}"
            words[-1] = f"{words[-1]} {random_state.choice(glitter)}"
            rows.append((label, frame, " ".join(words)))

    seen: set[str] = set()
    kept: list[tuple[str, str, str]] = []
    leaked = 0
    for label, frame, text in rows:
        key = normal(text)
        if not key or key in seen:
            continue
        seen.add(key)
        grams = trigrams(text)
        # A row that shares most of its trigrams with a held-out set is a leak — the judge
        # must never appear in the training data, or its verdict is worthless.
        if grams and len(grams & judge) / len(grams) > 0.5:
            leaked += 1
            continue
        kept.append((label, frame, text))

    OUT.write_text("\n".join(f"{label}\t{frame}\t{text}" for label, frame, text in kept) + "\n", encoding="utf-8")
    sells = sum(1 for label, _, _ in kept if label == "sell")
    print(f"corpus: {len(kept)} rows ({sells} sell, {len(kept) - sells} safe), {leaked} dropped as held-out leaks -> {OUT}")
    by_frame: dict[str, int] = {}
    for _, frame, _ in kept:
        by_frame[frame] = by_frame.get(frame, 0) + 1
    print("  " + ", ".join(f"{frame} {n}" for frame, n in sorted(by_frame.items(), key=lambda kv: -kv[1])))


if __name__ == "__main__":
    main()
