//! R989-F20: plan + exec + demux against real cargo, on the fixture crates in
//! tests/fixtures, plus the plan's must-not list, each held by a test.

use std::path::{Path, PathBuf};

use cargo_verdict::cargo_metadata::{diagnostic::DiagnosticLevel, Message};
use cargo_verdict::exec::{self, Exit};
use cargo_verdict::libtest::panic_location;
use std::collections::{BTreeMap, BTreeSet};

use cargo_verdict::plan::{host_triple, runner_script, LIBTEST_FLAGS};
use cargo_verdict::runner::{discover, CampRunner, CargoConfig, Cfgs};
use cargo_verdict::{parse, plan, BinaryKind, Demux, PlanOptions, Request, TestEvent};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn target_dir(tag: &str) -> String {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fixture-{tag}")).display().to_string()
}

fn opts() -> PlanOptions {
    // The camp's real configuration is not under test here: no runner.
    PlanOptions::new(host_triple().expect("rustc -vV"))
}

fn request(line: &str, target: &str) -> Request {
    let mut r = parse(&line.split_whitespace().collect::<Vec<_>>());
    r.env_allowlist.insert("CARGO_TARGET_DIR".into(), target_dir(target));
    r
}

/// Plan and run `line` in fixture `name`.
fn run(name: &str, line: &str, target: &str) -> (cargo_verdict::Plan, Demux, Exit) {
    let p = plan(&request(line, target), &opts());
    let mut d = Demux::new();
    let exit = exec::run(&p, Some(&fixture(name)), |s, l| d.push(s, l)).expect("cargo spawns");
    (p, d, exit)
}

fn all_fresh(name: &str, line: &str, target: &str) -> (usize, usize) {
    let r = request(line, target);
    let p = cargo_verdict::Plan {
        argv: r.render(),
        request: r,
        added: vec![],
        cargo_json: true,
        diagnostics_json: true,
        libtest_json: false,
        unstructured: vec![],
    };
    let mut d = Demux::new();
    exec::run(&p, Some(&fixture(name)), |s, l| d.push(s, l)).unwrap();
    let arts: Vec<bool> = d
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::CompilerArtifact(a) => Some(a.fresh),
            _ => None,
        })
        .collect();
    (arts.iter().filter(|f| **f).count(), arts.len())
}

#[test]
fn the_mixed_fixture_demuxes_into_its_parts_and_leaves_the_build_fresh() {
    let (p, d, exit) = run("mixed", "test --no-fail-fast", "mixed");
    assert!(p.libtest_json && p.diagnostics_json, "{p:#?}");
    // cargo's own verdict: a test failed.
    assert_eq!(exit, Exit::Code(101));

    // The warning arrives as a compiler-message, with its human rendering.
    let warnings: Vec<_> = d
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::CompilerMessage(c) if c.message.level == DiagnosticLevel::Warning => Some(&c.message),
            _ => None,
        })
        .collect();
    assert!(warnings.iter().any(|w| w.message.contains("unused variable")), "{warnings:#?}");
    assert!(warnings.iter().all(|w| w.rendered.is_some()));
    assert!(d.messages.iter().any(|m| matches!(m, Message::BuildFinished(b) if b.success)));

    let kinds: Vec<_> = d.binaries.iter().map(|b| (b.kind, b.structured)).collect();
    assert_eq!(
        kinds,
        [
            (BinaryKind::Unittests, true),
            (BinaryKind::Test, true),     // tests/it.rs
            (BinaryKind::Test, false),    // tests/noharness.rs, harness = false
            (BinaryKind::Doctest, false), // rustdoc, untouched
        ],
        "{:#?}",
        d.binaries
    );

    let unit = &d.binaries[0];
    assert!(d.artifact_of(unit).is_some(), "the unittests binary maps to its artifact");
    let failed = unit
        .events
        .iter()
        .find_map(|e| match e {
            TestEvent::TestFailed { name, stdout, .. } => Some((name, stdout)),
            _ => None,
        })
        .expect("t::bad failed");
    assert_eq!(failed.0, "t::bad");
    let out = failed.1.as_deref().unwrap();
    assert!(out.contains("captured before the panic"));
    assert_eq!(panic_location(out).as_deref(), Some("src/lib.rs:17:9"));
    assert!(unit.events.iter().any(|e| matches!(e, TestEvent::TestIgnored { name, message } if name == "t::ign" && message.as_deref() == Some("slow"))));
    assert!(unit.events.iter().any(|e| matches!(e, TestEvent::SuiteFinished { ok: false, passed: 1, failed: 1, ignored: 1, .. })));

    let noharness = &d.binaries[2];
    assert!(noharness.raw.iter().any(|&i| d.lines[i].text == "custom harness ran"));
    let doctest = &d.binaries[3];
    assert!(doctest.raw.iter().any(|&i| d.lines[i].text.starts_with("test result: ok. 1 passed")));
    assert!(
        !d.lines.iter().any(|l| l.text.contains("only accepted on the nightly compiler")),
        "the libtest flags must not reach rustdoc"
    );

    // MUST-NOT: never change a unit hash. After the planned run, a plain
    // test build and a plain build are entirely Fresh.
    let (fresh, total) = all_fresh("mixed", "test --no-run --message-format json", "mixed");
    assert!(total > 0 && fresh == total, "{fresh}/{total} fresh after a planned run");
    let (fresh, total) = all_fresh("mixed", "build --message-format json", "mixed");
    assert!(total > 0 && fresh == total, "{fresh}/{total} fresh after a planned run");
}

#[test]
fn a_filter_still_reaches_the_binary() {
    let (_, d, exit) = run("mixed", "test --lib ok", "filter");
    assert_eq!(exit, Exit::Code(0));
    assert_eq!(d.binaries.len(), 1);
    assert!(d.binaries[0].events.iter().any(|e| matches!(e, TestEvent::SuiteFinished { ok: true, passed: 1, filtered_out: 2, .. })));
}

#[test]
fn a_failing_build_is_a_compiler_error_and_cargo_s_exit_code() {
    let (p, d, exit) = run("broken", "check", "broken");
    assert!(!p.libtest_json);
    assert_eq!(exit, Exit::Code(101));
    let err = d
        .messages
        .iter()
        .find_map(|m| match m {
            Message::CompilerMessage(c) if c.message.level == DiagnosticLevel::Error => Some(&c.message),
            _ => None,
        })
        .expect("an error diagnostic");
    assert_eq!(err.code.as_ref().map(|c| c.code.as_str()), Some("E0308"));
    let span = err.spans.iter().find(|s| s.is_primary).unwrap();
    assert_eq!((span.file_name.as_str(), span.line_start), ("src/lib.rs", 2));
    assert!(d.messages.iter().any(|m| matches!(m, Message::BuildFinished(b) if !b.success)));
}

// ─── The plan's must-not list ─────────────────────────────────────────────────

const LINES: &[&str] = &[
    "test",
    "test -p x --lib",
    "test --doc",
    "+nightly test --workspace --features a -- --exact foo",
    "test --target aarch64-unknown-linux-gnu",
    "test --no-run",
    "test -- --format pretty",
    "test -- -- weird",
    "test --config target.x.runner=qemu",
    "test --message-format json-render-diagnostics",
    "test --message-format short",
    "check -p x",
    "clippy -- -D warnings",
    "build --release",
    "doc --no-deps",
    "bench",
    "nextest run",
    "run -p x",
    "fmt",
    "metadata",
    "build --help",
];

#[test]
fn the_plan_never_puts_rustc_bootstrap_on_cargo() {
    for line in LINES {
        let mut r = parse(&line.split_whitespace().collect::<Vec<_>>());
        r.env_allowlist.insert("RUST_LOG".into(), "x".into());
        let p = plan(&r, &PlanOptions::new("x86_64-unknown-linux-gnu"));
        assert_eq!(p.request.env_allowlist, r.env_allowlist, "{line}: the plan touched env");
        assert!(!p.request.env_allowlist.contains_key("RUSTC_BOOTSTRAP"), "{line}");
        for a in &p.argv {
            // The only RUSTC_BOOTSTRAP anywhere is inside the runner, which
            // cargo runs for test binaries, never for itself or rustc.
            if a.contains("RUSTC_BOOTSTRAP") {
                assert!(a.starts_with("target.") && a.contains(".runner=") && a.contains(LIBTEST_FLAGS), "{line}: {a}");
            }
        }
    }
}

#[test]
fn the_plan_never_changes_what_a_request_builds() {
    for line in LINES {
        let r = parse(&line.split_whitespace().collect::<Vec<_>>());
        let p = plan(&r, &PlanOptions::new("x86_64-unknown-linux-gnu"));
        // Undo exactly the two fields the plan may add to; what is left must
        // be the request as given.
        let mut back = p.request.clone();
        back.message_format.truncate(r.message_format.len());
        back.config.truncate(r.config.len());
        assert_eq!(back, r, "{line}: the plan changed more than output/runner flags");
        // And argv is the planned request, verbatim: nothing hidden.
        assert_eq!(p.argv, p.request.render(), "{line}");
        assert_eq!(parse(&p.argv), p.request, "{line}");
        // The additions spliced into the argv as written parse to the plan.
        let written: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parse(&p.apply_to(&r, &written)), p.request, "{line}: apply_to");
        // Every addition is listed.
        let added = (p.request.message_format.len() - r.message_format.len())
            + (p.request.config.len() - r.config.len());
        assert_eq!(added, p.added.len(), "{line}");
    }
}

#[test]
fn the_plan_is_honest_about_what_it_cannot_structure() {
    let o = PlanOptions::new("x86_64-unknown-linux-gnu");
    let p = |line: &str| plan(&parse(&line.split_whitespace().collect::<Vec<_>>()), &o);
    assert!(p("test -p x --lib").unstructured.is_empty());
    assert!(p("test").unstructured[0].contains("doctests"));
    for line in [
        "test -- --format pretty",
        "test -- -- weird",
        "test --config target.x.runner=qemu",
        "bench",
        "nextest run",
        "test --message-format short",
    ] {
        let pl = p(line);
        assert!(!pl.unstructured.is_empty(), "{line}");
    }
    assert!(!p("test --config target.x.runner=qemu").libtest_json);
    // A runner the plan cannot resolve: no runner added, so the camp's runs
    // as configured, and the tests are unstructured.
    let mut unknown = o.clone();
    unknown.runners.insert("x86_64-unknown-linux-gnu".into(), CampRunner::Unknown("two cfg tables match".into()));
    let pl = plan(&parse(&["test", "--lib"]), &unknown);
    assert!(!pl.libtest_json && pl.added.len() == 1 && pl.unstructured[0].contains("two cfg tables match"), "{pl:#?}");
    // Unknown harness = false targets: the same.
    let mut blind = o.clone();
    blind.no_harness = Err("cargo metadata failed".into());
    assert!(!plan(&parse(&["test", "--lib"]), &blind).libtest_json);
    // A resolved one is composed: it runs inside the scoped runner, after env.
    let mut camp = o.clone();
    let qemu = vec!["qemu-x86_64".to_string(), "-L".into(), "/sys root".into()];
    camp.runners.insert("x86_64-unknown-linux-gnu".into(), CampRunner::Compose { argv: qemu.clone(), source: "test".into() });
    camp.no_harness = Ok(BTreeSet::from(["strict".to_string()]));
    let pl = plan(&parse(&["test", "--lib"]), &camp);
    assert!(pl.libtest_json, "{pl:#?}");
    let script = runner_script(&qemu, &BTreeSet::from(["strict".to_string()]));
    assert!(script.contains(r#"exec env RUSTC_BOOTSTRAP=1 qemu-x86_64 -L '/sys root' "$0" "$@" -Z"#), "{script}");
    assert!(script.contains("rust_out|strict-????????????????) exec qemu-x86_64"), "{script}");
    let value = serde_json::to_string(&["sh", "-c", &script]).unwrap();
    assert_eq!(pl.request.config, [format!("target.x86_64-unknown-linux-gnu.runner={value}")]);
    assert!(pl.unstructured.iter().any(|n| n.contains("harness = false targets (strict)")), "{pl:#?}");
    // build.target, when the request names no --target, is what gets the runner.
    let mut cross = o.clone();
    cross.default_targets = vec!["aarch64-unknown-linux-gnu".into()];
    let pl = plan(&parse(&["test", "--lib"]), &cross);
    assert!(pl.request.config[0].starts_with("target.aarch64-unknown-linux-gnu.runner="), "{pl:#?}");
    // Non-verdicts run as written.
    for line in ["run -p x", "fmt", "metadata", "build --help"] {
        let pl = p(line);
        assert!(pl.added.is_empty() && pl.argv == pl.request.render(), "{line}");
    }
}


// ─── The camp's runner (R989-S3): read as cargo reads it, then composed ──────

/// A scratch tree `<tag>/{.cargo,a/b/.cargo}` and an env with an empty
/// CARGO_HOME, so only the files a case writes (and this checkout's own
/// ancestors, which set no runner) are read.
fn config_tree(tag: &str) -> (PathBuf, BTreeMap<String, String>) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("config").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    std::fs::create_dir_all(root.join("a/b/.cargo")).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
    let env = BTreeMap::from([("CARGO_HOME".to_string(), root.join("home").display().to_string())]);
    (root, env)
}

const T: &str = "x86_64-unknown-linux-gnu";

fn lookup(root: &Path, env: &BTreeMap<String, String>) -> Option<CampRunner> {
    CargoConfig::load(&root.join("a/b"), env).runner_for(T, || Ok(Cfgs::parse("unix\ntarget_os=\"linux\"\n")))
}

fn argv(r: Option<CampRunner>) -> Vec<String> {
    match r {
        Some(CampRunner::Compose { argv, .. }) => argv,
        other => panic!("expected a runner to compose, got {other:?}"),
    }
}

fn unknown(r: Option<CampRunner>) -> String {
    match r {
        Some(CampRunner::Unknown(why)) => why,
        other => panic!("expected an unresolvable runner, got {other:?}"),
    }
}

#[test]
fn the_camp_runner_is_read_with_cargo_s_precedence() {
    let (root, mut env) = config_tree("precedence");
    let deep = root.join("a/b/.cargo/config.toml");
    let up = root.join(".cargo/config.toml");
    assert_eq!(lookup(&root, &env), None);

    // The deepest file wins outright; two arrays are not concatenated.
    std::fs::write(&up, format!("[target.{T}]\nrunner = [\"up\", \"1\"]\n")).unwrap();
    std::fs::write(&deep, format!("[target.{T}]\nrunner = [\"deep\", \"2\"]\n")).unwrap();
    assert_eq!(argv(lookup(&root, &env)), ["deep", "2"]);
    // The environment beats every file; a string splits on whitespace.
    env.insert("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER".into(), "envr  --flag".into());
    assert_eq!(argv(lookup(&root, &env)), ["envr", "--flag"]);
    env.remove("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER");

    // A triple key anywhere beats a deeper cfg(…) key.
    std::fs::write(&deep, "[target.'cfg(unix)']\nrunner = [\"cfg\"]\n").unwrap();
    assert_eq!(argv(lookup(&root, &env)), ["up", "1"]);
    // The same cfg(…) key in two files is one key: the deepest wins.
    std::fs::write(&up, "[target.'cfg(unix)']\nrunner = \"cfg-up\"\n").unwrap();
    std::fs::write(&deep, "[target.'cfg(unix)']\nrunner = [\"cfg-deep\"]\n").unwrap();
    assert!(unknown(lookup(&root, &env)).contains("cargo refuses"), "string in one file, array in another");
    std::fs::write(&up, "[target.'cfg(unix)']\nrunner = [\"cfg-up\"]\n").unwrap();
    assert_eq!(argv(lookup(&root, &env)), ["cfg-deep"]);
    // A cfg(…) that does not match is no runner.
    std::fs::write(&up, "").unwrap();
    std::fs::write(&deep, "[target.'cfg(windows)']\nrunner = [\"w\"]\n").unwrap();
    assert_eq!(lookup(&root, &env), None);
    // Two different cfg(…) tables that match: cargo refuses, so do we.
    std::fs::write(&deep, "[target.'cfg(unix)']\nrunner = [\"u\"]\n[target.'cfg(target_os = \"linux\")']\nrunner = [\"l\"]\n").unwrap();
    assert!(unknown(lookup(&root, &env)).contains("several"));
    // cfg(…) evaluated without the rustflags cargo would apply: undecidable.
    std::fs::write(&deep, "[target.'cfg(unix)']\nrunner = [\"u\"]\n").unwrap();
    env.insert("RUSTFLAGS".into(), "--cfg foo".into());
    assert!(unknown(lookup(&root, &env)).contains("RUSTFLAGS"));
    env.remove("RUSTFLAGS");
    // rustc's cfg unreadable: undecidable, not "no runner".
    let r = CargoConfig::load(&root.join("a/b"), &env).runner_for(T, || Err("no rustc".into()));
    assert!(unknown(r).contains("no rustc"));
    // A file this reader cannot vouch for makes every lookup undecidable.
    std::fs::write(&up, "include = \"other.toml\"\n").unwrap();
    assert!(unknown(lookup(&root, &env)).contains("include"));
    std::fs::write(&up, "[target\n").unwrap();
    assert!(unknown(lookup(&root, &env)).contains("cannot read"));
}

#[test]
fn relative_runner_paths_resolve_where_cargo_resolves_them() {
    let (root, mut env) = config_tree("relative");
    // A file's: against the parent of its .cargo directory.
    std::fs::write(root.join(".cargo/config.toml"), format!("[target.{T}]\nrunner = [\"./tools/r\", \"x\"]\n")).unwrap();
    assert_eq!(argv(lookup(&root, &env)), [root.join("./tools/r").display().to_string(), "x".into()]);
    // Bare names are PATH lookups and stay as written.
    std::fs::write(root.join(".cargo/config.toml"), format!("[target.{T}]\nrunner = \"qemu-aarch64 -L /x\"\n")).unwrap();
    assert_eq!(argv(lookup(&root, &env)), ["qemu-aarch64", "-L", "/x"]);
    // The environment's: against cargo's cwd.
    env.insert("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER".into(), "../r".into());
    assert_eq!(argv(lookup(&root, &env)), [root.join("a/b/../r").display().to_string()]);
}

#[test]
fn build_target_is_read_from_env_then_files() {
    let (root, mut env) = config_tree("build-target");
    std::fs::write(root.join(".cargo/config.toml"), "[build]\ntarget = [\"aarch64-unknown-linux-gnu\", \"x86_64-unknown-linux-musl\"]\n").unwrap();
    let c = || CargoConfig::load(&root.join("a/b"), &env).build_targets();
    assert_eq!(c(), ["aarch64-unknown-linux-gnu", "x86_64-unknown-linux-musl"]);
    env.insert("CARGO_BUILD_TARGET".into(), "riscv64gc-unknown-linux-gnu".into());
    assert_eq!(CargoConfig::load(&root.join("a/b"), &env).build_targets(), ["riscv64gc-unknown-linux-gnu"]);
}

/// The `runner` fixture copied to `<tag>/crate`, beside a logging camp runner
/// `<tag>/camp-runner.sh` that records its argv and then runs it.
fn camp_tree(tag: &str) -> (PathBuf, PathBuf) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("camp").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    let krate = root.join("crate");
    for dir in ["src", "tests"] {
        std::fs::create_dir_all(krate.join(dir)).unwrap();
    }
    for f in ["Cargo.toml", "Cargo.lock", "src/lib.rs", "tests/strict.rs"] {
        std::fs::copy(fixture("runner").join(f), krate.join(f)).unwrap();
    }
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    let log = root.join("argv.log");
    let sh = root.join("camp-runner.sh");
    std::fs::write(&sh, format!("#!/bin/sh\ntag=$1; shift\necho \"$tag $*\" >> '{}'\nexec \"$@\"\n", log.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
    (root, krate)
}

/// Discover, plan and run `cargo test --no-fail-fast` in `krate` with
/// `extra_env` on cargo, and check what the camp's runner saw.
fn composed_run(tag: &str, root: &Path, krate: &Path, extra_env: &[(&str, String)]) {
    let mut env: BTreeMap<String, String> = std::env::vars().collect();
    for (k, v) in extra_env {
        env.insert(k.to_string(), v.clone());
    }
    let mut req = request("test --no-fail-fast", tag);
    for (k, v) in extra_env {
        req.env_allowlist.insert(k.to_string(), v.clone());
    }
    let mut env_for_discover = env.clone();
    env_for_discover.extend(req.env_allowlist.clone());
    let opts = discover(&req, krate, &env_for_discover);
    assert_eq!(opts.no_harness, Ok(BTreeSet::from(["strict".to_string()])), "{tag}");
    assert!(
        matches!(opts.runners.get(&opts.host_triple), Some(CampRunner::Compose { argv, .. }) if argv[1] == tag),
        "{tag}: {opts:#?}"
    );
    let p = plan(&req, &opts);
    assert!(p.libtest_json, "{tag}: {p:#?}");
    let mut d = Demux::new();
    let exit = exec::run(&p, Some(krate), |s, l| d.push(s, l)).expect("cargo spawns");
    let text: Vec<&str> = d.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(exit, Exit::Code(0), "{tag}: {text:#?}");

    // The unittests binary ran under the camp's runner, flags and all, and
    // spoke libtest JSON.
    let kinds: Vec<_> = d.binaries.iter().map(|b| (b.kind, b.structured)).collect();
    assert_eq!(
        kinds,
        [(BinaryKind::Unittests, true), (BinaryKind::Test, false), (BinaryKind::Doctest, false)],
        "{tag}: {:#?}",
        d.binaries
    );
    assert!(d.binaries[0].events.iter().any(|e| matches!(e, TestEvent::SuiteFinished { ok: true, passed: 2, .. })));
    let log = std::fs::read_to_string(root.join("argv.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert!(lines.iter().any(|l| l.starts_with(&format!("{tag} ")) && l.contains("/deps/runner-") && l.ends_with(LIBTEST_FLAGS)), "{tag}: {log}");
    // The harness = false binary ran under it too, with no argument at all —
    // `strict` exits 2 on any (exit 0 above is the proof) …
    assert!(lines.iter().any(|l| l.contains("/deps/strict-") && !l.contains("-Z")), "{tag}: {log}");
    assert!(text.contains(&"strict harness ran"), "{tag}");
    // … and so did rustdoc's doctest binary, which passed both doctests.
    assert!(lines.iter().any(|l| l.ends_with("/rust_out")), "{tag}: {log}");
    assert!(text.iter().any(|l| l.starts_with("test result: ok. 2 passed")), "{tag}: {text:#?}");
    assert!(!text.iter().any(|l| l.contains("only accepted on the nightly compiler")), "{tag}");

    // MUST-NOT: never change a unit hash. A plain run afterwards (the camp's
    // runner still configured) builds nothing.
    let mut plain = request("test --no-run --message-format json", tag);
    plain.env_allowlist.extend(req.env_allowlist.clone());
    let pp = cargo_verdict::Plan { argv: plain.render(), request: plain, added: vec![], cargo_json: true, diagnostics_json: true, libtest_json: false, unstructured: vec![] };
    let mut d = Demux::new();
    exec::run(&pp, Some(krate), |s, l| d.push(s, l)).unwrap();
    let arts: Vec<bool> = d.messages.iter().filter_map(|m| match m { Message::CompilerArtifact(a) => Some(a.fresh), _ => None }).collect();
    assert!(!arts.is_empty() && arts.iter().all(|f| *f), "{tag}: {arts:?}");
}

#[test]
fn a_camp_runner_in_a_config_file_is_composed_not_replaced() {
    // Relative, with a slash: it only works if resolved against the parent of
    // .cargo — cargo runs test binaries from crate/, not from here.
    let (root, krate) = camp_tree("file");
    let host = host_triple().unwrap();
    std::fs::write(root.join(".cargo/config.toml"), format!("[target.{host}]\nrunner = [\"./camp-runner.sh\", \"file\"]\n")).unwrap();
    composed_run("file", &root, &krate, &[]);
}

#[test]
fn a_camp_runner_in_a_cfg_table_is_composed_not_replaced() {
    let (root, krate) = camp_tree("cfg");
    std::fs::write(root.join(".cargo/config.toml"), "[target.'cfg(all(unix, not(windows)))']\nrunner = \"./camp-runner.sh cfg\"\n").unwrap();
    composed_run("cfg", &root, &krate, &[]);
}

#[test]
fn a_camp_runner_in_the_environment_is_composed_not_replaced() {
    let (root, krate) = camp_tree("env");
    let host = host_triple().unwrap();
    let key = format!("CARGO_TARGET_{}_RUNNER", host.to_uppercase().replace(['-', '.'], "_"));
    composed_run("env", &root, &krate, &[(key.as_str(), format!("{} env", root.join("camp-runner.sh").display()))]);
}
