use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use grammers_client::media::Media;
use grammers_client::message::{Button, Message, ReplyMarkup};
use grammers_client::session::types::PeerId;
use grammers_client::update::CallbackQuery;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use super::locks::View;
use super::{Ctx, locks, notice, stats};

pub const MODE: &str = "voice_monitor";
pub const WORDS_PREFIX: &str = "voice_word:";
const DISABLED_DEFAULT_PREFIX: &str = "voice_disabled:";

pub const ADD_COMMANDS: &[&str] = &[
    "فیلتر ویس کلمه",
    "فیلتر ویس",
    "افزودن کلمه ویس",
    "افزودن فیلتر ویس",
];
pub const REMOVE_COMMANDS: &[&str] = &["حذف کلمه ویس", "حذف فیلتر ویس", "لغو فیلتر ویس"];
pub const LIST_COMMANDS: &[&str] = &["لیست کلمات ویس", "کلمات ویس"];
const MAX_WORDS: usize = 200;
const MAX_WORD_CHARS: usize = 64;

const MAX_DURATION_SECONDS: f64 = 6.0 * 60.0 * 60.0;
const MAX_FILE_BYTES: usize = 256 * 1024 * 1024;
const WINDOW_SECONDS: f64 = 20.0;
const FULL_SCAN_SECONDS: f64 = 90.0;
const MAX_WINDOWS: usize = 6;
const JOB_TIMEOUT: Duration = Duration::from_secs(240);
const MAX_TRANSCRIPT_TEXT_CHARS: usize = 4_096;
const CALLBACK_TEXT_CHARS: usize = 180;
const DEFAULT_WORKERS: usize = 4;
const MAX_WORKERS: usize = 256;
const DEFAULT_WHISPER_WORKERS: usize = 1;
const MAX_WHISPER_WORKERS: usize = 4;
const DEFAULT_QUEUE_CAPACITY: usize = 128;
const MAX_QUEUE_CAPACITY: usize = 2_048;

static NEXT_INPUT: AtomicU64 = AtomicU64::new(1);

const DEFAULT_BAD_WORDS: &[&str] = &[
    "مادرجنده",
    "جنده",
    "کونی",
    "کس ننت",
    "کیری",
    "حرومزاده",
    "خارکسه",
    "کیرم",
];

#[derive(Debug, Deserialize)]
pub(crate) struct RecognitionReport {
    pub(crate) transcripts: Vec<Transcript>,
    pub(crate) usable_windows: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Transcript {
    pub(crate) text: String,
    pub(crate) confidence: Option<f32>,
}

struct VoiceJob {
    input: PathBuf,
    duration: f64,
    result: oneshot::Sender<Result<RecognitionReport, String>>,
}

struct WorkerProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub struct VoicePool {
    sender: mpsc::Sender<VoiceJob>,
}

impl VoicePool {
    pub fn new() -> Self {
        let local_whisper = std::env::var("VOICE_BACKEND")
            .map(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "whisper" | "faster-whisper"
                )
            })
            .unwrap_or(false);
        let default_workers = if local_whisper {
            DEFAULT_WHISPER_WORKERS
        } else {
            DEFAULT_WORKERS
        };
        let max_workers = if local_whisper {
            MAX_WHISPER_WORKERS
        } else {
            MAX_WORKERS
        };
        let workers = std::env::var("VOICE_WORKERS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(default_workers)
            .clamp(1, max_workers);
        let queue = std::env::var("VOICE_QUEUE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_QUEUE_CAPACITY)
            .clamp(1, MAX_QUEUE_CAPACITY);
        let (sender, receiver) = mpsc::channel(queue);
        let receiver = Arc::new(tokio::sync::Mutex::new(receiver));
        for index in 0..workers {
            tokio::spawn(worker_loop(Arc::clone(&receiver), index));
        }
        Self { sender }
    }

    pub async fn recognize(
        &self,
        input: PathBuf,
        duration: f64,
    ) -> Result<RecognitionReport, String> {
        let (result, response) = oneshot::channel();
        self.sender
            .send(VoiceJob {
                input,
                duration,
                result,
            })
            .await
            .map_err(|_| "voice recognizer queue stopped".to_owned())?;
        response
            .await
            .map_err(|_| "voice recognizer worker stopped".to_owned())?
    }
}

async fn worker_loop(receiver: Arc<tokio::sync::Mutex<mpsc::Receiver<VoiceJob>>>, index: usize) {
    let mut process: Option<WorkerProcess> = None;
    while let Some(job) = receiver.lock().await.recv().await {
        if process.is_none() {
            process = match start_worker(index) {
                Ok(process) => Some(process),
                Err(error) => {
                    eprintln!("voice monitor: worker {index} could not start: {error}");
                    None
                }
            };
        }

        let result = match process.as_mut() {
            Some(process) => process.request(&job.input, job.duration).await,
            None => Err("voice recognizer process unavailable".to_owned()),
        };
        if result.is_err() {
            if let Some(process) = process.as_mut() {
                let _ = process.child.kill().await;
            }
            process = None;
        }
        let _ = job.result.send(result);
    }
}

fn start_worker(index: usize) -> Result<WorkerProcess, String> {
    let python = std::env::var("VOICE_PYTHON").unwrap_or_else(|_| {
        let bundled = Path::new(".venv-voice/bin/python");
        if bundled.exists() {
            bundled.to_string_lossy().into_owned()
        } else {
            "python3".to_owned()
        }
    });
    let script =
        std::env::var("VOICE_MONITOR_SCRIPT").unwrap_or_else(|_| "voice_monitor.py".to_owned());
    let mut child = Command::new(python)
        .arg(script)
        .arg("--worker")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("worker {index}: {error}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("worker {index}: stdin was not piped"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("worker {index}: stdout was not piped"))?;
    Ok(WorkerProcess {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

impl WorkerProcess {
    async fn request(&mut self, input: &Path, duration: f64) -> Result<RecognitionReport, String> {
        let request = serde_json::json!({
            "input": input,
            "duration": duration,
            "window_seconds": WINDOW_SECONDS,
            "full_scan_seconds": FULL_SCAN_SECONDS,
            "max_windows": MAX_WINDOWS,
        });
        let line = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        self.stdin
            .write_all(&line)
            .await
            .map_err(|error| error.to_string())?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|error| error.to_string())?;
        self.stdin
            .flush()
            .await
            .map_err(|error| error.to_string())?;

        let mut response = String::new();
        let bytes = self
            .stdout
            .read_line(&mut response)
            .await
            .map_err(|error| error.to_string())?;
        if bytes == 0 {
            return Err("voice recognizer process exited".to_owned());
        }
        let value: serde_json::Value =
            serde_json::from_str(&response).map_err(|error| error.to_string())?;
        if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
            return Err(error.to_owned());
        }
        serde_json::from_value(value).map_err(|error| error.to_string())
    }
}

pub fn words(ctx: &Ctx, chat: i64) -> Vec<String> {
    let disabled = ctx
        .settings
        .flags_with_prefix(chat, DISABLED_DEFAULT_PREFIX);
    let custom = ctx.settings.flags_with_prefix(chat, WORDS_PREFIX);
    active_words(&disabled, &custom)
}

pub fn disabled_default_words(ctx: &Ctx, chat: i64) -> Vec<String> {
    let disabled = ctx
        .settings
        .flags_with_prefix(chat, DISABLED_DEFAULT_PREFIX)
        .into_iter()
        .map(|word| normalize(&word))
        .collect::<HashSet<_>>();
    DEFAULT_BAD_WORDS
        .iter()
        .copied()
        .filter(|word| disabled.contains(&normalize(word)))
        .map(str::to_owned)
        .collect()
}

pub fn words_title(ctx: &Ctx, chat: i64) -> String {
    let active = words(ctx, chat);
    let defaults = active
        .iter()
        .filter(|word| is_default_word(word).is_some())
        .count();
    let custom = active.len().saturating_sub(defaults);
    let disabled = disabled_default_words(ctx, chat).len();
    format!(
        "<b>پنل مدیریت</b> › <b>کلمات فیلتر ویس</b>\n\n\
         {} کلمه فعال است ({} پیش فرض و {} سفارشی).\n\
         {} کلمه پیش فرض حذف شده است.\n\n\
         برای افزودن یا بازگردانی «فیلتر ویس کلمه» و برای حذف «حذف کلمه ویس» را همراه کلمه بفرستید.",
        active.len(),
        defaults,
        custom,
        disabled,
    )
}

pub fn words_markup(ctx: &Ctx, chat: i64, opener: i64) -> ReplyMarkup {
    let disabled = disabled_default_words(ctx, chat);
    let mut rows = disabled
        .iter()
        .take(20)
        .map(|word| {
            vec![Button::data(
                format!("↺  {word}"),
                format!("p:{opener}:{chat}:vwr:{}", super::lists::word_id(word)).into_bytes(),
            )]
        })
        .collect::<Vec<_>>();
    let remaining = 20usize.saturating_sub(rows.len());
    rows.extend(
        words(ctx, chat)
            .iter()
            .take(remaining)
            .map(|word| {
                vec![Button::data(
                    format!("✗  {word}"),
                    format!("p:{opener}:{chat}:vw:{}", super::lists::word_id(word)).into_bytes(),
                )]
            })
            .collect::<Vec<_>>(),
    );
    rows.push(super::panel::back_row(opener, chat, "sec", "vw"));
    super::premium::buttons(&rows)
}

pub async fn remove_word(ctx: &Ctx, chat: i64, id: &str) {
    if let Some(word) = words(ctx, chat)
        .into_iter()
        .find(|word| super::lists::word_id(word) == id)
    {
        if is_default_word(&word).is_some() {
            ctx.settings
                .set(chat, &format!("{WORDS_PREFIX}{word}"), false)
                .await;
            ctx.settings
                .set(chat, &format!("{DISABLED_DEFAULT_PREFIX}{word}"), true)
                .await;
        } else {
            ctx.settings
                .set(chat, &format!("{WORDS_PREFIX}{word}"), false)
                .await;
        }
    }
}

pub async fn restore_word(ctx: &Ctx, chat: i64, id: &str) {
    if let Some(word) = disabled_default_words(ctx, chat)
        .into_iter()
        .find(|word| super::lists::word_id(word) == id)
    {
        ctx.settings
            .set(chat, &format!("{DISABLED_DEFAULT_PREFIX}{word}"), false)
            .await;
    }
}

pub async fn restore_all_defaults(ctx: &Ctx, chat: i64) -> usize {
    let disabled = disabled_default_words(ctx, chat);
    for word in &disabled {
        ctx.settings
            .set(chat, &format!("{DISABLED_DEFAULT_PREFIX}{word}"), false)
            .await;
    }
    disabled.len()
}

pub enum AddWordError {
    Empty,
    TooLong,
    Full,
}

pub async fn add_word(ctx: &Ctx, chat: i64, raw: &str) -> Result<bool, AddWordError> {
    let word = normalize(raw);
    if word.is_empty() {
        return Err(AddWordError::Empty);
    }
    if word.chars().count() > MAX_WORD_CHARS || word.contains('=') {
        return Err(AddWordError::TooLong);
    }
    let builtin = is_default_word(&word);
    let custom = ctx.settings.flags_with_prefix(chat, WORDS_PREFIX);
    if builtin.is_none() && custom.len() >= MAX_WORDS && !custom.iter().any(|known| known == &word)
    {
        return Err(AddWordError::Full);
    }
    let changed = match builtin {
        Some(word) => {
            let a = ctx
                .settings
                .set(chat, &format!("{WORDS_PREFIX}{word}"), false)
                .await;
            let b = ctx
                .settings
                .set(chat, &format!("{DISABLED_DEFAULT_PREFIX}{word}"), false)
                .await;
            a || b
        }
        None => {
            ctx.settings
                .set(chat, &format!("{WORDS_PREFIX}{word}"), true)
                .await
        }
    };
    Ok(changed)
}

pub fn word_profile(active_words: &[String]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for word in active_words {
        for byte in word.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

pub async fn handle(ctx: &Ctx, message: &Message) -> bool {
    let text = message.text().trim();
    if LIST_COMMANDS.contains(&text) {
        let Some(chat) = message.peer_id().bot_api_dialog_id() else {
            return false;
        };
        if !super::limits::allows(ctx, message, super::limits::SET).await {
            return true;
        }
        let active = words(ctx, chat);
        let body = if active.is_empty() {
            "لیست کلمات فعال ویس خالی است.\n\nبرای افزودن: «فیلتر ویس کلمه» را ریپلای کنید یا کلمه را بعدش بنویسید."
                .to_owned()
        } else {
            let shown = active
                .iter()
                .take(50)
                .cloned()
                .collect::<Vec<_>>()
                .join("، ");
            let tail = if active.len() > 50 {
                "\n… موارد بیشتری وجود دارد."
            } else {
                ""
            };
            format!(
                "کلمات فعال ویس ({})\n\n{}{}\n\nبرای حذف: «حذف کلمه ویس» را با همان کلمه بفرستید.",
                active.len(),
                shown,
                tail
            )
        };
        let _ = message.reply(body).await;
        return true;
    }

    let Some((add, command, inline)) = parse_command(text) else {
        return false;
    };
    if !super::limits::allows(ctx, message, super::limits::SET).await {
        return true;
    }
    let Some(chat) = message.peer_id().bot_api_dialog_id() else {
        return false;
    };
    let asked = if inline.is_empty() {
        message
            .get_reply()
            .await
            .ok()
            .flatten()
            .map(|replied| replied.text().trim().to_owned())
    } else {
        Some(inline.to_owned())
    };
    let Some(asked) = asked.filter(|word| !word.is_empty()) else {
        let _ = message
            .reply(format!(
                "کلمه را بعد از «{command}» بنویسید یا روی پیام آن ریپلای کنید."
            ))
            .await;
        return true;
    };

    if add {
        let (mark, result) = match add_word(ctx, chat, &asked).await {
            Ok(true) => ("✓", "به فیلتر ویس اضافه شد".to_owned()),
            Ok(false) => ("✓", "از قبل در فیلتر ویس بود".to_owned()),
            Err(AddWordError::Empty) => {
                let _ = message
                    .reply(super::premium::icon_text(
                        Some(super::premium::Icon::ErrorRed),
                        "این کلمه قابل استفاده نیست.",
                    ))
                    .await;
                return true;
            }
            Err(AddWordError::TooLong) => {
                let _ = message
                    .reply(super::premium::icon_text(Some(super::premium::Icon::ErrorRed), format!(
                        "این کلمه پذیرفته نمی شود: حداکثر {MAX_WORD_CHARS} نویسه و بدون «=» باشد."
                    )))
                    .await;
                return true;
            }
            Err(AddWordError::Full) => {
                let _ = message
                    .reply(format!("لیست کلمات سفارشی ویس پر است ({MAX_WORDS} کلمه)."))
                    .await;
                return true;
            }
        };
        let word = normalize(&asked);
        let _ = message
            .reply(super::premium::icon_text(
                Some(super::premium::Icon::Voice),
                format!("{mark} «{word}» {result}."),
            ))
            .await;
        return true;
    }

    let word = normalize(&asked);
    if word.is_empty() {
        let _ = message
            .reply(super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                "این کلمه قابل استفاده نیست.",
            ))
            .await;
        return true;
    }
    if word.chars().count() > MAX_WORD_CHARS || word.contains('=') {
        let _ = message
            .reply(super::premium::icon_text(
                Some(super::premium::Icon::ErrorRed),
                format!("این کلمه پذیرفته نمی شود: حداکثر {MAX_WORD_CHARS} نویسه و بدون «=» باشد."),
            ))
            .await;
        return true;
    }
    let builtin = is_default_word(&word);
    let changed = match builtin {
        Some(word) => {
            let custom_removed = ctx
                .settings
                .set(chat, &format!("{WORDS_PREFIX}{word}"), false)
                .await;
            let disabled = ctx
                .settings
                .set(chat, &format!("{DISABLED_DEFAULT_PREFIX}{word}"), true)
                .await;
            custom_removed || disabled
        }
        None => {
            ctx.settings
                .set(chat, &format!("{WORDS_PREFIX}{word}"), false)
                .await
        }
    };
    let (mark, result) = if changed {
        ("✗", "از فیلتر ویس حذف شد")
    } else {
        ("✗", "در فیلتر ویس نبود")
    };
    let _ = message
        .reply(super::premium::icon_text(
            Some(super::premium::Icon::Voice),
            format!("{mark} «{word}» {result}."),
        ))
        .await;
    true
}

fn parse_command(text: &str) -> Option<(bool, &'static str, &str)> {
    for (commands, add) in [(ADD_COMMANDS, true), (REMOVE_COMMANDS, false)] {
        for command in commands {
            let Some(rest) = text.strip_prefix(command) else {
                continue;
            };
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return Some((add, command, rest.trim()));
            }
        }
    }
    None
}

pub async fn watch(ctx: &Arc<Ctx>, message: &Message, chat: i64, view: &View<'_>) {
    if !locks::is_voice(view)
        || ctx.settings.is_locked(chat, "voice")
        || !ctx.settings.is_locked(chat, MODE)
    {
        return;
    }

    let Some(Media::Document(document)) = view.media() else {
        return;
    };
    let Some(duration) = document.duration() else {
        return;
    };
    if !(0.0..=MAX_DURATION_SECONDS).contains(&duration)
        || document.size().is_some_and(|size| size > MAX_FILE_BYTES)
    {
        return;
    }

    let file = document.id();
    let active_words = words(ctx, chat);
    let profile = word_profile(&active_words);
    if let Some(bad) = ctx.known_voice(file, profile) {
        if bad {
            let text = ctx.known_voice_text(file, profile);
            moderate(ctx, message, chat, text.as_deref()).await;
        }
        return;
    }

    let ctx = Arc::clone(ctx);
    let message = message.clone();
    let document = document.clone();
    let job = ctx.voice_job_slot().await;
    tokio::spawn(async move {
        let _job = job;
        analyze(
            ctx,
            message,
            chat,
            file,
            document,
            duration,
            (active_words, profile),
        )
        .await;
    });
}

async fn analyze(
    ctx: Arc<Ctx>,
    message: Message,
    chat: i64,
    file: i64,
    document: grammers_client::media::Document,
    duration: f64,
    word_config: (Vec<String>, u64),
) {
    let (active_words, profile) = word_config;
    let input = input_path(file);
    if let Err(error) = ctx.client.download_media(&document, &input).await {
        eprintln!("voice monitor: could not download {file} in {chat}: {error}");
        remove_input(&input).await;
        return;
    }

    let output = tokio::time::timeout(
        JOB_TIMEOUT,
        ctx.voice_pool().recognize(input.clone(), duration),
    )
    .await;
    remove_input(&input).await;

    let report = match output {
        Ok(Ok(report)) => report,
        Ok(Err(error)) => {
            eprintln!("voice monitor: recognizer failed for {file} in {chat}: {error}");
            return;
        }
        Err(_) => {
            eprintln!("voice monitor: recognizer timed out for {file} in {chat}");
            return;
        }
    };
    if report.usable_windows == 0 {
        return;
    }

    let bad = report
        .transcripts
        .iter()
        .filter(|transcript| transcript.confidence.is_none_or(|score| score >= 0.35))
        .any(|transcript| contains_bad_word_with(&transcript.text, &active_words));
    let transcript = bad.then(|| transcript_text(&report));
    ctx.remember_voice(file, profile, bad, transcript.clone());
    if bad {
        moderate(&ctx, &message, chat, transcript.as_deref()).await;
    }
}

async fn moderate(ctx: &Arc<Ctx>, message: &Message, chat: i64, transcript: Option<&str>) {
    if let Err(error) = message.delete().await {
        eprintln!("voice monitor: could not delete in {chat}: {error}");
        return;
    }
    ctx.bump(chat, stats::DELETED);

    let chances = match super::strict::punish(ctx, message, chat, MODE).await {
        super::strict::Outcome::Announced => {
            let action = super::cases::action_key(super::strict::action_of(ctx, chat));
            super::cases::record_delete(ctx, message, MODE, "واژه نامناسب در ویس", action).await;
            return;
        }
        super::strict::Outcome::Chances(left) => Some(left),
        super::strict::Outcome::Nothing => None,
    };
    super::cases::record_delete(ctx, message, MODE, "واژه نامناسب در ویس", "delete").await;
    let markup = transcript.filter(|text| !text.is_empty()).map(|text| {
        let speaker = message.sender_id().and_then(PeerId::bare_id);
        let key = ctx.remember_filtered_voice(chat, speaker, text.to_owned());
        super::premium::buttons(&[vec![Button::data(
            "متن تشخیص داده شده ویس",
            format!("v:{key}").into_bytes(),
        )]])
    });
    notice::send_with_markup(ctx, message, chat, "واژه نامناسب در ویس", chances, markup).await;
}

pub async fn on_callback(ctx: &Ctx, query: &CallbackQuery, payload: &str, chat: i64) {
    let Ok(key) = payload.parse::<u64>() else {
        return;
    };
    let Some((stored_chat, speaker, text)) = ctx.filtered_voice(key) else {
        let _ = query
            .answer()
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Timer),
                "متن ویس منقضی شده است.",
            ))
            .send()
            .await;
        return;
    };
    let Some(presser) = query.sender_id().bare_id() else {
        return;
    };
    if stored_chat != chat {
        let _ = query
            .answer()
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Locked),
                "این دکمه برای این گروه نیست.",
            ))
            .send()
            .await;
        return;
    }

    let is_admin = match ctx.chat_ref(chat).or(query.peer_ref().await.ok().flatten()) {
        Some(chat_ref) => super::is_admin(ctx, chat_ref, chat, presser).await,
        None => false,
    };
    if speaker != Some(presser) && !is_admin {
        let _ = query
            .answer()
            .alert(super::premium::plain_label(
                Some(super::premium::Icon::Locked),
                "این دکمه برای فرستنده ویس و ادمین ها است.",
            ))
            .send()
            .await;
        return;
    }

    let answer = if text.chars().count() > CALLBACK_TEXT_CHARS {
        format!(
            "{}…",
            text.chars().take(CALLBACK_TEXT_CHARS).collect::<String>()
        )
    } else {
        text
    };
    let _ = query.answer().alert(answer).send().await;
}

fn input_path(file: i64) -> PathBuf {
    let job = NEXT_INPUT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "groupbot-voice-{}-{file}-{job}.ogg",
        std::process::id()
    ))
}

async fn remove_input(path: &PathBuf) {
    let _ = tokio::fs::remove_file(path).await;
}

pub fn normalize(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    for character in text.chars() {
        let character = match character {
            'ي' | 'ى' => 'ی',
            'ك' => 'ک',
            'ۀ' | 'ة' => 'ه',
            'ؤ' => 'و',
            'إ' | 'أ' | 'ٱ' => 'ا',
            '\u{200c}' | '\u{200f}' | '\u{202a}'..='\u{202e}' => ' ',
            '\u{064b}'..='\u{065f}' | '\u{0670}' => continue,
            character if character.is_alphanumeric() => character.to_lowercase().next().unwrap(),
            _ => ' ',
        };
        normalized.push(character);
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
fn contains_bad_word(text: &str) -> bool {
    contains_bad_word_with(text, &active_words(&[], &[]))
}

pub fn contains_bad_word_with(text: &str, active_words: &[String]) -> bool {
    let normalized = format!(" {} ", normalize(text));
    active_words
        .iter()
        .any(|word| contains_normalized_word(&normalized, word))
}

fn contains_normalized_word(normalized: &str, word: &str) -> bool {
    let word = format!(" {} ", normalize(word));
    normalized.contains(&word)
}

fn transcript_text(report: &RecognitionReport) -> String {
    let text = report
        .transcripts
        .iter()
        .filter(|transcript| transcript.confidence.is_none_or(|score| score >= 0.35))
        .map(|transcript| transcript.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    text.chars().take(MAX_TRANSCRIPT_TEXT_CHARS).collect()
}

fn active_words(disabled: &[String], custom: &[String]) -> Vec<String> {
    let disabled = disabled
        .iter()
        .map(|word| normalize(word))
        .collect::<HashSet<_>>();
    let mut active = DEFAULT_BAD_WORDS
        .iter()
        .map(|word| (*word).to_owned())
        .filter(|word| !disabled.contains(&normalize(word)))
        .collect::<Vec<_>>();
    for word in custom
        .iter()
        .map(|word| normalize(word))
        .filter(|word| !word.is_empty())
    {
        if !active.iter().any(|known| normalize(known) == word) {
            active.push(word);
        }
    }
    active.sort_unstable();
    active
}

fn is_default_word(word: &str) -> Option<&'static str> {
    let normalized = normalize(word);
    DEFAULT_BAD_WORDS
        .iter()
        .copied()
        .find(|default| normalize(default) == normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_arabic_forms_and_zero_width_joiners() {
        assert_eq!(normalize("كِس\u{200c}ها و ي"), "کس ها و ی");
    }

    #[test]
    fn matches_words_without_matching_clean_substrings() {
        assert!(contains_bad_word("این یک مادرجنده است"));
        assert!(contains_bad_word("این یک کیری است"));
        assert!(!contains_bad_word("کسی کتاب را آورد"));
    }

    #[test]
    fn default_words_are_exact_and_can_be_removed_or_restored() {
        let defaults = active_words(&[], &[]);
        assert_eq!(defaults.len(), DEFAULT_BAD_WORDS.len());
        assert!(
            defaults
                .iter()
                .all(|word| DEFAULT_BAD_WORDS.contains(&word.as_str()))
        );
        assert!(!active_words(&["جنده".to_owned()], &[]).contains(&"جنده".to_owned()));
        assert_eq!(
            active_words(&[], &["جنده".to_owned()])
                .iter()
                .filter(|word| *word == "جنده")
                .count(),
            1
        );
        assert!(active_words(&[], &["احمق".to_owned()]).contains(&"احمق".to_owned()));
    }

    #[test]
    fn admins_can_customize_detection_without_masking_the_transcript() {
        assert!(!contains_bad_word("این آدم احمق است"));
        let active = active_words(&[], &["احمق".to_owned(), "اسم رمز".to_owned()]);
        assert!(contains_bad_word_with("این آدم احمق است", &active));
        assert_eq!(
            transcript_text(&RecognitionReport {
                transcripts: vec![Transcript {
                    text: "این آدم مادرجنده است و اسم رمز را گفت".to_owned(),
                    confidence: Some(0.9),
                }],
                usable_windows: 1,
            }),
            "این آدم مادرجنده است و اسم رمز را گفت"
        );
    }

    #[test]
    fn a_word_list_profile_changes_when_active_words_change() {
        assert_ne!(
            word_profile(&active_words(&[], &[])),
            word_profile(&active_words(&["جنده".to_owned()], &[]))
        );
        assert_ne!(
            word_profile(&active_words(&[], &[])),
            word_profile(&active_words(&[], &["احمق".to_owned()]))
        );
    }

    #[test]
    fn custom_word_commands_keep_add_and_remove_distinct() {
        assert_eq!(
            parse_command("فیلتر ویس کلمه تبلیغ"),
            Some((true, "فیلتر ویس کلمه", "تبلیغ"))
        );
        assert_eq!(
            parse_command("حذف کلمه ویس تبلیغ"),
            Some((false, "حذف کلمه ویس", "تبلیغ"))
        );
        assert!(parse_command("فیلتر ویس کلمه دیگری").is_some());
        assert!(parse_command("فیلتر ویس کلمات").is_some());
        assert!(parse_command("فیلتر ویسکلمه تبلیغ").is_none());
    }
}
