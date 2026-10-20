//! Run a [`Plan`] and hand back its lines in arrival order.
//!
//! Lines are split on `\n`; bytes that are not UTF-8 are replaced, which is
//! the one lossy step, and only in what this returns — the exit code is
//! cargo's own, never invented.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::demux::Stream;
use crate::plan::Plan;

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// cargo exited with this code.
    Code(i32),
    /// cargo was ended by a signal (its number, where the platform has
    /// them); there is no code to report.
    Signalled(Option<i32>),
}

/// Run `plan` with `cargo` (or `$CARGO`, as cargo sets for subcommands) in
/// `cwd`, calling `on_line` for each line as it arrives.
///
/// cargo's stdout and stderr share **one pipe**, so every line arrives in the
/// exact order it was written and is reported as [`Stream::Combined`]. Two
/// pipes read on two threads race: on 2026-10-17 a loaded run (this crate's
/// golden test, beside the engine test) reported a fixture's unittests binary
/// as unstructured with no counts, its libtest JSON having been read before
/// cargo's `Running` line for it. With one pipe, cargo's
/// `Running` line (written before it spawns the binary) always comes first.
pub fn run(plan: &Plan, cwd: Option<&Path>, mut on_line: impl FnMut(Stream, &str)) -> std::io::Result<Exit> {
    // `$CARGO` is the real binary; a `+toolchain` needs rustup's proxy.
    let cargo = match plan.request.toolchain {
        Some(_) => "cargo".into(),
        None => std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
    };
    let (pipe, writer) = std::io::pipe()?;
    let mut cmd = Command::new(cargo);
    cmd.args(&plan.argv)
        .envs(&plan.request.env_allowlist)
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd.spawn()?;
    // Drop our copies of the write end (moved into `cmd`), so the read below
    // ends when cargo and its children close theirs.
    drop(cmd);
    let mut r = BufReader::new(pipe);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if r.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        }
        on_line(Stream::Combined, &String::from_utf8_lossy(&buf));
    }
    let status = child.wait()?;
    Ok(match status.code() {
        Some(c) => Exit::Code(c),
        None => Exit::Signalled(signal(&status)),
    })
}

#[cfg(unix)]
fn signal(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

#[cfg(not(unix))]
fn signal(_: &std::process::ExitStatus) -> Option<i32> {
    None
}
