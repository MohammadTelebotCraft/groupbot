use grammers_client::message::Button;

use super::style::{Colour, choice, data as coloured, toggle};
use super::{
    Ctx, betrayal, biolink, captcha, flood, join, limits, notice, purge, raid, strict, tempmedia, warns,
};

pub struct Pick {
    pub id: &'static str,
    pub value: &'static str,
    pub label: &'static str,

    pub danger: bool,
}

pub enum Kind {
    Flag,

    Number {
        range: (u32, u32),
        presets: &'static [u32],
        per_row: usize,
        show: fn(u32) -> String,
        read: fn(&Ctx, i64) -> u32,
    },

    Pick {
        options: &'static [Pick],

        default: &'static str,
    },
}

pub struct Setting {
    pub id: &'static str,
    pub key: &'static str,

    pub label: &'static str,

    pub section: &'static str,
    pub kind: Kind,
}

fn plain(value: u32) -> String {
    value.to_string()
}

fn duration(value: u32) -> String {
    strict::time_label(value)
}

fn every(value: u32) -> String {
    join::seconds_label(value, "هر بار")
}

fn lifetime(value: u32) -> String {
    join::seconds_label(value, "بدون حذف")
}

fn never(value: u32) -> String {
    match value {
        0 => "هرگز".to_owned(),
        seconds => seconds.to_string(),
    }
}

fn off(value: u32) -> String {
    match value {
        0 => "خاموش".to_owned(),
        count => count.to_string(),
    }
}

fn all(value: u32) -> String {
    match value {
        0 => "همه".to_owned(),
        count => count.to_string(),
    }
}

fn clock(value: u32) -> String {
    super::extras::clock(value)
}

const CLOCK: (u32, u32) = (0, 1439);

const NIGHT_DEFAULT: (u32, u32) = (23 * 60, 7 * 60);

fn night_window(ctx: &Ctx, chat: i64) -> (u32, u32) {
    super::extras::night(ctx, chat).unwrap_or(NIGHT_DEFAULT)
}

fn night_from(ctx: &Ctx, chat: i64) -> u32 {
    night_window(ctx, chat).0
}

fn night_to(ctx: &Ctx, chat: i64) -> u32 {
    night_window(ctx, chat).1
}

fn auto_at(ctx: &Ctx, chat: i64) -> u32 {
    purge::auto_at(ctx, chat).unwrap_or(purge::AUTO_DEFAULT_AT)
}

fn report_at(ctx: &Ctx, chat: i64) -> u32 {
    super::stats::report_at(ctx, chat).unwrap_or(super::stats::REPORT_DEFAULT)
}

fn captcha_choices(ctx: &Ctx, chat: i64) -> u32 {
    captcha::choices(ctx, chat) as u32
}

fn required_adds(ctx: &Ctx, chat: i64) -> u32 {
    join::required_adds(ctx, chat).min(u64::from(u32::MAX)) as u32
}

const MUTE_OR_BAN: &[Pick] = &[
    Pick { id: "fl_mute", value: "mute", label: "سکوت", danger: false },
    Pick { id: "fl_ban", value: "ban", label: "بن", danger: true },
];

pub const SETTINGS: &[Setting] = &[

    Setting { id: "fl_on", key: flood::MODE, label: "ضد رگبار", section: "fl", kind: Kind::Flag },
    Setting { id: "bt_on", key: betrayal::MODE, label: "ضد خیانت ادمین", section: "bt", kind: Kind::Flag },
    Setting { id: "cp_on", key: captcha::MODE, label: "احراز هویت", section: "cp", kind: Kind::Flag },
    Setting { id: "nt_on", key: notice::MODE, label: "اعلان حذف", section: "nt", kind: Kind::Flag },
    Setting { id: "rd_on", key: raid::MODE, label: "ضد هجوم", section: "rd", kind: Kind::Flag },
    Setting { id: "strict", key: strict::MODE, label: "حالت سختگیرانه", section: "s", kind: Kind::Flag },
    Setting { id: "rk_on", key: super::stats::RANKS, label: "مقام خودکار", section: "adv", kind: Kind::Flag },
    Setting { id: "tmed_on", key: tempmedia::MODE, label: "رسانه موقت", section: "tmed", kind: Kind::Flag },
    Setting { id: "lim_on", key: limits::MODE, label: "محدودیت مدیران", section: "lim", kind: Kind::Flag },

    Setting {
        id: "fl_lim", key: flood::LIMIT, label: "پیام", section: "fl",
        kind: Kind::Number {
            range: flood::LIMIT_RANGE, presets: flood::LIMIT_PRESETS,
            per_row: 5, show: plain, read: flood::limit,
        },
    },
    Setting {
        id: "fl_win", key: flood::WINDOW, label: "ثانیه", section: "fl",
        kind: Kind::Number {
            range: flood::WINDOW_RANGE, presets: flood::WINDOW_PRESETS,
            per_row: 5, show: plain, read: flood::window,
        },
    },
    Setting {
        id: "bt_lim", key: betrayal::LIMIT, label: "حذف", section: "bt",
        kind: Kind::Number {
            range: betrayal::LIMIT_RANGE, presets: betrayal::LIMIT_PRESETS,
            per_row: 5, show: plain, read: betrayal::limit,
        },
    },
    Setting {
        id: "bt_win", key: betrayal::WINDOW, label: "دقیقه", section: "bt",
        kind: Kind::Number {
            range: betrayal::WINDOW_RANGE, presets: betrayal::WINDOW_PRESETS,
            per_row: 5, show: plain, read: betrayal::window,
        },
    },
    Setting {
        id: "wn_lim", key: warns::LIMIT, label: "تعداد اخطار", section: "wn",
        kind: Kind::Number {
            range: warns::LIMIT_RANGE, presets: warns::LIMIT_PRESETS,
            per_row: 5, show: plain, read: warns::limit,
        },
    },
    Setting {
        id: "s_lim", key: strict::LIMIT, label: "تعداد تخلف", section: "s",
        kind: Kind::Number {
            range: strict::LIMIT_RANGE, presets: strict::LIMIT_PRESETS,
            per_row: 5, show: plain, read: strict::limit,
        },
    },
    Setting {
        id: "s_time", key: strict::TIME, label: "مدت محدودیت", section: "s",
        kind: Kind::Number {
            range: strict::TIME_RANGE, presets: strict::TIME_PRESETS,
            per_row: 2, show: duration, read: strict::minutes,
        },
    },
    Setting {
        id: "rd_lim", key: raid::LIMIT, label: "عضو تازه", section: "rd",
        kind: Kind::Number {
            range: raid::LIMIT_RANGE, presets: raid::LIMIT_PRESETS,
            per_row: 5, show: plain, read: raid::limit,
        },
    },
    Setting {
        id: "rd_win", key: raid::WINDOW, label: "ثانیه", section: "rd",
        kind: Kind::Number {
            range: raid::WINDOW_RANGE, presets: raid::WINDOW_PRESETS,
            per_row: 5, show: plain, read: raid::window,
        },
    },
    Setting {
        id: "rd_time", key: raid::TIME, label: "مدت سکوت", section: "rd",
        kind: Kind::Number {
            range: raid::TIME_RANGE, presets: raid::TIME_PRESETS,
            per_row: 2, show: duration, read: raid::minutes,
        },
    },
    Setting {
        id: "cp_t", key: captcha::TIMEOUT, label: "مهلت (ثانیه)", section: "cp",
        kind: Kind::Number {
            range: captcha::TIMEOUT_RANGE, presets: captcha::TIMEOUT_PRESETS,
            per_row: 4, show: plain, read: captcha::timeout,
        },
    },
    Setting {
        id: "cp_n", key: captcha::CHOICES, label: "تعداد گزینه ها", section: "cp",
        kind: Kind::Number {
            range: captcha::CHOICES_RANGE, presets: captcha::CHOICES_PRESETS,
            per_row: 5, show: plain, read: captcha_choices,
        },
    },
    Setting {
        id: "nt_t", key: notice::TTL, label: "پاک شدن خودکار (ثانیه)", section: "nt",
        kind: Kind::Number {
            range: notice::TTL_RANGE, presets: notice::TTL_PRESETS,
            per_row: 5, show: never, read: notice::ttl,
        },
    },
    Setting {
        id: "gpe", key: join::PROMPT_EVERY, label: "فاصله بین اعلان ها", section: "gp",
        kind: Kind::Number {
            range: join::EVERY_RANGE, presets: join::EVERY_PRESETS,
            per_row: 3, show: every, read: join::prompt_every,
        },
    },
    Setting {
        id: "gpt", key: join::PROMPT_TTL, label: "حذف خودکار اعلان", section: "gp",
        kind: Kind::Number {
            range: join::TTL_RANGE, presets: join::TTL_PRESETS,
            per_row: 3, show: lifetime, read: join::prompt_ttl,
        },
    },
    Setting {
        id: "ad", key: join::ADD_REQUIRED, label: "تعداد اد اجباری", section: "ad",
        kind: Kind::Number {
            range: join::ADD_RANGE, presets: join::ADD_PRESETS,
            per_row: 3, show: off, read: required_adds,
        },
    },
    Setting {
        id: "tmed_min", key: tempmedia::MINUTES, label: "زمان حذف رسانه", section: "tmed",
        kind: Kind::Number {
            range: tempmedia::MINUTES_RANGE, presets: tempmedia::MINUTES_PRESETS,
            per_row: 3, show: duration, read: tempmedia::minutes,
        },
    },
    Setting {
        id: "apc", key: purge::AUTO_COUNT, label: "چند پیام هر بار", section: "ap",
        kind: Kind::Number {
            range: purge::AUTO_COUNT_RANGE, presets: purge::AUTO_COUNT_PRESETS,
            per_row: 3, show: all, read: purge::auto_count,
        },
    },

    Setting {
        id: "apt", key: purge::AUTO_AT, label: "ساعت پاکسازی", section: "ap",
        kind: Kind::Number {
            range: CLOCK, presets: purge::AUTO_AT_PRESETS,
            per_row: 3, show: clock, read: auto_at,
        },
    },
    Setting {
        id: "dr", key: super::stats::REPORT_AT, label: "ساعت گزارش، مثل 21:30", section: "dr",
        kind: Kind::Number {
            range: CLOCK, presets: super::stats::REPORT_PRESETS,
            per_row: 3, show: clock, read: report_at,
        },
    },
    Setting {
        id: "ngf", key: super::extras::NIGHT, label: "ساعت شروع، مثل 23:37", section: "ng",
        kind: Kind::Number { range: CLOCK, presets: &[], per_row: 3, show: clock, read: night_from },
    },
    Setting {
        id: "ngt", key: super::extras::NIGHT, label: "ساعت پایان، مثل 7:05", section: "ng",
        kind: Kind::Number { range: CLOCK, presets: &[], per_row: 3, show: clock, read: night_to },
    },

    Setting {
        id: "fl_act", key: flood::ACTION, label: "با متخلف", section: "fl",
        kind: Kind::Pick { options: MUTE_OR_BAN, default: "mute" },
    },
    Setting {
        id: "bl_on", key: biolink::LOCK, label: "قفل لینک در بایو", section: "bl",
        kind: Kind::Flag,
    },
    Setting {
        id: "bl_act", key: biolink::ACTION, label: "با متخلف", section: "bl",
        kind: Kind::Pick {
            options: &[
                Pick { id: "bl_del", value: "del", label: "فقط حذف", danger: false },
                Pick { id: "bl_mute", value: "mute", label: "سکوت", danger: false },
                Pick { id: "bl_kick", value: "kick", label: "اخراج", danger: true },
                Pick { id: "bl_ban", value: "ban", label: "بن", danger: true },
            ],
            default: "del",
        },
    },
    Setting {
        id: "bt_act", key: betrayal::ACTION, label: "با ادمین", section: "bt",
        kind: Kind::Pick {
            options: &[
                Pick { id: "bt_demote", value: "demote", label: "فقط عزل", danger: false },
                Pick { id: "bt_ban", value: "ban", label: "عزل و بن", danger: true },
            ],
            default: "demote",
        },
    },
    Setting {
        id: "cp_act", key: captcha::ACTION, label: "پس از مهلت", section: "cp",
        kind: Kind::Pick {
            options: &[
                Pick { id: "cp_mute", value: "mute", label: "سکوت", danger: false },
                Pick { id: "cp_kick", value: "kick", label: "اخراج", danger: true },
            ],
            default: "kick",
        },
    },
    Setting {
        id: "wn_act", key: warns::ACTION, label: "با سقف اخطار", section: "wn",
        kind: Kind::Pick {
            options: &[
                Pick { id: "wn_mute", value: "mute", label: "سکوت", danger: false },
                Pick { id: "wn_ban", value: "ban", label: "اخراج", danger: true },
            ],
            default: "ban",
        },
    },
    Setting {
        id: "s_act", key: strict::ACTION, label: "با متخلف", section: "s",
        kind: Kind::Pick {
            options: &[
                Pick { id: "strict_mute", value: "mute", label: "سکوت", danger: false },
                Pick { id: "strict_ban", value: "ban", label: "بن", danger: true },
            ],
            default: "mute",
        },
    },
    Setting {
        id: "tmed_who", key: tempmedia::AUDIENCE, label: "شامل چه کسانی", section: "tmed",
        kind: Kind::Pick {
            options: &[
                Pick { id: "tmed_plain", value: "plain", label: "بدون مقام", danger: false },
                Pick { id: "tmed_all", value: "all", label: "همه", danger: false },
            ],
            default: "plain",
        },
    },
    Setting {
        id: "an_act", key: super::answers::AUDIENCE, label: "مخاطب پاسخ ها", section: "an",
        kind: Kind::Pick {
            options: &[
                Pick { id: "an_all", value: "all", label: "همه", danger: false },
                Pick { id: "an_admins", value: "admins", label: "ادمین ها", danger: false },
                Pick { id: "an_vips", value: "vips", label: "ویژه ها", danger: false },
            ],
            default: "all",
        },
    },
];

pub fn find(id: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|setting| setting.id == id)
}

pub fn number(id: &str) -> Option<(&'static Setting, (u32, u32))> {
    let setting = find(id)?;
    match setting.kind {
        Kind::Number { range, .. } => Some((setting, range)),
        _ => None,
    }
}

pub async fn store(ctx: &Ctx, chat: i64, setting: &Setting, value: u32) {
    match setting.id {
        "ad" => join::set_required_adds(ctx, chat, u64::from(value)).await,

        "apt" => purge::set_auto_at(ctx, chat, Some(value)).await,
        "dr" => super::stats::set_report_at(ctx, chat, Some(value)).await,

        "ngf" => {
            let (_, to) = night_window(ctx, chat);
            super::extras::set_night(ctx, chat, Some((value, to))).await;
        }
        "ngt" => {
            let (from, _) = night_window(ctx, chat);
            super::extras::set_night(ctx, chat, Some((from, value))).await;
        }
        _ => {
            ctx.settings
                .set_value(chat, setting.key, &value.to_string())
                .await
        }
    }
}

pub fn shown(ctx: &Ctx, chat: i64, setting: &Setting) -> String {
    match &setting.kind {
        Kind::Number { show, read, .. } => show(read(ctx, chat)),
        _ => String::new(),
    }
}

pub async fn apply(ctx: &Ctx, chat: i64, action: &str) -> Option<&'static str> {
    if let Some((id, value)) = action.split_once(':')
        && let Some((setting, (min, max))) = number(id)
        && let Ok(value) = value.parse::<u32>()
    {
        store(ctx, chat, setting, value.clamp(min, max)).await;
        return Some(setting.section);
    }

    for setting in SETTINGS {
        match &setting.kind {
            Kind::Flag if setting.id == action => {
                let now_on = !ctx.settings.is_locked(chat, setting.key);
                ctx.settings.set(chat, setting.key, now_on).await;

                if super::locks::LOCKS.iter().any(|lock| lock.key == setting.key) {
                    super::strict::sync_pick(ctx, chat, setting.key, now_on).await;
                    super::bots::on_lock_set(ctx, chat, setting.key, now_on).await;
                }
                return Some(setting.section);
            }
            Kind::Pick { options, .. } => {
                if let Some(pick) = options.iter().find(|pick| pick.id == action) {
                    ctx.settings.set_value(chat, setting.key, pick.value).await;
                    return Some(setting.section);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn chosen(ctx: &Ctx, chat: i64, setting: &Setting) -> &'static str {
    let Kind::Pick { options, default } = &setting.kind else {
        return "";
    };
    let stored = ctx.settings.value(chat, setting.key);
    options
        .iter()
        .find(|pick| Some(pick.value) == stored.as_deref())
        .map_or(*default, |pick| pick.value)
}

pub fn rows(
    ctx: &Ctx,
    chat: i64,
    setting: &Setting,
    payload: &dyn Fn(&str) -> Vec<u8>,
) -> Vec<Vec<Button>> {
    match &setting.kind {
        Kind::Flag => {
            let on = ctx.settings.is_locked(chat, setting.key);
            vec![vec![toggle(
                format!("{}  {}", if on { "✓" } else { "✗" }, setting.label),
                payload(setting.id),
                on,
            )]]
        }

        Kind::Number { presets, per_row, show, read, .. } => {
            let current = read(ctx, chat);
            let mut rows = vec![vec![Button::data(setting.label, payload(setting.section))]];
            rows.extend(presets.chunks(*per_row).map(|block| {
                block
                    .iter()
                    .map(|&value| {
                        choice(
                            show(value),
                            payload(&format!("{}:{value}", setting.id)),
                            value == current,
                        )
                    })
                    .collect()
            }));

            rows.push(vec![Button::data(
                format!("✎  عدد دلخواه · {}", show(current)),
                payload(&format!("in:{}", setting.id)),
            )]);
            rows
        }

        Kind::Pick { options, .. } => {
            let current = chosen(ctx, chat, setting);
            vec![
                options
                    .iter()
                    .map(|pick| {
                        let target = payload(pick.id);
                        match (pick.danger, pick.value == current) {
                            (true, _) => coloured(pick.label, target, Colour::Danger),
                            (false, chosen) => choice(pick.label, target, chosen),
                        }
                    })
                    .collect(),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_claimed_once() {
        let mut actions: Vec<&str> = Vec::new();
        for setting in SETTINGS {
            actions.push(setting.id);
            if let Kind::Pick { options, .. } = &setting.kind {
                actions.extend(options.iter().map(|pick| pick.id));
            }
        }
        let count = actions.len();
        actions.sort_unstable();
        actions.dedup();
        assert_eq!(actions.len(), count, "two settings answer to the same action");
    }

    #[test]
    fn every_preset_sits_inside_its_range() {
        for setting in SETTINGS {
            let Kind::Number { range, presets, .. } = &setting.kind else {
                continue;
            };
            for &value in *presets {
                assert!(
                    value >= range.0 && value <= range.1,
                    "preset {value} for {} is outside {:?}",
                    setting.id,
                    range
                );
            }
        }
    }

    #[test]
    fn every_pick_defaults_to_one_of_its_options() {
        for setting in SETTINGS {
            let Kind::Pick { options, default } = &setting.kind else {
                continue;
            };
            assert!(
                options.iter().any(|pick| pick.value == *default),
                "{} defaults to {default}, which is not one of its options",
                setting.id
            );
        }
    }

    #[test]
    fn no_id_carries_a_colon() {
        for setting in SETTINGS {
            assert!(!setting.id.contains(':'), "{} has a colon in its id", setting.id);
            if let Kind::Pick { options, .. } = &setting.kind {
                for pick in *options {
                    assert!(!pick.id.contains(':'), "{} has a colon in its id", pick.id);
                }
            }
        }
    }

    #[test]
    fn payloads_fit_telegram() {
        for setting in SETTINGS {
            let longest = format!("p:{}:{}:in:{}", i64::MAX, i64::MIN, setting.id);
            assert!(longest.len() <= 64, "payload too long for {}", setting.id);
        }
    }
}
