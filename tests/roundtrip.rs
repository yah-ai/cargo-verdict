//! The round-trip property: for every argv, `parse ∘ render` is the identity
//! on what `parse` returns, so `parse(render(parse(x))) == parse(x)` and the
//! canonical rendering is itself a fixed point.
//!
//! Generated, not enumerated: random argvs drawn from a vocabulary built to be
//! hostile — every verb word, every modelled flag in every spelling, values
//! that look like flags, valueless value flags, `--` in odd places, unknown
//! flags beside positionals, toolchains out of position. A seeded xorshift
//! keeps it deterministic and dependency-free; a failure prints its argv.

use cargo_verdict::{parse, render, VERB_WORDS};

const FLAGS: &[&str] = &[
    // selection / targets / features / profile
    "-p", "-pfoo", "-p=foo", "--package", "--package=bar", "--workspace", "--all", "--exclude",
    "--exclude=baz", "--lib", "--bins", "--bin", "--bin=x", "--examples", "--example", "--tests",
    "--test", "--test=it", "--benches", "--bench", "--all-targets", "--doc", "--features",
    "--features=a,b", "-F", "-Fc", "--all-features", "--no-default-features", "--profile",
    "--profile=ci-dev", "--cargo-profile", "--release", "-r", "--target", "--target=x86",
    "--target-dir", "--target-dir=t", "--manifest-path", "--manifest-path=M", "--message-format",
    "--message-format=json", "--config", "--config=k=v", "-Z", "-Zflag", "-C", "-Cdir",
    // test
    "--no-run", "--no-fail-fast", "--exact", "--skip", "--skip=s", "--ignored",
    "--include-ignored", "--test-threads", "--test-threads=3", "--test-threads=x", "-j", "-j4",
    "--nocapture", "--no-capture", "--run-ignored", "--run-ignored=all", "--run-ignored=bogus",
    // help / version / globals
    "-h", "--help", "-V", "--version", "--list", "--explain", "--color", "-v", "-vv", "--locked",
    // unmodelled, known and unknown
    "--format", "--show-output", "-E", "--frob", "--frob=1", "-x", "--fix", "--open",
    "--", "--", "--",
];

const WORDS: &[&str] = &[
    "foo", "bar", "1", "json", "r", "run", "list", "archive", "self", "show-config", "-", "",
    "+nightly", "+stable", "a,b", "-1", "--weird", "=x",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

fn random_argv(rng: &mut Rng) -> Vec<String> {
    let len = rng.below(14);
    let mut argv = Vec::with_capacity(len);
    // Usually lead with a verb, sometimes with globals first, sometimes none.
    if rng.below(4) != 0 {
        for _ in 0..rng.below(3) {
            argv.push(rng.pick(FLAGS).to_string());
        }
        argv.push(rng.pick(VERB_WORDS).to_string());
    }
    for _ in 0..len {
        let tok = match rng.below(10) {
            0..=5 => rng.pick(FLAGS),
            6..=8 => rng.pick(WORDS),
            _ => rng.pick(VERB_WORDS),
        };
        argv.push(tok.to_string());
    }
    argv
}

#[track_caller]
fn assert_fixed_point(argv: &[String]) {
    let first = parse(argv);
    let rendered = render(&first);
    let second = parse(&rendered);
    assert_eq!(
        second, first,
        "\nparse(render(parse(x))) != parse(x)\n  x       = {argv:?}\n  render  = {rendered:?}"
    );
    assert_eq!(render(&second), rendered, "render is not canonical for {argv:?}");
    assert_eq!(second.class(), first.class(), "class moved across a render of {argv:?}");
}

#[test]
fn parse_then_render_then_parse_is_a_fixed_point() {
    for seed in 1..=40_000u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        assert_fixed_point(&random_argv(&mut rng));
    }
}

#[test]
fn every_verb_word_round_trips_alone_and_with_help() {
    for w in VERB_WORDS {
        assert_fixed_point(&[w.to_string()]);
        assert_fixed_point(&[w.to_string(), "--help".into()]);
        assert_fixed_point(&["-h".into(), w.to_string()]);
    }
}
