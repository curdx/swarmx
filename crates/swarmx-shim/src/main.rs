//! swarmx-shim — PTY-internal wrapper that reports lifecycle to the host
//! via OSC sequences (modeled after golutra's shim.rs).
//!
//!  argv[1..] = real CLI command + args.
//!
//! Wire protocol with the host (via the captured PTY byte stream):
//!
//!     \x1b]633;A\x07          → child is spawning (emitted before exec)
//!     \x1b]633;D;<code>\x07   → child exited with `<code>`
//!
//! The `]633` form is the iTerm2 / VSCode shell-integration namespace; it's
//! ignored by terminals that don't speak it, and our front-end recognises it
//! out-of-band without disturbing the visible stream.
//!
//! Why a separate binary instead of spawning `claude` directly:
//!   * cross-platform `wait()` semantics differ; this gives us *one* exit code
//!     channel the host can rely on.
//!   * the OSC sequence lands in the asciicast recording, so replays see the
//!     same lifecycle markers as the live session.
//!   * future: hangs / OOM diagnostics, env-sanitisation, etc.

use std::env;
use std::io::{self, Write};
use std::process::{Command, Stdio};

const OSC_READY: &str = "\x1b]633;A\x07";
const OSC_EXIT_PREFIX: &str = "\x1b]633;D;";

/// Prefix the host uses to distinguish shim-emitted launch errors from
/// errors printed by the real CLI itself.
const SHIM_LAUNCH_ERROR_MARKER: &str = "SHIM_LAUNCH_ERROR";

fn main() {
    let mut args = env::args();
    let _shim = args.next();
    let target = match args.next() {
        Some(v) => v,
        None => {
            eprintln!("{SHIM_LAUNCH_ERROR_MARKER}: no target command");
            std::process::exit(101);
        }
    };
    let target_args: Vec<String> = args.collect();

    // Emit the ready marker *before* spawn so the host can flip its
    // "spawning → ready" state even if the child takes a moment to print
    // its first prompt.
    print!("{OSC_READY}");
    let _ = io::stdout().flush();

    let child = Command::new(&target)
        .args(&target_args)
        // Inherit the PTY's stdio so the child sees `isatty()==true` and
        // the CLI launches its interactive (OAuth-capable) flow rather
        // than a non-interactive degraded mode.
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();

    match child {
        Ok(mut child) => {
            let status = match child.wait() {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("{SHIM_LAUNCH_ERROR_MARKER}: wait error='{err}'");
                    std::process::exit(103);
                }
            };
            let code = status.code().unwrap_or_else(|| {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(sig) = status.signal() {
                        return 128 + sig;
                    }
                }
                if status.success() {
                    0
                } else {
                    1
                }
            });
            print!("{OSC_EXIT_PREFIX}{code}\x07");
            let _ = io::stdout().flush();
            std::process::exit(code);
        }
        Err(err) => {
            eprintln!(
                "{SHIM_LAUNCH_ERROR_MARKER}: command='{target}' error='{err}'"
            );
            std::process::exit(102);
        }
    }
}
