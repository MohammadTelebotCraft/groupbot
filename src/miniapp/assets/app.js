(() => {
  "use strict";

  const tg = window.Telegram && window.Telegram.WebApp;
  const app = document.getElementById("app");
  const initData = (tg && tg.initData) || "";

  const STATE_MESSAGES = {
    auth_failed: "این صفحه باید از داخل ربات باز شود.",
    chat_unknown: "ربات دیگر در این گروه نیست یا شناخته شده نیست.",
    not_admin: "شما ادمین این گروه نیستید.",
    set_denied: "دسترسی تنظیمات برای شما بسته است.",
    case_denied: "دسترسی پرونده‌ها برای شما بسته است.",
  };

  const STORE_KEY = "gm.chat";


  const FA_DIGITS = "۰۱۲۳۴۵۶۷۸۹";
  function fa(value) {
    return String(value == null ? "" : value).replace(/\d/g, (d) => FA_DIGITS[d]);
  }

  function esc(text) {
    return String(text == null ? "" : text)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  function initial(title) {
    const clean = String(title || "").replace(/^[\s«»"'\-_.·]+/, "");
    return clean ? clean[0] : "؟";
  }

  function hue(chat) {
    const n = Math.abs(Number(chat) || 0);
    return (n % 10) * 36 + 15;
  }
  function avatarStyle(chat) {
    return "background: oklch(0.78 0.13 " + hue(chat) + ")";
  }

  function minutesToClock(value) {
    const m = Math.max(0, Math.min(1439, Number(value) || 0));
    return String(Math.floor(m / 60)).padStart(2, "0") + ":" + String(m % 60).padStart(2, "0");
  }
  function clockToMinutes(text) {
    const parts = String(text || "").split(":");
    if (parts.length !== 2) return null;
    const h = Number(parts[0]);
    const m = Number(parts[1]);
    if (!Number.isInteger(h) || !Number.isInteger(m) || h > 23 || m > 59 || h < 0 || m < 0) {
      return null;
    }
    return h * 60 + m;
  }


  const ICONS = {
    home: '<path d="M3 11 12 3l9 8"/><path d="M5 10v10h14V10"/><path d="M10 20v-6h4v6"/>',
    sliders:
      '<line x1="4" y1="21" x2="4" y2="14"/><line x1="4" y1="10" x2="4" y2="3"/><line x1="12" y1="21" x2="12" y2="12"/><line x1="12" y1="8" x2="12" y2="3"/><line x1="20" y1="21" x2="20" y2="16"/><line x1="20" y1="12" x2="20" y2="3"/><circle cx="4" cy="12" r="2"/><circle cx="12" cy="10" r="2"/><circle cx="20" cy="14" r="2"/>',
    lock: '<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>',
    unlock: '<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V7a4 4 0 0 1 7.75-1.5"/>',
    users:
      '<path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
    shield: '<path d="M12 22s8-4.2 8-11V5.2l-8-3-8 3V11c0 6.8 8 11 8 11Z"/>',
    shieldCheck:
      '<path d="M12 22s8-4.2 8-11V5.2l-8-3-8 3V11c0 6.8 8 11 8 11Z"/><path d="m9 12 2 2 4-4"/>',
    shieldOff:
      '<path d="M19.7 14a9 9 0 0 0 .3-3V5.2l-8-3-3.2 1.2"/><path d="M4.5 4.5 19.5 19.5"/><path d="M4 6.6V11c0 6.8 8 11 8 11a15 15 0 0 0 4.2-2.5"/>',
    activity: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
    bolt: '<path d="M13 2 3 14h7l-1 8 10-12h-7l1-8Z"/>',
    bell: '<path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.7 21a2 2 0 0 1-3.4 0"/>',
    alert:
      '<path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
    flame:
      '<path d="M12 22c4 0 6-2.5 6-6 0-3-2-5-2.5-7-1 3-4 4-4 8 0 1.5.5 2.5 1.5 3-2.5 0-4.5-2-4.5-5C8.5 11 10 9 10 6c-3 1.5-5 5-5 9 0 4 3 7 7 7Z"/>',
    clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 3"/>',
    sparkles: '<path d="M12 3 13.5 9 20 12l-6.5 3L12 21l-1.5-6L4 12l6.5-3Z"/>',
    trash:
      '<path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6"/>',
    send: '<path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/>',
    moon: '<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z"/>',
    link: '<path d="M9 17H7a5 5 0 0 1 0-10h2"/><path d="M15 7h2a5 5 0 1 1 0 10h-2"/><path d="M8 12h8"/>',
    chat:
      '<path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5Z"/>',
    chevL: '<path d="m15 18-6-6 6-6"/>',
    chevD: '<path d="m6 9 6 6 6-6"/>',
    check: '<path d="m5 12 5 5L20 7"/>',
    x: '<path d="M18 6 6 18M6 6l12 12"/>',
    mic: '<path d="M12 2a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/><path d="M19 10v1a7 7 0 0 1-14 0v-1"/><path d="M12 18v4"/><path d="M9 22h6"/>',
    ban: '<circle cx="12" cy="12" r="9"/><line x1="6.5" y1="6.5" x2="17.5" y2="17.5"/>',
    mute:
      '<path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.7 21a2 2 0 0 1-3.4 0"/><line x1="4" y1="4" x2="20" y2="20"/>',
    star: '<path d="m12 2 3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2Z"/>',
    funnel: '<path d="M22 3H2l8 9.46V19l4 2v-8.54L22 3Z"/>',
    image:
      '<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><path d="m21 15-5-5L5 21"/>',
    search: '<circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>',
    gear: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z"/>',
    hand: '<path d="M11 5H6a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-5"/><path d="m9 15 10-10 3 3-10 10H9v-3Z"/>',
    door: '<path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4"/><path d="M10 17l5-5-5-5"/><path d="M15 12H3"/>',
    fileText:
      '<path d="M14 3v4a1 1 0 0 0 1 1h4"/><path d="M17 21H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7l5 5v11a2 2 0 0 1-2 2Z"/><path d="M9 13h6M9 17h4"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    userPlus:
      '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M19 8v6M22 11h-6"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
    arrowL: '<path d="M19 12H5"/><path d="m12 19-7-7 7-7"/>',
    key: '<circle cx="8" cy="15" r="4"/><path d="m10.85 12.15 8.15-8.15"/><path d="m18 6 3 3"/><path d="m15 9 3 3"/>',
    wifiOff:
      '<path d="M1 1l22 22"/><path d="M16.7 16.7a5 5 0 0 0-9.4 0"/><path d="M5 12.6a9 9 0 0 1 3.2-2.3"/><path d="M12 20h.01"/><path d="M8.5 8.5A14 14 0 0 1 22 9"/>',
    refresh: '<path d="M21 12a9 9 0 1 1-2.6-6.4"/><path d="M21 3v6h-6"/>',
    terminal: '<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>',
    pause: '<rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/>',
    play: '<path d="M6 4l14 8-14 8V4Z"/>',
  };

  function icon(name, size, weight) {
    size = size || 18;
    const keys = {
      lock: "LOCKED", unlock: "UNLOCKED", ban: "MODERATION_HAMMER", mute: "MUTED",
      clock: "TIMER", send: "TELEGRAM_SEND", chat: "CHAT", mic: "VOICE",
      image: "IMAGE", fileText: "DOCUMENT_ACTIVITY", alert: "WARNING", pause: "PAUSE",
    };
    const key = keys[name] || name;
    const glyph = window.MODERATION_ICONS && window.MODERATION_ICONS[key];
    if (glyph) {
      return '<span class="semantic-icon" aria-hidden="true" data-icon="' + esc(key) +
        '" style="font-size:' + size + 'px">' + esc(glyph) + '</span>';
    }
    return (
      '<svg width="' + size + '" height="' + size +
      '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="' + (weight || 2) +
      '" stroke-linecap="round" stroke-linejoin="round">' + (ICONS[name] || "") + "</svg>"
    );
  }

  const CHEV = '<span class="chev">' + icon("chevL", 18) + "</span>";


  const S = {
    groups: null,
    chat: null,
    dash: null,
    health: null,
    activity: null,
    tab: "home",
    stack: [],
    sheet: null,
    confirm: null,
    adv: new Set(),
    pending: null,
    locksAll: false,
  };


  async function api(path, opts) {
    opts = opts || {};
    const headers = { Authorization: "tma " + initData };
    if (S.chat != null) headers["X-Chat"] = String(S.chat);
    if (opts.body) headers["Content-Type"] = "application/json";
    const res = await fetch("/api" + path, {
      method: opts.method || "GET",
      headers,
      body: opts.body ? JSON.stringify(opts.body) : undefined,
    });
    let data = null;
    try {
      data = await res.json();
    } catch (e) {
      data = null;
    }
    if (!res.ok) {
      const err = new Error((data && data.state) || "request_failed");
      err.detail = data && data.error;
      throw err;
    }
    return data;
  }

  async function write(run) {
    try {
      await run();
      return true;
    } catch (e) {
      report(e);
      return false;
    }
  }
  function report(e) {
    toast((e && e.detail) || STATE_MESSAGES[e && e.message] || "انجام نشد. دوباره امتحان کنید.");
  }

  let toastTimer = null;
  function toast(text, ok) {
    let el = document.getElementById("toast");
    if (!el) {
      el = document.createElement("div");
      el.id = "toast";
      document.body.appendChild(el);
    }
    el.className = ok ? "ok" : "";
    el.innerHTML = icon(ok ? "check" : "alert", 16) + "<span>" + esc(text) + "</span>";
    requestAnimationFrame(() => el.classList.add("show"));
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => el.classList.remove("show"), 3200);
  }

  function haptic(kind) {
    try {
      if (tg && tg.HapticFeedback) {
        if (kind === "ok") tg.HapticFeedback.notificationOccurred("success");
        else tg.HapticFeedback.impactOccurred("light");
      }
    } catch (e) {
    }
  }


  function section(id) {
    return (S.dash.sections || []).find((s) => s.id === id) || null;
  }
  function setting(id) {
    for (const s of S.dash.sections) {
      for (const it of s.settings) if (it.id === id) return it;
    }
    return null;
  }
  function isOn(id) {
    const it = setting(id);
    return !!(it && it.on);
  }
  function shown(id) {
    const it = setting(id);
    return it ? fa(it.shown) : "";
  }
  function pickLabel(id) {
    const it = setting(id);
    if (!it) return "";
    const o = it.options.find((o) => o.value === it.chosen);
    return o ? o.label : "";
  }
  function sectionEnabled(id) {
    const s = section(id);
    return !!(s && s.enabled);
  }


  const FEATURES = [
    {
      id: "fl", group: "محافظت", icon: "TIMER", title: "ضد رگبار",
      desc: "کسی که پشت سر هم پیام بفرستد متوقف می شود",
      flag: "fl_on", sections: ["fl"],
      effect: () => "بیش از " + shown("fl_lim") + " پیام در " + shown("fl_win") + " ثانیه ← " + pickLabel("fl_act"),
    },
    {
      id: "rd", group: "محافظت", icon: "shield", title: "ضد هجوم",
      desc: "اگر ناگهان عده زیادی عضو شوند، همه شان موقتا ساکت می شوند",
      flag: "rd_on", sections: ["rd"],
      effect: () => shown("rd_lim") + " عضو در " + shown("rd_win") + " ثانیه ← سکوت " + shown("rd_time"),
    },
    {
      id: "cp", group: "محافظت", icon: "key", title: "احراز هویت",
      desc: "عضو تازه باید ثابت کند ربات نیست",
      flag: "cp_on", sections: ["cp"],
      effect: () => "مهلت " + shown("cp_t") + " ثانیه · " + shown("cp_n") + " گزینه ← " + pickLabel("cp_act"),
    },
    {
      id: "bt", group: "محافظت", icon: "shieldOff", title: "ضد خیانت ادمین",
      desc: "ادمینی که پشت سر هم حذف کند، عزل می شود",
      flag: "bt_on", sections: ["bt"],
      effect: () => shown("bt_lim") + " حذف در " + shown("bt_win") + " دقیقه ← " + pickLabel("bt_act"),
    },
    {
      id: "wn", group: "محافظت", icon: "alert", title: "اخطار",
      desc: "با دستور «اخطار» به متخلف؛ با رسیدن به سقف، خودکار برخورد می شود",
      always: true, sections: ["wn"],
      effect: () => shown("wn_lim") + " اخطار ← " + pickLabel("wn_act"),
    },
    {
      id: "s", group: "محافظت", icon: "MODERATION_HAMMER", title: "حالت سختگیرانه",
      desc: "هر برخورد با قفل ها یک تخلف شمرده می شود و تکرارش محدودیت می آورد",
      flag: "strict", sections: ["s"],
      effect: () => shown("s_lim") + " تخلف ← " + pickLabel("s_act") + " " + shown("s_time"),
    },
    {
      id: "wp", group: "محافظت", icon: "trash", title: "پاکسازی پیام متخلف",
      desc: "با بن یا سکوت، پیام های قبلی همان نفر هم پاک می شود",
      flags: ["wp_ban", "wp_mute"],
      state: () => isOn("wp_ban") || isOn("wp_mute"),
      effect: () => {
        const parts = [];
        if (isOn("wp_ban")) parts.push("با بن");
        if (isOn("wp_mute")) parts.push("با سکوت");
        return parts.join(" و ");
      },
    },
    {
      id: "locks", group: "محتوا", icon: "lock", title: "قفل ها",
      desc: "چه چیزهایی در گروه فرستاده نشود",
      page: "locks",
      state: () => S.dash.locks_summary.active > 0,
      effect: () => fa(S.dash.locks_summary.active) + " از " + fa(S.dash.locks_summary.total) + " روشن",
    },
    {
      id: "bl", group: "محتوا", icon: "link", title: "لینک در بایو",
      desc: "کسی که در بایوی پروفایلش لینک دارد نمی تواند پیام بدهد",
      flag: "bl_on", sections: ["bl"],
      effect: () => "با متخلف: " + pickLabel("bl_act"),
    },
    {
      id: "tmed", group: "محتوا", icon: "clock", title: "رسانه موقت",
      desc: "عکس و ویدیو بعد از مدتی خودکار پاک می شود",
      flag: "tmed_on", sections: ["tmed"],
      effect: () => "حذف پس از " + shown("tmed_min") + " · " + pickLabel("tmed_who"),
    },
    {
      id: "filter", group: "محتوا", icon: "funnel", title: "فیلتر کلمات",
      desc: "پیامی که این کلمه ها را داشته باشد حذف می شود",
      page: "list:filter", neutral: true,
      effect: () => "فهرست کلمات",
    },
    {
      id: "cq", group: "هوشمند", icon: "sparkles", title: "نگهبان هوشمند",
      desc: "هر عکس و متن با هوش مصنوعی بررسی می شود، نه با کلمه",
      sections: ["cq"], custom: "ai",
      state: () => S.dash.locks_summary.ai_active > 0,
      effect: () => fa(S.dash.locks_summary.ai_active) + " موضوع فعال" + (isOn("cq_shadow") ? " · فقط بررسی" : ""),
    },
    {
      id: "imgf", group: "هوشمند", icon: "image", title: "فیلتر تصویری",
      desc: "یک موضوع بنویسید تا هر عکس نزدیک به آن حذف شود",
      page: "imgf", neutral: true,
      effect: () => "موضوع های شما",
    },
    {
      id: "vm", group: "هوشمند", icon: "mic", title: "نظارت بر ویس",
      desc: "ویس ها شنیده می شود و کلمه های نامناسب حذف می شود",
      flag: "vm_on", custom: "voice",
      effect: () => "فهرست کلمات نامناسب",
    },
    {
      id: "wc", group: "خودکار", icon: "hand", title: "خوشامد",
      desc: "به هر عضو تازه پیام خوشامد فرستاده می شود",
      page: "welcome", neutral: true, sections: ["wc"],
      effect: () => "حذف خودکار: " + shown("wct"),
    },
    {
      id: "ap", group: "خودکار", icon: "trash", title: "پاکسازی خودکار",
      desc: "هر روز در ساعتی که می گویید، پیام های قدیمی پاک می شود",
      enabled: "ap", off: "/purge/off", onAction: () => "apt:" + setting("apt").value, sections: ["ap"],
      effect: () => "هر روز " + shown("apt") + " · " + shown("apc") + " پیام",
    },
    {
      id: "dr", group: "خودکار", icon: "send", title: "گزارش روزانه",
      desc: "آمار روز، هر شب در گروه فرستاده می شود",
      enabled: "dr", off: "/report/off", onAction: () => "dr:" + setting("dr").value, sections: ["dr"],
      effect: () => "هر شب " + shown("dr"),
    },
    {
      id: "ng", group: "خودکار", icon: "moon", title: "قفل شب",
      desc: "در این بازه هیچ کس جز ادمین ها نمی تواند پیام بدهد",
      enabled: "ng", off: "/night/off", onAction: () => "ngf:" + setting("ngf").value, sections: ["ng"],
      effect: () => "از " + shown("ngf") + " تا " + shown("ngt"),
    },
    {
      id: "an", group: "خودکار", icon: "chat", title: "پاسخ خودکار",
      desc: "به کلمه های مشخص، پاسخ آماده داده می شود",
      always: true, sections: ["an"], custom: "answers",
      effect: () => "مخاطب: " + pickLabel("an_act"),
    },
    {
      id: "ad", group: "خودکار", icon: "userPlus", title: "اد اجباری",
      desc: "هر عضو باید چند نفر را اضافه کند تا بتواند پیام بدهد",
      sections: ["ad", "gp"],
      state: () => Number(setting("ad").value) > 0,
      effect: () => shown("ad") + (Number(setting("ad").value) > 0 ? " نفر · یادآوری " + shown("gpe") : ""),
    },
  ];

  const GROUPS = ["محافظت", "محتوا", "هوشمند", "خودکار"];

  function feature(id) {
    return FEATURES.find((f) => f.id === id) || null;
  }

  function featureState(f) {
    if (f.neutral) return null;
    if (f.always) return true;
    if (f.state) return f.state();
    if (f.flag) return isOn(f.flag);
    if (f.enabled) return sectionEnabled(f.enabled);
    return null;
  }

  function featureRow(f) {
    const st = featureState(f);
    const sub = st === false ? "خاموش" : f.effect();
    return (
      '<button class="row" data-open="' + f.id + '">' +
      '<span class="rico' + (st ? " on" : "") + '">' + icon(f.icon, 16) + "</span>" +
      '<span class="rt"><span class="t">' + f.title + '</span><span class="s">' + esc(sub) + "</span></span>" +
      CHEV + "</button>"
    );
  }


  async function start() {
    if (!initData) {
      app.innerHTML = '<div class="state error">' + STATE_MESSAGES.auth_failed + "</div>";
      return;
    }
    if (tg) {
      tg.ready();
      tg.expand();
      try {
        tg.setHeaderColor("#151a24");
        tg.setBackgroundColor("#151a24");
      } catch (e) {
      }
      if (tg.BackButton) tg.BackButton.onClick(back);
    }
    app.innerHTML = renderSkeleton();
    let groups;
    try {
      groups = (await api("/groups")).groups;
    } catch (e) {
      app.innerHTML = '<div class="state error">' + (STATE_MESSAGES[e.message] || "خطایی رخ داد.") + "</div>";
      return;
    }
    S.groups = groups;

    const launched = tg && tg.initDataUnsafe && tg.initDataUnsafe.start_param;
    let chat = Number(launched) || null;
    if (!chat) {
      const stored = Number(localStorage.getItem(STORE_KEY)) || null;
      if (stored && groups.some((g) => g.id === stored && g.known)) chat = stored;
    }
    if (!chat) {
      const known = groups.filter((g) => g.known);
      if (known.length === 1) chat = known[0].id;
    }
    if (!chat) {
      if (!groups.length) {
        renderNoGroups();
        return;
      }
      renderShellBare();
      openPicker();
      return;
    }
    await loadChat(chat);
  }

  function group() {
    return (S.groups || []).find((g) => g.id === S.chat) || null;
  }

  async function loadChat(chat) {
    S.chat = chat;
    S.dash = null;
    S.health = null;
    S.activity = null;
    S.stack = [];
    S.sheet = null;
    S.confirm = null;
    S.admins = null;
    S.lists = null;
    S.pending = null;
    S.locksAll = false;
    S.featureQuery = "";
    S.tab = "home";
    localStorage.setItem(STORE_KEY, String(chat));
    app.innerHTML = renderSkeleton();
    try {
      S.dash = await api("/dashboard");
    } catch (e) {
      renderChatError(e);
      return;
    }
    if (S.dash.state) {
      renderChatError(new Error(S.dash.state));
      return;
    }
    if (S.groups && !S.groups.some((g) => g.id === chat)) {
      S.groups.unshift({ id: chat, title: S.dash.chat.title, is_owner: S.dash.viewer.is_owner, known: true });
    }
    render();
    loadHealth();
    loadActivity();
  }

  async function loadHealth() {
    const chat = S.chat;
    try {
      const h = await api("/health");
      if (S.chat !== chat) return;
      S.health = h;
    } catch (e) {
      if (S.chat !== chat) return;
      S.health = "error";
    }
    if (S.tab === "home" && !S.stack.length) render();
  }

  async function loadActivity() {
    const chat = S.chat;
    try {
      const a = await api("/activity");
      if (S.chat !== chat) return;
      S.activity = a;
    } catch (e) {
      if (S.chat !== chat) return;
      S.activity = "error";
    }
    if ((S.tab === "home" || S.tab === "activity") && !S.stack.length) render();
  }

  async function refreshDash() {
    const chat = S.chat;
    const d = await api("/dashboard");
    if (S.chat !== chat) return;
    S.dash = d;
  }

  function renderChatError(e) {
    const msg = STATE_MESSAGES[e.message] || "دسترسی امکان پذیر نیست.";
    app.innerHTML =
      renderTop(true) +
      '<div id="main" class="nobar"><div class="empty"><div class="glyph">' + icon("shieldOff", 32, 1.6) + "</div>" +
      '<div class="t">' + esc(msg) + "</div>" +
      (S.groups && S.groups.length > 1 ? '<button class="btn" data-picker>انتخاب گروه دیگر</button>' : "") +
      "</div></div>" + renderOverlays();
    syncBack();
  }

  function renderNoGroups() {
    app.innerHTML =
      '<div class="top"><div class="grp"><div class="avatar" style="background:var(--card-2);color:var(--muted)">' +
      icon("shieldCheck", 18) + '</div><div><div class="gname">مدیریت گروه</div><div class="gsub">هنوز گروهی وصل نیست</div></div></div></div>' +
      '<div id="main" class="nobar"><div class="empty"><div class="glyph">' + icon("users", 32, 1.6) + "</div>" +
      '<div class="t">گروهی برای مدیریت نیست</div>' +
      '<div class="s">ربات را در گروه خود ادمین کنید و دسترسی حذف پیام و بن به آن بدهید. بعد این صفحه را دوباره باز کنید.</div>' +
      "</div></div>";
  }

  function renderSkeleton() {
    const sk = (w, h, r) => '<div class="sk" style="width:' + w + ";height:" + h + "px;border-radius:" + (r || 8) + 'px"></div>';
    return (
      '<div class="top"><div class="grp">' + sk("36px", 36, 11) +
      '<div style="display:flex;flex-direction:column;gap:6px">' + sk("140px", 14) + sk("90px", 10) + "</div></div>" + sk("40px", 40, 12) + "</div>" +
      '<div id="main"><div class="sec" style="padding-top:4px">' + sk("100%", 68, 16) + "</div>" +
      '<div class="sec"><div class="quick">' +
      ('<div style="display:flex;flex-direction:column;align-items:center;gap:6px">' + sk("52px", 52, 16) + sk("44px", 10) + "</div>").repeat(4) +
      '</div></div><div class="sec"><div class="h">' + sk("40px", 12) + "</div>" + sk("100%", 70, 16) + "</div>" +
      '<div class="sec"><div class="h">' + sk("52px", 12) + "</div>" + sk("100%", 208, 16) + "</div></div>"
    );
  }


  function renderTop(bare) {
    const g = group();
    const title = g ? g.title : S.dash ? S.dash.chat.title : "انتخاب گروه";
    const role = S.dash ? (S.dash.viewer.is_owner ? "شما مالک هستید" : "شما ادمین هستید") : g && g.is_owner ? "مالک" : "ادمین";
    const many = S.groups && S.groups.length > 1;
    return (
      '<div class="top">' +
      '<button class="grp" data-picker' + (many ? "" : " disabled") + '>' +
      '<span class="avatar" style="' + avatarStyle(S.chat) + '">' + esc(initial(title)) + "</span>" +
      '<span style="min-width:0"><span class="gname" style="display:block">' + title + '</span><span class="gsub" style="display:block">' +
      role + (many ? " · " + fa(S.groups.length) + " گروه" : "") + "</span></span>" +
      (many ? '<span class="chev" style="color:var(--muted)">' + icon("chevD", 16) + "</span>" : "") +
      "</button>" +
      (bare ? "" : '<button class="ibtn" data-push="settings" title="تنظیمات گروه">' + icon("gear", 20) + "</button>") +
      "</div>"
    );
  }

  function renderPageTop(title, sub, right) {
    return (
      '<div class="top"><div style="display:flex;align-items:center;gap:6px;min-width:0">' +
      '<button class="ibtn" data-back style="margin-right:-8px">' + icon("arrowL", 20) + "</button>" +
      '<div style="min-width:0"><div class="ptitle">' + title + '</div><div class="gsub">' + (sub || "") + "</div></div></div>" +
      (right || "") + "</div>"
    );
  }

  const TABS = [
    ["home", "home", "خانه"],
    ["features", "sliders", "امکانات"],
    ["members", "users", "اعضا"],
    ["activity", "activity", "فعالیت"],
  ];

  function renderTabs() {
    const issues = S.health && S.health !== "error" ? S.health.issues.length : 0;
    return (
      '<div id="tabs">' +
      TABS.map(
        ([id, ic, label]) =>
          '<button class="tab' + (S.tab === id ? " on" : "") + '" data-tab="' + id + '">' + icon(ic, 22) + "<span>" + label + "</span>" +
          (id === "home" && issues ? '<span class="badge">' + fa(issues) + "</span>" : "") + "</button>"
      ).join("") +
      "</div>"
    );
  }

  function renderOverlays() {
    return '<div id="dim" data-close></div><div id="sheet"></div>';
  }

  function renderShellBare() {
    app.innerHTML = renderTop(true) + '<div id="main" class="nobar"></div>' + renderOverlays();
  }

  function render() {
    if (!S.dash) return;
    const page = S.stack[S.stack.length - 1];
    let html;
    if (page) {
      html = renderPage(page);
    } else {
      html = renderTop(false) + '<div id="main">' + renderTab() + "</div>" + renderTabs();
    }
    html += renderOverlays() + renderConfirm();
    const y = window.scrollY;
    app.innerHTML = html;
    window.scrollTo(0, y);
    if (S.sheet) drawSheet();
    syncBack();
  }

  function renderTab() {
    switch (S.tab) {
      case "features":
        return renderFeatures();
      case "members":
        return renderMembers();
      case "activity":
        return renderActivity();
      default:
        return renderHome();
    }
  }

  function syncBack() {
    if (!tg || !tg.BackButton) return;
    const show = !!(S.stack.length || S.sheet || S.confirm);
    if (show) tg.BackButton.show();
    else tg.BackButton.hide();
  }

  function back() {
    if (S.confirm) {
      S.confirm = null;
      render();
      return;
    }
    if (S.sheet) {
      closeSheet();
      return;
    }
    if (S.stack.length) {
      const page = currentPage();
      if (page && page.id === "cases" && page.data && page.data.detail) {
        delete page.data.detail;
        render();
        return;
      }
      S.stack.pop();
      render();
    }
  }


  function todayCounts() {
    if (!S.activity || S.activity === "error") return null;
    const day = S.activity.days[0];
    const get = (k) => {
      const c = day.counters.find((c) => c.key === k);
      return c ? c.count : 0;
    };
    return { deleted: get("deleted"), moderated: get("muted") + get("banned"), joined: get("joined") };
  }

  function activeFeatureCount() {
    return FEATURES.filter((f) => featureState(f) === true && !f.always).length;
  }

  function renderHero() {
    const h = S.health;
    if (h == null) {
      return '<div class="hero"><div class="glyph mute">' + icon("refresh", 22) + '</div><div style="flex:1;min-width:0"><div class="t">در حال بررسی وضعیت گروه…</div><div class="s">' +
        fa(activeFeatureCount()) + " امکان روشن · " + fa(S.dash.locks_summary.active) + " قفل</div></div></div>";
    }
    if (h === "error") {
      return '<div class="hero"><div class="glyph mute">' + icon("wifiOff", 22) + '</div><div style="flex:1;min-width:0"><div class="t">وضعیت گروه خوانده نشد</div><div class="s">تنظیمات در دسترس است</div></div>' +
        '<button class="btn sm ghost" data-recheck>' + icon("refresh", 14) + " تلاش</button></div>";
    }
    if (!h.issues.length) {
      return '<div class="hero"><div class="glyph ok">' + icon("shieldCheck", 22) + '</div><div style="flex:1;min-width:0"><div class="t">گروه محافظت می شود</div><div class="s">' +
        fa(activeFeatureCount()) + " امکان روشن · " + fa(S.dash.locks_summary.active) + " قفل · بدون مشکل</div></div></div>";
    }
    const worst = h.issues.some((i) => i.severity === "bad") ? "bad" : "warn";
    return (
      '<div class="hero-wrap"><div class="hero"><div class="glyph ' + worst + '">' + icon("alert", 22) + "</div>" +
      '<div style="flex:1;min-width:0"><div class="t">' + fa(h.issues.length) + " مورد نیاز به توجه دارد</div>" +
      '<div class="s">' + (worst === "bad" ? "بخشی از محافظت کار نمی کند" : "یک تنظیم ناقص مانده") + "</div></div></div>" +
      h.issues.map(
        (i) =>
          '<div class="row"><span class="dot ' + i.severity + '"></span><span class="rt"><span class="t">' + esc(i.title) + '</span><span class="s wrap">' + esc(i.detail) + "</span></span>" +
          '<button class="btn sm ghost" data-fix="' + esc(i.fix) + '">' + (i.fix === "rights" ? "راهنما" : "تنظیم") + "</button></div>"
      ).join("") +
      "</div>"
    );
  }

  function renderHome() {
    const strictOn = isOn("strict");
    const nightOn = sectionEnabled("ng");
    const t = todayCounts();
    const v = (n) => (t ? '<div class="v">' + fa(n) + "</div>" : '<div class="v dim">—</div>');
    const top = ["fl", "cq", "locks", "ng"].map((id) => featureRow(feature(id))).join("");
    return (
      '<div class="sec" style="padding-top:4px">' + renderHero() + "</div>" +
      '<div class="sec"><div class="quick">' +
      '<button data-push="rights"><span class="q">' + icon("lock", 22) + '</span><span class="l">بستن گروه</span></button>' +
      '<button data-apply="strict"><span class="q' + (strictOn ? " on" : "") + '">' + icon("MODERATION_HAMMER", 22) + '</span><span class="l' + (strictOn ? " on" : "") + '">سختگیرانه</span></button>' +
      '<button data-open="ap"><span class="q">' + icon("trash", 22) + '</span><span class="l">پاکسازی</span></button>' +
      '<button data-open="ng"><span class="q' + (nightOn ? " on" : "") + '">' + icon("moon", 22) + '</span><span class="l' + (nightOn ? " on" : "") + '">قفل شب</span></button>' +
      "</div></div>" +
      '<div class="sec"><div class="h"><div class="t">امروز</div><button class="more" data-tab="activity">فعالیت ' + icon("chevL", 14) + "</button></div>" +
      '<div class="stats"><div>' + v(t && t.deleted) + '<div class="l">پیام حذف شده</div></div><div>' + v(t && t.moderated) + '<div class="l">سکوت و بن</div></div><div>' + v(t && t.joined) + '<div class="l">عضو تازه</div></div></div></div>' +
      '<div class="sec"><div class="h"><div class="t">امکانات</div><button class="more" data-tab="features">همه ' + icon("chevL", 14) + "</button></div>" +
      '<div class="list">' + top + "</div></div>"
    );
  }


  function renderFeatures() {
    const q = (S.featureQuery || "").trim();
    let html =
      '<div class="sec" style="padding-top:4px"><label class="search">' + icon("search", 16) +
      '<input id="feature-search" placeholder="جستجو در امکانات، مثلا «لینک»" value="' + esc(q) + '" /></label></div>';
    let any = false;
    GROUPS.forEach((g) => {
      const items = FEATURES.filter((f) => f.group === g && (!q || (f.title + " " + f.desc).includes(q)));
      if (!items.length) return;
      any = true;
      const on = items.filter((f) => featureState(f) === true).length;
      const switchable = items.filter((f) => featureState(f) !== null).length;
      html +=
        '<div class="sec"><div class="h"><div class="t">' + g + '</div><div class="count">' +
        (switchable ? fa(on) + " از " + fa(switchable) + " روشن" : "") + "</div></div>" +
        '<div class="list">' + items.map(featureRow).join("") + "</div></div>";
    });
    if (!any) html += '<div class="state">چیزی با این نام نیست.</div>';
    return html;
  }


  const LIST_KINDS = {
    ban: ["بن شده ها", "ban", "کسانی که از گروه بیرون شده اند"],
    mute: ["سکوت شده ها", "mute", "نمی توانند پیام بدهند"],
    vip: ["کاربران ویژه", "PREMIUM_STAR", "از همه قفل ها معاف اند"],
    free: ["معاف ها", "unlock", "از ضد رگبار و اد اجباری معاف اند"],
    filter: ["فیلتر کلمات", "funnel", "پیامی که این کلمه ها را دارد حذف می شود"],
    answer: ["پاسخ خودکار", "chat", "به این کلمه ها پاسخ آماده داده می شود"],
    cmd: ["دستور های سفارشی", "terminal", ""],
    pack: ["پک های استیکر", "sparkles", "استیکرهای این پک ها حذف می شوند"],
  };

  function renderMembers() {
    const a = S.admins;
    let html = '<div class="sec" style="padding-top:4px"><div class="h" style="padding-top:6px"><div class="t">ادمین ها</div>' +
      (a && a !== "error" ? '<div class="count">' + fa(a.admins.length) + "</div>" : "") + "</div>";
    if (!a) {
      html += '<div class="list"><div class="row"><div class="sk" style="width:32px;height:32px;border-radius:9px"></div><div class="rt"><div class="sk" style="width:120px;height:12px"></div></div></div>' +
        '<div class="row"><div class="sk" style="width:32px;height:32px;border-radius:9px"></div><div class="rt"><div class="sk" style="width:90px;height:12px"></div></div></div></div>';
    } else if (a === "error") {
      html += '<div class="list"><div class="row"><span class="rt"><span class="s">فهرست ادمین ها خوانده نشد.</span></span><button class="btn sm ghost" data-reload-admins>تلاش</button></div></div>';
    } else {
      html += '<div class="list">' + a.admins.map((ad) => {
        const removable = a.can_remove && !ad.is_creator;
        return (
          '<div class="row"><span class="avatar sm" style="' + avatarStyle(ad.id) + '">' + esc(initial(ad.name)) + "</span>" +
          '<span class="rt"><span class="t">' + ad.name + '</span><span class="s">' + (ad.is_creator ? "مالک گروه" : ad.is_bot ? "ربات" : "ادمین") + "</span></span>" +
          (ad.is_creator ? '<span class="pill ok">مالک</span>' : ad.is_bot ? '<span class="pill mute">ربات</span>' : "") +
          (removable ? '<button class="ibtn danger" data-remove-admin="' + ad.id + '" data-name="' + esc(ad.name) + '" title="عزل">' + icon("x", 18) + "</button>" : "") +
          "</div>"
        );
      }).join("") + "</div>";
      if (!a.can_remove) html += '<div class="hint start">فقط مالک گروه می تواند ادمین عزل کند.</div>';
    }
    html += "</div>";

    html += '<div class="sec"><div class="h"><div class="t">لیست ها</div></div><div class="list">' +
      ["ban", "mute", "vip", "free"].map((k) => listRow(k)).join("") + "</div></div>";

    html += '<div class="sec"><div class="h"><div class="t">دسترسی اعضای عادی</div></div><div class="list">' +
      '<button class="row" data-push="rights"><span class="rico">' + icon("chat", 16) + '</span><span class="rt"><span class="t">چه چیزی می توانند بفرستند</span><span class="s">پیام، عکس، ویدیو، ویس، استیکر، نظرسنجی…</span></span>' + CHEV + "</button>" +
      "</div></div>";
    return html;
  }

  function listRow(kind) {
    const [title, ic, desc] = LIST_KINDS[kind];
    const cached = S.lists && S.lists[kind];
    const count = cached ? '<span class="count">' + fa(cached.entries.length) + (cached.truncated ? "+" : "") + "</span>" : "";
    return (
      '<button class="row" data-push="list:' + kind + '"><span class="rico">' + icon(ic, 16) + '</span><span class="rt"><span class="t">' + title +
      '</span><span class="s">' + desc + "</span></span>" + count + CHEV + "</button>"
    );
  }

  async function loadAdmins() {
    const chat = S.chat;
    S.admins = null;
    try {
      const a = await api("/admins");
      if (S.chat !== chat) return;
      S.admins = a;
    } catch (e) {
      if (S.chat !== chat) return;
      S.admins = "error";
    }
    if (S.tab === "members" && !S.stack.length) render();
  }


  function renderActivity() {
    const a = S.activity;
    if (!a) return '<div class="sec" style="padding-top:8px"><div class="sk" style="height:110px;border-radius:16px"></div></div>';
    if (a === "error") {
      return '<div class="sec" style="padding-top:8px"><div class="hero"><div class="glyph mute">' + icon("wifiOff", 22) + '</div><div style="flex:1"><div class="t">آمار خوانده نشد</div></div><button class="btn sm ghost" data-reload-activity>' + icon("refresh", 14) + " تلاش</button></div></div>";
    }
    const today = a.days[0];
    const shownKeys = today.counters.filter((c) => c.count > 0);
    let html = '<div class="sec" style="padding-top:4px"><div class="h" style="padding-top:6px"><div class="t">امروز</div></div>';
    if (!shownKeys.length) {
      html += '<div class="note mute">' + icon("info", 16) + "<span>امروز هنوز رویدادی ثبت نشده. شمارش از اولین حذف، سکوت یا عضو تازه شروع می شود.</span></div>";
    } else {
      html += '<div class="list">' + today.counters.map((c) =>
        '<div class="row" style="min-height:44px"><span class="rt"><span class="t" style="font-weight:400">' + c.label + '</span></span><span class="count" style="color:' + (c.count ? "var(--text)" : "var(--muted-2)") + ';font-weight:600">' + fa(c.count) + "</span></div>"
      ).join("") + "</div>";
    }
    html += "</div>";

    const labelOf = (ago) => (ago === 1 ? "دیروز" : fa(ago) + " روز پیش");
    html += '<div class="sec"><div class="h"><div class="t">هفته گذشته</div></div><div class="list">' +
      a.days.slice(1).map((d) => {
        const get = (k) => {
          const c = d.counters.find((c) => c.key === k);
          return c ? c.count : 0;
        };
        const parts = [];
        if (get("deleted")) parts.push(fa(get("deleted")) + " حذف");
        if (get("muted") + get("banned")) parts.push(fa(get("muted") + get("banned")) + " سکوت و بن");
        if (get("warned")) parts.push(fa(get("warned")) + " اخطار");
        if (get("joined")) parts.push(fa(get("joined")) + " عضو تازه");
        return '<div class="row" style="min-height:44px"><span class="rt"><span class="t" style="font-weight:400">' + labelOf(d.ago) + '</span></span><span class="count">' + (parts.length ? parts.join(" · ") : "آرام") + "</span></div>";
      }).join("") + "</div></div>";
    html += '<div class="hint">شمارش ها از ربات است و با آمار خود تلگرام فرق دارد.</div>';
    return html;
  }


  const PAGE_LOADERS = {
    locks: () => api("/locks"),
    rights: () => api("/rights"),
    log: () => api("/log"),
    welcome: () => api("/welcome"),
    joingate: () => api("/join-gate"),
    imgf: () => api("/lists/imgf"),
    voice: () => api("/voice"),
    cases: () => api("/cases?status=open").then((data) => { data.status = "open"; data.user = ""; return data; }),
    settings: () => Promise.resolve({}),
  };

  async function pushPage(id) {
    const page = { id, data: null, error: null };
    S.stack.push(page);
    S.sheet = null;
    S.confirm = null;
    render();
    window.scrollTo(0, 0);
    await loadPage(page);
  }

  async function loadPage(page) {
    const chat = S.chat;
    page.data = null;
    page.error = null;
    try {
      if (page.id.startsWith("list:")) {
        const kind = page.id.slice(5);
        page.data = await api("/lists/" + kind);
        S.lists = S.lists || {};
        S.lists[kind] = page.data;
      } else {
        page.data = await PAGE_LOADERS[page.id]();
      }
    } catch (e) {
      page.error = (e && e.detail) || "خوانده نشد.";
    }
    if (S.chat !== chat) return;
    if (S.stack[S.stack.length - 1] === page) render();
  }

  function currentPage() {
    return S.stack[S.stack.length - 1] || null;
  }

  function renderPage(page) {
    const gtitle = S.dash.chat.title;
    if (page.error) {
      return renderPageTop(pageTitle(page), gtitle) + '<div id="main" class="nobar"><div class="empty"><div class="glyph">' + icon("wifiOff", 32, 1.6) + '</div><div class="t">' + esc(page.error) + '</div><button class="btn ghost" data-retry-page>' + icon("refresh", 16) + " تلاش دوباره</button></div></div>";
    }
    if (page.id.startsWith("list:")) return renderListPage(page, page.id.slice(5));
    switch (page.id) {
      case "locks":
        return renderLocksPage(page);
      case "rights":
        return renderRightsPage(page);
      case "log":
        return renderLogPage(page);
      case "welcome":
        return renderWelcomePage(page);
      case "joingate":
        return renderJoinGatePage(page);
      case "imgf":
        return renderImgfPage(page);
      case "voice":
        return renderVoicePage(page);
      case "cases":
        return renderCasesPage(page);
      case "settings":
        return renderSettingsPage();
      default:
        return renderPageTop("", gtitle) + '<div id="main" class="nobar"></div>';
    }
  }

  function pageTitle(page) {
    if (page.id.startsWith("list:")) return LIST_KINDS[page.id.slice(5)][0];
    return { locks: "قفل ها", rights: "دسترسی اعضا", log: "لاگ", welcome: "خوشامد", joingate: "عضویت اجباری در کانال", imgf: "فیلتر تصویری", voice: "کلمه های نامناسب ویس", cases: "پرونده ها", settings: "تنظیمات گروه" }[page.id] || "";
  }

  function loadingList() {
    return '<div class="sec" style="padding-top:12px"><div class="sk" style="height:160px;border-radius:16px"></div></div>';
  }

  function caseStatus(value) {
    return { open: "باز", resolved: "بسته", reversed: "لغوشده" }[value] || value;
  }

  function caseAction(value) {
    return { none: "بدون اقدام", delete: "حذف", warn: "اخطار", mute: "سکوت", ban: "بن", kick: "کیک" }[value] || value;
  }

  function renderCasesPage(page) {
    const d = page.data;
    let html = renderPageTop("پرونده ها", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!d) return html + loadingList() + "</div>";
    if (d.detail) {
      const c = d.detail;
      html += '<div class="sec"><div class="hero"><div class="glyph ' + (c.status === "open" ? "warn" : "ok") + '">' + icon("fileText", 25) + '</div><div><div class="t">پرونده ' + fa(c.id) + '</div><div class="s">' + esc(caseStatus(c.status)) + " · " + esc(caseAction(c.action)) + '</div></div></div></div>';
      html += '<div class="sec"><div class="list"><div class="row"><span class="rt"><span class="t">' + esc(c.subject_name || "کاربر نامشخص") + '</span><span class="s">' + esc(c.reason) + '</span></span></div></div>';
      if (c.evidence) html += '<div class="note mute" style="margin-top:10px">' + esc(c.evidence) + '</div>';
      html += '</div><div class="sec"><div class="h"><div class="t">رویدادها</div></div><div class="list">' + (d.events || []).map((ev) => '<div class="row" style="min-height:44px"><span class="rt"><span class="t">' + esc(ev.kind) + '</span><span class="s">' + esc(ev.actor_name || "ربات") + (ev.note ? " · " + esc(ev.note) : "") + '</span></span></div>').join("") + '</div></div>';
      html += '<div class="sec"><textarea class="text-input" id="case-note" maxlength="500" placeholder="یادداشت اختیاری" style="height:84px;padding:12px"></textarea><div style="display:flex;gap:8px;padding-top:10px">';
      if (c.status === "open") html += '<button class="btn ghost" data-case-action="none">بررسی شد</button><button class="btn danger" data-case-action="delete">حذف پیام</button>';
      if (c.status === "resolved" && ["warn", "mute", "ban"].includes(c.action)) html += '<button class="btn soft-danger block" data-case-action="reverse">لغو اقدام</button>';
      html += '<button class="btn ghost" data-case-note>ثبت یادداشت</button></div></div></div>';
      return html;
    }
    const status = d.status || "open";
    html += '<div class="sec" style="padding-top:2px"><div class="chips"><button class="chip ' + (status === "open" ? "on" : "") + '" data-case-filter="open">باز</button><button class="chip ' + (status === "all" ? "on" : "") + '" data-case-filter="all">همه</button></div><label class="search" style="margin-top:10px">' + icon("search", 16) + '<input id="case-user" inputmode="numeric" placeholder="شناسه عددی کاربر" value="' + esc(d.user || "") + '" /></label></div>';
    html += '<div class="sec"><div class="list">' + (d.cases || []).map((c) => '<button class="row" data-case="' + c.id + '"><span class="rico ' + (c.status === "open" ? "warn" : "") + '">' + icon("fileText", 15) + '</span><span class="rt"><span class="t">' + esc(c.subject_name || "کاربر نامشخص") + '</span><span class="s">#' + fa(c.id) + " · " + esc(c.reason) + '</span></span><span class="pill ' + (c.status === "open" ? "warn" : "mute") + '">' + esc(caseStatus(c.status)) + '</span>' + CHEV + '</button>').join("") + '</div>';
    if (!(d.cases || []).length) html += '<div class="hint">پرونده ای پیدا نشد.</div>';
    if (d.has_more && d.cases.length) html += '<button class="btn ghost block" data-case-more="' + d.cases[d.cases.length - 1].id + '">بیشتر</button>';
    return html + '</div></div>';
  }


  const LOCK_GROUPS = [
    ["رسانه", ["photo", "video", "gif", "sticker", "animsticker", "music", "voice", "file", "media", "spoiler", "story"]],
    ["متن و لینک", ["links", "hyperlink", "hashtag", "username", "mention", "emoji", "premoji", "english", "persian", "edit"]],
    ["منبع و پیوست", ["forward_channel", "forward_user", "anon", "bot", "button", "commands", "botcall", "service", "pin", "comment", "promoter", "contact", "location", "poll", "dice", "biolink"]],
  ];

  function renderLocksPage(page) {
    const sum = S.dash.locks_summary;
    const right = '<button class="btn sm ghost" data-locks-menu>همه ' + icon("chevD", 14) + "</button>";
    let html = renderPageTop("قفل ها", S.dash.chat.title + " · " + fa(sum.active) + " از " + fa(sum.total) + " روشن", right) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const q = (S.lockQuery || "").trim();
    html += '<div class="sec" style="padding-top:2px"><label class="search">' + icon("search", 16) + '<input id="lock-search" placeholder="جستجوی قفل…" value="' + esc(q) + '" /></label></div>';
    if (S.locksAll) {
      html += '<div class="sec" style="padding-top:12px"><div class="chips"><button class="chip wide" data-locks-all="1">' + icon("lock", 14) + ' همه روشن</button><button class="chip wide danger" data-locks-all="0">' + icon("unlock", 14) + " همه خاموش</button></div></div>";
    }
    const plain = page.data.plain;
    const used = new Set();
    const chip = (l) =>
      '<button class="chip' + (l.on ? " on" : "") + '" data-lock="' + esc(l.key) + '">' + icon(l.icon || (l.on ? "LOCKED" : "UNLOCKED"), 13, 2.4) + l.label + "</button>";
    const groupHtml = (title, items) => {
      const shownItems = items.filter((l) => !q || l.label.includes(q));
      if (!shownItems.length) return "";
      const on = items.filter((l) => l.on).length;
      return '<div class="sec"><div class="h"><div class="t">' + title + '</div><div class="count">' + fa(on) + " از " + fa(items.length) + '</div></div><div class="chips lockchips">' + shownItems.map(chip).join("") + "</div></div>";
    };
    LOCK_GROUPS.forEach(([title, keys]) => {
      const items = keys.map((k) => plain.find((l) => l.key === k)).filter(Boolean);
      items.forEach((l) => used.add(l.key));
      html += groupHtml(title, items);
    });
    const rest = plain.filter((l) => !used.has(l.key));
    if (rest.length) html += groupHtml("سایر", rest);
    if (page.data.ai.length) html += groupHtml("هوشمند", page.data.ai);
    return html + "</div>";
  }


  function renderRightsPage(page) {
    let html = renderPageTop("دسترسی اعضا", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const open = page.data.rights.filter((r) => r.open).length;
    const allClosed = open === 0;
    html += '<div class="sec" style="padding-top:2px"><div class="note' + (allClosed ? " warn" : " mute") + '">' + icon(allClosed ? "lock" : "info", 16) + "<span>" +
      (allClosed ? "گروه بسته است: هیچ عضو عادی نمی تواند چیزی بفرستد." : "هر دسترسی که بسته شود برای همه اعضای عادی بسته می شود؛ ادمین ها استثنا هستند.") + "</span></div></div>";
    html += '<div class="sec" style="padding-top:12px"><div class="chips">' +
      '<button class="chip wide danger" data-rights-all="0"' + (allClosed ? " disabled" : "") + ">" + icon("lock", 14) + " بستن گروه</button>" +
      '<button class="chip wide" data-rights-all="1"' + (open === page.data.rights.length ? " disabled" : "") + ">" + icon("unlock", 14) + " باز کردن همه</button></div></div>";
    html += '<div class="sec"><div class="h"><div class="t">اعضای عادی می توانند</div></div><div class="list">' +
      page.data.rights.map((r) =>
        '<div class="row"><span class="rico">' + icon(r.icon || (r.open ? "UNLOCKED" : "LOCKED"), 16) + '</span><span class="rt"><span class="t" style="font-weight:400">' + r.label + '</span></span><button class="swb" data-right="' + esc(r.key) + '"><span class="sw' + (r.open ? " on" : "") + '"></span></button></div>'
      ).join("") + "</div></div>";
    return html + "</div>";
  }


  function renderLogPage(page) {
    let html = renderPageTop("لاگ", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    html += '<div class="sec" style="padding-top:2px"><div class="list"><div class="row"><span class="rico' + (d.channel ? " on" : "") + '">' + icon("fileText", 16) + '</span><span class="rt"><span class="t">کانال لاگ</span><span class="s">' +
      (d.channel ? d.channel : "تنظیم نشده · از داخل گروه «تنظیم لاگ» بفرستید") + "</span></span>" + (d.channel ? '<span class="pill ok">وصل</span>' : '<span class="pill warn">ندارد</span>') + "</div></div></div>";
    html += '<div class="sec"><div class="h"><div class="t">چه رویدادهایی ثبت شود</div></div><div class="list">' +
      d.kinds.map((k) =>
        '<div class="row"><span class="rt"><span class="t" style="font-weight:400">' + k.label + '</span></span><button class="swb" data-log="' + esc(k.key) + '"><span class="sw' + (k.on ? " on" : "") + '"></span></button></div>'
      ).join("") + "</div></div>";
    if (d.channel) html += '<div class="sec" style="padding-top:16px"><button class="btn soft-danger block" data-confirm="log-off">قطع کانال لاگ</button></div>';
    return html + "</div>";
  }


  function renderWelcomePage(page) {
    let html = renderPageTop("خوشامد", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    html += '<div class="sec" style="padding-top:2px"><div class="field"><div class="fl"><span>متن خوشامد</span>' + (d.has_media ? "<span>همراه با رسانه</span>" : "") + "</div>" +
      '<textarea class="text-input" id="welcome-text" placeholder="سلام {نام}، به {گروه} خوش آمدی!">' + esc(d.text || "") + "</textarea>" +
      '<div class="hint start">تگ ها: {نام} {منشن} {آیدی} {یوزرنیم} {گروه}' + (d.has_media ? " · رسانه فقط از داخل گروه عوض می شود" : "") + "</div></div>" +
      '<div style="display:flex;gap:8px;padding-top:12px"><button class="btn" style="flex:1" data-welcome-save>ذخیره</button>' +
      (d.text || d.has_media ? '<button class="btn soft-danger" data-confirm="welcome-off">حذف خوشامد</button>' : "") + "</div></div>";
    const wct = setting("wct");
    if (wct) html += '<div class="sec"><div class="h"><div class="t">حذف خودکار پیام خوشامد</div></div>' + renderNumberField(wct, true) + "</div>";
    return html + "</div>";
  }


  function renderJoinGatePage(page) {
    let html = renderPageTop("عضویت اجباری در کانال", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    html += '<div class="sec" style="padding-top:2px"><div class="note mute">' + icon("info", 16) + "<span>تا وقتی عضو کانال نشوند نمی توانند پیام بدهند. ربات باید در آن کانال ادمین باشد.</span></div>" +
      '<div class="field"><div class="fl"><span>یوزرنیم یا لینک کانال</span></div><input class="text-input" id="join-gate-input" dir="ltr" placeholder="@channel" value="' + esc(d.channel || "") + '" /></div>' +
      '<div style="display:flex;gap:8px;padding-top:12px"><button class="btn" style="flex:1" data-joingate-save>ذخیره</button>' +
      (d.channel ? '<button class="btn soft-danger" data-joingate-off>خاموش</button>' : "") + "</div></div>";
    return html + "</div>";
  }


  function renderImgfPage(page) {
    let html = renderPageTop("فیلتر تصویری", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const entries = page.data.entries;
    html += '<div class="sec" style="padding-top:2px"><div class="note mute">' + icon("info", 16) + "<span>یک موضوع بنویسید، مثل «خودرو لوکس»؛ هر عکسی که به آن نزدیک باشد حذف می شود.</span></div>" +
      '<div style="display:flex;gap:8px;padding-top:12px"><input class="text-input" id="filter-phrase" placeholder="موضوع تازه" style="flex:1;min-width:0" /><button class="btn" data-filter-add>افزودن</button></div></div>';
    html += '<div class="sec"><div class="h"><div class="t">موضوع های شما</div><div class="count">' + fa(entries.length) + "</div></div>";
    if (!entries.length) html += '<div class="hint">هنوز موضوعی نساخته اید.</div>';
    else {
      html += '<div class="list">' + entries.map((e) => {
        const live = !e.name.endsWith(" ~");
        const label = live ? e.name : e.name.slice(0, -2);
        return '<div class="row"><span class="rico' + (live ? " on" : "") + '">' + icon("image", 16) + '</span><span class="rt"><span class="t">' + label + '</span><span class="s">' + (live ? "فعال" : "موقتا خاموش") + "</span></span>" +
          '<button class="ibtn" data-filter-toggle="' + esc(e.key) + '" title="' + (live ? "خاموش" : "روشن") + '">' + icon(live ? "pause" : "play", 18) + "</button>" +
          '<button class="ibtn danger" data-remove="imgf" data-key="' + esc(e.key) + '">' + icon("x", 18) + "</button></div>";
      }).join("") + "</div>";
    }
    return html + "</div></div>";
  }


  function renderVoicePage(page) {
    let html = renderPageTop("کلمه های نامناسب ویس", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    html += '<div class="sec" style="padding-top:2px"><div style="display:flex;gap:8px"><input class="text-input" id="voice-word" placeholder="کلمه تازه" style="flex:1;min-width:0" /><button class="btn" data-voice-add>افزودن</button></div></div>';
    html += '<div class="sec"><div class="h"><div class="t">فهرست</div><div class="count">' + fa(d.words.length) + "</div></div>";
    if (!d.words.length) html += '<div class="hint">فهرست خالی است.</div>';
    else {
      html += '<div class="list">' + d.words.map((w) =>
        '<div class="row" style="min-height:44px"><span class="rt"><span class="t" style="font-weight:400">' + w.word + '</span></span><button class="ibtn danger" data-voice-remove="' + esc(w.key) + '">' + icon("x", 18) + "</button></div>"
      ).join("") + "</div>";
    }
    html += "</div>";
    if (d.disabled_defaults.length) html += '<div class="sec" style="padding-top:16px"><button class="btn ghost block" data-voice-restore>بازگرداندن ' + fa(d.disabled_defaults.length) + " کلمه پیش فرض</button></div>";
    return html + "</div>";
  }


  const LIST_SEARCH_THRESHOLD = 8;

  function renderListPage(page, kind) {
    const [title, ic] = LIST_KINDS[kind];
    const d = page.data;
    const right = d && d.entries.length ? '<button class="ibtn danger" data-confirm="clear:' + kind + '" title="پاکسازی لیست">' + icon("trash", 20) + "</button>" : "";
    let html = renderPageTop(title, S.dash.chat.title + (d ? " · " + fa(d.entries.length) + (d.truncated ? "+" : "") + " مورد" : ""), right) + '<div id="main" class="nobar">';
    if (!d) return html + loadingList() + "</div>";
    const q = (S.listQuery || "").trim().toLowerCase();
    if (d.entries.length > LIST_SEARCH_THRESHOLD) {
      html += '<div class="sec" style="padding-top:2px"><label class="search">' + icon("search", 16) + '<input id="list-search" placeholder="جستجو در ' + fa(d.entries.length) + ' مورد…" value="' + esc(S.listQuery || "") + '" /></label></div>';
    }
    const shownEntries = d.entries.filter((e) => !q || e.name.toLowerCase().includes(q));
    html += '<div class="sec" style="padding-top:12px">';
    if (!d.entries.length) html += '<div class="empty" style="padding-top:60px"><div class="glyph">' + icon(ic, 30, 1.6) + '</div><div class="t">لیست خالی است</div><div class="s">' + (LIST_KINDS[kind][2] || "") + "</div></div>";
    else if (!shownEntries.length) html += '<div class="hint">چیزی پیدا نشد.</div>';
    else {
      html += '<div class="list">' + shownEntries.map((e) => {
        const pending = S.pending === kind + ":" + e.key;
        return '<div class="row"><span class="rico">' + icon(ic, 15) + '</span><span class="rt"><span class="t" style="font-weight:400">' + e.name + "</span></span>" +
          (pending
            ? '<button class="btn sm danger" data-remove="' + kind + '" data-key="' + esc(e.key) + '">حذف؟</button>'
            : '<button class="ibtn danger" data-remove="' + kind + '" data-key="' + esc(e.key) + '">' + icon("x", 18) + "</button>") +
          "</div>";
      }).join("") + "</div>";
      if (d.truncated) html += '<div class="hint">بیش از ' + fa(d.entries.length) + " مورد — برای یافتن مورد خاص جستجو کنید.</div>";
    }
    return html + "</div></div>";
  }


  function renderSettingsPage() {
    const g = group();
    const health = S.health && S.health !== "error" ? S.health : null;
    const flagRow = (id, ic, title, sub) => {
      const it = setting(id);
      if (!it) return "";
      return '<div class="row"><span class="rico' + (it.on ? " on" : "") + '">' + icon(ic, 16) + '</span><span class="rt"><span class="t">' + title + '</span><span class="s">' + sub + '</span></span><button class="swb" data-apply="' + id + '"><span class="sw' + (it.on ? " on" : "") + '"></span></button></div>';
    };
    const sheetRow = (fid, ic, title, sub) =>
      '<button class="row" data-open="' + fid + '"><span class="rico">' + icon(ic, 16) + '</span><span class="rt"><span class="t">' + title + '</span><span class="s">' + sub + "</span></span>" + CHEV + "</button>";
    const pageRow = (pid, ic, title, sub, on) =>
      '<button class="row" data-push="' + pid + '"><span class="rico' + (on ? " on" : "") + '">' + icon(ic, 16) + '</span><span class="rt"><span class="t">' + title + '</span><span class="s">' + sub + "</span></span>" + CHEV + "</button>";

    let html = renderPageTop("تنظیمات گروه", S.dash.chat.title) + '<div id="main" class="nobar">';
    html += '<div class="sec" style="padding-top:2px"><div class="list"><div class="row" style="min-height:64px"><span class="avatar lg" style="' + avatarStyle(S.chat) + '">' + esc(initial(S.dash.chat.title)) + "</span>" +
      '<span class="rt"><span class="t">' + S.dash.chat.title + '</span><span class="s">' + (S.dash.viewer.is_owner ? "شما مالک هستید" : "شما ادمین هستید") + (g && !g.known ? "" : "") + "</span></span>" +
      (health ? (health.bot_admin ? '<span class="pill ok">' + icon("check", 12, 3) + " ربات ادمین</span>" : '<span class="pill bad">ربات ادمین نیست</span>') : "") + "</div></div></div>";

    html += '<div class="sec"><div class="h"><div class="t">ورود اعضا</div></div><div class="list">' +
      pageRow("joingate", "door", "عضویت اجباری در کانال", "تا عضو کانال نشوند پیام نمی دهند") +
      sheetRow("ad", "userPlus", "اد اجباری", featureState(feature("ad")) ? feature("ad").effect() : "خاموش") +
      flagRow("lb_on", "refresh", "بازگشت اعضای خارج شده", "کسی که بیرون رفت دوباره اضافه می شود") +
      "</div></div>";

    html += '<div class="sec"><div class="h"><div class="t">لاگ و گزارش</div></div><div class="list">' +
      pageRow("log", "fileText", "کانال لاگ", "رویدادهای گروه کجا ثبت شود") +
      pageRow("cases", "shield", "پرونده ها", "گزارش ها و سابقه برخوردها") +
      sheetRow("dr", "send", "گزارش روزانه", sectionEnabled("dr") ? feature("dr").effect() : "خاموش") +
      "</div></div>";

    html += '<div class="sec"><div class="h"><div class="t">ادمین ها</div></div><div class="list">' +
      (S.dash.viewer.is_owner
        ? '<button class="row" data-open="lim"><span class="rico' + (isOn("lim_on") ? " on" : "") + '">' + icon("shieldOff", 16) + '</span><span class="rt"><span class="t">محدودیت مدیران</span><span class="s">' + (isOn("lim_on") ? "روشن · هر ادمین فقط کارهای مجازش" : "خاموش") + "</span></span>" + CHEV + "</button>"
        : '<div class="row"><span class="rico">' + icon("shieldOff", 16) + '</span><span class="rt"><span class="t">محدودیت مدیران</span><span class="s">فقط مالک ربات</span></span></div>') +
      flagRow("rk_on", "star", "مقام خودکار", "با فعالیت، عنوان می گیرند") +
      sheetRow("nt", "bell", "اعلان حذف", isOn("nt_on") ? "روشن · پاک شدن پس از " + shown("nt_t") + " ثانیه" : "خاموش") +
      "</div></div>";

    html += '<div class="sec"><div class="h"><div class="t">پاسخ ها و دستورها</div></div><div class="list">' +
      '<button class="row" data-open="response-policy"><span class="rico">' + icon("chat", 16) + '</span><span class="rt"><span class="t">پیام های ربات</span><span class="s">خصوصی کردن اعلان ها و خوشامد</span></span>' + CHEV + "</button>" +
      pageRow("list:answer", "chat", "پاسخ خودکار", "کلمه و پاسخ آماده") +
      pageRow("list:cmd", "terminal", "دستور های سفارشی", "") +
      pageRow("list:pack", "sparkles", "پک های استیکر", "پک هایی که حذف می شوند") +
      "</div></div>";
    return html + "</div>";
  }


  function openSheet(fid) {
    const f = feature(fid) || EXTRA_SHEETS[fid];
    if (!f) return;
    if (f.page) {
      pushPage(f.page);
      return;
    }
    S.sheet = { id: fid, data: null };
    S.confirm = null;
    render();
    if (f.custom === "ai") loadSheetData("/locks");
    if (f.custom === "response_policy") loadSheetData("/response-policy");
  }

  async function loadSheetData(path) {
    const sheet = S.sheet;
    try {
      const d = await api(path);
      if (S.sheet !== sheet) return;
      sheet.data = d;
      drawSheet();
    } catch (e) {
      report(e);
    }
  }

  function closeSheet() {
    S.sheet = null;
    S.pending = null;
    const dim = document.getElementById("dim");
    const sheet = document.getElementById("sheet");
    if (dim) dim.classList.remove("open");
    if (sheet) sheet.classList.remove("open");
    syncBack();
  }

  const EXTRA_SHEETS = {
    lim: { id: "lim", icon: "shieldOff", title: "محدودیت مدیران", desc: "با روشن بودن، هر ادمین فقط کارهایی را می تواند بکند که مالک به او داده", flag: "lim_on", sections: ["lim"] },
    nt: { id: "nt", icon: "bell", title: "اعلان حذف", desc: "وقتی پیامی حذف شد، به فرستنده اش گفته می شود چرا", flag: "nt_on", sections: ["nt"] },
    "response-policy": { id: "response-policy", icon: "chat", title: "پیام های ربات", desc: "فقط اعلان های حذف و خوشامد؛ بقیه پیام ها عادی می مانند", custom: "response_policy" },
  };

  function drawSheet() {
    const sheetEl = document.getElementById("sheet");
    const dim = document.getElementById("dim");
    if (!sheetEl || !S.sheet) return;
    if (S.sheet.id === "picker") return openPicker();
    const f = feature(S.sheet.id) || EXTRA_SHEETS[S.sheet.id];
    sheetEl.innerHTML = renderSheetBody(f);
    requestAnimationFrame(() => {
      dim.classList.add("open");
      sheetEl.classList.add("open");
    });
  }

  function masterControl(f) {
    if (f.always || f.neutral) return "";
    if (f.flag) return '<button class="swb" data-apply="' + f.flag + '"><span class="sw' + (isOn(f.flag) ? " on" : "") + '"></span></button>';
    if (f.enabled) {
      const on = sectionEnabled(f.enabled);
      return '<button class="swb" data-power="' + f.id + '"><span class="sw' + (on ? " on" : "") + '"></span></button>';
    }
    if (f.flags) return "";
    return "";
  }

  function renderSheetBody(f) {
    const st = featureState(f);
    let html = '<div class="handle"></div><div class="sheet-head"><span class="rico' + (st ? " on" : "") + '">' + icon(f.icon, 20) + '</span><div style="flex:1;min-width:0"><div class="t">' + f.title + '</div><div class="s">' + f.desc + "</div></div>" + masterControl(f) + "</div>";

    if (st === true && f.effect) {
      html += '<div class="note">' + icon("info", 16) + "<span>الان: " + esc(f.effect()) + "</span></div>";
    } else if (st === false && f.enabled) {
      html += '<div class="note mute">' + icon("info", 16) + "<span>خاموش است. با انتخاب یک ساعت روشن می شود.</span></div>";
    }

    const adv = [];
    (f.sections || []).forEach((sid) => {
      const s = section(sid);
      if (!s) return;
      s.settings.forEach((it) => {
        if (it.id === f.flag) return;
        if (it.kind === "flag") {
          adv.push(it);
          return;
        }
        if (it.kind === "number") html += renderNumberField(it, false);
        else if (it.kind === "pick") html += renderPickField(it);
      });
    });
    (f.flags || []).forEach((id) => {
      const it = setting(id);
      if (it) html += '<div class="row" style="padding-right:2px;padding-left:2px"><span class="rt"><span class="t" style="font-weight:400">' + (it.icon ? icon(it.icon, 14) : "") + it.label + '</span></span><button class="swb" data-apply="' + it.id + '"><span class="sw' + (it.on ? " on" : "") + '"></span></button></div>';
    });

    if (f.custom === "ai") html += renderAiBlock();
    if (f.custom === "response_policy") html += renderResponsePolicyBlock();
    if (f.custom === "voice") html += '<div class="list" style="margin-top:12px"><button class="row" data-push="voice"><span class="rico">' + icon("funnel", 16) + '</span><span class="rt"><span class="t">کلمه های نامناسب</span><span class="s">فهرست را ببینید و کم و زیاد کنید</span></span>' + CHEV + "</button></div>";
    if (f.custom === "answers") html += '<div class="list" style="margin-top:12px"><button class="row" data-push="list:answer"><span class="rico">' + icon("chat", 16) + '</span><span class="rt"><span class="t">کلمه ها و پاسخ ها</span><span class="s">از داخل گروه با «تنظیم پاسخ» اضافه می شود</span></span>' + CHEV + "</button></div>";

    if (adv.length) {
      const open = S.adv.has(f.id);
      html += '<div class="disclose"><button class="row" data-adv="' + f.id + '"><span class="rt"><span class="t">تنظیمات پیشرفته</span><span class="s">' + adv.map((a) => a.label).join("، ") + '</span></span><span class="chev">' + icon(open ? "chevD" : "chevL", 18) + "</span></button>";
      if (open) {
        html += '<div class="list">' + adv.map((it) =>
          '<div class="row"><span class="rt"><span class="t" style="font-weight:400">' + (it.icon ? icon(it.icon, 14) : "") + it.label + '</span></span><button class="swb" data-apply="' + it.id + '"><span class="sw' + (it.on ? " on" : "") + '"></span></button></div>'
        ).join("") + "</div>";
      }
      html += "</div>";
    }
    return html;
  }

  function renderAiBlock() {
    const d = S.sheet && S.sheet.data;
    if (!d) return '<div class="sk" style="height:120px;border-radius:16px;margin-top:12px"></div>';
    return '<div class="h" style="padding-top:14px"><div class="t">موضوع ها</div><div class="count">' + fa(d.ai.filter((l) => l.on).length) + " از " + fa(d.ai.length) + '</div></div><div class="chips lockchips">' +
      d.ai.map((l) => '<button class="chip' + (l.on ? " on" : "") + '" data-lock="' + esc(l.key) + '">' + icon(l.icon || (l.on ? "LOCKED" : "UNLOCKED"), 13, 2.4) + l.label + "</button>").join("") + "</div>" +
      '<div class="list" style="margin-top:14px"><button class="row" data-push="imgf"><span class="rico">' + icon("image", 16) + '</span><span class="rt"><span class="t">فیلتر تصویری شما</span><span class="s">موضوع های دلخواه</span></span>' + CHEV + "</button></div>";
  }

  function policyVisibilityLabel(value) {
    return { default: "پیش فرض", public: "عمومی", private: "خصوصی" }[value] || value;
  }

  function policyChoice(kind, id, visibility, selected) {
    return '<button class="chip' + (selected ? " on" : "") + '" data-policy-choice="' + esc(kind + ":" + id + ":" + visibility) + '">' + policyVisibilityLabel(visibility) + "</button>";
  }

  function renderResponsePolicyBlock() {
    const d = S.sheet && S.sheet.data;
    if (!d) return '<div class="sk" style="height:180px;border-radius:16px;margin-top:12px"></div>';
    let html = '<div class="note">' + icon("info", 16) + '<span>عمومی یعنی پیام عادی گروه؛ خصوصی فقط به همان کاربر نشان داده می شود.</span></div>';
    html += '<div class="list" style="margin-top:12px">' + d.overrides.map((item) => '<div class="field"><div class="fl"><span>' + esc(item.label) + '</span><b>' + policyVisibilityLabel(item.visibility) + '</b></div><div class="chips">' +
      policyChoice("kind", item.id, "public", item.visibility === "public") +
      policyChoice("kind", item.id, "private", item.visibility === "private") +
      "</div></div>").join("") + "</div>";
    html += '<button class="btn ghost" style="margin-top:14px;width:100%" data-policy-reset>بازگشت هر دو به عمومی</button>';
    return html;
  }

  function renderNumberField(it, standalone) {
    const chips = (it.presets || []).map((p) =>
      '<button class="chip' + (p.value === it.value ? " on" : "") + '" data-apply="' + it.id + ":" + p.value + '">' + fa(p.shown) + "</button>"
    ).join("");
    const custom = it.clock
      ? '<div class="inline-num"' + (chips ? "" : ' style="margin-top:0"') + '><input type="time" class="text-input" dir="ltr" value="' + minutesToClock(it.value) + '" data-clock-apply="' + it.id + '" data-clock-value="' + it.value + '" /></div>'
      : "";
    const numChip = it.clock
      ? ""
      : '<input type="number" class="chip-input" inputmode="numeric" min="' + it.range[0] + '" max="' + it.range[1] + '" value="' + it.value + '" data-number-apply="' + it.id + '" aria-label="دلخواه" />';
    return '<div class="field' + (standalone ? '" style="padding-top:0' : "") + '"><div class="fl"><span>' + (it.icon ? icon(it.icon, 14) : "") + it.label + "</span><b>" + fa(it.shown) + "</b></div>" +
      '<div class="chips">' + chips + numChip + "</div>" + custom + "</div>";
  }

  function renderPickField(it) {
    return '<div class="field"><div class="fl"><span>' + (it.icon ? icon(it.icon, 14) : "") + it.label + '</span></div><div class="chips">' +
      it.options.map((o) => '<button class="chip' + (o.value === it.chosen ? " on" : "") + (o.danger ? " danger" : "") + '" data-apply="' + o.id + '">' + (o.icon ? icon(o.icon, 14) : "") + o.label + "</button>").join("") + "</div></div>";
  }


  function openPicker() {
    S.sheet = { id: "picker", data: null };
    S.confirm = null;
    const sheetEl = document.getElementById("sheet");
    const dim = document.getElementById("dim");
    if (!sheetEl) return;
    const q = (S.groupQuery || "").trim();
    const groups = (S.groups || []).filter((g) => !q || g.title.includes(q));
    sheetEl.innerHTML =
      '<div class="handle"></div><div style="display:flex;align-items:center;justify-content:space-between;padding:0 4px 10px"><div style="font-size:16px;font-weight:700">گروه های شما</div><div class="count">' + fa(S.groups.length) + " گروه</div></div>" +
      (S.groups.length > 6 ? '<label class="search" style="margin-bottom:10px">' + icon("search", 16) + '<input id="group-search" placeholder="جستجوی گروه…" value="' + esc(q) + '" /></label>' : "") +
      '<div class="list" style="background:transparent">' +
      groups.map((g) => {
        const current = g.id === S.chat;
        return '<button class="row tap" data-pick-group="' + g.id + '"' + (g.known ? "" : " disabled") + ' style="padding-right:6px;padding-left:6px;min-height:56px' + (g.known ? "" : ";opacity:.55") + '">' +
          '<span class="avatar lg" style="' + avatarStyle(g.id) + '">' + esc(initial(g.title)) + "</span>" +
          '<span class="rt"><span class="t">' + g.title + '</span><span class="s">' + (g.is_owner ? "مالک" : "ادمین") + (g.known ? "" : " · ربات در دسترس نیست") + "</span></span>" +
          (current ? '<span class="chev" style="color:var(--accent-text)">' + icon("check", 20, 2.5) + "</span>" : g.known ? "" : '<span class="pill bad">بدون دسترسی</span>') +
          "</button>";
      }).join("") +
      (groups.length ? "" : '<div class="hint">گروهی با این نام نیست.</div>') +
      "</div>" +
      '<div style="display:flex;align-items:center;gap:10px;margin-top:8px;padding:12px 6px 0;border-top:1px solid var(--border);color:var(--muted);font-size:12.5px">' + icon("info", 16) + "<span>گروه دیگری دارید؟ ربات را در آن ادمین کنید تا اینجا اضافه شود.</span></div>";
    requestAnimationFrame(() => {
      dim.classList.add("open");
      sheetEl.classList.add("open");
    });
    syncBack();
  }


  const CONFIRMS = {
    "log-off": () => ({ title: "کانال لاگ قطع شود؟", sub: "رویدادها دیگر جایی ثبت نمی شود؛ بعدا از داخل گروه دوباره تنظیم می کنید.", label: "قطع", run: () => api("/log/off", { method: "POST" }).then(() => reloadTopPage()) }),
    "welcome-off": () => ({ title: "پیام خوشامد حذف شود؟", sub: "متن و رسانه اش پاک می شود.", label: "حذف", run: () => api("/welcome/off", { method: "POST" }).then(() => reloadTopPage()) }),
  };

  function confirmFor(key) {
    if (CONFIRMS[key]) return CONFIRMS[key]();
    if (key.startsWith("clear:")) {
      const kind = key.slice(6);
      const d = S.lists && S.lists[kind];
      const n = d ? d.entries.length : 0;
      return {
        title: "همه " + fa(n) + " مورد از «" + LIST_KINDS[kind][0] + "» حذف شود؟",
        sub: "برگشت ندارد." + (kind === "ban" ? " کسانی که بن شده اند می توانند دوباره بیایند." : ""),
        label: "حذف همه",
        run: () => api("/lists/" + kind, { method: "DELETE" }).then(() => reloadTopPage()),
      };
    }
    if (key.startsWith("admin:")) {
      const [, id, name] = key.split(":");
      return {
        title: "«" + name + "» عزل شود؟",
        sub: "از ادمینی گروه و ربات برداشته می شود. ربات فقط ادمین هایی را می تواند عزل کند که خودش اضافه کرده.",
        label: "عزل",
        run: () => api("/admins/" + id, { method: "DELETE" }).then(() => loadAdmins()),
      };
    }
    if (key === "locks-off") {
      return { title: "همه قفل ها خاموش شود؟", sub: "هر چیزی دوباره در گروه فرستاده می شود تا قفل ها را برگردانید.", label: "خاموش کردن همه", run: () => setAllLocks(false) };
    }
    if (key === "rights-close") {
      return { title: "گروه بسته شود؟", sub: "هیچ عضو عادی نمی تواند چیزی بفرستد؛ ادمین ها می توانند. هر وقت خواستید از همین جا باز کنید.", label: "بستن گروه", run: () => setAllRights(false) };
    }
    return null;
  }

  function renderConfirm() {
    const c = S.confirm;
    if (!c) return "";
    return '<div class="confirm"><div class="t">' + esc(c.title) + '</div><div class="s">' + esc(c.sub) + '</div><div class="b"><button class="btn ghost" data-confirm-cancel>انصراف</button><button class="btn danger" data-confirm-run>' + esc(c.label) + "</button></div></div>";
  }

  async function reloadTopPage() {
    const page = currentPage();
    if (page) await loadPage(page);
    else render();
  }


  async function afterWrite() {
    render();
  }

  async function applySetting(action) {
    const ok = await write(async () => {
      const d = await api("/settings/apply", { method: "POST", body: { action } });
      S.dash = d;
    });
    if (!ok) return;
    haptic();
    afterWrite();
  }

  async function applyPolicy(action) {
    if (!S.sheet || S.sheet.id !== "response-policy") return;
    const ok = await write(async () => {
      S.sheet.data = await api("/response-policy/apply", { method: "POST", body: action });
    });
    if (!ok) return;
    haptic();
    drawSheet();
  }

  async function toggleLock(key) {
    const page = currentPage();
    const ok = await write(async () => {
      S.dash = await api("/locks/" + encodeURIComponent(key) + "/toggle", { method: "POST" });
    });
    if (!ok) return;
    haptic();
    const flip = (arr) => arr.forEach((l) => { if (l.key === key) l.on = !l.on; });
    if (page && page.id === "locks" && page.data) {
      flip(page.data.plain);
      flip(page.data.ai);
    }
    if (S.sheet && S.sheet.data) flip(S.sheet.data.ai);
    render();
  }

  async function setAllLocks(on) {
    const ok = await write(async () => {
      S.dash = await api("/locks/all", { method: "POST", body: { on } });
    });
    if (!ok) return;
    haptic("ok");
    S.locksAll = false;
    await reloadTopPage();
  }

  async function setAllRights(open) {
    const page = currentPage();
    if (!page || !page.data) return;
    const targets = page.data.rights.filter((r) => r.open !== open);
    for (const r of targets) {
      const ok = await write(() => api("/rights/" + encodeURIComponent(r.key) + "/toggle", { method: "POST" }));
      if (!ok) break;
    }
    haptic("ok");
    await reloadTopPage();
  }

  async function powerFeature(fid) {
    const f = feature(fid);
    if (!f || !f.enabled) return;
    if (sectionEnabled(f.enabled)) {
      const ok = await write(async () => {
        S.dash = await api(f.off, { method: "POST" });
      });
      if (!ok) return;
    } else {
      const ok = await write(async () => {
        S.dash = await api("/settings/apply", { method: "POST", body: { action: f.onAction() } });
      });
      if (!ok) return;
    }
    haptic();
    render();
  }

  async function removeEntry(kind, key) {
    const page = currentPage();
    const ok = await write(() => api("/lists/" + kind + "/" + encodeURIComponent(key), { method: "DELETE" }));
    S.pending = null;
    if (!ok) {
      render();
      return;
    }
    haptic();
    if (page && page.data) {
      page.data.entries = page.data.entries.filter((e) => e.key !== key);
      if (S.lists) S.lists[kind] = page.data;
    }
    render();
  }


  app.addEventListener("click", onClick);
  app.addEventListener("keydown", onKeydown);
  app.addEventListener("focusout", onFocusOut);
  app.addEventListener("input", onInput);

  async function onClick(e) {
    const t = e.target;
    const hit = (sel) => t.closest(sel);
    let el;

    if ((el = hit("[data-tab]"))) {
      S.tab = el.dataset.tab;
      S.stack = [];
      if (S.tab === "members" && !S.admins) loadAdmins();
      render();
      window.scrollTo(0, 0);
      return;
    }
    if (hit("[data-back]")) return back();
    if (hit("[data-close]")) {
      if (S.sheet) closeSheet();
      return;
    }
    if (hit("[data-picker]")) return openPicker();
    if ((el = hit("[data-pick-group]"))) {
      const id = Number(el.dataset.pickGroup);
      closeSheet();
      if (id !== S.chat) loadChat(id);
      return;
    }
    if ((el = hit("[data-push]"))) return pushPage(el.dataset.push);
    if ((el = hit("[data-open]"))) return openSheet(el.dataset.open);
    if ((el = hit("[data-fix]"))) return fixIssue(el.dataset.fix);
    if (hit("[data-recheck]")) {
      S.health = null;
      render();
      loadHealth();
      return;
    }
    if (hit("[data-reload-activity]")) {
      S.activity = null;
      render();
      loadActivity();
      return;
    }
    if (hit("[data-reload-admins]")) return loadAdmins();
    if (hit("[data-retry-page]")) {
      const page = currentPage();
      if (page) {
        render();
        loadPage(page);
      }
      return;
    }
    if ((el = hit("[data-adv]"))) {
      const id = el.dataset.adv;
      if (S.adv.has(id)) S.adv.delete(id);
      else S.adv.add(id);
      drawSheet();
      return;
    }
    if ((el = hit("[data-policy-choice]"))) {
      const parts = el.dataset.policyChoice.split(":");
      if (parts.length !== 3) return;
      if (parts[0] !== "kind") return;
      return applyPolicy({ action: "set_kind", kind: parts[1], visibility: parts[2] });
    }
    if (hit("[data-policy-reset]")) {
      return applyPolicy({ action: "reset" });
    }
    if ((el = hit("[data-apply]"))) return applySetting(el.dataset.apply);
    if ((el = hit("[data-power]"))) return powerFeature(el.dataset.power);
    if ((el = hit("[data-off]"))) {
      const ok = await write(async () => {
        S.dash = await api(el.dataset.off, { method: "POST" });
      });
      if (ok) {
        haptic();
        render();
      }
      return;
    }
    if ((el = hit("[data-lock]"))) return toggleLock(el.dataset.lock);
    if (hit("[data-locks-menu]")) {
      S.locksAll = !S.locksAll;
      render();
      return;
    }
    if ((el = hit("[data-locks-all]"))) {
      if (el.dataset.locksAll === "1") return setAllLocks(true);
      S.confirm = confirmFor("locks-off");
      render();
      return;
    }
    if ((el = hit("[data-right]"))) {
      const key = el.dataset.right;
      const page = currentPage();
      const ok = await write(async () => {
        const d = await api("/rights/" + encodeURIComponent(key) + "/toggle", { method: "POST" });
        if (page) page.data = d;
      });
      if (ok) {
        haptic();
        render();
      }
      return;
    }
    if ((el = hit("[data-rights-all]"))) {
      if (el.dataset.rightsAll === "1") return setAllRights(true);
      S.confirm = confirmFor("rights-close");
      render();
      return;
    }
    if ((el = hit("[data-log]"))) {
      const key = el.dataset.log;
      const page = currentPage();
      const ok = await write(async () => {
        const d = await api("/log/" + encodeURIComponent(key) + "/toggle", { method: "POST" });
        if (page) page.data = d;
      });
      if (ok) {
        haptic();
        render();
      }
      return;
    }
    if ((el = hit("[data-confirm]"))) {
      S.confirm = confirmFor(el.dataset.confirm);
      render();
      return;
    }
    if ((el = hit("[data-remove-admin]"))) {
      S.confirm = confirmFor("admin:" + el.dataset.removeAdmin + ":" + el.dataset.name);
      render();
      return;
    }
    if (hit("[data-confirm-cancel]")) {
      S.confirm = null;
      render();
      return;
    }
    if (hit("[data-confirm-run]")) {
      const c = S.confirm;
      S.confirm = null;
      render();
      const ok = await write(c.run);
      if (ok) {
        haptic("ok");
        toast("انجام شد.", true);
        render();
      }
      return;
    }
    if ((el = hit("[data-remove]"))) {
      const kind = el.dataset.remove;
      const key = el.dataset.key;
      if (kind === "imgf") {
        const ok = await write(() => api("/lists/imgf/" + encodeURIComponent(key), { method: "DELETE" }));
        if (ok) reloadTopPage();
        return;
      }
      const tag = kind + ":" + key;
      if (S.pending === tag) return removeEntry(kind, key);
      S.pending = tag;
      render();
      clearTimeout(onClick.pendingTimer);
      onClick.pendingTimer = setTimeout(() => {
        if (S.pending === tag) {
          S.pending = null;
          render();
        }
      }, 3000);
      return;
    }
    if (S.pending && !hit("[data-remove]")) {
      S.pending = null;
      render();
    }

    if ((el = hit("[data-filter-toggle]"))) {
      const ok = await write(() => api("/filters/" + encodeURIComponent(el.dataset.filterToggle) + "/toggle", { method: "POST" }));
      if (ok) reloadTopPage();
      return;
    }
    if (hit("[data-filter-add]")) {
      const input = document.getElementById("filter-phrase");
      const phrase = input.value.trim();
      if (!phrase) return;
      const ok = await write(() => api("/filters", { method: "POST", body: { phrase } }));
      if (ok) {
        toast("موضوع اضافه شد.", true);
        reloadTopPage();
      }
      return;
    }
    if (hit("[data-voice-add]")) {
      const input = document.getElementById("voice-word");
      const word = input.value.trim();
      if (!word) return;
      const ok = await write(() => api("/voice/words", { method: "POST", body: { word } }));
      if (ok) reloadTopPage();
      return;
    }
    if ((el = hit("[data-voice-remove]"))) {
      const ok = await write(() => api("/voice/words/" + encodeURIComponent(el.dataset.voiceRemove), { method: "DELETE" }));
      if (ok) reloadTopPage();
      return;
    }
    if (hit("[data-voice-restore]")) {
      const ok = await write(() => api("/voice/restore", { method: "POST" }));
      if (ok) reloadTopPage();
      return;
    }
    if ((el = hit("[data-case]"))) {
      const page = currentPage();
      const detail = await api("/cases/" + el.dataset.case);
      if (page && page.id === "cases") {
        page.data.detail = detail;
        render();
        window.scrollTo(0, 0);
      }
      return;
    }
    if ((el = hit("[data-case-filter]"))) {
      const page = currentPage();
      if (!page) return;
      const status = el.dataset.caseFilter;
      const data = await api("/cases?status=" + status);
      data.status = status;
      data.user = "";
      page.data = data;
      render();
      return;
    }
    if ((el = hit("[data-case-more]"))) {
      const page = currentPage();
      if (!page || !page.data) return;
      const status = page.data.status || "open";
      const user = page.data.user ? "&user_id=" + encodeURIComponent(page.data.user) : "";
      const data = await api("/cases?status=" + status + "&before_id=" + el.dataset.caseMore + user);
      page.data.cases = page.data.cases.concat(data.cases || []);
      page.data.has_more = data.has_more;
      render();
      return;
    }
    if ((el = hit("[data-case-action]"))) {
      const page = currentPage();
      if (!page || !page.data || !page.data.detail) return;
      const id = page.data.detail.id;
      const note = (document.getElementById("case-note") || {}).value || "";
      const action = el.dataset.caseAction;
      const path = action === "reverse" ? "/cases/" + id + "/reverse" : "/cases/" + id + "/resolve";
      const body = action === "reverse" ? (note ? { note } : {}) : (note ? { action, note } : { action });
      const ok = await write(() => api(path, { method: "POST", body }));
      if (ok) {
        page.data.detail = await api("/cases/" + id);
        toast("پرونده به روز شد.", true);
        render();
      }
      return;
    }
    if (hit("[data-case-note]")) {
      const page = currentPage();
      if (!page || !page.data || !page.data.detail) return;
      const input = document.getElementById("case-note");
      const note = input.value.trim();
      if (!note) return;
      const id = page.data.detail.id;
      const ok = await write(() => api("/cases/" + id + "/notes", { method: "POST", body: { note } }));
      if (ok) {
        page.data.detail = await api("/cases/" + id);
        toast("یادداشت ثبت شد.", true);
        render();
      }
      return;
    }
    if (hit("[data-welcome-save]")) {
      const text = document.getElementById("welcome-text").value;
      const ok = await write(() => api("/welcome", { method: "POST", body: { text } }));
      if (ok) {
        toast("ذخیره شد.", true);
        reloadTopPage();
      }
      return;
    }
    if (hit("[data-joingate-save]")) {
      const channel = document.getElementById("join-gate-input").value;
      const ok = await write(() => api("/join-gate", { method: "POST", body: { channel } }));
      if (ok) {
        toast("ذخیره شد.", true);
        reloadTopPage();
      }
      return;
    }
    if (hit("[data-joingate-off]")) {
      const ok = await write(() => api("/join-gate", { method: "POST", body: { channel: "" } }));
      if (ok) reloadTopPage();
      return;
    }
  }

  function fixIssue(fix) {
    if (fix === "rights") {
      S.confirm = null;
      toast("در تلگرام: تنظیمات گروه ← ادمین ها ← ربات ← همه دسترسی ها را روشن کنید.", true);
      return;
    }
    if (fix === "log") return pushPage("log");
    if (fix.startsWith("feature:")) return openSheet(fix.slice(8));
  }

  function onKeydown(e) {
    if (e.key !== "Enter") return;
    const t = e.target;
    let el;
    if ((el = t.closest("[data-number-apply]"))) {
      e.preventDefault();
      applySetting(el.dataset.numberApply + ":" + el.value);
      return;
    }
    if (t.id === "filter-phrase") document.querySelector("[data-filter-add]").click();
    if (t.id === "voice-word") document.querySelector("[data-voice-add]").click();
    if (t.id === "join-gate-input") document.querySelector("[data-joingate-save]").click();
    if (t.id === "case-user") {
      const page = currentPage();
      const user = t.value.trim();
      if (!page || (user && !/^-?\d+$/.test(user))) return;
      const status = page.data.status || "open";
      api("/cases?status=" + status + (user ? "&user_id=" + encodeURIComponent(user) : "")).then((data) => {
        data.status = status;
        data.user = user;
        page.data = data;
        render();
      }).catch(report);
    }
  }

  function onFocusOut(e) {
    const clock = e.target.closest("[data-clock-apply]");
    if (!clock) return;
    const minutes = clockToMinutes(clock.value);
    if (minutes === null) return;
    if (String(minutes) === String(clock.dataset.clockValue)) return;
    applySetting(clock.dataset.clockApply + ":" + minutes);
  }

  function onInput(e) {
    const t = e.target;
    if (t.id === "feature-search") {
      S.featureQuery = t.value;
      const main = document.getElementById("main");
      const y = window.scrollY;
      main.innerHTML = renderFeatures();
      window.scrollTo(0, y);
      const again = document.getElementById("feature-search");
      again.focus();
      again.setSelectionRange(again.value.length, again.value.length);
    } else if (t.id === "lock-search") {
      S.lockQuery = t.value;
      filterChips("#main .lockchips .chip", t.value);
    } else if (t.id === "list-search") {
      S.listQuery = t.value;
      const q = t.value.trim().toLowerCase();
      document.querySelectorAll("#main .list .row").forEach((row) => {
        const name = row.querySelector(".t");
        row.hidden = q.length > 0 && !(name && name.textContent.toLowerCase().includes(q));
      });
    } else if (t.id === "group-search") {
      S.groupQuery = t.value;
      const q = t.value.trim();
      document.querySelectorAll("#sheet [data-pick-group]").forEach((row) => {
        const name = row.querySelector(".t");
        row.hidden = q.length > 0 && !(name && name.textContent.includes(q));
      });
    }
  }

  function filterChips(sel, query) {
    const q = query.trim();
    document.querySelectorAll(sel).forEach((chip) => {
      chip.hidden = q.length > 0 && !chip.textContent.includes(q);
    });
  }

  start();
})();
