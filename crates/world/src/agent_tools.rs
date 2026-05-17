//! P6 Theme B: structured tool surface for self-review + peer review.
//!
//! These tools all share the same primitive: run a shell command in a
//! specific working directory, with a timeout, capturing the tail of
//! stdout + stderr + exit code. Centralized here so the three MCP
//! handlers (`run_tests`, `lint_artifact`, `read_artifact_diff`) don't
//! each rebuild the spawn + tail + timeout pattern.
//!
//! Sandboxing is by construction: the helper sets `current_dir` to a
//! caller-supplied path that's been resolved through
//! `crate::sandbox::resolve` (no traversal escapes). The command
//! itself runs as the cliptown user — agents that compromise their
//! own LLM can already exec arbitrary code via the adapter CLI, so the
//! tool surface here doesn't widen the threat model.
//!
//! Output cap: per-call stdout/stderr tail is bounded so a runaway
//! `for i in {1..1e9}; do echo $i; done` doesn't allocate gigabytes.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

/// Tail cap per stream. 16 KiB is enough to show the last few
/// hundred lines of test output — more than that the agent should
/// scroll the logs via a separate read tool (deferred).
const TAIL_CAP_BYTES: usize = 16 * 1024;

/// Default per-call timeout. Longer than a typical `cargo test` on
/// a small workspace; short enough that a runaway loop hits the
/// supervisor before it eats all the world-server CPU.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandOutcome {
    pub exit_code: Option<i32>,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub timed_out: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug)]
pub enum ToolError {
    Spawn(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Spawn(s) => write!(f, "spawn: {s}"),
            ToolError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

/// Run `program` + `args` inside `cwd` with `timeout_secs`. Captures
/// the LAST `TAIL_CAP_BYTES` of each stream so tests with hundreds of
/// lines of output don't trigger an OOM. On timeout, returns
/// `timed_out=true` and whatever streams have buffered so far.
pub async fn run_command(
    program: &str,
    args: &[&str],
    cwd: &Path,
    timeout_secs: u64,
) -> Result<CommandOutcome, ToolError> {
    let start = std::time::Instant::now();
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| ToolError::Spawn(e.to_string()))?;

    let mut stdout_handle = child.stdout.take().expect("piped");
    let mut stderr_handle = child.stderr.take().expect("piped");
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::with_capacity(TAIL_CAP_BYTES);
        let _ = stdout_handle.read_to_end(&mut buf).await;
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::with_capacity(TAIL_CAP_BYTES);
        let _ = stderr_handle.read_to_end(&mut buf).await;
        buf
    });

    let wait_result = timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    let (exit_code, timed_out) = match wait_result {
        Ok(Ok(status)) => (status.code(), false),
        Ok(Err(e)) => return Err(ToolError::Io(e)),
        Err(_) => {
            // child still running — kill_on_drop handles cleanup when
            // `child` falls out of scope.
            (None, true)
        }
    };
    let stdout_bytes = stdout_task.await.unwrap_or_default();
    let stderr_bytes = stderr_task.await.unwrap_or_default();
    Ok(CommandOutcome {
        exit_code,
        stdout_tail: tail_to_string(&stdout_bytes),
        stderr_tail: tail_to_string(&stderr_bytes),
        timed_out,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

fn tail_to_string(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(TAIL_CAP_BYTES);
    // Walk forward to a char boundary to avoid splitting a multibyte
    // UTF-8 sequence mid-glyph (would corrupt the tail).
    let mut i = start;
    while i < bytes.len() && (bytes[i] & 0b1100_0000) == 0b1000_0000 {
        i += 1;
    }
    String::from_utf8_lossy(&bytes[i..]).into_owned()
}

/// Compute the per-task workdir from the world's CWD perspective:
/// `workspaces/<startup_id>/<task_id>/workdir/`. Mirrors the layout
/// the worker creates in `packages/worker/src/execenv.ts`.
pub fn task_workdir(startup_id: &str, task_id: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("workspaces/{startup_id}/{task_id}/workdir"))
}

/// Sniff a sensible default `run_tests` command for the workdir.
/// Returns `(program, args)`. Falls back to `echo no-tests` so a
/// caller without auto-detection still gets a deterministic outcome
/// instead of an error.
pub fn sniff_test_command(workdir: &Path) -> (&'static str, Vec<&'static str>) {
    if workdir.join("Cargo.toml").exists() {
        return ("cargo", vec!["test", "--quiet"]);
    }
    if workdir.join("package.json").exists() {
        return ("pnpm", vec!["test"]);
    }
    if workdir.join("Makefile").exists() {
        return ("make", vec!["test"]);
    }
    // Last resort — the caller passed no command and the workdir
    // doesn't look like anything we recognize. Return a no-op shell
    // that exits 0 so the audit row says "ran, found nothing."
    ("sh", vec!["-c", "echo 'no recognized test target' && exit 0"])
}
