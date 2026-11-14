(() => {
  "use strict";

  const tg = window.Telegram && window.Telegram.WebApp;
  const app = document.getElementById("app");
  const initData = (tg && tg.initData) || "";

  const STATE_MESSAGES = {
    auth_failed: "این صفحه باید از داخل ربات باز شود.",
    auth_unavailable: "سرور موقتا در دسترس نیست؛ کمی بعد دوباره امتحان کنید.",
    chat_unknown: "ربات دیگر در این گروه نیست یا شناخته شده نیست.",
    not_admin: "شما ادمین این گروه نیستید.",
    set_denied: "دسترسی تنظیمات برای شما بسته است.",
    case_denied: "دسترسی پرونده ها برای شما بسته است.",
  };

  const STORE_KEY = "gm.chat";
  const THEME_KEY = "gm.theme";
  const WIDE = window.matchMedia("(min-width: 900px)");

  function stored(key) {
    try {
      return window.localStorage.getItem(key);
    } catch (e) {
      return null;
    }
  }
  function store(key, value) {
    try {
      window.localStorage.setItem(key, value);
    } catch (e) {
    }
  }

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

  function decode(html) {
    const t = document.createElement("textarea");
    t.innerHTML = String(html == null ? "" : html);
    return t.value;
  }

  function bdi(html) {
    return "<bdi>" + html + "</bdi>";
  }

  const ZWNJ = String.fromCharCode(0x200c);

  function fold(text) {
    return String(text || "").trim().toLowerCase().replace(/ي/g, "ی").replace(/ك/g, "ک").split(ZWNJ).join(" ");
  }

  function initial(title) {
    const clean = decode(title).replace(/^[\s«»"'\-_.·]+/, "");
    return clean ? Array.from(clean)[0] : "؟";
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

  function ago(seconds) {
    const t = Number(seconds) || 0;
    if (!t) return "";
    const d = Math.max(0, Math.floor(Date.now() / 1000) - t);
    if (d < 60) return "همین الان";
    if (d < 3600) return fa(Math.floor(d / 60)) + " دقیقه پیش";
    if (d < 86400) return fa(Math.floor(d / 3600)) + " ساعت پیش";
    if (d < 604800) return fa(Math.floor(d / 86400)) + " روز پیش";
    try {
      return new Date(t * 1000).toLocaleDateString("fa-IR");
    } catch (e) {
      return fa(Math.floor(d / 86400)) + " روز پیش";
    }
  }

  const ICONS = {
    home: '<path d="M3 11 12 3l9 8"/><path d="M5 10v10h14V10"/><path d="M10 20v-6h4v6"/>',
    grid: '<rect x="3" y="3" width="7" height="7" rx="2"/><rect x="14" y="3" width="7" height="7" rx="2"/><rect x="3" y="14" width="7" height="7" rx="2"/><rect x="14" y="14" width="7" height="7" rx="2"/>',
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
    chart: '<path d="M3 3v18h18"/><path d="M7 15v2M12 10v7M17 6v11"/>',
    inbox: '<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11Z"/>',
    bolt: '<path d="M13 2 3 14h7l-1 8 10-12h-7l1-8Z"/>',
    bell: '<path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.7 21a2 2 0 0 1-3.4 0"/>',
    alert:
      '<path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
    clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 3"/>',
    hourglass: '<path d="M6 2h12M6 22h12"/><path d="M7 2v4a5 5 0 0 0 10 0V2"/><path d="M7 22v-4a5 5 0 0 1 10 0v4"/>',
    sparkles: '<path d="M12 3 13.5 9 20 12l-6.5 3L12 21l-1.5-6L4 12l6.5-3Z"/>',
    trash:
      '<path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6"/>',
    send: '<path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/>',
    moon: '<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
    monitor: '<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4"/>',
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
    book: '<path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/><path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2Z"/>',
    scroll: '<path d="M8 21h12a2 2 0 0 0 2-2v-2H10v2a2 2 0 1 1-4 0V5a2 2 0 1 0-4 0v3h4"/><path d="M19 17V5a2 2 0 0 0-2-2H4"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    userPlus:
      '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M19 8v6M22 11h-6"/>',
    userCog: '<circle cx="9" cy="7" r="4"/><path d="M3 21v-2a4 4 0 0 1 4-4h4"/><circle cx="18" cy="17" r="2.5"/><path d="M18 13v1.5M18 19.5V21M21.5 17H20M16 17h-1.5"/>',
    bot: '<rect x="4" y="8" width="16" height="12" rx="3"/><path d="M12 4v4"/><circle cx="12" cy="3" r="1"/><path d="M9 14h.01M15 14h.01"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
    arrowL: '<path d="M19 12H5"/><path d="m12 19-7-7 7-7"/>',
    arrowR: '<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>',
    arrowUp: '<path d="M12 19V5"/><path d="m5 12 7-7 7 7"/>',
    arrowDown: '<path d="M12 5v14"/><path d="m19 12-7 7-7-7"/>',
    key: '<circle cx="8" cy="15" r="4"/><path d="m10.85 12.15 8.15-8.15"/><path d="m18 6 3 3"/><path d="m15 9 3 3"/>',
    wifiOff:
      '<path d="M1 1l22 22"/><path d="M16.7 16.7a5 5 0 0 0-9.4 0"/><path d="M5 12.6a9 9 0 0 1 3.2-2.3"/><path d="M12 20h.01"/><path d="M8.5 8.5A14 14 0 0 1 22 9"/>',
    refresh: '<path d="M21 12a9 9 0 1 1-2.6-6.4"/><path d="M21 3v6h-6"/>',
    terminal: '<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>',
    pause: '<rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/>',
    play: '<path d="M6 4l14 8-14 8V4Z"/>',
    copy: '<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/>',
    snail: '<path d="M2 18h13a5 5 0 1 0-5-5v5"/><path d="M10 13a2 2 0 1 0 2-2"/><path d="M18 9V4M21 9V5"/>',
    gavel: '<path d="m14 13-7.5 7.5a2.12 2.12 0 0 1-3-3L11 10"/><path d="m16 16 6-6"/><path d="m8 8 6-6"/><path d="m9 7 8 8"/><path d="m21 11-8-8"/>',
    list: '<path d="M8 6h13M8 12h13M8 18h13"/><path d="M3 6h.01M3 12h.01M3 18h.01"/>',
  };

  function icon(name, size, weight) {
    size = size || 18;
    const keys = {
      lock: "LOCKED", unlock: "UNLOCKED", ban: "MODERATION_HAMMER", mute: "MUTED",
      clock: "TIMER", send: "TELEGRAM_SEND", chat: "CHAT", mic: "VOICE",
      image: "IMAGE", fileText: "DOCUMENT_ACTIVITY", alert: "WARNING", pause: "PAUSE",
    };
    const key = ICONS[name] ? null : keys[name] || name;
    const glyph = key && window.MODERATION_ICONS && window.MODERATION_ICONS[key];
    if (glyph) {
      return '<span class="semantic-icon" aria-hidden="true" data-icon="' + esc(key) +
        '" style="font-size:' + size + 'px">' + esc(glyph) + '</span>';
    }
    return (
      '<svg width="' + size + '" height="' + size +
      '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="' + (weight || 2) +
      '" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + (ICONS[name] || ICONS.info) + "</svg>"
    );
  }

  const CHEV = '<span class="chev">' + icon("chevL", 18) + "</span>";

  function swb(attr, on, label) {
    return '<button class="swb" ' + attr + ' role="switch" aria-checked="' + (on ? "true" : "false") + '"' +
      (label ? ' aria-label="' + esc(label) + '"' : "") + '><span class="sw' + (on ? " on" : "") + '"></span></button>';
  }

  const HEADER = { dark: "#080b13", light: "#f4f6fa" };

  function themePref() {
    const pref = stored(THEME_KEY);
    return pref === "dark" || pref === "light" ? pref : "auto";
  }

  function applyTheme() {
    const pref = themePref();
    let theme = pref;
    if (pref === "auto") {
      if (tg && tg.colorScheme && initData) theme = tg.colorScheme;
      else theme = window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
    }
    document.documentElement.dataset.theme = theme;
    const meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute("content", HEADER[theme]);
    if (tg) {
      try {
        tg.setHeaderColor(HEADER[theme]);
        tg.setBackgroundColor(HEADER[theme]);
        if (tg.setBottomBarColor) tg.setBottomBarColor(HEADER[theme]);
      } catch (e) {
      }
    }
  }

  const S = {
    groups: null,
    chat: null,
    dash: null,
    health: null,
    activity: null,
    rights: null,
    logInfo: null,
    welcomeInfo: null,
    slow: null,
    openCases: null,
    tab: "home",
    stack: [],
    sheet: null,
    confirm: null,
    palette: null,
    adv: new Set(),
    pending: null,
    busy: new Set(),
    lockCache: null,
    metric: "deleted",
    barSel: null,
    featCat: "all",
    lockView: "all",
    loadedAt: 0,
    refreshing: false,
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
      err.status = res.status;
      throw err;
    }
    if (res.status === 202) return Object.assign({ accepted: true }, data || {});
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
    if (e && e.status === 429) return toast((e && e.detail) || "کمی صبر کنید و دوباره امتحان کنید.", "warn");
    if (e && e.status === 403 && !e.detail) return toast("دسترسی این کار برای شما بسته است.", "warn");
    toast((e && e.detail) || STATE_MESSAGES[e && e.message] || "انجام نشد. دوباره امتحان کنید.");
  }

  function unsure(d) {
    if (!(d && d.accepted && d.error)) return false;
    toast(d.error, "warn");
    return true;
  }

  async function takeDash(d) {
    if (d && Array.isArray(d.sections)) {
      if (!d.chat || d.chat.id === S.chat) S.dash = d;
      return true;
    }
    try {
      await refreshDash();
    } catch (e) {
    }
    if (d && d.error) toast(d.error, "warn");
    else if (d && (d.accepted === "unknown" || d.delivery === "unknown")) toast("معلوم نیست ذخیره شد یا نه؛ وضعیت فعلی دوباره خوانده شد، پیش از تکرار نگاه کنید.", "warn");
    else if (d && d.accepted) toast("ثبت شد؛ کمی بعد در تلگرام اعمال می شود.", "warn");
    return !(d && (d.accepted || d.error));
  }

  let toastTimer = null;
  let toastAction = null;
  function toast(text, kind, action) {
    let el = document.getElementById("toast");
    if (!el) {
      el = document.createElement("div");
      el.id = "toast";
      el.setAttribute("role", "status");
      el.addEventListener("click", (e) => {
        if (e.target.closest(".ta") && toastAction) {
          const run = toastAction;
          toastAction = null;
          el.classList.remove("show");
          run();
        }
      });
      document.body.appendChild(el);
    }
    kind = kind === true ? "ok" : kind || "";
    const chat = S.chat;
    toastAction = action ? () => S.chat === chat && action.run() : null;
    const bar = document.getElementById("doc-bar");
    const place = S.sheet && !WIDE.matches ? " top" : bar && !bar.hidden ? " lift" : "";
    el.className = kind + place;
    el.setAttribute("role", kind === "ok" || kind === "warn" ? "status" : "alert");
    el.innerHTML = '<span class="ti">' + icon(kind === "ok" ? "check" : kind === "warn" ? "info" : "alert", 15, 2.4) + '</span><span class="tt">' + esc(text) + "</span>" +
      (action ? '<button class="ta">' + esc(action.label) + "</button>" : "");
    requestAnimationFrame(() => el.classList.add("show"));
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      el.classList.remove("show");
      toastAction = null;
    }, action ? 5000 : 3200);
  }

  function haptic(kind) {
    try {
      if (tg && tg.HapticFeedback) {
        if (kind === "ok") tg.HapticFeedback.notificationOccurred("success");
        else if (kind === "warn") tg.HapticFeedback.notificationOccurred("warning");
        else if (kind === "sel") tg.HapticFeedback.selectionChanged();
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
  function lockOn(key) {
    const d = S.lockCache;
    if (!d) return null;
    const l = d.plain.concat(d.ai || []).find((l) => l.key === key);
    return l ? !!l.on : null;
  }

  const FEATURES = [
    {
      id: "fl", group: "محافظت", icon: "bolt", title: "ضد رگبار",
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
      tip: "در گروه روی پیام متخلف ریپلای کنید و «اخطار» بفرستید. «حذف اخطار» و «اخطارها» هم هست.",
    },
    {
      id: "s", group: "محافظت", icon: "gavel", title: "حالت سختگیرانه",
      desc: "هر برخورد با قفل ها یک تخلف شمرده می شود و تکرارش محدودیت می آورد",
      flag: "strict", sections: ["s"], custom: "strict", load: "/strict-causes",
      effect: () => shown("s_lim") + " تخلف ← " + pickLabel("s_act") + " " + shown("s_time"),
    },
    {
      id: "wp", group: "محافظت", icon: "trash", title: "پاکسازی پیام متخلف",
      desc: "با بن یا سکوت، پیام های قبلی همان نفر هم پاک می شود",
      flags: ["wp_ban", "wp_mute"],
      warn: "پیام های پاک شده برنمی گردند، و این کار برای سکوت و بن های خودکار (ضد رگبار، سقف اخطار، حالت سختگیرانه) هم انجام می شود.",
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
      id: "tmed", group: "محتوا", icon: "hourglass", title: "رسانه موقت",
      desc: "رسانه ها بعد از مدتی خودکار پاک می شوند",
      flag: "tmed_on", sections: ["tmed"], custom: "tmed", load: "/tempmedia",
      effect: () => "حذف پس از " + shown("tmed_min") + " · " + pickLabel("tmed_who"),
      tip: "«همه» یعنی رسانه ادمین ها هم پاک می شود.",
    },
    {
      id: "filter", group: "محتوا", icon: "funnel", title: "فیلتر کلمات",
      desc: "پیامی که این کلمه ها را داشته باشد حذف می شود",
      page: "list:filter", neutral: true,
      effect: () => "افزودن و حذف کلمه",
    },
    {
      id: "cq", group: "هوشمند", icon: "sparkles", title: "نگهبان هوشمند",
      desc: "هر عکس و متن با هوش مصنوعی بررسی می شود، نه با کلمه",
      sections: ["cq"], custom: "ai", load: "/locks",
      state: () => S.dash.locks_summary.ai_active > 0,
      effect: () => fa(S.dash.locks_summary.ai_active) + " موضوع فعال" + (isOn("cq_shadow") && !(S.sheet && S.sheet.id === "cq") ? " · فقط بررسی" : ""),
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
      effect: () => "ویس ها بررسی و کلمه های نامناسب حذف می شود",
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
      desc: "آمار روز، هر روز در ساعتی که می گویید در گروه فرستاده می شود",
      enabled: "dr", off: "/report/off", onAction: () => "dr:" + setting("dr").value, sections: ["dr"], custom: "report",
      effect: () => "هر روز ساعت " + shown("dr"),
    },
    {
      id: "ng", group: "خودکار", icon: "moon", title: "قفل شب",
      desc: "در این بازه هیچ کس جز ادمین ها نمی تواند پیام بدهد",
      enabled: "ng", off: "/night/off", onAction: () => "ngf:" + setting("ngf").value, sections: ["ng"],
      effect: () => "از " + shown("ngf") + " تا " + shown("ngt"),
    },
    {
      id: "sl", group: "خودکار", icon: "snail", title: "اسلوموشن",
      desc: "هر عضو بین دو پیامش باید کمی صبر کند",
      custom: "slow", load: "/slow",
      state: () => !!(S.slow && S.slow.seconds > 0),
      effect: () => (S.slow ? slowLabel(S.slow.seconds) : "—"),
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

  const GROUPS = [
    ["محافظت", "shield", "جلوی اسپم، هجوم و خرابکاری را می گیرد"],
    ["محتوا", "lock", "چه چیزی در گروه فرستاده شود"],
    ["هوشمند", "sparkles", "بررسی تصویر، متن و ویس با هوش مصنوعی"],
    ["خودکار", "clock", "کارهایی که ربات خودش انجام می دهد"],
  ];

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

  function featureSwitch(f) {
    if (f.flag) return swb('data-apply="' + f.flag + '"', isOn(f.flag), f.title);
    if (f.enabled) return swb('data-power="' + f.id + '"', sectionEnabled(f.enabled), f.title);
    return "";
  }

  function featureRow(f) {
    const st = featureState(f);
    const sub = st === false ? "خاموش" : f.effect();
    const control = featureSwitch(f);
    return (
      '<div class="row tap" data-open="' + f.id + '">' +
      '<span class="rico' + (st ? " on" : "") + '">' + icon(f.icon, 17) + "</span>" +
      '<button class="rt" data-open="' + f.id + '"><span class="t">' + f.title + '</span><span class="s' + (st ? " on" : "") + '">' + esc(sub) + "</span></button>" +
      (control || CHEV) + "</div>"
    );
  }

  function slowLabel(seconds) {
    const step = S.slow && S.slow.steps ? S.slow.steps.find((s) => s.value === seconds) : null;
    if (step) return fa(step.label);
    return seconds ? fa(seconds) + " ثانیه" : "خاموش";
  }

  function checks() {
    const h = S.health && S.health !== "error" ? S.health : null;
    const rightsOk = h ? !h.issues.some((i) => i.fix === "rights") : null;
    const linksOn = lockOn("links");
    const logOk = S.logInfo && S.logInfo !== "error" ? !!S.logInfo.channel : null;
    const welcomeOk = S.welcomeInfo && S.welcomeInfo !== "error" ? !!(S.welcomeInfo.text || S.welcomeInfo.has_media) : null;
    return [
      { id: "rights", title: "همه دسترسی ها را به ربات بدهید", short: "دسترسی ربات", sub: "بدون آن هیچ قفل و فیلتری کار نمی کند", done: rightsOk, fix: () => openSheet("rights-help") },
      { id: "fl", title: "ضد رگبار را روشن کنید", short: "ضد رگبار", sub: "جلوی پیام های پشت سر هم را می گیرد", done: isOn("fl_on"), fix: () => openSheet("fl") },
      { id: "cp", title: "احراز هویت را روشن کنید", short: "احراز هویت", sub: "ربات های اسپم نمی توانند عضو بمانند", done: isOn("cp_on"), fix: () => openSheet("cp") },
      { id: "rd", title: "ضد هجوم را روشن کنید", short: "ضد هجوم", sub: "در برابر ورود دسته جمعی", done: isOn("rd_on"), fix: () => openSheet("rd") },
      { id: "links", title: "قفل لینک را روشن کنید", short: "قفل لینک", sub: "رایج ترین راه تبلیغ در گروه ها", done: linksOn, fix: () => pushPage("locks") },
      { id: "log", title: "کانال لاگ را وصل کنید", short: "کانال لاگ", sub: "سابقه همه برخوردها جایی ثبت شود", done: logOk, fix: () => pushPage("log") },
      { id: "wc", title: "پیام خوشامد بنویسید", short: "خوشامد", sub: "عضو تازه قوانین را می بیند", done: welcomeOk, fix: () => pushPage("welcome") },
    ];
  }

  function score() {
    const list = checks().filter((c) => c.done !== null);
    if (!list.length) return null;
    const done = list.filter((c) => c.done).length;
    return { pct: Math.round((done / list.length) * 100), done, total: list.length, list };
  }

  const SITE = window.GM_SITE || {};

  const NEEDED_RIGHTS = "delete_messages+restrict_members+invite_users+pin_messages+change_info+promote_members";

  function addToGroupLink() {
    return SITE.bot ? "https://t.me/" + SITE.bot + "?startgroup=new&admin=" + NEEDED_RIGHTS : "";
  }

  function openLink(url) {
    if (!url) return;
    try {
      if (tg && initData && /^https:\/\/t\.me\//.test(url) && tg.openTelegramLink) return tg.openTelegramLink(url);
      if (tg && initData && tg.openLink) return tg.openLink(url);
    } catch (e) {
    }
    window.open(url, "_blank", "noopener");
  }

  const SITE_FEATURES = [
    ["shield", "ضد اسپم و حمله", "ضد رگبار پیام های پشت سر هم را متوقف می کند، ضد هجوم ورود دسته جمعی را می خواباند و احراز هویت، ربات های اسپم را بیرون نگه می دارد.", ["ضد رگبار", "ضد هجوم", "احراز هویت", "لینک در بایو"]],
    ["lock", "قفل روی هر محتوا", "لینک، فوروارد، عکس، استیکر، منشن، ربات و بیش از سی مورد دیگر؛ به همراه فیلتر کلمه و قفل پک استیکر.", ["۳۷ قفل", "فیلتر کلمه", "پک استیکر"]],
    ["shieldOff", "ضد خیانت ادمین", "ادمینی که پشت سر هم پیام حذف کند خودکار برخورد می شود، و برای هر ادمین معلوم می کنید چه کارهایی مجاز است.", ["ضد خیانت", "محدودیت مدیران"]],
    ["hand", "خوشامد و قوانین", "پیام خوشامد با نام عضو تازه که چند ثانیه بعد خودش پاک می شود، و قوانینی که با یک کلمه نشان داده می شود.", ["خوشامد", "قوانین", "عضویت اجباری کانال"]],
    ["clock", "کارهای خودکار", "قفل شب، پاکسازی روزانه، اسلوموشن، رسانه موقت و پاسخ آماده به کلمه هایی که خودتان تعیین می کنید.", ["قفل شب", "پاکسازی", "پاسخ خودکار"]],
    ["inbox", "پرونده و گزارش", "هر برخورد ربات و هر گزارش عضو یک پرونده می شود؛ آمار هر روز و کانال لاگ، سابقه کامل را نگه می دارد.", ["پرونده ها", "کانال لاگ", "گزارش روزانه"]],
  ];

  const SITE_AI = ["محتوای غیراخلاقی", "تبلیغ داخل عکس", "خرید و فروش", "سیگار و قلیان", "مشروب", "سلاح", "قمار و شرط بندی", "مواد مخدر", "خون"];

  const SITE_STEPS = [
    ["ربات را به گروه اضافه کنید", "با دکمه «افزودن ربات به گروه» و انتخاب گروه."],
    ["ادمینش کنید", "دسترسی های لازم از پیش انتخاب شده اند (حذف پیام، مسدود کردن، افزودن کاربران، سنجاق، تغییر اطلاعات و افزودن ادمین)؛ فقط تایید کنید."],
    ["خودش فعال می شود", "با قفل لینک، فایل، سرویس تلگرام و ورود ربات و یک پیام خوشامد شروع می کند؛ بقیه را از داشبورد تنظیم کنید."],
  ];

  const SITE_FAQ = [
    ["ربات به چه دسترسی هایی نیاز دارد؟", "شش دسترسی: حذف پیام ها، مسدود کردن کاربران، افزودن کاربران، سنجاق کردن پیام ها، تغییر اطلاعات گروه و افزودن ادمین جدید. تا کامل نشده فعال نمی شود و دقیقا می گوید کدام مانده است."],
    ["گروه معمولی را هم مدیریت می کند؟", "ربات برای سوپرگروه ساخته شده است. وقتی در یک گروه معمولی ادمینش کنید، تلگرام خودش آن را سوپرگروه می کند و نصب ادامه پیدا می کند."],
    ["دستورها چطور است؟", "فارسی و بدون اسلش. روی پیام کسی ریپلای کنید و بنویسید «بن»، «سکوت ۱ ساعت» یا «اخطار». «راهنما» در گروه فهرست کامل را نشان می دهد."],
    ["هوش مصنوعی اشتباه حذف نمی کند؟", "تصمیم ها چند مرحله ای اند: برای موضوع هایی مثل سلاح و سیگار و برای فیلترهای نوشتاری، پیش از حذف یک مدل دوم دوباره به عکس نگاه می کند. حساسیت قابل تنظیم است، هر نگهبان حالت «فقط بررسی» دارد و فیلترهای تصویری تازه اول فقط بررسی می کنند تا مطمئن شوید."],
    ["کلینر چیست؟", "یک اکانت کمکی که کارهایی را انجام می دهد که تلگرام به ربات ها اجازه نمی دهد، مثل پاک کردن همه پیام های یک نفر و اسلوموشن. پس از کامل شدن دسترسی ها خودش اضافه می شود."],
    ["تنظیمات را از کجا عوض کنم؟", "در گروه «پنل» بفرستید یا داشبورد را باز کنید؛ هر تغییری فورا در گروه اعمال می شود."],
  ];

  const SITE_LINKS = [["CHANNEL", "کانال", "send"], ["SUPPORT", "پشتیبانی", "chat"], ["SOURCE", "کد منبع", "terminal"]];

  function unbroken(text) {
    return String(text).replace(/(^|\s)(ن?می) /g, "$1$2&nbsp;");
  }

  function siteDevice() {
    const r = 26;
    const c = 2 * Math.PI * r;
    const sw = (on) => '<span class="sw' + (on ? " on" : "") + '"></span>';
    const row = (ic, t, s, on) => '<div class="lp-s-row"><span class="lp-s-ico' + (on ? " on" : "") + '">' + icon(ic, 14) + '</span><span class="lp-s-rt"><b>' + t + "</b><small" + (on ? ' class="on"' : "") + ">" + s + "</small></span>" + sw(on) + "</div>";
    const tile = (ic, l, on) => '<span class="lp-s-tile' + (on ? " on" : "") + '"><i>' + icon(ic, 15) + "</i>" + l + "</span>";
    return (
      '<div class="lp-device" aria-hidden="true"><div class="lp-notch"></div><div class="lp-screen">' +
      '<div class="lp-s-top"><span class="lp-s-av">گ</span><span><b>گروه برنامه نویسان</b><small>مالک · ۳ گروه</small></span></div>' +
      '<div class="lp-s-hero"><div class="lp-s-ring"><svg width="62" height="62" viewBox="0 0 62 62"><circle cx="31" cy="31" r="' + r + '" fill="none" stroke-width="6" class="bgc"/><circle cx="31" cy="31" r="' + r + '" fill="none" stroke-width="6" stroke-linecap="round" class="fgc" stroke-dasharray="' + c.toFixed(1) + '" stroke-dashoffset="' + (c * 0.14).toFixed(1) + '"/></svg><b>۸۶</b></div>' +
      "<div><small>وضعیت گروه</small><b>گروه محافظت می شود</b><small>۹ امکان روشن · ۱۲ قفل</small></div></div>" +
      '<div class="lp-s-quick">' + tile("unlock", "بستن گروه") + tile("gavel", "سختگیرانه", true) + tile("key", "احراز هویت", true) + "</div>" +
      '<div class="lp-s-kpis"><div><small>حذف شده · امروز</small><b>۴۸</b><span class="lp-s-spark"><i style="height:40%"></i><i style="height:65%"></i><i style="height:30%"></i><i style="height:80%"></i><i style="height:55%"></i><i style="height:70%"></i><i style="height:100%"></i></span></div>' +
      '<div><small>عضو تازه · امروز</small><b>۱۲۳</b><span class="lp-s-spark"><i style="height:50%"></i><i style="height:35%"></i><i style="height:60%"></i><i style="height:45%"></i><i style="height:90%"></i><i style="height:75%"></i><i style="height:100%"></i></span></div></div>' +
      '<div class="lp-s-list">' + row("bolt", "ضد رگبار", "بیش از ۵ پیام در ۱۰ ثانیه ← سکوت", true) + row("sparkles", "نگهبان هوشمند", "۴ موضوع فعال", true) + row("shield", "ضد هجوم", "خاموش", false) + "</div>" +
      '<div class="lp-s-tabs">' + [["home", 1], ["grid"], ["users"], ["chart"], ["gear"]].map(([ic, on]) => '<span class="' + (on ? "on" : "") + '">' + icon(ic, 15) + "</span>").join("") + "</div>" +
      "</div></div>"
    );
  }

  function renderSite() {
    const add = addToGroupLink();
    const appLink = SITE.app || "";
    const links = SITE_LINKS.filter(([key]) => SITE.links && SITE.links[key]);
    const addBtn = (cls, label) => add ? '<a class="lp-btn ' + (cls || "") + '" href="' + esc(add) + '" target="_blank" rel="noopener">' + icon("plus", 18, 2.4) + "<span>" + (label || "افزودن ربات به گروه") + "</span></a>" : "";
    const appBtn = (cls) => appLink ? '<a class="lp-btn ghost ' + (cls || "") + '" href="' + esc(appLink) + '" target="_blank" rel="noopener">' + icon("grid", 17) + "<span>باز کردن داشبورد</span></a>" : "";
    const pref = themePref();
    const nextTheme = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    const head = (kicker, title, sub) => '<div class="lp-head"><span class="lp-kicker">' + kicker + "</span><h2>" + unbroken(title) + "</h2>" + (sub ? "<p>" + sub + "</p>" : "") + "</div>";

    let html = '<div class="lp">';
    html += '<header class="lp-nav"><div class="lp-wrap lp-nav-in">' +
      '<a class="lp-brand" href="#top"><span class="lp-logo">' + icon("shieldCheck", 18, 2.2) + "</span>مدیریت گروه</a>" +
      '<nav class="lp-links"><a href="#features">امکانات</a><a href="#ai">هوش مصنوعی</a><a href="#dashboard">داشبورد</a><a href="#start">راه اندازی</a><a href="#faq">سوال ها</a></nav>' +
      '<div class="lp-nav-acts"><button class="lp-ibtn" data-site-theme="' + nextTheme + '" aria-label="تغییر پوسته" title="' + (pref === "auto" ? "پوسته خودکار" : "تغییر پوسته") + '">' + icon(nextTheme === "light" ? "sun" : "moon", 18) + "</button>" +
      (add ? '<a class="lp-btn sm" href="' + esc(add) + '" target="_blank" rel="noopener">' + icon("plus", 16, 2.4) + '<span>افزودن به گروه</span></a>' : "") + "</div></div></header>";

    html += '<section class="lp-hero" id="top"><div class="lp-glow"></div><div class="lp-wrap lp-hero-in"><div class="lp-hero-copy">' +
      '<span class="lp-pill">' + icon("sparkles", 14) + " ربات مدیریت گروه برای تلگرام</span>" +
      "<h1>گروه تلگرامت را <em>امن و مرتب</em> نگه دار</h1>" +
      "<p>ضد اسپم و تبلیغ، قفل روی هر نوع محتوا، نگهبان هوشمند تصویر و یک داشبورد کامل؛ همه داخل خود تلگرام و با دستورهای ساده فارسی.</p>" +
      '<div class="lp-ctas">' + addBtn("lg") + appBtn("lg") + "</div>" +
      '<ul class="lp-checks"><li>' + icon("check", 14, 3) + "فعال سازی خودکار</li><li>" + icon("check", 14, 3) + "دستورهای فارسی بدون اسلش</li><li>" + icon("check", 14, 3) + "داشبورد داخل تلگرام</li></ul>" +
      '</div><div class="lp-hero-art">' + siteDevice() +
      '<div class="lp-float a"><span class="lp-fi bad">' + icon("trash", 15) + "</span><span><b>لینک تبلیغاتی حذف شد</b><small>قفل لینک · همین الان</small></span></div>" +
      '<div class="lp-float b"><span class="lp-fi ok">' + icon("key", 15) + "</span><span><b>ربات اسپم بیرون ماند</b><small>احراز هویت · ۲ دقیقه پیش</small></span></div>" +
      "</div></div></section>";

    html += '<section class="lp-facts"><div class="lp-wrap lp-facts-in">' +
      [["۳۷", "نوع قفل محتوا"], ["۹", "نگهبان هوشمند تصویر"], ["۲۰", "امکان محافظت و خودکار"], ["۳", "قدم تا راه اندازی"]].map(([n, l]) => "<div><b>" + n + "</b><span>" + l + "</span></div>").join("") +
      "</div></section>";

    html += '<section class="lp-sec" id="features"><div class="lp-wrap">' +
      head("امکانات", "هرچه برای اداره یک گروه لازم است", "از جلوی اسپم را گرفتن تا گزارش آخر شب؛ هر امکان با یک لمس روشن و خاموش می شود.") +
      '<div class="lp-grid">' + SITE_FEATURES.map(([ic, t, s, tags]) =>
        '<article class="lp-card"><span class="lp-ci">' + icon(ic, 20) + "</span><h3>" + t + "</h3><p>" + s + '</p><div class="lp-tags">' + tags.map((x) => "<span>" + x + "</span>").join("") + "</div></article>").join("") +
      "</div></div></section>";

    html += '<section class="lp-sec lp-ai" id="ai"><div class="lp-wrap lp-ai-in"><div>' +
      head("هوش مصنوعی", "نگهبانی که تصویر را می بیند، نه فقط کلمه را", "عکس، استیکر، گیف و پیش نمایش لینک با هوش مصنوعی بررسی می شود؛ تصمیم ها چند مرحله ای اند و هر نگهبان حالت «فقط بررسی» هم دارد.") +
      '<div class="lp-topics">' + SITE_AI.map((t) => "<span>" + icon("check", 13, 3) + t + "</span>").join("") + "</div></div>" +
      '<div class="lp-ai-cards">' +
      [["image", "فیلتر تصویری دلخواه", "یک موضوع بنویسید، مثل «خودرو لوکس»؛ هر عکس نزدیک به آن حذف می شود."],
        ["users", "قفل چهره", "روی عکس یک نفر «قفل چهره» بزنید تا تصویرهای او در گروه حذف شود."],
        ["mic", "نظارت بر ویس", "ویس ها شنیده می شود و اگر کلمه نامناسبی داشته باشد حذف می شود."],
        ["info", "اول فقط بررسی", "فیلتر تصویری تازه اول فقط بررسی می کند و چیزی حذف نمی کند؛ هر وقت مطمئن شدید روشنش کنید."]].map(([ic, t, s]) =>
        '<div class="lp-mini"><span class="lp-ci sm">' + icon(ic, 17) + "</span><div><h4>" + t + "</h4><p>" + s + "</p></div></div>").join("") +
      "</div></div></section>";

    html += '<section class="lp-sec" id="dashboard"><div class="lp-wrap lp-split"><div class="lp-split-art">' + siteChat() + "</div><div>" +
      head("داشبورد و دستورها", "همه چیز داخل خود تلگرام", "بدون رفتن به هیچ سایتی: داشبورد با دکمه «پنل» در گروه باز می شود، و هر کاری هم با یک کلمه فارسی در خود گروه انجام می شود.") +
      '<ul class="lp-list">' + [
        ["shieldCheck", "امتیاز امنیت گروه و فهرست کارهای مانده"],
        ["refresh", "روشن و خاموش با یک لمس، و برگرداندن هر تغییر"],
        ["chart", "آمار هفت روز: حذف، سکوت، بن و عضو تازه"],
        ["inbox", "پرونده ها: رسیدگی، یادداشت و لغو برخورد"],
        ["moon", "پوسته تیره و روشن، هماهنگ با تلگرام"],
      ].map(([ic, t]) => "<li><span>" + icon(ic, 16) + "</span>" + t + "</li>").join("") + "</ul>" +
      '<div class="lp-cmds">' + ["بن", "سکوت ۱ ساعت", "اخطار", "قفل لینک", "قفل گروه ۲ ساعت", "حذف ۵۰", "آمار", "پنل"].map((c) => "<code>" + c + "</code>").join("") + "</div>" +
      "</div></div></section>";

    html += '<section class="lp-sec" id="start"><div class="lp-wrap">' + head("راه اندازی", "سه قدم تا یک گروه امن", "") +
      '<ol class="lp-steps">' + SITE_STEPS.map(([t, s], i) => '<li><span class="lp-n">' + fa(i + 1) + "</span><h3>" + t + "</h3><p>" + s + "</p></li>").join("") + "</ol>" +
      (add ? '<div class="lp-center">' + addBtn("lg") + "</div>" : "") + "</div></section>";

    html += '<section class="lp-sec" id="faq"><div class="lp-wrap lp-narrow">' + head("سوال ها", "پرسش های رایج", "") +
      '<div class="lp-faq">' + SITE_FAQ.map(([q, a], i) => "<details" + (i === 0 ? " open" : "") + "><summary>" + q + '<span class="lp-plus">' + icon("plus", 18) + "</span></summary><p>" + a + "</p></details>").join("") + "</div></div></section>";

    html += '<section class="lp-final"><div class="lp-wrap"><div class="lp-final-in"><span class="lp-logo lg">' + icon("shieldCheck", 26, 2) + "</span>" +
      "<h2>همین حالا گروهت را امن کن</h2><p>اضافه کنید، ادمین کنید، تمام. بقیه با ربات.</p>" +
      '<div class="lp-ctas center">' + addBtn("lg light") + appBtn("lg light-ghost") + "</div></div></div></section>";

    html += '<footer class="lp-foot"><div class="lp-wrap lp-foot-in"><a class="lp-brand" href="#top"><span class="lp-logo">' + icon("shieldCheck", 16, 2.2) + "</span>مدیریت گروه</a>" +
      (links.length ? '<div class="lp-foot-links">' + links.map(([key, label, ic]) => '<a href="' + esc(SITE.links[key]) + '" target="_blank" rel="noopener">' + icon(ic, 15) + label + "</a>").join("") + "</div>" : "") +
      "<small>ساخته شده برای گروه های فارسی زبان تلگرام</small></div></footer>";
    html += "</div>";

    const first = !document.body.classList.contains("site");
    document.body.classList.add("site");
    document.body.classList.remove("nobar");
    app.innerHTML = html;
    if (first && location.hash.length > 1) {
      const target = document.getElementById(location.hash.slice(1));
      if (target) target.scrollIntoView();
    }
  }

  function siteChat() {
    const msg = (who, text, cls, extra) => '<div class="lp-msg ' + cls + '">' + (who ? "<b>" + who + "</b>" : "") + "<span>" + text + "</span>" + (extra || "") + "</div>";
    return (
      '<div class="lp-chat" aria-hidden="true"><div class="lp-chat-top"><span class="lp-s-av">گ</span><span><b>گروه برنامه نویسان</b><small>۱٬۲۴۸ عضو</small></span></div>' +
      '<div class="lp-chat-body">' +
      msg("مهمان ۴۲", "تخفیف ویژه، همین الان عضو شوید <bdi>t.me/offer</bdi>", "in struck", "<em>حذف شد · قفل لینک</em>") +
      msg("سارا · ادمین", '<span class="lp-reply">مهمان ۴۲</span>سکوت ۱ ساعت', "out") +
      msg("ربات", "✓ مهمان ۴۲ برای ۱ ساعت سکوت شد.", "in bot") +
      msg("سارا · ادمین", "قفل لینک", "out") +
      msg("ربات", "✓ قفل لینک فعال شد.", "in bot") +
      '</div><div class="lp-panelbtn">' + icon("grid", 15) + " باز کردن پنل</div></div>"
    );
  }

  function renderFatal(glyph, title, sub, retry) {
    app.innerHTML =
      '<aside id="rail"></aside><div id="view"><div id="main" class="nobar"><div class="empty fatal"><div class="glyph">' + icon(glyph, 32, 1.6) + "</div>" +
      '<div class="t">' + esc(title) + "</div>" + (sub ? '<div class="s">' + esc(sub) + "</div>" : "") +
      (retry ? '<button class="btn ghost" data-reboot>' + icon("refresh", 16) + " تلاش دوباره</button>" : "") +
      "</div></div></div>";
    document.body.classList.add("nobar");
  }

  async function start() {
    applyTheme();
    if (!initData) {
      renderSite();
      return;
    }
    if (tg && !start.wired) {
      start.wired = true;
      tg.ready();
      tg.expand();
      try {
        if (tg.disableVerticalSwipes) tg.disableVerticalSwipes();
        if (tg.onEvent) tg.onEvent("themeChanged", applyTheme);
        if (tg.SettingsButton) {
          tg.SettingsButton.show();
          tg.SettingsButton.onClick(() => S.dash && leaveTo(() => goTab("settings")));
        }
      } catch (e) {
      }
      if (tg.BackButton) tg.BackButton.onClick(back);
    }
    app.innerHTML = renderSkeleton();
    let groups;
    try {
      groups = (await api("/groups")).groups;
    } catch (e) {
      const auth = e.message === "auth_failed";
      renderFatal(auth ? "shieldOff" : "wifiOff", e.detail || STATE_MESSAGES[e.message] || "گروه ها خوانده نشد.",
        auth ? "نشست شما معتبر نیست یا منقضی شده؛ مینی اپ را ببندید و دوباره از ربات باز کنید." : "اتصال را بررسی کنید و دوباره امتحان کنید.", !auth);
      return;
    }
    S.groups = groups;

    const launched = tg && tg.initDataUnsafe && tg.initDataUnsafe.start_param;
    let chat = Number(launched) || null;
    if (!chat) {
      const last = Number(stored(STORE_KEY)) || null;
      if (last && groups.some((g) => g.id === last && g.known)) chat = last;
    }
    if (!chat) {
      const known = groups.filter((g) => g.known);
      if (known.length === 1) chat = known[0].id;
    }
    if (!chat) {
      if (!groups.some((g) => g.known)) {
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
    S.rights = null;
    S.logInfo = null;
    S.welcomeInfo = null;
    S.slow = null;
    S.openCases = null;
    S.stack = [];
    S.sheet = null;
    S.confirm = null;
    S.palette = null;
    S.admins = null;
    S.lists = null;
    S.joinGate = null;
    S.lockBusy = null;
    S.rightsBusy = false;
    S.caseBusy = null;
    S.lockCache = null;
    S.pending = null;
    S.featureQuery = "";
    S.barSel = null;
    S.tab = "home";
    setClosingConfirmation(false);
    store(STORE_KEY, String(chat));
    app.innerHTML = renderSkeleton();
    try {
      S.dash = await api("/dashboard");
    } catch (e) {
      renderChatError(e);
      return;
    }
    if (S.dash.state || !Array.isArray(S.dash.sections)) {
      const state = S.dash.state;
      S.dash = null;
      renderChatError(new Error(state || "request_failed"));
      return;
    }
    S.loadedAt = Date.now();
    if (S.groups && !S.groups.some((g) => g.id === chat)) {
      S.groups.unshift({ id: chat, title: S.dash.chat.title, is_owner: S.dash.viewer.is_owner, known: true });
    }
    render();
    loadHealth();
    loadActivity();
    loadRights();
    loadSide();
  }

  function rerenderIfIdle(tabs) {
    if (S.stack.length || S.palette) return;
    if (tabs && !tabs.includes(S.tab)) return;
    if (S.sheet) return drawSheet();
    render();
  }

  async function loadHealth() {
    const chat = S.chat;
    try {
      const h = await api("/health");
      if (S.chat !== chat) return;
      S.health = h;
    } catch (e) {
      if (S.chat !== chat) return;
      if (S.health == null) S.health = "error";
    }
    rerenderIfIdle();
  }

  async function loadActivity() {
    const chat = S.chat;
    try {
      const a = await api("/activity");
      if (S.chat !== chat) return;
      S.activity = a;
    } catch (e) {
      if (S.chat !== chat) return;
      if (S.activity == null) S.activity = "error";
    }
    rerenderIfIdle(["home", "reports"]);
  }

  async function loadRights() {
    const chat = S.chat;
    try {
      const r = await api("/rights");
      if (S.chat !== chat) return;
      S.rights = r;
    } catch (e) {
      if (S.chat !== chat) return;
      if (e && e.message === "unseeded") S.rights = "unseeded";
      else if (S.rights == null) S.rights = "error";
      if (S.rights === "unseeded") S.rightsHelp = e.detail || "";
    }
    rerenderIfIdle(["home", "members"]);
  }

  async function loadSide() {
    const chat = S.chat;
    const mine = (set) => (d) => { if (S.chat === chat) set(d); };
    const jobs = [
      api("/locks").then(mine((d) => { S.lockCache = d; })),
      api("/log").then(mine((d) => { S.logInfo = d; }), mine(() => { if (S.logInfo == null) S.logInfo = "error"; })),
      api("/welcome").then(mine((d) => { S.welcomeInfo = d; }), mine(() => { if (S.welcomeInfo == null) S.welcomeInfo = "error"; })),
      api("/slow").then(mine((d) => { S.slow = d; })),
      api("/cases?status=open&limit=5").then(mine((d) => { S.openCases = d; }), mine((e) => { if (S.openCases == null || S.openCases === "error") S.openCases = e && e.status === 403 ? "denied" : "error"; })),
    ];
    await Promise.allSettled(jobs);
    if (S.chat !== chat) return;
    rerenderIfIdle();
  }

  async function refreshDash() {
    const chat = S.chat;
    const d = await api("/dashboard");
    if (S.chat !== chat) return;
    if (d && Array.isArray(d.sections)) S.dash = d;
  }

  async function refreshAll(quiet) {
    if (!S.dash || S.refreshing) return;
    S.refreshing = true;
    if (!quiet) render();
    const page = currentPage();
    const jobs = [refreshDash(), loadHealth(), loadActivity(), loadRights(), loadSide()];
    if (S.admins) jobs.push(loadAdmins());
    if (page) jobs.push(loadPage(page));
    const results = await Promise.allSettled(jobs);
    S.refreshing = false;
    S.loadedAt = Date.now();
    render();
    if (!quiet) {
      if (results[0].status === "rejected") toast("به روز نشد؛ اتصال را بررسی کنید.");
      else {
        haptic("sel");
        toast("به روز شد.", "ok");
      }
    }
  }

  function renderChatError(e) {
    const known = Object.prototype.hasOwnProperty.call(STATE_MESSAGES, e.message) && e.message !== "auth_unavailable";
    const msg = STATE_MESSAGES[e.message] || "اطلاعات این گروه خوانده نشد.";
    const sub = e.message === "not_admin" ? "اگر تازه ادمین شده اید، چند دقیقه بعد دوباره امتحان کنید."
      : e.message === "chat_unknown" ? "ربات را دوباره به گروه اضافه و ادمین کنید."
      : known ? "" : "اتصال را بررسی کنید و دوباره امتحان کنید.";
    app.innerHTML =
      '<aside id="rail"></aside><div id="view">' + renderTop(true) +
      '<div id="main" class="nobar"><div class="empty"><div class="glyph">' + icon(known ? "shieldOff" : "wifiOff", 32, 1.6) + "</div>" +
      '<div class="t">' + esc(msg) + "</div>" + (sub ? '<div class="s">' + esc(sub) + "</div>" : "") +
      '<div class="btns" style="justify-content:center">' +
      (!known || e.message === "not_admin" ? '<button class="btn ghost" data-retry-chat>' + icon("refresh", 16) + " تلاش دوباره</button>" : "") +
      (S.groups && S.groups.length > 1 ? '<button class="btn" data-picker>انتخاب گروه دیگر</button>' : "") +
      (e.message === "chat_unknown" && addToGroupLink() ? '<button class="btn" data-open-link="' + esc(addToGroupLink()) + '">' + icon("plus", 16) + " افزودن دوباره ربات</button>" : "") +
      "</div></div></div></div>" + renderOverlays();
    document.body.classList.add("nobar");
    syncBack();
  }

  function renderNoGroups() {
    const add = addToGroupLink();
    app.innerHTML =
      '<aside id="rail"></aside><div id="view"><div class="top"><div class="grp"><span class="avatar brand">' +
      icon("shieldCheck", 18, 2.2) + '</span><span><span class="gname" style="display:block">مدیریت گروه</span><span class="gsub" style="display:block">هنوز گروهی وصل نیست</span></span></div></div>' +
      '<div id="main" class="nobar"><div class="onboard">' +
      '<div class="ob-hero"><span class="ob-mark">' + icon("shieldCheck", 30, 1.9) + "</span>" +
      "<h1>اولین گروهتان را وصل کنید</h1><p>ربات را به گروه اضافه و ادمین کنید؛ خودش فعال می شود و گروه همین جا ظاهر می شود.</p></div>" +
      '<ol class="ob-steps">' + SITE_STEPS.map(([t, s], i) => '<li><span class="ob-n">' + fa(i + 1) + '</span><span class="rt"><span class="t wrap">' + t + '</span><span class="s wrap">' + s + "</span></span></li>").join("") + "</ol>" +
      '<div class="ob-acts">' +
      (add ? '<button class="btn block lg" data-open-link="' + esc(add) + '">' + icon("plus", 18, 2.4) + " افزودن ربات به گروه</button>" : "") +
      '<button class="btn ghost block" data-reboot>' + icon("refresh", 16) + " گروه را اضافه کردم، بررسی کن</button></div>" +
      '<div class="ob-gets"><div class="h"><div class="t">بعد از وصل شدن</div></div><div class="list">' +
      [["shield", "ضد اسپم، ضد هجوم و احراز هویت", "با یک لمس روشن می شوند"],
        ["lock", "۳۷ قفل محتوا و فیلتر کلمه", "لینک، فوروارد، عکس، استیکر و بیشتر"],
        ["sparkles", "نگهبان هوشمند تصویر", "محتوای غیراخلاقی، تبلیغ، سلاح و…"],
        ["chart", "آمار و پرونده ها", "هر برخورد ثبت می شود"]].map(([ic, t, s]) =>
        '<div class="row"><span class="rico on">' + icon(ic, 17) + '</span><span class="rt"><span class="t">' + t + '</span><span class="s">' + s + "</span></span></div>").join("") +
      "</div></div></div></div></div>";
    document.body.classList.add("nobar");
  }

  function renderSkeleton() {
    const sk = (w, h, r) => '<div class="sk" style="width:' + w + ";height:" + h + "px;border-radius:" + (r || 8) + 'px"></div>';
    return (
      '<aside id="rail"></aside><div id="view"><div class="top"><div class="grp">' + sk("36px", 36, 11) +
      '<div style="display:flex;flex-direction:column;gap:6px">' + sk("140px", 14) + sk("90px", 10) + "</div></div>" + sk("40px", 40, 12) + "</div>" +
      '<div id="main"><div class="sec first">' + sk("100%", 150, 22) + "</div>" +
      '<div class="sec"><div class="quick">' + sk("100%", 78, 16).repeat(4) + "</div></div>" +
      '<div class="sec"><div class="kpis">' + sk("100%", 96, 16).repeat(4) + "</div></div>" +
      '<div class="sec">' + sk("100%", 220, 18) + "</div></div></div>"
    );
  }

  const TABS = [
    ["home", "home", "خانه"],
    ["features", "grid", "امکانات"],
    ["members", "users", "اعضا"],
    ["reports", "chart", "گزارش ها"],
    ["settings", "gear", "تنظیمات"],
  ];
  const TAB_TITLES = Object.fromEntries(TABS.map(([id, , label]) => [id, label]));

  function openCaseCount() {
    const c = S.openCases;
    if (!c || c === "denied" || c === "error" || !c.cases) return 0;
    return c.cases.length;
  }

  function caseBadge() {
    const n = openCaseCount();
    return n ? n + (S.openCases.has_more ? "+" : "") : 0;
  }

  function groupButton(extra) {
    const g = group();
    const title = g ? g.title : S.dash ? S.dash.chat.title : "انتخاب گروه";
    const role = S.dash ? (S.dash.viewer.is_owner ? "مالک" : "ادمین") : g && g.is_owner ? "مالک" : "ادمین";
    const many = S.groups && S.groups.length > 1;
    const chosen = !!(g || S.dash);
    const h = S.health && S.health !== "error" ? S.health : null;
    const dot = h ? '<span class="hdot ' + (h.issues.some((i) => i.severity === "bad") ? "bad" : h.issues.length ? "warn" : "ok") + '"></span>' : "";
    return (
      '<button class="grp' + (extra || "") + '" data-picker aria-label="' + esc("گروه: " + decode(title) + "، تغییر گروه") + '">' +
      (chosen
        ? '<span class="avatar" style="' + avatarStyle(S.chat) + '">' + esc(initial(title)) + dot + "</span>"
        : '<span class="avatar" style="background:var(--card-2);color:var(--muted)">' + icon("users", 18) + "</span>") +
      '<span style="min-width:0;flex:1"><span class="gname" style="display:block">' + bdi(title) + '</span><span class="gsub" style="display:block">' +
      (chosen ? role + (many ? " · " + fa(S.groups.length) + " گروه" : "") : fa((S.groups || []).length) + " گروه در دسترس") + "</span></span>" +
      '<span class="chev" style="color:var(--muted)">' + icon("chevD", 16) + "</span>" +
      "</button>"
    );
  }

  function topActions(bare) {
    if (bare) return "";
    return '<div class="acts">' +
      '<button class="ibtn" data-palette title="جستجو (Ctrl+K)" aria-label="جستجو">' + icon("search", 20) + "</button>" +
      '<button class="ibtn' + (S.refreshing ? " spin" : "") + '" data-refresh title="به روزرسانی" aria-label="به روزرسانی">' + icon("refresh", 19) + "</button>" +
      "</div>";
  }

  function renderTop(bare) {
    const wideTitle = S.tab === "home" ? '<div class="wide-only" style="min-width:0"><div class="ptitle">' + TAB_TITLES.home + '</div><div class="gsub">' + (S.dash ? S.dash.chat.title : "") + "</div></div>" : '<div class="wide-only"></div>';
    return '<div class="top">' + groupButton(bare ? "" : " hide-wide") + (bare ? "" : wideTitle) + topActions(bare) + "</div>";
  }

  function renderPageTop(title, sub, right) {
    return (
      '<div class="top"><div class="lead">' +
      '<button class="ibtn" data-back aria-label="بازگشت">' + icon("arrowR", 20) + "</button>" +
      '<div style="min-width:0"><div class="ptitle">' + title + '</div><div class="gsub">' + (sub ? bdi(sub) : "") + "</div></div></div>" +
      '<div class="acts">' + (right || "") + "</div></div>"
    );
  }

  function issueCount() {
    return S.health && S.health !== "error" ? S.health.issues.length : 0;
  }

  function tabBadge(id) {
    if (id === "home") return issueCount();
    if (id === "reports") return caseBadge();
    return 0;
  }

  function renderTabs() {
    return (
      '<nav id="tabs">' +
      TABS.map(([id, ic, label]) => {
        const badge = tabBadge(id);
        return '<button class="tab' + (S.tab === id ? " on" : "") + '" data-tab="' + id + '"' + (S.tab === id ? ' aria-current="page"' : "") + '><span class="ti">' + icon(ic, 21) + "</span><span>" + label + "</span>" +
          (badge ? '<span class="badge">' + fa(badge) + "</span>" : "") + "</button>";
      }).join("") +
      "</nav>"
    );
  }

  const RAIL_PAGES = [
    ["locks", "lock", "قفل ها"],
    ["rights", "chat", "دسترسی اعضا"],
    ["welcome", "hand", "خوشامد"],
    ["list:filter", "funnel", "فیلتر کلمات"],
    ["cases", "inbox", "پرونده ها"],
    ["log", "fileText", "کانال لاگ"],
  ];

  function renderRail() {
    if (!S.dash) return '<aside id="rail"></aside>';
    const root = S.stack[0];
    const nav = (attr, ic, label, on, badge) =>
      '<button class="nav' + (on ? " on" : "") + '" ' + attr + ">" + icon(ic, 18) + "<span>" + label + "</span>" + (badge ? '<span class="badge">' + fa(badge) + "</span>" : "") + "</button>";
    return (
      '<aside id="rail"><div class="brand"><span class="logo">' + icon("shieldCheck", 18) + "</span>مدیریت گروه</div>" + groupButton() +
      '<button class="searchbtn" data-palette>' + icon("search", 16) + "<span>جستجو</span><kbd>Ctrl K</kbd></button>" +
      TABS.map(([id, ic, label]) => nav('data-tab="' + id + '"', ic, label, !root && S.tab === id, tabBadge(id))).join("") +
      '<div class="rh">دسترسی سریع</div>' +
      RAIL_PAGES.map(([id, ic, label]) => nav('data-push-root="' + id + '"', ic, label, root && root.id === id, id === "cases" ? caseBadge() : 0)).join("") +
      '<div class="rfoot"><kbd>Ctrl K</kbd> جستجو · <kbd>Esc</kbd> بازگشت</div>' +
      "</aside>"
    );
  }

  function renderOverlays() {
    const sh = S.sheet;
    const open = sh && !sh.fresh;
    return '<div id="dim" data-close' + (open ? ' class="open"' : "") + '></div><div id="sheet" role="dialog" aria-modal="true" aria-labelledby="sheet-t" tabindex="-1"' + (open ? ' class="open"' : "") + (sh ? "" : " hidden") + ">" + (sh ? sheetHtml() : "") + "</div>" + renderPalette();
  }

  const FOCUS_SCOPES = [".confirm", "#palette", "#sheet", "#view", "#rail", "#tabs"];

  function focusTarget(el) {
    const scope = FOCUS_SCOPES.find((s) => el.closest(s));
    if (!scope) return null;
    const tag = el.tagName.toLowerCase();
    let sel = tag + (el.id ? "#" + CSS.escape(el.id) : "");
    for (const a of el.attributes) {
      if (a.name.startsWith("data-")) sel += "[" + a.name + '="' + CSS.escape(a.value) + '"]';
    }
    if (sel === tag) return null;
    const root = document.querySelector(scope);
    const index = root ? Array.from(root.querySelectorAll(sel)).indexOf(el) : 0;
    return { scope, sel, index: Math.max(0, index) };
  }

  function restoreFocus(t) {
    const root = t && app.querySelector(t.scope);
    if (!root) return;
    const all = root.querySelectorAll(t.sel);
    const el = all[Math.min(t.index, all.length - 1)];
    if (el) el.focus({ preventScroll: true });
  }

  function typedValues() {
    const out = {};
    app.querySelectorAll("#view input[id], #view textarea[id]").forEach((el) => {
      if (el.type === "search" || el.type === "time" || el.id === "doc-text") return;
      if (el.value !== el.defaultValue) out[el.id] = el.value;
    });
    return out;
  }

  function restoreTyped(values) {
    Object.keys(values).forEach((id) => {
      const el = document.getElementById(id);
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA")) el.value = values[id];
    });
  }

  function renderShellBare() {
    app.innerHTML = '<aside id="rail"></aside><div id="view">' + renderTop(true) + '<div id="main" class="nobar">' + pickPrompt() + "</div></div>" + renderOverlays();
    document.body.classList.add("nobar");
  }

  function pickPrompt() {
    const add = addToGroupLink();
    return '<div class="empty"><div class="glyph">' + icon("users", 32, 1.6) + '</div><div class="t">یک گروه انتخاب کنید</div><div class="s">' + fa((S.groups || []).length) + ' گروه در دسترس است.</div><div class="btns" style="justify-content:center"><button class="btn" data-picker>' + icon("users", 16) + " انتخاب گروه</button>" +
      (add ? '<button class="btn ghost" data-open-link="' + esc(add) + '">' + icon("plus", 16) + " افزودن به گروه دیگر</button>" : "") + "</div></div>";
  }

  function render() {
    if (!S.dash) return;
    const page = S.stack[S.stack.length - 1];
    const view = page ? renderPage(page) : renderTop(false) + '<div id="main">' + renderTab() + "</div>";
    const html = renderRail() + '<div id="view">' + view + "</div>" + (page ? "" : renderTabs()) + renderOverlays() + renderConfirm();
    const y = window.scrollY;
    const active = document.activeElement;
    const focused = active && active.id;
    const focusKey = active && active !== document.body && app.contains(active) ? focusTarget(active) : null;
    const sheetY = (document.getElementById("sheet") || {}).scrollTop || 0;
    const typed = render.page === page && render.tab === S.tab && !S.dropTyped ? typedValues() : null;
    S.dropTyped = false;
    app.innerHTML = html;
    if (typed) restoreTyped(typed);
    render.page = page;
    render.tab = S.tab;
    document.body.classList.toggle("nobar", !!page);
    window.scrollTo(0, y);
    const sheetEl = document.getElementById("sheet");
    if (sheetEl) sheetEl.scrollTop = sheetY;
    if (focused && (focused === "palette-input" || focused.endsWith("-search"))) {
      const el = document.getElementById(focused);
      if (el) {
        el.focus();
        el.setSelectionRange(el.value.length, el.value.length);
      }
    } else if (S.confirm && !(active && active.closest && active.closest(".confirm"))) {
      const cancel = app.querySelector(".confirm [data-confirm-cancel].btn");
      if (cancel) cancel.focus({ preventScroll: true });
    } else if (focusKey) {
      restoreFocus(focusKey);
    }
    if (S.sheet && S.sheet.fresh) openSheetAnimation();
    if (page && DOCS[page.id]) syncDoc(page);
    syncBack();
  }

  function renderTab() {
    switch (S.tab) {
      case "features":
        return renderFeatures();
      case "members":
        return renderMembers();
      case "reports":
        return renderReports();
      case "settings":
        return renderSettings();
      default:
        return renderHome();
    }
  }

  function syncBack() {
    if (!tg || !tg.BackButton) return;
    const show = !!(S.stack.length || S.sheet || S.confirm || S.palette || S.tab !== "home");
    if (show) tg.BackButton.show();
    else tg.BackButton.hide();
  }

  function back() {
    if (S.palette) return closePalette();
    if (S.confirm) {
      if (S.confirm.busy) return;
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
      if (page && DOCS[page.id] && docDirty(page)) {
        return leaveTo(() => popPage());
      }
      if (page && page.id === "cases" && page.data && page.data.detail) {
        delete page.data.detail;
        if (page.listStale) loadCases(page, page.data.status || "open", page.data.user || "");
        render();
        return;
      }
      popPage();
      return;
    }
    if (S.tab !== "home") goTab("home");
  }

  function popPage() {
    flushField();
    const page = S.stack.pop();
    setClosingConfirmation(false);
    if (page && page.fromSheet) return openSheet(page.fromSheet);
    render();
  }

  function counter(day, key) {
    const c = day.counters.find((c) => c.key === key);
    return c ? c.count : 0;
  }

  const METRICS = [
    { id: "deleted", label: "پیام حذف شده", keys: ["deleted"], icon: "trash" },
    { id: "moderated", label: "سکوت و بن", keys: ["muted", "banned"], icon: "gavel" },
    { id: "warned", label: "اخطار", keys: ["warned"], icon: "alert" },
    { id: "joined", label: "عضو تازه", keys: ["joined"], icon: "userPlus" },
    { id: "left", label: "خروج", keys: ["left"], icon: "door" },
  ];

  function metricOf(day, id) {
    const m = METRICS.find((m) => m.id === id) || METRICS[0];
    return m.keys.reduce((n, k) => n + counter(day, k), 0);
  }

  function activeFeatureCount() {
    return FEATURES.filter((f) => featureState(f) === true && !f.always).length;
  }

  function ring(pct) {
    const r = 30;
    const c = 2 * Math.PI * r;
    const off = c * (1 - Math.max(0, Math.min(100, pct)) / 100);
    return '<div class="ring"><svg width="72" height="72" viewBox="0 0 72 72"><circle class="bgc" cx="36" cy="36" r="' + r + '" fill="none" stroke-width="7"/>' +
      '<circle class="fgc" cx="36" cy="36" r="' + r + '" fill="none" stroke-width="7" stroke-linecap="round" stroke-dasharray="' + c.toFixed(1) + '" stroke-dashoffset="' + off.toFixed(1) + '"/></svg>' +
      '<div class="v">' + fa(pct) + "<small>امتیاز</small></div></div>";
  }

  function renderHero() {
    const h = S.health;
    const sc = score();
    const facts = fa(activeFeatureCount()) + " امکان روشن · " + fa(S.dash.locks_summary.active) + " قفل" + (S.dash.locks_summary.ai_active ? " · " + fa(S.dash.locks_summary.ai_active) + " نگهبان هوشمند" : "");
    let cls = "";
    let kicker = "وضعیت گروه";
    let title;
    let sub = facts;
    let acts = "";
    if (h == null) {
      cls = " neutral";
      title = "در حال بررسی…";
    } else if (h === "error") {
      cls = " neutral";
      title = "وضعیت ربات خوانده نشد";
      sub = "تنظیمات در دسترس است · " + facts;
      acts = '<button class="hbtn" data-recheck>' + icon("refresh", 14) + " تلاش دوباره</button>";
    } else if (h.issues.length) {
      const bad = h.issues.some((i) => i.severity === "bad");
      cls = bad ? " bad" : " warn";
      title = fa(h.issues.length) + " مورد نیاز به توجه دارد";
      sub = bad ? "بخشی از محافظت الان کار نمی کند؛ جزئیات و راه حل در زیر." : "یک تنظیم ناقص مانده است؛ جزئیات در زیر.";
      acts = '<button class="hbtn" data-recheck>' + icon("refresh", 14) + " بررسی دوباره</button>";
    } else {
      cls = " ok";
      title = sc && sc.pct >= 100 ? "گروه کاملا محافظت می شود" : "گروه محافظت می شود";
      if (sc && sc.pct < 100) sub = facts + " · " + fa(sc.total - sc.done) + " قدم تا محافظت کامل";
    }
    return (
      '<div class="hero' + cls + '"><div class="top-line">' + (sc ? ring(sc.pct) : "") +
      '<div style="flex:1;min-width:0"><div class="k">' + kicker + '</div><div class="t">' + title + '</div><div class="s">' + sub + "</div></div></div>" +
      (acts ? '<div class="acts">' + acts + "</div>" : "") + "</div>"
    );
  }

  function renderIssues() {
    const h = S.health;
    if (!h || h === "error" || !h.issues.length) return "";
    return '<div class="list issues">' + h.issues.map((i) =>
      '<div class="row"><span class="rico ' + (i.severity === "bad" ? "bad" : "warn") + '">' + icon("alert", 17) + '</span><span class="rt"><span class="t wrap">' + esc(i.title) + '</span><span class="s wrap">' + esc(i.detail) + "</span></span>" +
      '<button class="btn sm ghost" data-fix="' + esc(i.fix) + '">' + (i.fix === "rights" ? "راهنما" : "تنظیم") + "</button></div>"
    ).join("") + "</div>";
  }

  function renderSetup() {
    const sc = score();
    if (!sc || sc.pct >= 100) return "";
    const items = checks().filter((c) => c.done !== null);
    const done = items.filter((c) => c.done);
    const left = sc.total - sc.done;
    const fixing = new Set(S.health && S.health !== "error" ? S.health.issues.map((i) => i.fix) : []);
    return '<div class="sec" id="setup"><div class="card"><div class="setup-head"><div class="t"><span>راه اندازی گروه</span><span class="pill accent">' + fa(sc.done) + " از " + fa(sc.total) + '</span></div>' +
      '<div class="s">' + (left === 1 ? "یک قدم تا محافظت کامل مانده است." : fa(left) + " قدم تا محافظت کامل مانده است.") + '</div><div class="progress"><span style="width:' + sc.pct + '%"></span></div></div>' +
      items.map((c, i) => c.done || fixing.has(c.id) ? "" :
        '<button class="row" data-check="' + i + '"><span class="check"></span><span class="rt"><span class="t">' + c.title + '</span><span class="s">' + c.sub + "</span></span>" + CHEV + "</button>"
      ).join("") +
      (done.length ? '<div class="row done-sum"><span class="check done">' + icon("check", 14, 3) + '</span><span class="rt"><span class="t n">' + fa(done.length) + ' مورد انجام شده</span><span class="s">' + done.map((c) => c.short).join("، ") + "</span></span></div>" : "") +
      "</div></div>";
  }

  function groupState() {
    const r = S.rights;
    if (!r || r === "error" || r === "unseeded" || !Array.isArray(r.rights)) return null;
    const lock = r.lock || {};
    const until = lock.until && lock.until > Date.now() / 1000 ? lock.until : null;
    const allClosed = r.rights.length > 0 && r.rights.every((x) => !x.open);
    const manual = !!lock.manual;
    const night = !!lock.night;
    return { manual, until, night, allClosed, closed: manual || !!until || night || allClosed };
  }

  function groupClosed() {
    const g = groupState();
    return g ? g.closed : null;
  }

  function groupStateText(g) {
    if (!g) return S.rights === "unseeded" ? "هنوز تنظیم نشده" : S.rights === "error" ? "خوانده نشد" : "…";
    if (g.manual) return "بسته است";
    if (g.until) return "بسته تا " + fa(new Date(g.until * 1000).toLocaleTimeString("fa-IR", { hour: "2-digit", minute: "2-digit" }));
    if (g.night) return "قفل شب تا " + shown("ngt");
    if (g.allClosed) return "همه دسترسی ها بسته";
    return "باز است";
  }

  function renderQuick() {
    const g = groupState();
    const closed = g ? g.closed : null;
    const q = (attr, ic, label, state, cls) =>
      "<button " + attr + ' class="' + (cls || "") + '" aria-label="' + esc(label + "، " + state) + '"><span class="q">' + icon(ic, 20) + '</span><span class="l">' + label + '</span><span class="st">' + esc(state) + "</span></button>";
    const flag = (id) => (isOn(id) ? "روشن" : "خاموش");
    const night = sectionEnabled("ng");
    const purge = sectionEnabled("ap");
    const slow = S.slow && S.slow !== "error" ? S.slow.seconds : null;
    const lockText = S.lockBusy === "closing" ? "در حال بستن…" : S.lockBusy === "opening" ? "در حال باز کردن…" : groupStateText(g);
    return (
      '<div class="quick">' +
      q('data-quick="close"', closed ? "lock" : "unlock", "قفل گروه", lockText, (closed ? "bad" : "") + (S.lockBusy ? " busy" : "")) +
      q('data-apply="strict"', "gavel", "سختگیرانه", flag("strict"), isOn("strict") ? "on" : "") +
      q('data-apply="cp_on"', "key", "احراز هویت", flag("cp_on"), isOn("cp_on") ? "on" : "") +
      q('data-open="ng"', "moon", "قفل شب", night ? shown("ngf") + " تا " + shown("ngt") : "خاموش", night ? "on" : "") +
      q('data-open="sl"', "snail", "اسلوموشن", slow == null ? "…" : slow > 0 ? slowLabel(slow) : "خاموش", slow > 0 ? "on" : "") +
      q('data-open="ap"', "trash", "پاکسازی خودکار", purge ? shown("apt") : "خاموش", purge ? "on" : "") +
      "</div>"
    );
  }

  function renderKpis() {
    const a = S.activity && S.activity !== "error" ? S.activity : null;
    const ids = [["deleted", "حذف شده"], ["moderated", "سکوت و بن"], ["joined", "عضو تازه"], ["warned", "اخطار"]];
    return '<div class="kpis">' + ids.map(([id, label]) => {
      const m = METRICS.find((x) => x.id === id);
      if (!a) return '<div class="kpi"><div class="l">' + icon(m.icon, 14) + label + '</div><div class="v dim">—</div><div class="d">امروز</div></div>';
      const days = a.days.slice().sort((x, y) => y.ago - x.ago);
      const values = days.map((d) => metricOf(d, id));
      const max = Math.max(1, ...values);
      const now = values[values.length - 1];
      const prev = values.length > 1 ? values[values.length - 2] : null;
      const delta = prev != null ? "دیروز " + fa(prev) : "";
      return '<button class="kpi" data-kpi="' + id + '"><div class="l">' + icon(m.icon, 14) + label + " · امروز</div><div class=\"v\">" + fa(now) + '</div><div class="d">' + delta + "</div>" +
        '<div class="spark">' + values.map((v) => '<i style="height:' + Math.max(8, (v / max) * 100) + '%"></i>').join("") + "</div></button>";
    }).join("") + "</div>";
  }

  function renderHomeCases() {
    const c = S.openCases;
    if (!c || c === "denied" || c === "error" || !c.cases || !c.cases.length) return "";
    return '<div class="sec"><div class="h"><div class="t">پرونده های باز <span class="count">· ' + fa(caseBadge()) + '</span></div><button class="more" data-push="cases">همه ' + icon("chevL", 14) + "</button></div>" +
      '<div class="list">' + c.cases.slice(0, 3).map(caseRow).join("") + "</div></div>";
  }

  function renderHome() {
    const top = ["fl", "cq", "locks", "cp", "rd"].map((id) => featureRow(feature(id))).join("");
    return (
      '<div class="grid2 wide-left"><div class="col">' +
      '<div class="sec first">' + renderHero() + renderIssues() + "</div>" +
      renderSetup() +
      '<div class="sec"><div class="h"><div class="t">اقدام سریع</div></div>' + renderQuick() + "</div>" +
      '<div class="sec"><div class="h"><div class="t">امروز در گروه</div><button class="more" data-tab="reports">گزارش کامل ' + icon("chevL", 14) + "</button></div>" + renderKpis() + "</div>" +
      '</div><div class="col">' +
      renderHomeCases() +
      '<div class="sec"><div class="h"><div class="t">محافظت اصلی</div><button class="more" data-tab="features">همه امکانات ' + icon("chevL", 14) + "</button></div>" +
      '<div class="list">' + top + "</div></div>" +
      "</div></div>"
    );
  }

  function renderFeatures() {
    const cat = S.featCat;
    const total = FEATURES.filter((f) => featureState(f) !== null && !f.always).length;
    const html = '<div class="phead"><div class="t">امکانات</div><div class="s">' + fa(activeFeatureCount()) + " از " + fa(total) + " امکان روشن است · روی هر کدام بزنید تا تنظیمش کنید</div></div>" +
      '<div class="sec"><label class="search">' + icon("search", 16) +
      '<input id="feature-search" type="search" placeholder="جستجو، مثلا «لینک» یا «ویس»" aria-label="جستجوی امکانات و قفل ها" value="' + esc(S.featureQuery || "") + '" /></label>' +
      '<div class="chips scroll" style="margin-top:10px">' +
      [["all", "همه"]].concat(GROUPS.map(([g]) => [g, g])).concat([["on", "روشن ها"], ["off", "خاموش ها"]]).map(([id, label]) =>
        '<button class="chip sm' + (cat === id ? " on" : "") + '" data-feat-cat="' + esc(id) + '" aria-pressed="' + (cat === id) + '">' + label + "</button>").join("") +
      "</div></div>";
    return html + '<div id="feat-results">' + featureResults() + "</div>";
  }

  function featureResults() {
    const q = fold(S.featureQuery);
    const cat = S.featCat;
    let html = "";
    if (q && S.lockCache && (cat === "all" || cat === "محتوا" || cat === "on" || cat === "off")) {
      const locks = S.lockCache.plain.concat(S.lockCache.ai || []).filter((l) => fold(decode(l.label)).includes(q) && (cat !== "on" || l.on) && (cat !== "off" || !l.on));
      if (locks.length) {
        html += '<div class="sec"><div class="h"><div class="t with-ic"><span class="hic">' + icon("lock", 15) + '</span><span>قفل ها<span class="sub">روی هر کدام بزنید تا روشن یا خاموش شود</span></span></div><button class="more" data-push="locks">همه قفل ها ' + icon("chevL", 14) + "</button></div>" +
          '<div class="lockgrid">' + locks.slice(0, 9).map(lockTile).join("") + "</div></div>";
      }
    }
    let any = !!html;
    let body = "";
    GROUPS.forEach(([g, gic, gdesc]) => {
      if (cat !== "all" && cat !== "on" && cat !== "off" && cat !== g) return;
      const items = FEATURES.filter((f) => {
        if (f.group !== g) return false;
        if (q && !fold(f.title + " " + f.desc).includes(q)) return false;
        const st = featureState(f);
        if (cat === "on" && st !== true) return false;
        if (cat === "off" && st !== false) return false;
        return true;
      });
      if (!items.length) return;
      any = true;
      const all = FEATURES.filter((f) => f.group === g);
      const on = all.filter((f) => featureState(f) === true && !f.always).length;
      const switchable = all.filter((f) => featureState(f) !== null && !f.always).length;
      body +=
        '<div class="sec"><div class="h"><div class="t with-ic"><span class="hic">' + icon(gic, 15) + "</span><span>" + g + '<span class="sub">' + gdesc + '</span></span></div><div class="count">' +
        (switchable ? fa(on) + " از " + fa(switchable) + " روشن" : "") + "</div></div>" +
        '<div class="list">' + items.map(featureRow).join("") + "</div></div>";
    });
    html += '<div class="tiles">' + body + "</div>";
    if (!any) html += '<div class="empty sm"><div class="glyph">' + icon("search", 24) + '</div><div class="t">' + (q ? "چیزی با این نام نیست" : cat === "on" ? "هیچ امکانی روشن نیست" : "همه امکانات روشن است") + "</div>" + (q ? '<div class="s">جستجوی کامل تر، همه تنظیم ها و صفحه ها: <button class="more" data-palette>جستجوی سراسری</button></div>' : "") + "</div>";
    html += '<div class="sec"><button class="btn outline block" data-open="help">' + icon("book", 16) + " دستورهایی که از داخل گروه کار می کنند</button></div>";
    return html;
  }

  const LIST_KINDS = {
    ban: ["بن شده ها", "ban", "کسانی که از گروه بیرون شده اند"],
    mute: ["سکوت شده ها", "mute", "نمی توانند پیام بدهند"],
    vip: ["کاربران ویژه", "star", "از همه قفل ها معاف اند"],
    free: ["معاف ها", "unlock", "از عضویت اجباری کانال و اد اجباری معاف اند"],
    filter: ["فیلتر کلمات", "funnel", "پیامی که این کلمه ها را دارد حذف می شود"],
    answer: ["پاسخ خودکار", "chat", "به این کلمه ها پاسخ آماده داده می شود"],
    cmd: ["دستور های سفارشی", "terminal", "کلمه هایی که خودتان برای بن، سکوت یا اخراج ساخته اید"],
    pack: ["پک های استیکر", "sparkles", "استیکرهای این پک ها حذف می شوند"],
  };

  const LIST_HOWTO = {
    ban: "در گروه روی پیام کاربر ریپلای کنید و «بن» بفرستید.",
    mute: "در گروه روی پیام کاربر ریپلای کنید و «سکوت» یا «سکوت ۱ ساعت» بفرستید.",
    vip: "در گروه روی پیام کاربر ریپلای کنید و «ویژه» بفرستید.",
    free: "در گروه روی پیام کاربر ریپلای کنید و «معاف» بفرستید.",
    cmd: "در گروه بفرستید: «تنظیم دستور زنجیر بن»",
    pack: "در گروه روی یک استیکر ریپلای کنید و «قفل پک» بفرستید.",
  };

  function renderMembers() {
    const a = S.admins;
    let admins = '<div class="sec"><div class="h"><div class="t">ادمین ها' + (a && a !== "error" ? ' <span class="count">· ' + fa(a.admins.length) + "</span>" : "") + "</div>" +
      '<button class="more" data-open="lim">محدودیت ها ' + icon("chevL", 14) + "</button></div>";
    if (!a) {
      admins += '<div class="list">' + ('<div class="row"><div class="sk" style="width:36px;height:36px;border-radius:11px"></div><div class="rt"><div class="sk" style="width:120px;height:12px"></div></div></div>').repeat(3) + "</div>";
    } else if (a === "error") {
      admins += '<div class="list"><div class="row"><span class="rico warn">' + icon("wifiOff", 17) + '</span><span class="rt"><span class="t n">فهرست ادمین ها خوانده نشد.</span></span><button class="btn sm ghost" data-reload-admins>' + icon("refresh", 14) + " تلاش دوباره</button></div></div>";
    } else {
      admins += '<div class="list">' + a.admins.map((ad) => {
        const known = typeof ad.removable === "boolean";
        const removable = a.can_remove && !ad.is_creator && !ad.is_bot && (!known || ad.removable);
        const role = ad.is_creator ? "مالک گروه" : ad.is_bot ? "ربات" : !known ? "ادمین" : ad.removable ? "ادمین · ربات می تواند عزلش کند" : "ادمین · فقط از تلگرام عزل می شود";
        return (
          '<div class="row"><span class="avatar sm" style="' + avatarStyle(ad.id) + '">' + esc(initial(ad.name)) + "</span>" +
          '<span class="rt"><span class="t">' + bdi(ad.name) + '</span><span class="s">' + role + "</span></span>" +
          (ad.is_creator ? '<span class="pill ok">' + icon("star", 11, 2.6) + " مالک</span>" : "") +
          (removable ? '<button class="ibtn danger" data-remove-admin="' + esc(String(ad.id)) + '" title="عزل" aria-label="' + esc("عزل " + decode(ad.name)) + '">' + icon("x", 18) + "</button>" : "") +
          "</div>"
        );
      }).join("") + "</div>";
      admins += '<div class="hint start">' + (a.can_remove ? "برای ادمین کردن کسی، در گروه روی پیامش «ترفیع» بفرستید." : "فقط مالک گروه می تواند ادمین عزل کند.") + "</div>";
    }
    admins += "</div>";

    const g = groupState();
    const closed = g ? g.closed : null;
    const jg = S.joinGate && S.joinGate !== "error" ? S.joinGate : null;
    const adOn = featureState(feature("ad"));
    let rights = '<div class="sec"><div class="h"><div class="t">ورود و دسترسی اعضا</div></div><div class="list">' +
      '<button class="row" data-push="rights"><span class="rico' + (closed ? " bad" : closed === false ? " on" : "") + '">' + icon(closed ? "lock" : "chat", 17) + '</span><span class="rt"><span class="t">دسترسی اعضای عادی</span><span class="s' + (closed ? " bad" : "") + '">' +
      (closed ? "گروه " + groupStateText(g) : closed === false ? "گروه باز است · چه چیزی می توانند بفرستند" : groupStateText(g)) + "</span></span>" + CHEV + "</button>" +
      '<button class="row" data-open="ad"><span class="rico' + (adOn ? " on" : "") + '">' + icon("userPlus", 17) + '</span><span class="rt"><span class="t">اد اجباری</span><span class="s' + (adOn ? " on" : "") + '">' + (adOn ? esc(feature("ad").effect()) : "خاموش") + "</span></span>" + CHEV + "</button>" +
      '<button class="row" data-push="joingate"><span class="rico' + (jg && jg.channel ? " on" : "") + '">' + icon("door", 17) + '</span><span class="rt"><span class="t">عضویت اجباری در کانال</span><span class="s' + (jg && jg.channel ? " on" : "") + '">' +
      (jg ? (jg.channel ? 'روشن · <bdi dir="ltr">@' + esc(decode(jg.channel).replace(/^@/, "")) + "</bdi>" : "خاموش") : "تا عضو کانال نشوند پیام نمی دهند") + "</span></span>" + CHEV + "</button>" +
      "</div></div>";

    const lists = '<div class="sec"><div class="h"><div class="t">لیست ها</div></div><div class="list">' +
      ["ban", "mute", "vip", "free"].map((k) => listRow(k)).join("") + "</div></div>";
    return '<div class="phead"><div class="t">اعضا</div><div class="s">ادمین ها، لیست ها و آنچه اعضا مجازند بفرستند</div></div>' +
      '<div class="grid2"><div class="col">' + admins + rights + '</div><div class="col">' + lists + "</div></div>";
  }

  function listRow(kind) {
    const [title, ic, desc] = LIST_KINDS[kind];
    const cached = S.lists && S.lists[kind];
    const count = cached ? '<span class="count">' + fa(cached.entries.length) + (cached.truncated ? "+" : "") + "</span>" : "";
    return (
      '<button class="row" data-push="list:' + kind + '"><span class="rico">' + icon(ic, 17) + '</span><span class="rt"><span class="t">' + title +
      '</span><span class="s">' + desc + "</span></span>" + count + CHEV + "</button>"
    );
  }

  async function loadAdmins() {
    const chat = S.chat;
    if (!S.admins || S.admins === "error") S.admins = null;
    try {
      const a = await api("/admins");
      if (S.chat !== chat) return;
      S.admins = a;
    } catch (e) {
      if (S.chat !== chat) return;
      if (!S.admins) S.admins = "error";
    }
    rerenderIfIdle(["members"]);
  }

  const WEEKDAYS = ["یکشنبه", "دوشنبه", "سه شنبه", "چهارشنبه", "پنجشنبه", "جمعه", "شنبه"];

  function dayName(d) {
    if (d.ago === 0) return "امروز";
    if (d.ago === 1) return "دیروز";
    return WEEKDAYS[(Number(d.day) + 4) % 7];
  }

  function renderChart(a) {
    const metric = METRICS.find((m) => m.id === S.metric) || METRICS[0];
    const days = a.days.slice().sort((x, y) => y.ago - x.ago);
    const values = days.map((d) => metricOf(d, metric.id));
    const max = Math.max(1, ...values);
    const peak = values.indexOf(Math.max(...values));
    const sel = S.barSel == null ? 0 : S.barSel;
    const selDay = a.days.find((d) => d.ago === sel) || a.days[0];
    const total = values.reduce((n, v) => n + v, 0);
    const bars = days.map((d, i) => {
      const v = values[i];
      const label = d.ago === 0 || d.ago === sel || (i === peak && v > 0);
      return '<div class="bar' + (d.ago === 0 ? " today" : "") + (d.ago === sel ? " sel" : "") + '" data-bar="' + d.ago + '" title="' + esc(dayName(d) + ": " + fa(v)) + '">' +
        '<span class="val' + (label ? "" : " hide") + '">' + fa(v) + '</span><span class="fill" style="height:' + (v / max) * 100 + '%"></span></div>';
    }).join("");
    return (
      '<div class="chips scroll" style="margin-bottom:12px">' +
      METRICS.map((m) => '<button class="chip sm' + (m.id === metric.id ? " on" : "") + '" data-metric="' + m.id + '">' + m.label + "</button>").join("") +
      "</div>" +
      '<div class="chart"><div class="head"><div><div class="big">' + fa(metricOf(selDay, metric.id)) + '</div><div class="cap">' + metric.label + " · " + dayName(selDay) + "</div></div>" +
      '<div class="cap">جمع ۷ روز · <b style="color:var(--text)">' + fa(total) + "</b></div></div>" +
      '<div class="bars" role="img" aria-label="' + esc(metric.label + " در ۷ روز گذشته") + '"><span class="gl" style="bottom:50%"></span>' + bars + "</div>" +
      '<div class="blabels">' + days.map((d) => '<span class="' + (d.ago === 0 ? "today" : "") + '">' + dayName(d) + "</span>").join("") + "</div></div>"
    );
  }

  function renderReports() {
    const a = S.activity;
    let left = '<div class="phead"><div class="t">گزارش ها</div><div class="s">آمار هفته، پرونده ها و کانال لاگ</div></div>';
    if (!a) left += '<div class="sec"><div class="sk" style="height:240px;border-radius:18px"></div></div>';
    else if (a === "error") {
      left += '<div class="sec"><div class="card"><div class="empty sm"><div class="glyph">' + icon("wifiOff", 24) + '</div><div class="t">آمار خوانده نشد</div><div class="s" style="margin-bottom:14px">اتصال را بررسی کنید.</div><button class="btn sm ghost" data-reload-activity>' + icon("refresh", 14) + " تلاش دوباره</button></div></div></div>";
    } else {
      left += '<div class="sec">' + renderChart(a) + "</div>";
      const sel = S.barSel == null ? 0 : S.barSel;
      const day = a.days.find((d) => d.ago === sel) || a.days[0];
      const shownKeys = day.counters.filter((x) => x.count > 0);
      left += '<div class="sec"><div class="h"><div class="t">جزئیات ' + dayName(day) + "</div></div>";
      if (!shownKeys.length) {
        left += '<div class="note mute">' + icon("info", 16) + "<span>" + (sel === 0 ? "امروز هنوز رویدادی ثبت نشده. شمارش از اولین حذف، سکوت یا عضو تازه شروع می شود." : "در این روز رویدادی ثبت نشده.") + "</span></div>";
      } else {
        left += '<div class="list">' + shownKeys.map((x) =>
          '<div class="row compact"><span class="rt"><span class="t n">' + x.label + '</span></span><span class="count strong">' + fa(x.count) + "</span></div>"
        ).join("") + "</div>";
      }
      left += "</div>";
    }

    let right = "";
    const c = S.openCases;
    if (c !== "denied") {
      right += '<div class="sec"><div class="h"><div class="t">پرونده های باز' + (openCaseCount() ? ' <span class="count">· ' + fa(caseBadge()) + "</span>" : "") + '</div><button class="more" data-push="cases">همه پرونده ها ' + icon("chevL", 14) + "</button></div>";
      if (!c) right += '<div class="sk" style="height:110px;border-radius:18px"></div>';
      else if (c === "error") right += '<div class="list"><div class="row"><span class="rico warn">' + icon("wifiOff", 17) + '</span><span class="rt"><span class="t n">پرونده ها خوانده نشد.</span></span><button class="btn sm ghost" data-reload-cases>' + icon("refresh", 14) + " تلاش دوباره</button></div></div>";
      else if (!c.cases.length) right += '<div class="card"><div class="empty sm"><div class="glyph">' + icon("shieldCheck", 24) + '</div><div class="t">پرونده باز نیست</div><div class="s">همه گزارش ها رسیدگی شده است.</div></div></div>';
      else right += '<div class="list">' + c.cases.slice(0, 4).map(caseRow).join("") + "</div>";
      right += "</div>";
    }

    const log = S.logInfo && S.logInfo !== "error" ? S.logInfo : null;
    right += '<div class="sec"><div class="h"><div class="t">ثبت و گزارش</div></div><div class="list">' +
      '<button class="row" data-push="log"><span class="rico' + (log && log.channel ? " on" : log ? " warn" : "") + '">' + icon("fileText", 17) + '</span><span class="rt"><span class="t">کانال لاگ</span><span class="s' + (log && log.channel ? " on" : "") + '">' +
      (log ? (log.channel ? "وصل · " + fa(log.kinds.filter((k) => k.on).length) + " نوع رویداد ثبت می شود" : "وصل نیست؛ برخوردها جایی ثبت نمی شود") : S.logInfo === "error" ? "خوانده نشد" : "…") + "</span></span>" +
      (log && !log.channel ? '<span class="pill warn">وصل کنید</span>' : "") + CHEV + "</button>" +
      featureRow(feature("dr")) + "</div></div>";
    right += '<div class="hint">شمارش ها از ربات است و با آمار خود تلگرام فرق دارد. آمار ماه، ساعت ها و فعال ترین اعضا: «آمار» در گروه.</div>';
    return '<div class="grid2 wide-left"><div class="col">' + left + '</div><div class="col">' + right + "</div></div>";
  }

  function renderSettings() {
    const health = S.health && S.health !== "error" ? S.health : null;
    const flagRow = (id, ic, title, sub) => {
      const it = setting(id);
      if (!it) return "";
      return '<div class="row"><span class="rico' + (it.on ? " on" : "") + '">' + icon(ic, 17) + '</span><span class="rt"><span class="t">' + title + '</span><span class="s">' + sub + "</span></span>" + swb('data-apply="' + id + '"', it.on, title) + "</div>";
    };
    const sheetRow = (fid, ic, title, sub, on) =>
      '<button class="row" data-open="' + fid + '"><span class="rico' + (on ? " on" : "") + '">' + icon(ic, 17) + '</span><span class="rt"><span class="t">' + title + '</span><span class="s' + (on ? " on" : "") + '">' + sub + "</span></span>" + CHEV + "</button>";
    const pageRow = (pid, ic, title, sub, on) =>
      '<button class="row" data-push="' + pid + '"><span class="rico' + (on ? " on" : "") + '">' + icon(ic, 17) + '</span><span class="rt"><span class="t">' + title + '</span><span class="s' + (on ? " on" : "") + '">' + sub + "</span></span>" + CHEV + "</button>";
    const sec = (title, rows) => '<div class="sec"><div class="h"><div class="t">' + title + '</div></div><div class="list">' + rows + "</div></div>";
    const pref = themePref();

    let pill = "";
    if (health) {
      const bad = health.issues.some((i) => i.severity === "bad");
      pill = !health.bot_admin ? '<span class="pill bad">ربات ادمین نیست</span>'
        : bad ? '<span class="pill bad">' + icon("alert", 12, 2.6) + " دسترسی ناقص</span>"
        : health.issues.length ? '<span class="pill warn">' + fa(health.issues.length) + " مورد مانده</span>"
        : '<span class="pill ok">' + icon("check", 12, 3) + " ربات فعال</span>";
      if (health.issues.length || !health.bot_admin) pill = '<button class="pillbtn" data-tab="home" aria-label="دیدن مشکل ها در خانه">' + pill + "</button>";
    }
    let html = '<div class="phead"><div class="t">تنظیمات</div><div class="s">رفتار ربات در این گروه</div></div>';
    html += '<div class="sec"><div class="card"><div class="profile"><span class="avatar xl" style="' + avatarStyle(S.chat) + '">' + esc(initial(S.dash.chat.title)) + "</span>" +
      '<span class="rt"><span class="t">' + bdi(S.dash.chat.title) + '</span><span class="s">' + (S.dash.viewer.is_owner ? "شما مالک هستید" : "شما ادمین هستید") + "</span></span>" + pill + "</div>" +
      '<div class="row" style="border-top:1px solid var(--border)"><span class="rico">' + icon("key", 17) + '</span><span class="rt"><span class="t">شناسه گروه</span><span class="s"><bdi dir="ltr" class="num">' + S.chat + "</bdi></span></span>" +
      '<button class="ibtn" data-copy="' + S.chat + '" title="کپی" aria-label="کپی شناسه گروه">' + icon("copy", 18) + "</button></div>" +
      '<button class="row" data-picker><span class="rico">' + icon("users", 17) + '</span><span class="rt"><span class="t">' + (S.groups && S.groups.length > 1 ? "تغییر گروه" : "گروه های شما") + '</span><span class="s">' + (S.groups && S.groups.length > 1 ? fa(S.groups.length) + " گروه در دسترس" : "افزودن ربات به گروه دیگر") + "</span></span>" + CHEV + "</button>" +
      "</div></div>";

    const welcome = S.welcomeInfo && S.welcomeInfo !== "error" ? S.welcomeInfo : null;
    const welcomeSet = !!(welcome && (welcome.text || welcome.has_media));
    const gate = S.joinGate && S.joinGate !== "error" ? S.joinGate : null;
    let col1 = sec("ورود اعضا",
      featureRow(feature("cp")) +
      pageRow("welcome", "hand", "پیام خوشامد", welcome ? (welcomeSet ? "نوشته شده · حذف خودکار: " + shown("wct") : "نوشته نشده") : "متن و حذف خودکار", welcomeSet) +
      pageRow("rules", "scroll", "قوانین گروه", "با «قوانین» در گروه نمایش داده می شود") +
      pageRow("joingate", "door", "عضویت اجباری در کانال", gate ? (gate.channel ? "روشن" : "خاموش") : "تا عضو کانال نشوند پیام نمی دهند", !!(gate && gate.channel)) +
      flagRow("lb_on", "refresh", "بازگشت اعضای خارج شده", "کسی که بیرون رفت دوباره اضافه می شود"));
    col1 += sec("پیام های ربات",
      sheetRow("response-policy", "chat", "دیده شدن پیام ها", "خصوصی کردن اعلان ها و خوشامد") +
      featureRow(EXTRA_SHEETS.nt) +
      flagRow("rk_on", "star", "مقام خودکار", "با فعالیت، عنوان می گیرند") +
      pageRow("list:answer", "chat", "پاسخ خودکار", "کلمه و پاسخ آماده") +
      pageRow("list:cmd", "terminal", "دستور های سفارشی", "کلمه های خودتان برای بن و سکوت"));

    let col2 = sec("مدیران",
      (S.dash.viewer.is_owner ? featureRow(EXTRA_SHEETS.lim) : sheetRow("lim", "userCog", "محدودیت مدیران", (isOn("lim_on") ? "روشن" : "خاموش") + " · فقط مالک تغییر می دهد", isOn("lim_on"))) +
      featureRow(feature("bt")) +
      '<button class="row" data-tab="members"><span class="rico">' + icon("users", 17) + '</span><span class="rt"><span class="t">فهرست ادمین ها</span><span class="s">مشاهده و عزل</span></span>' + CHEV + "</button>");
    const log = S.logInfo && S.logInfo !== "error" ? S.logInfo : null;
    col2 += sec("ثبت و گزارش",
      pageRow("log", "fileText", "کانال لاگ", log ? (log.channel ? "وصل" : "وصل نیست") : S.logInfo === "error" ? "خوانده نشد" : "…", !!(log && log.channel)) +
      featureRow(feature("dr")));
    col2 += sec("ظاهر",
      '<div class="row" style="display:block;padding:12px"><div class="seg" role="radiogroup" aria-label="پوسته">' +
      [["auto", "monitor", "خودکار"], ["dark", "moon", "تیره"], ["light", "sun", "روشن"]].map(([id, ic, label]) =>
        '<button role="radio" aria-checked="' + (pref === id) + '" class="' + (pref === id ? "on" : "") + '" data-theme-set="' + id + '">' + icon(ic, 14) + " " + label + "</button>").join("") +
      '</div><div class="hint start">«خودکار» از پوسته تلگرام پیروی می کند.</div></div>');
    const rightsOk = health && !health.issues.some((i) => i.fix === "rights");
    const add = addToGroupLink();
    const share = SITE.bot ? "https://t.me/share/url?url=" + encodeURIComponent("https://t.me/" + SITE.bot) + "&text=" + encodeURIComponent("ربات مدیریت گروه؛ ضد اسپم، قفل محتوا و داشبورد کامل") : "";
    col2 += sec("راهنما و ربات",
      sheetRow("help", "book", "دستورهای داخل گروه", "کارهایی که با ریپلای روی پیام انجام می شود") +
      sheetRow("rights-help", "shieldCheck", "دسترسی های لازم ربات", rightsOk ? "همه دسترسی ها داده شده" : "راهنمای ادمین کردن ربات", rightsOk) +
      (add ? '<button class="row" data-open-link="' + esc(add) + '"><span class="rico">' + icon("plus", 17) + '</span><span class="rt"><span class="t">افزودن ربات به گروه دیگر</span><span class="s">دسترسی های لازم از پیش انتخاب شده</span></span>' + CHEV + "</button>" : "") +
      (share ? '<button class="row" data-open-link="' + esc(share) + '"><span class="rico">' + icon("send", 17) + '</span><span class="rt"><span class="t">معرفی ربات به دیگران</span><span class="s">فرستادن لینک ربات در تلگرام</span></span>' + CHEV + "</button>" : ""));
    html += '<div class="grid2"><div class="col">' + col1 + '</div><div class="col">' + col2 + "</div></div>";
    html += '<div class="about">داده ها مستقیم از ربات خوانده می شوند · تغییرات فورا در گروه اعمال می شود</div>';
    return html;
  }

  const PAGE_LOADERS = {
    locks: () => Promise.all([api("/locks"), api("/bot-lock").catch(() => null), api("/allowed-bots").catch(() => null)]).then(([d, b, a]) => { d.botOptions = b; d.allowedBots = a; return d; }),
    rights: () => api("/rights"),
    log: () => api("/log"),
    welcome: () => api("/welcome"),
    rules: () => api("/rules"),
    joingate: () => api("/join-gate"),
    imgf: () => api("/lists/imgf"),
    voice: () => api("/voice"),
    cases: (page) => {
      const was = page && page.data;
      const status = (was && was.status) || "open";
      const user = (was && was.user) || "";
      return api("/cases?status=" + status + (user ? "&user_id=" + encodeURIComponent(user) : "")).then(async (data) => {
        data.status = status;
        data.user = user;
        if (was && was.detail) data.detail = await api("/cases/" + was.detail.id).catch(() => was.detail);
        return data;
      });
    },
  };

  async function pushPage(id) {
    if (id === "settings") return goTab("settings");
    flushField();
    const page = { id, data: null, error: null };
    if (S.sheet && S.sheet.id !== "picker") page.fromSheet = S.sheet.id;
    S.stack.push(page);
    S.sheet = null;
    S.confirm = null;
    S.palette = null;
    S.listQuery = "";
    render();
    window.scrollTo(0, 0);
    await loadPage(page);
  }

  function pushRoot(id) {
    return leaveTo(() => {
      S.stack = [];
      setClosingConfirmation(false);
      return pushPage(id);
    });
  }

  async function loadPage(page) {
    const chat = S.chat;
    const keep = page.data;
    page.error = null;
    try {
      if (page.id.startsWith("list:")) {
        const kind = page.id.slice(5);
        const [d, mode] = await Promise.all([
          api("/lists/" + kind + (page.remoteQ ? "?q=" + encodeURIComponent(page.remoteQ) : "")),
          kind === "filter" ? api("/filter-words").catch(() => null) : Promise.resolve(null),
        ]);
        if (S.chat !== chat) return;
        page.data = d;
        page.mode = mode;
        S.lists = S.lists || {};
        if (!page.remoteQ) S.lists[kind] = page.data;
      } else {
        const data = await PAGE_LOADERS[page.id](page);
        if (S.chat !== chat) return;
        page.data = data;
        if (page.id === "locks") S.lockCache = page.data;
        if (page.id === "rights") S.rights = page.data;
        if (page.id === "log") S.logInfo = page.data;
        if (page.id === "welcome") S.welcomeInfo = page.data;
        if (page.id === "joingate") S.joinGate = page.data;
        if (DOCS[page.id]) {
          page.saved = decode(page.data.text || "");
          if (page.draft == null || !keep) page.draft = page.saved;
        }
      }
    } catch (e) {
      if (keep) page.data = keep;
      else page.error = (e && e.detail) || (e && e.status === 429 ? "کمی صبر کنید و دوباره امتحان کنید." : e && e.status === 403 ? "دسترسی این بخش برای شما بسته است." : "خوانده نشد.");
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
      case "rules":
        return renderDocPage(page);
      case "joingate":
        return renderJoinGatePage(page);
      case "imgf":
        return renderImgfPage(page);
      case "voice":
        return renderVoicePage(page);
      case "cases":
        return renderCasesPage(page);
      default:
        return renderPageTop("", gtitle) + '<div id="main" class="nobar"></div>';
    }
  }

  const PAGE_TITLES = { locks: "قفل ها", rights: "دسترسی اعضا", log: "کانال لاگ", welcome: "پیام خوشامد", rules: "قوانین گروه", joingate: "عضویت اجباری در کانال", imgf: "فیلتر تصویری", voice: "کلمه های نامناسب ویس", cases: "پرونده ها" };

  function pageTitle(page) {
    if (page.id.startsWith("list:")) return LIST_KINDS[page.id.slice(5)][0];
    return PAGE_TITLES[page.id] || "";
  }

  function loadingList() {
    return '<div class="sec first"><div class="sk" style="height:180px;border-radius:18px"></div></div>';
  }

  function searchBox(id, placeholder, value) {
    return '<label class="search">' + icon("search", 16) + '<input id="' + id + '" type="search" placeholder="' + esc(placeholder) + '" value="' + esc(value || "") + '" /></label>';
  }

  const CASE_STATUS = { open: ["باز", "warn"], resolved: ["رسیدگی شده", "ok"], reversed: ["لغو شده", "mute"] };
  const CASE_ACTION = { none: "بدون اقدام", delete: "حذف پیام", warn: "اخطار", mute: "سکوت", ban: "بن", kick: "اخراج", unmute: "رفع سکوت", unban: "رفع بن", reverse: "لغو برخورد" };
  const CASE_SOURCE = { automatic: "خودکار، توسط ربات", moderator: "توسط ادمین", report: "گزارش یک عضو" };
  const CASE_EVENT = {
    reported: ["گزارش شد", "warn", "alert"],
    action_succeeded: ["اقدام انجام شد", "ok", "check"],
    action_failed: ["اقدام ناموفق بود", "bad", "x"],
    resolved: ["رسیدگی شد", "ok", "check"],
    reversed: ["اقدام لغو شد", "warn", "refresh"],
    note: ["یادداشت", "", "fileText"],
  };

  function caseStatus(value) {
    return (CASE_STATUS[value] || [value])[0];
  }
  function caseAction(value) {
    return CASE_ACTION[value] || value;
  }

  function caseRow(c) {
    const st = CASE_STATUS[c.status] || [c.status, "mute"];
    return '<button class="row" data-case="' + c.id + '"><span class="rico ' + (c.status === "open" ? "warn" : "") + '">' + icon(c.source === "report" ? "alert" : "fileText", 16) + '</span><span class="rt"><span class="t">' + (c.subject_name ? "<bdi>" + esc(c.subject_name) + "</bdi>" : "کاربر نامشخص") + '</span><span class="s">' + esc(c.reason) + " · " + ago(c.created_at) + '</span></span><span class="pill ' + st[1] + '">' + esc(st[0]) + "</span></button>";
  }

  const MEDIA_KIND = { text: "متن", photo: "عکس", video: "ویدیو", animation: "گیف", gif: "گیف", sticker: "استیکر", voice: "ویس", video_note: "ویدیو مسیج", audio: "آهنگ", music: "آهنگ", document: "فایل", file: "فایل", poll: "نظرسنجی", contact: "مخاطب", location: "موقعیت" };

  function messageLink(c) {
    const chat = String(S.chat);
    if (!c.message || !/^-100\d+$/.test(chat)) return "";
    return "https://t.me/c/" + chat.slice(4) + "/" + c.message;
  }

  function renderCasesPage(page) {
    const d = page.data;
    let html = renderPageTop(d && d.detail ? "پرونده " + fa(d.detail.id) : "پرونده ها", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!d) return html + loadingList() + "</div>";
    if (d.detail) {
      const c = d.detail;
      const st = CASE_STATUS[c.status] || [c.status, "mute"];
      const busy = S.caseBusy;
      html += '<div class="sec first"><div class="card"><div class="profile"><span class="rico ' + (st[1] === "warn" ? "warn" : "ok") + '" style="width:48px;height:48px;border-radius:15px">' + icon("fileText", 22) + '</span><span class="rt"><span class="t wrap">' + (c.subject_name ? "<bdi>" + esc(c.subject_name) + "</bdi>" : "کاربر نامشخص") + '</span><span class="s wrap">' + esc(c.reason) + '</span></span><span class="pill ' + st[1] + '">' + esc(st[0]) + "</span></div>" +
        '<dl class="kv" style="border-top:1px solid var(--border);margin:0">' +
        "<dt>اقدام</dt><dd>" + esc(caseAction(c.action)) + "</dd>" +
        "<dt>منبع</dt><dd>" + esc(CASE_SOURCE[c.source] || c.source || "—") + "</dd>" +
        (c.subject ? '<dt>شناسه کاربر</dt><dd><bdi dir="ltr" class="num">' + esc(c.subject) + "</bdi></dd>" : "") +
        (c.media_kind ? "<dt>نوع پیام</dt><dd>" + esc(MEDIA_KIND[c.media_kind] || c.media_kind) + "</dd>" : "") +
        "<dt>زمان</dt><dd>" + esc(ago(c.created_at)) + "</dd>" +
        "</dl></div>";
      if (c.evidence) html += '<div class="h" style="padding-top:16px"><div class="t">متن پیام</div></div><div class="evidence">' + esc(c.evidence) + "</div>";
      const link = c.status === "open" ? messageLink(c) : "";
      if (link) html += '<button class="btn outline block" style="margin-top:12px" data-case-open="' + esc(link) + '">' + icon("chat", 16) + " دیدن پیام در گروه</button>";
      html += "</div>";
      const events = c.events || [];
      if (events.length) {
        html += '<div class="sec"><div class="h"><div class="t">تاریخچه</div></div><div class="card timeline">' + events.map((ev) => {
          const k = CASE_EVENT[ev.kind] || [ev.kind, "", "info"];
          const named = ev.action && ev.action !== "none" && ev.action !== "reverse" ? " · " + esc(caseAction(ev.action)) : "";
          return '<div class="tl"><span class="tdot ' + k[1] + '">' + icon(k[2], 12, 2.6) + '</span><div class="tb"><div class="tt">' + esc(k[0]) + named + '</div><div class="ts"><bdi>' + esc(ev.actor_name && !/^\d+$/.test(ev.actor_name) ? ev.actor_name : ev.actor ? "ادمین" : "ربات") + "</bdi> · " + esc(ago(ev.created_at)) + (ev.note ? " · " + esc(ev.note) : "") + "</div></div></div>";
        }).join("") + "</div></div>";
      }
      const dis = busy ? " disabled" : "";
      html += '<div class="sec"><div class="h"><div class="t">رسیدگی</div></div><textarea class="text-input" id="case-note" maxlength="500" placeholder="یادداشت اختیاری؛ در تاریخچه ثبت می شود" aria-label="یادداشت پرونده" style="min-height:84px"></textarea><div class="btns" style="padding-top:10px">';
      if (c.status === "open") html += '<button class="btn ghost" data-case-action="none"' + dis + ">" + (busy === "none" ? '<span class="spinner"></span>' : icon("check", 16)) + ' بررسی شد، بدون اقدام</button><button class="btn danger" data-case-action="delete"' + dis + ">" + (busy === "delete" ? '<span class="spinner"></span>' : icon("trash", 16)) + " حذف پیام</button>";
      if (c.status === "resolved" && ["warn", "mute", "ban"].includes(c.action)) html += '<button class="btn soft-danger" data-case-action="reverse"' + dis + ">" + (busy === "reverse" ? '<span class="spinner"></span>' : "") + "لغو " + esc(caseAction(c.action)) + "</button>";
      html += '<button class="btn outline" data-case-note' + dis + ">ثبت یادداشت</button></div></div></div>";
      return html;
    }
    const status = d.status || "open";
    const loading = S.casesLoading;
    html += '<div class="sec first"><div class="seg" role="tablist" aria-label="وضعیت پرونده">' +
      [["open", "باز"], ["resolved", "رسیدگی شده"], ["all", "همه"]].map(([id, label]) => '<button role="tab" aria-selected="' + (status === id) + '" class="' + (status === id ? "on" : "") + '" data-case-filter="' + id + '">' + label + "</button>").join("") +
      '</div><div class="addrow" style="margin-top:10px"><label class="search" style="flex:1">' + icon("search", 16) + '<input id="case-user" inputmode="numeric" enterkeyhint="search" placeholder="شناسه عددی کاربر" aria-label="جستجو با شناسه عددی کاربر" value="' + esc(d.user || "") + '" /></label><button class="btn ghost" data-case-search>جستجو</button></div>' +
      (d.user ? '<div class="hint start">پرونده های کاربر <bdi dir="ltr" class="num">' + esc(d.user) + '</bdi> · <button class="more" data-case-clear>همه کاربران</button></div>' : "") + "</div>";
    html += '<div class="sec' + (loading ? " loading" : "") + '">';
    if (!(d.cases || []).length) html += '<div class="card"><div class="empty sm"><div class="glyph">' + icon("shieldCheck", 26) + '</div><div class="t">پرونده ای نیست</div><div class="s">' + (d.user ? "این کاربر پرونده ای در این بخش ندارد." : status === "open" ? "همه گزارش ها رسیدگی شده است." : "چیزی پیدا نشد.") + "</div></div></div>";
    else html += '<div class="list">' + d.cases.map(caseRow).join("") + "</div>";
    if (d.has_more && d.cases.length) html += '<button class="btn ghost block" style="margin-top:12px" data-case-more="' + d.cases[d.cases.length - 1].id + '"' + (S.casesMore ? " disabled" : "") + ">" + (S.casesMore ? '<span class="spinner"></span> در حال خواندن…' : "پرونده های بیشتر") + "</button>";
    html += '<div class="hint">اعضا با ریپلای و «گزارش» پرونده باز می کنند؛ برخوردهای خودکار ربات هم اینجا ثبت می شود.</div>';
    return html + "</div></div>";
  }

  const LOCK_GROUPS = [
    ["رسانه", ["photo", "video", "gif", "sticker", "animsticker", "music", "voice", "file", "media", "spoiler", "story"]],
    ["متن و لینک", ["links", "hyperlink", "hashtag", "username", "mention", "emoji", "premoji", "english", "persian", "edit"]],
    ["منبع و پیوست", ["forward_channel", "forward_user", "anon", "bot", "button", "commands", "botcall", "service", "pin", "comment", "promoter", "contact", "location", "poll", "dice", "biolink"]],
  ];

  function lockGroups(plain) {
    const used = new Set();
    const out = LOCK_GROUPS.map(([title, keys]) => {
      const items = keys.map((k) => plain.find((l) => l.key === k)).filter(Boolean);
      items.forEach((l) => used.add(l.key));
      return [title, items];
    });
    const rest = plain.filter((l) => !used.has(l.key));
    if (rest.length) out.push(["سایر", rest]);
    return out.filter(([, items]) => items.length);
  }

  function lockChip(l) {
    return '<button class="chip' + (l.on ? " on" : "") + '" data-lock="' + esc(l.key) + '" aria-pressed="' + (l.on ? "true" : "false") + '">' + icon(l.icon || (l.on ? "LOCKED" : "UNLOCKED"), 13, 2.4) + l.label + "</button>";
  }

  function shadowFlag(key) {
    if (key === "advert") return "ad_shadow";
    if (key === "trade") return "tr_shadow";
    if (/^c_/.test(key)) return "cq_shadow";
    return null;
  }
  function isShadowed(key) {
    const flag = shadowFlag(key);
    return !!(flag && S.dash && isOn(flag));
  }

  function lockTile(l) {
    const shadow = l.on && isShadowed(l.key);
    return '<button class="lock' + (l.on ? " on" : "") + (shadow ? " shadow" : "") + '" data-lock="' + esc(l.key) + '" aria-pressed="' + (l.on ? "true" : "false") + '"' + (shadow ? ' title="روشن، ولی فقط بررسی می کند و حذف نمی کند"' : "") + '><span class="li">' + icon(l.icon || (l.on ? "LOCKED" : "UNLOCKED"), 14, 2.4) + '</span><span class="ln">' + l.label + (shadow ? "<small>فقط بررسی، بدون حذف</small>" : "") + '</span><span class="ls"></span></button>';
  }

  function renderLocksPage(page) {
    const sum = S.dash.locks_summary;
    let html = renderPageTop("قفل ها", fa(sum.active) + " از " + fa(sum.total) + " روشن") + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const q = fold(S.lockQuery);
    const view = S.lockView;
    html += '<div class="sec first">' + searchBox("lock-search", "جستجوی قفل، مثلا «لینک» یا «عکس»", S.lockQuery) +
      '<div class="seg" style="margin-top:10px" role="tablist" aria-label="نمایش">' +
      [["all", "همه قفل ها"], ["on", "روشن ها"], ["off", "خاموش ها"]].map(([id, label]) => '<button role="tab" aria-selected="' + (view === id) + '" class="' + (view === id ? "on" : "") + '" data-lock-view="' + id + '">' + label + "</button>").join("") +
      "</div></div>";
    const visible = (l) => (!q || fold(l.label).includes(q)) && (view === "all" || (view === "on" ? l.on : !l.on));
    const groupHtml = (title, items, bulk) => {
      const shownItems = items.filter(visible);
      if (!shownItems.length) return "";
      const on = items.filter((l) => l.on).length;
      const all = on === items.length;
      const action = bulk
        ? '<button class="hbtn' + (all ? " off" : "") + '" data-lock-group="' + esc(title) + '" data-on="' + (all ? 0 : 1) + '">' + (all ? "خاموش کردن همه" : "روشن کردن همه") + "</button>"
        : '<button class="more" data-open="cq">تنظیمات ' + icon("chevL", 14) + "</button>";
      return '<div class="sec"><div class="h"><div class="t">' + title + ' <span class="count">· ' + fa(on) + " از " + fa(items.length) + "</span></div>" + action + '</div><div class="lockgrid">' + shownItems.map(lockTile).join("") + "</div></div>";
    };
    let body = "";
    lockGroups(page.data.plain).forEach(([title, items]) => {
      body += groupHtml(title, items, true);
    });
    if (page.data.ai.length) body += groupHtml("نگهبان هوشمند", page.data.ai, false);
    html += body || '<div class="empty sm"><div class="glyph">' + icon("search", 24) + '</div><div class="t">قفلی با این مشخصات نیست</div>' + (q ? '<div class="s">«' + esc(S.lockQuery) + "» در نام هیچ قفلی نیست.</div>" : "") + "</div>";
    if (q) return html + "</div>";

    const botOn = !!(page.data.plain.find((l) => l.key === "bot") || {}).on;
    const b = page.data.botOptions;
    if (b && b.items) {
      html += '<div class="sec"><div class="h"><div class="t">رفتار قفل ربات' + (botOn ? "" : '<span class="sub">با روشن کردن قفل «ربات» اعمال می شود</span>') + '</div></div><div class="list' + (botOn ? "" : " idle") + '">' +
        b.items.filter((it) => it.key !== "bot").map((it) => '<div class="row"><span class="rt"><span class="t n">' + esc(it.label) + "</span></span>" + swb('data-botopt="' + esc(it.key) + '"', it.on, it.label) + "</div>").join("") +
        "</div></div>";
    }
    const ab = page.data.allowedBots;
    if (ab && ab.bots) {
      html += '<div class="sec"><div class="h"><div class="t">ربات های مجاز <span class="count">· ' + fa(ab.bots.length) + '</span><span class="sub">قفل ربات این ها را حذف نمی کند</span></div></div>' +
        '<div class="addrow"><input class="text-input" id="allow-bot" dir="ltr" autocapitalize="off" autocomplete="off" enterkeyhint="done" placeholder="@examplebot" aria-label="یوزرنیم ربات" /><button class="btn" data-allow-bot>' + icon("plus", 16) + " افزودن</button></div>" +
        (ab.bots.length ? '<div class="list" style="margin-top:10px">' + ab.bots.map((b) => '<div class="row compact"><span class="rico sm on">' + icon("bot", 15) + '</span><span class="rt"><span class="t n"><bdi dir="ltr">' + (b.name || b.id) + '</bdi></span></span><button class="ibtn danger" data-disallow-bot="' + esc(b.id) + '" aria-label="' + esc("حذف " + decode(String(b.name || b.id)) + " از ربات های مجاز") + '">' + icon("x", 18) + "</button></div>").join("") + "</div>" : "") +
        "</div>";
    }

    const packs = S.lists && S.lists.pack;
    html += '<div class="sec"><div class="h"><div class="t">بیشتر</div></div><div class="list">' +
      '<button class="row" data-push="list:pack"><span class="rico">' + icon("sparkles", 17) + '</span><span class="rt"><span class="t">پک های استیکر قفل شده</span><span class="s">قفل کردن: در گروه روی استیکر «قفل پک»</span></span>' + (packs ? '<span class="count">' + fa(packs.entries.length) + "</span>" : "") + CHEV + "</button>" +
      '<button class="row" data-push="list:filter"><span class="rico">' + icon("funnel", 17) + '</span><span class="rt"><span class="t">فیلتر کلمات</span><span class="s">پیامی که این کلمه ها را دارد حذف می شود</span></span>' + CHEV + "</button>" +
      "</div></div>";

    html += '<div class="sec"><div class="card bulk"><div class="rt"><span class="t">همه قفل ها با هم</span><span class="s">ادمین ها و کاربران ویژه از همه قفل ها معاف اند.</span></div><div class="btns">' +
      '<button class="btn sm ghost" data-locks-all="1">' + icon("lock", 14) + ' روشن کردن همه</button><button class="btn sm soft-danger" data-locks-all="0">' + icon("unlock", 14) + " خاموش کردن همه</button></div></div></div>";
    return html + "</div>";
  }

  function renderRightsPage(page) {
    let html = renderPageTop("دسترسی اعضا", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const rights = page.data.rights;
    const open = rights.filter((r) => r.open).length;
    const lock = page.data.lock || {};
    const until = lock.until && lock.until > Date.now() / 1000 ? lock.until : null;
    const g = { manual: !!lock.manual, until, night: !!lock.night, allClosed: open === 0 };
    const shut = g.manual || !!g.until || g.night;
    const busy = S.lockBusy;
    let act;
    if (g.manual || g.until) act = '<button class="btn" data-group-lock="0"' + (busy ? " disabled" : "") + ">" + icon("unlock", 16) + (busy === "opening" ? " در حال باز کردن…" : " باز کردن گروه") + "</button>";
    else if (g.night) act = '<button class="btn ghost" data-open="ng">' + icon("moon", 16) + " تنظیم قفل شب</button>";
    else if (g.allClosed) act = '<button class="btn" data-rights-all="1"' + (S.rightsBusy ? " disabled" : "") + ">" + icon("unlock", 16) + " باز کردن همه دسترسی ها</button>";
    else act = '<button class="btn soft-danger" data-group-lock="1"' + (busy ? " disabled" : "") + ">" + icon("lock", 16) + (busy === "closing" ? " در حال بستن…" : " بستن گروه") + "</button>";
    const sub = g.manual ? "با «قفل گروه» بسته شده؛ فقط ادمین ها می توانند پیام بدهند."
      : g.until ? "تا " + groupStateText(g).replace("بسته تا ", "") + " بسته است؛ بعد خودش باز می شود."
      : g.night ? "قفل شب روشن است؛ در پایان بازه خودش باز می شود."
      : g.allClosed ? "همه دسترسی ها یکی یکی بسته شده اند؛ هیچ عضو عادی نمی تواند چیزی بفرستد."
      : "اعضای عادی هر چه زیر روشن است را می توانند بفرستند.";
    const closedNow = shut || g.allClosed;
    html += '<div class="sec first"><div class="card state-card' + (closedNow ? " shut" : "") + '"><div class="profile"><span class="rico ' + (closedNow ? "bad" : "ok") + '" style="width:48px;height:48px;border-radius:15px">' + icon(closedNow ? "lock" : "unlock", 22) + '</span><span class="rt"><span class="t">' + (closedNow ? "گروه بسته است" : "گروه باز است") + '</span><span class="s wrap">' + sub + "</span></span></div>" +
      '<div class="state-act">' + act + "</div></div>" +
      (page.data.pending ? '<div class="note warn" style="margin-top:10px">' + icon("clock", 16) + "<span>تغییری در صف اعمال است و تا چند لحظه دیگر در تلگرام دیده می شود.</span></div>" : "") + "</div>";
    const rbusy = S.rightsBusy;
    html += '<div class="sec"><div class="h"><div class="t">اعضای عادی می توانند <span class="count">· ' + fa(open) + " از " + fa(rights.length) + "</span>" +
      (shut ? '<span class="sub">بعد از باز شدن گروه همین ها اعمال می شود</span>' : "") + '</div><div class="hacts">' +
      '<button class="hbtn" data-rights-all="1"' + (open === rights.length || rbusy ? " disabled" : "") + ">همه باز</button>" +
      '<button class="hbtn off" data-rights-all="0"' + (open === 0 || rbusy ? " disabled" : "") + ">همه بسته</button></div></div>" +
      '<div class="list' + (rbusy ? " busy" : "") + '">' +
      rights.map((r) =>
        '<div class="row"><span class="rico sm' + (r.open ? " on" : "") + '">' + icon(r.icon || (r.open ? "UNLOCKED" : "LOCKED"), 15) + '</span><span class="rt"><span class="t n">' + r.label + '</span></span><button class="swb" data-right="' + esc(r.key) + '" role="switch" aria-checked="' + (r.open ? "true" : "false") + '" aria-label="' + String(r.label).replace(/"/g, "&quot;") + '"><span class="sw' + (r.open ? " on" : "") + '"></span></button></div>'
      ).join("") + "</div></div>";
    html += '<div class="hint">ادمین ها همیشه می توانند بفرستند. بستن برای مدت مشخص: در گروه «قفل گروه ۲ ساعت».</div>';
    return html + "</div>";
  }

  function renderLogPage(page) {
    let html = renderPageTop("کانال لاگ", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    const on = d.kinds.filter((k) => k.on).length;
    html += '<div class="sec first"><div class="card"><div class="profile"><span class="rico' + (d.channel ? " ok" : " warn") + '" style="width:48px;height:48px;border-radius:15px">' + icon("fileText", 22) + '</span><span class="rt"><span class="t">' + (d.channel ? "کانال وصل است" : "کانالی وصل نیست") + '</span><span class="s">' +
      (d.channel ? (/^-?\d+$/.test(decode(d.channel)) ? 'کانال خصوصی · <bdi dir="ltr" class="num">' + d.channel + "</bdi>" : '<bdi dir="ltr">' + (decode(d.channel).startsWith("@") ? "" : "@") + d.channel + "</bdi>") : "رویدادها جایی ثبت نمی شود") + "</span></span>" + (d.channel ? '<span class="pill ok">وصل</span>' : '<span class="pill warn">ندارد</span>') + "</div></div>" +
      '<div class="field"><div class="fl"><span>' + (d.channel ? "تغییر کانال" : "وصل کردن کانال") + '</span></div><div class="addrow"><input class="text-input" id="log-channel" dir="ltr" autocapitalize="off" autocomplete="off" enterkeyhint="done" placeholder="@channel" /><button class="btn" data-log-connect>' + (d.channel ? "تغییر" : "وصل کن") + "</button></div>" +
      '<div class="hint start">اول ربات را در کانال ادمین کنید. ربات یک پیام آزمایشی در کانال می فرستد. برای کانال خصوصی: یک پیام کانال را در گروه فوروارد کنید و روی آن «تنظیم لاگ» بزنید.</div></div></div>';
    html += '<div class="sec"><div class="h"><div class="t">چه رویدادهایی ثبت شود <span class="count">· ' + fa(on) + " از " + fa(d.kinds.length) + "</span>" + (d.channel ? "" : '<span class="sub">پس از وصل شدن کانال ثبت می شود</span>') + "</div></div>" +
      '<div class="list' + (d.channel ? "" : " idle") + '">' +
      d.kinds.map((k) =>
        '<div class="row"><span class="rt"><span class="t n">' + k.label + "</span></span>" + swb('data-log="' + esc(k.key) + '"', k.on, k.label) + "</div>"
      ).join("") + "</div></div>";
    if (d.channel) html += '<div class="sec"><button class="btn soft-danger block" data-confirm="log-off">' + icon("x", 16) + " قطع کانال لاگ</button></div>";
    return html + "</div>";
  }

  const DOCS = {
    welcome: {
      title: "پیام خوشامد", path: "/welcome", placeholder: "سلام {نام}، به {گروه} خوش آمدی!", off: "welcome-off",
      tags: [["{نام}", "نام"], ["{منشن}", "منشن"], ["{آیدی}", "آیدی"], ["{یوزرنیم}", "یوزرنیم"], ["{گروه}", "گروه"]],
    },
    rules: {
      title: "قوانین گروه", path: "/rules", placeholder: "۱. تبلیغ ممنوع\n۲. احترام به همه اعضا\n…", off: "rules-off", tags: [],
    },
  };

  function docDirty(page) {
    return page && page.draft != null && page.saved != null && page.draft !== page.saved;
  }

  function docPreview(text) {
    const samples = { "{نام}": "علی", "{منشن}": "علی", "{آیدی}": "123456789", "{یوزرنیم}": "@ali", "{گروه}": decode(S.dash.chat.title) };
    let html = esc(text);
    Object.keys(samples).forEach((tag) => {
      html = html.split(esc(tag)).join('<span class="ph">' + esc(samples[tag]) + "</span>");
    });
    return html || '<span style="color:var(--muted)">متنی نوشته نشده.</span>';
  }

  function setClosingConfirmation(on) {
    try {
      if (!tg) return;
      if (on && tg.enableClosingConfirmation) tg.enableClosingConfirmation();
      if (!on && tg.disableClosingConfirmation) tg.disableClosingConfirmation();
    } catch (e) {
    }
  }

  function syncDoc(page) {
    const dirty = docDirty(page);
    const bar = document.getElementById("doc-bar");
    if (bar) bar.hidden = !dirty;
    const preview = document.getElementById("doc-preview");
    if (preview) preview.innerHTML = docPreview(page.draft || "");
    const count = document.getElementById("doc-count");
    if (count) count.textContent = fa(Array.from(page.draft || "").length) + " نویسه";
    setClosingConfirmation(dirty);
  }

  function renderDocPage(page) {
    const doc = DOCS[page.id];
    let html = renderPageTop(doc.title, S.dash.chat.title) + '<div id="main" class="nobar has-savebar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    const has = !!(d.text || d.has_media);
    html += '<div class="grid2"><div class="col"><div class="sec first"><div class="field" style="padding-top:0"><div class="fl"><span>متن</span><span id="doc-count" class="count"></span></div>' +
      '<textarea class="text-input" id="doc-text" aria-label="' + esc("متن " + doc.title) + '" placeholder="' + esc(doc.placeholder) + '">' + esc(page.draft || "") + "</textarea>" +
      (doc.tags.length ? '<div class="chips" style="margin-top:10px">' + doc.tags.map(([tag, label]) => '<button class="chip sm" data-insert="' + esc(tag) + '" aria-label="' + esc("گذاشتن " + label + " در متن") + '">' + icon("plus", 12) + label + "</button>").join("") + "</div>" +
        '<div class="hint start">روی هر برچسب بزنید تا در متن گذاشته شود.' + (d.has_media ? " · رسانه همراه فقط از داخل گروه عوض می شود" : " · برای خوشامد با عکس: در گروه روی عکس ریپلای کنید و «تنظیم خوشامد» بفرستید.") + "</div>" : '<div class="hint start">اعضا با فرستادن «قوانین» در گروه این متن را می بینند.</div>') + "</div>" +
      (has ? '<div style="padding-top:14px"><button class="btn soft-danger block" data-confirm="' + doc.off + '">' + icon("trash", 16) + " حذف " + doc.title + "</button></div>" : "") +
      "</div></div>";
    html += '<div class="col"><div class="sec"><div class="h"><div class="t">پیش نمایش</div>' + (d.has_media ? '<span class="pill mute">' + icon("image", 12) + " همراه رسانه</span>" : "") + '</div><div class="chatbg"><div class="preview" id="doc-preview"></div></div></div>';
    const wct = page.id === "welcome" ? setting("wct") : null;
    if (wct) html += '<div class="sec"><div class="h"><div class="t">حذف خودکار پیام خوشامد</div></div><div class="card pad">' + renderNumberField(wct, true) + "</div></div>";
    html += "</div></div>";
    html += '<div class="savebar" id="doc-bar" hidden><span class="s"><span class="dot warn"></span>تغییرات ذخیره نشده</span><button class="btn ghost sm" data-doc-revert>کنار گذاشتن</button><button class="btn sm" data-doc-save>' + icon("check", 16) + " ذخیره</button></div>";
    return html + "</div>";
  }

  function renderJoinGatePage(page) {
    let html = renderPageTop("عضویت اجباری در کانال", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    html += '<div class="sec first">' +
      (d.channel ? '<div class="list" style="margin-bottom:12px"><div class="row"><span class="rico on">' + icon("door", 17) + '</span><span class="rt"><span class="t"><bdi dir="ltr">@' + esc(decode(d.channel).replace(/^@/, "")) + '</bdi></span><span class="s on">روشن · تا عضو این کانال نشوند پیام نمی دهند</span></span></div></div>' : "") +
      '<div class="note mute">' + icon("info", 16) + "<span>تا وقتی عضو کانال نشوند نمی توانند پیام بدهند. ربات باید در آن کانال ادمین باشد.</span></div>" +
      '<div class="field"><div class="fl"><span>' + (d.channel ? "تغییر کانال" : "یوزرنیم کانال عمومی") + '</span></div><input class="text-input" id="join-gate-input" dir="ltr" autocapitalize="off" autocomplete="off" enterkeyhint="done" placeholder="@mychannel" aria-label="یوزرنیم کانال" value="' + esc(d.channel ? "@" + decode(d.channel).replace(/^@/, "") : "") + '" /><div class="hint start">یوزرنیم یا لینک t.me کانال؛ ربات اول بررسی می کند که کانال پیدا شود.</div></div>' +
      '<div class="btns" style="padding-top:12px"><button class="btn" data-joingate-save>' + icon("check", 16) + (d.channel ? " ذخیره کانال تازه" : " روشن کردن") + "</button>" +
      (d.channel ? '<button class="btn soft-danger" data-joingate-off>خاموش کردن</button>' : "") + "</div></div>";
    return html + "</div>";
  }

  function renderImgfPage(page) {
    let html = renderPageTop("فیلتر تصویری", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const entries = page.data.entries;
    html += '<div class="sec first"><div class="note">' + icon("sparkles", 16) + "<span>یک موضوع بنویسید، مثل «خودرو لوکس»؛ هر عکسی که به آن نزدیک باشد حذف می شود. اول در حالت «فقط بررسی» ساخته می شود.</span></div>" +
      '<div class="addrow" style="padding-top:12px"><input class="text-input" id="filter-phrase" maxlength="32" enterkeyhint="done" placeholder="موضوع تازه" /><button class="btn" data-filter-add>' + icon("plus", 16) + " افزودن</button></div></div>";
    html += '<div class="sec"><div class="h"><div class="t">موضوع های شما</div><div class="count">' + fa(entries.length) + " از ۸</div></div>";
    if (!entries.length) html += '<div class="card"><div class="empty sm"><div class="glyph">' + icon("image", 24) + '</div><div class="t">هنوز موضوعی نساخته اید</div><div class="s">برای فیلتر با نمونه عکس: در گروه روی عکس «فیلتر این ‹نام›».</div></div></div>';
    else {
      html += '<div class="list">' + entries.map((e) => {
        const live = !e.name.endsWith(" ~");
        const label = live ? e.name : e.name.slice(0, -2);
        const pending = S.pending === "imgf:" + e.key;
        return '<div class="row"><span class="rico' + (live ? " on" : "") + '">' + icon("image", 17) + '</span><span class="rt"><span class="t">' + bdi(label) + '</span><span class="s' + (live ? " on" : "") + '">' + (live ? "فعال · حذف می کند" : "فقط بررسی · حذف نمی کند") + "</span></span>" +
          swb('data-filter-toggle="' + esc(e.key) + '"', live, decode(label)) +
          (pending
            ? '<button class="btn sm danger" data-remove="imgf" data-key="' + esc(e.key) + '">حذف؟</button>'
            : '<button class="ibtn danger" data-remove="imgf" data-key="' + esc(e.key) + '" aria-label="' + esc("حذف موضوع " + decode(label)) + '">' + icon("x", 18) + "</button>") +
          "</div>";
      }).join("") + "</div>";
    }
    return html + "</div></div>";
  }

  function renderVoicePage(page) {
    let html = renderPageTop("کلمه های نامناسب ویس", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    const q = fold(S.listQuery);
    html += '<div class="sec first"><div class="addrow"><input class="text-input" id="voice-word" maxlength="64" enterkeyhint="done" placeholder="کلمه تازه" aria-label="کلمه تازه" /><button class="btn" data-voice-add>' + icon("plus", 16) + " افزودن</button></div>" +
      (d.words.length > 8 ? '<div style="margin-top:10px">' + searchBox("list-search", "جستجو در " + fa(d.words.length) + " کلمه…", S.listQuery) + "</div>" : "") + "</div>";
    html += '<div class="sec"><div class="h"><div class="t">فهرست <span class="count">· ' + fa(d.words.length) + "</span></div>" +
      (d.words.length ? '<button class="more danger" data-confirm="voice-clear">پاک کردن همه</button>' : "") + "</div>";
    if (!d.words.length) html += '<div class="card"><div class="empty sm"><div class="glyph">' + icon("mic", 24) + '</div><div class="t">فهرست خالی است</div></div></div>';
    else {
      html += '<div class="list">' + d.words.map((w) =>
        '<div class="row compact"' + (q && !fold(w.word).includes(q) ? " hidden" : "") + '><span class="rt"><span class="t n"><bdi>' + esc(w.word) + '</bdi></span></span>' +
          (S.pending === "voice:" + w.key
            ? '<button class="btn sm danger" data-voice-remove="' + esc(w.key) + '">حذف؟</button>'
            : '<button class="ibtn danger" data-voice-remove="' + esc(w.key) + '" aria-label="' + esc("حذف " + w.word) + '">' + icon("x", 18) + "</button>") + "</div>"
      ).join("") + "</div>";
    }
    html += "</div>";
    if (d.disabled_defaults.length) html += '<div class="sec"><button class="btn ghost block" data-voice-restore>' + icon("refresh", 15) + " بازگرداندن " + fa(d.disabled_defaults.length) + " کلمه پیش فرض</button></div>";
    return html + "</div>";
  }

  const LIST_SEARCH_THRESHOLD = 8;

  function entryName(kind, e) {
    if ((kind === "vip" || kind === "free") && /^\d+$/.test(decode(e.name))) {
      const ad = S.admins && S.admins !== "error" ? S.admins.admins.find((a) => String(a.id) === decode(e.name)) : null;
      return ad ? bdi(ad.name) : 'کاربر <bdi dir="ltr" class="num">' + e.name + "</bdi>";
    }
    return bdi(e.name);
  }

  const REMOVE_WORD = { ban: "رفع بن", mute: "رفع سکوت" };

  function renderListPage(page, kind) {
    const [title, ic, desc] = LIST_KINDS[kind];
    const d = page.data;
    const right = d && d.entries.length ? '<button class="ibtn danger" data-confirm="clear:' + kind + '" title="پاکسازی لیست" aria-label="پاکسازی همه لیست">' + icon("trash", 20) + "</button>" : "";
    let html = renderPageTop(title, d ? fa(d.entries.length) + (d.truncated ? "+" : "") + " مورد" : S.dash.chat.title, right) + '<div id="main" class="nobar">';
    if (!d) return html + loadingList() + "</div>";
    const q = fold(S.listQuery);
    const remote = (kind === "ban" || kind === "mute") && (d.truncated || page.remoteQ);
    html += '<div class="sec first">';
    if (kind === "filter") {
      const exact = page.mode && page.mode.exact;
      html += '<div class="addrow"><input class="text-input" id="filter-word" maxlength="64" enterkeyhint="done" placeholder="کلمه تازه، مثلا «تبلیغ»" /><button class="btn" data-word-add>' + icon("plus", 16) + " افزودن</button></div>" +
        (page.mode ? '<div class="field"><div class="fl"><span>کلمه کجا پیدا شود</span></div><div class="seg">' +
          '<button class="' + (exact ? "" : "on") + '" data-word-exact="0">هر جای متن</button><button class="' + (exact ? "on" : "") + '" data-word-exact="1">فقط کلمه جدا</button></div>' +
          '<div class="hint start">' + (exact ? "«بت» در «صحبت» پیدا نمی شود؛ فقط وقتی جدا آمده باشد." : "«بت» در «صحبت» هم پیدا می شود؛ برای کلمه های کوتاه «فقط کلمه جدا» را بزنید.") + "</div></div>" : "");
    } else if (kind === "answer") {
      html += '<div class="card pad"><div class="fl" style="font-size:13px;font-weight:600;margin-bottom:10px">پاسخ تازه یا ویرایش</div>' +
        '<input class="text-input" id="answer-trigger" maxlength="64" placeholder="وقتی کسی نوشت… مثلا «سلام»" />' +
        '<textarea class="text-input" id="answer-text" maxlength="3500" placeholder="ربات جواب بدهد… مثلا «درود، خوش آمدی»" style="min-height:84px;margin-top:8px"></textarea>' +
        '<button class="btn block" style="margin-top:10px" data-answer-add>' + icon("check", 16) + " ذخیره پاسخ</button>" +
        '<div class="hint start">برای ویرایش، روی یک پاسخ در فهرست بزنید. مخاطب پاسخ ها: ' + esc(pickLabel("an_act")) + ' · <button class="more" data-open="an">تغییر</button> · پاسخ با عکس: در گروه روی پیام «تنظیم پاسخ ‹کلمه›».</div></div>';
    } else if (LIST_HOWTO[kind]) {
      html += '<div class="note mute">' + icon("info", 16) + "<span>" + LIST_HOWTO[kind] + "</span></div>";
    }
    if (d.entries.length > LIST_SEARCH_THRESHOLD || remote) {
      html += '<div style="margin-top:10px">' + searchBox("list-search", remote ? "جستجوی نام در همه " + title : "جستجو در " + fa(d.entries.length) + " مورد…", S.listQuery) + "</div>";
    }
    html += "</div>";
    html += '<div class="sec">';
    if (!d.entries.length) {
      html += '<div class="card"><div class="empty sm"><div class="glyph">' + icon(page.remoteQ ? "search" : ic, 26, 1.8) + '</div><div class="t">' + (page.remoteQ ? "کسی با این نام پیدا نشد" : "لیست خالی است") + '</div><div class="s">' + (page.remoteQ ? "" : desc || "") + "</div></div></div>";
    } else {
      const removeWord = REMOVE_WORD[kind] || "حذف";
      html += '<div class="list">' + d.entries.map((e) => {
        const pending = S.pending === kind + ":" + e.key;
        const hide = !remote && q && !fold(decode(entryName(kind, e).replace(/<[^>]+>/g, "")) + " " + decode(e.name)).includes(q);
        const plain = decode(e.name);
        const body = kind === "answer"
          ? '<button class="rt" data-answer-edit="' + esc(e.key) + '" aria-label="' + esc("ویرایش پاسخ " + plain) + '"><span class="t">' + bdi(e.name) + '</span><span class="s">' + (e.preview ? bdi(e.preview) : e.media ? "فقط رسانه" : "") + "</span></button>" + (e.media ? '<span class="pill mute">' + icon("image", 12) + " رسانه</span>" : "")
          : '<span class="rt"><span class="t n">' + entryName(kind, e) + "</span></span>";
        return '<div class="row"' + (hide ? " hidden" : "") + '><span class="rico sm">' + icon(ic, 15) + "</span>" + body +
          (pending
            ? '<button class="btn sm danger" data-remove="' + kind + '" data-key="' + esc(e.key) + '">' + removeWord + "؟</button>"
            : '<button class="ibtn danger" data-remove="' + kind + '" data-key="' + esc(e.key) + '" aria-label="' + esc(removeWord + " " + plain) + '" title="' + removeWord + '">' + icon("x", 18) + "</button>") +
          "</div>";
      }).join("") + "</div>";
      if (d.truncated && !page.remoteQ) html += '<div class="hint">فقط ' + fa(d.entries.length) + " مورد اول نمایش داده می شود؛ برای پیدا کردن بقیه، نام را جستجو کنید.</div>";
    }
    return html + "</div></div>";
  }

  function searchListRemote(page, kind, value) {
    clearTimeout(searchListRemote.timer);
    searchListRemote.timer = setTimeout(async function run(retried) {
      const q = String(value || "").trim().slice(0, 64);
      if (currentPage() !== page) return;
      try {
        const d = await api("/lists/" + kind + (q ? "?q=" + encodeURIComponent(q) : ""));
        if (currentPage() !== page || String(S.listQuery || "").trim().slice(0, 64) !== q) return;
        page.data = d;
        page.remoteQ = q;
        if (!q) S.lists[kind] = d;
        render();
      } catch (e) {
        if (e && e.status === 429 && retried !== true) searchListRemote.timer = setTimeout(() => run(true), 1100);
        else report(e);
      }
    }, 1100);
  }

  function openSheet(fid) {
    const f = feature(fid) || EXTRA_SHEETS[fid];
    if (!f) return;
    if (f.page) {
      pushPage(f.page);
      return;
    }
    S.sheetReturn = document.activeElement && app.contains(document.activeElement) ? focusTarget(document.activeElement) : null;
    S.sheet = { id: fid, data: null, fresh: true };
    S.confirm = null;
    S.palette = null;
    render();
    if (f.load) loadSheetData(f.load);
  }

  function openSheetAnimation() {
    const sheetEl = document.getElementById("sheet");
    const dim = document.getElementById("dim");
    S.sheet.fresh = false;
    if (!sheetEl) return;
    void sheetEl.offsetHeight;
    requestAnimationFrame(() => {
      dim.classList.add("open");
      sheetEl.classList.add("open");
      const close = WIDE.matches && sheetEl.querySelector(".sheet-close");
      (close || sheetEl).focus({ preventScroll: true });
    });
  }

  async function loadSheetData(path) {
    const sheet = S.sheet;
    try {
      const d = await api(path);
      if (S.sheet !== sheet) return;
      sheet.data = d;
      if (path === "/locks") S.lockCache = d;
      if (path === "/slow") S.slow = d;
      drawSheet();
    } catch (e) {
      if (S.sheet === sheet) {
        sheet.data = "error";
        drawSheet();
      }
      report(e);
    }
  }

  function closeSheet() {
    flushField();
    S.sheet = null;
    S.pending = null;
    const dim = document.getElementById("dim");
    const sheet = document.getElementById("sheet");
    if (dim) dim.classList.remove("open");
    if (sheet) {
      sheet.classList.remove("open");
      sheet.style.transform = "";
      setTimeout(() => {
        if (S.sheet || !sheet.isConnected) return;
        if (S.dash && !S.palette && !S.confirm) {
          render();
          if (S.sheetReturn) restoreFocus(S.sheetReturn);
        } else sheet.innerHTML = "";
        S.sheetReturn = null;
      }, 240);
    }
    syncBack();
  }

  const EXTRA_SHEETS = {
    lim: { id: "lim", icon: "userCog", title: "محدودیت مدیران", desc: "با روشن بودن، هر ادمین فقط کارهایی را می تواند بکند که اینجا مجاز است", flag: "lim_on", sections: ["lim"], custom: "lim", load: "/limits",
      effect: () => "هر ادمین فقط کارهای مجازش" },
    nt: { id: "nt", icon: "bell", title: "اعلان حذف", desc: "وقتی پیامی حذف شد، به فرستنده اش گفته می شود چرا", flag: "nt_on", sections: ["nt"],
      effect: () => (setting("nt_t") && setting("nt_t").value === 0 ? "اعلان پاک نمی شود" : "اعلان پس از " + shown("nt_t") + " ثانیه پاک می شود") },
    "response-policy": { id: "response-policy", icon: "chat", title: "دیده شدن پیام های ربات", desc: "فقط اعلان های حذف و خوشامد؛ بقیه پیام ها عادی می مانند", custom: "response_policy", load: "/response-policy", neutral: true },
    "rights-help": { id: "rights-help", icon: "shieldCheck", title: "دسترسی های ربات", desc: "ربات بدون این دسترسی ها نمی تواند گروه را محافظت کند", custom: "rights_help", neutral: true },
    help: { id: "help", icon: "book", title: "دستورهای داخل گروه", desc: "این کارها به یک پیام یا کاربر در گروه نیاز دارند", custom: "help", neutral: true },
  };

  function drawSheet() {
    const sheetEl = document.getElementById("sheet");
    if (!sheetEl || !S.sheet) return;
    const y = sheetEl.scrollTop;
    sheetEl.innerHTML = sheetHtml();
    sheetEl.scrollTop = y;
  }

  function sheetHtml() {
    if (S.sheet.id === "picker") return pickerHtml();
    const f = feature(S.sheet.id) || EXTRA_SHEETS[S.sheet.id];
    return f ? renderSheetBody(f) : "";
  }

  function masterControl(f) {
    if (f.always || f.neutral) return "";
    if (f.id === "lim" && !S.dash.viewer.is_owner) return '<span class="pill ' + (isOn("lim_on") ? "ok" : "mute") + '">' + (isOn("lim_on") ? "روشن" : "خاموش") + "</span>";
    if (f.flag) return swb('data-apply="' + f.flag + '"', isOn(f.flag), f.title);
    if (f.enabled) return swb('data-power="' + f.id + '"', sectionEnabled(f.enabled), f.title);
    return "";
  }

  function sheetError() {
    return '<div class="card" style="margin-top:12px"><div class="empty sm"><div class="glyph">' + icon("wifiOff", 24) + '</div><div class="t">این بخش خوانده نشد</div><div class="s" style="margin-bottom:14px">اتصال را بررسی کنید و دوباره امتحان کنید.</div><button class="btn sm ghost" data-sheet-retry>' + icon("refresh", 14) + " تلاش دوباره</button></div></div>";
  }

  function sheetLoading(h) {
    return '<div class="sk" style="height:' + (h || 120) + 'px;border-radius:18px;margin-top:12px"></div>';
  }

  function renderSheetBody(f) {
    const st = featureState(f);
    let html = '<div class="handle" aria-hidden="true"></div><div class="sheet-head"><span class="rico' + (st ? " on" : "") + '">' + icon(f.icon, 22) + '</span><div style="flex:1;min-width:0"><div class="t" id="sheet-t">' + f.title + '</div><div class="s">' + f.desc + "</div></div>" + masterControl(f) +
      '<button class="ibtn sheet-close" data-close aria-label="بستن">' + icon("x", 20) + "</button></div>";

    const shadowed = f.id === "cq" ? [["cq_shadow", "موضوع ها"], ["ad_shadow", "تبلیغ"], ["tr_shadow", "خرید و فروش"]].filter(([id]) => isOn(id)).map(([, l]) => l) : [];
    if (st === true && f.effect && !f.always) {
      html += '<div class="status-line on"><span class="dot on"></span><span>روشن · ' + esc(f.effect()) + (shadowed.length ? " · فقط بررسی: " + shadowed.join("، ") : "") + "</span></div>";
    } else if (st === true && f.effect) {
      html += '<div class="status-line"><span class="dot"></span><span>' + esc(f.effect()) + "</span></div>";
    } else if (st === false && f.enabled) {
      html += '<div class="status-line"><span class="dot"></span><span>خاموش · با انتخاب یک ساعت یا کلید بالا روشن می شود.</span></div>';
    } else if (st === false && f.flags) {
      html += '<div class="status-line"><span class="dot"></span><span>خاموش · برای روشن کردن، یکی از کلیدهای زیر را بزنید.</span></div>';
    } else if (st === false && f.flag) {
      html += '<div class="status-line"><span class="dot"></span><span>خاموش · تنظیمات زیر با روشن کردن کلید بالا اعمال می شود.</span></div>';
    }

    const adv = [];
    let fields = "";
    (f.sections || []).forEach((sid) => {
      const s = section(sid);
      if (!s) return;
      s.settings.forEach((it) => {
        if (it.id === f.flag) return;
        if (it.kind === "flag") {
          adv.push(it);
          return;
        }
        if (it.kind === "number") fields += renderNumberField(it, false);
        else if (it.kind === "pick") fields += renderPickField(it);
      });
    });
    (f.flags || []).forEach((id) => {
      const it = setting(id);
      if (it) fields += '<div class="row" style="padding-right:0;padding-left:0"><span class="rt"><span class="t n">' + (it.icon ? icon(it.icon, 14) + " " : "") + it.label + "</span></span>" + swb('data-apply="' + it.id + '"', it.on, it.label) + "</div>";
    });

    const custom = SHEET_BLOCKS[f.custom];
    const fieldsHtml = fields ? '<div class="sheet-card">' + fields + "</div>" : "";
    if (f.id === "cq") html += (custom ? custom(f) : "") + fieldsHtml;
    else html += fieldsHtml + (custom ? custom(f) : "");
    if (f.tip) html += '<div class="note mute" style="margin-top:12px">' + icon("info", 16) + "<span>" + f.tip + "</span></div>";
    if (f.warn) html += '<div class="note warn" style="margin-top:12px">' + icon("alert", 16) + "<span>" + f.warn + "</span></div>";

    if (f.id === "cq" && adv.length) {
      html += '<div class="h" style="padding-top:16px"><div class="t">فقط بررسی، بدون حذف<span class="sub">روشن باشد، فقط در کانال لاگ ثبت می شود</span></div></div><div class="list">' +
        adv.map((it) => '<div class="row"><span class="rt"><span class="t n">' + it.label + "</span></span>" + swb('data-apply="' + it.id + '"', it.on, it.label) + "</div>").join("") + "</div>";
      adv.length = 0;
    }

    if (adv.length) {
      const open = S.adv.has(f.id);
      html += '<div class="disclose list"><button class="row" data-adv="' + f.id + '" aria-expanded="' + open + '"><span class="rico sm">' + icon("sliders", 15) + '</span><span class="rt"><span class="t">تنظیمات پیشرفته</span><span class="s">' + adv.map((a) => a.label).join("، ") + '</span></span><span class="chev">' + icon(open ? "chevD" : "chevL", 18) + "</span></button>";
      if (open) {
        html += adv.map((it) =>
          '<div class="row"><span class="rt"><span class="t n">' + (it.icon ? icon(it.icon, 14) + " " : "") + it.label + "</span></span>" + swb('data-apply="' + it.id + '"', it.on, it.label) + "</div>"
        ).join("");
      }
      html += "</div>";
    }
    return html;
  }

  const HELP = [
    ["مدیریت کاربر", [["بن", "بیرون کردن برای همیشه"], ["سکوت ۱ ساعت", "سکوت موقت"], ["رفع سکوت / رفع بن", "برداشتن محدودیت"], ["اخطار", "یک اخطار؛ با سقف، خودکار برخورد می شود"], ["ویژه / معاف", "معاف از قفل ها یا ضد رگبار"]]],
    ["ادمین ها", [["ترفیع", "ادمین کردن با دسترسی های ربات"], ["عزل ادمین", "برداشتن ادمین"], ["تنظیم تگ ‹عنوان›", "عنوان کنار نام عضو"]]],
    ["پیام ها", [["سنجاق", "سنجاق پیام"], ["حذف ۵۰", "پاک کردن پیام های اخیر"], ["گزارش", "فرستادن پیام به پرونده ها"], ["تاریخچه", "سابقه برخوردهای یک کاربر"]]],
    ["محتوا", [["قفل پک", "روی استیکر: همه استیکرهای آن پک حذف شود"], ["فیلتر این ‹نام›", "روی عکس: فیلتر تصویری از روی نمونه"], ["قفل چهره ‹نام›", "روی عکس چهره: حذف عکس های آن شخص"], ["تنظیم خوشامد", "روی عکس یا پیام: خوشامد با رسانه"]]],
    ["گروه", [["قفل گروه ۲ ساعت", "بستن موقت گروه"], ["آمار", "آمار ماه، ساعت ها و فعال ترین ها"], ["افزودن کلینر", "اکانت کمکی برای پاکسازی کامل و اسلوموشن"], ["پنل", "پنل کامل داخل گروه"]]],
  ];

  const SHEET_BLOCKS = {
    ai: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading();
      if (d === "error") return sheetError();
      return '<div class="h" style="padding-top:16px"><div class="t">موضوع ها <span class="count">· ' + fa(d.ai.filter((l) => l.on).length) + " از " + fa(d.ai.length) + '</span></div></div><div class="lockgrid">' +
        d.ai.map(lockTile).join("") + "</div>" +
        '<div class="list" style="margin-top:12px"><button class="row" data-push="imgf"><span class="rico">' + icon("image", 17) + '</span><span class="rt"><span class="t">فیلتر تصویری شما</span><span class="s">موضوع های دلخواه</span></span>' + CHEV + "</button></div>";
    },
    response_policy: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(180);
      if (d === "error") return sheetError();
      let html = '<div class="note" style="margin-top:4px">' + icon("info", 16) + "<span>عمومی یعنی پیام عادی گروه؛ خصوصی فقط به همان کاربر نشان داده می شود.</span></div><div class=\"sheet-card\">";
      html += d.overrides.map((item) => '<div class="field"><div class="fl"><span>' + esc(item.label) + "</span><b>" + policyVisibilityLabel(item.visibility) + '</b></div><div class="seg">' +
        policyChoice("kind", item.id, "public", item.visibility === "public") +
        policyChoice("kind", item.id, "private", item.visibility === "private") +
        "</div></div>").join("");
      html += '</div><button class="btn ghost block" style="margin-top:12px" data-policy-reset>بازگشت هر دو به عمومی</button>';
      return html;
    },
    rights_help: () => {
      const h = S.health && S.health !== "error" ? S.health : null;
      const missing = h ? h.issues.filter((i) => i.fix === "rights") : [];
      return (missing.length ? '<div class="list" style="margin-bottom:12px">' + missing.map((i) => '<div class="row"><span class="rico bad sm">' + icon("x", 14, 3) + '</span><span class="rt"><span class="t wrap">' + esc(i.title) + '</span><span class="s wrap">' + esc(i.detail) + "</span></span></div>").join("") + "</div>" : h ? '<div class="status-line on" style="margin-bottom:12px"><span class="dot on"></span><span>ربات همه دسترسی های لازم را دارد.</span></div>' : "") +
        '<div class="card pad"><ol class="steps"><li>در تلگرام وارد تنظیمات گروه شوید.</li><li>بخش «ادمین ها» را باز کنید و ربات را انتخاب کنید.</li><li>همه دسترسی ها را روشن کنید، به خصوص حذف پیام، بن کاربران و افزودن ادمین.</li><li>ذخیره کنید و به اینجا برگردید.</li></ol></div>' +
        '<button class="btn block" style="margin-top:14px" data-recheck>' + icon("refresh", 16) + " بررسی دوباره</button>";
    },
    voice: () => '<div class="list" style="margin-top:12px"><button class="row" data-push="voice"><span class="rico">' + icon("list", 17) + '</span><span class="rt"><span class="t">کلمه های نامناسب</span><span class="s">فهرست را ببینید و کم و زیاد کنید</span></span>' + CHEV + "</button></div>",
    answers: () => '<div class="list" style="margin-top:12px"><button class="row" data-push="list:answer"><span class="rico">' + icon("chat", 17) + '</span><span class="rt"><span class="t">کلمه ها و پاسخ ها</span><span class="s">ببینید، اضافه یا حذف کنید</span></span>' + CHEV + "</button></div>",
    report: () => '<button class="btn outline block" style="margin-top:12px" data-report-test>' + icon("send", 16) + " ارسال آزمایشی گزارش امروز در گروه</button>" +
      '<div class="hint">گزارش آزمایشی برای همه اعضای گروه فرستاده می شود.</div>',
    strict: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(110);
      if (d === "error") return sheetError();
      if (!d.causes.length) return '<div class="note mute" style="margin-top:12px">' + icon("info", 16) + "<span>هیچ قفل یا فیلتری روشن نیست؛ اول یکی را روشن کنید تا اینجا بیاید.</span></div>" +
        '<button class="btn ghost block" style="margin-top:10px" data-push="locks">' + icon("lock", 16) + " رفتن به قفل ها</button>";
      return '<div class="sheet-card"><div class="field"><div class="fl"><span>چه چیزهایی تخلف حساب شود</span><b>' + fa(d.causes.filter((c) => c.on).length) + " از " + fa(d.causes.length) + '</b></div><div class="chips">' +
        d.causes.map((c) => '<button class="chip' + (c.on ? " on" : "") + '" data-cause="' + esc(c.key) + '" aria-pressed="' + c.on + '">' + (c.on ? icon("check", 13, 3) : "") + esc(c.label) + "</button>").join("") +
        '</div><div class="hint start">فقط موارد انتخاب شده به شمارش تخلف اضافه می شوند؛ دست کم یکی همیشه می ماند.</div></div></div>';
    },
    tmed: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(90);
      if (d === "error") return sheetError();
      return '<div class="sheet-card"><div class="field"><div class="fl"><span>کدام رسانه ها موقت باشند</span><b>' + fa(d.kinds.filter((k) => k.on).length) + " از " + fa(d.kinds.length) + '</b></div><div class="chips">' +
        d.kinds.map((k) => '<button class="chip' + (k.on ? " on" : "") + '" data-tmed-kind="' + esc(k.name) + '" aria-pressed="' + k.on + '">' + (k.on ? icon("check", 13, 3) : "") + esc(k.label) + "</button>").join("") + "</div></div></div>";
    },
    lim: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(220);
      if (d === "error") return sheetError();
      return '<div class="h" style="padding-top:16px"><div class="t">ادمین ها مجازند <span class="count">· ' + fa(d.caps.filter((c) => c.allowed).length) + " از " + fa(d.caps.length) + "</span></div></div>" +
        (isOn("lim_on") ? "" : '<div class="note mute" style="margin-bottom:10px">' + icon("info", 16) + "<span>تا کلید بالا روشن نشود، همه ادمین ها همه کار می توانند بکنند.</span></div>") +
        '<div class="list">' + d.caps.map((c) => '<div class="row"><span class="rt"><span class="t n">' + esc(c.label) + "</span></span>" +
          (d.can_edit ? swb('data-cap="' + esc(c.name) + '"', c.allowed, c.label) : '<span class="pill ' + (c.allowed ? "ok" : "mute") + '">' + (c.allowed ? "مجاز" : "بسته") + "</span>") + "</div>").join("") + "</div>" +
        '<div class="hint start">این محدودیت ها روی همه ادمین ها به جز مالک اعمال می شود.</div>';
    },
    slow: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(110);
      if (d === "error") return sheetError();
      return '<div class="sheet-card"><div class="field"><div class="fl"><span>فاصله بین دو پیام هر عضو</span><b>' + slowLabel(d.seconds) + '</b></div><div class="chips">' +
        d.steps.map((s) => '<button class="chip' + (s.value === d.seconds ? " on" : "") + '" data-slow="' + s.value + '" aria-pressed="' + (s.value === d.seconds) + '">' + fa(s.label) + "</button>").join("") + "</div></div></div>" +
        '<div class="note mute" style="margin-top:12px">' + icon("info", 16) + "<span>اسلوموشن را فقط اکانت کلینر می تواند تنظیم کند؛ اگر کلینر ندارید در گروه «افزودن کلینر» بفرستید.</span></div>";
    },
    help: () => HELP.map(([title, rows]) => '<div class="h" style="padding-top:14px"><div class="t">' + title + '</div></div><div class="list">' +
      rows.map(([cmd, what]) => '<div class="row compact"><code class="cmd">' + esc(cmd) + '</code><span class="rt"><span class="s wrap" style="margin:0">' + esc(what) + "</span></span></div>").join("") + "</div>").join("") +
      '<div class="hint start">بیشتر این دستورها روی پیام کاربر ریپلای می شوند. «راهنما» در گروه فهرست کامل را نشان می دهد.</div>',
  };

  function policyVisibilityLabel(value) {
    return { default: "پیش فرض", public: "عمومی", private: "خصوصی" }[value] || value;
  }

  function policyChoice(kind, id, visibility, selected) {
    return '<button class="' + (selected ? "on" : "") + '" data-policy-choice="' + esc(kind + ":" + id + ":" + visibility) + '">' + policyVisibilityLabel(visibility) + "</button>";
  }

  const FIELDS = {
    fl_lim: ["بیش از چند پیام", "پیام"],
    fl_win: ["در چند ثانیه", "ثانیه"],
    bt_lim: ["بیش از چند حذف", "حذف"],
    bt_win: ["در چند دقیقه", "دقیقه"],
    rd_lim: ["بیش از چند عضو تازه", "عضو"],
    rd_win: ["در چند ثانیه", "ثانیه"],
    rd_time: [null, "دقیقه"],
    s_lim: [null, "تخلف"],
    s_time: [null, "دقیقه"],
    tmed_min: [null, "دقیقه"],
    cp_t: ["مهلت پاسخ", "ثانیه"],
    cp_n: [null, "گزینه"],
    nt_t: [null, "ثانیه"],
    wn_lim: [null, "اخطار"],
    gpe: [null, "ثانیه"],
    gpt: [null, "ثانیه"],
    wct: ["پاک شدن پس از", "ثانیه"],
    ad: [null, "نفر"],
    apc: [null, "پیام"],
    cq_lim: ["آستانه حذف موضوع ها", null, "عدد کمتر یعنی سخت گیرتر: عکس های بیشتری حذف می شود و اشتباه هم بیشتر."],
    tr_lim: ["آستانه حذف خرید و فروش", null, "عدد کمتر یعنی سخت گیرتر: آگهی های بیشتری حذف می شود و اشتباه هم بیشتر."],
  };

  function fieldLabel(it) {
    const f = FIELDS[it.id];
    if (f && f[0]) return f[0];
    return it.clock ? it.label.split("،")[0] : it.label;
  }

  function renderNumberField(it, standalone) {
    const f = FIELDS[it.id] || [];
    const unit = f[1];
    const chips = (it.presets || []).map((p) =>
      '<button class="chip' + (p.value === it.value ? " on" : "") + (it.id === "apc" && p.value === 0 ? " danger" : "") + '" data-apply="' + it.id + ":" + p.value + '" aria-pressed="' + (p.value === it.value) + '">' + fa(p.shown) + "</button>"
    ).join("");
    const custom = it.clock
      ? '<div class="inline-num"' + (chips ? "" : ' style="margin-top:0"') + '><input type="time" class="text-input" dir="ltr" value="' + minutesToClock(it.value) + '" data-clock-apply="' + it.id + '" data-clock-value="' + it.value + '" aria-label="' + esc(fieldLabel(it)) + '" /></div>'
      : "";
    const numChip = it.clock
      ? ""
      : '<input type="number" class="chip-input" inputmode="numeric" enterkeyhint="done" min="' + it.range[0] + '" max="' + it.range[1] + '" value="' + ((it.presets || []).some((p) => p.value === it.value) ? "" : it.value) + '" data-number-apply="' + it.id + '" data-value="' + it.value + '" placeholder="' + (unit ? "دلخواه، " + unit : "دلخواه") + '" aria-label="' + esc("مقدار دلخواه " + fieldLabel(it) + (unit ? " به " + unit : "")) + '" />';
    const plain = /^\d+$/.test(String(it.shown));
    return '<div class="field"' + (standalone ? ' style="padding-top:0"' : "") + '><div class="fl"><span>' + (it.icon ? icon(it.icon, 14) + " " : "") + fieldLabel(it) + "</span><b>" + fa(it.shown) + (plain && unit ? " " + unit : "") + "</b></div>" +
      '<div class="chips">' + chips + numChip + "</div>" + custom + (f[2] ? '<div class="hint start">' + f[2] + "</div>" : "") + "</div>";
  }

  function renderPickField(it) {
    return '<div class="field"><div class="fl"><span>' + (it.icon ? icon(it.icon, 14) + " " : "") + it.label + '</span></div><div class="chips">' +
      it.options.map((o) => '<button class="chip' + (o.value === it.chosen ? " on" : "") + (o.danger ? " danger" : "") + '" data-apply="' + o.id + '">' + (o.icon ? icon(o.icon, 14) : "") + o.label + "</button>").join("") + "</div></div>";
  }

  function openPicker() {
    S.sheet = { id: "picker", data: null, fresh: true };
    S.confirm = null;
    S.palette = null;
    if (S.dash) render();
    else {
      app.innerHTML = '<aside id="rail"></aside><div id="view">' + renderTop(true) + '<div id="main" class="nobar">' + pickPrompt() + "</div></div>" + renderOverlays();
      document.body.classList.add("nobar");
      openSheetAnimation();
    }
    syncBack();
  }

  function pickerHtml() {
    const q = fold(S.groupQuery);
    const groups = S.groups || [];
    return '<div class="handle"></div><div class="sheet-head"><span class="rico on">' + icon("users", 22) + '</span><div style="flex:1;min-width:0"><div class="t" id="sheet-t">گروه های شما</div><div class="s">' + fa(groups.length) + " گروه که در آن ادمین هستید</div></div>" +
      '<button class="ibtn sheet-close" data-close aria-label="بستن">' + icon("x", 20) + "</button></div>" +
      (groups.length > 6 ? '<div style="margin-bottom:10px">' + searchBox("group-search", "جستجوی گروه…", S.groupQuery) + "</div>" : "") +
      '<div class="list">' +
      groups.map((g) => {
        const current = g.id === S.chat;
        const hide = q && !fold(decode(g.title)).includes(q);
        const add = !g.known && addToGroupLink();
        return '<button class="row tap' + (g.known ? "" : " gone") + '" ' + (g.known ? 'data-pick-group="' + g.id + '"' : add ? 'data-open-link="' + esc(add) + '"' : "disabled") + (hide ? " hidden" : "") + ' style="min-height:62px">' +
          '<span class="avatar lg" style="' + avatarStyle(g.id) + '">' + esc(initial(g.title)) + "</span>" +
          '<span class="rt"><span class="t">' + bdi(g.title) + '</span><span class="s">' + (g.is_owner ? "مالک" : "ادمین") + (g.known ? "" : " · ربات در این گروه نیست" + (add ? "؛ برای افزودن دوباره بزنید" : "")) + "</span></span>" +
          (current ? '<span class="pill accent">' + icon("check", 12, 3) + " فعلی</span>" : g.known ? CHEV : '<span class="pill warn">' + icon("plus", 11, 2.6) + " افزودن</span>") +
          "</button>";
      }).join("") +
      "</div>" +
      (addToGroupLink()
        ? '<div class="list" style="margin-top:12px"><button class="row" data-open-link="' + esc(addToGroupLink()) + '"><span class="rico on">' + icon("plus", 18, 2.4) + '</span><span class="rt"><span class="t">افزودن به گروه دیگر</span><span class="s">ادمینش کنید تا اینجا اضافه شود</span></span>' + CHEV + "</button></div>"
        : '<div class="note mute" style="margin-top:12px">' + icon("info", 16) + "<span>گروه دیگری دارید؟ ربات را در آن ادمین کنید تا اینجا اضافه شود.</span></div>");
  }

  const PALETTE_PAGES = [
    ["locks", "lock", "قفل ها", "همه قفل های محتوا"],
    ["rights", "chat", "دسترسی اعضا", "بستن و باز کردن گروه"],
    ["welcome", "hand", "پیام خوشامد", "متن پیام خوشامد"],
    ["rules", "scroll", "قوانین گروه", "متن قوانین"],
    ["joingate", "door", "عضویت اجباری در کانال", "تا عضو کانال نشوند پیام نمی دهند"],
    ["log", "fileText", "کانال لاگ", "کانال لاگ و رویدادها"],
    ["cases", "inbox", "پرونده ها", "گزارش ها و برخوردها"],
    ["imgf", "image", "فیلتر تصویری", "موضوع های دلخواه"],
    ["voice", "mic", "کلمه های نامناسب ویس", "فهرست کلمات"],
  ];

  function paletteItems() {
    const items = [];
    const featureFlags = new Set();
    FEATURES.forEach((f) => {
      if (f.flag) featureFlags.add(f.flag);
      const st = featureState(f);
      items.push({ group: "امکانات", icon: f.icon, title: f.title, sub: st === false ? "خاموش" : f.desc, go: () => openSheet(f.id), control: featureSwitch(f), on: st });
    });
    PALETTE_PAGES.forEach(([id, ic, title, sub]) => items.push({ group: "صفحه ها", icon: ic, title, sub, go: () => pushRoot(id) }));
    Object.keys(EXTRA_SHEETS).forEach((id) => {
      const f = EXTRA_SHEETS[id];
      if (id === "lim" && !S.dash.viewer.is_owner) return;
      items.push({ group: "صفحه ها", icon: f.icon, title: f.title, sub: f.desc, go: () => openSheet(id) });
    });
    Object.keys(LIST_KINDS).forEach((kind) => {
      const [title, ic, desc] = LIST_KINDS[kind];
      items.push({ group: "لیست ها", icon: ic, title, sub: desc || "لیست", go: () => pushRoot("list:" + kind) });
    });
    TABS.forEach(([id, ic, label]) => items.push({ group: "صفحه ها", icon: ic, title: label, sub: "زبانه", go: () => leaveTo(() => goTab(id)) }));
    S.dash.sections.forEach((s) => {
      s.settings.forEach((it) => {
        if (it.kind !== "flag" || featureFlags.has(it.id)) return;
        if (it.id === "lim_on" && !S.dash.viewer.is_owner) return;
        items.push({ group: "تنظیمات", icon: it.icon || "sliders", title: it.label, sub: s.label, control: swb('data-apply="' + it.id + '"', it.on, it.label), go: null });
      });
    });
    if (S.lockCache) {
      S.lockCache.plain.concat(S.lockCache.ai).forEach((l) => {
        items.push({ group: "قفل ها", icon: l.icon || "lock", title: "قفل " + l.label, sub: l.on ? "روشن" : "خاموش", control: lockChip(l), go: null });
      });
    }
    return items;
  }

  function paletteResults() {
    const q = fold(S.palette.q);
    let items = paletteItems();
    const seen = new Set();
    items = items.filter((it) => {
      const key = fold(decode(it.title));
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
    if (q) return items.filter((it) => fold(decode(it.title) + " " + decode(it.sub)).includes(q)).slice(0, 40);
    return items.filter((it) => it.group === "صفحه ها").concat(items.filter((it) => it.group === "امکانات"));
  }

  function renderPaletteResults() {
    const results = paletteResults();
    S.palette.results = results;
    if (S.palette.idx >= results.length) S.palette.idx = 0;
    if (!results.length) return '<div class="state">چیزی پیدا نشد.</div>';
    let html = "";
    let last = null;
    results.forEach((it, i) => {
      if (it.group !== last) {
        html += '<div class="grp-h">' + it.group + "</div>";
        last = it.group;
      }
      html += '<div class="row tap' + (i === S.palette.idx ? " act" : "") + '" data-pal-go="' + i + '" role="option"><span class="rico' + (it.on ? " on" : "") + '">' + icon(it.icon, 16) + '</span><span class="rt"><span class="t">' + it.title + '</span><span class="s">' + esc(decode(it.sub)) + "</span></span>" +
        (it.control ? '<span style="display:flex">' + it.control + "</span>" : CHEV) + "</div>";
    });
    return html;
  }

  function renderPalette() {
    if (!S.palette || !S.dash) return "";
    return '<div id="palette" data-pal-close><div class="pal" role="dialog" aria-label="جستجو">' +
      '<label class="search">' + icon("search", 18) + '<input id="palette-input" type="search" autocomplete="off" placeholder="یک امکان، قفل یا تنظیم را جستجو کنید…" value="' + esc(S.palette.q) + '" /></label>' +
      '<div class="res" id="palette-res" role="listbox">' + renderPaletteResults() + "</div>" +
      '<div class="foot"><span><kbd>↑</kbd> <kbd>↓</kbd> حرکت</span><span><kbd>Enter</kbd> باز کردن</span><span><kbd>Esc</kbd> بستن</span></div></div></div>';
  }

  function openPalette() {
    if (!S.dash || (S.confirm && S.confirm.busy)) return;
    flushField();
    S.palette = { q: "", idx: 0 };
    S.sheet = null;
    S.confirm = null;
    render();
    const input = document.getElementById("palette-input");
    if (input) input.focus();
    if (!S.lockCache) {
      api("/locks").then((d) => {
        S.lockCache = d;
        if (S.palette) drawPalette();
      }).catch(() => {});
    }
  }

  function drawPalette() {
    const res = document.getElementById("palette-res");
    if (res) res.innerHTML = renderPaletteResults();
  }

  function closePalette() {
    S.palette = null;
    const el = document.getElementById("palette");
    if (el) el.remove();
    syncBack();
  }

  function paletteGo(i) {
    const it = S.palette && S.palette.results && S.palette.results[i];
    if (!it) return;
    if (!it.go) {
      const control = document.querySelector('[data-pal-go="' + i + '"] [data-apply], [data-pal-go="' + i + '"] [data-lock], [data-pal-go="' + i + '"] [data-power]');
      if (control) control.click();
      return;
    }
    closePalette();
    it.go();
  }

  function goTab(id) {
    if (S.confirm && S.confirm.busy) return;
    flushField();
    S.tab = id;
    S.stack = [];
    S.barSel = null;
    S.sheet = null;
    S.confirm = null;
    S.palette = null;
    setClosingConfirmation(false);
    if (id === "members" && !S.admins) loadAdmins();
    if ((id === "members" || id === "settings") && !S.joinGate) loadJoinGate();
    render();
    window.scrollTo(0, 0);
  }

  async function loadJoinGate() {
    const chat = S.chat;
    try {
      const d = await api("/join-gate");
      if (S.chat === chat) S.joinGate = d;
    } catch (e) {
      if (S.chat === chat) S.joinGate = "error";
    }
    rerenderIfIdle(["members", "settings"]);
  }

  function thenReload(d) {
    return reloadTopPage().then(() => d);
  }

  function clearDraftAndReload(d) {
    const page = currentPage();
    if (page) page.draft = null;
    return thenReload(d);
  }

  const CONFIRMS = {
    "log-off": () => ({ title: "کانال لاگ قطع شود؟", sub: "رویدادها دیگر جایی ثبت نمی شود؛ هر وقت خواستید دوباره وصل کنید.", label: "قطع", run: () => api("/log/off", { method: "POST" }).then(thenReload) }),
    "welcome-off": () => ({
      title: "پیام خوشامد حذف شود؟", sub: "متن و رسانه اش پاک می شود.", label: "حذف",
      run: () => api("/welcome/off", { method: "POST" }).then(clearDraftAndReload),
    }),
    "rules-off": () => ({
      title: "قوانین حذف شود؟", sub: "«قوانین» در گروه دیگر چیزی نشان نمی دهد.", label: "حذف",
      run: () => api("/rules", { method: "POST", body: { text: "" } }).then(clearDraftAndReload),
    }),
    "voice-clear": () => ({
      title: "همه کلمه های ویس پاک شود؟", sub: "کلمه های سفارشی حذف و پیش فرض ها خاموش می شوند؛ پیش فرض ها با یک دکمه برمی گردند.", label: "پاک کردن همه",
      run: () => api("/voice/clear", { method: "POST" }).then((d) => {
        const page = currentPage();
        if (page && d && d.words) page.data = d;
        return d;
      }),
    }),
  };

  function confirmFor(key) {
    if (CONFIRMS[key]) return CONFIRMS[key]();
    if (key.startsWith("clear:")) {
      const kind = key.slice(6);
      const d = S.lists && S.lists[kind];
      const n = d ? d.entries.length : 0;
      const many = d && d.truncated ? "همه موارد (بیش از " + fa(n) + ")" : "همه " + fa(n) + " مورد";
      return {
        title: many + " از «" + LIST_KINDS[kind][0] + "» حذف شود؟",
        sub: kind === "ban" ? "همه از بن درمی آیند و می توانند دوباره به گروه بیایند. برگشت ندارد."
          : kind === "mute" ? "سکوت همه برداشته می شود و دوباره می توانند پیام بدهند. برگشت ندارد."
          : "برگشت ندارد.",
        label: kind === "ban" ? "رفع بن همه" : kind === "mute" ? "رفع سکوت همه" : "حذف همه",
        run: () => api("/lists/" + kind, { method: "DELETE" }).then(thenReload),
      };
    }
    if (key === "locks-on") {
      return { title: "همه قفل ها روشن شود؟", sub: "متن فارسی و انگلیسی، لینک، عکس و همه چیز دیگر برای اعضای عادی بسته می شود؛ فقط ادمین ها و کاربران ویژه می توانند بفرستند.", label: "روشن کردن همه", quiet: true, run: () => setAllLocks(true).then((sure) => sure && toast("همه قفل ها روشن شد.", "ok")) };
    }
    if (key === "locks-off") {
      return { title: "همه قفل ها خاموش شود؟", sub: "هر چیزی دوباره در گروه فرستاده می شود تا قفل ها را برگردانید.", label: "خاموش کردن همه", quiet: true, run: () => setAllLocks(false).then((sure) => sure && toast("همه قفل ها خاموش شد.", "ok")) };
    }
    if (key.startsWith("lock-group:")) {
      const title = key.slice(11);
      return { title: "همه قفل های «" + title + "» روشن شود؟", sub: "در میان آن ها قفل متن فارسی و انگلیسی هم هست؛ اعضای عادی دیگر نمی توانند چیزی بنویسند.", label: "روشن کردن همه", quiet: true, run: () => lockGroupOn(title) };
    }
    if (key === "rights-close") {
      return { title: "گروه بسته شود؟", sub: "هیچ عضو عادی نمی تواند چیزی بفرستد؛ ادمین ها می توانند. دسترسی هایی که تنظیم کرده اید دست نمی خورد و با باز کردن گروه همان ها برمی گردد.", label: "بستن گروه", quiet: true, run: () => setGroupLock(true) };
    }
    if (key === "rights-close-all") {
      return { title: "همه دسترسی ها بسته شود؟", sub: "هر دسترسی جداگانه بسته می شود. برای بستن موقت گروه «بستن گروه» بهتر است، چون باز کردنش تنظیم ها را برمی گرداند.", label: "بستن همه", quiet: true, run: () => setAllRights(false) };
    }
    if (key === "apc-all") {
      return { title: "هر روز همه پیام های گروه پاک شود؟", sub: "در ساعت پاکسازی، همه تاریخچه گروه حذف می شود، نه فقط پیام های اخیر. پیام حذف شده برنمی گردد.", label: "پاک کردن همه", quiet: true, run: () => applySetting("apc:0") };
    }
    if (key === "report-test") {
      return { tone: "accent", title: "گزارش امروز در گروه فرستاده شود؟", sub: "همه اعضا آن را می بینند.", label: "ارسال", quiet: true, run: sendTestReport };
    }
    if (key === "case-delete") {
      return { title: "پیام گزارش شده حذف شود؟", sub: "پیام از گروه پاک می شود و پرونده بسته می شود. پیام حذف شده برنمی گردد.", label: "حذف پیام", quiet: true, run: () => runCaseAction("delete") };
    }
    if (key === "case-reverse") {
      const page = currentPage();
      const c = page && page.data && page.data.detail;
      const what = c ? caseAction(c.action) : "برخورد";
      return { title: "«" + what + "» لغو شود؟", sub: "محدودیت این کاربر برداشته می شود و در تاریخچه پرونده ثبت می شود.", label: "لغو " + what, quiet: true, run: () => runCaseAction("reverse") };
    }
    if (key === "doc-revert") {
      return { title: "تغییرات کنار گذاشته شود؟", sub: "متن به آخرین نسخه ذخیره شده برمی گردد.", label: "کنار گذاشتن", quiet: true, run: async () => {
        const page = currentPage();
        if (page) page.draft = page.saved;
        setClosingConfirmation(false);
      } };
    }
    return null;
  }

  function lockGroupOn(title) {
    const page = currentPage();
    if (!page || !page.data) return;
    const grp = lockGroups(page.data.plain).find(([t]) => t === title);
    if (grp) return setLocks(grp[1].filter((l) => !l.on).map((l) => l.key), true);
  }

  async function sendTestReport() {
    const ok = await write(() => api("/report/test", { method: "POST" }));
    if (ok) {
      haptic("ok");
      toast("گزارش آزمایشی در گروه فرستاده شد.", "ok");
    }
  }

  async function runCaseAction(action) {
    const page = currentPage();
    if (!page || !page.data || !page.data.detail) return;
    const id = page.data.detail.id;
    return guarded("case:" + id, async () => {
      const note = ((document.getElementById("case-note") || {}).value || "").trim();
      const path = action === "reverse" ? "/cases/" + id + "/reverse" : "/cases/" + id + "/resolve";
      const body = action === "reverse" ? (note ? { note } : {}) : (note ? { action, note } : { action });
      S.caseBusy = action;
      render();
      let d = null;
      const ok = await write(async () => {
        d = await api(path, { method: "POST", body });
        page.data.detail = await api("/cases/" + id);
      });
      S.caseBusy = null;
      page.listStale = true;
      if (ok) S.dropTyped = true;
      render();
      if (!ok) return;
      const msg = String((d && d.message) || "");
      if (msg.startsWith("✓")) {
        haptic("ok");
        toast(msg.replace(/^✓\s*/, ""), "ok");
      } else if (msg) {
        haptic("warn");
        toast(msg, "warn");
      }
      api("/cases?status=open&limit=5").then((c) => { S.openCases = c; }).catch(() => {});
    });
  }

  function confirmAdmin(id) {
    const ad = S.admins && S.admins !== "error" ? S.admins.admins.find((a) => String(a.id) === String(id)) : null;
    const name = ad ? decode(ad.name) : "این ادمین";
    return {
      title: "«" + name + "» عزل شود؟",
      sub: "از ادمینی گروه و ربات برداشته می شود و دیگر به تنظیمات دسترسی ندارد.",
      label: "عزل",
      run: () => api("/admins/" + id, { method: "DELETE" }).then((d) => loadAdmins().then(() => d)),
    };
  }

  function renderConfirm() {
    const c = S.confirm;
    if (!c) return "";
    const accent = c.tone === "accent";
    return '<div id="confirm-dim" data-confirm-cancel></div><div class="confirm' + (accent ? " accent" : "") + '" role="alertdialog" aria-modal="true" aria-labelledby="confirm-t" aria-describedby="confirm-s"><div class="ci">' + icon(accent ? "send" : "alert", 22) + '</div><div class="t" id="confirm-t">' + esc(c.title) + '</div><div class="s" id="confirm-s">' + esc(c.sub) + '</div><div class="b"><button class="btn ghost" data-confirm-cancel' + (c.busy ? " disabled" : "") + '>انصراف</button><button class="btn ' + (accent ? "" : "danger") + (c.busy ? " busy" : "") + '" data-confirm-run' + (c.busy ? " disabled" : "") + ">" + (c.busy ? '<span class="spinner"></span>' : "") + esc(c.label) + "</button></div></div>";
  }

  async function reloadTopPage() {
    const page = currentPage();
    if (page) await loadPage(page);
    else render();
  }

  function describeChange(action) {
    const at = action.indexOf(":");
    const id = at < 0 ? action : action.slice(0, at);
    const val = at < 0 ? null : action.slice(at + 1);
    const it = setting(id);
    if (it && it.kind === "flag") {
      const owner = FEATURES.find((f) => f.flag === id) || Object.values(EXTRA_SHEETS).find((f) => f.flag === id);
      return { msg: (owner ? owner.title : it.label) + (it.on ? " خاموش شد" : " روشن شد"), undo: id };
    }
    if (it && it.kind === "number" && val != null) {
      if (String(it.value) === val) return null;
      const preset = (it.presets || []).find((p) => String(p.value) === val);
      const shownVal = it.clock ? minutesToClock(val) : preset ? preset.shown : val;
      const unit = (FIELDS[id] || [])[1];
      return { msg: fieldLabel(it) + " ← " + fa(shownVal) + (/^\d+$/.test(String(shownVal)) && unit ? " " + unit : ""), undo: id + ":" + it.value };
    }
    for (const s of S.dash.sections) {
      for (const p of s.settings) {
        if (p.kind !== "pick") continue;
        const o = p.options.find((o) => o.id === action);
        if (!o) continue;
        if (o.value === p.chosen) return null;
        const cur = p.options.find((c) => c.value === p.chosen);
        return { msg: p.label + " ← " + o.label, undo: cur ? cur.id : null };
      }
    }
    return null;
  }

  async function guarded(key, run) {
    if (S.busy.has(key)) return false;
    S.busy.add(key);
    try {
      return await run();
    } finally {
      S.busy.delete(key);
    }
  }

  function redraw() {
    if (S.sheet && S.sheet.id !== "picker") {
      const sheetEl = document.getElementById("sheet");
      const keep = sheetEl && sheetEl.classList.contains("open");
      render();
      const again = document.getElementById("sheet");
      if (keep && again) again.classList.add("open");
      return;
    }
    render();
  }

  async function applySetting(action, opts) {
    return guarded("set:" + action, async () => {
      const change = describeChange(action);
      let sure = false;
      const ok = await write(async () => {
        sure = await takeDash(await api("/settings/apply", { method: "POST", body: { action } }));
      });
      if (ok && /^ng[ft]:/.test(action)) loadRights();
      redraw();
      if (!ok) return false;
      haptic("sel");
      if (sure && change && !(opts && opts.quiet)) {
        toast(change.msg, "ok", change.undo ? { label: "برگرداندن", run: () => applySetting(change.undo, { quiet: true }) } : null);
      }
      return true;
    });
  }

  async function applyPolicy(action) {
    if (!S.sheet || S.sheet.id !== "response-policy") return;
    const sheet = S.sheet;
    let d = null;
    const ok = await write(async () => {
      d = await api("/response-policy/apply", { method: "POST", body: action });
    });
    if (!ok || S.sheet !== sheet) return;
    if (unsure(d) || !d || !d.overrides) return loadSheetData("/response-policy");
    sheet.data = d;
    haptic("sel");
    drawSheet();
  }

  async function sheetWrite(path, opts, done) {
    const sheet = S.sheet;
    let d = null;
    const ok = await write(async () => {
      d = await api(path, Object.assign({ method: "POST" }, opts || {}));
    });
    if (ok && unsure(d)) {
      const f = sheet && (feature(sheet.id) || EXTRA_SHEETS[sheet.id]);
      if (f && f.load && S.sheet === sheet) loadSheetData(f.load);
      return false;
    }
    if (ok && sheet && S.sheet === sheet && d) {
      sheet.data = d;
      if (done) done(d);
      haptic("sel");
    }
    if (S.sheet) drawSheet();
    return ok;
  }

  function lockLabel(key) {
    const pools = [S.lockCache, currentPage() && currentPage().id === "locks" ? currentPage().data : null, S.sheet && S.sheet.data];
    for (const d of pools) {
      if (!d || !d.plain) continue;
      const l = d.plain.concat(d.ai || []).find((l) => l.key === key);
      if (l) return l;
    }
    return null;
  }

  function setLockCaches(keys, on) {
    const set = new Set(keys);
    const pools = new Set([S.lockCache, S.sheet && S.sheet.data]);
    const page = currentPage();
    if (page && page.id === "locks") pools.add(page.data);
    pools.forEach((d) => {
      if (!d || !d.plain) return;
      d.plain.concat(d.ai || []).forEach((l) => {
        if (set.has(l.key)) l.on = typeof on === "function" ? on(l) : on;
      });
    });
  }

  async function toggleLock(key, opts) {
    return guarded("lock:" + key, async () => {
      const l = lockLabel(key);
      let sure = false;
      const ok = await write(async () => {
        sure = await takeDash(await api("/locks/" + encodeURIComponent(key) + "/toggle", { method: "POST" }));
      });
      if (!ok) return redraw();
      haptic("sel");
      if (sure && l) setLockCaches([key], !l.on);
      else refreshLocks();
      redraw();
      if (S.palette) drawPalette();
      if (sure && l && !(opts && opts.quiet)) {
        toast("قفل " + decode(l.label) + (l.on ? " روشن شد" : " خاموش شد"), "ok", { label: "برگرداندن", run: () => toggleLock(key, { quiet: true }) });
      }
    });
  }

  async function setLocks(keys, on, opts) {
    if (!keys.length) return;
    return guarded("locks:" + keys.join(","), async () => {
      let sure = false;
      const ok = await write(async () => {
        sure = await takeDash(await api("/locks/set", { method: "POST", body: { keys, on } }));
      });
      if (!ok) return render();
      haptic("ok");
      if (sure) setLockCaches(keys, on);
      else refreshLocks();
      render();
      if (sure && !(opts && opts.quiet)) {
        toast(fa(keys.length) + " قفل " + (on ? "روشن شد" : "خاموش شد"), "ok", { label: "برگرداندن", run: () => setLocks(keys, !on, { quiet: true }) });
      }
    });
  }

  async function setAllLocks(on) {
    let sure = false;
    const ok = await write(async () => {
      sure = await takeDash(await api("/locks/all", { method: "POST", body: { on } }));
    });
    if (!ok) return false;
    haptic("ok");
    S.lockCache = null;
    await reloadTopPage();
    if (!S.lockCache) api("/locks").then((d) => { S.lockCache = d; }).catch(() => {});
    return sure;
  }

  function rightsData() {
    const page = currentPage();
    if (page && page.id === "rights" && page.data) return page.data;
    return S.rights && S.rights !== "error" && S.rights !== "unseeded" ? S.rights : null;
  }

  function takeRights(d) {
    if (!d || !Array.isArray(d.rights)) return false;
    S.rights = d;
    const page = currentPage();
    if (page && page.id === "rights") page.data = d;
    return true;
  }

  async function setGroupLock(on, opts) {
    return guarded("group-lock", async () => {
      S.lockBusy = on ? "closing" : "opening";
      redraw();
      let d = null;
      const ok = await write(async () => {
        d = await api("/rights/lock", { method: "POST", body: { on } });
      });
      S.lockBusy = null;
      const was = groupState();
      const sure = ok && !unsure(d) && takeRights(d);
      if (ok && !sure) {
        if (d && d.pending) toast("ثبت شد؛ کمی بعد در تلگرام اعمال می شود.", "warn");
        await loadRights();
        const page = currentPage();
        if (page && page.id === "rights") await loadPage(page);
      }
      redraw();
      if (!sure) return false;
      haptic("ok");
      const now = groupState();
      if (!(opts && opts.quiet)) {
        const text = on ? "گروه بسته شد؛ فقط ادمین ها می توانند پیام بدهند." : now && now.closed ? "قفل گروه برداشته شد؛ گروه هنوز " + groupStateText(now) + "." : "گروه باز شد.";
        const undoable = on || (was && was.manual && !was.until);
        toast(text, "ok", undoable ? { label: "برگرداندن", run: () => setGroupLock(!on, { quiet: true }) } : null);
      }
      return true;
    });
  }

  async function setAllRights(open) {
    const data = rightsData();
    if (!data) return;
    return guarded("rights-all", async () => {
      let state = data.rights;
      const total = state.filter((r) => r.open !== open).length;
      let done = 0;
      let queued = false;
      let failed = false;
      S.rightsBusy = true;
      render();
      for (let i = 0; i < 32; i++) {
        const r = state.find((x) => x.open !== open);
        if (!r) break;
        let d = null;
        const ok = await write(async () => {
          d = await api("/rights/" + encodeURIComponent(r.key) + "/toggle", { method: "POST" });
        });
        if (!ok) {
          failed = true;
          break;
        }
        done++;
        if (d && Array.isArray(d.rights)) state = d.rights;
        else {
          queued = true;
          break;
        }
      }
      S.rightsBusy = false;
      await loadRights();
      const page = currentPage();
      if (page && page.id === "rights") await loadPage(page);
      else render();
      if (failed && !done) return;
      haptic(failed ? "warn" : "ok");
      if (failed) toast(fa(done) + " از " + fa(total) + " دسترسی " + (open ? "باز" : "بسته") + " شد؛ بقیه انجام نشد.", "warn");
      else if (queued) toast("ثبت شد؛ کمی بعد در تلگرام اعمال می شود.", "warn");
      else toast(open ? "همه دسترسی ها باز شد." : "همه دسترسی ها بسته شد.", "ok");
    });
  }

  async function powerFeature(fid) {
    const f = feature(fid);
    if (!f || !f.enabled) return;
    return guarded("power:" + fid, async () => {
      const wasOn = sectionEnabled(f.enabled);
      const restore = f.onAction();
      let sure = false;
      const ok = await write(async () => {
        const d = wasOn
          ? await api(f.off, { method: "POST" })
          : await api("/settings/apply", { method: "POST", body: { action: restore } });
        sure = await takeDash(d);
      });
      if (fid === "ng") loadRights();
      redraw();
      if (!ok) return;
      haptic("sel");
      const undo = wasOn ? () => applySetting(restore, { quiet: true }) : () => powerFeature(fid);
      if (sure) toast(f.title + (wasOn ? " خاموش شد" : " روشن شد"), "ok", { label: "برگرداندن", run: undo });
    });
  }

  async function removeEntry(kind, key) {
    const page = currentPage();
    const entry = page && page.data && page.data.entries ? page.data.entries.find((e) => e.key === key) : null;
    let d = null;
    S.pending = null;
    const ok = await write(async () => {
      d = await api("/lists/" + kind + "/" + encodeURIComponent(key), { method: "DELETE" });
    });
    if (!ok) {
      render();
      return;
    }
    if (unsure(d)) return reloadTopPage();
    haptic();
    if (kind === "imgf") {
      toast("موضوع حذف شد.", "ok");
      return reloadTopPage();
    }
    if (page && page.data) {
      page.data.entries = page.data.entries.filter((e) => e.key !== key);
      if (S.lists && !page.remoteQ) S.lists[kind] = page.data;
    }
    render();
    if (kind === "filter" && entry) {
      const word = decode(entry.name);
      toast("«" + word + "» حذف شد", "ok", { label: "برگرداندن", run: () => addFilterWord(word) });
    } else {
      const who = entry ? "«" + decode(entry.name) + "» " : "";
      toast(kind === "ban" ? who + "از بن درآمد." : kind === "mute" ? "سکوت " + who + "برداشته شد." : who + "از لیست حذف شد.", "ok");
    }
  }

  async function addFilterWord(word) {
    word = String(word || "").trim();
    if (!word) return false;
    let d = null;
    const ok = await write(async () => {
      d = await api("/filter-words", { method: "POST", body: { word } });
    });
    if (!ok) return false;
    haptic("ok");
    S.dropTyped = true;
    if (d && d.accepted && d.error) toast(d.error, "warn");
    else toast(d && d.added === false ? "«" + word + "» از قبل در لیست بود." : "«" + word + "» اضافه شد.", "ok");
    await reloadTopPage();
    return true;
  }

  function refreshLocks() {
    const chat = S.chat;
    api("/locks").then((d) => {
      if (S.chat !== chat) return;
      S.lockCache = d;
      const page = currentPage();
      if (page && page.id === "locks" && page.data) {
        d.botOptions = page.data.botOptions;
        d.allowedBots = page.data.allowedBots;
        page.data = d;
      }
      redraw();
    }).catch(() => {});
  }

  const TEXT_LOCKS = ["persian", "english"];

  async function setSlow(seconds) {
    const ok = await write(async () => {
      const d = await api("/slow", { method: "POST", body: { seconds } });
      if (d && typeof d.seconds === "number") {
        S.slow = d;
        if (S.sheet && S.sheet.id === "sl") S.sheet.data = d;
      }
    });
    if (ok) haptic("sel");
    redraw();
  }

  async function allowBot(username) {
    const page = currentPage();
    let d = null;
    const ok = await write(async () => {
      d = await api("/allowed-bots", { method: "POST", body: { username } });
      if (page && page.data && d && d.bots) page.data.allowedBots = d;
    });
    if (ok) {
      if (!unsure(d)) {
        haptic("ok");
        S.dropTyped = true;
        toast(username + " مجاز شد.", "ok");
      }
      render();
    }
  }

  async function addVoiceWord(word) {
    const page = currentPage();
    const ok = await write(async () => {
      const d = await api("/voice/words", { method: "POST", body: { word } });
      if (page && page.id === "voice" && d && d.words) page.data = d;
    });
    if (ok) {
      haptic("ok");
      S.dropTyped = true;
      render();
    }
    return ok;
  }

  async function addAnswer(trigger, text) {
    return guarded("answer-add", async () => {
      let d = null;
      const ok = await write(async () => {
        d = await api("/answers", { method: "POST", body: { trigger, text } });
      });
      if (!ok) return;
      haptic("ok");
      S.dropTyped = true;
      toast(d && d.accepted && d.error ? d.error : "پاسخ «" + decode((d && d.trigger) || trigger.trim()) + "» ذخیره شد.", d && d.accepted ? "warn" : "ok");
      const t = document.getElementById("answer-trigger");
      const b = document.getElementById("answer-text");
      if (t) t.value = "";
      if (b) b.value = "";
      await reloadTopPage();
    });
  }

  async function toggleImageFilter(key, el) {
    const page = currentPage();
    const entry = page && page.data ? page.data.entries.find((x) => x.key === key) : null;
    const wasLive = entry ? !entry.name.endsWith(" ~") : null;
    const label = entry ? decode(wasLive ? entry.name : entry.name.slice(0, -2)) : "";
    return guarded("imgf:" + key, async () => {
      if (el) optimistic(el);
      const ok = await write(() => api("/filters/" + encodeURIComponent(key) + "/toggle", { method: "POST" }));
      await reloadTopPage();
      if (!ok || wasLive == null) return;
      haptic("sel");
      toast("«" + label + "» " + (wasLive ? "فقط بررسی می کند؛ عکسی حذف نمی شود." : "فعال شد؛ عکس های نزدیک به آن حذف می شود."), "ok",
        { label: "برگرداندن", run: () => toggleImageFilter(key) });
    });
  }

  async function loadCases(page, status, user) {
    return guarded("cases-load", async () => {
      S.casesLoading = status;
      render();
      await write(async () => {
        const data = await api("/cases?status=" + status + (user ? "&user_id=" + encodeURIComponent(user) : ""));
        data.status = status;
        data.user = user || "";
        page.data = data;
      });
      S.casesLoading = null;
      page.listStale = false;
      render();
    });
  }

  function searchCases(page, value) {
    const user = String(value || "").trim().replace(/[۰-۹]/g, (d) => String(FA_DIGITS.indexOf(d)));
    if (user && !/^-?\d+$/.test(user)) return toast("شناسه کاربر فقط عدد است، مثل 123456789.", "warn");
    if (!page || !page.data) return;
    return loadCases(page, page.data.status || "open", user);
  }

  function leaveTo(next) {
    if (S.confirm && S.confirm.busy) return;
    flushField();
    const page = S.stack.find((p) => DOCS[p.id] && docDirty(p));
    if (page) {
      S.palette = null;
      S.confirm = {
        title: "تغییرات ذخیره نشده کنار گذاشته شود؟",
        sub: "متنی که نوشته اید ذخیره نشده است.",
        label: "کنار گذاشتن",
        quiet: true,
        run: async () => {
          page.draft = page.saved;
          setClosingConfirmation(false);
          next();
        },
      };
      render();
      return;
    }
    next();
  }

  function optimistic(el) {
    if (el.classList.contains("swb")) {
      const sw = el.querySelector(".sw");
      if (sw) sw.classList.toggle("on");
      el.classList.add("busy");
      return;
    }
    if (el.classList.contains("lock")) {
      el.classList.toggle("on");
      el.classList.add("busy");
      return;
    }
    if (el.classList.contains("chip")) {
      if (el.dataset.lock || el.dataset.tmedKind || el.dataset.cause) el.classList.toggle("on");
      else {
        const row = el.parentElement;
        if (row) row.querySelectorAll(".chip").forEach((c) => c.classList.remove("on"));
        el.classList.add("on");
      }
      el.classList.add("busy");
      return;
    }
    if (el.closest(".quick")) el.classList.toggle("on");
  }

  async function copyText(text) {
    try {
      await navigator.clipboard.writeText(text);
      toast("کپی شد.", "ok");
    } catch (e) {
      toast("کپی نشد؛ دستی انتخاب کنید.", "warn");
    }
  }

  app.addEventListener("click", onClick);
  app.addEventListener("keydown", onKeydown);
  app.addEventListener("focusout", onFocusOut);
  app.addEventListener("input", onInput);
  document.addEventListener("keydown", onGlobalKey);

  async function onClick(e) {
    const t = e.target;
    const hit = (sel) => t.closest(sel);
    let el;

    if (hit("[data-reboot]")) return start();
    if ((el = hit("[data-site-theme]"))) {
      store(THEME_KEY, el.dataset.siteTheme);
      applyTheme();
      const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
      el.dataset.siteTheme = next;
      el.innerHTML = icon(next === "light" ? "sun" : "moon", 18);
      return;
    }
    if ((el = hit("[data-open-link]"))) return openLink(el.dataset.openLink);
    if (hit("[data-retry-chat]") && S.chat != null) return loadChat(S.chat);

    if (S.palette) {
      if (t.id === "palette") return closePalette();
      if ((el = hit("[data-pal-go]")) && !hit("[data-apply], [data-lock], [data-power]")) return paletteGo(Number(el.dataset.palGo));
    }

    if ((el = hit("[data-apply]"))) {
      if (S.busy.has("set:" + el.dataset.apply)) return;
      if (el.dataset.apply === "apc:0" && setting("apc") && setting("apc").value !== 0) {
        S.confirm = confirmFor("apc-all");
        return render();
      }
      optimistic(el);
      await applySetting(el.dataset.apply);
      if (S.palette) drawPalette();
      return;
    }
    if ((el = hit("[data-power]"))) {
      optimistic(el);
      return powerFeature(el.dataset.power);
    }
    if ((el = hit("[data-lock]"))) {
      if (S.busy.has("lock:" + el.dataset.lock)) return;
      optimistic(el);
      return toggleLock(el.dataset.lock);
    }

    if ((el = hit("[data-tab]"))) {
      const id = el.dataset.tab;
      return leaveTo(() => goTab(id));
    }
    if (hit("[data-back]")) return back();
    if (hit("[data-close]")) {
      if (S.sheet) closeSheet();
      return;
    }
    if (hit("[data-palette]")) return openPalette();
    if (hit("[data-refresh]")) return refreshAll();
    if (hit("[data-picker]")) return openPicker();
    if ((el = hit("[data-pick-group]"))) {
      const id = Number(el.dataset.pickGroup);
      if (id === S.chat) return closeSheet();
      return leaveTo(() => {
        S.sheet = null;
        loadChat(id);
      });
    }
    if ((el = hit("[data-push-root]"))) return pushRoot(el.dataset.pushRoot);
    if ((el = hit("[data-push]"))) return pushPage(el.dataset.push);
    if ((el = hit("[data-open]"))) return openSheet(el.dataset.open);
    if ((el = hit("[data-fix]"))) return fixIssue(el.dataset.fix);
    if ((el = hit("[data-quick]"))) return quick(el.dataset.quick);
    if ((el = hit("[data-check]"))) {
      const c = checks().filter((x) => x.done !== null)[Number(el.dataset.check)];
      if (c && !c.done) c.fix();
      return;
    }
    if ((el = hit("[data-scroll]"))) {
      const target = document.getElementById(el.dataset.scroll);
      if (target) target.scrollIntoView({ behavior: "smooth", block: "start" });
      return;
    }
    if ((el = hit("[data-kpi]"))) {
      S.metric = el.dataset.kpi;
      return goTab("reports");
    }
    if (hit("[data-recheck]")) {
      if (S.sheet) closeSheet();
      S.health = null;
      render();
      await loadHealth();
      if (S.health && S.health !== "error") toast(S.health.issues.length ? fa(S.health.issues.length) + " مورد هنوز مانده." : "همه چیز درست است.", S.health.issues.length ? "warn" : "ok");
      return;
    }
    if (hit("[data-reload-cases]")) {
      S.openCases = null;
      render();
      api("/cases?status=open&limit=5").then((d) => { S.openCases = d; }, (e) => { S.openCases = e && e.status === 403 ? "denied" : "error"; }).then(() => rerenderIfIdle());
      return;
    }
    if (hit("[data-reload-activity]")) {
      S.activity = null;
      render();
      loadActivity();
      return;
    }
    if (hit("[data-reload-admins]")) {
      S.admins = null;
      render();
      return loadAdmins();
    }
    if (hit("[data-sheet-retry]")) {
      const f = S.sheet && (feature(S.sheet.id) || EXTRA_SHEETS[S.sheet.id]);
      if (f && f.load) {
        S.sheet.data = null;
        drawSheet();
        loadSheetData(f.load);
      }
      return;
    }
    if (hit("[data-retry-page]")) {
      const page = currentPage();
      if (page) {
        page.error = null;
        page.data = null;
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
    if ((el = hit("[data-metric]"))) {
      S.metric = el.dataset.metric;
      render();
      return;
    }
    if ((el = hit("[data-bar]"))) {
      S.barSel = Number(el.dataset.bar);
      haptic("sel");
      render();
      return;
    }
    if ((el = hit("[data-feat-cat]"))) {
      S.featCat = el.dataset.featCat;
      haptic("sel");
      render();
      return;
    }
    if ((el = hit("[data-lock-view]"))) {
      S.lockView = el.dataset.lockView;
      render();
      return;
    }
    if ((el = hit("[data-lock-group]"))) {
      const page = currentPage();
      if (!page || !page.data) return;
      const grp = lockGroups(page.data.plain).find(([title]) => title === el.dataset.lockGroup);
      if (!grp) return;
      const on = el.dataset.on === "1";
      if (on && grp[1].some((l) => !l.on && TEXT_LOCKS.includes(l.key))) {
        S.confirm = confirmFor("lock-group:" + grp[0]);
        return render();
      }
      return setLocks(grp[1].filter((l) => l.on !== on).map((l) => l.key), on);
    }
    if ((el = hit("[data-theme-set]"))) {
      store(THEME_KEY, el.dataset.themeSet);
      applyTheme();
      haptic("sel");
      render();
      return;
    }
    if ((el = hit("[data-copy]"))) return copyText(el.dataset.copy);
    if ((el = hit("[data-policy-choice]"))) {
      const parts = el.dataset.policyChoice.split(":");
      if (parts.length !== 3) return;
      if (parts[0] !== "kind") return;
      return applyPolicy({ action: "set_kind", kind: parts[1], visibility: parts[2] });
    }
    if (hit("[data-policy-reset]")) {
      return applyPolicy({ action: "reset" });
    }
    if ((el = hit("[data-tmed-kind]"))) {
      const path = "/tempmedia/" + encodeURIComponent(el.dataset.tmedKind) + "/toggle";
      if (S.busy.has(path)) return;
      optimistic(el);
      return guarded(path, () => sheetWrite(path));
    }
    if ((el = hit("[data-cause]"))) {
      const path = "/strict-causes/" + encodeURIComponent(el.dataset.cause) + "/toggle";
      if (S.busy.has(path)) return;
      optimistic(el);
      return guarded(path, () => sheetWrite(path));
    }
    if ((el = hit("[data-cap]"))) {
      const path = "/limits/" + encodeURIComponent(el.dataset.cap) + "/toggle";
      if (S.busy.has(path)) return;
      optimistic(el);
      return guarded(path, () => sheetWrite(path));
    }
    if ((el = hit("[data-slow]"))) {
      const seconds = Number(el.dataset.slow);
      const before = S.slow && S.slow !== "error" ? S.slow.seconds : null;
      if (seconds === before) return;
      optimistic(el);
      const ok = await sheetWrite("/slow", { body: { seconds } }, (d) => { S.slow = d; });
      if (ok) {
        const undo = before == null ? null : { label: "برگرداندن", run: () => setSlow(before) };
        toast(seconds ? "اسلوموشن: " + slowLabel(seconds) : "اسلوموشن خاموش شد.", "ok", undo);
      }
      return;
    }
    if (hit("[data-report-test]")) {
      S.confirm = confirmFor("report-test");
      return render();
    }
    if (hit("[data-allow-bot]")) {
      const input = document.getElementById("allow-bot");
      const username = input.value.trim();
      if (!username) return input.focus();
      return allowBot(username);
    }
    if ((el = hit("[data-disallow-bot]"))) {
      const page = currentPage();
      const id = el.dataset.disallowBot;
      const was = page && page.data && page.data.allowedBots ? page.data.allowedBots.bots.find((b) => String(b.id) === id) : null;
      const name = was && was.name ? decode(was.name) : "";
      const ok = await write(async () => {
        const d = await api("/allowed-bots/" + encodeURIComponent(id), { method: "DELETE" });
        if (page && page.data && d && d.bots) page.data.allowedBots = d;
      });
      if (ok) {
        haptic();
        const back = /^@?[A-Za-z0-9_]{5,}$/.test(name) ? { label: "برگرداندن", run: () => allowBot(name) } : null;
        toast((name ? name + " " : "") + "از ربات های مجاز حذف شد.", "ok", back);
        render();
      }
      return;
    }
    if ((el = hit("[data-botopt]"))) {
      const key = el.dataset.botopt;
      const page = currentPage();
      optimistic(el);
      const ok = await write(async () => {
        const d = await api("/bot-lock/" + encodeURIComponent(key) + "/toggle", { method: "POST" });
        if (page && page.data && d && d.items) page.data.botOptions = d;
      });
      if (ok) haptic("sel");
      render();
      return;
    }
    if ((el = hit("[data-off]"))) {
      const ok = await write(async () => {
        await takeDash(await api(el.dataset.off, { method: "POST" }));
      });
      if (ok) {
        haptic();
        render();
      }
      return;
    }
    if ((el = hit("[data-locks-all]"))) {
      S.confirm = confirmFor(el.dataset.locksAll === "1" ? "locks-on" : "locks-off");
      render();
      return;
    }
    if ((el = hit("[data-right]"))) {
      const key = el.dataset.right;
      if (S.busy.has("right:" + key)) return;
      optimistic(el);
      const page = currentPage();
      return guarded("right:" + key, async () => {
        let d = null;
        const ok = await write(async () => {
          d = await api("/rights/" + encodeURIComponent(key) + "/toggle", { method: "POST" });
        });
        if (ok && d && d.rights) {
          if (page) page.data = d;
          S.rights = d;
          haptic("sel");
        } else if (ok) {
          toast("ثبت شد؛ کمی بعد در تلگرام اعمال می شود.", "warn");
          if (page) await loadPage(page);
        }
        render();
      });
    }
    if ((el = hit("[data-rights-all]"))) {
      if (S.rightsBusy) return;
      if (el.dataset.rightsAll === "1") return setAllRights(true);
      S.confirm = confirmFor("rights-close-all");
      render();
      return;
    }
    if ((el = hit("[data-group-lock]"))) {
      if (S.lockBusy) return;
      if (el.dataset.groupLock === "0") return setGroupLock(false);
      S.confirm = confirmFor("rights-close");
      render();
      return;
    }
    if ((el = hit("[data-log]"))) {
      const key = el.dataset.log;
      optimistic(el);
      const page = currentPage();
      const ok = await write(async () => {
        const d = await api("/log/" + encodeURIComponent(key) + "/toggle", { method: "POST" });
        if (page && d && d.kinds) {
          page.data = d;
          S.logInfo = d;
        } else if (d && d.error) toast(d.error, "warn");
      });
      if (ok) haptic("sel");
      render();
      return;
    }
    if (hit("[data-log-connect]")) {
      const input = document.getElementById("log-channel");
      const channel = input.value.trim();
      if (!channel) return input.focus();
      el = hit("[data-log-connect]");
      el.disabled = true;
      const page = currentPage();
      let d = null;
      const ok = await write(async () => {
        d = await api("/log/channel", { method: "POST", body: { channel } });
        if (page && d && d.kinds) {
          page.data = d;
          S.logInfo = d;
        }
      });
      el.disabled = false;
      if (ok) {
        if (unsure(d)) {
          if (page) loadPage(page);
        } else {
          haptic("ok");
          S.dropTyped = true;
          toast("کانال لاگ وصل شد.", "ok");
        }
        loadHealth();
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
      S.confirm = confirmAdmin(el.dataset.removeAdmin);
      render();
      return;
    }
    if (hit("[data-confirm-cancel]")) {
      if (S.confirm && S.confirm.busy) return;
      S.confirm = null;
      render();
      return;
    }
    if (hit("[data-confirm-run]")) {
      const c = S.confirm;
      if (!c || c.busy) return;
      c.busy = true;
      render();
      let d = null;
      const ok = await write(async () => {
        d = await c.run();
      });
      if (S.confirm === c) S.confirm = null;
      if (ok) {
        if (unsure(d)) {
          render();
          return;
        }
        if (!c.quiet) {
          haptic("ok");
          toast("انجام شد.", "ok");
        }
      }
      render();
      return;
    }
    if ((el = hit("[data-remove]"))) {
      const kind = el.dataset.remove;
      const key = el.dataset.key;
      const tag = kind + ":" + key;
      if (S.pending === tag) return removeEntry(kind, key);
      S.pending = tag;
      haptic();
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
    if (S.pending && !hit("[data-remove], [data-voice-remove]")) {
      S.pending = null;
      render();
    }

    if ((el = hit("[data-insert]"))) {
      const area = document.getElementById("doc-text");
      const page = currentPage();
      if (!area || !page) return;
      const tag = el.dataset.insert;
      const start = area.selectionStart == null ? area.value.length : area.selectionStart;
      const end = area.selectionEnd == null ? start : area.selectionEnd;
      area.value = area.value.slice(0, start) + tag + area.value.slice(end);
      area.focus();
      area.setSelectionRange(start + tag.length, start + tag.length);
      page.draft = area.value;
      syncDoc(page);
      haptic("sel");
      return;
    }
    if (hit("[data-doc-revert]")) {
      S.confirm = confirmFor("doc-revert");
      return render();
    }
    if (hit("[data-doc-save]")) {
      const page = currentPage();
      if (!page || !DOCS[page.id]) return;
      const text = page.draft || "";
      if (!text.trim() && page.data && (page.data.text || page.data.has_media)) {
        S.confirm = confirmFor(DOCS[page.id].off);
        return render();
      }
      el = hit("[data-doc-save]");
      el.disabled = true;
      let d = null;
      const ok = await write(async () => {
        d = await api(DOCS[page.id].path, { method: "POST", body: { text } });
      });
      el.disabled = false;
      if (ok && !unsure(d)) {
        haptic("ok");
        page.saved = text;
        page.draft = text;
        setClosingConfirmation(false);
        toast("ذخیره شد.", "ok");
        reloadTopPage();
      }
      return;
    }
    if ((el = hit("[data-filter-toggle]"))) {
      return toggleImageFilter(el.dataset.filterToggle, el);
    }
    if (hit("[data-filter-add]")) {
      const input = document.getElementById("filter-phrase");
      const phrase = input.value.trim();
      if (!phrase) return input.focus();
      el = hit("[data-filter-add]");
      el.disabled = true;
      let reply = null;
      const ok = await write(async () => {
        reply = await api("/filters", { method: "POST", body: { phrase } });
      });
      el.disabled = false;
      if (ok) {
        haptic("ok");
        toast((reply && reply.notice) || "«" + phrase + "» اضافه شد و فعلا فقط بررسی می کند؛ برای حذف عکس ها، کلیدش را روشن کنید.", "ok");
        input.value = "";
        reloadTopPage();
      }
      return;
    }
    if (hit("[data-word-add]")) {
      const input = document.getElementById("filter-word");
      if (!input.value.trim()) return input.focus();
      return addFilterWord(input.value);
    }
    if (hit("[data-answer-add]")) {
      const trig = document.getElementById("answer-trigger");
      const body = document.getElementById("answer-text");
      if (!trig.value.trim()) return trig.focus();
      if (!body.value.trim()) return body.focus();
      const trigger = trig.value;
      const text = body.value;
      const page = currentPage();
      const same = page && page.data ? page.data.entries.find((x) => fold(decode(x.name)) === fold(trigger)) : null;
      if (same) {
        S.confirm = {
          title: "«" + decode(same.name) + "» از قبل پاسخ دارد؛ جایگزین شود؟",
          sub: (same.media ? "پاسخ قبلی و رسانه همراهش با این متن عوض می شود." : "پاسخ قبلی با این متن عوض می شود.") + (same.formatted ? " قالب بندی آن (پررنگ، لینک) هم از بین می رود؛ برای نگه داشتنش از داخل گروه ویرایش کنید." : ""),
          label: "جایگزین کن",
          quiet: true,
          run: () => addAnswer(trigger, text),
        };
        return render();
      }
      return addAnswer(trigger, text);
    }
    if ((el = hit("[data-answer-edit]"))) {
      const page = currentPage();
      const entry = page && page.data ? page.data.entries.find((x) => x.key === el.dataset.answerEdit) : null;
      const trig = document.getElementById("answer-trigger");
      const body = document.getElementById("answer-text");
      if (!entry || !trig || !body) return;
      trig.value = decode(entry.name);
      body.value = decode(entry.text != null ? entry.text : entry.preview || "");
      body.focus();
      body.scrollIntoView({ block: "center", behavior: "smooth" });
      return;
    }
    if ((el = hit("[data-word-exact]"))) {
      const page = currentPage();
      const on = el.dataset.wordExact === "1";
      if (!page || (page.mode && page.mode.exact === on)) return;
      const ok = await write(async () => {
        page.mode = await api("/filter-words/exact", { method: "POST", body: { on } });
      });
      if (ok) haptic("sel");
      render();
      return;
    }
    if (hit("[data-voice-add]")) {
      const input = document.getElementById("voice-word");
      const word = input.value.trim();
      if (!word) return input.focus();
      if (await addVoiceWord(word)) {
        const again = document.getElementById("voice-word");
        if (again) again.focus();
      }
      return;
    }
    if ((el = hit("[data-voice-remove]"))) {
      const key = el.dataset.voiceRemove;
      const tag = "voice:" + key;
      if (S.pending !== tag) {
        S.pending = tag;
        haptic();
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
      S.pending = null;
      const page = currentPage();
      const was = page && page.data ? page.data.words.find((w) => w.key === key) : null;
      const ok = await write(async () => {
        const d = await api("/voice/words/" + encodeURIComponent(key), { method: "DELETE" });
        if (page && d && d.words) page.data = d;
      });
      render();
      if (ok) {
        haptic();
        const word = was ? decode(was.word) : "";
        toast(word ? "«" + word + "» حذف شد" : "حذف شد.", "ok", word ? { label: "برگرداندن", run: () => addVoiceWord(word) } : null);
      }
      return;
    }
    if (hit("[data-voice-restore]")) {
      const ok = await write(() => api("/voice/restore", { method: "POST" }));
      if (ok) reloadTopPage();
      return;
    }
    if ((el = hit("[data-case]"))) {
      let page = currentPage();
      if (!page || page.id !== "cases") {
        await pushPage("cases");
        page = currentPage();
      }
      let detail = null;
      const ok = await write(async () => {
        detail = await api("/cases/" + el.dataset.case);
      });
      if (ok && page && page.id === "cases" && page.data) {
        page.data.detail = detail;
        render();
        window.scrollTo(0, 0);
      }
      return;
    }
    if ((el = hit("[data-case-filter]"))) {
      const page = currentPage();
      if (!page || !page.data) return;
      return loadCases(page, el.dataset.caseFilter, page.data.user || "");
    }
    if (hit("[data-case-clear]")) {
      const page = currentPage();
      if (page && page.data) loadCases(page, page.data.status || "open", "");
      return;
    }
    if (hit("[data-case-search]")) {
      const page = currentPage();
      const input = document.getElementById("case-user");
      if (page && input) searchCases(page, input.value);
      return;
    }
    if ((el = hit("[data-case-more]"))) {
      const page = currentPage();
      if (!page || !page.data) return;
      const before = el.dataset.caseMore;
      return guarded("cases-more", async () => {
        const status = page.data.status || "open";
        const user = page.data.user ? "&user_id=" + encodeURIComponent(page.data.user) : "";
        S.casesMore = true;
        render();
        await write(async () => {
          const data = await api("/cases?status=" + status + "&before_id=" + before + user);
          const seen = new Set(page.data.cases.map((c) => c.id));
          page.data.cases = page.data.cases.concat((data.cases || []).filter((c) => !seen.has(c.id)));
          page.data.has_more = data.has_more;
        });
        S.casesMore = false;
        render();
      });
    }
    if ((el = hit("[data-case-action]"))) {
      const action = el.dataset.caseAction;
      if (action === "none") return runCaseAction("none");
      S.confirm = confirmFor(action === "reverse" ? "case-reverse" : "case-delete");
      return render();
    }
    if ((el = hit("[data-case-open]"))) return openLink(el.dataset.caseOpen);
    if (hit("[data-case-note]")) {
      const page = currentPage();
      if (!page || !page.data || !page.data.detail) return;
      const input = document.getElementById("case-note");
      const note = input.value.trim();
      if (!note) return input.focus();
      const id = page.data.detail.id;
      return guarded("case-note:" + id, async () => {
        const ok = await write(async () => {
          await api("/cases/" + id + "/notes", { method: "POST", body: { note } });
          page.data.detail = await api("/cases/" + id);
        });
        if (ok) {
          toast("یادداشت ثبت شد.", "ok");
          S.dropTyped = true;
          render();
        }
      });
    }
    if (hit("[data-joingate-save]")) {
      const input = document.getElementById("join-gate-input");
      const channel = input.value.trim();
      if (!channel) return input.focus();
      el = hit("[data-joingate-save]");
      el.disabled = true;
      let d = null;
      const ok = await write(async () => {
        d = await api("/join-gate", { method: "POST", body: { channel } });
      });
      el.disabled = false;
      if (ok) {
        if (!unsure(d)) {
          haptic("ok");
          S.dropTyped = true;
          toast("عضویت اجباری روشن شد؛ تا عضو کانال نشوند پیام نمی دهند.", "ok");
        }
        S.joinGate = null;
        reloadTopPage();
      }
      return;
    }
    if (hit("[data-joingate-off]")) {
      let d = null;
      const ok = await write(async () => {
        d = await api("/join-gate", { method: "POST", body: { channel: "" } });
      });
      if (ok) {
        if (!unsure(d)) toast("عضویت اجباری خاموش شد.", "ok");
        S.joinGate = null;
        reloadTopPage();
      }
      return;
    }
  }

  function quick(what) {
    if (what !== "close" || S.lockBusy) return;
    const g = groupState();
    if (!g) return pushPage("rights");
    if (g.manual || g.until) return setGroupLock(false);
    if (g.night) return openSheet("ng");
    if (g.allClosed) return pushPage("rights");
    S.confirm = confirmFor("rights-close");
    render();
  }

  function fixIssue(fix) {
    if (fix === "rights") return openSheet("rights-help");
    if (fix === "log") return pushPage("log");
    if (fix.startsWith("feature:")) return openSheet(fix.slice(8));
  }

  function onKeydown(e) {
    const t = e.target;
    if (t.id === "palette-input") {
      const results = (S.palette && S.palette.results) || [];
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        if (!results.length) return;
        S.palette.idx = (S.palette.idx + (e.key === "ArrowDown" ? 1 : results.length - 1)) % results.length;
        drawPalette();
        const act = document.querySelector("#palette .row.act");
        if (act) act.scrollIntoView({ block: "nearest" });
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        paletteGo(S.palette.idx);
      }
      return;
    }
    if (e.key !== "Enter") return;
    let el;
    if ((el = t.closest("[data-number-apply]"))) {
      e.preventDefault();
      el.blur();
      return;
    }
    if (t.id === "filter-phrase") document.querySelector("[data-filter-add]").click();
    if (t.id === "filter-word") document.querySelector("[data-word-add]").click();
    if (t.id === "voice-word") document.querySelector("[data-voice-add]").click();
    if (t.id === "join-gate-input") document.querySelector("[data-joingate-save]").click();
    if (t.id === "log-channel") document.querySelector("[data-log-connect]").click();
    if (t.id === "allow-bot") document.querySelector("[data-allow-bot]").click();
    if (t.id === "answer-trigger") {
      e.preventDefault();
      document.getElementById("answer-text").focus();
    }
    if ((e.ctrlKey || e.metaKey) && t.id === "doc-text") {
      const save = document.querySelector("[data-doc-save]");
      if (save) save.click();
    }
    if (t.id === "case-user") {
      e.preventDefault();
      searchCases(currentPage(), t.value);
    }
  }

  function onGlobalKey(e) {
    const typing = e.target && (e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA");
    if (!S.dash) return;
    if ((e.ctrlKey || e.metaKey) && (e.key === "k" || e.key === "K")) {
      e.preventDefault();
      if (S.palette) closePalette();
      else openPalette();
      return;
    }
    if (e.key === "/" && !typing && !S.palette) {
      e.preventDefault();
      openPalette();
      return;
    }
    if (e.key === "Escape") {
      if (S.palette || S.confirm || S.sheet || S.stack.length) {
        e.preventDefault();
        back();
      }
    }
  }

  function onFocusOut(e) {
    commitField(e.target);
  }

  function commitField(target) {
    const num = target && target.closest && target.closest("[data-number-apply]");
    if (num) {
      const value = num.value.trim();
      if (value === "" || value === num.dataset.value) return;
      const n = Number(value);
      const min = Number(num.min);
      const max = Number(num.max);
      if (!Number.isInteger(n) || n < min || n > max) {
        toast("عددی بین " + fa(min) + " تا " + fa(max) + " بنویسید.", "warn");
        num.value = num.defaultValue;
        return;
      }
      num.dataset.value = String(n);
      if (num.dataset.numberApply === "apc" && n === 0) {
        S.confirm = confirmFor("apc-all");
        return render();
      }
      applySetting(num.dataset.numberApply + ":" + n);
      return;
    }
    const clock = target && target.closest && target.closest("[data-clock-apply]");
    if (!clock) return;
    const minutes = clockToMinutes(clock.value);
    if (minutes === null) return;
    if (String(minutes) === String(clock.dataset.clockValue)) return;
    clock.dataset.clockValue = String(minutes);
    applySetting(clock.dataset.clockApply + ":" + minutes);
  }

  function flushField() {
    const el = document.activeElement;
    if (el && el.matches && el.matches("[data-number-apply], [data-clock-apply]")) commitField(el);
  }

  function onInput(e) {
    const t = e.target;
    if (t.id === "palette-input") {
      S.palette.q = t.value;
      S.palette.idx = 0;
      drawPalette();
    } else if (t.id === "feature-search") {
      S.featureQuery = t.value;
      const box = document.getElementById("feat-results");
      if (box) box.innerHTML = featureResults();
      if (t.value && !S.lockCache) api("/locks").then((d) => { S.lockCache = d; const b = document.getElementById("feat-results"); if (b) b.innerHTML = featureResults(); }).catch(() => {});
    } else if (t.id === "lock-search") {
      S.lockQuery = t.value;
      render();
    } else if (t.id === "list-search") {
      S.listQuery = t.value;
      const page = currentPage();
      const kind = page && page.id.startsWith("list:") ? page.id.slice(5) : null;
      if ((kind === "ban" || kind === "mute") && page.data && (page.data.truncated || page.remoteQ)) {
        return searchListRemote(page, kind, t.value);
      }
      const q = fold(t.value);
      document.querySelectorAll("#main .list .row").forEach((row) => {
        const name = row.querySelector(".t");
        row.hidden = q.length > 0 && !(name && fold(name.textContent).includes(q));
      });
    } else if (t.id === "group-search") {
      S.groupQuery = t.value;
      const q = fold(t.value);
      document.querySelectorAll("#sheet [data-pick-group]").forEach((row) => {
        const name = row.querySelector(".t");
        row.hidden = q.length > 0 && !(name && fold(name.textContent).includes(q));
      });
    } else if (t.id === "doc-text") {
      const page = currentPage();
      if (!page) return;
      page.draft = t.value;
      syncDoc(page);
    }
  }

  let drag = null;
  document.addEventListener("touchstart", (e) => {
    if (e.touches.length !== 1) return;
    const y = e.touches[0].clientY;
    const sheetEl = e.target.closest && e.target.closest("#sheet");
    if (sheetEl && S.sheet && !WIDE.matches && sheetEl.scrollTop <= 0 && !e.target.closest("input, textarea")) {
      drag = { kind: "sheet", y, dy: 0, el: sheetEl };
      return;
    }
    if (!sheetEl && !S.sheet && !S.palette && !S.confirm && S.dash && window.scrollY <= 0 && !e.target.closest("input, textarea, #tabs")) {
      drag = { kind: "ptr", y, dy: 0 };
    }
  }, { passive: true });

  document.addEventListener("touchmove", (e) => {
    if (!drag) return;
    drag.dy = Math.max(0, e.touches[0].clientY - drag.y);
    if (drag.kind === "sheet") {
      if (drag.dy > 0) {
        drag.el.classList.add("drag");
        drag.el.style.transform = "translateY(" + drag.dy + "px)";
      }
    } else {
      const ptr = ptrEl();
      const pull = Math.min(drag.dy * 0.5, 80);
      ptr.style.opacity = String(Math.min(1, pull / 50));
      ptr.style.transform = "translateY(" + (pull - 40) + "px) rotate(" + -pull * 4 + "deg)";
    }
  }, { passive: true });

  document.addEventListener("touchend", () => {
    if (!drag) return;
    const d = drag;
    drag = null;
    if (d.kind === "sheet") {
      d.el.classList.remove("drag");
      d.el.style.transform = "";
      if (d.dy > 100) closeSheet();
      return;
    }
    const ptr = ptrEl();
    if (d.dy * 0.5 > 60) {
      ptr.classList.add("go");
      ptr.style.transform = "translateY(20px)";
      refreshAll().finally(() => {
        ptr.classList.remove("go");
        ptr.style.opacity = "0";
        ptr.style.transform = "";
      });
    } else {
      ptr.style.opacity = "0";
      ptr.style.transform = "";
    }
  });

  function ptrEl() {
    let el = document.getElementById("ptr");
    if (!el) {
      el = document.createElement("div");
      el.id = "ptr";
      el.innerHTML = icon("refresh", 18);
      document.body.appendChild(el);
    }
    return el;
  }

  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible" || !S.dash || Date.now() - S.loadedAt <= 60000) return;
    const page = currentPage();
    const typing = Array.from(app.querySelectorAll("#main input, #main textarea")).some((el) => el.value !== el.defaultValue && el.type !== "search" && el.type !== "time");
    if (typing || S.confirm || S.sheet || (page && page.id === "cases" && page.data && page.data.detail) || (page && DOCS[page.id] && docDirty(page))) return;
    refreshAll(true);
  });
  function onMedia(query, run) {
    if (query.addEventListener) query.addEventListener("change", run);
    else if (query.addListener) query.addListener(run);
  }
  onMedia(window.matchMedia("(prefers-color-scheme: light)"), applyTheme);
  onMedia(WIDE, () => S.dash && render());

  start();
})();
