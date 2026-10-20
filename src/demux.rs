//! Demux: one ordered stream of cargo's stdout and stderr lines in, typed
//! events out, and every byte kept.
//!
//! Each stdout line is one of three things (W371 §4.5 module 3):
//!
//! - a **cargo message** — a JSON object with a `reason` key, parsed with
//!   `cargo_metadata::Message`, not by hand;
//! - a **libtest event** — a JSON object with a `type` key, read through the
//!   versioned adapter in [`crate::libtest`];
//! - **raw** — anything else, kept in order.
//!
//! stderr carries cargo's `Running <source> (<binary>)` and `Doc-tests <crate>`
//! lines, which say which test binary the stdout that follows belongs to.
//! Attribution therefore depends on the two streams arriving in write order.
//! Two pipes read separately do NOT guarantee that — it raced under load on
//! 2026-10-17 — so [`crate::exec::run`] gives cargo one pipe for both and
//! feeds lines as [`Stream::Combined`]; a caller holding separate streams can
//! still push them as `Stdout`/`Stderr` and accepts the race. A libtest event
//! with no open binary is attached to an `Unknown` one rather than guessed
//! onto a neighbour.
//!
//! A binary that prints no libtest JSON within its first
//! [`FORMAT_PROBE_LIMIT`] stdout lines — `harness = false`, a custom harness,
//! the doctest runner — is marked `structured: false` and its output is kept
//! raw. The same pattern as task-runs' `CargoBeholder` (`FORMAT_PROBE_LIMIT`
//! in oss/qed/crates/task-runs/src/beholders.rs).

use cargo_metadata::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::libtest::{self, TestEvent};

/// stdout lines a test binary may print before its first libtest event and
/// still be read as structured.
pub const FORMAT_PROBE_LIMIT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
    /// Both, through one pipe, in exact write order ([`crate::exec::run`]).
    Combined,
}

/// One captured line, verbatim, without its newline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Captured {
    pub stream: Stream,
    pub text: String,
}

/// What one stdout line is.
#[derive(Debug, Clone)]
pub enum StdoutLine {
    Cargo(Box<Message>),
    Libtest(TestEvent),
    Raw,
}

/// Classify one stdout line.
pub fn classify(line: &str) -> StdoutLine {
    let t = line.trim();
    if !t.starts_with('{') {
        return StdoutLine::Raw;
    }
    let Ok(v) = serde_json::from_str::<Value>(t) else {
        return StdoutLine::Raw;
    };
    if v.get("reason").is_some() {
        // A `reason` this cargo_metadata does not know stays raw: kept, not
        // misread.
        return match serde_json::from_value::<Message>(v) {
            Ok(m) => StdoutLine::Cargo(Box::new(m)),
            Err(_) => StdoutLine::Raw,
        };
    }
    match libtest::adapter().read(&v) {
        Some(e) => StdoutLine::Libtest(e),
        None => StdoutLine::Raw,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BinaryKind {
    /// `Running unittests src/lib.rs (…)` — a lib's or bin's own tests.
    Unittests,
    /// `Running tests/it.rs (…)` — an integration test target.
    Test,
    /// `Running benches/b.rs (…)`.
    Bench,
    /// `Running examples/e.rs (…)`.
    Example,
    /// `Doc-tests <crate>`.
    Doctest,
    /// Output seen with no `Running` line before it.
    Unknown,
}

/// One test binary's run, as cargo announced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryRun {
    pub kind: BinaryKind,
    /// `src/lib.rs`, `tests/it.rs`; the crate name for doctests.
    pub source: Option<String>,
    /// The binary's path as cargo printed it (relative to cargo's cwd).
    pub executable: Option<String>,
    /// Did it speak libtest JSON within its first lines?
    pub structured: bool,
    pub events: Vec<TestEvent>,
    /// Indices into [`Demux::lines`] of this binary's stdout that was not a
    /// libtest event, in order.
    pub raw: Vec<usize>,
    #[serde(skip)]
    unstructured_lines: usize,
}

impl BinaryRun {
    fn new(kind: BinaryKind, source: Option<String>, executable: Option<String>) -> Self {
        BinaryRun { kind, source, executable, structured: false, events: Vec::new(), raw: Vec::new(), unstructured_lines: 0 }
    }

    /// Past the probe without a libtest event: everything from here is raw.
    fn given_up(&self) -> bool {
        !self.structured && self.unstructured_lines >= FORMAT_PROBE_LIMIT
    }

    /// The file name of [`Self::executable`], for matching against
    /// `compiler-artifact.executable`.
    pub fn executable_name(&self) -> Option<&str> {
        let p = self.executable.as_deref()?;
        Some(p.rsplit(['/', '\\']).next().unwrap_or(p))
    }
}

/// Parse cargo's stderr announcement of a test binary.
fn announcement(line: &str) -> Option<BinaryRun> {
    let t = line.trim();
    if let Some(krate) = t.strip_prefix("Doc-tests ") {
        return Some(BinaryRun::new(BinaryKind::Doctest, Some(krate.trim().to_string()), None));
    }
    let rest = t.strip_prefix("Running ")?;
    // `unittests src/lib.rs (target/debug/deps/x-hash)` or `tests/it.rs (…)`.
    let (head, exe) = match rest.rfind(" (") {
        Some(i) if rest.ends_with(')') => (&rest[..i], Some(rest[i + 2..rest.len() - 1].to_string())),
        _ => (rest, None),
    };
    let (kind, source) = match head.strip_prefix("unittests ") {
        Some(src) => (BinaryKind::Unittests, src),
        None if head.starts_with("tests/") || head.starts_with("tests\\") => (BinaryKind::Test, head),
        None if head.starts_with("benches/") || head.starts_with("benches\\") => (BinaryKind::Bench, head),
        None if head.starts_with("examples/") || head.starts_with("examples\\") => (BinaryKind::Example, head),
        // A test target with a custom `path` outside tests/.
        None => (BinaryKind::Test, head),
    };
    Some(BinaryRun::new(kind, Some(source.to_string()), exe))
}

/// The text a terminal would show for `line`: what follows the last carriage
/// return (cargo's progress bar is drawn, then erased with `\r` + `ESC[K`, on
/// the very line the next JSON message is written to — measured under a PTY,
/// 2026-10-17), with ANSI CSI sequences (colour, erase) removed. A JSON
/// message never carries a raw ESC (JSON escapes control characters), so this
/// cannot corrupt one.
pub fn visible(line: &str) -> std::borrow::Cow<'_, str> {
    let tail = line.rsplit('\r').next().unwrap_or(line);
    if !tail.contains('\u{1b}') && tail.len() == line.len() {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(tail.len());
    let mut chars = tail.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            // Parameters and intermediates, then one final byte in @..~.
            for f in chars.by_ref() {
                if ('@'..='~').contains(&f) {
                    break;
                }
            }
        } else if c != '\u{f}' {
            out.push(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// cargo's own status lines that can follow a test binary on a combined
/// stream (`error: test failed, to rerun pass `--lib``, the `N targets
/// failed:` list), so they are not mistaken for its output.
fn is_cargo_status(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("error: test failed, to rerun pass")
        || t.starts_with("error: doctest failed, to rerun pass")
        || (t.starts_with("error: ") && t.ends_with("failed:") && t.contains(" target"))
        || (line.starts_with("    `--") && t.ends_with('`'))
}

/// The demultiplexer. Feed it lines in arrival order with [`Demux::push`].
#[derive(Debug)]
pub struct Demux {
    /// Every line, both streams, verbatim and in order. Nothing is dropped —
    /// unless the demux was made with [`Demux::streaming`], for a caller that
    /// already stores the bytes; then this stays empty, and so do the index
    /// lists below.
    pub lines: Vec<Captured>,
    /// cargo's JSON messages, in order.
    pub messages: Vec<Message>,
    /// Test binaries, in the order cargo ran them.
    pub binaries: Vec<BinaryRun>,
    /// Indices into [`Self::lines`] of stdout (or combined) lines that were
    /// neither a cargo message nor any binary's output: cargo's progress and
    /// closing lines on a combined stream, anything before the first binary.
    pub raw_stdout: Vec<usize>,
    retain_lines: bool,
    pushed: usize,
}

impl Default for Demux {
    fn default() -> Demux {
        Demux {
            lines: Vec::new(),
            messages: Vec::new(),
            binaries: Vec::new(),
            raw_stdout: Vec::new(),
            retain_lines: true,
            pushed: 0,
        }
    }
}

impl Demux {
    pub fn new() -> Demux {
        Demux::default()
    }

    /// A demux that keeps the parsed structure but not the lines themselves:
    /// for a run store (task-runs) that already keeps every byte, where a
    /// second copy would only cost memory on a long run.
    pub fn streaming() -> Demux {
        Demux { retain_lines: false, ..Demux::default() }
    }

    /// Lines pushed so far, retained or not.
    pub fn pushed(&self) -> usize {
        self.pushed
    }

    pub fn push(&mut self, stream: Stream, raw: &str) {
        let idx = self.pushed;
        self.pushed += 1;
        let keep = self.retain_lines;
        if keep {
            self.lines.push(Captured { stream, text: raw.to_string() });
        }
        // Read what a terminal would show; keep the bytes as they came.
        let visible = visible(raw);
        let line = visible.as_ref();
        let announced = match stream {
            Stream::Stdout => None,
            Stream::Stderr | Stream::Combined => announcement(line),
        };
        if let Some(b) = announced {
            self.binaries.push(b);
            return;
        }
        match stream {
            Stream::Stderr => {}
            Stream::Stdout | Stream::Combined => match classify(line) {
                StdoutLine::Cargo(m) => self.messages.push(*m),
                StdoutLine::Libtest(e) => {
                    // Without cargo's `Running` lines (a caller that sees
                    // stdout only), a second `suite started` is the only sign
                    // that a new binary began: group by it rather than pour
                    // every binary's results into one.
                    let new_suite = matches!(e, TestEvent::SuiteStarted { .. })
                        && self.binaries.last().is_some_and(|b| {
                            b.events.iter().any(|e| matches!(e, TestEvent::SuiteStarted { .. }))
                        });
                    if self.binaries.is_empty() || new_suite {
                        self.binaries.push(BinaryRun::new(BinaryKind::Unknown, None, None));
                    }
                    let b = self.binaries.last_mut().expect("just ensured");
                    if b.given_up() {
                        if keep {
                            b.raw.push(idx);
                        }
                    } else {
                        b.structured = true;
                        b.events.push(e);
                    }
                }
                // cargo's own closing lines are not the last binary's output.
                StdoutLine::Raw if stream == Stream::Combined && is_cargo_status(line) => {
                    if keep {
                        self.raw_stdout.push(idx)
                    }
                }
                StdoutLine::Raw => match self.binaries.last_mut() {
                    Some(b) => {
                        if !b.structured {
                            b.unstructured_lines += 1;
                        }
                        if keep {
                            b.raw.push(idx);
                        }
                    }
                    None => {
                        if keep {
                            self.raw_stdout.push(idx)
                        }
                    }
                },
            },
        }
    }

    /// Feed a whole capture, e.g. from a fixture.
    pub fn extend<'a>(&mut self, lines: impl IntoIterator<Item = (Stream, &'a str)>) {
        for (s, l) in lines {
            self.push(s, l);
        }
    }

    /// The compiler-artifact that produced `binary`, matched by file name.
    pub fn artifact_of(&self, binary: &BinaryRun) -> Option<&cargo_metadata::Artifact> {
        let name = binary.executable_name()?;
        self.messages.iter().find_map(|m| match m {
            Message::CompilerArtifact(a) => a
                .executable
                .as_ref()
                .filter(|e| e.file_name() == Some(name))
                .map(|_| a),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcements_parse() {
        let b = announcement("     Running unittests src/lib.rs (target/debug/deps/probe-48f1c1bd67fc1817)").unwrap();
        assert_eq!(b.kind, BinaryKind::Unittests);
        assert_eq!(b.source.as_deref(), Some("src/lib.rs"));
        assert_eq!(b.executable_name(), Some("probe-48f1c1bd67fc1817"));
        let b = announcement("     Running tests/it.rs (target/debug/deps/it-7f914c64967faa1a)").unwrap();
        assert_eq!(b.kind, BinaryKind::Test);
        let b = announcement("   Doc-tests probe").unwrap();
        assert_eq!((b.kind, b.source.as_deref()), (BinaryKind::Doctest, Some("probe")));
        assert!(announcement("   Compiling probe v0.1.0").is_none());
    }

    #[test]
    fn a_binary_that_never_speaks_json_is_unstructured_and_kept() {
        let mut d = Demux::new();
        d.push(Stream::Stderr, "     Running tests/noharness.rs (target/debug/deps/noharness-1)");
        for i in 0..FORMAT_PROBE_LIMIT {
            d.push(Stream::Stdout, &format!("line {i}"));
        }
        // Too late to count: past the probe.
        d.push(Stream::Stdout, r#"{ "type": "suite", "event": "started", "test_count": 1 }"#);
        let b = &d.binaries[0];
        assert!(!b.structured);
        assert!(b.events.is_empty());
        assert_eq!(b.raw.len(), FORMAT_PROBE_LIMIT + 1);
        assert_eq!(d.lines.len(), FORMAT_PROBE_LIMIT + 2, "every byte is kept");
    }

    #[test]
    fn without_running_lines_suites_still_split_into_binaries() {
        let mut d = Demux::streaming();
        for _ in 0..2 {
            d.push(Stream::Stdout, r#"{ "type": "suite", "event": "started", "test_count": 1 }"#);
            d.push(Stream::Stdout, r#"{ "type": "test", "name": "a", "event": "ok" }"#);
            d.push(Stream::Stdout, r#"{ "type": "suite", "event": "ok", "passed": 1, "failed": 0, "ignored": 0, "measured": 0, "filtered_out": 0 }"#);
        }
        assert_eq!(d.binaries.len(), 2);
        assert!(d.binaries.iter().all(|b| b.structured && b.events.len() == 3));
        assert!(d.lines.is_empty() && d.pushed() == 6);
    }

    #[test]
    fn a_pty_line_reads_as_the_terminal_shows_it() {
        // Verbatim shape from a PTY run: progress bar, CR, erase, then JSON.
        let line = "\u{1b}[1m\u{1b}[96m    Building\u{1b}[0m [ ] 0/1: x(test)\r\u{1b}[K{\"reason\":\"build-finished\",\"success\":true}";
        assert_eq!(visible(line), r#"{"reason":"build-finished","success":true}"#);
        let mut d = Demux::new();
        d.push(Stream::Combined, line);
        d.push(Stream::Combined, "\u{1b}[1m\u{1b}[92m     Running\u{1b}[0m unittests src/lib.rs (target/debug/deps/x-1)");
        assert_eq!(d.messages.len(), 1);
        assert_eq!(d.binaries.len(), 1);
        assert_eq!(d.lines[0].text, line, "the bytes are kept as they came");
        assert_eq!(visible("plain"), "plain");
    }

    #[test]
    fn unknown_reasons_and_junk_json_stay_raw() {
        assert!(matches!(classify(r#"{"reason":"from-the-future","x":1}"#), StdoutLine::Raw));
        assert!(matches!(classify("{not json"), StdoutLine::Raw));
        assert!(matches!(classify(r#"{"reason":"build-finished","success":true}"#), StdoutLine::Cargo(_)));
    }
}
