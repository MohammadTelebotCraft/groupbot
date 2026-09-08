
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_MESSAGES: usize = 4096;
const PER_CHAT: usize = 16;
const TTL: Duration = Duration::from_secs(180);

struct Entry {
    chat: i64,
    id: i32,
    sender: Option<i64>,
    at: Instant,
    text: String,
}

#[derive(Default)]
pub struct History {
    entries: VecDeque<Entry>,
}

impl History {
    pub fn is_current(&self, chat: i64, id: i32, text: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.chat == chat && e.id == id && e.text == text && e.at.elapsed() < TTL)
    }

    pub fn observe(
        &mut self,
        chat: i64,
        id: i32,
        sender: Option<i64>,
        reply: Option<i32>,
        text: &str,
    ) -> Option<String> {
        self.observe_at(chat, id, sender, reply, text, Instant::now())
    }

    fn observe_at(
        &mut self,
        chat: i64,
        id: i32,
        sender: Option<i64>,
        reply: Option<i32>,
        text: &str,
        now: Instant,
    ) -> Option<String> {
        while self
            .entries
            .front()
            .is_some_and(|e| now.duration_since(e.at) >= TTL)
        {
            self.entries.pop_front();
        }
        self.entries.retain(|e| e.chat != chat || e.id != id);
        let parent = if let Some(reply) = reply {
            self.entries
                .iter()
                .rev()
                .find(|e| e.chat == chat && e.id == reply)
        } else {
            self.entries
                .iter()
                .filter(|e| e.chat == chat)
                .max_by_key(|e| e.id)
                .filter(|e| sender.is_some() && e.sender == sender && e.id < id)
        }
        .map(|e| e.text.clone());
        {
            if self.entries.iter().filter(|e| e.chat == chat).count() >= PER_CHAT
                && let Some(at) = self.entries.iter().position(|e| e.chat == chat)
            {
                self.entries.remove(at);
            }
            if self.entries.len() >= MAX_MESSAGES {
                self.entries.pop_front();
            }
            self.entries.push_back(Entry {
                chat,
                id,
                sender: if text.is_empty() || text.len() > 16_384 {
                    None
                } else {
                    sender
                },
                at: now,
                text: if text.len() <= 16_384 {
                    text.to_owned()
                } else {
                    String::new()
                },
            });
        }
        parent
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Followup {
    Sell,
    Buy,
    Amount,
    Contact,
    Exchange,
}

pub fn followup(clean: &str) -> Option<Followup> {
    let body = clean.trim_matches(|c: char| c.is_whitespace() || "!،,.".contains(c));
    match body {
        "آره میفروشم" | "بله میفروشم" | "میفروشم" | "yes selling" | "yes i sell it" => {
            Some(Followup::Sell)
        }
        "میخرم" | "آره میخرم" | "خریدارم" | "i'll buy it" | "i will buy it" => {
            Some(Followup::Buy)
        }
        "پیوی" | "بیا پیوی" | "پیوی بیا" | "دایرکت بده" | "dm me" => {
            Some(Followup::Contact)
        }
        "آره عوض میکنم" | "معاوضه میکنم" | "yes i'll trade" => {
            Some(Followup::Exchange)
        }
        _ => {
            let digits = super::super::digits(body);
            let words: Vec<_> = digits.split_whitespace().collect();
            let number = words.first().is_some_and(|word| {
                word.chars().any(|c| c.is_ascii_digit())
                    && word
                        .chars()
                        .all(|c| c.is_ascii_digit() || ".٫,".contains(c))
            });
            (number
                && (words.len() == 1
                    || (words.len() == 2 && ["تومن", "تومان", "دلار", "usdt"].contains(&words[1]))))
            .then_some(Followup::Amount)
        }
    }
}

pub fn object(text: &str) -> Option<&'static str> {
    [
        "اکانت",
        "گوشی",
        "آیتم",
        "اسکین",
        "ماشین",
        "تبلت",
        "لپتاپ",
        "سکه",
        "تتر",
        "کتاب",
        "account",
        "skin",
        "phone",
    ]
    .into_iter()
    .find(|word| super::standalone(text, word).is_some())
}

pub fn resolve(kind: Followup, current: &str, object: &str) -> String {
    match kind {
        Followup::Sell => format!("{object} رو میفروشم"),
        Followup::Buy => format!("{object} رو میخرم، خریدارم"),
        Followup::Amount => format!(
            "خریدار {object} هستم، پیشنهادم {}",
            super::super::digits(current)
        ),
        Followup::Contact => format!("برای خرید و فروش {object} پیام بده پیوی"),
        Followup::Exchange => format!("این {object} رو معاوضه میکنم"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_scoped_fresh_bounded_and_replaced_on_edit() {
        let mut history = History::default();
        let now = Instant::now();
        history.observe_at(1, 10, Some(2), None, "اکانت فروشی", now);
        assert!(
            history
                .observe_at(2, 11, Some(2), Some(10), "۵۰۰", now)
                .is_none()
        );
        assert!(
            history
                .observe_at(1, 11, Some(3), None, "۵۰۰", now)
                .is_none()
        );
        assert_eq!(
            history
                .observe_at(1, 12, Some(3), Some(10), "۵۰۰", now)
                .as_deref(),
            Some("اکانت فروشی")
        );
        history.observe_at(1, 10, Some(2), None, "سلام", now);
        assert_eq!(
            history
                .observe_at(1, 13, Some(3), Some(10), "۵۰۰", now)
                .as_deref(),
            Some("سلام")
        );
        assert!(
            history
                .observe_at(1, 14, Some(3), Some(10), "۵۰۰", now + TTL)
                .is_none()
        );
        for id in 20..100 {
            history.observe_at(1, id, Some(2), None, "hello", now + TTL);
        }
        assert_eq!(
            history.entries.iter().filter(|e| e.chat == 1).count(),
            PER_CHAT
        );
        for id in 100..5000 {
            history.observe_at(i64::from(id), id, Some(2), None, "hello", now + TTL);
        }
        assert_eq!(history.entries.len(), MAX_MESSAGES);
    }

    #[test]
    fn harmless_replies_never_inherit_transaction_intent() {
        for text in [
            "ممنون",
            "نه",
            "آره",
            "گزارش شد",
            "فروش ممنوعه",
            "۵۰۰ نفر",
            "thanks",
            "don't sell it",
            "قیمت دلار ۵۰۰",
            "آره میفروشم؟",
        ] {
            assert_eq!(followup(text), None, "{text}");
        }
        assert_eq!(followup("۵۰۰"), Some(Followup::Amount));
        assert_eq!(followup("آره میفروشم"), Some(Followup::Sell));
    }

    #[test]
    fn empty_messages_and_old_edits_do_not_bridge_unrelated_conversations() {
        let mut history = History::default();
        history.observe(1, 10, Some(1), None, "اکانت فروشی");
        history.observe(1, 11, Some(2), None, "سلام");
        history.observe(1, 10, Some(1), None, "اکانت فروشی ۵۰۰");
        assert!(history.observe(1, 12, Some(1), None, "۵۰۰").is_none());
        history.observe(1, 13, Some(2), None, "");
        assert!(history.observe(1, 14, Some(1), None, "۵۰۰").is_none());
        let obfuscated = format!("اکانت {} ۵۰۰", "ف".repeat(600));
        history.observe(1, 15, Some(1), None, &obfuscated);
        assert!(history.is_current(1, 15, &obfuscated));
        history.observe(1, 15, Some(1), None, "سلام");
        assert!(!history.is_current(1, 15, &obfuscated));
    }
}
