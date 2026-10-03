//! Names and paths (§5.2).
//!
//! - Depth 1: a path is one element.
//! - `[A-Za-z0-9._-]`, 1–128 bytes, not starting with `.` or `-`, not ending with `.`, no `..`.
//! - Case-folded uniqueness, so a bundle cannot clobber itself on macOS or Windows.
//! - No Windows device name (`CON`, `PRN`, `AUX`, `NUL`, `COM0`–`COM9`, `LPT0`–`LPT9`), with any
//!   extension.

use std::collections::BTreeSet;

use crate::Refusal;

pub const MAX_NAME_BYTES: usize = 128;

/// Checks one file name.
pub fn check_name(name: &str) -> Result<(), Refusal> {
    let b = name.as_bytes();
    let bad = |why: &'static str| Err(Refusal::Name { name: name.to_owned(), why });
    if b.is_empty() || b.len() > MAX_NAME_BYTES {
        return bad("length is not 1–128 bytes");
    }
    if !b.iter().all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')) {
        return bad("a byte outside [A-Za-z0-9._-] (no directories, no spaces)");
    }
    if b[0] == b'.' || b[0] == b'-' {
        return bad("starts with `.` or `-`");
    }
    if b[b.len() - 1] == b'.' {
        return bad("ends with `.`");
    }
    if name.contains("..") {
        return bad("contains `..`");
    }
    if is_windows_device_name(name) {
        return bad("a Windows device name");
    }
    Ok(())
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM0`–`COM9`, `LPT0`–`LPT9`, in any case, with any extension.
pub fn is_windows_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit())
}

/// Checks every name and that no two differ only in case.
pub fn check_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<(), Refusal> {
    let mut folded = BTreeSet::new();
    for n in names {
        check_name(n)?;
        if !folded.insert(n.to_ascii_lowercase()) {
            return Err(Refusal::CaseCollision(n.to_owned()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in
            ["LICENSE", "README.md", "model-00001-of-00002.safetensors", "a", "x_y.gguf", "qwen25-1.5b-a16.palwart"]
        {
            assert!(check_name(ok).is_ok(), "{ok}");
        }
        let long = "a".repeat(129);
        for bad in [
            "",
            ".hidden",
            "-rf",
            "dot.",
            "a..b",
            "../x",
            "a/b",
            "a\\b",
            "sp ace",
            "ü.gguf",
            "CON",
            "con.txt",
            "Com1.json",
            "LPT9",
            "nul.gguf",
            &long,
            "a\0b",
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        assert!(check_name("COM10").is_ok());
        assert!(check_name("CONFIG.json").is_ok());
    }

    #[test]
    fn case_collisions() {
        assert!(matches!(check_names(["README.md", "readme.md"]), Err(Refusal::CaseCollision(_))));
        assert!(check_names(["README.md", "LICENSE"]).is_ok());
    }
}
