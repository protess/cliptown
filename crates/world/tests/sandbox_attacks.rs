use cliptown_world::sandbox::resolve;
use std::fs;
use std::os::unix::fs::symlink;
use tempfile::tempdir;

fn root() -> tempfile::TempDir {
    let d = tempdir().unwrap();
    fs::create_dir_all(d.path().join("artifacts")).unwrap();
    d
}

#[test]
fn rejects_empty() {
    let d = root();
    assert!(resolve(d.path(), "").is_err());
}

#[test]
fn rejects_dot_dot_escape() {
    let d = root();
    assert!(resolve(d.path(), "../etc/passwd").is_err());
    assert!(resolve(d.path(), "../../etc/passwd").is_err());
    assert!(resolve(d.path(), "././../etc/passwd").is_err());
}

#[test]
fn rejects_unix_absolute() {
    let d = root();
    assert!(resolve(d.path(), "/etc/passwd").is_err());
}

#[test]
fn rejects_windows_drive_letter() {
    let d = root();
    assert!(resolve(d.path(), "C:\\Windows\\System32").is_err());
    assert!(resolve(d.path(), "Z:notes").is_err());
}

#[test]
fn rejects_unc_path() {
    let d = root();
    assert!(resolve(d.path(), "\\\\server\\share\\evil").is_err());
    assert!(resolve(d.path(), "//server/share/evil").is_err());
}

#[test]
fn rejects_nul_byte() {
    let d = root();
    assert!(resolve(d.path(), "artifacts/foo\0.md").is_err());
}

#[test]
fn rejects_too_long() {
    let d = root();
    let long = "a".repeat(5000);
    assert!(resolve(d.path(), &long).is_err());
}

#[test]
fn rejects_bidi_rtl_override() {
    let d = root();
    // U+202E RIGHT-TO-LEFT OVERRIDE
    let attack = format!("artifacts/foo{}gpj.exe", '\u{202E}');
    assert!(resolve(d.path(), &attack).is_err());
}

#[test]
fn rejects_non_nfc() {
    let d = root();
    // Decomposed e + combining acute (NFD) — NFC form is precomposed é
    let nfd = "artifacts/cafe\u{0301}.md";
    assert!(resolve(d.path(), nfd).is_err());
}

#[test]
fn rejects_trailing_dot() {
    let d = root();
    assert!(resolve(d.path(), "artifacts/foo.").is_err());
}

#[test]
fn rejects_trailing_space() {
    let d = root();
    assert!(resolve(d.path(), "artifacts/foo ").is_err());
}

#[test]
fn rejects_symlink_escape() {
    let d = root();
    let outside = d.path().parent().unwrap().to_path_buf();
    symlink(&outside, d.path().join("artifacts").join("link")).unwrap();
    assert!(resolve(d.path(), "artifacts/link/passwd").is_err());
}

#[test]
fn allows_legit_artifact_path() {
    let d = root();
    let r = resolve(d.path(), "artifacts/T1.md").unwrap();
    assert!(r.starts_with(d.path().canonicalize().unwrap()));
}

#[test]
fn allows_nested_legit_path() {
    let d = root();
    fs::create_dir_all(d.path().join("artifacts").join("subdir")).unwrap();
    let r = resolve(d.path(), "artifacts/subdir/file.md").unwrap();
    assert!(r.starts_with(d.path().canonicalize().unwrap()));
}
