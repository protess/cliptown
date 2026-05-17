//! P6 Theme B: structured tool surface tests.
//!
//! Uses safe shell commands (`echo`, `false`, `sleep`) for
//! deterministic behavior across CI hosts. The handler-level tests
//! that exercise `run_tests` / `lint_artifact` end-to-end live under
//! mcp_handlers (require workdir setup + task fixture).

use cliptown_world::agent_tools::{run_command, sniff_test_command, task_workdir, DEFAULT_TIMEOUT_SECS};

#[tokio::test]
async fn run_command_captures_stdout_and_exit_zero() {
    let cwd = std::env::temp_dir();
    let out = run_command("sh", &["-c", "echo hello world"], &cwd, DEFAULT_TIMEOUT_SECS)
        .await
        .unwrap();
    assert_eq!(out.exit_code, Some(0));
    assert!(out.stdout_tail.contains("hello world"), "got {}", out.stdout_tail);
    assert!(!out.timed_out);
}

#[tokio::test]
async fn run_command_captures_stderr_and_nonzero_exit() {
    let cwd = std::env::temp_dir();
    let out = run_command(
        "sh",
        &["-c", "echo oops >&2 && exit 7"],
        &cwd,
        DEFAULT_TIMEOUT_SECS,
    )
    .await
    .unwrap();
    assert_eq!(out.exit_code, Some(7));
    assert!(out.stderr_tail.contains("oops"));
    assert!(!out.timed_out);
}

#[tokio::test]
async fn run_command_times_out_after_deadline() {
    let cwd = std::env::temp_dir();
    let out = run_command("sleep", &["5"], &cwd, 1).await.unwrap();
    assert!(out.timed_out, "sleep 5 with 1s timeout must timeout");
    assert!(out.exit_code.is_none() || out.exit_code != Some(0));
    assert!(out.elapsed_ms >= 900, "elapsed_ms should ≈ 1000ms");
}

#[tokio::test]
async fn run_command_caps_oversize_stdout() {
    let cwd = std::env::temp_dir();
    // 64KiB of stdout — way over the 16KiB cap.
    let out = run_command(
        "sh",
        &["-c", "dd if=/dev/zero bs=1024 count=64 2>/dev/null | tr '\\000' 'A'"],
        &cwd,
        DEFAULT_TIMEOUT_SECS,
    )
    .await
    .unwrap();
    assert_eq!(out.exit_code, Some(0));
    // Tail is capped at 16KiB; we should see fewer than 32KiB chars.
    assert!(
        out.stdout_tail.len() <= 17 * 1024,
        "stdout tail should be capped to ~16KiB, got {}",
        out.stdout_tail.len()
    );
}

#[test]
fn task_workdir_uses_relative_layout() {
    let p = task_workdir("s1", "T1");
    assert_eq!(p.to_string_lossy(), "workspaces/s1/T1/workdir");
}

#[test]
fn sniff_test_command_picks_cargo_for_rust_workdir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"").unwrap();
    let (program, args) = sniff_test_command(dir.path());
    assert_eq!(program, "cargo");
    assert_eq!(args, vec!["test", "--quiet"]);
}

#[test]
fn sniff_test_command_picks_pnpm_for_node_workdir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    let (program, args) = sniff_test_command(dir.path());
    assert_eq!(program, "pnpm");
    assert_eq!(args, vec!["test"]);
}

#[test]
fn sniff_test_command_falls_back_to_no_op_shell() {
    let dir = tempfile::tempdir().unwrap();
    let (program, args) = sniff_test_command(dir.path());
    assert_eq!(program, "sh");
    assert_eq!(args[0], "-c");
    assert!(args[1].contains("exit 0"));
}
