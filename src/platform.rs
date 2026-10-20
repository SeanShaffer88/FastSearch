//! Platform layer: walking a directory tree to build an index.
//!
//! v0.1 walks with `std::fs` — portable, correct, and testable on any OS.
//! It does NOT follow symlinks (a link loop can never hang an index build),
//! and unreadable directories are skipped and counted, never fatal.
//! The NTFS fast paths (USN Journal / MFT enumeration) are the v0.2 plan;
//! see ROADMAP.md. Nothing in this layer is Windows-only in v0.1.

use crate::{ERR_NOT_FOUND, ERR_PLATFORM};

/// A platform result: value, or (error code, human message).
pub type PResult<T> = Result<T, (i32, String)>;

#[derive(Debug, Default)]
pub struct WalkResult {
    /// (path, is_dir) for every file and directory under the root,
    /// including the root's immediate children at every depth.
    pub paths: Vec<(String, bool)>,
    /// Directories that could not be read (skipped, not fatal).
    pub unreadable_dirs: usize,
}

/// Walk `root` recursively. Errors only when the root itself is unusable.
pub fn walk(root: &str) -> PResult<WalkResult> {
    let meta = std::fs::metadata(root)
        .map_err(|e| (ERR_NOT_FOUND, format!("cannot read root '{root}': {e}")))?;
    if !meta.is_dir() {
        return Err((ERR_PLATFORM, format!("'{root}' is not a directory")));
    }
    let mut out = WalkResult::default();
    let mut stack: Vec<std::path::PathBuf> = vec![std::path::PathBuf::from(root)];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => {
                out.unreadable_dirs += 1;
                continue;
            }
        };
        for entry in rd.flatten() {
            let path = entry.path();
            // file_type() does not follow symlinks — exactly what we want.
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            let is_dir = ft.is_dir();
            let is_link = ft.is_symlink();
            out.paths
                .push((path.to_string_lossy().into_owned(), is_dir));
            if is_dir && !is_link {
                stack.push(path);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!("fs-walk-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("sub/deep")).unwrap();
        fs::write(base.join("alpha.txt"), "a").unwrap();
        fs::write(base.join("sub/beta.rs"), "b").unwrap();
        fs::write(base.join("sub/deep/gamma.txt"), "g").unwrap();
        base
    }

    #[test]
    fn walks_recursively() {
        let base = fixture();
        let r = walk(&base.to_string_lossy()).unwrap();
        let names: Vec<&str> = r
            .paths
            .iter()
            .map(|(p, _)| p.rsplit(['/', '\\']).next().unwrap())
            .collect();
        assert!(names.contains(&"alpha.txt"));
        assert!(names.contains(&"beta.rs"));
        assert!(names.contains(&"gamma.txt"));
        assert!(names.contains(&"sub"));
        assert!(r.paths.iter().any(|(p, d)| p.ends_with("sub") && *d));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn bad_root_is_an_error() {
        assert!(walk("/definitely/not/a/real/path/xyz").is_err());
        let f = std::env::temp_dir().join("fs-walk-a-file.txt");
        fs::write(&f, "x").unwrap();
        assert!(walk(&f.to_string_lossy()).is_err());
        let _ = fs::remove_file(&f);
    }
}
