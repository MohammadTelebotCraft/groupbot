use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use serde_json::{Value, json};

use crate::handlers::{self, Ctx, extras, locks, purge, setting, stats};

use super::auth::AdminGate;

const SECTIONS: &[(&str, &str)] = &[
    ("fl", "ضد رگبار"),
    ("bt", "ضد خیانت ادمین"),
    ("cp", "احراز هویت"),
    ("nt", "اعلان حذف"),
    ("rd", "ضد هجوم"),
    ("s", "حالت سختگیرانه"),
    ("sec", "امنیت و ورود"),
    ("tmed", "رسانه موقت"),
    ("lim", "محدودیت مدیران"),
    ("cq", "نگهبان هوشمند"),
    ("wn", "اخطار"),
    ("gp", "یادآوری اد اجباری"),
    ("ad", "اد اجباری"),
    ("ap", "پاکسازی خودکار"),
    ("dr", "گزارش روزانه"),
    ("ng", "قفل شب"),
    ("bl", "لینک در بایو"),
    ("an", "پاسخ خودکار"),
    ("adv", "تنظیمات پیشرفته"),
    ("wc", "خوشامد"),
];

fn section_label(id: &str) -> String {
    SECTIONS
        .iter()
        .find(|(section, _)| *section == id)
        .map_or_else(|| id.to_owned(), |(_, label)| (*label).to_owned())
}

fn setting_json(ctx: &Ctx, chat: i64, item: &setting::Setting) -> Value {
    match &item.kind {
        setting::Kind::Flag => json!({
            "id": item.id,
            "kind": "flag",
            "label": item.label,
            "on": ctx.settings.is_locked(chat, item.key),
        }),
        setting::Kind::Number {
            range,
            presets,
            show,
            read,
            ..
        } => {
            let (range, presets, show, read) = (*range, *presets, *show, *read);
            let current = read(ctx, chat);
            json!({
                "id": item.id,
                "kind": "number",
                "label": item.label,
                "value": current,
                "shown": show(current),
                "range": [range.0, range.1],

                "clock": range == setting::CLOCK,
                "presets": presets
                    .iter()
                    .map(|&value| json!({ "value": value, "shown": show(value) }))
                    .collect::<Vec<_>>(),
            })
        }
        setting::Kind::Pick { options, .. } => {
            let chosen = setting::chosen(ctx, chat, item);
            json!({
                "id": item.id,
                "kind": "pick",
                "label": item.label,
                "chosen": chosen,
                "options": options
                    .iter()
                    .map(|pick| json!({
                        "id": pick.id,
                        "value": pick.value,
                        "label": pick.label,
                        "danger": pick.danger,
                    }))
                    .collect::<Vec<_>>(),
            })
        }
    }
}

pub async fn dashboard(State(ctx): State<Arc<Ctx>>, gate: AdminGate) -> impl IntoResponse {
    let chat = gate.chat;

    let title = handlers::esc(
        &ctx.settings
            .value(chat, handlers::TITLE)
            .unwrap_or_else(|| chat.to_string()),
    );

    let mut order: Vec<&'static str> = Vec::new();
    for item in setting::SETTINGS {
        if !order.contains(&item.section) {
            order.push(item.section);
        }
    }
    let sections: Vec<Value> = order
        .iter()
        .map(|&section| {
            let items: Vec<Value> = setting::SETTINGS
                .iter()
                .filter(|item| item.section == section)
                .map(|item| setting_json(&ctx, chat, item))
                .collect();

            let enabled = match section {
                "ap" => Some(purge::auto_at(&ctx, chat).is_some()),
                "dr" => Some(stats::report_at(&ctx, chat).is_some()),
                "ng" => Some(extras::night(&ctx, chat).is_some()),
                _ => None,
            };
            json!({
                "id": section,
                "label": section_label(section),
                "settings": items,
                "enabled": enabled,
            })
        })
        .collect();

    let active = locks::plain()
        .filter(|lock| ctx.settings.is_locked(chat, lock.key))
        .count();
    let total = locks::plain().count();
    let ai_active = locks::LOCKS
        .iter()
        .filter(|lock| locks::is_ai(lock.key) && ctx.settings.is_locked(chat, lock.key))
        .count();

    Json(json!({
        "chat": { "id": chat, "title": title },
        "viewer": { "user_id": gate.user, "is_owner": gate.is_owner },
        "locks_summary": { "active": active, "total": total, "ai_active": ai_active },
        "sections": sections,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_has_a_persian_label() {
        for item in setting::SETTINGS {
            assert!(
                SECTIONS.iter().any(|(id, _)| *id == item.section),
                "section «{}» (from setting «{}») has no label, so the mini app would print \
                 its id",
                item.section,
                item.id
            );
        }
    }

    #[test]
    fn no_label_is_orphaned() {
        for (id, _) in SECTIONS {
            assert!(
                setting::SETTINGS.iter().any(|item| item.section == *id),
                "«{id}» is labelled but no setting lives in it"
            );
        }
    }
}
