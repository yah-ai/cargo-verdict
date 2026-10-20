//! What the plan needs to know about the machine before it adds a test runner:
//! the target runner cargo would already use, and which targets are not
//! libtest harnesses.
//!
//! **The scoped runner composes; it never replaces.** `--config
//! target.<triple>.runner=…` silently *replaces* a runner set in a config file
//! (triple or `cfg(…)` form) or in `CARGO_TARGET_<TRIPLE>_RUNNER` (W371 §4.4
//! R989-S3 case 3), so the plan reads the effective runner here and runs it
//! inside its own (`… exec env RUSTC_BOOTSTRAP=1 <camp runner…> "$0" "$@" …`).
//! Stable cargo cannot report the effective runner (`cargo config get` is
//! nightly-only), so [`CargoConfig`] reads it the way cargo does. Each rule
//! below was measured on cargo 1.97.0, 2026-10-18, with logging runners:
//!
//! 1. `--config` > `CARGO_TARGET_<TRIPLE>_RUNNER` > config files.
//! 2. Config files: each ancestor of the cwd's `.cargo/config(.toml)`, deepest
//!    first, then `$CARGO_HOME`'s. For the runner key the deepest file wins
//!    outright: two arrays are **not** concatenated.
//! 3. A `target.<triple>.runner` in any file beats every
//!    `target.'cfg(…)'.runner`, even a deeper one.
//! 4. Two different `cfg(…)` tables that both match are a cargo error; the
//!    same `cfg(…)` key in two files is one key, and the deepest wins.
//! 5. A string in one file and an array in another fail to load at all.
//! 6. A relative program path containing `/` resolves against the parent of
//!    the `.cargo` directory for a file, and against cargo's cwd for env and
//!    `--config`. Without a `/` it is looked up on `PATH`.
//!
//! **Anything this reader cannot decide refuses structure, never the runner.**
//! A config it cannot parse, an `include`, a `cfg(…)` key it cannot read or
//! evaluate, `cfg(…)` keys alongside configured rustflags (cargo evaluates cfg
//! with them): each becomes [`CampRunner::Unknown`], and the plan then adds no
//! runner at all, so the camp's own runner runs exactly as before and the tests
//! are reported `structured: false`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::request::Request;
use crate::verb::Verb;

/// A target runner the camp configures, as cargo would resolve it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CampRunner {
    /// cargo runs a test binary as `<argv…> <binary> <args…>`. A relative
    /// program path is already resolved.
    Compose { argv: Vec<String>, source: String },
    /// A runner may be configured, and this reader cannot say which.
    Unknown(String),
}

/// One config file, as cargo layers it.
#[derive(Debug, Clone)]
struct Layer {
    path: PathBuf,
    /// What a relative program path in this file resolves against.
    base: PathBuf,
    table: toml::Table,
}

/// cargo's configuration as seen from one directory: the environment, then
/// the config files, highest precedence first.
#[derive(Debug, Clone)]
pub struct CargoConfig {
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    layers: Vec<Layer>,
    /// Why this reader cannot vouch for what it read (parse errors,
    /// `include`). Non-empty makes every runner lookup [`CampRunner::Unknown`].
    problems: Vec<String>,
}

impl CargoConfig {
    /// Read the config cargo would read when started in `cwd` with `env` (the
    /// whole environment of the cargo process, not overrides).
    pub fn load(cwd: &Path, env: &BTreeMap<String, String>) -> CargoConfig {
        let mut dirs: Vec<PathBuf> = cwd.ancestors().map(|d| d.join(".cargo")).collect();
        let home = env
            .get("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| env.get("HOME").map(|h| Path::new(h).join(".cargo")));
        if let Some(h) = home {
            dirs.push(if h.is_relative() { cwd.join(h) } else { h });
        }
        let mut seen = BTreeSet::new();
        let mut cfg = CargoConfig { cwd: cwd.to_path_buf(), env: env.clone(), layers: Vec::new(), problems: Vec::new() };
        for dir in dirs {
            // cargo reads `config` over `config.toml` when a directory has both.
            let Some(path) = ["config", "config.toml"].iter().map(|f| dir.join(f)).find(|p| p.is_file()) else {
                continue;
            };
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(key) {
                continue;
            }
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| {
                t.parse::<toml::Table>().map_err(|e| e.to_string())
            }) {
                Ok(table) => {
                    if table.contains_key("include") {
                        cfg.problems.push(format!("{} uses `include`, which this reader does not follow", path.display()));
                    }
                    let base = dir.parent().unwrap_or(&dir).to_path_buf();
                    cfg.layers.push(Layer { path, base, table });
                }
                Err(e) => cfg.problems.push(format!("cannot read {}: {}", path.display(), e.lines().next().unwrap_or(""))),
            }
        }
        cfg
    }

    /// The triples cargo builds for when a request names no `--target`
    /// (`CARGO_BUILD_TARGET`, then `build.target`); empty means the host.
    pub fn build_targets(&self) -> Vec<String> {
        if let Some(t) = self.env.get("CARGO_BUILD_TARGET").filter(|t| !t.is_empty()) {
            return vec![t.clone()];
        }
        self.layers
            .iter()
            .find_map(|l| l.table.get("build")?.get("target"))
            .map(|v| match v {
                toml::Value::String(s) => vec![s.clone()],
                toml::Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default()
    }

    /// The runner cargo would use for `triple`, or `None` when there is none.
    /// `cfgs` is called only when a `cfg(…)` table has to be evaluated.
    pub fn runner_for(&self, triple: &str, cfgs: impl FnOnce() -> Result<Cfgs, String>) -> Option<CampRunner> {
        if !self.problems.is_empty() {
            return Some(CampRunner::Unknown(self.problems.join("; ")));
        }
        let env_key = format!("CARGO_TARGET_{}_RUNNER", triple.to_uppercase().replace(['-', '.'], "_"));
        if let Some(v) = self.env.get(&env_key) {
            let argv: Vec<String> = v.split_whitespace().map(str::to_string).collect();
            return Some(if argv.is_empty() {
                CampRunner::Unknown(format!("${env_key} is set but empty"))
            } else {
                CampRunner::Compose { argv: resolve(argv, &self.cwd), source: format!("${env_key}") }
            });
        }
        if let Some(r) = self.table_runner(triple) {
            return Some(r);
        }

        // cfg(…) tables: the distinct keys that set a runner, each read from
        // its deepest file.
        let keys: BTreeSet<&String> = self
            .layers
            .iter()
            .filter_map(|l| l.table.get("target")?.as_table())
            .flat_map(|t| t.iter().filter(|(k, v)| k.starts_with("cfg(") && v.get("runner").is_some()).map(|(k, _)| k))
            .collect();
        if keys.is_empty() {
            return None;
        }
        if let Some(why) = self.rustflags_configured(triple) {
            return Some(CampRunner::Unknown(format!(
                "a target.'cfg(…)'.runner is configured, and cargo evaluates cfg with {why}, which this reader does not apply"
            )));
        }
        let cfgs = match cfgs() {
            Ok(c) => c,
            Err(e) => return Some(CampRunner::Unknown(format!("cannot read rustc's cfg for {triple}: {e}"))),
        };
        let mut matched = Vec::new();
        for key in keys {
            match parse_cfg_key(key) {
                Ok(expr) if expr.eval(&cfgs) => matched.push(key),
                Ok(_) => {}
                Err(e) => return Some(CampRunner::Unknown(format!("cannot read `target.'{key}'`: {e}"))),
            }
        }
        match matched.as_slice() {
            [] => None,
            [key] => self.table_runner(key),
            many => Some(CampRunner::Unknown(format!(
                "several target.'cfg(…)'.runner tables match {triple} ({}); cargo refuses this config",
                many.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
            ))),
        }
    }

    /// `target.<key>.runner` from the deepest file that sets it.
    fn table_runner(&self, key: &str) -> Option<CampRunner> {
        let hits: Vec<(&Layer, &toml::Value)> = self
            .layers
            .iter()
            .filter_map(|l| Some((l, l.table.get("target")?.get(key)?.get("runner")?)))
            .collect();
        let (layer, value) = *hits.first()?;
        let source = format!("target.{}.runner in {}", quote_key(key), layer.path.display());
        if hits.iter().any(|(_, v)| v.type_str() != value.type_str()) {
            return Some(CampRunner::Unknown(format!("{source} is a {} in one file and not in another; cargo refuses this config", value.type_str())));
        }
        Some(match argv_of(value) {
            Ok(argv) => CampRunner::Compose { argv: resolve(argv, &layer.base), source },
            Err(e) => CampRunner::Unknown(format!("{source}: {e}")),
        })
    }

    fn rustflags_configured(&self, triple: &str) -> Option<String> {
        let tkey = format!("CARGO_TARGET_{}_RUSTFLAGS", triple.to_uppercase().replace(['-', '.'], "_"));
        for k in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", tkey.as_str()] {
            if self.env.get(k).is_some_and(|v| !v.trim().is_empty()) {
                return Some(format!("${k}"));
            }
        }
        for l in &self.layers {
            let in_build = l.table.get("build").and_then(|b| b.get("rustflags")).is_some();
            let in_target = l.table.get("target").and_then(|t| t.as_table()).is_some_and(|t| {
                t.iter().any(|(k, v)| (k == triple || k.starts_with("cfg(")) && v.get("rustflags").is_some())
            });
            if in_build || in_target {
                return Some(format!("the rustflags in {}", l.path.display()));
            }
        }
        None
    }

    /// Does any file set a runner at all? (For `--config <file>` arguments.)
    fn sets_any_runner(&self) -> bool {
        self.layers.iter().any(|l| {
            l.table.get("target").and_then(|t| t.as_table()).is_some_and(|t| t.values().any(|v| v.get("runner").is_some()))
        })
    }
}

fn quote_key(k: &str) -> String {
    if k.starts_with("cfg(") { format!("'{k}'") } else { k.to_string() }
}

fn argv_of(v: &toml::Value) -> Result<Vec<String>, String> {
    let argv: Vec<String> = match v {
        toml::Value::String(s) => s.split_whitespace().map(str::to_string).collect(),
        toml::Value::Array(a) => a
            .iter()
            .map(|x| x.as_str().map(str::to_string).ok_or_else(|| "an array entry is not a string".to_string()))
            .collect::<Result<_, _>>()?,
        other => return Err(format!("expected a string or an array, found a {}", other.type_str())),
    };
    if argv.is_empty() {
        return Err("it is empty".into());
    }
    Ok(argv)
}

/// A relative program path with a `/` is resolved against `base`; anything
/// else (a bare name for `PATH`, an absolute path) is kept.
fn resolve(mut argv: Vec<String>, base: &Path) -> Vec<String> {
    let p = Path::new(&argv[0]);
    if p.is_relative() && argv[0].contains('/') {
        argv[0] = base.join(p).display().to_string();
    }
    argv
}

// ─── cfg(…) ──────────────────────────────────────────────────────────────────

/// What `rustc --print cfg` printed: bare names and `name="value"` pairs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cfgs(BTreeSet<(String, Option<String>)>);

impl Cfgs {
    pub fn parse(print_cfg: &str) -> Cfgs {
        Cfgs(
            print_cfg
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|l| match l.split_once('=') {
                    Some((k, v)) => (k.trim().to_string(), Some(v.trim().trim_matches('"').to_string())),
                    None => (l.to_string(), None),
                })
                .collect(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Name(String),
    KeyValue(String, String),
    All(Vec<Expr>),
    Any(Vec<Expr>),
    Not(Box<Expr>),
}

impl Expr {
    fn eval(&self, c: &Cfgs) -> bool {
        match self {
            Expr::Name(n) => c.0.contains(&(n.clone(), None)),
            Expr::KeyValue(k, v) => c.0.contains(&(k.clone(), Some(v.clone()))),
            Expr::All(xs) => xs.iter().all(|x| x.eval(c)),
            Expr::Any(xs) => xs.iter().any(|x| x.eval(c)),
            Expr::Not(x) => !x.eval(c),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Str(String),
    Punct(char),
}

fn tokens(s: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(&c) = it.peek() {
        if c.is_whitespace() {
            it.next();
        } else if "(),=".contains(c) {
            out.push(Tok::Punct(c));
            it.next();
        } else if c == '"' {
            it.next();
            let mut v = String::new();
            loop {
                match it.next() {
                    Some('"') => break,
                    Some('\\') => return Err("escapes in cfg strings are not read".into()),
                    Some(ch) => v.push(ch),
                    None => return Err("unterminated string".into()),
                }
            }
            out.push(Tok::Str(v));
        } else if c.is_alphanumeric() || c == '_' {
            let mut v = String::new();
            while let Some(&ch) = it.peek().filter(|ch| ch.is_alphanumeric() || **ch == '_') {
                v.push(ch);
                it.next();
            }
            out.push(Tok::Ident(v));
        } else {
            return Err(format!("unexpected `{c}`"));
        }
    }
    Ok(out)
}

/// `cfg(<predicate>)`, as a `[target.'…']` key spells it.
fn parse_cfg_key(key: &str) -> Result<Expr, String> {
    let toks = tokens(key)?;
    let mut i = 0;
    let expect = |i: &mut usize, t: Tok| -> Result<(), String> {
        if toks.get(*i) == Some(&t) {
            *i += 1;
            Ok(())
        } else {
            Err(format!("expected {t:?} at token {i}"))
        }
    };
    expect(&mut i, Tok::Ident("cfg".into()))?;
    expect(&mut i, Tok::Punct('('))?;
    let e = expr(&toks, &mut i)?;
    expect(&mut i, Tok::Punct(')'))?;
    if i != toks.len() {
        return Err("trailing tokens".into());
    }
    Ok(e)
}

fn expr(toks: &[Tok], i: &mut usize) -> Result<Expr, String> {
    let Some(Tok::Ident(name)) = toks.get(*i) else { return Err(format!("expected a name at token {i}")) };
    *i += 1;
    match toks.get(*i) {
        Some(Tok::Punct('(')) if matches!(name.as_str(), "all" | "any" | "not") => {
            *i += 1;
            let mut list = Vec::new();
            while toks.get(*i) != Some(&Tok::Punct(')')) {
                list.push(expr(toks, i)?);
                match toks.get(*i) {
                    Some(Tok::Punct(',')) => *i += 1,
                    Some(Tok::Punct(')')) => {}
                    _ => return Err(format!("expected `,` or `)` at token {i}")),
                }
            }
            *i += 1;
            Ok(match name.as_str() {
                "all" => Expr::All(list),
                "any" => Expr::Any(list),
                _ => {
                    let [one] = <[Expr; 1]>::try_from(list).map_err(|_| "not(…) takes one predicate".to_string())?;
                    Expr::Not(Box::new(one))
                }
            })
        }
        Some(Tok::Punct('=')) => {
            *i += 1;
            let Some(Tok::Str(v)) = toks.get(*i) else { return Err(format!("expected a string at token {i}")) };
            *i += 1;
            Ok(Expr::KeyValue(name.clone(), v.clone()))
        }
        _ => Ok(Expr::Name(name.clone())),
    }
}

// ─── harness = false ─────────────────────────────────────────────────────────

/// The file stems (`<name>` of `deps/<name>-<hash>`) of every target in
/// `manifests` whose table says `harness = false`. `cargo metadata` does not
/// expose `harness`, so the manifests are read.
pub fn no_harness_stems<'a>(manifests: impl IntoIterator<Item = (&'a str, &'a Path)>) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for (package, manifest) in manifests {
        let text = std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
        let table: toml::Table = text.parse().map_err(|e: toml::de::Error| format!("{}: {}", manifest.display(), e.message()))?;
        let no_harness = |t: &toml::Value| t.get("harness").and_then(toml::Value::as_bool) == Some(false);
        if let Some(lib) = table.get("lib").filter(|l| no_harness(l)) {
            out.insert(lib.get("name").and_then(toml::Value::as_str).unwrap_or(package).replace('-', "_"));
        }
        for kind in ["bin", "test", "bench", "example"] {
            for t in table.get(kind).and_then(toml::Value::as_array).into_iter().flatten() {
                if let Some(name) = t.get("name").and_then(toml::Value::as_str).filter(|_| no_harness(t)) {
                    out.insert(name.replace('-', "_"));
                }
            }
        }
    }
    Ok(out)
}

// ─── discovery ───────────────────────────────────────────────────────────────

/// Everything [`crate::plan()`] needs to know about the machine for `request`,
/// run in `cwd` with `env` (the whole environment cargo will get). Spawns
/// `rustc -vV`, `cargo metadata --no-deps` and, only when a `cfg(…)` runner has
/// to be evaluated, `rustc --print cfg`, and only for a `cargo test` that runs
/// tests; any other request gets [`crate::PlanOptions::new`] with no host.
pub fn discover(request: &Request, cwd: &Path, env: &BTreeMap<String, String>) -> crate::PlanOptions {
    if request.verb != Verb::Test || request.test.no_run {
        return crate::PlanOptions::new("");
    }
    let dir = match &request.globals.directory {
        Some(d) => cwd.join(d),
        None => cwd.to_path_buf(),
    };
    let rustc = |args: &[&str]| -> Result<String, String> {
        let mut cmd = match env.get("RUSTC") {
            Some(r) => Command::new(r),
            None => {
                let mut c = Command::new("rustc");
                if let Some(tc) = &request.toolchain {
                    c.arg(format!("+{tc}"));
                }
                c
            }
        };
        let out = cmd.args(args).current_dir(&dir).env_clear().envs(env).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("rustc failed").to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let host = rustc(&["-vV"])
        .ok()
        .and_then(|o| o.lines().find_map(|l| l.strip_prefix("host: ").map(str::to_string)))
        .unwrap_or_default();
    let mut opts = crate::PlanOptions::new(host.clone());
    let config = CargoConfig::load(&dir, env);
    opts.default_targets = config.build_targets();

    // A `--config <file>` the request passes is cargo's highest layer.
    let request_file_runner = request.globals.config.iter().chain(&request.config).filter(|c| !c.contains('=')).find(|f| {
        let p = dir.join(f);
        std::fs::read_to_string(&p).ok().and_then(|t| t.parse::<toml::Table>().ok()).is_some_and(|table| {
            CargoConfig { cwd: dir.clone(), env: BTreeMap::new(), layers: vec![Layer { path: p, base: dir.clone(), table }], problems: vec![] }
                .sets_any_runner()
        })
    });

    let triples: Vec<String> = if !request.target_triples.is_empty() {
        request.target_triples.clone()
    } else if !opts.default_targets.is_empty() {
        opts.default_targets.clone()
    } else {
        vec![host]
    };
    for t in triples.iter().filter(|t| !t.is_empty()) {
        let r = match request_file_runner {
            Some(f) => Some(CampRunner::Unknown(format!("the request's --config file {f} sets a target runner"))),
            None => config.runner_for(t, || rustc(&["--print", "cfg", "--target", t]).map(|o| Cfgs::parse(&o))),
        };
        if let Some(r) = r {
            opts.runners.insert(t.clone(), r);
        }
    }

    opts.no_harness = workspace_no_harness(request, &dir, env);
    opts
}

fn workspace_no_harness(request: &Request, dir: &Path, env: &BTreeMap<String, String>) -> Result<BTreeSet<String>, String> {
    let mut cmd = Command::new(env.get("CARGO").map(String::as_str).unwrap_or("cargo"));
    if let Some(tc) = request.toolchain.as_ref().filter(|_| !env.contains_key("CARGO")) {
        cmd.arg(format!("+{tc}"));
    }
    cmd.args(["metadata", "--no-deps", "--format-version", "1"]);
    if let Some(m) = &request.manifest_path {
        cmd.args(["--manifest-path", m]);
    }
    let out = cmd.current_dir(dir).env_clear().envs(env).output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("cargo metadata: {}", err.lines().find(|l| l.starts_with("error")).unwrap_or("failed")));
    }
    let meta: cargo_metadata::Metadata =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata printed unreadable JSON: {e}"))?;
    no_harness_stems(meta.packages.iter().map(|p| (p.name.as_str(), p.manifest_path.as_std_path())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux() -> Cfgs {
        Cfgs::parse("debug_assertions\npanic=\"unwind\"\ntarget_arch=\"x86_64\"\ntarget_os=\"linux\"\ntarget_family=\"unix\"\nunix\n")
    }

    #[test]
    fn cfg_keys_evaluate_like_cargo_s() {
        let c = linux();
        for (key, want) in [
            ("cfg(unix)", true),
            ("cfg(windows)", false),
            ("cfg(target_os = \"linux\")", true),
            ("cfg(all(unix, target_arch = \"x86_64\"))", true),
            ("cfg(all(unix, target_arch = \"aarch64\"))", false),
            ("cfg(any(windows, target_os = \"linux\",))", true),
            ("cfg(not(windows))", true),
            ("cfg(all())", true),
            ("cfg(any())", false),
        ] {
            assert_eq!(parse_cfg_key(key).map(|e| e.eval(&c)), Ok(want), "{key}");
        }
        for bad in ["cfg(unix", "cfg()", "cfg(not(a, b))", "cfg(a = b)", "cfg(a) x", "unix"] {
            assert!(parse_cfg_key(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn relative_program_paths_resolve_only_with_a_slash() {
        let b = Path::new("/w");
        assert_eq!(resolve(vec!["./r".into(), "x".into()], b), ["/w/./r", "x"]);
        assert_eq!(resolve(vec!["qemu".into()], b), ["qemu"]);
        assert_eq!(resolve(vec!["/abs/r".into()], b), ["/abs/r"]);
    }
}
