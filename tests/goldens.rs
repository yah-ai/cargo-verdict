//! R989-F21: every fixture run renders to a checked-in cargo-verdict/1 JSON
//! per `want`, every golden validates against the emitted JSON Schema, and
//! `cargo verdict` exits with cargo's own exit code.
//!
//! Regenerate after an intended change with
//! `UPDATE_GOLDENS=1 cargo test --test goldens`, then read the diff.

use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_verdict::exec;
use cargo_verdict::plan::host_triple;
use cargo_verdict::verdict::schema;
use cargo_verdict::{parse, plan, Demux, PlanOptions, Verdict, Want};
use serde_json::Value;

const CASES: &[(&str, &str, &str)] = &[
    ("mixed", "test --no-fail-fast", "mixed-test"),
    ("mixed", "test --lib ok", "mixed-green"),
    ("broken", "check", "broken-check"),
];

const WANTS: &[(Want, &str)] = &[
    (Want::Summary, "summary"),
    (Want::Failures, "failures"),
    (Want::Diagnostics, "diagnostics"),
    (Want::Full, "full"),
];

/// The built CLI. `env!("CARGO_BIN_EXE_cargo-verdict")` is the usual spelling,
/// but a `sh` rustc-wrapper (this repo's .cargo/rustc-wrapper.sh) drops it:
/// dash does not pass on environment names that are not shell identifiers,
/// and `-` makes this one such. So fall back to cargo's layout: the test
/// binary sits in `<target>/<profile>/deps/`, the CLI one level up.
fn cli() -> PathBuf {
    match option_env!("CARGO_BIN_EXE_cargo-verdict") {
        Some(p) => PathBuf::from(p),
        None => {
            let exe = std::env::current_exe().unwrap();
            exe.parent().and_then(Path::parent).unwrap().join(format!("cargo-verdict{}", std::env::consts::EXE_SUFFIX))
        }
    }
}

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn target_dir(tag: &str) -> String {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("golden-{tag}")).display().to_string()
}

fn golden_path(tag: &str, want: &str) -> PathBuf {
    manifest_dir().join("tests/goldens").join(format!("{tag}.{want}.json"))
}

/// Replace what differs between machines and runs.
fn normalize(mut v: Value, host: &str) -> Value {
    // Planted by this test, not part of the request under test.
    v["request"]["env_allowlist"] = serde_json::json!({});
    // Cache state decides compiled vs fresh; the total is what is stable.
    let units = &mut v["build"]["units"];
    let total = units["compiled"].as_u64().unwrap() + units["fresh"].as_u64().unwrap();
    *units = serde_json::json!({ "compiled": total, "fresh": 0 });
    fn walk(v: &mut Value, host: &str) {
        match v {
            Value::String(s) => *s = scrub(&s.replace(host, "<host>")),
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, host)),
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if k == "duration_ms" && !x.is_null() {
                        *x = Value::from(0);
                    } else {
                        walk(x, host);
                    }
                }
            }
            _ => {}
        }
    }
    walk(&mut v, host);
    v
}

/// Thread ids in panic lines and libtest's timings.
fn scrub(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let thread = rest.find("' (").filter(|&i| {
            let tail = &rest[i + 3..];
            let digits = tail.chars().take_while(char::is_ascii_digit).count();
            digits > 0 && tail[digits..].starts_with(") panicked")
        });
        let timing = rest.find("finished in ");
        match (thread, timing) {
            (Some(i), t) if t.is_none_or(|t| i < t) => {
                out.push_str(&rest[..i + 3]);
                out.push('N');
                let tail = &rest[i + 3..];
                rest = &tail[tail.chars().take_while(char::is_ascii_digit).count()..];
            }
            (_, Some(t)) => {
                out.push_str(&rest[..t + "finished in ".len()]);
                out.push_str("Xs");
                let tail = &rest[t + "finished in ".len()..];
                let n = tail.find('s').map_or(tail.len(), |e| e + 1);
                rest = &tail[n..];
            }
            _ => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

fn run_case(fixture: &str, line: &str, tag: &str, host: &str) -> Verdict {
    let mut request = parse(&line.split_whitespace().collect::<Vec<_>>());
    let mut planned = request.clone();
    planned.env_allowlist.insert("CARGO_TARGET_DIR".into(), target_dir(tag));
    // Backtraces depend on the machine; libtest's own panic line does not.
    planned.env_allowlist.insert("RUST_BACKTRACE".into(), "0".into());
    let p = plan(&planned, &PlanOptions::new(host));
    let mut d = Demux::new();
    let exit = exec::run(&p, Some(&manifest_dir().join("tests/fixtures").join(fixture)), |s, l| d.push(s, l)).unwrap();
    request.env_allowlist = planned.env_allowlist;
    Verdict::collect(&request, &p, &d, exit)
}

#[test]
fn each_fixture_renders_to_its_golden_per_want_and_each_golden_validates() {
    let host = host_triple().unwrap();
    let schema = schema();
    let update = std::env::var_os("UPDATE_GOLDENS").is_some();
    let mut mismatches = Vec::new();
    for (fixture, line, tag) in CASES {
        let full = run_case(fixture, line, tag, &host);
        for (want, name) in WANTS {
            let got = normalize(serde_json::to_value(full.project(*want)).unwrap(), &host);
            if let Err(e) = validate(&schema, &schema, &got, "$") {
                panic!("{tag}.{name} does not validate against the schema: {e}");
            }
            let path = golden_path(tag, name);
            let text = serde_json::to_string_pretty(&got).unwrap() + "\n";
            if update {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &text).unwrap();
                continue;
            }
            let want_text = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("missing golden {}; run with UPDATE_GOLDENS=1", path.display()));
            if want_text != text {
                mismatches.push(format!("{}\n--- got ---\n{text}", path.display()));
            }
        }
    }
    assert!(mismatches.is_empty(), "goldens differ:\n{}", mismatches.join("\n"));
}

#[test]
fn the_views_say_what_the_ticket_says() {
    let dir = manifest_dir().join("tests/goldens");
    let read = |f: &str| -> Value { serde_json::from_str(&std::fs::read_to_string(dir.join(f)).unwrap()).unwrap() };
    // A green run under summary is counts only.
    let green = read("mixed-green.summary.json");
    assert_eq!(green["outcome"], "pass");
    assert_eq!(green["tests"]["totals"]["passed"], 1);
    assert_eq!(green["build"]["diagnostics"], serde_json::json!([]));
    assert!(green["tests"]["binaries"].as_array().unwrap().iter().all(|b| b["failures"] == serde_json::json!([])));
    // A red one under failures carries the name, the panic location and the output.
    let red = read("mixed-test.failures.json");
    assert_eq!(red["outcome"], "test_failed");
    let f = &red["tests"]["binaries"][0]["failures"][0];
    assert_eq!(f["name"], "t::bad");
    assert_eq!(f["location"], "src/lib.rs:17:9");
    assert!(f["output"].as_str().unwrap().contains("captured before the panic"));
    // Warnings are in diagnostics, not in failures.
    assert_eq!(read("mixed-test.failures.json")["build"]["diagnostics"], serde_json::json!([]));
    assert_eq!(read("mixed-test.diagnostics.json")["build"]["diagnostics"][0]["level"], "warning");
    // A broken build names its error under failures.
    let broken = read("broken-check.failures.json");
    assert_eq!(broken["outcome"], "build_failed");
    assert_eq!(broken["build"]["diagnostics"][0]["code"], "E0308");
    assert_eq!(broken["build"]["diagnostics"][0]["line"], 2);
}

#[test]
fn cargo_verdict_exits_with_cargo_s_own_code() {
    for (fixture, args, code) in [
        ("mixed", &["test", "--no-fail-fast"][..], 101),
        ("mixed", &["test", "--lib", "ok", "--json"][..], 0),
        ("broken", &["check", "--want", "summary"][..], 101),
    ] {
        let out = Command::new(cli())
            .arg("verdict")
            .args(args)
            .current_dir(manifest_dir().join("tests/fixtures").join(fixture))
            .env("CARGO_TARGET_DIR", target_dir(&format!("cli-{fixture}")))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(code), "{fixture} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        if args.contains(&"--json") {
            let v: Value = serde_json::from_slice(&out.stdout).expect("--json prints only JSON on stdout");
            assert_eq!(v["schema"], "cargo-verdict/1");
            assert_eq!(v["exit_code"], 0);
        }
    }
    // A non-verdict is a usage error, not a cargo run.
    let out = Command::new(cli()).args(["verdict", "fmt"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

// ─── A JSON Schema validator for the subset schemars 1 emits ──────────────────

fn resolve<'a>(root: &'a Value, r: &str) -> &'a Value {
    let name = r.strip_prefix("#/$defs/").unwrap_or_else(|| panic!("unsupported $ref {r}"));
    &root["$defs"][name]
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "null" => v.is_null(),
        "boolean" => v.is_boolean(),
        "string" => v.is_string(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        other => panic!("unsupported type {other}"),
    }
}

fn validate(root: &Value, s: &Value, v: &Value, at: &str) -> Result<(), String> {
    if s == &Value::Bool(true) {
        return Ok(());
    }
    let s = s.as_object().ok_or_else(|| format!("{at}: schema is not an object"))?;
    for k in s.keys() {
        assert!(
            matches!(
                k.as_str(),
                "$ref" | "$defs" | "$schema" | "title" | "description" | "type" | "properties" | "required"
                    | "additionalProperties" | "items" | "enum" | "const" | "oneOf" | "anyOf" | "format"
                    | "minimum"
            ),
            "the validator does not understand `{k}`; teach it before trusting it"
        );
    }
    if let Some(r) = s.get("$ref").and_then(Value::as_str) {
        validate(root, resolve(root, r), v, at)?;
    }
    if let Some(t) = s.get("type") {
        let ok = match t {
            Value::String(t) => type_ok(t, v),
            Value::Array(ts) => ts.iter().any(|t| type_ok(t.as_str().unwrap(), v)),
            _ => false,
        };
        if !ok {
            return Err(format!("{at}: {v} is not of type {t}"));
        }
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array) {
        if !e.contains(v) {
            return Err(format!("{at}: {v} not in {e:?}"));
        }
    }
    if let Some(c) = s.get("const") {
        if c != v {
            return Err(format!("{at}: {v} != const {c}"));
        }
    }
    if let Some(min) = s.get("minimum").and_then(Value::as_f64) {
        if v.as_f64().is_some_and(|n| n < min) {
            return Err(format!("{at}: {v} < {min}"));
        }
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(alts) = s.get(key).and_then(Value::as_array) {
            let n = alts.iter().filter(|a| validate(root, a, v, at).is_ok()).count();
            if n == 0 || (key == "oneOf" && n > 1) {
                return Err(format!("{at}: {n} of {key} match {v}"));
            }
        }
    }
    if let (Some(items), Some(arr)) = (s.get("items"), v.as_array()) {
        for (i, x) in arr.iter().enumerate() {
            validate(root, items, x, &format!("{at}[{i}]"))?;
        }
    }
    if let Some(obj) = v.as_object() {
        let props = s.get("properties").and_then(Value::as_object);
        if let Some(req) = s.get("required").and_then(Value::as_array) {
            for r in req {
                if !obj.contains_key(r.as_str().unwrap()) {
                    return Err(format!("{at}: missing required `{r}`"));
                }
            }
        }
        for (k, x) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(ps) => validate(root, ps, x, &format!("{at}.{k}"))?,
                None => match s.get("additionalProperties") {
                    Some(Value::Bool(false)) => return Err(format!("{at}: unexpected property `{k}`")),
                    Some(extra @ Value::Object(_)) => validate(root, extra, x, &format!("{at}.{k}"))?,
                    _ => {}
                },
            }
        }
    }
    Ok(())
}

#[test]
fn the_validator_rejects_what_it_should() {
    // Negative controls: a validator that passes everything proves nothing.
    let schema = schema();
    let good = serde_json::to_value(
        Verdict::collect(
            &parse(&["check"]),
            &plan(&parse(&["check"]), &PlanOptions::new("x")),
            &Demux::new(),
            exec::Exit::Code(0),
        ),
    )
    .unwrap();
    validate(&schema, &schema, &good, "$").unwrap();
    let mut bad = good.clone();
    bad["outcome"] = "sort_of".into();
    assert!(validate(&schema, &schema, &bad, "$").is_err());
    let mut bad = good.clone();
    bad.as_object_mut().unwrap().remove("build");
    assert!(validate(&schema, &schema, &bad, "$").is_err());
    let mut bad = good;
    bad["tests"]["totals"]["passed"] = "many".into();
    assert!(validate(&schema, &schema, &bad, "$").is_err());
}
