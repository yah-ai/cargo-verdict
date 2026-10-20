//! The execution plan: what cargo-verdict adds to a request so that its output
//! can be read as structure, and nothing else.
//!
//! **The plan adds output and runner flags only**, and lists each one in
//! [`Plan::added`]; [`Plan::argv`] is exactly what runs. It never adds a flag
//! that changes what is built, and never adds environment to cargo: the one
//! variable libtest JSON needs, `RUSTC_BOOTSTRAP`, is set by the test
//! *runner*, inside the process cargo spawns for a test binary, because on
//! cargo itself proc-macro2's build script tracks it and a whole new
//! generation gets built (W371 §4.4(3)).
//!
//! Two departures from W371 §4.5 as drafted, both measured on rustc/cargo
//! 1.97.0, 2026-10-17, and both held by tests:
//!
//! 1. **`--message-format=json`, not `json-render-diagnostics`.** Under
//!    `json-render-diagnostics` cargo renders diagnostics to stderr *instead
//!    of* emitting `compiler-message` lines: a `cargo check` with one warning
//!    printed one `compiler-artifact` and one `build-finished`, and no
//!    `compiler-message`. Under `json` the warning arrives as a
//!    `compiler-message` whose `rendered` field is the human text. The typed
//!    door needs the diagnostics, so it asks for `json`.
//! 2. **The libtest flags travel inside the runner, not after `--`.** `cargo
//!    test -- -Z unstable-options --format json` reaches rustdoc's doctest
//!    harness too, which runs without `RUSTC_BOOTSTRAP` and fails with "the
//!    option `Z` is only accepted on the nightly compiler" — an invented
//!    failure (W371 §4.4 R989-S3 case 2). The runner ([`runner_script`])
//!    appends them only for the binaries cargo runs itself, so filters still
//!    reach the binaries and a plain build afterwards is Fresh.
//!
//! **The runner composes with the camp's own** (R989-S3 case 3: a `--config`
//! runner *replaces* one set in a config file or in the environment,
//! silently). [`crate::runner::discover`] reads the effective runner the way
//! cargo does, and the script runs it: `exec env RUSTC_BOOTSTRAP=1 <camp
//! runner…> "$0" "$@" -Z …`. When it cannot be sure which runner cargo would
//! use, the plan adds no runner at all and says so in [`Plan::unstructured`].
//!
//! **Binaries that are not libtest harnesses get their arguments untouched**:
//! every target whose manifest says `harness = false` (R989-S3 case 1: such a
//! binary receives the flags as plain argv, and one that rejects unknown
//! arguments fails), and rustdoc's doctest binaries (`rust_out`), which cargo
//! also hands to the runner. They still run under the camp's runner, and are
//! reported `structured: false`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::request::Request;
use crate::runner::CampRunner;
use crate::verb::{Class, Verb};

/// What the scoped runner appends for a libtest binary.
pub const LIBTEST_FLAGS: &str = "-Z unstable-options --format json --report-time";

/// The script the scoped runner runs. cargo calls `sh -c SCRIPT <binary>
/// <args…>`, so `$0` is the binary. `camp_runner` is the runner cargo would
/// otherwise use (empty for none); `no_harness` are the file stems of
/// `harness = false` targets, which, like rustdoc's `rust_out`, run under the
/// camp's runner with their arguments untouched.
///
/// A stem is matched as `<stem>-<16 characters>`, cargo's `deps/` naming, so a
/// lib and a `harness = false` test with the same name both lose structure
/// (never the other way round).
pub fn runner_script(camp_runner: &[String], no_harness: &BTreeSet<String>) -> String {
    let pre: String = camp_runner.iter().map(|w| format!("{} ", sh_quote(w))).collect();
    let mut pats = vec!["rust_out".to_string()];
    pats.extend(no_harness.iter().map(|n| format!("{}-{}", sh_quote(n), "?".repeat(16))));
    format!(
        r#"case "${{0##*/}}" in {}) exec {pre}"$0" "$@";; esac; exec env RUSTC_BOOTSTRAP=1 {pre}"$0" "$@" {LIBTEST_FLAGS}"#,
        pats.join("|")
    )
}

fn sh_quote(w: &str) -> String {
    if !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c)) {
        w.to_string()
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
}

/// What the caller knows about the machine ([`crate::runner::discover`]
/// fills it from the real one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanOptions {
    /// rustc's host triple (`rustc -vV`'s `host:`), used when the request
    /// names no `--target` and the config sets no `build.target`.
    pub host_triple: String,
    /// `CARGO_BUILD_TARGET` / `build.target`: what cargo builds for when the
    /// request names no `--target`. Empty means the host.
    pub default_targets: Vec<String>,
    /// The runner cargo would use for each triple, where there is one.
    pub runners: BTreeMap<String, CampRunner>,
    /// File stems of the `harness = false` targets, or why they are unknown
    /// (which keeps the libtest flags off every binary).
    pub no_harness: Result<BTreeSet<String>, String>,
    /// Is there a POSIX `sh` to host the scoped runner?
    pub posix_sh: bool,
}

impl PlanOptions {
    /// A machine with no camp runner and no `harness = false` target.
    pub fn new(host_triple: impl Into<String>) -> PlanOptions {
        PlanOptions {
            host_triple: host_triple.into(),
            default_targets: Vec::new(),
            runners: BTreeMap::new(),
            no_harness: Ok(BTreeSet::new()),
            posix_sh: cfg!(unix),
        }
    }

    /// The triples a request runs its tests for.
    fn triples<'a>(&'a self, req: &'a Request) -> Vec<&'a str> {
        if !req.target_triples.is_empty() {
            req.target_triples.iter().map(String::as_str).collect()
        } else if !self.default_targets.is_empty() {
            self.default_targets.iter().map(String::as_str).collect()
        } else {
            vec![self.host_triple.as_str()]
        }
    }
}

/// A planned request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// The request as it will run: the original plus what [`Self::added`] lists.
    pub request: Request,
    /// Exactly what runs, after `cargo`.
    pub argv: Vec<String>,
    /// Every flag the plan added, as it appears in `argv`.
    pub added: Vec<String>,
    /// stdout carries cargo's JSON messages.
    pub cargo_json: bool,
    /// …and they include `compiler-message` (not under `json-render-diagnostics`).
    pub diagnostics_json: bool,
    /// Test binaries are asked for libtest JSON.
    pub libtest_json: bool,
    /// Why some structure is off, one line each. Empty when nothing is.
    pub unstructured: Vec<String>,
}

impl Plan {
    /// The plan's additions as argv tokens (`--message-format`, `json`,
    /// `--config`, `target.….runner=[…]`), in the order they render.
    pub fn added_argv(&self, original: &Request) -> Vec<String> {
        let mut out = Vec::new();
        for f in &self.request.message_format[original.message_format.len()..] {
            out.extend(["--message-format".to_string(), f.clone()]);
        }
        for c in &self.request.config[original.config.len()..] {
            out.extend(["--config".to_string(), c.clone()]);
        }
        out
    }

    /// `args` (cargo's arguments as the caller wrote them, without `cargo`)
    /// with the plan's additions spliced in before cargo's own `--`, or at the
    /// end. For callers that must keep the user's spelling rather than the
    /// canonical [`Plan::argv`] — a run store showing the command as typed.
    /// `parse` of the result equals [`Plan::request`] whenever `parse(args)`
    /// equals `original` (held by a test).
    pub fn apply_to<S: AsRef<str>>(&self, original: &Request, args: &[S]) -> Vec<String> {
        let args: Vec<String> = args.iter().map(|a| a.as_ref().to_string()).collect();
        let at = cargo_args_end(&args);
        let mut out = args[..at].to_vec();
        out.extend(self.added_argv(original));
        out.extend_from_slice(&args[at..]);
        out
    }
}

/// Where cargo's own arguments end in `args` (without `cargo`): the index of
/// the `--` after the verb, or `args.len()`. Where a planned flag can be
/// inserted without becoming the program's argument. Before the verb, a value
/// flag's value is skipped so it is not taken for the verb, as
/// [`crate::parse`] does.
pub fn cargo_args_end<S: AsRef<str>>(args: &[S]) -> usize {
    cargo_separator(args).unwrap_or(args.len())
}

fn cargo_separator<S: AsRef<str>>(args: &[S]) -> Option<usize> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_ref();
        if a.starts_with('+') || a == "-h" || a == "--help" {
            i += 1;
        } else if matches!(a, "--config" | "-Z" | "-C" | "--color") {
            i += 2;
        } else if a.starts_with('-') && a != "-" {
            i += 1;
        } else {
            break;
        }
    }
    args.iter().skip(i + 1).position(|a| a.as_ref() == "--").map(|p| p + i + 1)
}

/// Plan `req`. Requests that are not verdicts are returned unchanged.
pub fn plan(req: &Request, opts: &PlanOptions) -> Plan {
    let mut p = Plan {
        request: req.clone(),
        argv: Vec::new(),
        added: Vec::new(),
        cargo_json: false,
        diagnostics_json: false,
        libtest_json: false,
        unstructured: Vec::new(),
    };
    if req.class() != Class::Verdict {
        p.argv = req.render();
        return p;
    }

    if matches!(req.verb, Verb::Nextest(_)) {
        // nextest's `--message-format` is its own, and its JSON is not adapted:
        // the operator chose cargo as the runner (W371 §8 call 5).
        p.unstructured.push("nextest output is not adapted; run `cargo test` for structure".into());
        p.argv = req.render();
        return p;
    }

    // ── cargo's messages ──
    if req.message_format.is_empty() {
        p.request.message_format.push("json".into());
        p.added.push("--message-format json".into());
        p.cargo_json = true;
        p.diagnostics_json = true;
    } else {
        p.cargo_json = req.message_format.iter().any(|f| f.starts_with("json"));
        p.diagnostics_json =
            p.cargo_json && !req.message_format.iter().any(|f| f == "json-render-diagnostics");
        if !p.cargo_json {
            p.unstructured.push("the request chose a human --message-format".into());
        } else if !p.diagnostics_json {
            p.unstructured.push("json-render-diagnostics sends diagnostics to stderr, not as JSON".into());
        }
    }

    // ── libtest ──
    if req.verb == Verb::Bench {
        p.unstructured.push("bench harnesses (criterion and friends) do not take libtest's flags".into());
    } else if req.verb == Verb::Test && !req.test.no_run {
        match libtest_refusal(req, opts) {
            Some(why) => p.unstructured.push(why),
            None => {
                let none = BTreeSet::new();
                let no_harness = opts.no_harness.as_ref().unwrap_or(&none);
                for t in opts.triples(req) {
                    let camp: &[String] = match opts.runners.get(t) {
                        Some(CampRunner::Compose { argv, .. }) => argv,
                        _ => &[],
                    };
                    let runner = serde_json::to_string(&["sh", "-c", &runner_script(camp, no_harness)])
                        .expect("three strings serialize");
                    let cfg = format!("target.{t}.runner={runner}");
                    p.added.push(format!("--config {cfg}"));
                    p.request.config.push(cfg);
                }
                p.libtest_json = true;
                if req.targets.is_default() || req.targets.doc {
                    p.unstructured.push(
                        "doctests run without libtest JSON (rustdoc's harness takes no libtest flags on stable, so the runner passes them through untouched); they are reported structured: false".into(),
                    );
                }
                if !no_harness.is_empty() {
                    p.unstructured.push(format!(
                        "harness = false targets ({}) get their arguments untouched and are reported structured: false",
                        no_harness.iter().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
        }
    }

    p.argv = p.request.render();
    p
}

/// Why this test request cannot carry the scoped runner, if it cannot.
fn libtest_refusal(req: &Request, opts: &PlanOptions) -> Option<String> {
    if !opts.posix_sh {
        return Some("no POSIX sh to host the scoped test runner".into());
    }
    if opts.host_triple.is_empty() && req.target_triples.is_empty() {
        return Some("rustc's host triple is unknown, so the runner cannot be scoped".into());
    }
    if req.config.iter().chain(&req.globals.config).any(|c| sets_runner(c)) {
        return Some("the request sets its own target runner".into());
    }
    let triples = opts.triples(req);
    if let Some(t) = triples.iter().find(|t| !is_bare_key(t)) {
        return Some(format!("target `{t}` cannot be named in a --config key"));
    }
    for t in &triples {
        if let Some(CampRunner::Unknown(why)) = opts.runners.get(*t) {
            return Some(format!(
                "a target runner may be configured for {t} and cannot be resolved ({why}); the plan adds no runner, so it runs as configured"
            ));
        }
    }
    if let Err(why) = &opts.no_harness {
        return Some(format!("the harness = false targets are unknown ({why}), so the libtest flags cannot be kept from them"));
    }
    if req.trailing.iter().any(|a| a == "--") {
        // Our flags are appended to the binary's argv; past a `--` they would
        // become filters.
        return Some("the request passes args after a second `--`".into());
    }
    if req.trailing.iter().any(|a| {
        matches!(a.as_str(), "-Z" | "--format" | "--logfile" | "--list")
            || a.starts_with("--format=")
            || a.starts_with("-Z")
    }) {
        return Some("the request chose its own libtest output".into());
    }
    None
}

/// Does a `--config` value set a target runner? (`target.<t>.runner=…`, and
/// `target.'cfg(…)'.runner`.) A `--config <file>` is read by
/// [`crate::runner::discover`].
fn sets_runner(cfg: &str) -> bool {
    let key = cfg.split('=').next().unwrap_or(cfg);
    key.trim().starts_with("target.") && key.trim_end().ends_with(".runner")
}

fn is_bare_key(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// rustc's host triple, from `rustc -vV`.
pub fn host_triple() -> std::io::Result<String> {
    let out = std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("-vV")
        .output()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .ok_or_else(|| std::io::Error::other("`rustc -vV` printed no host line"))
}
