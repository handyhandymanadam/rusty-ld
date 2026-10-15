//! Error, warning and fatal-error reporting.
//!
//! Errors don't abort the link immediately: the linker keeps going so that
//! all problems are reported in one run, then exits before writing the
//! output. Fatal errors are for conditions the linker can't continue past.

use std::fmt;
use std::io::{self, Write};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::time::Instant;

/// Whether to demangle symbol names in diagnostics. This is process-wide
/// state so that `Display` implementations, which have no access to the
/// linker context, can consult it.
static DEMANGLE: AtomicBool = AtomicBool::new(true);

pub fn set_demangle(enabled: bool) {
    DEMANGLE.store(enabled, Ordering::Relaxed);
}

pub fn demangle_enabled() -> bool {
    DEMANGLE.load(Ordering::Relaxed)
}

// Diagnostic settings, error state and output serialization are process-wide.
static COLOR: AtomicBool = AtomicBool::new(false);
static FATAL_WARNINGS: AtomicBool = AtomicBool::new(false);
static SUPPRESS_WARNINGS: AtomicBool = AtomicBool::new(false);
static NOINHIBIT_EXEC: AtomicBool = AtomicBool::new(false);
static HAS_ERROR: AtomicBool = AtomicBool::new(false);
static OUTPUT_LOCK: Mutex<()> = Mutex::new(());

pub fn set_color(on: bool) {
    COLOR.store(on, Ordering::Relaxed);
}

pub fn set_fatal_warnings(on: bool) {
    FATAL_WARNINGS.store(on, Ordering::Relaxed);
}

pub fn set_suppress_warnings(on: bool) {
    SUPPRESS_WARNINGS.store(on, Ordering::Relaxed);
}

pub fn set_noinhibit_exec(on: bool) {
    NOINHIBIT_EXEC.store(on, Ordering::Relaxed);
}

pub fn noinhibit_exec() -> bool {
    NOINHIBIT_EXEC.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Fatal,
    Error,
    Warning,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Fatal => "fatal",
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }

    fn prefixes(self) -> (&'static str, &'static str) {
        match self {
            Level::Fatal => ("mold: fatal: ", "mold: \x1b[0;1;31mfatal:\x1b[0m "),
            Level::Error => ("mold: error: ", "mold: \x1b[0;1;31merror:\x1b[0m "),
            Level::Warning => ("mold: warning: ", "mold: \x1b[0;1;35mwarning:\x1b[0m "),
        }
    }
}

// Format each message before taking the lock so diagnostics from different
// threads cannot interleave.
fn emit(prefix_mono: &str, prefix_color: &str, msg: fmt::Arguments) {
    let prefix = if COLOR.load(Ordering::Relaxed) { prefix_color } else { prefix_mono };
    let text = format!("{prefix}{msg}\n");
    let _guard = OUTPUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = io::stderr().write_all(text.as_bytes());
}

/// Extra facts about a diagnostic for `--diagnostics-format=json`. The
/// plain-text output ignores them.
#[derive(Default, Clone)]
pub struct Detail {
    /// A stable code such as `L0001`. If None, one is derived from the
    /// message.
    pub code: Option<&'static str>,
    pub files: Vec<String>,
    pub symbols: Vec<String>,
    /// Lines of the message that are suggestions, not facts, so JSON keeps
    /// them apart from the notes.
    pub suggestions: Vec<Suggestion>,
    /// Where the things the message is about are: the definitions of a
    /// duplicate symbol, the references to an undefined one.
    pub locations: Vec<Location>,
    /// `locations` says what the `notes` say, so below `full` the notes are
    /// left out (a fact is stated once).
    pub locations_replace_notes: bool,
    /// How many more references to the symbol there are than `locations`
    /// lists (an undefined symbol).
    pub references_omitted: usize,
}

#[derive(Clone)]
pub struct Location {
    /// `reference` or `definition`.
    pub role: &'static str,
    /// The input file whose debug info or symbol table gave the location.
    pub object: String,
    /// The source file, when known (or the object file again).
    pub file: String,
    /// The source line, or 0 if it is not known (the field is then left out).
    pub line: u64,
    /// The function that holds a reference, if known.
    pub function: Option<String>,
}

#[derive(Clone)]
pub struct Suggestion {
    pub message: String,
    /// The command-line argument that applies it, if there is one.
    pub option: Option<String>,
    /// A shorter message for below `full`, when `option` already says part
    /// of it.
    pub short: Option<String>,
}

/// The version of the JSON diagnostic format.
pub const JSON_SCHEMA: u32 = 1;

/// How much each JSON record says. `Full` is everything the first version
/// wrote; `Normal` leaves out what repeats another field and every empty or
/// zero field (an absent field means empty, zero or null); `Brief` is the
/// code, the message, one place, one fix and counts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum DetailLevel {
    Brief,
    Normal,
    Full,
}

impl DetailLevel {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "brief" => Some(Self::Brief),
            "normal" => Some(Self::Normal),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

// 0 brief, 1 normal, 2 full. Without agent mode, records stay as they
// always were (full).
static DETAIL: AtomicU8 = AtomicU8::new(2);
// The level named by --agent-detail, 3 if none.
static DETAIL_FLAG: AtomicU8 = AtomicU8::new(3);
static AGENT_ON: AtomicBool = AtomicBool::new(false);

pub fn detail_level() -> DetailLevel {
    match DETAIL.load(Ordering::Relaxed) {
        0 => DetailLevel::Brief,
        1 => DetailLevel::Normal,
        _ => DetailLevel::Full,
    }
}

/// True below `full`.
pub fn is_terse() -> bool {
    detail_level() != DetailLevel::Full
}

/// Records the level of `--agent-detail`, or clears it.
pub fn set_detail_flag(level: Option<DetailLevel>) {
    DETAIL_FLAG.store(level.map_or(3, |l| l as u8), Ordering::Relaxed);
    refresh_detail();
}

/// Agent mode is on or off. The records are `normal` in agent mode unless
/// `--agent-detail` or `RUSTY_AGENT_DETAIL` says otherwise, and `full`
/// (the format before the levels existed) without it.
pub fn set_agent_on(on: bool) {
    AGENT_ON.store(on, Ordering::Relaxed);
    refresh_detail();
}

pub fn agent_on() -> bool {
    AGENT_ON.load(Ordering::Relaxed)
}

fn refresh_detail() {
    let flag = DETAIL_FLAG.load(Ordering::Relaxed);
    let level = if flag != 3 {
        flag
    } else if AGENT_ON.load(Ordering::Relaxed) {
        std::env::var("RUSTY_AGENT_DETAIL")
            .ok()
            .and_then(|v| DetailLevel::parse(v.trim()))
            .map_or(1, |l| l as u8)
    } else {
        2
    };
    DETAIL.store(level, Ordering::Relaxed);
}

/// Builds one JSON object, `schema` and `type` first.
pub struct JsonObj {
    s: String,
}

impl JsonObj {
    pub fn new(ty: &str) -> Self {
        Self { s: format!("{{\"schema\":{JSON_SCHEMA},\"type\":\"{ty}\"") }
    }

    fn key(&mut self, k: &str) {
        self.s.push_str(",\"");
        self.s.push_str(k);
        self.s.push_str("\":");
    }

    pub fn str(&mut self, k: &str, v: &str) -> &mut Self {
        self.key(k);
        json_string(&mut self.s, v);
        self
    }

    pub fn num(&mut self, k: &str, v: u64) -> &mut Self {
        self.key(k);
        self.s.push_str(&v.to_string());
        self
    }

    pub fn bool(&mut self, k: &str, v: bool) -> &mut Self {
        self.key(k);
        self.s.push_str(if v { "true" } else { "false" });
        self
    }

    pub fn null(&mut self, k: &str) -> &mut Self {
        self.key(k);
        self.s.push_str("null");
        self
    }

    pub fn strs(&mut self, k: &str, v: &[String]) -> &mut Self {
        self.key(k);
        json_array(&mut self.s, v);
        self
    }

    /// `v` must already be valid JSON.
    pub fn raw(&mut self, k: &str, v: &str) -> &mut Self {
        self.key(k);
        self.s.push_str(v);
        self
    }

    pub fn finish(mut self) -> String {
        self.s.push('}');
        self.s
    }
}

/// With `--stable-diagnostics` in agent mode, writes the random part of a
/// temporary file name as `XXXXXX`, so that two runs of the same link give
/// the same text: `/tmp/cc3Ka9Zq.o` and `/tmp/a-1b2c3d.o` under the
/// temporary directories only.
pub fn stable_text(s: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if !(STABLE.load(Ordering::Relaxed) && AGENT_ON.load(Ordering::Relaxed)) {
        return Cow::Borrowed(s);
    }
    let tmp = std::env::var("TMPDIR").unwrap_or_default();
    let mut dirs: Vec<&str> = vec!["/tmp/", "/var/tmp/"];
    if !tmp.is_empty() && tmp != "/tmp" {
        dirs.push(tmp.trim_end_matches('/'));
    }
    if !dirs.iter().any(|d| s.contains(*d)) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    // Handle each whitespace-free word, so a path inside a message works.
    for (i, word) in s.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let under_tmp = dirs.iter().any(|d| word.contains(*d));
        if !under_tmp {
            out.push_str(word);
            continue;
        }
        let (dir, name) = match word.rfind('/') {
            Some(k) => word.split_at(k + 1),
            None => ("", word),
        };
        let fixed = stable_name(name);
        if fixed != name {
            changed = true;
        }
        out.push_str(dir);
        out.push_str(&fixed);
    }
    if changed { Cow::Owned(out) } else { Cow::Borrowed(s) }
}

/// `ccAbC123.o` -> `ccXXXXXX.o`, `a-1b2c3d.o` -> `a-XXXXXX.o`,
/// `tmp.AbCdEfGhIj` -> `tmp.XXXXXXXXXX`.
fn stable_name(name: &str) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(k) if k > 0 => name.split_at(k),
        _ => (name, ""),
    };
    let alnum = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric());
    if let Some(rest) = stem.strip_prefix("cc")
        && rest.len() == 6
        && alnum(rest)
    {
        return format!("ccXXXXXX{ext}");
    }
    if let Some((head, tail)) = stem.rsplit_once('-')
        && tail.len() == 6
        && alnum(tail)
        && tail.bytes().any(|b| b.is_ascii_digit())
    {
        return format!("{head}-XXXXXX{ext}");
    }
    if let Some(tail) = name.strip_prefix("tmp.")
        && tail.len() >= 6
        && alnum(tail)
    {
        return format!("tmp.{}", "X".repeat(tail.len()));
    }
    name.to_string()
}

/// What a record is for the size budget.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Class {
    /// An error: kept longest.
    Error,
    /// The answer to something the caller asked for.
    Requested,
    /// A warning.
    Warning,
    /// The link summary: always printed.
    Summary,
}

const SOFT_BUDGET: usize = 12 * 1024;
const HARD_BUDGET: usize = 20 * 1024;

#[derive(Default)]
struct Budget {
    used: usize,
    omitted: std::collections::BTreeMap<&'static str, usize>,
}

static BUDGET: Mutex<Budget> = Mutex::new(Budget { used: 0, omitted: std::collections::BTreeMap::new() });

/// Whether a record of `len` bytes may go on standard output or error. At
/// `normal` and `brief`, after 12 KB only errors and requested answers go
/// on, and after 20 KB nothing but the summary. The agent channels always
/// get every record. Counts what it lets through.
fn budget_allows(len: usize, class: Class, ty: &'static str) -> bool {
    if !AGENT_ON.load(Ordering::Relaxed) || !is_terse() {
        return true;
    }
    let mut b = BUDGET.lock().unwrap_or_else(|e| e.into_inner());
    let ok = match class {
        Class::Summary => true,
        _ if b.used >= HARD_BUDGET => false,
        Class::Error | Class::Requested => true,
        Class::Warning => b.used < SOFT_BUDGET,
    };
    if ok {
        b.used += len;
    } else {
        *b.omitted.entry(ty).or_insert(0) += 1;
    }
    ok
}

/// Says, in one last record, how many records the size budget kept off the
/// screen. The agent channels have them all. Does nothing if none were left
/// out; safe to call twice.
pub fn finish_records() {
    let omitted = std::mem::take(&mut BUDGET.lock().unwrap_or_else(|e| e.into_inner()).omitted);
    if omitted.is_empty() {
        return;
    }
    let total: usize = omitted.values().sum();
    let mut by = String::from("{");
    for (i, (k, v)) in omitted.iter().enumerate() {
        if i > 0 {
            by.push(',');
        }
        by.push_str(&format!("\"{k}\":{v}"));
    }
    by.push('}');
    let mut o = JsonObj::new("records-omitted");
    o.num("records_omitted", total as u64).raw("by_type", &by).str(
        "more",
        "the size budget (12 KB, 20 KB) kept these off the screen; RUSTY_AGENT_OUT has every record, --agent-detail=full has no budget",
    );
    let mut text = o.finish();
    text.push('\n');
    let _guard = OUTPUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = io::stderr().write_all(text.as_bytes());
}

/// Writes a record to standard error if the budget allows, and returns
/// whether it did. The caller copies it to the channels.
fn write_err_record(text: &str, class: Class, ty: &'static str) {
    if budget_allows(text.len(), class, ty) {
        let _ = io::stderr().write_all(text.as_bytes());
    }
}

static ERRORS: AtomicUsize = AtomicUsize::new(0);
static WARNINGS: AtomicUsize = AtomicUsize::new(0);
static START: OnceLock<Instant> = OnceLock::new();
static SUMMARY_PATH: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// Starts the clock that --link-summary reports. The first call wins.
pub fn start_clock() {
    START.get_or_init(Instant::now);
}

pub fn elapsed_seconds() -> f64 {
    START.get().map_or(0.0, |t| t.elapsed().as_secs_f64())
}

pub fn error_count() -> usize {
    ERRORS.load(Ordering::Relaxed)
}

pub fn warning_count() -> usize {
    WARNINGS.load(Ordering::Relaxed)
}

/// Asks for a summary file even if the link fails.
pub fn set_summary_path(path: Option<std::path::PathBuf>) {
    *SUMMARY_PATH.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

/// Whether run-specific times are left out of records: stable mode in agent
/// mode.
pub fn hide_times() -> bool {
    STABLE.load(Ordering::Relaxed) && AGENT_ON.load(Ordering::Relaxed)
}

/// Sends the link-summary record to the agent channels and, in agent mode
/// with JSON records, to standard error, so that a harness that reads that
/// stream gets it too. (The file `--link-summary` names gets it as well.)
pub fn summary_record(text: &str) {
    if AGENT_ON.load(Ordering::Relaxed) && JSON.load(Ordering::Relaxed) {
        let _guard = OUTPUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        write_err_record(text, Class::Summary, "link-summary");
    }
    tee_record(text);
}

/// Writes the summary of a link that failed. The linker calls its own
/// function for one that worked, which clears the path first.
fn write_failure_summary() {
    let Some(path) = SUMMARY_PATH.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let full = detail_level() == DetailLevel::Full;
    let (errors, warnings) = (error_count(), warning_count());
    let mut o = JsonObj::new("link-summary");
    o.str("status", "failed");
    if !hide_times() {
        o.raw("elapsed_seconds", &format!("{:.3}", elapsed_seconds()));
    }
    if full || errors > 0 {
        o.num("errors", errors as u64);
    }
    if full || warnings > 0 {
        o.num("warnings", warnings as u64);
    }
    let mut text = o.finish();
    text.push('\n');
    summary_record(&text);
    if let Err(e) = std::fs::write(&path, &text) {
        summary_write_warning(&path, &e);
    }
}

/// The summary file cannot be written. In agent mode that is a warning like
/// those of the copy channels; the link has already worked or failed for
/// other reasons. Without agent mode the link fails, as it always did.
pub fn summary_write_warning(path: &std::path::Path, e: &io::Error) {
    if AGENT_ON.load(Ordering::Relaxed) {
        channel_warning(
            "L0014",
            "--link-summary",
            &format!("cannot write {}: {}", path.display(), strerror(e)),
            "check the path given to --link-summary=FILE".to_string(),
        );
    } else {
        crate::fatal!("--link-summary: cannot write {}: {}", path.display(), strerror(e));
    }
}

static STABLE: AtomicBool = AtomicBool::new(false);
/// Diagnostics held back by --stable-diagnostics: (message, printed form,
/// whether the printed form is a JSON record that the agent channels get).
static HELD: Mutex<Vec<(String, String, bool, Class)>> = Mutex::new(Vec::new());

/// Holds errors and warnings back and prints them sorted, so that the same
/// link prints the same messages in the same order on every run.
pub fn set_stable(on: bool) {
    STABLE.store(on, Ordering::Relaxed);
}

pub fn stable_enabled() -> bool {
    STABLE.load(Ordering::Relaxed)
}

/// Prints the diagnostics held back by [`set_stable`], sorted by message.
pub fn flush_held() {
    let mut held = std::mem::take(&mut *HELD.lock().unwrap_or_else(|e| e.into_inner()));
    if held.is_empty() {
        return;
    }
    held.sort();
    let _guard = OUTPUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut err = io::stderr().lock();
    for (_, text, record, class) in held {
        if record {
            if budget_allows(text.len(), class, "diagnostic") {
                let _ = err.write_all(text.as_bytes());
            }
            tee_record(&text);
        } else {
            let _ = err.write_all(text.as_bytes());
        }
    }
}

/// Where RUSTY_AGENT_OUT and RUSTY_AGENT_FD send a copy of every JSON
/// record. Open only in agent mode.
#[derive(Default)]
struct Channels {
    file: Option<std::fs::File>,
    fd: Option<std::fs::File>,
}

static CHANNELS: Mutex<Channels> = Mutex::new(Channels { file: None, fd: None });
/// Agent mode is on, so the channels are to be opened; they are opened by
/// the first record or the end of option parsing, whichever comes first, so
/// that RUSTY_AGENT=1 followed by --no-agent-mode never touches them.
static CHANNELS_WANTED: AtomicBool = AtomicBool::new(false);
static CHANNELS_ON: AtomicBool = AtomicBool::new(false);

/// Appends one JSON record (a line; the newline is added if missing) to the
/// agent channels, if any are open. Each record is one write, so it is on its
/// way the moment this returns. A channel that stops working is ignored: it
/// is a copy, and the link must not depend on it.
pub fn tee_record(text: &str) {
    if !CHANNELS_ON.load(Ordering::Relaxed) {
        return;
    }
    let mut line = String::with_capacity(text.len() + 1);
    line.push_str(text);
    if !text.ends_with('\n') {
        line.push('\n');
    }
    let mut ch = CHANNELS.lock().unwrap_or_else(|e| e.into_inner());
    let ch = &mut *ch;
    for f in [&mut ch.file, &mut ch.fd].into_iter().flatten() {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Prints a JSON record on standard output, and copies it to the agent
/// channels. `ty` is the record's type, for the size budget's count.
pub fn out_record(ty: &'static str, text: &str) {
    let _guard = OUTPUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if budget_allows(text.len() + 1, Class::Requested, ty) {
        let _ = writeln!(io::stdout().lock(), "{text}");
    }
    tee_record(text);
}

/// Closes the agent channels (--no-agent-mode, or a new run of the option
/// parser). A channel's file descriptor is not ours, but the handle we made
/// for it is, and closing that one does not touch the caller's.
pub fn close_agent_channels() {
    CHANNELS_WANTED.store(false, Ordering::Relaxed);
    CHANNELS_ON.store(false, Ordering::Relaxed);
    *CHANNELS.lock().unwrap_or_else(|e| e.into_inner()) = Channels::default();
}

/// What a bad RUSTY_AGENT_FD is: the reason, as a warning's text.
#[cfg(unix)]
fn open_agent_fd(value: &str) -> Result<std::fs::File, String> {
    use std::fs::OpenOptions;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("`{value}` is not a decimal number"));
    }
    let n: u64 = value.parse().map_err(|_| format!("`{value}` is too large"))?;
    if n < 3 {
        return Err(format!("{n} is below 3 (0, 1 and 2 are standard input, output and error)"));
    }
    // The only safe way to use a descriptor that someone else opened is to
    // open it again through the system's table of open descriptors: that
    // makes a new handle on the same file, pipe or terminal, and leaves the
    // caller's descriptor alone. Linux's fdinfo says how it was opened, so
    // a read-only descriptor is turned down; other systems skip that check.
    let info = format!("/proc/self/fdinfo/{n}");
    let mut proc_dir = true;
    match std::fs::read_to_string(&info) {
        Ok(text) => {
            let flags = text
                .lines()
                .find_map(|l| l.strip_prefix("flags:"))
                .and_then(|v| u32::from_str_radix(v.trim(), 8).ok());
            if flags.is_some_and(|f| f & 3 == 0) {
                return Err(format!("descriptor {n} is open for reading only"));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound && std::path::Path::new("/proc/self").exists() => {
            return Err(format!("descriptor {n} is not open"));
        }
        Err(_) => proc_dir = false,
    }
    let path = if proc_dir { format!("/proc/self/fd/{n}") } else { format!("/dev/fd/{n}") };
    OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| format!("descriptor {n} cannot be written: {}", strerror(&e)))
}

#[cfg(not(unix))]
fn open_agent_fd(_value: &str) -> Result<std::fs::File, String> {
    Err("file descriptors are not supported on this system".to_string())
}

fn channel_warning(code: &'static str, var: &str, problem: &str, hint: String) {
    let detail = Detail {
        code: Some(code),
        suggestions: vec![Suggestion { message: hint.clone(), option: None, short: None }],
        ..Detail::default()
    };
    // A copy channel must never fail the link, so this ignores
    // --fatal-warnings and -w.
    report(
        Level::Warning,
        Some(&detail),
        format_args!("{var}: {problem}\n>>> hint: {hint}"),
    );
}

/// Agent mode has been switched on: the channels are to be opened.
pub fn want_agent_channels() {
    CHANNELS_WANTED.store(true, Ordering::Relaxed);
}

/// Opens the channels that RUSTY_AGENT_OUT (a file to append to) and
/// RUSTY_AGENT_FD (a descriptor the caller left open) name, if agent mode
/// wants them and they are not open yet. A bad value is one warning (L0012
/// for the file, L0013 for the descriptor), never an error. Must not be
/// called with the output lock held.
pub fn ensure_agent_channels() {
    if !CHANNELS_WANTED.load(Ordering::Relaxed) || CHANNELS_ON.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut problems: Vec<(&'static str, &str, String, String)> = Vec::new();
    let mut ch = Channels::default();
    if let Some(path) = std::env::var_os("RUSTY_AGENT_OUT") {
        let shown = path.to_string_lossy().into_owned();
        let hint = "check the path or unset RUSTY_AGENT_OUT".to_string();
        if path.is_empty() {
            problems.push(("L0012", "RUSTY_AGENT_OUT", "the path is empty".into(), hint));
        } else {
            match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => ch.file = Some(f),
                Err(e) => problems.push((
                    "L0012",
                    "RUSTY_AGENT_OUT",
                    format!("cannot open {shown}: {}", strerror(&e)),
                    hint,
                )),
            }
        }
    }
    if let Some(value) = std::env::var_os("RUSTY_AGENT_FD") {
        let hint = "check the descriptor or unset RUSTY_AGENT_FD".to_string();
        match open_agent_fd(&value.to_string_lossy()) {
            Ok(f) => ch.fd = Some(f),
            Err(why) => problems.push(("L0013", "RUSTY_AGENT_FD", why, hint)),
        }
    }
    *CHANNELS.lock().unwrap_or_else(|e| e.into_inner()) = ch;
    // The warnings go out after both channels are open, so each channel
    // that works also gets the warning about the other.
    for (code, var, why, hint) in problems {
        channel_warning(code, var, &why, hint);
    }
}

static JSON: AtomicBool = AtomicBool::new(false);

/// Prints diagnostics as one JSON object per line instead of text.
pub fn set_json(on: bool) {
    JSON.store(on, Ordering::Relaxed);
}

pub fn json_enabled() -> bool {
    JSON.load(Ordering::Relaxed)
}

/// The stable code for a message with no code of its own.
pub fn classify(level: &str, msg: &str) -> &'static str {
    const TABLE: &[(&str, &str)] = &[
        ("undefined symbol: ", "L0001"),
        ("duplicate symbol: ", "L0002"),
        ("library not found", "L0003"),
        ("cannot open ", "L0004"),
        ("unknown command line option", "L0006"),
        ("multiple common symbols", "L0007"),
        ("refers to a discarded COMDAT", "L0008"),
        ("--no-allow-shlib-undefined", "L0009"),
        ("relocation against symbol", "L0010"),
        ("DT_TEXTREL", "L0011"),
    ];
    for (pat, code) in TABLE {
        if msg.contains(pat) {
            return code;
        }
    }
    if msg.contains("relocation") && msg.contains("out of range") {
        return "L0005";
    }
    match level {
        "fatal" => "L0300",
        "error" => "L0100",
        _ => "L0200",
    }
}

pub fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn json_array(out: &mut String, items: &[String]) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_string(out, item);
    }
    out.push(']');
}

/// Collapses the repeats of one "referenced by FILE" note: the first keeps
/// its place and gets the count.
fn collapse_references(notes: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(notes.len());
    let mut counts: Vec<(usize, usize)> = Vec::new(); // (index in out, times)
    for n in notes {
        if n.starts_with("referenced by ") {
            if let Some(&(i, _)) = counts.iter().find(|(i, _)| out[*i] == n) {
                counts.iter_mut().find(|(j, _)| *j == i).unwrap().1 += 1;
                continue;
            }
            counts.push((out.len(), 1));
        }
        out.push(n);
    }
    for (i, times) in counts {
        if times > 1 {
            out[i] = format!("{} ({times} references)", out[i]);
        }
    }
    out
}

/// A path under the current directory is shown relative to it.
fn relative_to_cwd(path: std::borrow::Cow<'_, str>) -> std::borrow::Cow<'_, str> {
    if let Ok(cwd) = std::env::current_dir()
        && let Some(rest) = std::path::Path::new(&*path).strip_prefix(&cwd).ok().and_then(|r| r.to_str())
        && !rest.is_empty()
    {
        return std::borrow::Cow::Owned(rest.to_string());
    }
    path
}

fn location_json(l: &Location, level: DetailLevel) -> String {
    let mut s = String::from("{");
    let full = level == DetailLevel::Full;
    let file = if full { stable_text(&l.file) } else { relative_to_cwd(stable_text(&l.file)) };
    let object = stable_text(&l.object);
    // Below full the object file is named only when it adds something.
    if full || object != file {
        s.push_str("\"object\":");
        json_string(&mut s, &object);
        s.push(',');
    }
    s.push_str("\"file\":");
    json_string(&mut s, &file);
    if l.line > 0 {
        s.push_str(&format!(",\"line\":{}", l.line));
    }
    if let Some(f) = &l.function {
        s.push_str(",\"function\":");
        json_string(&mut s, f);
    }
    if full {
        s.push_str(",\"role\":");
        json_string(&mut s, l.role);
    }
    s.push('}');
    s
}

fn suggestion_json(sg: &Suggestion, level: DetailLevel) -> String {
    let mut s = String::from("{\"message\":");
    let message = if level == DetailLevel::Full {
        &sg.message
    } else {
        sg.short.as_ref().unwrap_or(&sg.message)
    };
    json_string(&mut s, message);
    match &sg.option {
        Some(o) => {
            s.push_str(",\"option\":");
            json_string(&mut s, o);
        }
        None if level == DetailLevel::Full => s.push_str(",\"option\":null"),
        None => {}
    }
    s.push('}');
    s
}

/// One JSON line for a diagnostic. The first line of the message is
/// `message`; the rest go to `notes` with the leading `>>> ` removed. How
/// much of it there is depends on the detail level.
pub fn json_record(level: &str, detail: Option<&Detail>, msg: &str, rendered: &str) -> String {
    let lvl = detail_level();
    let mut lines = msg.lines();
    let first = lines.next().unwrap_or("");
    // Hint lines are in `suggestions`.
    let mut notes: Vec<String> = lines
        .filter(|l| !l.starts_with(">>> hint:"))
        .map(|l| l.strip_prefix(">>>").unwrap_or(l).trim().to_string())
        .collect();
    notes = collapse_references(notes);
    if hide_times() {
        for n in &mut notes {
            if let std::borrow::Cow::Owned(t) = stable_text(n) {
                *n = t;
            }
        }
    }
    let code = detail.and_then(|d| d.code).unwrap_or_else(|| classify(level, msg));
    let empty = Detail::default();
    let d = detail.unwrap_or(&empty);
    let mut o = JsonObj::new("diagnostic");
    o.str("code", code).str("severity", level).str("message", &stable_text(first));
    let json_list = |items: Vec<String>| format!("[{}]", items.join(","));
    match lvl {
        DetailLevel::Full => {
            o.strs("files", &d.files).strs("symbols", &d.symbols).strs("notes", &notes);
            o.raw(
                "suggestions",
                &json_list(d.suggestions.iter().map(|s| suggestion_json(s, lvl)).collect()),
            );
            o.raw(
                "locations",
                &json_list(d.locations.iter().map(|l| location_json(l, lvl)).collect()),
            );
            o.str("rendered", rendered);
        }
        DetailLevel::Normal => {
            if !d.locations.is_empty() {
                o.raw(
                    "locations",
                    &json_list(d.locations.iter().map(|l| location_json(l, lvl)).collect()),
                );
            }
            if d.references_omitted > 0 {
                o.num("references_omitted", d.references_omitted as u64);
            }
            if !notes.is_empty() && !d.locations_replace_notes {
                o.strs("notes", &notes);
            }
            if !d.suggestions.is_empty() {
                o.raw(
                    "suggestions",
                    &json_list(d.suggestions.iter().map(|s| suggestion_json(s, lvl)).collect()),
                );
            }
        }
        DetailLevel::Brief => {
            if let Some(l) = d.locations.first() {
                o.raw("location", &location_json(l, lvl));
                let more = d.locations.len() - 1 + d.references_omitted;
                if more > 0 {
                    o.num("more_locations", more as u64);
                }
            }
            if let Some(sg) = d.suggestions.first() {
                let fix = sg.option.as_ref().or(sg.short.as_ref()).unwrap_or(&sg.message);
                o.str("fix", fix);
                if d.suggestions.len() > 1 {
                    o.num("more_fixes", d.suggestions.len() as u64 - 1);
                }
            }
            if !notes.is_empty() && !d.locations_replace_notes {
                o.num("note_count", notes.len() as u64);
            }
        }
    }
    o.finish()
}

fn report(level: Level, detail: Option<&Detail>, msg: fmt::Arguments) {
    ensure_agent_channels();
    match level {
        Level::Warning => WARNINGS.fetch_add(1, Ordering::Relaxed),
        _ => ERRORS.fetch_add(1, Ordering::Relaxed),
    };
    let hold = level != Level::Fatal && STABLE.load(Ordering::Relaxed);
    if JSON.load(Ordering::Relaxed) {
        let msg = msg.to_string();
        let rendered = format!("{}{msg}\n", level.prefixes().0);
        let mut text = json_record(level.name(), detail, &msg, &rendered);
        text.push('\n');
        let class = if level == Level::Warning { Class::Warning } else { Class::Error };
        if hold {
            HELD.lock().unwrap_or_else(|e| e.into_inner()).push((msg, text, true, class));
            return;
        }
        let _guard = OUTPUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        write_err_record(&text, class, "diagnostic");
        tee_record(&text);
    } else if hold {
        let msg = msg.to_string();
        let (mono, color) = level.prefixes();
        let prefix = if COLOR.load(Ordering::Relaxed) { color } else { mono };
        let text = format!("{prefix}{msg}\n");
        HELD.lock().unwrap_or_else(|e| e.into_inner()).push((msg, text, false, Class::Error));
    } else {
        let (mono, color) = level.prefixes();
        emit(mono, color, msg);
    }
}

/// Reports an unrecoverable error and exits.
pub fn fatal(msg: fmt::Arguments) -> ! {
    flush_held();
    report(Level::Fatal, None, msg);
    exit_after_cleanup(1);
}

/// Reports an error. With `--noinhibit-exec` it is downgraded to a warning.
pub fn error(msg: fmt::Arguments) {
    error_with(None, msg);
}

/// Like [`error`], with facts for the JSON output.
pub fn error_with(detail: Option<&Detail>, msg: fmt::Arguments) {
    if NOINHIBIT_EXEC.load(Ordering::Relaxed) {
        report(Level::Warning, detail, msg);
    } else {
        report(Level::Error, detail, msg);
        HAS_ERROR.store(true, Ordering::Relaxed);
    }
}

/// Reports a warning. With `--fatal-warnings` it is promoted to an error.
pub fn warn(msg: fmt::Arguments) {
    warn_with(None, msg);
}

/// Like [`warn`], with facts for the JSON output.
pub fn warn_with(detail: Option<&Detail>, msg: fmt::Arguments) {
    if SUPPRESS_WARNINGS.load(Ordering::Relaxed) {
        return;
    }
    if FATAL_WARNINGS.load(Ordering::Relaxed) {
        report(Level::Error, detail, msg);
        HAS_ERROR.store(true, Ordering::Relaxed);
    } else {
        report(Level::Warning, detail, msg);
    }
}

/// Prints a message with no prefix to stderr: some reports of what the
/// link did (the Mach-O linker's -why_live, -why_load and text
/// relocations) come in lines of their own.
pub fn notice(msg: fmt::Arguments) {
    emit("", "", msg);
}

/// Prints an informational message to stdout.
pub fn out(msg: fmt::Arguments) {
    let text = format!("{msg}\n");
    let _guard = OUTPUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = io::stdout().write_all(text.as_bytes());
}

/// Returns the text that C's strerror gives for an I/O error. Unlike
/// `io::Error`'s `Display`, it doesn't append " (os error N)". An error that
/// doesn't come from the OS is formatted as usual.
pub fn strerror(err: &io::Error) -> String {
    #[cfg(not(windows))]
    if let Some(errno) = err.raw_os_error() {
        let mut buf = [0u8; 256];
        // SAFETY: strerror_r writes at most `buf.len()` bytes to `buf`.
        let ret = unsafe { libc::strerror_r(errno, buf.as_mut_ptr().cast(), buf.len()) };
        if ret == 0
            && let Ok(msg) = std::ffi::CStr::from_bytes_until_nul(&buf)
        {
            return msg.to_string_lossy().into_owned();
        }
    }
    err.to_string()
}

/// Exits with a failure status if any error has been reported.
pub fn checkpoint() {
    if HAS_ERROR.load(Ordering::Relaxed) {
        exit_after_cleanup(1);
    }
}

/// Removes a partially-written output file, then terminates the process
/// without running destructors. Input files are mapped for the process's
/// lifetime, so there is nothing else to release.
pub fn exit_after_cleanup(status: i32) -> ! {
    flush_held();
    finish_records();
    if status != 0 {
        write_failure_summary();
        flush_held();
    }
    crate::output_file::cleanup();
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
    // SAFETY: `_exit` only terminates the process.
    #[cfg(not(windows))]
    unsafe {
        libc::_exit(status)
    }
    #[cfg(windows)]
    std::process::exit(status)
}

/// Makes a panic remove a partially-written output file, as a fatal error
/// does, before the default hook reports the panic. Nothing recovers from
/// a panic, so it always terminates the process.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        crate::output_file::cleanup();
        default_hook(info);
    }));
}

#[macro_export]
macro_rules! fatal {
    ($($arg:tt)*) => {
        $crate::error::fatal(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::error::error(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::error::warn(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! notice {
    ($($arg:tt)*) => {
        $crate::error::notice(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! out {
    ($($arg:tt)*) => {
        $crate::error::out(format_args!($($arg)*))
    };
}
