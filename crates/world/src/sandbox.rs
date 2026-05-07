//! Sandbox path resolver. The single most security-sensitive path in the world.
//! Rejects any path that could escape the per-startup workspace root.
//! All file operations from CLI agents (M3+) and operator artifact reads route here.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

/// Attack rejection rules (spec §6.3, §10):
/// - Empty path
/// - NUL byte
/// - Path > 4096 bytes
/// - Absolute path (Unix or Windows drive-letter or UNC)
/// - Bidi/RTL Unicode controls (homograph attacks)
/// - Non-NFC normalization
/// - Trailing dot or trailing space (Windows legacy quirks)
/// - `..` traversal that escapes root after `realpath`
/// - Symlinks pointing outside root
pub fn resolve(root: &Path, candidate: &str) -> Result<PathBuf> {
    if candidate.is_empty() { return Err(anyhow!("empty path")); }
    if candidate.contains('\0') { return Err(anyhow!("nul byte in path")); }
    if candidate.len() > 4096 { return Err(anyhow!("path too long")); }

    let p = Path::new(candidate);
    if p.is_absolute() { return Err(anyhow!("absolute path forbidden")); }

    // Windows-style absolute (drive letter or UNC)
    if candidate.len() >= 2 && candidate.chars().nth(1) == Some(':') {
        return Err(anyhow!("windows absolute"));
    }
    if candidate.starts_with("\\\\") || candidate.starts_with("//") {
        return Err(anyhow!("UNC path forbidden"));
    }

    // Unicode RTL override + bidi controls
    const FORBIDDEN_CTRL: &[char] = &[
        '\u{202E}', '\u{202D}', '\u{202A}', '\u{202B}', '\u{202C}',
        '\u{200E}', '\u{200F}',
    ];
    if candidate.chars().any(|c| FORBIDDEN_CTRL.contains(&c)) {
        return Err(anyhow!("bidi control char"));
    }

    // Require NFC normalization (reject anything that would change under NFC)
    use unicode_normalization::UnicodeNormalization;
    let nfc: String = candidate.nfc().collect();
    if nfc != candidate { return Err(anyhow!("non-NFC path")); }

    // Trailing dot/space (Windows legacy quirks)
    if candidate.ends_with('.') || candidate.ends_with(' ') {
        return Err(anyhow!("trailing dot or space"));
    }

    let joined = root.join(p);
    let canon_root = root.canonicalize().map_err(|e| anyhow!("root canonicalize: {e}"))?;

    // realpath the joined path; if it doesn't exist yet, realpath the parent and append the basename.
    let canon = match joined.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            let parent = joined.parent().ok_or_else(|| anyhow!("no parent"))?;
            let parent_c = parent.canonicalize().map_err(|e| anyhow!("parent canonicalize: {e}"))?;
            if !parent_c.starts_with(&canon_root) {
                return Err(anyhow!("parent escapes root"));
            }
            parent_c.join(joined.file_name().ok_or_else(|| anyhow!("no file name"))?)
        }
    };

    if !canon.starts_with(&canon_root) {
        return Err(anyhow!("path escapes root"));
    }
    Ok(canon)
}
