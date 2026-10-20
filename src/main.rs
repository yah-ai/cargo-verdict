//! `cargo verdict` — the CLI face.
//!
//! Installed as `cargo-verdict` on PATH, cargo runs it for `cargo verdict …`
//! and passes `verdict` as the first argument; invoked directly, it is absent.
//! Usable from a terminal, a QED step, or a camp that does not run yah.
//!
//! The exit code is cargo's own. A usage error is 2 and says so on stderr;
//! a cargo ended by a signal exits 128 + the signal, the shell's convention,
//! while the JSON says `exit_code: null`.

use std::process::ExitCode;

use cargo_verdict::exec::{self, Exit};
use cargo_verdict::runner::discover;
use cargo_verdict::verdict::{schema, Binary};
use cargo_verdict::{parse, plan, Class, Demux, Stream, Verdict, Want};

const USAGE: &str = "\
usage: cargo verdict <verb> [cargo args…] [--want summary|failures|diagnostics|full] [--json]
       cargo verdict parse <cargo args…>
       cargo verdict schema

  <verb>   a cargo verdict: check, clippy, build, test, doc, bench, rustc, …
           runs it with structured output and prints the result: a summary
           by default, the cargo-verdict/1 JSON with --json. --want picks the
           view (default: failures). Exits with cargo's exit code.
  parse    print the normalized request, its class and canonical argv as JSON
  schema   print the JSON Schema of cargo-verdict/1

  --want and --json are read before any `--`; everything else is cargo's.";

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("cargo verdict: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("verdict") {
        args.remove(0);
    }
    match args.first().map(String::as_str) {
        None => usage_error("no verb"),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("parse") => {
            let request = parse(&args[1..]);
            let out = serde_json::json!({
                "class": request.class(),
                "argv": request.render(),
                "request": request,
            });
            println!("{}", serde_json::to_string_pretty(&out).expect("a Request always serializes"));
            ExitCode::SUCCESS
        }
        Some("schema") => {
            println!("{}", serde_json::to_string_pretty(&schema()).expect("a schema serializes"));
            ExitCode::SUCCESS
        }
        Some(_) => run(args),
    }
}

/// Our own flags, taken out of the argv before cargo's first `--`.
fn own_flags(args: Vec<String>) -> Result<(Vec<String>, Want, bool), String> {
    let mut want = Want::default();
    let mut json = false;
    let mut cargo = Vec::with_capacity(args.len());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--" {
            cargo.push(a);
            cargo.extend(it);
            break;
        }
        if a == "--json" {
            json = true;
        } else if a == "--want" {
            want = it.next().ok_or("--want needs a value")?.parse()?;
        } else if let Some(v) = a.strip_prefix("--want=") {
            want = v.parse()?;
        } else {
            cargo.push(a);
        }
    }
    Ok((cargo, want, json))
}

fn run(args: Vec<String>) -> ExitCode {
    let (cargo_args, want, json) = match own_flags(args) {
        Ok(x) => x,
        Err(e) => return usage_error(&e),
    };
    let request = parse(&cargo_args);
    if request.class() != Class::Verdict {
        return usage_error(&format!(
            "`cargo {}` is a {:?} request, not a verdict; run it with cargo",
            cargo_args.join(" "),
            request.class()
        ));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let opts = discover(&request, &cwd, &env);
    let plan = plan(&request, &opts);

    let mut demux = Demux::new();
    let exit = match exec::run(&plan, None, |stream, line| {
        // cargo's progress (and any binary's own prints) is the operator's to
        // watch; it goes to stderr, so stdout stays the result alone.
        if stream != Stream::Stdout && !line.trim_start().starts_with('{') {
            eprintln!("{line}");
        }
        demux.push(stream, line);
    }) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("cargo verdict: cannot run cargo: {e}");
            return ExitCode::from(2);
        }
    };
    let verdict = Verdict::collect(&request, &plan, &demux, exit).project(want);
    if json {
        println!("{}", serde_json::to_string_pretty(&verdict).expect("a Verdict serializes"));
    } else {
        print!("{}", human(&verdict));
    }
    match exit {
        Exit::Code(c) => ExitCode::from(u8::try_from(c).unwrap_or(1)),
        Exit::Signalled(sig) => ExitCode::from(sig.map_or(1, |s| (128 + s).clamp(0, 255) as u8)),
    }
}

/// The terminal rendering of a projected verdict.
fn human(v: &Verdict) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let code = v.exit_code.map_or("signal".to_string(), |c| c.to_string());
    let _ = writeln!(s, "cargo verdict: {:?} (exit {code}) — cargo {}", v.outcome, v.argv.join(" "));
    let _ = writeln!(
        s,
        "build: {} error(s), {} warning(s); {} compiled, {} fresh",
        v.build.counts.error, v.build.counts.warning, v.build.units.compiled, v.build.units.fresh
    );
    for d in &v.build.diagnostics {
        match &d.rendered {
            Some(r) => s.push_str(r),
            None => {
                let _ = writeln!(s, "{}: {}", d.level, d.message);
            }
        }
    }
    let t = &v.tests.totals;
    if !v.tests.binaries.is_empty() {
        let _ = writeln!(
            s,
            "tests: {} passed, {} failed, {} ignored, {} filtered out{}",
            t.passed,
            t.failed,
            t.ignored,
            t.filtered_out,
            if t.unknown_binaries > 0 { format!("; {} binaries unstructured", t.unknown_binaries) } else { String::new() }
        );
    }
    for b in &v.tests.binaries {
        for f in &b.failures {
            let _ = writeln!(s, "  FAILED {} {}", label(b), f.name);
            if let Some(l) = &f.location {
                let _ = writeln!(s, "    at {l}");
            }
            if f.output_truncated_bytes > 0 {
                let _ = writeln!(s, "    … {} bytes dropped", f.output_truncated_bytes);
            }
            for line in f.output.lines() {
                let _ = writeln!(s, "    | {line}");
            }
        }
        if let Some(o) = &b.output {
            let _ = writeln!(s, "  output of {} (structured: {}):", label(b), b.structured);
            if b.output_truncated_bytes > 0 {
                let _ = writeln!(s, "    … {} bytes dropped", b.output_truncated_bytes);
            }
            for line in o.lines() {
                let _ = writeln!(s, "    | {line}");
            }
        }
    }
    for n in &v.notes {
        let _ = writeln!(s, "note: {n}");
    }
    s
}

fn label(b: &Binary) -> String {
    match (&b.package, &b.target) {
        (Some(p), Some(t)) if p != t => format!("{p}/{t}"),
        (Some(p), _) => p.clone(),
        (None, Some(t)) => t.clone(),
        (None, None) => format!("{:?}", b.kind).to_lowercase(),
    }
}
