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

  const HEADER = { dark: "#14161f", light: "#f6f7fb" };

  function themePref() {
    const pref = stored(THEME_KEY);
    return pref === "dark" || pref === "light" ? pref : "auto";
  }

  function applyTheme() {
    const pref = themePref();
    let theme = pref;
    if (pref === "auto") {
      if (tg && tg.colorScheme) theme = tg.colorScheme;
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
      S.dash = d;
      return;
    }
    try {
      await refreshDash();
    } catch (e) {
    }
    if (d && d.error) toast(d.error, "warn");
    else if (d && d.accepted) toast("ثبت شد؛ کمی بعد در تلگرام اعمال می شود.", "warn");
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
    toastAction = action ? action.run : null;
    el.className = kind;
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
      enabled: "dr", off: "/report/off", onAction: () => "dr:" + setting("dr").value, sections: ["dr"], custom: "report",
      effect: () => "هر شب " + shown("dr"),
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
      '<div class="row tap" data-open="' + f.id + '" role="button" tabindex="0">' +
      '<span class="rico' + (st ? " on" : "") + '">' + icon(f.icon, 17) + "</span>" +
      '<span class="rt"><span class="t">' + f.title + '</span><span class="s' + (st ? " on" : "") + '">' + esc(sub) + "</span></span>" +
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
      { id: "rights", title: "ربات با همه دسترسی ها ادمین است", sub: "بدون آن هیچ قفل و فیلتری کار نمی کند", done: rightsOk, fix: () => openSheet("rights-help") },
      { id: "fl", title: "ضد رگبار روشن است", sub: "جلوی پیام های پشت سر هم را می گیرد", done: isOn("fl_on"), fix: () => openSheet("fl") },
      { id: "cp", title: "احراز هویت روشن است", sub: "ربات های اسپم نمی توانند عضو بمانند", done: isOn("cp_on"), fix: () => openSheet("cp") },
      { id: "rd", title: "ضد هجوم روشن است", sub: "در برابر ورود دسته جمعی", done: isOn("rd_on"), fix: () => openSheet("rd") },
      { id: "links", title: "قفل لینک روشن است", sub: "رایج ترین راه تبلیغ در گروه ها", done: linksOn, fix: () => pushPage("locks") },
      { id: "log", title: "کانال لاگ وصل است", sub: "سابقه همه برخوردها جایی ثبت شود", done: logOk, fix: () => pushPage("log") },
      { id: "wc", title: "پیام خوشامد نوشته شده", sub: "عضو تازه قوانین را می بیند", done: welcomeOk, fix: () => pushPage("welcome") },
    ];
  }

  function score() {
    const list = checks().filter((c) => c.done !== null);
    if (!list.length) return null;
    const done = list.filter((c) => c.done).length;
    return { pct: Math.round((done / list.length) * 100), done, total: list.length, list };
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
      renderFatal("shieldOff", STATE_MESSAGES.auth_failed, "در گروه «پنل» بفرستید یا از دکمه منوی ربات وارد شوید.", false);
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
          tg.SettingsButton.onClick(() => S.dash && goTab("settings"));
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
    S.lockCache = null;
    S.pending = null;
    S.featureQuery = "";
    S.barSel = null;
    S.tab = "home";
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
      S.rights = "error";
    }
    rerenderIfIdle(["home", "members"]);
  }

  async function loadSide() {
    const chat = S.chat;
    const jobs = [
      api("/locks").then((d) => { S.lockCache = d; }),
      api("/log").then((d) => { S.logInfo = d; }, () => { S.logInfo = "error"; }),
      api("/welcome").then((d) => { S.welcomeInfo = d; }, () => { S.welcomeInfo = "error"; }),
      api("/slow").then((d) => { S.slow = d; }),
      api("/cases?status=open&limit=5").then((d) => { S.openCases = d; }, () => { S.openCases = "denied"; }),
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
      (known ? "" : '<button class="btn ghost" data-retry-chat>' + icon("refresh", 16) + " تلاش دوباره</button>") +
      (S.groups && S.groups.length > 1 ? '<button class="btn" data-picker>انتخاب گروه دیگر</button>' : "") +
      "</div></div></div></div>" + renderOverlays();
    document.body.classList.add("nobar");
    syncBack();
  }

  function renderNoGroups() {
    app.innerHTML =
      '<aside id="rail"></aside><div id="view"><div class="top"><div class="grp"><div class="avatar" style="background:var(--card-2);color:var(--muted)">' +
      icon("shieldCheck", 18) + '</div><div><div class="gname">مدیریت گروه</div><div class="gsub">هنوز گروهی وصل نیست</div></div></div></div>' +
      '<div id="main" class="nobar"><div class="empty"><div class="glyph">' + icon("users", 32, 1.6) + "</div>" +
      '<div class="t">گروهی برای مدیریت نیست</div>' +
      '<div class="s">ربات را به گروه خود اضافه و ادمین کنید و همه دسترسی ها را به آن بدهید. بعد این صفحه را دوباره باز کنید.</div>' +
      '<button class="btn" data-reboot>' + icon("refresh", 16) + " بررسی دوباره</button>" +
      "</div></div></div>";
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
    if (!c || c === "denied" || !c.cases) return 0;
    return c.cases.length;
  }

  function groupButton(extra) {
    const g = group();
    const title = g ? g.title : S.dash ? S.dash.chat.title : "انتخاب گروه";
    const role = S.dash ? (S.dash.viewer.is_owner ? "مالک" : "ادمین") : g && g.is_owner ? "مالک" : "ادمین";
    const many = S.groups && S.groups.length > 1;
    const chosen = !!(g || S.dash);
    return (
      '<button class="grp' + (extra || "") + '" data-picker' + (many ? "" : " disabled") + ' aria-label="انتخاب گروه">' +
      (chosen
        ? '<span class="avatar" style="' + avatarStyle(S.chat) + '">' + esc(initial(title)) + "</span>"
        : '<span class="avatar" style="background:var(--card-2);color:var(--muted)">' + icon("users", 18) + "</span>") +
      '<span style="min-width:0;flex:1"><span class="gname" style="display:block">' + title + '</span><span class="gsub" style="display:block">' +
      (chosen ? role + (many ? " · " + fa(S.groups.length) + " گروه" : "") : fa((S.groups || []).length) + " گروه در دسترس") + "</span></span>" +
      (many ? '<span class="chev" style="color:var(--muted)">' + icon("chevD", 16) + "</span>" : "") +
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
    const wideTitle = '<div class="wide-only" style="min-width:0"><div class="ptitle">' + (TAB_TITLES[S.tab] || "") + '</div><div class="gsub">' + (S.dash ? S.dash.chat.title : "") + "</div></div>";
    return '<div class="top">' + groupButton(bare ? "" : " hide-wide") + (bare ? "" : wideTitle) + topActions(bare) + "</div>";
  }

  function renderPageTop(title, sub, right) {
    return (
      '<div class="top"><div class="lead">' +
      '<button class="ibtn" data-back aria-label="بازگشت">' + icon("arrowL", 20) + "</button>" +
      '<div style="min-width:0"><div class="ptitle">' + title + '</div><div class="gsub">' + (sub || "") + "</div></div></div>" +
      '<div class="acts">' + (right || "") + "</div></div>"
    );
  }

  function issueCount() {
    return S.health && S.health !== "error" ? S.health.issues.length : 0;
  }

  function tabBadge(id) {
    if (id === "home") return issueCount();
    if (id === "reports") return openCaseCount();
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
      RAIL_PAGES.map(([id, ic, label]) => nav('data-push-root="' + id + '"', ic, label, root && root.id === id, id === "cases" ? openCaseCount() : 0)).join("") +
      '<div class="rfoot"><kbd>Ctrl K</kbd> جستجو · <kbd>Esc</kbd> بازگشت</div>' +
      "</aside>"
    );
  }

  function renderOverlays() {
    const sh = S.sheet;
    const open = sh && !sh.fresh;
    return '<div id="dim" data-close' + (open ? ' class="open"' : "") + '></div><div id="sheet" role="dialog" aria-modal="true"' + (open ? ' class="open"' : "") + ">" + (sh ? sheetHtml() : "") + "</div>" + renderPalette();
  }

  function renderShellBare() {
    app.innerHTML = '<aside id="rail"></aside><div id="view">' + renderTop(true) + '<div id="main" class="nobar"></div></div>' + renderOverlays();
    document.body.classList.add("nobar");
  }

  function render() {
    if (!S.dash) return;
    const page = S.stack[S.stack.length - 1];
    const view = page ? renderPage(page) : renderTop(false) + '<div id="main">' + renderTab() + "</div>";
    const html = renderRail() + '<div id="view">' + view + "</div>" + (page ? "" : renderTabs()) + renderOverlays() + renderConfirm();
    const y = window.scrollY;
    const focused = document.activeElement && document.activeElement.id;
    const sheetY = (document.getElementById("sheet") || {}).scrollTop || 0;
    app.innerHTML = html;
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
        S.confirm = {
          title: "تغییرات ذخیره نشده کنار گذاشته شود؟",
          sub: "متنی که نوشته اید ذخیره نشده است.",
          label: "کنار گذاشتن",
          quiet: true,
          run: async () => {
            page.draft = page.saved;
            S.stack.pop();
            setClosingConfirmation(false);
          },
        };
        render();
        return;
      }
      if (page && page.id === "cases" && page.data && page.data.detail) {
        delete page.data.detail;
        render();
        return;
      }
      S.stack.pop();
      render();
      return;
    }
    if (S.tab !== "home") goTab("home");
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
      title = "در حال بررسی…";
    } else if (h === "error") {
      title = "وضعیت ربات خوانده نشد";
      sub = "تنظیمات در دسترس است · " + facts;
      acts = '<button class="hbtn" data-recheck>' + icon("refresh", 14) + " تلاش دوباره</button>";
    } else if (h.issues.length) {
      const bad = h.issues.some((i) => i.severity === "bad");
      cls = bad ? " bad" : " warn";
      title = fa(h.issues.length) + " مورد نیاز به توجه دارد";
      sub = bad ? "بخشی از محافظت الان کار نمی کند." : "یک تنظیم ناقص مانده است.";
      acts = '<button class="hbtn solid" data-fix="' + esc(h.issues[0].fix) + '">رفع مشکل</button><button class="hbtn" data-recheck>' + icon("refresh", 14) + " بررسی دوباره</button>";
    } else {
      title = sc && sc.pct >= 100 ? "گروه کاملا محافظت می شود" : "گروه محافظت می شود";
      if (sc && sc.pct < 100) acts = '<button class="hbtn solid" data-scroll="setup">تکمیل راه اندازی</button>';
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
    return '<div class="sec" id="setup"><div class="card"><div class="setup-head"><div class="t"><span>راه اندازی گروه</span><span class="pill accent">' + fa(sc.done) + " از " + fa(sc.total) + '</span></div>' +
      '<div class="s">چند قدم تا محافظت کامل؛ روی هر مورد بزنید.</div><div class="progress"><span style="width:' + sc.pct + '%"></span></div></div>' +
      items.map((c, i) =>
        '<button class="row' + (c.done ? " done" : "") + '" data-check="' + i + '"><span class="check' + (c.done ? " done" : "") + '">' + icon("check", 14, 3) + '</span><span class="rt"><span class="t">' + c.title + '</span><span class="s">' + c.sub + "</span></span>" + (c.done ? "" : CHEV) + "</button>"
      ).join("") + "</div></div>";
  }

  function groupClosed() {
    if (!S.rights || S.rights === "error" || !S.rights.rights) return null;
    return S.rights.rights.every((r) => !r.open);
  }

  function renderQuick() {
    const closed = groupClosed();
    const q = (attr, ic, label, cls) =>
      "<button " + attr + (cls ? ' class="' + cls + '"' : "") + '><span class="q">' + icon(ic, 20) + '</span><span class="l">' + label + "</span></button>";
    return (
      '<div class="quick">' +
      q('data-quick="close"', closed ? "lock" : "unlock", closed ? "گروه بسته است" : "بستن گروه", closed ? "bad" : "") +
      q('data-apply="strict"', "gavel", "سختگیرانه", isOn("strict") ? "on" : "") +
      q('data-apply="cp_on"', "key", "احراز هویت", isOn("cp_on") ? "on" : "") +
      q('data-open="ng"', "moon", "قفل شب", sectionEnabled("ng") ? "on" : "") +
      q('data-open="sl"', "snail", "اسلوموشن", S.slow && S.slow.seconds > 0 ? "on" : "") +
      q('data-open="ap"', "trash", "پاکسازی", sectionEnabled("ap") ? "on" : "") +
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
      let delta = "";
      if (prev != null) {
        const diff = now - prev;
        delta = diff === 0 ? "مثل دیروز" : '<span class="' + (diff > 0 ? "up" : "down") + (id === "joined" ? " good" : "") + '">' + icon(diff > 0 ? "arrowUp" : "arrowDown", 11, 3) + fa(Math.abs(diff)) + "</span> از دیروز";
      }
      return '<button class="kpi" data-kpi="' + id + '"><div class="l">' + icon(m.icon, 14) + label + " · امروز</div><div class=\"v\">" + fa(now) + '</div><div class="d">' + delta + "</div>" +
        '<div class="spark">' + values.map((v) => '<i style="height:' + Math.max(8, (v / max) * 100) + '%"></i>').join("") + "</div></button>";
    }).join("") + "</div>";
  }

  function renderHomeCases() {
    const c = S.openCases;
    if (!c || c === "denied" || !c.cases || !c.cases.length) return "";
    return '<div class="sec"><div class="h"><div class="t">گزارش های باز</div><button class="more" data-push="cases">همه ' + icon("chevL", 14) + "</button></div>" +
      '<div class="list">' + c.cases.slice(0, 3).map(caseRow).join("") + "</div></div>";
  }

  function renderHome() {
    const top = ["fl", "cq", "locks", "cp", "rd"].map((id) => featureRow(feature(id))).join("");
    return (
      '<div class="grid2 wide-left"><div class="col">' +
      '<div class="sec first">' + renderHero() + renderIssues() + "</div>" +
      '<div class="sec"><div class="h"><div class="t">اقدام سریع</div></div>' + renderQuick() + "</div>" +
      '<div class="sec"><div class="h"><div class="t">امروز در گروه</div><button class="more" data-tab="reports">گزارش کامل ' + icon("chevL", 14) + "</button></div>" + renderKpis() + "</div>" +
      renderHomeCases() +
      '</div><div class="col">' +
      renderSetup() +
      '<div class="sec"><div class="h"><div class="t">محافظت اصلی</div><button class="more" data-tab="features">همه امکانات ' + icon("chevL", 14) + "</button></div>" +
      '<div class="list">' + top + "</div></div>" +
      "</div></div>"
    );
  }

  function renderFeatures() {
    const q = fold(S.featureQuery);
    const cat = S.featCat;
    const total = FEATURES.filter((f) => featureState(f) !== null && !f.always).length;
    let html = '<div class="phead"><div class="t">امکانات</div><div class="s">' + fa(activeFeatureCount()) + " از " + fa(total) + " امکان روشن است · روی هر کدام بزنید تا تنظیمش کنید</div></div>" +
      '<div class="sec"><label class="search">' + icon("search", 16) +
      '<input id="feature-search" type="search" placeholder="جستجو، مثلا «لینک» یا «ویس»" value="' + esc(S.featureQuery || "") + '" /></label>' +
      '<div class="chips scroll" style="margin-top:10px">' +
      [["all", "همه"]].concat(GROUPS.map(([g]) => [g, g])).concat([["on", "روشن ها"], ["off", "خاموش ها"]]).map(([id, label]) =>
        '<button class="chip sm' + (cat === id ? " on" : "") + '" data-feat-cat="' + esc(id) + '">' + label + "</button>").join("") +
      "</div></div>";
    let any = false;
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
        '<div class="sec"><div class="h"><div class="t">' + g + '<span class="sub">' + gdesc + '</span></div><div class="count">' +
        (switchable ? fa(on) + " از " + fa(switchable) + " روشن" : "") + "</div></div>" +
        '<div class="list">' + items.map(featureRow).join("") + "</div></div>";
    });
    html += '<div class="tiles">' + body + "</div>";
    if (!any) html += '<div class="empty sm"><div class="glyph">' + icon("search", 24) + '</div><div class="t">' + (q ? "چیزی با این نام نیست" : cat === "on" ? "هیچ امکانی روشن نیست" : "همه امکانات روشن است") + "</div></div>";
    html += '<div class="sec"><button class="btn outline block" data-open="help">' + icon("book", 16) + " دستورهایی که از داخل گروه کار می کنند</button></div>";
    return html;
  }

  const LIST_KINDS = {
    ban: ["بن شده ها", "ban", "کسانی که از گروه بیرون شده اند"],
    mute: ["سکوت شده ها", "mute", "نمی توانند پیام بدهند"],
    vip: ["کاربران ویژه", "star", "از همه قفل ها معاف اند"],
    free: ["معاف ها", "unlock", "از ضد رگبار و اد اجباری معاف اند"],
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
      (S.dash.viewer.is_owner ? '<button class="more" data-open="lim">محدودیت ها ' + icon("chevL", 14) + "</button>" : "") + "</div>";
    if (!a) {
      admins += '<div class="list">' + ('<div class="row"><div class="sk" style="width:36px;height:36px;border-radius:11px"></div><div class="rt"><div class="sk" style="width:120px;height:12px"></div></div></div>').repeat(3) + "</div>";
    } else if (a === "error") {
      admins += '<div class="list"><div class="row"><span class="rt"><span class="s">فهرست ادمین ها خوانده نشد.</span></span><button class="btn sm ghost" data-reload-admins>تلاش</button></div></div>';
    } else {
      admins += '<div class="list">' + a.admins.map((ad) => {
        const removable = a.can_remove && !ad.is_creator && !ad.is_bot;
        return (
          '<div class="row"><span class="avatar sm" style="' + avatarStyle(ad.id) + '">' + esc(initial(ad.name)) + "</span>" +
          '<span class="rt"><span class="t">' + ad.name + '</span><span class="s">' + (ad.is_creator ? "مالک گروه" : ad.is_bot ? "ربات" : "ادمین") + "</span></span>" +
          (ad.is_creator ? '<span class="pill ok">مالک</span>' : ad.is_bot ? '<span class="pill mute">ربات</span>' : "") +
          (removable ? '<button class="ibtn sm danger" data-remove-admin="' + ad.id + '" data-name="' + ad.name + '" title="عزل" aria-label="عزل">' + icon("x", 17) + "</button>" : "") +
          "</div>"
        );
      }).join("") + "</div>";
      admins += '<div class="hint start">' + (a.can_remove ? "ربات فقط ادمین هایی را عزل می کند که خودش اضافه کرده. برای ترفیع، در گروه روی پیام کاربر «ترفیع» بفرستید." : "فقط مالک گروه می تواند ادمین عزل کند.") + "</div>";
    }
    admins += "</div>";

    const closed = groupClosed();
    let rights = '<div class="sec"><div class="h"><div class="t">دسترسی اعضای عادی</div></div><div class="list">' +
      '<button class="row" data-push="rights"><span class="rico' + (closed ? " bad" : " on") + '">' + icon(closed ? "lock" : "chat", 17) + '</span><span class="rt"><span class="t">' + (closed ? "گروه بسته است" : "چه چیزی می توانند بفرستند") + '</span><span class="s">' +
      (closed ? "هیچ عضو عادی نمی تواند پیام بدهد" : "پیام، عکس، ویدیو، ویس، استیکر، نظرسنجی…") + "</span></span>" + CHEV + "</button>" +
      '<button class="row" data-open="ad"><span class="rico' + (featureState(feature("ad")) ? " on" : "") + '">' + icon("userPlus", 17) + '</span><span class="rt"><span class="t">اد اجباری</span><span class="s">' + (featureState(feature("ad")) ? esc(feature("ad").effect()) : "خاموش") + "</span></span>" + CHEV + "</button>" +
      '<button class="row" data-push="joingate"><span class="rico">' + icon("door", 17) + '</span><span class="rt"><span class="t">عضویت اجباری در کانال</span><span class="s">تا عضو کانال نشوند پیام نمی دهند</span></span>' + CHEV + "</button>" +
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
    let left = '<div class="phead"><div class="t">گزارش ها</div><div class="s">آمار هفته، گزارش های اعضا و کانال لاگ</div></div>';
    if (!a) left += '<div class="sec"><div class="sk" style="height:240px;border-radius:18px"></div></div>';
    else if (a === "error") {
      left += '<div class="sec"><div class="card"><div class="empty sm"><div class="glyph">' + icon("wifiOff", 24) + '</div><div class="t">آمار خوانده نشد</div><div class="s" style="margin-bottom:14px">اتصال را بررسی کنید.</div><button class="btn sm ghost" data-reload-activity>' + icon("refresh", 14) + " تلاش دوباره</button></div></div></div>";
    } else {
      left += '<div class="sec">' + renderChart(a) + "</div>";
    }

    let right = "";
    const c = S.openCases;
    if (c !== "denied") {
      right += '<div class="sec"><div class="h"><div class="t">پرونده ها' + (openCaseCount() ? ' <span class="count">· ' + fa(openCaseCount()) + (c && c.has_more ? "+" : "") + " باز</span>" : "") + '</div><button class="more" data-push="cases">همه ' + icon("chevL", 14) + "</button></div>";
      if (!c) right += '<div class="sk" style="height:110px;border-radius:18px"></div>';
      else if (!c.cases.length) right += '<div class="card"><div class="empty sm"><div class="glyph">' + icon("shieldCheck", 24) + '</div><div class="t">پرونده باز نیست</div><div class="s">همه گزارش ها رسیدگی شده است.</div></div></div>';
      else right += '<div class="list">' + c.cases.slice(0, 4).map(caseRow).join("") + "</div>";
      right += "</div>";
    }

    if (a && a !== "error") {
      const sel = S.barSel == null ? 0 : S.barSel;
      const day = a.days.find((d) => d.ago === sel) || a.days[0];
      const shownKeys = day.counters.filter((x) => x.count > 0);
      right += '<div class="sec"><div class="h"><div class="t">جزئیات ' + dayName(day) + "</div></div>";
      if (!shownKeys.length) {
        right += '<div class="note mute">' + icon("info", 16) + "<span>" + (sel === 0 ? "امروز هنوز رویدادی ثبت نشده. شمارش از اولین حذف، سکوت یا عضو تازه شروع می شود." : "در این روز رویدادی ثبت نشده.") + "</span></div>";
      } else {
        right += '<div class="list">' + day.counters.map((x) =>
          '<div class="row compact"><span class="rt"><span class="t n">' + x.label + '</span></span><span class="count" style="color:' + (x.count ? "var(--text)" : "var(--muted-2)") + ';font-weight:650">' + fa(x.count) + "</span></div>"
        ).join("") + "</div>";
      }
      right += "</div>";
    }

    const log = S.logInfo && S.logInfo !== "error" ? S.logInfo : null;
    right += '<div class="sec"><div class="h"><div class="t">ثبت و گزارش</div></div><div class="list">' +
      '<button class="row" data-push="log"><span class="rico' + (log && log.channel ? " on" : "") + '">' + icon("fileText", 17) + '</span><span class="rt"><span class="t">کانال لاگ</span><span class="s">' +
      (log ? (log.channel ? "وصل · " + fa(log.kinds.filter((k) => k.on).length) + " رویداد ثبت می شود" : "وصل نیست") : "…") + "</span></span>" +
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
      '<button class="row" data-open="' + fid + '"><span class="rico' + (on ? " on" : "") + '">' + icon(ic, 17) + '</span><span class="rt"><span class="t">' + title + '</span><span class="s">' + sub + "</span></span>" + CHEV + "</button>";
    const pageRow = (pid, ic, title, sub, on) =>
      '<button class="row" data-push="' + pid + '"><span class="rico' + (on ? " on" : "") + '">' + icon(ic, 17) + '</span><span class="rt"><span class="t">' + title + '</span><span class="s">' + sub + "</span></span>" + CHEV + "</button>";
    const sec = (title, rows) => '<div class="sec"><div class="h"><div class="t">' + title + '</div></div><div class="list">' + rows + "</div></div>";
    const pref = themePref();

    let html = '<div class="phead"><div class="t">تنظیمات</div><div class="s">رفتار ربات در این گروه</div></div>';
    html += '<div class="sec"><div class="card"><div class="profile"><span class="avatar xl" style="' + avatarStyle(S.chat) + '">' + esc(initial(S.dash.chat.title)) + "</span>" +
      '<span class="rt"><span class="t">' + S.dash.chat.title + '</span><span class="s">' + (S.dash.viewer.is_owner ? "شما مالک هستید" : "شما ادمین هستید") + "</span></span>" +
      (health ? (health.bot_admin ? '<span class="pill ok">' + icon("check", 12, 3) + " ربات فعال</span>" : '<span class="pill bad">ربات ادمین نیست</span>') : "") + "</div>" +
      '<div class="row" style="border-top:1px solid var(--border)"><span class="rico">' + icon("key", 17) + '</span><span class="rt"><span class="t">شناسه گروه</span><span class="s" dir="ltr" style="text-align:right">' + S.chat + "</span></span>" +
      '<button class="ibtn" data-copy="' + S.chat + '" title="کپی" aria-label="کپی شناسه">' + icon("copy", 18) + "</button></div>" +
      (S.groups && S.groups.length > 1 ? '<button class="row" data-picker><span class="rico">' + icon("users", 17) + '</span><span class="rt"><span class="t">تغییر گروه</span><span class="s">' + fa(S.groups.length) + " گروه در دسترس</span></span>" + CHEV + "</button>" : "") +
      "</div></div>";

    let col1 = sec("ورود اعضا",
      sheetRow("cp", "key", "احراز هویت", isOn("cp_on") ? esc(feature("cp").effect()) : "خاموش", isOn("cp_on")) +
      pageRow("welcome", "hand", "پیام خوشامد", "متن و حذف خودکار", S.welcomeInfo && S.welcomeInfo !== "error" && !!S.welcomeInfo.text) +
      pageRow("rules", "scroll", "قوانین گروه", "با «قوانین» در گروه نمایش داده می شود") +
      pageRow("joingate", "door", "عضویت اجباری در کانال", "تا عضو کانال نشوند پیام نمی دهند") +
      flagRow("lb_on", "refresh", "بازگشت اعضای خارج شده", "کسی که بیرون رفت دوباره اضافه می شود"));
    col1 += sec("پیام های ربات",
      sheetRow("response-policy", "chat", "دیده شدن پیام ها", "خصوصی کردن اعلان ها و خوشامد") +
      sheetRow("nt", "bell", "اعلان حذف", isOn("nt_on") ? "روشن · پاک شدن پس از " + shown("nt_t") + " ثانیه" : "خاموش", isOn("nt_on")) +
      flagRow("rk_on", "star", "مقام خودکار", "با فعالیت، عنوان می گیرند") +
      pageRow("list:answer", "chat", "پاسخ خودکار", "کلمه و پاسخ آماده") +
      pageRow("list:cmd", "terminal", "دستور های سفارشی", "کلمه های خودتان برای بن و سکوت"));

    let col2 = sec("مدیران",
      (S.dash.viewer.is_owner
        ? sheetRow("lim", "userCog", "محدودیت مدیران", isOn("lim_on") ? "روشن · هر ادمین فقط کارهای مجازش" : "خاموش", isOn("lim_on"))
        : '<div class="row"><span class="rico">' + icon("userCog", 17) + '</span><span class="rt"><span class="t">محدودیت مدیران</span><span class="s">فقط مالک ربات تغییر می دهد · ' + (isOn("lim_on") ? "روشن" : "خاموش") + "</span></span></div>") +
      sheetRow("bt", "shieldOff", "ضد خیانت ادمین", isOn("bt_on") ? esc(feature("bt").effect()) : "خاموش", isOn("bt_on")) +
      '<button class="row" data-tab="members"><span class="rico">' + icon("users", 17) + '</span><span class="rt"><span class="t">فهرست ادمین ها</span><span class="s">مشاهده و عزل</span></span>' + CHEV + "</button>");
    col2 += sec("لاگ و گزارش",
      pageRow("log", "fileText", "کانال لاگ", S.logInfo && S.logInfo !== "error" && S.logInfo.channel ? "وصل" : "وصل نیست", S.logInfo && S.logInfo.channel) +
      pageRow("cases", "inbox", "پرونده ها", "گزارش ها و سابقه برخوردها") +
      sheetRow("dr", "send", "گزارش روزانه", sectionEnabled("dr") ? esc(feature("dr").effect()) : "خاموش", sectionEnabled("dr")));
    col2 += sec("ظاهر",
      '<div class="row" style="display:block;padding:12px"><div class="seg">' +
      [["auto", "monitor", "خودکار"], ["dark", "moon", "تیره"], ["light", "sun", "روشن"]].map(([id, ic, label]) =>
        '<button class="' + (pref === id ? "on" : "") + '" data-theme-set="' + id + '">' + icon(ic, 14) + " " + label + "</button>").join("") +
      '</div><div class="hint start">«خودکار» از پوسته تلگرام پیروی می کند.</div></div>');
    col2 += sec("راهنما",
      sheetRow("help", "book", "دستورهای داخل گروه", "کارهایی که با ریپلای روی پیام انجام می شود") +
      sheetRow("rights-help", "shieldCheck", "دسترسی های لازم ربات", health && !health.issues.some((i) => i.fix === "rights") ? "همه دسترسی ها داده شده" : "راهنمای ادمین کردن ربات", health && !health.issues.some((i) => i.fix === "rights")));
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
    cases: () => api("/cases?status=open").then((data) => { data.status = "open"; data.user = ""; return data; }),
  };

  async function pushPage(id) {
    if (id === "settings") return goTab("settings");
    const page = { id, data: null, error: null };
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
    const page = currentPage();
    if (page && DOCS[page.id] && docDirty(page)) {
      toast("اول تغییرات را ذخیره یا رها کنید.", "warn");
      return;
    }
    S.stack = [];
    setClosingConfirmation(false);
    return pushPage(id);
  }

  async function loadPage(page) {
    const chat = S.chat;
    const keep = page.data;
    page.error = null;
    try {
      if (page.id.startsWith("list:")) {
        const kind = page.id.slice(5);
        const [d, mode] = await Promise.all([
          api("/lists/" + kind),
          kind === "filter" ? api("/filter-words").catch(() => null) : Promise.resolve(null),
        ]);
        page.data = d;
        page.mode = mode;
        S.lists = S.lists || {};
        S.lists[kind] = page.data;
      } else {
        page.data = await PAGE_LOADERS[page.id]();
        if (page.id === "locks") S.lockCache = page.data;
        if (page.id === "rights") S.rights = page.data;
        if (page.id === "log") S.logInfo = page.data;
        if (page.id === "welcome") S.welcomeInfo = page.data;
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
  const CASE_ACTION = { none: "بدون اقدام", delete: "حذف پیام", warn: "اخطار", mute: "سکوت", ban: "بن", kick: "اخراج", unmute: "رفع سکوت", unban: "رفع بن" };
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
    return '<button class="row" data-case="' + c.id + '"><span class="rico ' + (c.status === "open" ? "warn" : "") + '">' + icon(c.source === "report" ? "alert" : "fileText", 16) + '</span><span class="rt"><span class="t">' + esc(c.subject_name || "کاربر نامشخص") + '</span><span class="s">' + esc(c.reason) + " · " + ago(c.created_at) + '</span></span><span class="pill ' + st[1] + '">' + esc(st[0]) + "</span></button>";
  }

  function renderCasesPage(page) {
    const d = page.data;
    let html = renderPageTop(d && d.detail ? "پرونده " + fa(d.detail.id) : "پرونده ها", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!d) return html + loadingList() + "</div>";
    if (d.detail) {
      const c = d.detail;
      const st = CASE_STATUS[c.status] || [c.status, "mute"];
      html += '<div class="sec first"><div class="card"><div class="profile"><span class="rico ' + (st[1] === "warn" ? "warn" : "ok") + '" style="width:48px;height:48px;border-radius:15px">' + icon("fileText", 22) + '</span><span class="rt"><span class="t wrap">' + esc(c.subject_name || "کاربر نامشخص") + '</span><span class="s wrap">' + esc(c.reason) + '</span></span><span class="pill ' + st[1] + '">' + esc(st[0]) + "</span></div>" +
        '<dl class="kv" style="border-top:1px solid var(--border);margin:0">' +
        "<dt>اقدام</dt><dd>" + esc(caseAction(c.action)) + "</dd>" +
        "<dt>منبع</dt><dd>" + esc(CASE_SOURCE[c.source] || c.source || "—") + "</dd>" +
        (c.subject ? '<dt>شناسه کاربر</dt><dd dir="ltr" style="text-align:left">' + esc(c.subject) + "</dd>" : "") +
        (c.media_kind ? "<dt>نوع پیام</dt><dd>" + esc(c.media_kind) + "</dd>" : "") +
        "<dt>زمان</dt><dd>" + esc(ago(c.created_at)) + "</dd>" +
        "</dl></div>";
      if (c.evidence) html += '<div class="h" style="padding-top:16px"><div class="t">متن پیام</div></div><div class="evidence">' + esc(c.evidence) + "</div>";
      html += "</div>";
      const events = c.events || [];
      if (events.length) {
        html += '<div class="sec"><div class="h"><div class="t">تاریخچه</div></div><div class="card timeline">' + events.map((ev) => {
          const k = CASE_EVENT[ev.kind] || [ev.kind, "", "info"];
          return '<div class="tl"><span class="tdot ' + k[1] + '">' + icon(k[2], 12, 2.6) + '</span><div class="tb"><div class="tt">' + esc(k[0]) + (ev.action ? " · " + esc(caseAction(ev.action)) : "") + '</div><div class="ts">' + esc(ev.actor_name && !/^\d+$/.test(ev.actor_name) ? ev.actor_name : ev.actor ? "ادمین" : "ربات") + " · " + esc(ago(ev.created_at)) + (ev.note ? " · " + esc(ev.note) : "") + "</div></div></div>";
        }).join("") + "</div></div>";
      }
      html += '<div class="sec"><div class="h"><div class="t">رسیدگی</div></div><textarea class="text-input" id="case-note" maxlength="500" placeholder="یادداشت اختیاری؛ در تاریخچه ثبت می شود" style="min-height:84px"></textarea><div class="btns" style="padding-top:10px">';
      if (c.status === "open") html += '<button class="btn ghost" data-case-action="none">' + icon("check", 16) + ' بررسی شد</button><button class="btn danger" data-case-action="delete">' + icon("trash", 16) + " حذف پیام</button>";
      if (c.status === "resolved" && ["warn", "mute", "ban"].includes(c.action)) html += '<button class="btn soft-danger" data-case-action="reverse">لغو ' + esc(caseAction(c.action)) + "</button>";
      html += '<button class="btn outline" data-case-note>ثبت یادداشت</button></div></div></div>';
      return html;
    }
    const status = d.status || "open";
    html += '<div class="sec first"><div class="seg">' +
      [["open", "باز"], ["resolved", "رسیدگی شده"], ["all", "همه"]].map(([id, label]) => '<button class="' + (status === id ? "on" : "") + '" data-case-filter="' + id + '">' + label + "</button>").join("") +
      '</div><label class="search" style="margin-top:10px">' + icon("search", 16) + '<input id="case-user" inputmode="numeric" enterkeyhint="search" placeholder="جستجو با شناسه عددی کاربر" value="' + esc(d.user || "") + '" /></label></div>';
    html += '<div class="sec">';
    if (!(d.cases || []).length) html += '<div class="card"><div class="empty sm"><div class="glyph">' + icon("shieldCheck", 26) + '</div><div class="t">پرونده ای نیست</div><div class="s">' + (status === "open" ? "همه گزارش ها رسیدگی شده است." : "چیزی پیدا نشد.") + "</div></div></div>";
    else html += '<div class="list">' + d.cases.map(caseRow).join("") + "</div>";
    if (d.has_more && d.cases.length) html += '<button class="btn ghost block" style="margin-top:12px" data-case-more="' + d.cases[d.cases.length - 1].id + '">بیشتر</button>';
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

  function lockTile(l) {
    return '<button class="lock' + (l.on ? " on" : "") + '" data-lock="' + esc(l.key) + '" aria-pressed="' + (l.on ? "true" : "false") + '"><span class="li">' + icon(l.icon || (l.on ? "LOCKED" : "UNLOCKED"), 14, 2.4) + '</span><span class="ln">' + l.label + '</span><span class="ls"></span></button>';
  }

  function renderLocksPage(page) {
    const sum = S.dash.locks_summary;
    let html = renderPageTop("قفل ها", fa(sum.active) + " از " + fa(sum.total) + " روشن") + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const q = fold(S.lockQuery);
    const view = S.lockView;
    html += '<div class="sec first">' + searchBox("lock-search", "جستجوی قفل…", S.lockQuery) +
      '<div style="display:flex;gap:8px;margin-top:10px"><div class="seg" style="flex:1">' +
      [["all", "همه"], ["on", "روشن"], ["off", "خاموش"]].map(([id, label]) => '<button class="' + (view === id ? "on" : "") + '" data-lock-view="' + id + '">' + label + "</button>").join("") +
      '</div></div><div class="btns" style="margin-top:10px"><button class="btn sm ghost" data-locks-all="1">' + icon("lock", 14) + ' همه روشن</button><button class="btn sm soft-danger" data-locks-all="0">' + icon("unlock", 14) + " همه خاموش</button></div></div>";
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
    html += body || '<div class="empty sm"><div class="glyph">' + icon("search", 24) + '</div><div class="t">قفلی با این مشخصات نیست</div></div>';
    const b = page.data.botOptions;
    if (b && b.items && !q) {
      html += '<div class="sec"><div class="h"><div class="t">رفتار قفل ربات</div></div><div class="list">' +
        b.items.filter((it) => it.key !== "bot").map((it) => '<div class="row"><span class="rt"><span class="t n">' + esc(it.label) + "</span></span>" + swb('data-botopt="' + esc(it.key) + '"', it.on, it.label) + "</div>").join("") +
        "</div></div>";
    }
    const ab = page.data.allowedBots;
    if (ab && ab.bots && !q) {
      html += '<div class="sec"><div class="h"><div class="t">ربات های مجاز <span class="count">· ' + fa(ab.bots.length) + '</span><span class="sub">قفل ربات این ها را حذف نمی کند</span></div></div>' +
        '<div class="addrow"><input class="text-input" id="allow-bot" dir="ltr" autocapitalize="off" autocomplete="off" enterkeyhint="done" placeholder="@examplebot" /><button class="btn" data-allow-bot>' + icon("plus", 16) + " افزودن</button></div>" +
        (ab.bots.length ? '<div class="list" style="margin-top:10px">' + ab.bots.map((b) => '<div class="row compact"><span class="rico sm on">' + icon("bot", 15) + '</span><span class="rt"><span class="t n" dir="ltr" style="text-align:right">' + (b.name || b.id) + '</span></span><button class="ibtn sm danger" data-disallow-bot="' + esc(b.id) + '" aria-label="حذف">' + icon("x", 17) + "</button></div>").join("") + "</div>" : "") +
        "</div>";
    }
    html += '<div class="hint">ادمین ها و کاربران ویژه از قفل ها معاف اند. قفل پک استیکر: در گروه روی استیکر «قفل پک».</div>';
    return html + "</div>";
  }

  function renderRightsPage(page) {
    let html = renderPageTop("دسترسی اعضا", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const rights = page.data.rights;
    const open = rights.filter((r) => r.open).length;
    const allClosed = open === 0;
    html += '<div class="sec first"><div class="note' + (allClosed ? " warn" : " mute") + '">' + icon(allClosed ? "lock" : "info", 16) + "<span>" +
      (allClosed ? "گروه بسته است: هیچ عضو عادی نمی تواند چیزی بفرستد." : "هر دسترسی که بسته شود برای همه اعضای عادی بسته می شود؛ ادمین ها استثنا هستند.") + "</span></div>" +
      (page.data.pending ? '<div class="note warn" style="margin-top:8px">' + icon("clock", 16) + "<span>تغییری در صف اعمال است و تا چند لحظه دیگر در تلگرام دیده می شود.</span></div>" : "") +
      '<div class="btns" style="margin-top:12px">' +
      '<button class="btn soft-danger" data-rights-all="0"' + (allClosed ? " disabled" : "") + ">" + icon("lock", 15) + " بستن گروه</button>" +
      '<button class="btn ghost" data-rights-all="1"' + (open === rights.length ? " disabled" : "") + ">" + icon("unlock", 15) + " باز کردن همه</button></div></div>";
    html += '<div class="sec"><div class="h"><div class="t">اعضای عادی می توانند <span class="count">· ' + fa(open) + " از " + fa(rights.length) + '</span></div></div><div class="list">' +
      rights.map((r) =>
        '<div class="row"><span class="rico sm' + (r.open ? " on" : "") + '">' + icon(r.icon || (r.open ? "UNLOCKED" : "LOCKED"), 15) + '</span><span class="rt"><span class="t n">' + r.label + '</span></span><button class="swb" data-right="' + esc(r.key) + '" role="switch" aria-checked="' + (r.open ? "true" : "false") + '"><span class="sw' + (r.open ? " on" : "") + '"></span></button></div>'
      ).join("") + "</div></div>";
    html += '<div class="hint">بستن موقت گروه: در گروه «قفل گروه ۲ ساعت». قفل شب در «امکانات».</div>';
    return html + "</div>";
  }

  function renderLogPage(page) {
    let html = renderPageTop("کانال لاگ", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    const on = d.kinds.filter((k) => k.on).length;
    html += '<div class="sec first"><div class="card"><div class="profile"><span class="rico' + (d.channel ? " ok" : " warn") + '" style="width:48px;height:48px;border-radius:15px">' + icon("fileText", 22) + '</span><span class="rt"><span class="t">' + (d.channel ? "کانال وصل است" : "کانالی وصل نیست") + '</span><span class="s"' + (d.channel ? ' dir="ltr" style="text-align:right"' : "") + ">" +
      (d.channel ? d.channel : "رویدادها جایی ثبت نمی شود") + "</span></span>" + (d.channel ? '<span class="pill ok">وصل</span>' : '<span class="pill warn">ندارد</span>') + "</div></div>" +
      '<div class="field"><div class="fl"><span>' + (d.channel ? "تغییر کانال" : "وصل کردن کانال") + '</span></div><div class="addrow"><input class="text-input" id="log-channel" dir="ltr" autocapitalize="off" autocomplete="off" enterkeyhint="done" placeholder="@channel" /><button class="btn" data-log-connect>' + (d.channel ? "تغییر" : "وصل کن") + "</button></div>" +
      '<div class="hint start">اول ربات را در کانال ادمین کنید. ربات یک پیام آزمایشی در کانال می فرستد. برای کانال خصوصی: یک پیام کانال را در گروه فوروارد کنید و روی آن «تنظیم لاگ» بزنید.</div></div></div>';
    html += '<div class="sec"><div class="h"><div class="t">چه رویدادهایی ثبت شود <span class="count">· ' + fa(on) + " از " + fa(d.kinds.length) + "</span></div></div>" +
      '<div class="list">' +
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
      '<textarea class="text-input" id="doc-text" placeholder="' + esc(doc.placeholder) + '">' + esc(page.draft || "") + "</textarea>" +
      (doc.tags.length ? '<div class="chips" style="margin-top:10px">' + doc.tags.map(([tag, label]) => '<button class="chip sm" data-insert="' + esc(tag) + '">' + icon("plus", 12) + label + "</button>").join("") + "</div>" +
        '<div class="hint start">روی هر برچسب بزنید تا در متن گذاشته شود.' + (d.has_media ? " · رسانه همراه فقط از داخل گروه عوض می شود" : " · برای خوشامد با عکس: در گروه روی عکس ریپلای کنید و «تنظیم خوشامد» بفرستید.") + "</div>" : '<div class="hint start">اعضا با فرستادن «قوانین» در گروه این متن را می بینند.</div>') + "</div>" +
      (has ? '<div style="padding-top:14px"><button class="btn soft-danger block" data-confirm="' + doc.off + '">' + icon("trash", 16) + " حذف " + doc.title + "</button></div>" : "") +
      "</div></div>";
    html += '<div class="col"><div class="sec"><div class="h"><div class="t">پیش نمایش</div>' + (d.has_media ? '<span class="pill mute">' + icon("image", 12) + " همراه رسانه</span>" : "") + '</div><div class="chatbg"><div class="preview" id="doc-preview"></div></div></div>';
    const wct = page.id === "welcome" ? setting("wct") : null;
    if (wct) html += '<div class="sec"><div class="h"><div class="t">حذف خودکار پیام خوشامد</div></div><div class="card pad">' + renderNumberField(wct, true) + "</div></div>";
    html += "</div></div>";
    html += '<div class="savebar" id="doc-bar" hidden><span class="s"><span class="dot warn"></span>تغییرات ذخیره نشده</span><button class="btn ghost sm" data-doc-revert>برگرداندن</button><button class="btn sm" data-doc-save>' + icon("check", 16) + " ذخیره</button></div>";
    return html + "</div>";
  }

  function renderJoinGatePage(page) {
    let html = renderPageTop("عضویت اجباری در کانال", S.dash.chat.title) + '<div id="main" class="nobar">';
    if (!page.data) return html + loadingList() + "</div>";
    const d = page.data;
    html += '<div class="sec first">' +
      (d.channel ? '<div class="list" style="margin-bottom:12px"><div class="row"><span class="rico on">' + icon("door", 17) + '</span><span class="rt"><span class="t" dir="ltr" style="text-align:right">' + d.channel + '</span><span class="s">فعال · تا عضو نشوند پیام نمی دهند</span></span><span class="pill ok">روشن</span></div></div>' : "") +
      '<div class="note mute">' + icon("info", 16) + "<span>تا وقتی عضو کانال نشوند نمی توانند پیام بدهند. ربات باید در آن کانال ادمین باشد.</span></div>" +
      '<div class="field"><div class="fl"><span>یوزرنیم یا لینک کانال</span></div><input class="text-input" id="join-gate-input" dir="ltr" autocapitalize="off" autocomplete="off" enterkeyhint="done" placeholder="@channel" value="' + esc(decode(d.channel || "")) + '" /></div>' +
      '<div class="btns" style="padding-top:12px"><button class="btn" data-joingate-save>' + icon("check", 16) + " ذخیره</button>" +
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
        return '<div class="row"><span class="rico' + (live ? " on" : "") + '">' + icon("image", 17) + '</span><span class="rt"><span class="t">' + label + '</span><span class="s' + (live ? " on" : "") + '">' + (live ? "فعال · حذف می کند" : "فقط بررسی · حذف نمی کند") + "</span></span>" +
          swb('data-filter-toggle="' + esc(e.key) + '"', live, label) +
          (pending
            ? '<button class="btn sm danger" data-remove="imgf" data-key="' + esc(e.key) + '">حذف؟</button>'
            : '<button class="ibtn sm danger" data-remove="imgf" data-key="' + esc(e.key) + '" aria-label="حذف">' + icon("x", 17) + "</button>") +
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
    html += '<div class="sec first"><div class="addrow"><input class="text-input" id="voice-word" maxlength="64" enterkeyhint="done" placeholder="کلمه تازه" /><button class="btn" data-voice-add>' + icon("plus", 16) + " افزودن</button></div>" +
      (d.words.length > 8 ? '<div style="margin-top:10px">' + searchBox("list-search", "جستجو در " + fa(d.words.length) + " کلمه…", S.listQuery) + "</div>" : "") + "</div>";
    html += '<div class="sec"><div class="h"><div class="t">فهرست <span class="count">· ' + fa(d.words.length) + "</span></div>" +
      (d.words.length ? '<button class="more" style="color:var(--danger)" data-confirm="voice-clear">پاک کردن همه</button>' : "") + "</div>";
    if (!d.words.length) html += '<div class="card"><div class="empty sm"><div class="glyph">' + icon("mic", 24) + '</div><div class="t">فهرست خالی است</div></div></div>';
    else {
      html += '<div class="list">' + d.words.map((w) =>
        '<div class="row compact"' + (q && !fold(w.word).includes(q) ? " hidden" : "") + '><span class="rt"><span class="t n">' + esc(w.word) + '</span></span><button class="ibtn sm danger" data-voice-remove="' + esc(w.key) + '" aria-label="حذف">' + icon("x", 17) + "</button></div>"
      ).join("") + "</div>";
    }
    html += "</div>";
    if (d.disabled_defaults.length) html += '<div class="sec"><button class="btn ghost block" data-voice-restore>' + icon("refresh", 15) + " بازگرداندن " + fa(d.disabled_defaults.length) + " کلمه پیش فرض</button></div>";
    return html + "</div>";
  }

  const LIST_SEARCH_THRESHOLD = 8;

  function renderListPage(page, kind) {
    const [title, ic, desc] = LIST_KINDS[kind];
    const d = page.data;
    const right = d && d.entries.length ? '<button class="ibtn danger" data-confirm="clear:' + kind + '" title="پاکسازی لیست" aria-label="پاکسازی لیست">' + icon("trash", 20) + "</button>" : "";
    let html = renderPageTop(title, d ? fa(d.entries.length) + (d.truncated ? "+" : "") + " مورد" : S.dash.chat.title, right) + '<div id="main" class="nobar">';
    if (!d) return html + loadingList() + "</div>";
    const q = fold(S.listQuery);
    html += '<div class="sec first">';
    if (kind === "filter") {
      const exact = page.mode && page.mode.exact;
      html += '<div class="addrow"><input class="text-input" id="filter-word" maxlength="64" enterkeyhint="done" placeholder="کلمه تازه، مثلا «تبلیغ»" /><button class="btn" data-word-add>' + icon("plus", 16) + " افزودن</button></div>" +
        (page.mode ? '<div class="field"><div class="fl"><span>کلمه کجا پیدا شود</span></div><div class="seg">' +
          '<button class="' + (exact ? "" : "on") + '" data-word-exact="0">هر جای متن</button><button class="' + (exact ? "on" : "") + '" data-word-exact="1">فقط کلمه جدا</button></div>' +
          '<div class="hint start">' + (exact ? "«بت» در «صحبت» پیدا نمی شود؛ فقط وقتی جدا آمده باشد." : "«بت» در «صحبت» هم پیدا می شود؛ برای کلمه های کوتاه «فقط کلمه جدا» را بزنید.") + "</div></div>" : "");
    } else if (kind === "answer") {
      html += '<div class="card pad"><div class="fl" style="font-size:13px;font-weight:600;margin-bottom:10px">پاسخ تازه</div>' +
        '<input class="text-input" id="answer-trigger" maxlength="64" placeholder="وقتی کسی نوشت… مثلا «سلام»" />' +
        '<textarea class="text-input" id="answer-text" maxlength="3500" placeholder="ربات جواب بدهد… مثلا «درود، خوش آمدی»" style="min-height:84px;margin-top:8px"></textarea>' +
        '<button class="btn block" style="margin-top:10px" data-answer-add>' + icon("plus", 16) + " افزودن پاسخ</button>" +
        '<div class="hint start">مخاطب پاسخ ها: ' + esc(pickLabel("an_act")) + ' · <button class="more" data-open="an">تغییر</button> · پاسخ با عکس: در گروه روی پیام «تنظیم پاسخ ‹کلمه›».</div></div>';
    } else if (LIST_HOWTO[kind]) {
      html += '<div class="note mute">' + icon("info", 16) + "<span>" + LIST_HOWTO[kind] + "</span></div>";
    }
    if (d.entries.length > LIST_SEARCH_THRESHOLD) {
      html += '<div style="margin-top:10px">' + searchBox("list-search", "جستجو در " + fa(d.entries.length) + " مورد…", S.listQuery) + "</div>";
    }
    html += "</div>";
    html += '<div class="sec">';
    if (!d.entries.length) html += '<div class="card"><div class="empty sm"><div class="glyph">' + icon(ic, 26, 1.8) + '</div><div class="t">لیست خالی است</div><div class="s">' + (desc || "") + "</div></div></div>";
    else {
      html += '<div class="list">' + d.entries.map((e) => {
        const pending = S.pending === kind + ":" + e.key;
        const hide = q && !fold(decode(e.name)).includes(q);
        return '<div class="row"' + (hide ? " hidden" : "") + '><span class="rico sm">' + icon(ic, 15) + '</span><span class="rt"><span class="t n">' + e.name + "</span></span>" +
          (pending
            ? '<button class="btn sm danger" data-remove="' + kind + '" data-key="' + esc(e.key) + '">حذف؟</button>'
            : '<button class="ibtn sm danger" data-remove="' + kind + '" data-key="' + esc(e.key) + '" aria-label="حذف">' + icon("x", 17) + "</button>") +
          "</div>";
      }).join("") + "</div>";
      if (d.truncated) html += '<div class="hint">بیش از ' + fa(d.entries.length) + " مورد؛ برای یافتن مورد خاص جستجو کنید.</div>";
    }
    return html + "</div></div>";
  }

  function openSheet(fid) {
    const f = feature(fid) || EXTRA_SHEETS[fid];
    if (!f) return;
    if (f.page) {
      pushPage(f.page);
      return;
    }
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
      const first = sheetEl.querySelector("button, input");
      if (first && WIDE.matches) first.focus({ preventScroll: true });
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
        if (S.dash && !S.palette && !S.confirm) render();
        else sheet.innerHTML = "";
      }, 240);
    }
    syncBack();
  }

  const EXTRA_SHEETS = {
    lim: { id: "lim", icon: "userCog", title: "محدودیت مدیران", desc: "با روشن بودن، هر ادمین فقط کارهایی را می تواند بکند که اینجا مجاز است", flag: "lim_on", sections: ["lim"], custom: "lim", load: "/limits" },
    nt: { id: "nt", icon: "bell", title: "اعلان حذف", desc: "وقتی پیامی حذف شد، به فرستنده اش گفته می شود چرا", flag: "nt_on", sections: ["nt"] },
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
    if (f.flag) return swb('data-apply="' + f.flag + '"', isOn(f.flag), f.title);
    if (f.enabled) return swb('data-power="' + f.id + '"', sectionEnabled(f.enabled), f.title);
    return "";
  }

  function sheetLoading(h) {
    return '<div class="sk" style="height:' + (h || 120) + 'px;border-radius:18px;margin-top:12px"></div>';
  }

  function renderSheetBody(f) {
    const st = featureState(f);
    let html = '<div class="handle"></div><div class="sheet-head"><span class="rico' + (st ? " on" : "") + '">' + icon(f.icon, 22) + '</span><div style="flex:1;min-width:0"><div class="t">' + f.title + '</div><div class="s">' + f.desc + "</div></div>" + masterControl(f) +
      '<button class="ibtn sheet-close" data-close aria-label="بستن">' + icon("x", 20) + "</button></div>";

    if (st === true && f.effect && !f.always) {
      html += '<div class="status-line on"><span class="dot on"></span><span>روشن · ' + esc(f.effect()) + "</span></div>";
    } else if (st === true && f.effect) {
      html += '<div class="status-line"><span class="dot"></span><span>' + esc(f.effect()) + "</span></div>";
    } else if (st === false && f.enabled) {
      html += '<div class="status-line"><span class="dot"></span><span>خاموش · با انتخاب یک ساعت یا کلید بالا روشن می شود.</span></div>';
    } else if (st === false && (f.flag || f.flags)) {
      html += '<div class="status-line"><span class="dot"></span><span>خاموش · تنظیمات زیر با روشن کردن کلید اعمال می شود.</span></div>';
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
    if (fields) html += '<div class="sheet-card">' + fields + "</div>";

    const custom = SHEET_BLOCKS[f.custom];
    if (custom) html += custom(f);
    if (f.tip) html += '<div class="note mute" style="margin-top:12px">' + icon("info", 16) + "<span>" + f.tip + "</span></div>";

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
      if (d === "error") return "";
      return '<div class="h" style="padding-top:16px"><div class="t">موضوع ها <span class="count">· ' + fa(d.ai.filter((l) => l.on).length) + " از " + fa(d.ai.length) + '</span></div></div><div class="lockgrid">' +
        d.ai.map(lockTile).join("") + "</div>" +
        '<div class="list" style="margin-top:12px"><button class="row" data-push="imgf"><span class="rico">' + icon("image", 17) + '</span><span class="rt"><span class="t">فیلتر تصویری شما</span><span class="s">موضوع های دلخواه</span></span>' + CHEV + "</button></div>";
    },
    response_policy: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(180);
      if (d === "error") return "";
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
    report: () => '<button class="btn outline block" style="margin-top:12px" data-report-test>' + icon("send", 16) + " ارسال آزمایشی گزارش امروز در گروه</button>",
    strict: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(110);
      if (d === "error") return "";
      if (!d.causes.length) return '<div class="note mute" style="margin-top:12px">' + icon("info", 16) + "<span>هیچ قفل یا فیلتری روشن نیست؛ اول یکی را روشن کنید تا اینجا بیاید.</span></div>";
      return '<div class="sheet-card"><div class="field"><div class="fl"><span>چه چیزهایی تخلف حساب شود</span><b>' + fa(d.causes.filter((c) => c.on).length) + " از " + fa(d.causes.length) + '</b></div><div class="chips">' +
        d.causes.map((c) => '<button class="chip' + (c.on ? " on" : "") + '" data-cause="' + esc(c.key) + '" aria-pressed="' + c.on + '">' + (c.on ? icon("check", 13, 3) : "") + esc(c.label) + "</button>").join("") +
        '</div><div class="hint start">فقط موارد انتخاب شده به شمارش تخلف اضافه می شوند؛ دست کم یکی همیشه می ماند.</div></div></div>';
    },
    tmed: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(90);
      if (d === "error") return "";
      return '<div class="sheet-card"><div class="field"><div class="fl"><span>کدام رسانه ها موقت باشند</span><b>' + fa(d.kinds.filter((k) => k.on).length) + " از " + fa(d.kinds.length) + '</b></div><div class="chips">' +
        d.kinds.map((k) => '<button class="chip' + (k.on ? " on" : "") + '" data-tmed-kind="' + esc(k.name) + '" aria-pressed="' + k.on + '">' + (k.on ? icon("check", 13, 3) : "") + esc(k.label) + "</button>").join("") + "</div></div></div>";
    },
    lim: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(220);
      if (d === "error") return "";
      return '<div class="h" style="padding-top:16px"><div class="t">ادمین ها مجازند <span class="count">· ' + fa(d.caps.filter((c) => c.allowed).length) + " از " + fa(d.caps.length) + "</span></div></div>" +
        (isOn("lim_on") ? "" : '<div class="note mute" style="margin-bottom:10px">' + icon("info", 16) + "<span>تا کلید بالا روشن نشود، همه ادمین ها همه کار می توانند بکنند.</span></div>") +
        '<div class="list">' + d.caps.map((c) => '<div class="row"><span class="rt"><span class="t n">' + esc(c.label) + "</span></span>" +
          (d.can_edit ? swb('data-cap="' + esc(c.name) + '"', c.allowed, c.label) : '<span class="pill ' + (c.allowed ? "ok" : "mute") + '">' + (c.allowed ? "مجاز" : "بسته") + "</span>") + "</div>").join("") + "</div>" +
        '<div class="hint start">این محدودیت ها روی همه ادمین ها به جز مالک اعمال می شود.</div>';
    },
    slow: () => {
      const d = S.sheet && S.sheet.data;
      if (!d) return sheetLoading(110);
      if (d === "error") return "";
      return '<div class="sheet-card"><div class="field"><div class="fl"><span>فاصله بین دو پیام هر عضو</span><b>' + slowLabel(d.seconds) + '</b></div><div class="chips">' +
        d.steps.map((s) => '<button class="chip' + (s.value === d.seconds ? " on" : "") + (s.value === 0 ? " danger" : "") + '" data-slow="' + s.value + '">' + fa(s.label) + "</button>").join("") + "</div></div></div>" +
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

  function renderNumberField(it, standalone) {
    const chips = (it.presets || []).map((p) =>
      '<button class="chip' + (p.value === it.value ? " on" : "") + '" data-apply="' + it.id + ":" + p.value + '">' + fa(p.shown) + "</button>"
    ).join("");
    const custom = it.clock
      ? '<div class="inline-num"' + (chips ? "" : ' style="margin-top:0"') + '><input type="time" class="text-input" dir="ltr" value="' + minutesToClock(it.value) + '" data-clock-apply="' + it.id + '" data-clock-value="' + it.value + '" aria-label="' + esc(it.label) + '" /></div>'
      : "";
    const numChip = it.clock
      ? ""
      : '<input type="number" class="chip-input" inputmode="numeric" enterkeyhint="done" min="' + it.range[0] + '" max="' + it.range[1] + '" value="' + ((it.presets || []).some((p) => p.value === it.value) ? "" : it.value) + '" data-number-apply="' + it.id + '" data-value="' + it.value + '" placeholder="دلخواه" aria-label="مقدار دلخواه ' + esc(it.label) + '" />';
    return '<div class="field"' + (standalone ? ' style="padding-top:0"' : "") + '><div class="fl"><span>' + (it.icon ? icon(it.icon, 14) + " " : "") + (it.clock ? it.label.split("،")[0] : it.label) + "</span><b>" + fa(it.shown) + "</b></div>" +
      '<div class="chips">' + chips + numChip + "</div>" + custom + "</div>";
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
      app.innerHTML = '<aside id="rail"></aside><div id="view">' + renderTop(true) + '<div id="main" class="nobar"></div></div>' + renderOverlays();
      document.body.classList.add("nobar");
      openSheetAnimation();
    }
    syncBack();
  }

  function pickerHtml() {
    const q = fold(S.groupQuery);
    const groups = S.groups || [];
    return '<div class="handle"></div><div class="sheet-head"><span class="rico on">' + icon("users", 22) + '</span><div style="flex:1;min-width:0"><div class="t">گروه های شما</div><div class="s">' + fa(groups.length) + " گروه که در آن ادمین هستید</div></div>" +
      '<button class="ibtn sheet-close" data-close aria-label="بستن">' + icon("x", 20) + "</button></div>" +
      (groups.length > 6 ? '<div style="margin-bottom:10px">' + searchBox("group-search", "جستجوی گروه…", S.groupQuery) + "</div>" : "") +
      '<div class="list">' +
      groups.map((g) => {
        const current = g.id === S.chat;
        const hide = q && !fold(decode(g.title)).includes(q);
        return '<button class="row tap" data-pick-group="' + g.id + '"' + (g.known ? "" : " disabled") + (hide ? " hidden" : "") + ' style="min-height:62px' + (g.known ? "" : ";opacity:.55") + '">' +
          '<span class="avatar lg" style="' + avatarStyle(g.id) + '">' + esc(initial(g.title)) + "</span>" +
          '<span class="rt"><span class="t">' + g.title + '</span><span class="s">' + (g.is_owner ? "مالک" : "ادمین") + (g.known ? "" : " · ربات در دسترس نیست") + "</span></span>" +
          (current ? '<span class="pill accent">' + icon("check", 12, 3) + " فعلی</span>" : g.known ? CHEV : '<span class="pill bad">بدون دسترسی</span>') +
          "</button>";
      }).join("") +
      "</div>" +
      '<div class="note mute" style="margin-top:12px">' + icon("info", 16) + "<span>گروه دیگری دارید؟ ربات را در آن ادمین کنید تا اینجا اضافه شود.</span></div>";
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
    TABS.forEach(([id, ic, label]) => items.push({ group: "صفحه ها", icon: ic, title: label, sub: "زبانه", go: () => goTab(id) }));
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
    if (q) items = items.filter((it) => fold(decode(it.title) + " " + decode(it.sub)).includes(q));
    else items = items.filter((it) => it.group === "صفحه ها" || it.group === "امکانات").slice(0, 16);
    return items.slice(0, 40);
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
    if (!S.dash) return;
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
    S.tab = id;
    S.stack = [];
    S.barSel = null;
    S.sheet = null;
    if (id === "members" && !S.admins) loadAdmins();
    render();
    window.scrollTo(0, 0);
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
      return {
        title: "همه " + fa(n) + " مورد از «" + LIST_KINDS[kind][0] + "» حذف شود؟",
        sub: "برگشت ندارد." + (kind === "ban" ? " کسانی که بن شده اند می توانند دوباره بیایند." : ""),
        label: "حذف همه",
        run: () => api("/lists/" + kind, { method: "DELETE" }).then(thenReload),
      };
    }
    if (key === "locks-off") {
      return { title: "همه قفل ها خاموش شود؟", sub: "هر چیزی دوباره در گروه فرستاده می شود تا قفل ها را برگردانید.", label: "خاموش کردن همه", quiet: true, run: () => setAllLocks(false).then(() => toast("همه قفل ها خاموش شد.", "ok")) };
    }
    if (key === "rights-close") {
      return { title: "گروه بسته شود؟", sub: "هیچ عضو عادی نمی تواند چیزی بفرستد؛ ادمین ها می توانند. هر وقت خواستید با همین دکمه باز کنید.", label: "بستن گروه", quiet: true, run: () => setAllRights(false) };
    }
    return null;
  }

  function confirmAdmin(id, name) {
    return {
      title: "«" + decode(name) + "» عزل شود؟",
      sub: "از ادمینی گروه و ربات برداشته می شود. ربات فقط ادمین هایی را می تواند عزل کند که خودش اضافه کرده.",
      label: "عزل",
      run: () => api("/admins/" + id, { method: "DELETE" }).then((d) => loadAdmins().then(() => d)),
    };
  }

  function renderConfirm() {
    const c = S.confirm;
    if (!c) return "";
    return '<div id="confirm-dim" data-confirm-cancel></div><div class="confirm" role="alertdialog" aria-modal="true"><div class="ci">' + icon("alert", 22) + '</div><div class="t">' + esc(c.title) + '</div><div class="s">' + esc(c.sub) + '</div><div class="b"><button class="btn ghost" data-confirm-cancel>انصراف</button><button class="btn danger" data-confirm-run>' + esc(c.label) + "</button></div></div>";
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
      return { msg: it.label + " ← " + fa(shownVal), undo: id + ":" + it.value };
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
      const ok = await write(async () => {
        await takeDash(await api("/settings/apply", { method: "POST", body: { action } }));
      });
      redraw();
      if (!ok) return false;
      haptic("sel");
      if (change && !(opts && opts.quiet)) {
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
      const ok = await write(async () => {
        await takeDash(await api("/locks/" + encodeURIComponent(key) + "/toggle", { method: "POST" }));
      });
      if (!ok) return redraw();
      haptic("sel");
      setLockCaches([key], (x) => !x.on);
      redraw();
      if (S.palette) drawPalette();
      if (l && !(opts && opts.quiet)) {
        toast("قفل " + decode(l.label) + (l.on ? " روشن شد" : " خاموش شد"), "ok", { label: "برگرداندن", run: () => toggleLock(key, { quiet: true }) });
      }
    });
  }

  async function setLocks(keys, on, opts) {
    if (!keys.length) return;
    return guarded("locks:" + keys.join(","), async () => {
      const ok = await write(async () => {
        await takeDash(await api("/locks/set", { method: "POST", body: { keys, on } }));
      });
      if (!ok) return render();
      haptic("ok");
      setLockCaches(keys, on);
      render();
      if (!(opts && opts.quiet)) {
        toast(fa(keys.length) + " قفل " + (on ? "روشن شد" : "خاموش شد"), "ok", { label: "برگرداندن", run: () => setLocks(keys, !on, { quiet: true }) });
      }
    });
  }

  async function setAllLocks(on) {
    const ok = await write(async () => {
      await takeDash(await api("/locks/all", { method: "POST", body: { on } }));
    });
    if (!ok) return;
    haptic("ok");
    S.lockCache = null;
    await reloadTopPage();
    if (!S.lockCache) api("/locks").then((d) => { S.lockCache = d; }).catch(() => {});
  }

  function rightsData() {
    const page = currentPage();
    if (page && page.id === "rights" && page.data) return page.data;
    return S.rights && S.rights !== "error" ? S.rights : null;
  }

  async function setAllRights(open) {
    const data = rightsData();
    if (!data) return;
    const targets = data.rights.filter((r) => r.open !== open);
    let queued = false;
    for (const r of targets) {
      let d = null;
      const ok = await write(async () => {
        d = await api("/rights/" + encodeURIComponent(r.key) + "/toggle", { method: "POST" });
      });
      if (!ok) break;
      if (d && d.accepted) queued = true;
    }
    haptic("ok");
    if (queued) toast("ثبت شد؛ کمی بعد در تلگرام اعمال می شود.", "warn");
    else toast(open ? "گروه باز شد." : "گروه بسته شد.", "ok");
    await loadRights();
    const page = currentPage();
    if (page && page.id === "rights") await loadPage(page);
    else render();
  }

  async function powerFeature(fid) {
    const f = feature(fid);
    if (!f || !f.enabled) return;
    return guarded("power:" + fid, async () => {
      const wasOn = sectionEnabled(f.enabled);
      const ok = await write(async () => {
        const d = wasOn
          ? await api(f.off, { method: "POST" })
          : await api("/settings/apply", { method: "POST", body: { action: f.onAction() } });
        await takeDash(d);
      });
      redraw();
      if (!ok) return;
      haptic("sel");
      toast(f.title + (wasOn ? " خاموش شد" : " روشن شد"), "ok", { label: "برگرداندن", run: () => powerFeature(fid) });
    });
  }

  async function removeEntry(kind, key) {
    const page = currentPage();
    const entry = page && page.data && page.data.entries ? page.data.entries.find((e) => e.key === key) : null;
    const ok = await write(() => api("/lists/" + kind + "/" + encodeURIComponent(key), { method: "DELETE" }));
    S.pending = null;
    if (!ok) {
      render();
      return;
    }
    haptic();
    if (kind === "imgf") {
      toast("موضوع حذف شد.", "ok");
      return reloadTopPage();
    }
    if (page && page.data) {
      page.data.entries = page.data.entries.filter((e) => e.key !== key);
      if (S.lists) S.lists[kind] = page.data;
    }
    render();
    if (kind === "filter" && entry) {
      const word = decode(entry.name);
      toast("«" + word + "» حذف شد", "ok", { label: "برگرداندن", run: () => addFilterWord(word) });
    } else {
      toast("حذف شد.", "ok");
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
    if (d && d.accepted && d.error) toast(d.error, "warn");
    else toast(d && d.added === false ? "«" + word + "» از قبل در لیست بود." : "«" + word + "» اضافه شد.", "ok");
    await reloadTopPage();
    return true;
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
    if (hit("[data-retry-chat]") && S.chat != null) return loadChat(S.chat);

    if (S.palette) {
      if (t.id === "palette") return closePalette();
      if ((el = hit("[data-pal-go]")) && !hit("[data-apply], [data-lock], [data-power]")) return paletteGo(Number(el.dataset.palGo));
    }

    if ((el = hit("[data-apply]"))) {
      if (S.busy.has("set:" + el.dataset.apply)) return;
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

    if ((el = hit("[data-tab]"))) return goTab(el.dataset.tab);
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
      S.sheet = null;
      if (id !== S.chat) loadChat(id);
      else closeSheet();
      return;
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
      optimistic(el);
      return sheetWrite("/tempmedia/" + encodeURIComponent(el.dataset.tmedKind) + "/toggle");
    }
    if ((el = hit("[data-cause]"))) {
      optimistic(el);
      return sheetWrite("/strict-causes/" + encodeURIComponent(el.dataset.cause) + "/toggle");
    }
    if ((el = hit("[data-cap]"))) {
      optimistic(el);
      return sheetWrite("/limits/" + encodeURIComponent(el.dataset.cap) + "/toggle");
    }
    if ((el = hit("[data-slow]"))) {
      const seconds = Number(el.dataset.slow);
      optimistic(el);
      const ok = await sheetWrite("/slow", { body: { seconds } }, (d) => { S.slow = d; });
      if (ok) toast(seconds ? "اسلوموشن: " + slowLabel(seconds) : "اسلوموشن خاموش شد.", "ok");
      return;
    }
    if (hit("[data-report-test]")) {
      el = hit("[data-report-test]");
      el.disabled = true;
      const ok = await write(() => api("/report/test", { method: "POST" }));
      el.disabled = false;
      if (ok) {
        haptic("ok");
        toast("گزارش آزمایشی در گروه فرستاده شد.", "ok");
      }
      return;
    }
    if (hit("[data-allow-bot]")) {
      const input = document.getElementById("allow-bot");
      const username = input.value.trim();
      if (!username) return input.focus();
      const page = currentPage();
      let d = null;
      const ok = await write(async () => {
        d = await api("/allowed-bots", { method: "POST", body: { username } });
        if (page && page.data && d && d.bots) page.data.allowedBots = d;
      });
      if (ok) {
        if (!unsure(d)) {
          haptic("ok");
          toast(username + " مجاز شد.", "ok");
        }
        render();
      }
      return;
    }
    if ((el = hit("[data-disallow-bot]"))) {
      const page = currentPage();
      const ok = await write(async () => {
        const d = await api("/allowed-bots/" + encodeURIComponent(el.dataset.disallowBot), { method: "DELETE" });
        if (page && page.data && d && d.bots) page.data.allowedBots = d;
      });
      if (ok) {
        haptic();
        toast("از ربات های مجاز حذف شد.", "ok");
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
      if (el.dataset.locksAll === "1") return setAllLocks(true);
      S.confirm = confirmFor("locks-off");
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
      if (el.dataset.rightsAll === "1") return setAllRights(true);
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
      S.confirm = confirmAdmin(el.dataset.removeAdmin, el.dataset.name);
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
      if (!c) return;
      S.confirm = null;
      render();
      let d = null;
      const ok = await write(async () => {
        d = await c.run();
      });
      if (ok) {
        if (unsure(d)) {
          render();
          return;
        }
        if (!c.quiet) {
          haptic("ok");
          toast("انجام شد.", "ok");
        }
        render();
      }
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
    if (S.pending && !hit("[data-remove]")) {
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
      const page = currentPage();
      if (!page) return;
      page.draft = page.saved;
      render();
      return;
    }
    if (hit("[data-doc-save]")) {
      const page = currentPage();
      if (!page || !DOCS[page.id]) return;
      const text = page.draft || "";
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
        setClosingConfirmation(false);
        toast("ذخیره شد.", "ok");
        reloadTopPage();
      }
      return;
    }
    if ((el = hit("[data-filter-toggle]"))) {
      optimistic(el);
      const ok = await write(() => api("/filters/" + encodeURIComponent(el.dataset.filterToggle) + "/toggle", { method: "POST" }));
      if (ok) haptic("sel");
      reloadTopPage();
      return;
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
        toast((reply && reply.notice) || "موضوع اضافه شد؛ برای حذف کردن، کلیدش را روشن کنید.", "ok");
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
      el = hit("[data-answer-add]");
      el.disabled = true;
      let d = null;
      const ok = await write(async () => {
        d = await api("/answers", { method: "POST", body: { trigger: trig.value, text: body.value } });
      });
      el.disabled = false;
      if (ok) {
        haptic("ok");
        toast(d && d.accepted && d.error ? d.error : "پاسخ «" + decode((d && d.trigger) || trig.value.trim()) + "» ذخیره شد.", d && d.accepted ? "warn" : "ok");
        reloadTopPage();
      }
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
      const page = currentPage();
      const ok = await write(async () => {
        const d = await api("/voice/words", { method: "POST", body: { word } });
        if (page && d && d.words) page.data = d;
      });
      if (ok) {
        haptic("ok");
        render();
        const again = document.getElementById("voice-word");
        if (again) again.focus();
      }
      return;
    }
    if ((el = hit("[data-voice-remove]"))) {
      const page = currentPage();
      const ok = await write(async () => {
        const d = await api("/voice/words/" + encodeURIComponent(el.dataset.voiceRemove), { method: "DELETE" });
        if (page && d && d.words) page.data = d;
      });
      if (ok) {
        haptic();
        render();
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
      if (!page) return;
      const status = el.dataset.caseFilter;
      const user = page.data && page.data.user;
      await write(async () => {
        const data = await api("/cases?status=" + status + (user ? "&user_id=" + encodeURIComponent(user) : ""));
        data.status = status;
        data.user = user || "";
        page.data = data;
      });
      render();
      return;
    }
    if ((el = hit("[data-case-more]"))) {
      const page = currentPage();
      if (!page || !page.data) return;
      const status = page.data.status || "open";
      const user = page.data.user ? "&user_id=" + encodeURIComponent(page.data.user) : "";
      await write(async () => {
        const data = await api("/cases?status=" + status + "&before_id=" + el.dataset.caseMore + user);
        page.data.cases = page.data.cases.concat(data.cases || []);
        page.data.has_more = data.has_more;
      });
      render();
      return;
    }
    if ((el = hit("[data-case-action]"))) {
      const page = currentPage();
      if (!page || !page.data || !page.data.detail) return;
      const id = page.data.detail.id;
      const note = ((document.getElementById("case-note") || {}).value || "").trim();
      const action = el.dataset.caseAction;
      const path = action === "reverse" ? "/cases/" + id + "/reverse" : "/cases/" + id + "/resolve";
      const body = action === "reverse" ? (note ? { note } : {}) : (note ? { action, note } : { action });
      const ok = await write(async () => {
        const d = await api(path, { method: "POST", body });
        page.data.detail = await api("/cases/" + id);
        toast((d && d.message) || "پرونده به روز شد.", "ok");
      });
      if (ok) {
        haptic("ok");
        render();
        api("/cases?status=open&limit=5").then((d) => { S.openCases = d; }).catch(() => {});
      }
      return;
    }
    if (hit("[data-case-note]")) {
      const page = currentPage();
      if (!page || !page.data || !page.data.detail) return;
      const input = document.getElementById("case-note");
      const note = input.value.trim();
      if (!note) return input.focus();
      const id = page.data.detail.id;
      const ok = await write(async () => {
        await api("/cases/" + id + "/notes", { method: "POST", body: { note } });
        page.data.detail = await api("/cases/" + id);
      });
      if (ok) {
        toast("یادداشت ثبت شد.", "ok");
        render();
      }
      return;
    }
    if (hit("[data-joingate-save]")) {
      const channel = document.getElementById("join-gate-input").value.trim();
      if (!channel) return document.getElementById("join-gate-input").focus();
      let d = null;
      const ok = await write(async () => {
        d = await api("/join-gate", { method: "POST", body: { channel } });
      });
      if (ok) {
        if (!unsure(d)) {
          haptic("ok");
          toast("ذخیره شد.", "ok");
        }
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
        reloadTopPage();
      }
      return;
    }
  }

  function quick(what) {
    if (what !== "close") return;
    const closed = groupClosed();
    if (closed == null) return pushPage("rights");
    if (closed) return setAllRights(true);
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
    if ((e.key === "Enter" || e.key === " ") && t.matches && t.matches(".row.tap[data-open]")) {
      e.preventDefault();
      openSheet(t.dataset.open);
      return;
    }
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
      const page = currentPage();
      const user = t.value.trim();
      if (!page || (user && !/^-?\d+$/.test(user))) {
        if (user) toast("شناسه باید عدد باشد.", "warn");
        return;
      }
      const status = page.data.status || "open";
      api("/cases?status=" + status + (user ? "&user_id=" + encodeURIComponent(user) : "")).then((data) => {
        data.status = status;
        data.user = user;
        page.data = data;
        render();
      }).catch(report);
    }
  }

  function onGlobalKey(e) {
    const typing = e.target && (e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA");
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
    const num = e.target.closest && e.target.closest("[data-number-apply]");
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
      applySetting(num.dataset.numberApply + ":" + n);
      return;
    }
    const clock = e.target.closest && e.target.closest("[data-clock-apply]");
    if (!clock) return;
    const minutes = clockToMinutes(clock.value);
    if (minutes === null) return;
    if (String(minutes) === String(clock.dataset.clockValue)) return;
    applySetting(clock.dataset.clockApply + ":" + minutes);
  }

  function onInput(e) {
    const t = e.target;
    if (t.id === "palette-input") {
      S.palette.q = t.value;
      S.palette.idx = 0;
      drawPalette();
    } else if (t.id === "feature-search") {
      S.featureQuery = t.value;
      render();
    } else if (t.id === "lock-search") {
      S.lockQuery = t.value;
      render();
    } else if (t.id === "list-search") {
      S.listQuery = t.value;
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
    if (sheetEl && S.sheet && sheetEl.scrollTop <= 0 && !e.target.closest("input, textarea")) {
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
    if (document.visibilityState === "visible" && S.dash && Date.now() - S.loadedAt > 60000) refreshAll(true);
  });
  function onMedia(query, run) {
    if (query.addEventListener) query.addEventListener("change", run);
    else if (query.addListener) query.addListener(run);
  }
  onMedia(window.matchMedia("(prefers-color-scheme: light)"), applyTheme);
  onMedia(WIDE, () => S.dash && render());

  start();
})();
