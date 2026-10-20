//! `render(Request) -> argv`.
//!
//! Emits one canonical spelling: long-form verbs, `--profile NAME` rather than
//! `--release`, features joined by commas, test filters after `--`. Opaque
//! lists (`passthrough`, `args`, `trailing`) come out verbatim, in their
//! original order, after everything modelled, so a value an unknown flag took
//! is still beside it.

use crate::request::{Ignored, Request};
use crate::verb::{Dialect, Verb};

/// Render a [`Request`] as cargo's arguments, without the leading `cargo`.
pub fn render(r: &Request) -> Vec<String> {
    let mut out = Out(Vec::new());
    if let Some(tc) = &r.toolchain {
        out.push(format!("+{tc}"));
    }
    if let Some(dir) = &r.globals.directory {
        out.attached("-C", dir);
    }
    for v in &r.globals.config {
        out.value("--config", v);
    }
    for v in &r.globals.unstable {
        out.attached("-Z", v);
    }
    out.extend(&r.globals.passthrough);

    let dialect = r.verb.dialect();
    if dialect == Dialect::Opaque {
        // Position is meaning for an external subcommand, so a help flag cargo
        // saw before the verb stays before it.
        if r.help {
            out.push("--help".into());
        }
        // `cargo` and `cargo help` are one request; the bare spelling is the
        // canonical one, so a valueless flag before it cannot take the word
        // `help` as its value on reparse.
        if !(r.verb == Verb::Help && r.args.is_empty()) {
            out.push(r.verb.word().into());
            if let Verb::Nextest(Some(sub)) = &r.verb {
                out.push(sub.clone());
            }
        }
        out.extend(&r.args);
        return out.0;
    }

    out.push(r.verb.word().into());
    let nextest = r.verb.is_nextest();
    if let Verb::Nextest(Some(sub)) = &r.verb {
        out.push(sub.clone());
    }
    // Operands sit right after the verb, where no flag can claim one.
    out.extend(&r.args);

    if let Some(v) = &r.manifest_path {
        out.value("--manifest-path", v);
    }
    if let Some(v) = &r.target_dir {
        out.value("--target-dir", v);
    }

    let s = &r.selection;
    out.flag("--workspace", s.workspace);
    for p in &s.packages {
        out.value("--package", p);
    }
    for p in &s.exclude {
        out.value("--exclude", p);
    }

    let t = &r.targets;
    out.flag("--lib", t.lib);
    out.flag("--bins", t.bins);
    for v in &t.bin {
        out.optional_value("--bin", v);
    }
    out.flag("--examples", t.examples);
    for v in &t.example {
        out.optional_value("--example", v);
    }
    out.flag("--tests", t.tests);
    for v in &t.test {
        out.optional_value("--test", v);
    }
    out.flag("--benches", t.benches);
    for v in &t.bench {
        out.optional_value("--bench", v);
    }
    out.flag("--all-targets", t.all_targets);
    out.flag("--doc", t.doc);

    let f = &r.features;
    out.flag("--all-features", f.all);
    out.flag("--no-default-features", f.no_default);
    if !f.list.is_empty() {
        out.value("--features", &f.list.join(","));
    }

    if let Some(p) = &r.profile {
        out.value(if nextest { "--cargo-profile" } else { "--profile" }, p);
    }
    for v in &r.target_triples {
        out.value("--target", v);
    }
    for v in &r.message_format {
        out.value("--message-format", v);
    }
    for v in &r.config {
        out.value("--config", v);
    }
    for v in &r.unstable {
        out.attached("-Z", v);
    }

    let test = &r.test;
    let mut harness = Out(Vec::new());
    if dialect == Dialect::Test {
        out.flag("--no-run", test.no_run);
        out.flag("--no-fail-fast", !test.fail_fast);
        if nextest {
            if let Some(n) = test.threads {
                out.push(format!("--test-threads={n}"));
            }
            out.flag("--no-capture", test.nocapture);
            match test.ignored {
                Ignored::Exclude => {}
                Ignored::Include => out.push("--run-ignored=all".into()),
                Ignored::Only => out.push("--run-ignored=only".into()),
            }
        }
        // Filters first: nothing rendered before them can take one as a value.
        harness.extend(&test.filters);
        harness.flag("--exact", test.exact);
        for v in &test.skip {
            harness.value("--skip", v);
        }
        if !nextest {
            match test.ignored {
                Ignored::Exclude => {}
                Ignored::Include => harness.push("--include-ignored".into()),
                Ignored::Only => harness.push("--ignored".into()),
            }
            if let Some(n) = test.threads {
                harness.push(format!("--test-threads={n}"));
            }
            harness.flag("--nocapture", test.nocapture);
        }
    }

    out.flag("--help", r.help);
    out.extend(&r.passthrough);
    harness.extend(&r.trailing);
    if !harness.0.is_empty() {
        out.push("--".into());
        out.0.extend(harness.0);
    }
    out.0
}

struct Out(Vec<String>);

impl Out {
    fn push(&mut self, s: String) {
        self.0.push(s);
    }
    fn extend(&mut self, xs: &[String]) {
        self.0.extend(xs.iter().cloned());
    }
    fn flag(&mut self, name: &str, on: bool) {
        if on {
            self.0.push(name.into());
        }
    }
    /// `--name value`, or `--name=value` when the value would not survive as a
    /// separate token (it is `--`, or looks like a flag).
    fn value(&mut self, name: &str, v: &str) {
        if v.starts_with('-') {
            self.0.push(format!("{name}={v}"));
        } else {
            self.0.push(name.into());
            self.0.push(v.into());
        }
    }
    /// For `--bin [NAME]`: a separate value is only taken if it is positional,
    /// which [`Out::value`]'s rule already guarantees.
    fn optional_value(&mut self, name: &str, v: &str) {
        self.value(name, v)
    }
    /// A short flag with no long form (`-Z`, `-C`): attached when unsafe.
    fn attached(&mut self, name: &str, v: &str) {
        if v.starts_with('-') {
            self.0.push(format!("{name}{v}"));
        } else {
            self.0.push(name.into());
            self.0.push(v.into());
        }
    }
}
