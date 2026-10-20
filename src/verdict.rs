//! The result schema, `cargo-verdict/1` (W371 §4.5 module 4).
//!
//! A published contract: the yah `cargo.*` verbs return it verbatim as MCP
//! `structuredContent`, and QED and noisetable read it. [`schema()`] is its
//! JSON Schema (the MCP `outputSchema`). Fields are only ever added under `/1`;
//! a rename or a removal is `/2`.
//!
//! **One structure, several views.** [`Verdict::collect`] builds the whole
//! thing once from a plan, a demux and an exit; [`Verdict::project`] narrows it
//! to a [`Want`]. The views are pruned copies of the same type (RFC 3558's
//! recommended shape), not separate code paths, so a field means the same
//! thing in every view.
//!
//! The camp adds `run_id`, `lane`, `served_by` and the rung-0 `skew` verdict
//! *outside* this schema (R989 P5).

use cargo_metadata::diagnostic::{Diagnostic as RustcDiagnostic, DiagnosticLevel};
use cargo_metadata::Message;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::demux::{BinaryKind, BinaryRun, Demux};
use crate::exec::Exit;
use crate::libtest::{panic_location, TestEvent};
use crate::plan::Plan;
use crate::request::Request;

/// The schema identifier every result carries.
pub const SCHEMA: &str = "cargo-verdict/1";

/// Bytes of captured output a failure keeps under [`Want::Failures`]. The
/// *tail* is kept: libtest prints the panic last.
pub const FAILURE_OUTPUT_LIMIT: usize = 4096;

/// Which view of the result to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Want {
    /// Counts only: outcome, build counts, test totals and per-binary counts.
    Summary,
    /// Summary, plus error diagnostics and each failing test's name, location
    /// and output (tail, truncated to [`FAILURE_OUTPUT_LIMIT`] with the bytes
    /// dropped shown).
    #[default]
    Failures,
    /// Summary, plus every error and warning diagnostic, rendered.
    Diagnostics,
    /// Everything, nothing truncated, including unstructured binaries' raw output.
    Full,
}

impl std::str::FromStr for Want {
    type Err = String;
    fn from_str(s: &str) -> Result<Want, String> {
        match s {
            "summary" => Ok(Want::Summary),
            "failures" => Ok(Want::Failures),
            "diagnostics" => Ok(Want::Diagnostics),
            "full" => Ok(Want::Full),
            _ => Err(format!("--want takes summary | failures | diagnostics | full, not `{s}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    TestFailed,
    BuildFailed,
    /// cargo was ended by a signal; `exit_code` is null.
    Interrupted,
}

/// One cargo-verdict result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Verdict {
    /// Always [`SCHEMA`].
    pub schema: String,
    /// Which view this is.
    pub want: Want,
    /// The request as normalized (before planning).
    pub request: Request,
    /// What ran, after `cargo`: the plan's argv, flags it added included.
    pub argv: Vec<String>,
    /// What the plan added to the request, one entry per flag.
    pub added: Vec<String>,
    /// cargo's exit code; null only when cargo was ended by a signal.
    pub exit_code: Option<i32>,
    pub outcome: Outcome,
    pub build: Build,
    pub tests: Tests,
    /// Where structure is missing, and why (from the plan).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Build {
    /// cargo's `build-finished.success`; null when cargo printed no JSON.
    pub success: Option<bool>,
    pub counts: Counts,
    pub diagnostics: Vec<Diagnostic>,
    pub units: Units,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Counts {
    pub error: u64,
    pub warning: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Units {
    /// `compiler-artifact` lines with `fresh: false`.
    pub compiled: u64,
    /// `compiler-artifact` lines with `fresh: true`.
    pub fresh: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Diagnostic {
    /// `error` or `warning`.
    pub level: String,
    /// `E0308`, `unused_variables`, `clippy::needless_return`.
    pub code: Option<String>,
    pub message: String,
    /// The package the diagnostic came from.
    pub package: Option<String>,
    pub file: Option<String>,
    pub line: Option<u64>,
    pub col: Option<u64>,
    /// The first machine-applicable-or-not replacement rustc suggested.
    pub suggestion: Option<String>,
    /// rustc's human rendering.
    pub rendered: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Tests {
    /// Sums over binaries whose counts are known.
    pub totals: Totals,
    pub binaries: Vec<Binary>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Totals {
    pub passed: u64,
    pub failed: u64,
    pub ignored: u64,
    pub filtered_out: u64,
    pub measured: u64,
    /// Binaries whose counts could not be read at all.
    pub unknown_binaries: u64,
}

/// One test binary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Binary {
    pub package: Option<String>,
    /// The cargo target's name (`mixed`, `it`); null for doctests.
    pub target: Option<String>,
    pub kind: BinaryKind,
    /// Spoke libtest JSON. When false the counts below come from libtest's
    /// human `test result:` line if it printed one, else are null — and the
    /// raw output is the source of truth.
    pub structured: bool,
    pub passed: Option<u64>,
    pub failed: Option<u64>,
    pub ignored: Option<u64>,
    pub filtered_out: Option<u64>,
    pub duration_ms: Option<u64>,
    pub failures: Vec<Failure>,
    /// The binary's stdout that was not a libtest event (all of it, when
    /// unstructured). Present under `full`, and under `failures` for an
    /// unstructured binary that failed; truncated like a failure's output.
    pub output: Option<String>,
    pub output_truncated_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Failure {
    pub name: String,
    /// `file:line:col` of the panic.
    pub location: Option<String>,
    pub output: String,
    pub output_truncated_bytes: u64,
}

/// The JSON Schema of [`Verdict`].
pub fn schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Verdict)).expect("a schema serializes")
}

impl Verdict {
    /// Build the full result. `want` is [`Want::Full`]; narrow it with
    /// [`Verdict::project`].
    pub fn collect(original: &Request, plan: &Plan, demux: &Demux, exit: Exit) -> Verdict {
        let build = build(demux);
        let tests = tests(demux);
        let exit_code = match exit {
            Exit::Code(c) => Some(c),
            Exit::Signalled(_) => None,
        };
        let outcome = match exit_code {
            None => Outcome::Interrupted,
            Some(0) => Outcome::Pass,
            Some(_) if build.success == Some(false) || build.counts.error > 0 => Outcome::BuildFailed,
            Some(_) if !tests.binaries.is_empty() => Outcome::TestFailed,
            Some(_) => Outcome::BuildFailed,
        };
        Verdict {
            schema: SCHEMA.to_string(),
            want: Want::Full,
            request: original.clone(),
            argv: plan.argv.clone(),
            added: plan.added.clone(),
            exit_code,
            outcome,
            build,
            tests,
            notes: plan.unstructured.clone(),
        }
    }

    /// The `want` view of this result.
    pub fn project(&self, want: Want) -> Verdict {
        let mut v = self.clone();
        v.want = want;
        match want {
            Want::Full => {}
            Want::Summary => {
                v.build.diagnostics.clear();
                for b in &mut v.tests.binaries {
                    b.failures.clear();
                    b.output = None;
                    b.output_truncated_bytes = 0;
                }
            }
            Want::Diagnostics => {
                for b in &mut v.tests.binaries {
                    b.failures.clear();
                    b.output = None;
                    b.output_truncated_bytes = 0;
                }
            }
            Want::Failures => {
                v.build.diagnostics.retain(|d| d.level == "error");
                for b in &mut v.tests.binaries {
                    for f in &mut b.failures {
                        f.output_truncated_bytes += truncate_tail(&mut f.output, FAILURE_OUTPUT_LIMIT);
                    }
                    let failed = b.failed.map_or(!b.structured && v.outcome == Outcome::TestFailed, |n| n > 0);
                    if b.structured || !failed {
                        b.output = None;
                        b.output_truncated_bytes = 0;
                    } else if let Some(o) = &mut b.output {
                        b.output_truncated_bytes += truncate_tail(o, FAILURE_OUTPUT_LIMIT);
                    }
                }
            }
        }
        v
    }
}

/// Keep the last `limit` bytes of `s` (on a char boundary); return how many
/// were dropped.
fn truncate_tail(s: &mut String, limit: usize) -> u64 {
    if s.len() <= limit {
        return 0;
    }
    let mut cut = s.len() - limit;
    while !s.is_char_boundary(cut) {
        cut += 1;
    }
    s.drain(..cut);
    cut as u64
}

/// `path+file:///…/mixed#0.1.0` → `mixed`; `registry+…#serde@1.0.0` → `serde`.
fn package_name(repr: &str) -> String {
    let (url, frag) = repr.rsplit_once('#').unwrap_or((repr, ""));
    match frag.split_once('@') {
        Some((name, _)) => name.to_string(),
        None => url.trim_end_matches('/').rsplit('/').next().unwrap_or(url).to_string(),
    }
}

fn diagnostic(d: &RustcDiagnostic, package: Option<String>) -> Option<Diagnostic> {
    let level = match d.level {
        DiagnosticLevel::Error | DiagnosticLevel::Ice => "error",
        DiagnosticLevel::Warning => "warning",
        _ => return None,
    };
    let primary = d.spans.iter().find(|s| s.is_primary).or(d.spans.first());
    let suggestion = d
        .spans
        .iter()
        .chain(d.children.iter().flat_map(|c| &c.spans))
        .find_map(|s| s.suggested_replacement.clone());
    Some(Diagnostic {
        level: level.to_string(),
        code: d.code.as_ref().map(|c| c.code.clone()),
        message: d.message.clone(),
        package,
        file: primary.map(|s| s.file_name.clone()),
        line: primary.map(|s| s.line_start as u64),
        col: primary.map(|s| s.column_start as u64),
        suggestion,
        rendered: d.rendered.clone(),
    })
}

fn build(demux: &Demux) -> Build {
    let mut b = Build { success: None, counts: Counts::default(), diagnostics: Vec::new(), units: Units::default() };
    for m in &demux.messages {
        match m {
            Message::CompilerMessage(c) => {
                if let Some(d) = diagnostic(&c.message, Some(package_name(&c.package_id.repr))) {
                    // A lib and its test harness both report a lib's warning;
                    // cargo shows it once ("1 duplicate"), and so does this.
                    if !b.diagnostics.contains(&d) {
                        match d.level.as_str() {
                            "error" => b.counts.error += 1,
                            _ => b.counts.warning += 1,
                        }
                        b.diagnostics.push(d);
                    }
                }
            }
            Message::CompilerArtifact(a) => {
                if a.fresh {
                    b.units.fresh += 1;
                } else {
                    b.units.compiled += 1;
                }
            }
            Message::BuildFinished(f) => b.success = Some(f.success),
            _ => {}
        }
    }
    b
}

/// libtest's human summary line, for an unstructured binary that printed one:
/// `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; …`.
/// The declared last resort (W371 §4.5): counts only, never failures.
fn summary_line(line: &str) -> Option<[u64; 5]> {
    let rest = line.trim().strip_prefix("test result: ")?;
    let (_, counts) = rest.split_once(". ")?;
    let mut out = [0u64; 5];
    for (slot, label) in ["passed", "failed", "ignored", "measured", "filtered out"].iter().enumerate() {
        let field = counts.split(';').map(str::trim).find(|f| f.ends_with(label))?;
        out[slot] = field.split_whitespace().next()?.parse().ok()?;
    }
    Some(out)
}

fn tests(demux: &Demux) -> Tests {
    let mut t = Tests::default();
    for run in &demux.binaries {
        let b = binary(demux, run);
        match b.passed {
            Some(p) => {
                t.totals.passed += p;
                t.totals.failed += b.failed.unwrap_or(0);
                t.totals.ignored += b.ignored.unwrap_or(0);
                t.totals.filtered_out += b.filtered_out.unwrap_or(0);
            }
            None => t.totals.unknown_binaries += 1,
        }
        if let Some(TestEvent::SuiteFinished { measured, .. }) =
            run.events.iter().rev().find(|e| matches!(e, TestEvent::SuiteFinished { .. }))
        {
            t.totals.measured += measured;
        }
        t.binaries.push(b);
    }
    t
}

fn binary(demux: &Demux, run: &BinaryRun) -> Binary {
    let artifact = demux.artifact_of(run);
    let output: Vec<&str> = run.raw.iter().map(|&i| demux.lines[i].text.as_str()).collect();
    let mut b = Binary {
        package: match run.kind {
            BinaryKind::Doctest => run.source.clone(),
            _ => artifact.map(|a| package_name(&a.package_id.repr)),
        },
        target: artifact.map(|a| a.target.name.clone()),
        kind: run.kind,
        structured: run.structured,
        passed: None,
        failed: None,
        ignored: None,
        filtered_out: None,
        duration_ms: None,
        failures: Vec::new(),
        output: (!output.is_empty()).then(|| output.join("\n")),
        output_truncated_bytes: 0,
    };
    if run.structured {
        for e in &run.events {
            match e {
                TestEvent::TestFailed { name, stdout, message, .. } => {
                    let mut out = stdout.clone().unwrap_or_default();
                    if let Some(m) = message {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(m);
                    }
                    b.failures.push(Failure {
                        name: name.clone(),
                        location: panic_location(&out),
                        output: out,
                        output_truncated_bytes: 0,
                    });
                }
                TestEvent::TestTimeout { name } => b.failures.push(Failure {
                    name: name.clone(),
                    location: None,
                    output: "exceeded --ensure-time".into(),
                    output_truncated_bytes: 0,
                }),
                TestEvent::SuiteFinished { passed, failed, ignored, filtered_out, exec_time, .. } => {
                    b.passed = Some(*passed);
                    b.failed = Some(*failed);
                    b.ignored = Some(*ignored);
                    b.filtered_out = Some(*filtered_out);
                    b.duration_ms = exec_time.map(|s| (s * 1000.0).round() as u64);
                }
                _ => {}
            }
        }
    } else if let Some([p, f, i, _, fo]) = output.iter().rev().find_map(|l| summary_line(l)) {
        b.passed = Some(p);
        b.failed = Some(f);
        b.ignored = Some(i);
        b.filtered_out = Some(fo);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_come_out_of_package_ids() {
        assert_eq!(package_name("path+file:///tmp/x/mixed#0.1.0"), "mixed");
        assert_eq!(package_name("path+file:///tmp/x/dir#other@0.1.0"), "other");
        assert_eq!(package_name("registry+https://github.com/rust-lang/crates.io-index#serde@1.0.228"), "serde");
    }

    #[test]
    fn summary_lines_parse_and_only_summary_lines() {
        assert_eq!(
            summary_line("test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 3 filtered out; finished in 0.01s"),
            Some([1, 0, 2, 0, 3])
        );
        assert_eq!(summary_line("test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out"), Some([0, 1, 0, 0, 0]));
        assert_eq!(summary_line("custom harness ran"), None);
    }

    #[test]
    fn truncation_keeps_the_tail_and_counts_what_it_dropped() {
        let mut s = format!("{}panicked at x.rs:1:1:", "a".repeat(10));
        let dropped = truncate_tail(&mut s, 8);
        assert_eq!(s, "x.rs:1:1:".chars().skip(1).collect::<String>());
        assert_eq!(dropped as usize, 10 + "panicked at x".len());
    }
}
