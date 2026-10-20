//! `parse(argv) -> Request`.
//!
//! Total: every argv parses. What the model does not name is kept verbatim
//! (`Request::passthrough`, `args`, `trailing`), so a parse never refuses a
//! line and a render never loses one.
//!
//! The one rule that makes that safe is **adjacency**. A flag this module does
//! not know might take a value, and if it does, splitting the two would hand
//! the value to the wrong owner on render (a test filter, say). So an unknown
//! flag swallows a directly following non-flag token as a pair, and the pair
//! renders adjacent. The cost of guessing wrong is under-modelling (a filter
//! kept as opaque passthrough), never a changed meaning.

use crate::request::{Ignored, Request};
use crate::verb::{Dialect, Verb};

/// Post-verb cargo flags that take no value and are not modelled.
const CARGO_BOOL_FLAGS: &[&str] = &[
    "-v", "-vv", "-vvv", "--verbose", "-q", "--quiet", "--locked", "--offline", "--frozen",
    "--keep-going", "--ignore-rust-version", "--future-incompat-report", "--timings",
    "--unit-graph", "--build-plan", "--open", "--no-deps", "--document-private-items",
    "--fix", "--allow-dirty", "--allow-staged", "--allow-no-vcs", "--broken-code",
    "--edition", "--edition-idioms", "--no-verify", "--dry-run", "--list", "--force",
    "-f", "--debug", "--no-track",
    // nextest's
    "--hide-progress-bar", "--fail-fast", "--ignore-default-filter", "--no-tests-fail",
];

/// Post-verb cargo flags that take a value and are not modelled. Unknown flags
/// are glued to a following non-flag token anyway; this table exists for the
/// values that *do* look like flags (`-j -1`).
const CARGO_VALUE_FLAGS: &[&str] = &[
    "-j", "--jobs", "--color", "--artifact-dir", "--out-dir", "--lockfile-path", "--registry",
    "--index", "--token", "--root", "--git", "--branch", "--tag", "--rev", "--path",
    "--version", "--vers", "--crate-type", "--print",
];

/// nextest's own value flags (only under `cargo nextest`). `--profile`/`-P` and
/// `--message-format` are here because under nextest they are nextest's, not
/// cargo's; cargo's profile is `--cargo-profile`.
const NEXTEST_VALUE_FLAGS: &[&str] = &[
    "-E", "--filterset", "--filter-expr", "-P", "--profile", "--partition", "--retries",
    "--max-fail", "--no-tests", "--status-level", "--final-status-level", "--failure-output",
    "--success-output", "--show-progress", "--archive-file", "--archive-format",
    "--tool-config-file", "--config-file", "--build-jobs", "--message-format",
    "--message-format-version", "--workspace-remap", "--extract-to", "--target-dir-remap",
    "--binaries-metadata", "--cargo-metadata",
];

/// libtest flags (after `--`) that take no value and are not modelled.
const LIBTEST_BOOL_FLAGS: &[&str] = &[
    "--show-output", "--list", "--quiet", "-q", "--report-time", "--ensure-time",
    "--shuffle", "--force-run-in-process", "--bench", "--test", "-h", "--help",
    "--exclude-should-panic",
];

/// libtest flags (after `--`) that take a value and are not modelled.
const LIBTEST_VALUE_FLAGS: &[&str] = &["--format", "--logfile", "-Z", "--color", "--shuffle-seed"];

/// Long flags whose `--name=value` spelling this module splits. Any other
/// `--x=y` is one opaque token.
const SPLIT_LONG: &[&str] = &[
    "--package", "--exclude", "--manifest-path", "--target-dir", "--features", "--profile",
    "--cargo-profile", "--target", "--message-format", "--config", "--bin", "--example",
    "--test", "--bench", "--test-threads", "--run-ignored", "--skip",
];

/// A positional is anything that is not a flag; a lone `-` is a positional.
fn is_positional(tok: &str) -> bool {
    !tok.starts_with('-') || tok == "-"
}

/// Split `--name=value` (for [`SPLIT_LONG`] names) and attached short values
/// (`-pfoo`, `-p=foo`) into name and inline value.
fn split<'a>(tok: &'a str, shorts: &[&'static str]) -> (&'a str, Option<&'a str>) {
    if tok.starts_with("--") {
        if let Some(eq) = tok.find('=') {
            if SPLIT_LONG.contains(&&tok[..eq]) {
                return (&tok[..eq], Some(&tok[eq + 1..]));
            }
        }
        return (tok, None);
    }
    for s in shorts {
        if tok.len() > s.len() && tok.starts_with(s) {
            let v = &tok[s.len()..];
            return (&tok[..s.len()], Some(v.strip_prefix('=').unwrap_or(v)));
        }
    }
    (tok, None)
}

/// A cursor over the argv.
struct Cursor<'a> {
    toks: Vec<&'a str>,
    i: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<&'a str> {
        self.toks.get(self.i).copied()
    }
    fn next(&mut self) -> Option<&'a str> {
        let t = self.peek()?;
        self.i += 1;
        Some(t)
    }
    fn rest(&mut self) -> Vec<String> {
        let r = self.toks[self.i..].iter().map(|s| s.to_string()).collect();
        self.i = self.toks.len();
        r
    }
    /// A required value: inline, else the next token unless it is `--` (which
    /// always ends cargo's own args).
    fn value(&mut self, inline: Option<&'a str>) -> Option<String> {
        if let Some(v) = inline {
            return Some(v.to_string());
        }
        match self.peek() {
            Some(t) if t != "--" => {
                self.i += 1;
                Some(t.to_string())
            }
            _ => None,
        }
    }
    /// An optional value (`--bin [NAME]`): inline, else the next token only if
    /// it is a positional.
    fn optional_value(&mut self, inline: Option<&'a str>) -> Option<String> {
        if let Some(v) = inline {
            return Some(v.to_string());
        }
        match self.peek() {
            Some(t) if is_positional(t) => {
                self.i += 1;
                Some(t.to_string())
            }
            _ => None,
        }
    }
}

/// Parse cargo's arguments — the argv *after* the `cargo` program name — into
/// a [`Request`].
pub fn parse<S: AsRef<str>>(args: &[S]) -> Request {
    let mut c = Cursor { toks: args.iter().map(AsRef::as_ref).collect(), i: 0 };
    let mut r = Request::new(Verb::Help);
    let verb = parse_globals(&mut c, &mut r);
    let Some(verb) = verb else {
        // `cargo`, `cargo -h`, `cargo -v`: cargo prints its help. The help flag
        // is implied by the verb, so it is not also recorded.
        r.help = false;
        return r;
    };
    r.verb = verb;
    match r.verb {
        Verb::Version | Verb::List | Verb::Explain => {
            r.args = c.rest();
            return r;
        }
        Verb::Nextest(_) => {
            if let Some(t) = c.peek().filter(|t| is_positional(t)) {
                c.i += 1;
                let sub = if t == "r" { "run" } else { t };
                r.verb = Verb::Nextest(Some(sub.to_string()));
            }
        }
        _ => {}
    }
    match r.verb.dialect() {
        Dialect::Opaque => r.args = c.rest(),
        Dialect::Build => parse_cargo_args(&mut c, &mut r, false),
        Dialect::Test => parse_cargo_args(&mut c, &mut r, true),
    }
    if r.verb == Verb::Help {
        // `cargo -h help` asks for help once.
        r.help = false;
    }
    r
}

/// Everything before the verb. Returns the verb, or `None` if there is none.
fn parse_globals<'a>(c: &mut Cursor<'a>, r: &mut Request) -> Option<Verb> {
    while let Some(tok) = c.next() {
        if let Some(tc) = tok.strip_prefix('+') {
            // rustup reads a toolchain only in argv[1]; the old recognizer
            // skipped one anywhere before the verb, and so does this, taking
            // the first as the toolchain.
            if r.toolchain.is_none() {
                r.toolchain = Some(tc.to_string());
            } else {
                r.globals.passthrough.push(tok.to_string());
            }
            continue;
        }
        if is_positional(tok) {
            return Some(Verb::from_word(tok));
        }
        let (name, inline) = split(tok, &["-Z", "-C"]);
        match name {
            "-V" | "--version" => return Some(Verb::Version),
            "--list" => return Some(Verb::List),
            "--explain" => return Some(Verb::Explain),
            "-h" | "--help" => r.help = true,
            "--config" | "-Z" | "-C" | "--color" => match c.value(inline) {
                Some(v) => match name {
                    "--config" => r.globals.config.push(v),
                    "-Z" => r.globals.unstable.push(v),
                    "-C" => r.globals.directory = Some(v),
                    _ => r.globals.passthrough.extend([tok.to_string(), v]),
                },
                None => r.globals.passthrough.push(tok.to_string()),
            },
            _ => r.globals.passthrough.push(tok.to_string()),
        }
    }
    None
}

/// The flags after a build- or test-dialect verb, up to and including `--`.
fn parse_cargo_args<'a>(c: &mut Cursor<'a>, r: &mut Request, test: bool) {
    let nextest = r.verb.is_nextest();
    while let Some(tok) = c.next() {
        if tok == "--" {
            if test {
                parse_harness(c, r);
            } else {
                r.trailing = c.rest();
            }
            return;
        }
        if is_positional(tok) {
            if test {
                r.test.filters.push(tok.to_string());
            } else {
                r.args.push(tok.to_string());
            }
            continue;
        }
        let (name, inline) = split(tok, &["-p", "-F", "-Z"]);
        // Each arm either models the flag or falls through to `unmodelled`.
        let modelled = match name {
            "-h" | "--help" => {
                r.help = true;
                true
            }
            "--workspace" | "--all" => {
                r.selection.workspace = true;
                true
            }
            "--lib" => set(&mut r.targets.lib),
            "--bins" => set(&mut r.targets.bins),
            "--examples" => set(&mut r.targets.examples),
            "--tests" => set(&mut r.targets.tests),
            "--benches" => set(&mut r.targets.benches),
            "--all-targets" => set(&mut r.targets.all_targets),
            "--doc" => set(&mut r.targets.doc),
            "--all-features" => set(&mut r.features.all),
            "--no-default-features" => set(&mut r.features.no_default),
            "--release" | "-r" => {
                r.profile = Some("release".to_string());
                true
            }
            "--no-run" if test => set(&mut r.test.no_run),
            "--no-fail-fast" if test => {
                r.test.fail_fast = false;
                true
            }
            "--no-capture" | "--nocapture" if nextest && test => set(&mut r.test.nocapture),
            "--bin" | "--example" | "--test" | "--bench" => match c.optional_value(inline) {
                Some(v) => {
                    let list = match name {
                        "--bin" => &mut r.targets.bin,
                        "--example" => &mut r.targets.example,
                        "--test" => &mut r.targets.test,
                        _ => &mut r.targets.bench,
                    };
                    list.push(v);
                    true
                }
                None => false,
            },
            "--test-threads" | "-j" if nextest && test => {
                let save = c.i;
                match c.value(inline).and_then(|v| v.parse::<u32>().ok()) {
                    Some(n) => {
                        r.test.threads = Some(n);
                        true
                    }
                    None => {
                        c.i = save;
                        false
                    }
                }
            }
            "--run-ignored" if nextest && test => {
                let save = c.i;
                let which = match c.value(inline).as_deref() {
                    Some("default") => Some(Ignored::Exclude),
                    Some("all") => Some(Ignored::Include),
                    Some("only") => Some(Ignored::Only),
                    _ => None,
                };
                match which {
                    Some(w) => {
                        r.test.ignored = w;
                        true
                    }
                    None => {
                        c.i = save;
                        false
                    }
                }
            }
            "--profile" | "--message-format" if nextest => false,
            "-p" | "--package" | "--exclude" | "--manifest-path" | "--target-dir" | "--features"
            | "-F" | "--profile" | "--cargo-profile" | "--target" | "--message-format"
            | "--config" | "-Z" => {
                if name == "--cargo-profile" && !nextest {
                    false
                } else {
                    match c.value(inline) {
                        Some(v) => {
                            match name {
                                "-p" | "--package" => r.selection.packages.push(v),
                                "--exclude" => r.selection.exclude.push(v),
                                "--manifest-path" => r.manifest_path = Some(v),
                                "--target-dir" => r.target_dir = Some(v),
                                "--features" | "-F" => r.features.list.extend(
                                    v.split([',', ' '])
                                        .filter(|f| !f.is_empty())
                                        .map(str::to_string),
                                ),
                                "--profile" | "--cargo-profile" => r.profile = Some(v),
                                "--target" => r.target_triples.push(v),
                                "--message-format" => r.message_format.push(v),
                                "--config" => r.config.push(v),
                                _ => r.unstable.push(v),
                            }
                            true
                        }
                        None => false,
                    }
                }
            }
            _ => false,
        };
        if !modelled {
            unmodelled(c, &mut r.passthrough, tok, nextest);
        }
    }
}

fn set(flag: &mut bool) -> bool {
    *flag = true;
    true
}

/// Keep an unmodelled cargo flag verbatim, with its value if it has one.
fn unmodelled(c: &mut Cursor<'_>, out: &mut Vec<String>, tok: &str, nextest: bool) {
    out.push(tok.to_string());
    if self_contained(tok) {
        return;
    }
    let known_value = CARGO_VALUE_FLAGS.contains(&tok)
        || (nextest && NEXTEST_VALUE_FLAGS.contains(&tok))
        // A modelled value flag that failed to take its value (`--bin -v`
        // lists targets; `-p --` has none) stays a lone token.
        || SPLIT_LONG.contains(&tok)
        || matches!(tok, "-p" | "-F" | "-Z");
    if known_value {
        if !matches!(tok, "--bin" | "--example" | "--test" | "--bench") {
            if let Some(v) = c.value(None) {
                out.push(v);
            }
        }
        return;
    }
    if CARGO_BOOL_FLAGS.contains(&tok) {
        return;
    }
    glue(c, out);
}

/// A flag token that cannot be waiting for a separate value: `--name=value`,
/// or a short flag with something attached (`-j4`, `-Zflag`, `-vv`). A short
/// *cluster* ending in a value flag (`-vp foo`) is misread by this, and is
/// rare enough to accept.
fn self_contained(tok: &str) -> bool {
    if tok.starts_with("--") {
        tok.contains('=')
    } else {
        tok.len() > 2
    }
}

/// The adjacency rule: an unknown flag takes a directly following positional
/// with it.
fn glue(c: &mut Cursor<'_>, out: &mut Vec<String>) {
    if let Some(t) = c.peek().filter(|t| is_positional(t)) {
        c.i += 1;
        out.push(t.to_string());
    }
}

/// libtest's args, after `--`.
fn parse_harness(c: &mut Cursor<'_>, r: &mut Request) {
    while let Some(tok) = c.next() {
        if tok == "--" {
            // A second `--` ends libtest's own options; keep the rest verbatim.
            r.trailing.push(tok.to_string());
            r.trailing.extend(c.rest());
            return;
        }
        if is_positional(tok) {
            r.test.filters.push(tok.to_string());
            continue;
        }
        let (name, inline) = split(tok, &[]);
        let modelled = match name {
            "--exact" => set(&mut r.test.exact),
            "--include-ignored" => {
                r.test.ignored = Ignored::Include;
                true
            }
            "--ignored" => {
                r.test.ignored = Ignored::Only;
                true
            }
            "--nocapture" | "--no-capture" => set(&mut r.test.nocapture),
            "--skip" => match c.value(inline) {
                Some(v) => {
                    r.test.skip.push(v);
                    true
                }
                None => false,
            },
            "--test-threads" => {
                let save = c.i;
                match c.value(inline).and_then(|v| v.parse::<u32>().ok()) {
                    Some(n) => {
                        r.test.threads = Some(n);
                        true
                    }
                    None => {
                        c.i = save;
                        false
                    }
                }
            }
            _ => false,
        };
        if modelled {
            continue;
        }
        r.trailing.push(tok.to_string());
        if self_contained(tok) || LIBTEST_BOOL_FLAGS.contains(&tok) {
            continue;
        }
        if LIBTEST_VALUE_FLAGS.contains(&tok) || matches!(tok, "--skip" | "--test-threads") {
            if let Some(v) = c.value(None) {
                r.trailing.push(v);
            }
            continue;
        }
        glue(c, &mut r.trailing);
    }
}
