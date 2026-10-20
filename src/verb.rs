//! Cargo's verbs, and the class each one belongs to.
//!
//! The class is W323 §1's split of cargo by *what an invocation does to shared
//! state*, not by its name: a verdict is expensive and shareable, a mutation is
//! cheap and unshareable, a query is neither. Every later phase keys on it, so
//! it falls out of the verb by table, never by a caller's guess.

use serde::{Deserialize, Serialize};

/// W323 §1's four classes, plus the honest fifth: a subcommand this crate has
/// never heard of (a third-party `cargo-foo`, a user alias) cannot be sorted
/// without running it, and pretending otherwise is how a lock holder goes
/// invisible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Builds and answers a question about the tree: `check`, `test`, `doc`, …
    /// Shareable: two identical requests can be served by one run.
    Verdict,
    /// Builds, then hands the tree to a long-lived process: `run`, `watch`.
    /// W323 §1: a verdict prologue followed by a tenancy body.
    Tenancy,
    /// Changes the declaration, the source or the derivation: `add`, `fmt`,
    /// `fix`, `clean`, `install`, `publish`, …. Never shareable.
    Mutation,
    /// Reads and prints: `metadata`, `tree`, `--version`, any `--help`.
    /// Takes no target-dir lock and must never queue.
    Query,
    /// A subcommand this crate does not know.
    Unknown,
}

/// A cargo subcommand, normalized: aliases (`b`, `t`, `c`, `d`, `r`, `rm`)
/// resolve to the long form and render back as it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Verb {
    // ── verdicts ──
    Build,
    Check,
    Clippy,
    Test,
    Bench,
    Doc,
    Rustc,
    Rustdoc,
    Package,
    /// `cargo nextest <sub>`. The sub-subcommand decides the class; `r`
    /// normalizes to `run`. `None` is a bare `cargo nextest` (prints help).
    Nextest(Option<String>),
    // ── tenancy ──
    Run,
    /// The conventional `xtask = "run -p xtask --"` alias (matklad's cargo-xtask
    /// pattern; this camp's `.cargo/config.toml` defines it). It is `run`, so
    /// everything after it belongs to the xtask binary, not to cargo — which is
    /// why `cargo xtask install --profile debug` does not select a profile.
    Xtask,
    Watch,
    // ── mutations ──
    Fix,
    Fmt,
    Add,
    Remove,
    Update,
    Vendor,
    GenerateLockfile,
    Clean,
    Install,
    Uninstall,
    Publish,
    Yank,
    Owner,
    Login,
    Logout,
    New,
    Init,
    // ── queries ──
    /// `cargo help [topic]`, `cargo -h`, or a bare `cargo`.
    Help,
    /// `cargo version`, `cargo --version`, `cargo -V`.
    Version,
    /// `cargo --list`.
    List,
    /// `cargo --explain CODE`.
    Explain,
    Metadata,
    Tree,
    LocateProject,
    Pkgid,
    Search,
    Info,
    ReadManifest,
    VerifyProject,
    Report,
    Config,
    /// Anything else, verbatim. Never holds a name [`Verb::from_word`] knows.
    Other(String),
}

/// How the arguments after a verb are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    /// cargo's build flags are modelled; positionals are operands; everything
    /// after `--` is the program's (rustc's, clippy-driver's, the binary's).
    Build,
    /// [`Dialect::Build`], plus positionals are test filters and the args after
    /// `--` are read as libtest's.
    Test,
    /// Nothing after the verb is modelled. External subcommands parse their own
    /// argv (`cargo fmt --config` is rustfmt's `--config`, not cargo's), so the
    /// only faithful thing is to keep it verbatim.
    Opaque,
}

/// Every word [`Verb::from_word`] accepts, alias spellings included. The
/// round-trip property test draws from it.
pub const VERB_WORDS: &[&str] = &[
    "build", "b", "check", "c", "clippy", "test", "t", "bench", "doc", "d", "rustc",
    "rustdoc", "package", "nextest", "run", "r", "xtask", "watch", "fix", "fmt", "add",
    "remove", "rm", "update", "vendor", "generate-lockfile", "clean", "install",
    "uninstall", "publish", "yank", "owner", "login", "logout", "new", "init", "help",
    "version", "metadata", "tree", "locate-project", "pkgid", "search", "info",
    "read-manifest", "verify-project", "report", "config",
];

impl Verb {
    /// The verb a subcommand word names. `nextest`'s sub-subcommand is read by
    /// the parser, so this returns `Nextest(None)` for it.
    pub fn from_word(word: &str) -> Verb {
        match word {
            "build" | "b" => Verb::Build,
            "check" | "c" => Verb::Check,
            "clippy" => Verb::Clippy,
            "test" | "t" => Verb::Test,
            "bench" => Verb::Bench,
            "doc" | "d" => Verb::Doc,
            "rustc" => Verb::Rustc,
            "rustdoc" => Verb::Rustdoc,
            "package" => Verb::Package,
            "nextest" => Verb::Nextest(None),
            "run" | "r" => Verb::Run,
            "xtask" => Verb::Xtask,
            "watch" => Verb::Watch,
            "fix" => Verb::Fix,
            "fmt" => Verb::Fmt,
            "add" => Verb::Add,
            "remove" | "rm" => Verb::Remove,
            "update" => Verb::Update,
            "vendor" => Verb::Vendor,
            "generate-lockfile" => Verb::GenerateLockfile,
            "clean" => Verb::Clean,
            "install" => Verb::Install,
            "uninstall" => Verb::Uninstall,
            "publish" => Verb::Publish,
            "yank" => Verb::Yank,
            "owner" => Verb::Owner,
            "login" => Verb::Login,
            "logout" => Verb::Logout,
            "new" => Verb::New,
            "init" => Verb::Init,
            "help" => Verb::Help,
            "version" => Verb::Version,
            "metadata" => Verb::Metadata,
            "tree" => Verb::Tree,
            "locate-project" => Verb::LocateProject,
            "pkgid" => Verb::Pkgid,
            "search" => Verb::Search,
            "info" => Verb::Info,
            "read-manifest" => Verb::ReadManifest,
            "verify-project" => Verb::VerifyProject,
            "report" => Verb::Report,
            "config" => Verb::Config,
            other => Verb::Other(other.to_string()),
        }
    }

    /// The long-form word this verb renders as. `List` and `Explain` have no
    /// subcommand word; they render as the global flags `--list` / `--explain`.
    pub fn word(&self) -> &str {
        match self {
            Verb::Build => "build",
            Verb::Check => "check",
            Verb::Clippy => "clippy",
            Verb::Test => "test",
            Verb::Bench => "bench",
            Verb::Doc => "doc",
            Verb::Rustc => "rustc",
            Verb::Rustdoc => "rustdoc",
            Verb::Package => "package",
            Verb::Nextest(_) => "nextest",
            Verb::Run => "run",
            Verb::Xtask => "xtask",
            Verb::Watch => "watch",
            Verb::Fix => "fix",
            Verb::Fmt => "fmt",
            Verb::Add => "add",
            Verb::Remove => "remove",
            Verb::Update => "update",
            Verb::Vendor => "vendor",
            Verb::GenerateLockfile => "generate-lockfile",
            Verb::Clean => "clean",
            Verb::Install => "install",
            Verb::Uninstall => "uninstall",
            Verb::Publish => "publish",
            Verb::Yank => "yank",
            Verb::Owner => "owner",
            Verb::Login => "login",
            Verb::Logout => "logout",
            Verb::New => "new",
            Verb::Init => "init",
            Verb::Help => "help",
            Verb::Version => "version",
            Verb::List => "--list",
            Verb::Explain => "--explain",
            Verb::Metadata => "metadata",
            Verb::Tree => "tree",
            Verb::LocateProject => "locate-project",
            Verb::Pkgid => "pkgid",
            Verb::Search => "search",
            Verb::Info => "info",
            Verb::ReadManifest => "read-manifest",
            Verb::VerifyProject => "verify-project",
            Verb::Report => "report",
            Verb::Config => "config",
            Verb::Other(w) => w,
        }
    }

    /// The class this verb belongs to on its own. [`crate::Request::class`]
    /// refines it with what the flags say (`--help`, `clippy --fix`); callers
    /// holding a request should ask the request.
    pub fn class(&self) -> Class {
        match self {
            Verb::Build
            | Verb::Check
            | Verb::Clippy
            | Verb::Test
            | Verb::Bench
            | Verb::Doc
            | Verb::Rustc
            | Verb::Rustdoc
            | Verb::Package => Class::Verdict,
            Verb::Nextest(sub) => match sub.as_deref() {
                Some("run" | "list" | "archive") => Class::Verdict,
                // `cargo nextest` alone prints help; `show-config` reads.
                None | Some("show-config" | "help") => Class::Query,
                Some("self") => Class::Mutation,
                Some(_) => Class::Unknown,
            },
            Verb::Run | Verb::Xtask | Verb::Watch => Class::Tenancy,
            Verb::Fix
            | Verb::Fmt
            | Verb::Add
            | Verb::Remove
            | Verb::Update
            | Verb::Vendor
            | Verb::GenerateLockfile
            | Verb::Clean
            | Verb::Install
            | Verb::Uninstall
            | Verb::Publish
            | Verb::Yank
            | Verb::Owner
            | Verb::Login
            | Verb::Logout
            | Verb::New
            | Verb::Init => Class::Mutation,
            Verb::Help
            | Verb::Version
            | Verb::List
            | Verb::Explain
            | Verb::Metadata
            | Verb::Tree
            | Verb::LocateProject
            | Verb::Pkgid
            | Verb::Search
            | Verb::Info
            | Verb::ReadManifest
            | Verb::VerifyProject
            | Verb::Report
            | Verb::Config => Class::Query,
            Verb::Other(_) => Class::Unknown,
        }
    }

    pub(crate) fn dialect(&self) -> Dialect {
        match self {
            Verb::Test | Verb::Bench => Dialect::Test,
            Verb::Nextest(sub) => match sub.as_deref() {
                Some("run" | "list") => Dialect::Test,
                Some("archive") => Dialect::Build,
                _ => Dialect::Opaque,
            },
            Verb::Build
            | Verb::Check
            | Verb::Clippy
            | Verb::Doc
            | Verb::Rustc
            | Verb::Rustdoc
            | Verb::Package
            | Verb::Run
            | Verb::Fix
            | Verb::Clean
            | Verb::Install
            | Verb::Publish => Dialect::Build,
            _ => Dialect::Opaque,
        }
    }

    /// The profile cargo uses when the request names none.
    pub fn default_profile(&self) -> &'static str {
        match self {
            Verb::Test | Verb::Nextest(_) => "test",
            Verb::Bench => "bench",
            Verb::Install => "release",
            _ => "dev",
        }
    }

    pub(crate) fn is_nextest(&self) -> bool {
        matches!(self, Verb::Nextest(_))
    }
}
