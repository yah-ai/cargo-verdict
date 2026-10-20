//! The parse corpus (R989-F19).
//!
//! Three sources, each a section below:
//!
//! 1. every cargo argv the old recognizer's tests cover
//!    (`build_skew_stamp::from_args`, crates/yah/agent-tools at the time of
//!    writing; R989-T4 moves it into crates/yah/rust-plugin), because `parse`
//!    replaces it (R989-F22) and must classify everything it classified;
//! 2. W323 §3's misses: `doc`, `bench`, `nextest`, and `+toolchain`;
//! 3. R989-B7's query spellings.
//!
//! Every row asserts the class and the normalized request, the latter as its
//! canonical rendering — which pins the whole `Request`, because a render
//! reparses to the request it came from (asserted in every row too).

use cargo_verdict::{parse, render, Class, Ignored, Request, Verb};

fn argv(line: &str) -> Vec<&str> {
    line.split_whitespace().collect()
}

/// Parse `line`, assert its class and canonical rendering, and that the
/// rendering reparses to the same request. Returns the request for any
/// further field-level asserts.
#[track_caller]
fn row(line: &str, class: Class, canonical: &str) -> Request {
    let r = parse(&argv(line));
    assert_eq!(r.class(), class, "class of `cargo {line}`: {r:#?}");
    let rendered = render(&r);
    assert_eq!(rendered.join(" "), canonical, "canonical form of `cargo {line}`");
    assert_eq!(parse(&rendered), r, "`cargo {line}` does not survive render → parse");
    r
}

// ─── 1. The old recognizer's corpus ───────────────────────────────────────────
//
// The recognizer returned `Some(CargoBuild)` for build/test/check/clippy (and
// their aliases) and for the `xtask` alias; those are exactly the requests
// whose class is Verdict, plus `Verb::Xtask`. Its `None`s are every other
// class. Its four fields map to `verb`, `packages_in_scope()`, `manifest_path`
// and `profile`/`profile_dir()`; `cwd` came from the shell, not argv, and
// stays the Bash door's to carry.

/// What the old recognizer admitted, expressed over a `Request`. Kept here so
/// the equivalence is stated once; R989-F22 owns the real policy.
fn recognizer_would_admit(r: &Request) -> bool {
    matches!(r.verb, Verb::Build | Verb::Test | Verb::Check | Verb::Clippy | Verb::Xtask)
        && matches!(r.class(), Class::Verdict | Class::Tenancy)
}

#[test]
fn a_plain_cargo_test_is_recognised() {
    let r = row("test -p yah --lib", Class::Verdict, "test --package yah --lib");
    assert_eq!(r.verb, Verb::Test);
    assert_eq!(r.packages_in_scope(), ["yah"]);
    assert!(r.targets.lib);
}

#[test]
fn subcommand_aliases_normalize() {
    assert_eq!(row("c -p yah", Class::Verdict, "check --package yah").verb, Verb::Check);
    assert_eq!(row("b", Class::Verdict, "build").verb, Verb::Build);
    assert_eq!(row("t", Class::Verdict, "test").verb, Verb::Test);
    assert_eq!(row("clippy", Class::Verdict, "clippy").verb, Verb::Clippy);
    // Beyond the recognizer's set: cargo's other built-in aliases.
    assert_eq!(row("d", Class::Verdict, "doc").verb, Verb::Doc);
    assert_eq!(row("r", Class::Tenancy, "run").verb, Verb::Run);
    assert_eq!(row("rm serde", Class::Mutation, "remove serde").verb, Verb::Remove);
}

#[test]
fn every_package_spelling_is_collected() {
    let r = row(
        "check -p a --package b --package=c -pd -p=e",
        Class::Verdict,
        "check --package a --package b --package c --package d --package e",
    );
    assert_eq!(r.packages_in_scope(), ["a", "b", "c", "d", "e"]);
}

#[test]
fn a_toolchain_selector_is_not_the_subcommand() {
    let r = row("+nightly clippy -p yah", Class::Verdict, "+nightly clippy --package yah");
    assert_eq!(r.verb, Verb::Clippy);
    assert_eq!(r.toolchain.as_deref(), Some("nightly"));
    assert_eq!(r.packages_in_scope(), ["yah"]);
}

#[test]
fn a_global_flag_value_is_not_the_subcommand() {
    let r = row("--color never test -p yah", Class::Verdict, "--color never test --package yah");
    assert_eq!(r.verb, Verb::Test);
    assert_eq!(r.globals.passthrough, ["--color", "never"]);
    let r = row(
        "--config k=v -Z flag -C oss/qed check",
        Class::Verdict,
        "-C oss/qed --config k=v -Z flag check",
    );
    assert_eq!(r.globals.directory.as_deref(), Some("oss/qed"));
    assert_eq!(r.globals.config, ["k=v"]);
    assert_eq!(r.globals.unstable, ["flag"]);
}

#[test]
fn the_wrapped_and_compound_spellings_reduce_to_these_argvs() {
    // The recognizer's wrapper/compound tests (`timeout 600 cargo …`,
    // `cargo … 2>&1 | tail`, `(cd … && cargo clippy …)`) are bash_ast's half;
    // what reaches `parse` is the cargo argv they peel down to.
    for (line, verb) in [
        ("test -p yah", Verb::Test),
        ("test -p yah --lib", Verb::Test),
        ("check -p yah", Verb::Check),
        ("build -p yah", Verb::Build),
        ("clippy -p yah", Verb::Clippy),
    ] {
        let r = parse(&argv(line));
        assert_eq!(r.verb, verb, "{line}");
        assert_eq!(r.packages_in_scope(), ["yah"], "{line}");
        assert_eq!(r.class(), Class::Verdict, "{line}");
        assert!(recognizer_would_admit(&r), "{line}");
    }
    // `a_list_takes_the_first_build_not_the_last`: the first argv is board's.
    assert_eq!(parse(&argv("check -p board")).packages_in_scope(), ["board"]);
}

#[test]
fn what_the_recognizer_refused_is_classed_not_dropped() {
    // Its negative controls. None of these is a verdict, and none is admitted;
    // each now says *what* it is.
    for (line, class, canonical) in [
        ("run -p yah", Class::Tenancy, "run --package yah"),
        ("fmt", Class::Mutation, "fmt"),
        ("add serde", Class::Mutation, "add serde"),
        ("install cargo-nextest", Class::Mutation, "install cargo-nextest"),
        ("", Class::Query, ""),
    ] {
        let r = row(line, class, canonical);
        assert!(!recognizer_would_admit(&r), "`cargo {line}` was refused by the recognizer");
    }
}

#[test]
fn a_workspace_build_takes_the_whole_tree() {
    assert!(row("test --workspace", Class::Verdict, "test --workspace")
        .packages_in_scope()
        .is_empty());
    // `--all` is `--workspace`'s deprecated spelling. The `-p` is kept, not
    // dropped — a render never loses input — but is out of scope.
    let r = row("build --all -p ignored", Class::Verdict, "build --workspace --package ignored");
    assert!(r.packages_in_scope().is_empty());
    assert_eq!(r.selection.packages, ["ignored"]);
    assert!(row("test", Class::Verdict, "test").packages_in_scope().is_empty());
}

#[test]
fn harness_args_after_a_double_dash_are_the_harness_s() {
    let r = row(
        "test -p yah -- --nocapture -p bogus",
        Class::Verdict,
        "test --package yah -- --nocapture -p bogus",
    );
    assert_eq!(r.packages_in_scope(), ["yah"]);
    assert!(r.test.nocapture);
    // libtest has no `-p`: it is kept verbatim with the token it may own.
    assert_eq!(r.trailing, ["-p", "bogus"]);
    assert!(r.test.filters.is_empty());
}

#[test]
fn the_xtask_alias_is_admitted_and_its_tail_is_the_xtask_binary_s() {
    for line in [
        "xtask install",
        "xtask install --dest /Applications/yah.app/Contents/MacOS/yah",
        "xtask install --profile debug",
    ] {
        let r = row(line, Class::Tenancy, line);
        assert_eq!(r.verb, Verb::Xtask, "{line}");
        assert!(recognizer_would_admit(&r), "{line}");
        // `xtask = "run -p xtask --"`: cargo builds xtask on the dev profile
        // whatever follows. The recognizer read `--profile debug` as cargo's;
        // it is the xtask binary's argument, and the lane is `debug` either way.
        assert_eq!(r.profile, None, "{line}");
        assert_eq!(r.profile_dir(), "debug", "{line}");
    }
    // `cargo xtask --help` is the xtask binary's help, after a build.
    assert_eq!(parse(&argv("xtask --help")).class(), Class::Tenancy);
}

#[test]
fn manifest_path_and_profile_say_which_lock() {
    // R719-B11 / R719-B13's shapes.
    let r = row(
        "build --manifest-path oss/qed/Cargo.toml --release",
        Class::Verdict,
        "build --manifest-path oss/qed/Cargo.toml --profile release",
    );
    assert_eq!(r.manifest_path.as_deref(), Some("oss/qed/Cargo.toml"));
    assert_eq!(r.profile_dir(), "release");
    let r = row(
        "check --manifest-path=oss/qed/Cargo.toml --profile=ci-dev",
        Class::Verdict,
        "check --manifest-path oss/qed/Cargo.toml --profile ci-dev",
    );
    assert_eq!(r.profile_dir(), "ci-dev");
    for (line, dir) in [
        ("test", "debug"),
        ("build -r", "release"),
        ("build --profile dev", "debug"),
        ("test --profile test", "debug"),
        ("build --profile bench", "release"),
        ("build --profile release-dev", "release-dev"),
    ] {
        assert_eq!(parse(&argv(line)).profile_dir(), dir, "{line}");
    }
}

// ─── 2. W323 §3's misses ──────────────────────────────────────────────────────

#[test]
fn doc_and_bench_are_verdicts() {
    let r = row("doc -p x --no-deps --open", Class::Verdict, "doc --package x --no-deps --open");
    assert_eq!(r.verb, Verb::Doc);
    assert_eq!(r.passthrough, ["--no-deps", "--open"]);
    let r = row("bench -p x parse", Class::Verdict, "bench --package x -- parse");
    assert_eq!(r.verb, Verb::Bench);
    assert_eq!(r.test.filters, ["parse"]);
    // cargo's `bench` profile inherits `release`.
    assert_eq!(r.profile_dir(), "release");
    row("bench --no-run", Class::Verdict, "bench --no-run");
}

#[test]
fn nextest_is_a_verdict_with_its_own_dialect() {
    let r = row("nextest run -p x", Class::Verdict, "nextest run --package x");
    assert_eq!(r.verb, Verb::Nextest(Some("run".into())));
    assert_eq!(r.profile_dir(), "debug");
    assert_eq!(row("nextest r", Class::Verdict, "nextest run").verb, Verb::Nextest(Some("run".into())));

    // `--profile` / `-P` / `--message-format` are nextest's own; cargo's
    // profile is `--cargo-profile`; `-j` is test threads, not build jobs.
    let r = row(
        "nextest run --profile ci -E test(foo) --cargo-profile ci-dev -j 4 --no-capture --run-ignored all --no-fail-fast foo -- --exact",
        Class::Verdict,
        "nextest run --cargo-profile ci-dev --no-fail-fast --test-threads=4 --no-capture --run-ignored=all --profile ci -E test(foo) -- foo --exact",
    );
    assert_eq!(r.profile.as_deref(), Some("ci-dev"));
    assert_eq!(r.test.threads, Some(4));
    assert!(r.test.nocapture && r.test.exact && !r.test.fail_fast);
    assert_eq!(r.test.ignored, Ignored::Include);
    assert_eq!(r.test.filters, ["foo"]);
    assert_eq!(r.passthrough, ["--profile", "ci", "-E", "test(foo)"]);

    row("nextest list --workspace", Class::Verdict, "nextest list --workspace");
    row("nextest archive --archive-file a.tar.zst", Class::Verdict, "nextest archive --archive-file a.tar.zst");
    row("nextest", Class::Query, "nextest");
    row("nextest --help", Class::Query, "nextest --help");
    row("nextest run --help", Class::Query, "nextest run --help");
    row("nextest show-config test-groups", Class::Query, "nextest show-config test-groups");
    row("nextest self update", Class::Mutation, "nextest self update");
}

#[test]
fn the_toolchain_is_part_of_the_request() {
    // W323 §3: dropped, `cargo +nightly test` and `cargo test` would share a
    // result the moment dedup lands.
    let nightly = row("+nightly test -p x", Class::Verdict, "+nightly test --package x");
    let stable = parse(&argv("test -p x"));
    assert_ne!(nightly, stable);
    assert_eq!(nightly.toolchain.as_deref(), Some("nightly"));
    row("+1.85.0 check", Class::Verdict, "+1.85.0 check");
}

// ─── 3. R989-B7's query spellings ─────────────────────────────────────────────

#[test]
fn query_spellings_are_queries() {
    for (line, canonical) in [
        ("build --help", "build --help"),
        ("build -h -p x", "build --package x --help"),
        ("test -h", "test --help"),
        ("help build", "help build"),
        ("help", ""),
        ("-h", ""),
        ("--help", ""),
        ("metadata --format-version 1", "metadata --format-version 1"),
        ("metadata --no-deps --format-version=1", "metadata --no-deps --format-version=1"),
        ("tree -i foo", "tree -i foo"),
        ("tree -e features", "tree -e features"),
        ("--version", "version"),
        ("-V", "version"),
        ("version", "version"),
        ("pkgid", "pkgid"),
        ("locate-project --workspace", "locate-project --workspace"),
        ("--list", "--list"),
        ("--explain E0308", "--explain E0308"),
        ("fmt --help", "fmt --help"),
        ("add -h", "add -h"),
        ("+nightly --version", "+nightly version"),
    ] {
        row(line, Class::Query, canonical);
    }
}

#[test]
fn a_help_after_the_double_dash_is_the_binary_s_not_cargo_s() {
    // These build first; the help goes to the test binary / the program.
    row("test -- --help", Class::Verdict, "test -- --help");
    row("run -- --help", Class::Tenancy, "run -- --help");
    row("fmt -- --help", Class::Mutation, "fmt -- --help");
}

// ─── Beyond the three sources: the shapes later phases depend on ─────────────

#[test]
fn the_scoped_runner_probe_line_parses() {
    // W371 §4.4(2), the line R989-F20's plan generates.
    let r = row(
        r#"test -v --config target.aarch64-apple-darwin.runner=["env","RUSTC_BOOTSTRAP=1"] -- -Z unstable-options --format json"#,
        Class::Verdict,
        r#"test --config target.aarch64-apple-darwin.runner=["env","RUSTC_BOOTSTRAP=1"] -v -- -Z unstable-options --format json"#,
    );
    assert_eq!(r.config, [r#"target.aarch64-apple-darwin.runner=["env","RUSTC_BOOTSTRAP=1"]"#]);
    assert_eq!(r.trailing, ["-Z", "unstable-options", "--format", "json"]);
    assert!(r.test.filters.is_empty(), "`json` is --format's value, not a filter");
}

#[test]
fn test_selection_is_modelled() {
    let r = row(
        "test -p x --test integration --no-fail-fast parse -- --exact --skip slow --include-ignored --test-threads 2 --show-output",
        Class::Verdict,
        "test --package x --test integration --no-fail-fast -- parse --exact --skip slow --include-ignored --test-threads=2 --show-output",
    );
    assert_eq!(r.targets.test, ["integration"]);
    assert_eq!(r.test.filters, ["parse"]);
    assert_eq!(r.test.skip, ["slow"]);
    assert!(r.test.exact && !r.test.fail_fast);
    assert_eq!(r.test.ignored, Ignored::Include);
    assert_eq!(r.test.threads, Some(2));
    assert_eq!(r.trailing, ["--show-output"]);
    let r = parse(&argv("test -- --ignored foo bar --test-threads=8 --nocapture"));
    assert_eq!(r.test.ignored, Ignored::Only);
    assert_eq!(r.test.filters, ["foo", "bar"]);
    assert_eq!(r.test.threads, Some(8));
    assert!(r.test.nocapture);
}

#[test]
fn features_targets_and_triples() {
    let r = row(
        "build --features a,b -F c --features=d --all-features --no-default-features --bins --bin x --example e --all-targets --target wasm32-unknown-unknown --target-dir t --message-format json --exclude z --workspace",
        Class::Verdict,
        "build --target-dir t --workspace --exclude z --bins --bin x --example e --all-targets --all-features --no-default-features --features a,b,c,d --target wasm32-unknown-unknown --message-format json",
    );
    assert_eq!(r.features.list, ["a", "b", "c", "d"]);
    assert_eq!(r.target_triples, ["wasm32-unknown-unknown"]);
    assert_eq!(r.target_dir.as_deref(), Some("t"));
    // A bare `--bin` lists the binaries; it stays verbatim.
    row("run --bin", Class::Tenancy, "run --bin");
}

#[test]
fn clippy_fix_mutates_and_clippy_lints_are_trailing() {
    row("clippy --fix --allow-dirty", Class::Mutation, "clippy --fix --allow-dirty");
    let r = row("clippy -p x -- -D warnings", Class::Verdict, "clippy --package x -- -D warnings");
    assert_eq!(r.trailing, ["-D", "warnings"]);
}

#[test]
fn unknown_flags_keep_their_values_and_unknown_verbs_are_unknown() {
    // `--frobnicate` is unknown; `foo` might be its value, so it is not a filter.
    let r = row("test --frobnicate foo", Class::Verdict, "test --frobnicate foo");
    assert!(r.test.filters.is_empty());
    assert_eq!(r.passthrough, ["--frobnicate", "foo"]);
    // `-j` is known to take a value, even one that looks like a flag.
    assert_eq!(parse(&argv("build -j -1")).passthrough, ["-j", "-1"]);
    // Attached short values are self-contained: `foo` is a filter.
    assert_eq!(parse(&argv("test -j4 foo")).test.filters, ["foo"]);
    row("udeps --all-targets", Class::Unknown, "udeps --all-targets");
    row("udeps --help", Class::Query, "udeps --help");
}

#[test]
fn every_class_has_its_verbs() {
    for (line, class) in [
        ("rustc -- -C opt-level=3", Class::Verdict),
        ("rustdoc", Class::Verdict),
        ("package --allow-dirty", Class::Verdict),
        ("watch -x check", Class::Tenancy),
        ("fix --edition", Class::Mutation),
        ("update -p serde", Class::Mutation),
        ("clean -p x --release", Class::Mutation),
        ("vendor", Class::Mutation),
        ("generate-lockfile", Class::Mutation),
        ("publish --dry-run", Class::Mutation),
        ("new foo", Class::Mutation),
        ("search serde", Class::Query),
        ("info serde", Class::Query),
        ("read-manifest", Class::Query),
        ("verify-project", Class::Query),
        ("report future-incompatibilities", Class::Query),
        ("config get", Class::Query),
    ] {
        let r = parse(&argv(line));
        assert_eq!(r.class(), class, "{line}");
        assert_eq!(parse(&render(&r)), r, "{line}");
    }
}

#[test]
fn env_is_not_argv() {
    let mut r = parse(&argv("test -p x"));
    r.env_allowlist.insert("RUST_LOG".into(), "debug".into());
    assert_eq!(render(&r).join(" "), "test --package x");
}
