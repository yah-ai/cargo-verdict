//! libtest's JSON output, behind a versioned adapter.
//!
//! libtest `--format json` is unstable: RFC 3558 calls its own format
//! underspecified and the stabilization goal is still open (W371 §4.3). So
//! nothing outside this module reads libtest JSON directly. Each adapter names
//! the format revision it reads, maps it into [`TestEvent`] (this crate's own,
//! stable vocabulary), and turns anything it does not recognize into
//! [`TestEvent::Unrecognized`] rather than an error. When the upstream format
//! moves, a new adapter is added beside the old one; the result schema does
//! not move.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One libtest event, in this crate's vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TestEvent {
    SuiteStarted {
        test_count: u64,
    },
    TestStarted {
        name: String,
    },
    TestOk {
        name: String,
        exec_time: Option<f64>,
    },
    TestFailed {
        name: String,
        exec_time: Option<f64>,
        /// Captured output; libtest puts the panic message here too.
        stdout: Option<String>,
        /// Set for `#[should_panic]` mismatches and similar.
        message: Option<String>,
    },
    TestIgnored {
        name: String,
        /// The `#[ignore = "reason"]` reason.
        message: Option<String>,
    },
    /// `--ensure-time` exceeded.
    TestTimeout {
        name: String,
    },
    SuiteFinished {
        ok: bool,
        passed: u64,
        failed: u64,
        ignored: u64,
        measured: u64,
        filtered_out: u64,
        exec_time: Option<f64>,
    },
    Bench {
        name: String,
        median: Option<f64>,
        deviation: Option<f64>,
    },
    /// A `type`-keyed object this adapter does not know, kept whole.
    Unrecognized {
        raw: Value,
    },
}

/// Reads one revision of libtest's JSON.
pub trait LibtestAdapter: Send + Sync {
    /// Which upstream format this reads, e.g. `libtest-json/1`.
    fn id(&self) -> &'static str;
    /// Map one JSON object with a `type` key. `None` means "not libtest at
    /// all" (no `type` key); an unknown `type`/`event` is `Unrecognized`.
    fn read(&self, obj: &Value) -> Option<TestEvent>;
}

/// libtest's JSON as emitted by Rust 1.x up to at least 1.97 (measured
/// 2026-10-17 on 1.97.0): `{"type":"suite"|"test"|"bench", "event": …}`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LibtestJsonV1;

impl LibtestAdapter for LibtestJsonV1 {
    fn id(&self) -> &'static str {
        "libtest-json/1"
    }

    fn read(&self, obj: &Value) -> Option<TestEvent> {
        let ty = obj.get("type")?.as_str()?;
        let s = |k: &str| obj.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| obj.get(k).and_then(Value::as_u64).unwrap_or(0);
        let f = |k: &str| obj.get(k).and_then(Value::as_f64);
        let event = obj.get("event").and_then(Value::as_str);
        let unrecognized = || TestEvent::Unrecognized { raw: obj.clone() };
        Some(match (ty, event) {
            ("suite", Some("started")) => TestEvent::SuiteStarted { test_count: n("test_count") },
            ("suite", Some(e @ ("ok" | "failed"))) => TestEvent::SuiteFinished {
                ok: e == "ok",
                passed: n("passed"),
                failed: n("failed"),
                ignored: n("ignored"),
                measured: n("measured"),
                filtered_out: n("filtered_out"),
                exec_time: f("exec_time"),
            },
            ("test", Some(e)) => {
                let Some(name) = s("name") else { return Some(unrecognized()) };
                match e {
                    "started" => TestEvent::TestStarted { name },
                    "ok" => TestEvent::TestOk { name, exec_time: f("exec_time") },
                    "failed" => TestEvent::TestFailed {
                        name,
                        exec_time: f("exec_time"),
                        stdout: s("stdout"),
                        message: s("message"),
                    },
                    "ignored" => TestEvent::TestIgnored { name, message: s("message") },
                    "timeout" => TestEvent::TestTimeout { name },
                    _ => unrecognized(),
                }
            }
            ("bench", _) => match s("name") {
                Some(name) => TestEvent::Bench { name, median: f("median"), deviation: f("deviation") },
                None => unrecognized(),
            },
            _ => unrecognized(),
        })
    }
}

/// The adapter for the toolchain at hand. One today.
pub fn adapter() -> &'static dyn LibtestAdapter {
    &LibtestJsonV1
}

/// The `file:line:col` of the first panic in a failed test's captured output:
/// libtest's own text, `thread '…' panicked at src/lib.rs:5:77:`. This is the
/// one place structure has to be read out of text, and it is a location, not
/// a verdict: the verdict came from the JSON event.
pub fn panic_location(output: &str) -> Option<String> {
    let rest = &output[output.find("panicked at ")? + "panicked at ".len()..];
    let line = rest.lines().next()?;
    let loc = line.strip_suffix(':').unwrap_or(line).trim();
    // `path:line:col` — two numeric fields from the right.
    let mut parts = loc.rsplitn(3, ':');
    let col = parts.next()?;
    let ln = parts.next()?;
    let file = parts.next()?;
    (!file.is_empty() && col.parse::<u32>().is_ok() && ln.parse::<u32>().is_ok())
        .then(|| loc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(line: &str) -> TestEvent {
        adapter().read(&serde_json::from_str(line).unwrap()).unwrap()
    }

    #[test]
    fn the_measured_1_97_lines_map() {
        // Verbatim from the 2026-10-17 probe (rustc 1.97.0).
        assert_eq!(
            read(r#"{ "type": "suite", "event": "started", "test_count": 3 }"#),
            TestEvent::SuiteStarted { test_count: 3 }
        );
        assert_eq!(
            read(r#"{ "type": "test", "name": "t::ign", "event": "ignored", "message": "slow" }"#),
            TestEvent::TestIgnored { name: "t::ign".into(), message: Some("slow".into()) }
        );
        assert!(matches!(
            read(r#"{ "type": "test", "name": "t::bad", "event": "failed", "exec_time": 0.1, "stdout": "cap\n" }"#),
            TestEvent::TestFailed { stdout: Some(_), exec_time: Some(_), .. }
        ));
        assert_eq!(
            read(r#"{ "type": "suite", "event": "failed", "passed": 1, "failed": 1, "ignored": 1, "measured": 0, "filtered_out": 0, "exec_time": 0.1 }"#),
            TestEvent::SuiteFinished { ok: false, passed: 1, failed: 1, ignored: 1, measured: 0, filtered_out: 0, exec_time: Some(0.1) }
        );
    }

    #[test]
    fn an_unknown_shape_is_kept_not_dropped() {
        assert!(matches!(read(r#"{"type":"test","event":"allowed_fail","name":"x"}"#), TestEvent::Unrecognized { .. }));
        assert!(matches!(read(r#"{"type":"report","x":1}"#), TestEvent::Unrecognized { .. }));
        assert!(adapter().read(&serde_json::json!({"reason": "x"})).is_none());
    }

    #[test]
    fn panic_locations_are_read_from_libtest_s_own_line() {
        let out = "cap\n\nthread 't::bad' (31622) panicked at src/lib.rs:5:77:\nboom\n";
        assert_eq!(panic_location(out).as_deref(), Some("src/lib.rs:5:77"));
        assert_eq!(panic_location("no panic here"), None);
        assert_eq!(panic_location("panicked at nowhere:\n"), None);
    }
}
