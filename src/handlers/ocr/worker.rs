use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

pub const ENV: &str = "GROUPBOT_OCR_WORKER";
const READY: &[u8] = b"GROUPBOT-OCR-READY-1\n";
const START_DEADLINE: Duration = Duration::from_secs(10);
const READ_DEADLINE: Duration = Duration::from_millis(2_500);
const ENGINE_DEADLINE_MS: c_int = 2_000;
const IDLE: Duration = Duration::from_secs(600);
const KEPT: usize = 8;
const MAX_SIDE: u32 = 4096;
const TSV_HEADER: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Output {
    Text,
    Tsv,
}

type Api = *mut c_void;

struct Engine {
    _library: libloading::Library,
    api: Api,
    monitor: *mut c_void,
    set_page_seg_mode: unsafe extern "C" fn(Api, c_int),
    set_variable: unsafe extern "C" fn(Api, *const c_char, *const c_char) -> c_int,
    set_image: unsafe extern "C" fn(Api, *const u8, c_int, c_int, c_int, c_int),
    recognize: unsafe extern "C" fn(Api, *mut c_void) -> c_int,
    utf8_text: unsafe extern "C" fn(Api) -> *mut c_char,
    tsv_text: unsafe extern "C" fn(Api, c_int) -> *mut c_char,
    delete_text: unsafe extern "C" fn(*const c_char),
    clear: unsafe extern "C" fn(Api),
    set_deadline: unsafe extern "C" fn(*mut c_void, c_int),
}

impl Engine {
    fn load(directory: &std::path::Path) -> Result<Self, String> {
        let tessdata = CString::new(directory.join("tessdata").to_string_lossy().as_bytes())
            .map_err(|e| e.to_string())?;
        unsafe {
            let library = libloading::Library::new(directory.join("libtesseract.so.5"))
                .map_err(|e| format!("could not load libtesseract: {e}"))?;
            macro_rules! symbol {
                ($name:literal) => {
                    *library
                        .get($name)
                        .map_err(|e| format!("libtesseract lacks a symbol: {e}"))?
                };
            }
            let create: unsafe extern "C" fn() -> Api = symbol!(b"TessBaseAPICreate\0");
            let init: unsafe extern "C" fn(Api, *const c_char, *const c_char) -> c_int =
                symbol!(b"TessBaseAPIInit3\0");
            let create_monitor: unsafe extern "C" fn() -> *mut c_void =
                symbol!(b"TessMonitorCreate\0");
            let engine = Engine {
                set_page_seg_mode: symbol!(b"TessBaseAPISetPageSegMode\0"),
                set_variable: symbol!(b"TessBaseAPISetVariable\0"),
                set_image: symbol!(b"TessBaseAPISetImage\0"),
                recognize: symbol!(b"TessBaseAPIRecognize\0"),
                utf8_text: symbol!(b"TessBaseAPIGetUTF8Text\0"),
                tsv_text: symbol!(b"TessBaseAPIGetTsvText\0"),
                delete_text: symbol!(b"TessDeleteText\0"),
                clear: symbol!(b"TessBaseAPIClear\0"),
                set_deadline: symbol!(b"TessMonitorSetDeadlineMSecs\0"),
                api: create(),
                monitor: create_monitor(),
                _library: library,
            };
            if engine.api.is_null() || engine.monitor.is_null() {
                return Err("libtesseract would not create an engine".to_owned());
            }
            if init(engine.api, tessdata.as_ptr(), c"fas+eng".as_ptr()) != 0 {
                return Err("libtesseract would not load fas+eng".to_owned());
            }
            (engine.set_variable)(engine.api, c"user_defined_dpi".as_ptr(), c"150".as_ptr());
            Ok(engine)
        }
    }

    fn read(&self, request: &Request) -> Result<String, String> {
        let whitelist = CString::new(request.whitelist.as_bytes()).map_err(|e| e.to_string())?;
        unsafe {
            (self.set_page_seg_mode)(self.api, c_int::from(request.psm));
            (self.set_variable)(self.api, c"tessedit_char_whitelist".as_ptr(), whitelist.as_ptr());
            (self.set_image)(
                self.api,
                request.pixels.as_ptr(),
                request.width as c_int,
                request.height as c_int,
                3,
                request.width as c_int * 3,
            );
            (self.set_deadline)(self.monitor, ENGINE_DEADLINE_MS);
            let recognized = (self.recognize)(self.api, self.monitor);
            let raw = match (recognized, request.output) {
                (0, Output::Text) => (self.utf8_text)(self.api),
                (0, Output::Tsv) => (self.tsv_text)(self.api, 0),
                _ => std::ptr::null_mut(),
            };
            let text = (!raw.is_null()).then(|| CStr::from_ptr(raw).to_string_lossy().into_owned());
            if !raw.is_null() {
                (self.delete_text)(raw);
            }
            (self.clear)(self.api);
            let text = text.ok_or_else(|| "Tesseract exceeded the two-second deadline".to_owned())?;
            Ok(match request.output {
                Output::Text => text,
                Output::Tsv => format!("{TSV_HEADER}{text}"),
            })
        }
    }
}

struct Request {
    output: Output,
    psm: u8,
    whitelist: String,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Request {
    fn write(&self, to: &mut impl Write) -> std::io::Result<()> {
        let mut head = vec![u8::from(self.output == Output::Tsv), self.psm];
        head.extend_from_slice(&(self.whitelist.len() as u16).to_le_bytes());
        head.extend_from_slice(self.whitelist.as_bytes());
        head.extend_from_slice(&self.width.to_le_bytes());
        head.extend_from_slice(&self.height.to_le_bytes());
        to.write_all(&head)?;
        to.write_all(&self.pixels)?;
        to.flush()
    }

    fn read(from: &mut impl Read) -> std::io::Result<Option<Self>> {
        let mut head = [0u8; 4];
        match from.read_exact(&mut head) {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            other => other?,
        }
        let invalid = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_owned());
        let mut whitelist = vec![0u8; usize::from(u16::from_le_bytes([head[2], head[3]]))];
        from.read_exact(&mut whitelist)?;
        let mut size = [0u8; 8];
        from.read_exact(&mut size)?;
        let width = u32::from_le_bytes(size[..4].try_into().unwrap());
        let height = u32::from_le_bytes(size[4..].try_into().unwrap());
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(invalid("picture size out of bounds"));
        }
        let mut pixels = vec![0u8; width as usize * height as usize * 3];
        from.read_exact(&mut pixels)?;
        Ok(Some(Request {
            output: if head[0] == 1 { Output::Tsv } else { Output::Text },
            psm: head[1],
            whitelist: String::from_utf8(whitelist).map_err(|_| invalid("whitelist is not UTF-8"))?,
            width,
            height,
            pixels,
        }))
    }
}

fn write_answer(to: &mut impl Write, answer: &Result<String, String>) -> std::io::Result<()> {
    let (status, body) = match answer {
        Ok(text) => (0u8, text.as_bytes()),
        Err(error) => (1u8, error.as_bytes()),
    };
    to.write_all(&[status])?;
    to.write_all(&(body.len() as u32).to_le_bytes())?;
    to.write_all(body)?;
    to.flush()
}

fn read_answer(from: &mut impl Read) -> std::io::Result<Result<String, String>> {
    let mut head = [0u8; 5];
    from.read_exact(&mut head)?;
    let length = u32::from_le_bytes(head[1..].try_into().unwrap()) as usize;
    if length > 1 << 20 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "answer too long"));
    }
    let mut body = vec![0u8; length];
    from.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).into_owned();
    Ok(if head[0] == 0 { Ok(body) } else { Err(body) })
}

pub fn serve() -> i32 {
    let Some(directory) = std::env::var_os(ENV) else {
        return 2;
    };
    let engine = match Engine::load(std::path::Path::new(&directory)) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("ocr worker: {error}");
            return 1;
        }
    };
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    if output.write_all(READY).and_then(|()| output.flush()).is_err() {
        return 1;
    }
    loop {
        let request = match Request::read(&mut input) {
            Ok(Some(request)) => request,
            Ok(None) => return 0,
            Err(_) => return 1,
        };
        if write_answer(&mut output, &engine.read(&request)).is_err() {
            return 1;
        }
    }
}

struct Worker {
    child: Child,
    pipes: Option<(ChildStdin, ChildStdout)>,
    used: Instant,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static KEPT_WORKERS: Mutex<Vec<Worker>> = Mutex::new(Vec::new());
static UNAVAILABLE: AtomicBool = AtomicBool::new(false);
static REAPING: AtomicBool = AtomicBool::new(false);

enum Failed {
    Worker(String),
    Deadline,
}

fn on_pipes<T: Send + 'static>(
    worker: &mut Worker,
    deadline: Duration,
    exchange: impl FnOnce(&mut ChildStdin, &mut ChildStdout) -> std::io::Result<T> + Send + 'static,
) -> Result<T, Failed> {
    let (mut stdin, mut stdout) = worker
        .pipes
        .take()
        .ok_or_else(|| Failed::Worker("the worker lost its pipes".to_owned()))?;
    let (done, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let result = exchange(&mut stdin, &mut stdout);
        let _ = done.send((stdin, stdout, result));
    });
    match answer.recv_timeout(deadline) {
        Ok((stdin, stdout, Ok(value))) => {
            worker.pipes = Some((stdin, stdout));
            Ok(value)
        }
        Ok((_, _, Err(error))) => Err(Failed::Worker(error.to_string())),
        Err(_) => Err(Failed::Deadline),
    }
}

fn start(mut command: Command) -> Result<Worker, String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start an OCR worker: {e}"))?;
    let pipes = child.stdin.take().zip(child.stdout.take());
    let mut worker = Worker {
        child,
        pipes,
        used: Instant::now(),
    };
    let ready = on_pipes(&mut worker, START_DEADLINE, |_, stdout| {
        let mut seen = Vec::new();
        let mut byte = [0u8; 1];
        while !seen.ends_with(READY) {
            stdout.read_exact(&mut byte)?;
            seen.push(byte[0]);
            if seen.len() > 64 * 1024 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no ready line"));
            }
        }
        Ok(())
    });
    match ready {
        Ok(()) => Ok(worker),
        Err(Failed::Worker(error)) => Err(format!("the OCR worker did not start: {error}")),
        Err(Failed::Deadline) => Err("the OCR worker did not start in time".to_owned()),
    }
}

fn checkout(command: impl FnOnce() -> Option<Command>) -> Option<Worker> {
    let kept = KEPT_WORKERS.lock().unwrap_or_else(PoisonError::into_inner).pop();
    if kept.is_some() {
        return kept;
    }
    if UNAVAILABLE.load(Ordering::Acquire) {
        return None;
    }
    match command().map(start) {
        Some(Ok(worker)) => Some(worker),
        Some(Err(error)) => {
            UNAVAILABLE.store(true, Ordering::Release);
            log::warn!("advert: {error}; reading by command line from now on");
            None
        }
        None => None,
    }
}

fn give_back(mut worker: Worker) {
    worker.used = Instant::now();
    let mut kept = KEPT_WORKERS.lock().unwrap_or_else(PoisonError::into_inner);
    if kept.len() < KEPT {
        kept.push(worker);
    }
    drop(kept);
    if !REAPING.swap(true, Ordering::AcqRel) {
        std::thread::spawn(reap);
    }
}

fn reap() {
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let mut kept = KEPT_WORKERS.lock().unwrap_or_else(PoisonError::into_inner);
        let idle = kept
            .extract_if(.., |worker| worker.used.elapsed() >= IDLE)
            .collect::<Vec<_>>();
        if kept.is_empty() {
            REAPING.store(false, Ordering::Release);
            drop(kept);
            drop(idle);
            return;
        }
        drop(kept);
        drop(idle);
    }
}

pub fn read(
    command: impl FnOnce() -> Option<Command>,
    image: &image::RgbImage,
    psm: &str,
    whitelist: &str,
    output: Output,
) -> Option<Result<String, String>> {
    let request = Request {
        output,
        psm: psm.parse().ok()?,
        whitelist: whitelist.to_owned(),
        width: image.width(),
        height: image.height(),
        pixels: image.as_raw().clone(),
    };
    if request.width > MAX_SIDE || request.height > MAX_SIDE || request.whitelist.len() > 1024 {
        return None;
    }
    let mut worker = checkout(command)?;
    let answer = on_pipes(&mut worker, READ_DEADLINE, move |stdin, stdout| {
        request.write(stdin)?;
        read_answer(stdout)
    });
    match answer {
        Ok(answer) => {
            give_back(worker);
            Some(answer)
        }
        Err(Failed::Deadline) => Some(Err("Tesseract exceeded the two-second deadline".to_owned())),
        Err(Failed::Worker(error)) => {
            log::warn!("advert: an OCR worker failed ({error}); reading by command line");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_and_an_answer_survive_the_pipe() {
        let request = Request {
            output: Output::Tsv,
            psm: 13,
            whitelist: "۰۱۲0123".to_owned(),
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 4, 5, 6],
        };
        let mut bytes = Vec::new();
        request.write(&mut bytes).unwrap();
        let back = Request::read(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(
            (back.output, back.psm, back.whitelist.as_str(), back.width, back.height),
            (Output::Tsv, 13, "۰۱۲0123", 2, 1)
        );
        assert_eq!(back.pixels, request.pixels);
        assert!(Request::read(&mut &[][..]).unwrap().is_none());

        let mut bytes = Vec::new();
        write_answer(&mut bytes, &Ok("متن".to_owned())).unwrap();
        write_answer(&mut bytes, &Err("deadline".to_owned())).unwrap();
        let mut from = bytes.as_slice();
        assert_eq!(read_answer(&mut from).unwrap(), Ok("متن".to_owned()));
        assert_eq!(read_answer(&mut from).unwrap(), Err("deadline".to_owned()));
    }

    #[test]
    fn an_oversized_picture_is_refused_before_allocation() {
        let mut bytes = vec![0, 6, 0, 0];
        bytes.extend_from_slice(&(MAX_SIDE + 1).to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        assert!(Request::read(&mut bytes.as_slice()).is_err());
    }
}
