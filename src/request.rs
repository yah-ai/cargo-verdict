//! The request model: one cargo invocation, normalized.
//!
//! A [`Request`] is what both doors agree a cargo line *means*. It is built by
//! [`crate::parse`] from an argv and turned back into one by [`crate::render`];
//! `parse(render(parse(x))) == parse(x)` holds for every argv (tests/roundtrip.rs).
//!
//! **Coverage gaps never block structure.** A flag this model does not name is
//! kept verbatim in a passthrough list, in order, so it still reaches cargo and
//! still enters any key built from the request — as opaque bytes. That is the
//! `jbr/cargo-mcp` failure designed out (W323 §2: its `CargoBuild` had no
//! `--features`, so a whole class of builds could not be expressed at all).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::verb::{Class, Dialect, Verb};

/// One cargo invocation. Field order is render order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Request {
    /// `+nightly` — the rustup toolchain selector. W323 §3: dropping this is a
    /// correctness bug the moment results are shared, because `cargo +nightly
    /// test` and `cargo test` are different verdicts.
    pub toolchain: Option<String>,
    /// Flags written *before* the verb. Kept apart from the post-verb ones
    /// because for an external subcommand the position is the meaning.
    pub globals: Globals,
    pub verb: Verb,
    /// `-h` / `--help` given to cargo itself (never one after `--`, which the
    /// binary or harness receives). Makes the request a [`Class::Query`].
    pub help: bool,
    /// `--manifest-path`, verbatim. Says *which workspace*, so which `target/`
    /// a build contends on (R719-B11).
    pub manifest_path: Option<String>,
    /// `--target-dir`, verbatim.
    pub target_dir: Option<String>,
    pub selection: Selection,
    pub targets: Targets,
    pub features: Features,
    /// `--profile NAME`; `--release` / `-r` normalize to `release`; nextest's
    /// `--cargo-profile` lands here too (its own `--profile` is not cargo's).
    pub profile: Option<String>,
    /// `--target TRIPLE`, each occurrence. Named for what it holds, because
    /// [`Targets`] already means cargo's `--lib`/`--bin` target selection.
    pub target_triples: Vec<String>,
    /// `--message-format`, each occurrence. Not nextest's flag of that name.
    pub message_format: Vec<String>,
    /// Post-verb `--config KEY=VALUE`, each occurrence.
    pub config: Vec<String>,
    /// Post-verb `-Z FLAG`, each occurrence.
    pub unstable: Vec<String>,
    pub test: TestArgs,
    /// Environment the request carries into cargo's process. An argv carries
    /// none, so `parse` leaves this empty and `render` never emits it; the typed
    /// door fills it, and it is part of what a dedup key must cover (W371 §4.2).
    pub env_allowlist: BTreeMap<String, String>,
    /// Operands. For a build-dialect verb, the positionals cargo itself takes
    /// (`cargo install ripgrep`); for an opaque verb, *everything* after the
    /// verb, verbatim (`cargo fmt --check`, `cargo xtask install …`).
    pub args: Vec<String>,
    /// Post-verb cargo flags this model does not name, verbatim and in order.
    /// A flag and the value it takes stay adjacent.
    pub passthrough: Vec<String>,
    /// After `--`, what this model does not name, verbatim and in order:
    /// rustc's args for `cargo rustc`, the program's for `cargo run`, the
    /// unmodelled libtest args for `cargo test`. Rendered after the modelled
    /// harness args, so the relative order of the opaque ones never changes.
    pub trailing: Vec<String>,
}

/// Flags before the verb: `cargo -C dir --config k=v +nightly -Z x -v build`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Globals {
    /// `-C DIR` (unstable): change directory before doing anything.
    pub directory: Option<String>,
    pub config: Vec<String>,
    pub unstable: Vec<String>,
    /// Everything else before the verb (`-v`, `--locked`, `--color never`).
    pub passthrough: Vec<String>,
}

/// Which packages: `-p`/`--package` each occurrence, `--workspace` (or its
/// deprecated spelling `--all`), `--exclude` each occurrence.
///
/// Kept faithful rather than collapsed — `--workspace -p x` keeps both — so a
/// render never drops something the user wrote. [`Request::packages_in_scope`]
/// is the collapsed view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Selection {
    pub workspace: bool,
    pub packages: Vec<String>,
    pub exclude: Vec<String>,
}

/// cargo's target selection. A named list (`bin`) is `--bin NAME` each
/// occurrence; the plural flag (`bins`) is `--bins`. A bare `--bin` with no
/// name lists the targets instead and is kept in passthrough.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Targets {
    pub lib: bool,
    pub bins: bool,
    pub bin: Vec<String>,
    pub examples: bool,
    pub example: Vec<String>,
    pub tests: bool,
    pub test: Vec<String>,
    pub benches: bool,
    pub bench: Vec<String>,
    pub all_targets: bool,
    /// `--doc` (`cargo test` only): doctests only.
    pub doc: bool,
}

impl Targets {
    pub fn is_default(&self) -> bool {
        self == &Targets::default()
    }
}

/// `--features` (comma- or space-separated, each occurrence, `-F` too),
/// `--all-features`, `--no-default-features`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Features {
    pub list: Vec<String>,
    pub all: bool,
    pub no_default: bool,
}

/// Which ignored tests run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Ignored {
    /// Not ignored ones (libtest's default).
    #[default]
    Exclude,
    /// `--include-ignored` (nextest: `--run-ignored all`).
    Include,
    /// `--ignored` (nextest: `--run-ignored only`).
    Only,
}

/// What a test run selects and how the harness runs it. A filter is a
/// *runtime* argument (W323 §6): three requests differing only here compile one
/// binary, which is what adequacy (W298 rung 2.5) relies on.
///
/// Read for `test`, `bench` and `nextest run|list`; default for every other verb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TestArgs {
    /// `cargo test NAME` and every free argument after `--`.
    pub filters: Vec<String>,
    pub exact: bool,
    /// `--skip PATTERN`, each occurrence.
    pub skip: Vec<String>,
    pub ignored: Ignored,
    /// `false` for `--no-fail-fast`.
    pub fail_fast: bool,
    /// `--no-run`: build the test binaries, run nothing.
    pub no_run: bool,
    /// `--test-threads N` (nextest: also `-j N`).
    pub threads: Option<u32>,
    /// `--nocapture` (nextest: `--no-capture`).
    pub nocapture: bool,
}

impl Default for TestArgs {
    fn default() -> Self {
        TestArgs {
            filters: Vec::new(),
            exact: false,
            skip: Vec::new(),
            ignored: Ignored::Exclude,
            fail_fast: true,
            no_run: false,
            threads: None,
            nocapture: false,
        }
    }
}

impl Request {
    /// An otherwise-empty request for `verb`.
    pub fn new(verb: Verb) -> Request {
        Request {
            toolchain: None,
            globals: Globals::default(),
            verb,
            help: false,
            manifest_path: None,
            target_dir: None,
            selection: Selection::default(),
            targets: Targets::default(),
            features: Features::default(),
            profile: None,
            target_triples: Vec::new(),
            message_format: Vec::new(),
            config: Vec::new(),
            unstable: Vec::new(),
            test: TestArgs::default(),
            env_allowlist: BTreeMap::new(),
            args: Vec::new(),
            passthrough: Vec::new(),
            trailing: Vec::new(),
        }
    }

    /// The class of this request: the verb's, refined by its flags.
    ///
    /// - Any `-h`/`--help` cargo itself sees makes it a [`Class::Query`]
    ///   (R989-B7: `cargo build --help` must never queue behind real builds).
    ///   One after `--` does not: `cargo test -- --help` builds the tests.
    ///   For an opaque verb the help flag sits in `args`, so that is scanned
    ///   too, up to its own `--`. `xtask` is exempt: its whole tail is the
    ///   xtask binary's (`run -p xtask --`), so cargo compiles it regardless.
    /// - `clippy --fix` rewrites source, so it is a [`Class::Mutation`], as
    ///   `cargo fix` is.
    pub fn class(&self) -> Class {
        if self.help {
            return Class::Query;
        }
        if self.verb.dialect() == Dialect::Opaque
            && self.verb != Verb::Xtask
            && self
                .args
                .iter()
                .take_while(|a| a.as_str() != "--")
                .any(|a| a == "-h" || a == "--help")
        {
            return Class::Query;
        }
        if self.verb == Verb::Clippy && self.passthrough.iter().any(|a| a == "--fix") {
            return Class::Mutation;
        }
        self.verb.class()
    }

    /// The packages this request is scoped to; empty means the whole default
    /// member set (no `-p`) or the whole workspace (`--workspace`), and a
    /// caller's closure must then cover the whole tree. This is the field the
    /// old recognizer called `CargoBuild::packages`.
    pub fn packages_in_scope(&self) -> &[String] {
        if self.selection.workspace {
            &[]
        } else {
            &self.selection.packages
        }
    }

    /// The profile cargo will build with: the one named, else the verb's
    /// default (`test` for test and nextest, `bench` for bench, `release`
    /// for install, else `dev`).
    pub fn effective_profile(&self) -> &str {
        self.profile.as_deref().unwrap_or_else(|| self.verb.default_profile())
    }

    /// The directory under `target/` that [`Self::effective_profile`] writes
    /// to, and so whose `.cargo-lock` the build takes: `dev`/`test` → `debug`,
    /// `release`/`bench` → `release`, a custom profile keeps its own name.
    ///
    /// Differs from the old recognizer's `CargoBuild::profile_dir` in exactly
    /// one place, deliberately: a profile-less `cargo bench` is `release`
    /// (cargo's `bench` profile inherits it), which the recognizer never saw
    /// because it did not recognize `bench`.
    pub fn profile_dir(&self) -> &str {
        match self.effective_profile() {
            "dev" | "test" => "debug",
            "release" | "bench" => "release",
            other => other,
        }
    }

    /// The argv this request renders to (without the leading `cargo`).
    pub fn render(&self) -> Vec<String> {
        crate::render::render(self)
    }
}
