//! cargo-verdict: structured results for cargo's build and test verdicts.
//!
//! W371 §4.5. Three faces — this library, the `cargo verdict` CLI, and
//! a versioned output schema ([`verdict::schema`]) — over four modules:
//!
//! 1. **Request model + argv parser** ([`Request`], [`parse`], [`render`]).
//!    One normalized shape for a cargo invocation, read the same way whether it
//!    arrived as a shell line or a typed request. `parse` is total, `render` is
//!    canonical, and `parse(render(parse(x))) == parse(x)` for every argv. The
//!    [`Class`] (verdict / tenancy / mutation / query) falls out of the verb.
//! 2. **Execution plan** ([`plan()`]): adds output and runner flags only, and
//!    says which in [`Plan::added`]. [`exec::run`] runs one.
//! 3. **Demux + parsers** ([`Demux`]): cargo messages via `cargo_metadata`,
//!    libtest events via the versioned adapter in [`libtest`], raw kept.
//! 4. **Result schema `cargo-verdict/1`** ([`Verdict`], [`verdict::schema`]),
//!    with `want` projections as views over one structure.

pub mod demux;
pub mod exec;
pub mod libtest;
mod parse;
pub mod plan;
pub mod runner;
mod render;
mod request;
mod verb;
pub mod verdict;

pub use parse::parse;
pub use render::render;
pub use request::{Features, Globals, Ignored, Request, Selection, Targets, TestArgs};
pub use verb::{Class, Verb, VERB_WORDS};
pub use demux::{BinaryKind, BinaryRun, Captured, Demux, Stream};
pub use libtest::TestEvent;
pub use plan::{plan, Plan, PlanOptions};
pub use verdict::{Outcome, Verdict, Want, SCHEMA};
/// Re-exported so consumers name the same `Message` type the demux returns.
pub use cargo_metadata;
