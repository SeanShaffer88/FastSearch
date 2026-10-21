//! Platform layer: walking a directory tree to build an index.
//!
//! v0.1 walks with `std::fs` — portable, correct, and testable on any OS.
//! Observed symlinks and Windows reparse entries are not traversed,
//! and unreadable directories are skipped and counted, never fatal.
//! The NTFS fast paths (USN Journal / MFT enumeration) are the v0.2 plan;
//! see ROADMAP.md. Concurrent replacement races are outside this survey's guarantees.

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
    /// Directory entries or metadata that could not be inspected.
    pub unreadable_entries: usize,
    /// Links/reparse entries listed without traversing their targets.
    pub skipped_links: usize,
}

/// Bounds for filesystem index construction (not a process-memory guarantee).
#[derive(Clone, Copy, Debug)]
pub struct WalkLimits {
    pub max_entries: usize,
    pub max_path_bytes: usize,
    pub max_depth: usize,
}
impl Default for WalkLimits {
    fn default() -> Self {
        Self {
            max_entries: 2_000_000,
            max_path_bytes: 256 * 1024 * 1024,
            max_depth: 256,
        }
    }
}

fn link_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

/// Walk with default bounds. Observed links/reparse entries are indexed but
/// never traversed. Concurrent replacements can race these checks; not a
/// filesystem snapshot or a containment boundary.
pub fn walk(root: &str) -> PResult<WalkResult> {
    walk_with_limits(root, WalkLimits::default())
}

pub fn walk_with_limits(root: &str, limits: WalkLimits) -> PResult<WalkResult> {
    walk_with_control(root, limits, |_| true)
}

/// Calls control(0) before filesystem access and then once per discovered entry.
/// Return false to cancel. Counts are monotonic, not completion percentages.
/// Calls are synchronous and OS I/O may block between checkpoints.
pub fn walk_with_control(
    root: &str,
    limits: WalkLimits,
    mut control: impl FnMut(u64) -> bool,
) -> PResult<WalkResult> {
    let cancelled = || {
        (
            crate::ERR_CANCELLED,
            "Filesystem scan cancelled; previous index retained".into(),
        )
    };
    if !control(0) {
        return Err(cancelled());
    }
    let mut examined = 0u64;
    let meta = std::fs::symlink_metadata(root)
        .map_err(|e| (ERR_NOT_FOUND, format!("cannot read root '{root}': {e}")))?;
    if link_or_reparse(&meta) || !meta.is_dir() {
        return Err((
            ERR_PLATFORM,
            format!("'{root}' must be a real directory, not a link/reparse entry"),
        ));
    }
    let first = std::fs::read_dir(root)
        .map_err(|e| (ERR_PLATFORM, format!("cannot enumerate root '{root}': {e}")))?;
    let mut out = WalkResult::default();
    let mut stack = vec![(std::path::PathBuf::from(root), first, 0usize)];
    let mut path_bytes = 0usize;
    while let Some((_, entries, depth)) = stack.last_mut() {
        let entry = match entries.next() {
            None => {
                stack.pop();
                continue;
            }
            Some(Err(_)) => {
                out.unreadable_entries += 1;
                continue;
            }
            Some(Ok(entry)) => entry,
        };
        examined += 1;
        if !control(examined) {
            return Err(cancelled());
        }
        let depth = *depth;
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => {
                out.unreadable_entries += 1;
                continue;
            }
        };
        let name = path.to_string_lossy().into_owned();
        path_bytes = path_bytes
            .checked_add(name.len())
            .ok_or((ERR_PLATFORM, "Path byte count overflow".into()))?;
        if out.paths.len() >= limits.max_entries || path_bytes > limits.max_path_bytes {
            return Err((
                ERR_PLATFORM,
                "Filesystem index size limit exceeded; choose a narrower root".into(),
            ));
        }
        let linked = link_or_reparse(&meta);
        let is_dir = meta.is_dir();
        out.paths.push((name, is_dir));
        if linked {
            out.skipped_links += 1;
        } else if is_dir {
            if depth >= limits.max_depth {
                return Err((
                    ERR_PLATFORM,
                    "Filesystem depth limit exceeded; choose a narrower root".into(),
                ));
            }
            match std::fs::read_dir(&path) {
                Ok(entries) => stack.push((path, entries, depth + 1)),
                Err(_) => out.unreadable_dirs += 1,
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
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let base = loop {
            let path = std::env::temp_dir().join(format!(
                "fs-walk-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("cannot create fixture: {e}"),
            }
        };
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
    fn bounds_fail_instead_of_returning_truncated_success() {
        let base = fixture();
        let root = base.to_str().unwrap();
        let baseline = walk(root).unwrap();
        let bytes = baseline.paths.iter().map(|(p, _)| p.len()).sum();
        let exact = WalkLimits {
            max_entries: baseline.paths.len(),
            max_path_bytes: bytes,
            max_depth: 10,
        };
        assert_eq!(
            walk_with_limits(root, exact).unwrap().paths.len(),
            baseline.paths.len()
        );
        for limits in [
            WalkLimits {
                max_entries: baseline.paths.len() - 1,
                ..exact
            },
            WalkLimits {
                max_path_bytes: bytes - 1,
                ..exact
            },
            WalkLimits {
                max_depth: 0,
                ..exact
            },
        ] {
            assert_eq!(walk_with_limits(root, limits).unwrap_err().0, ERR_PLATFORM);
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn root_links_are_rejected_and_nested_links_never_followed() {
        use std::os::unix::fs::symlink;
        let base = fixture();
        let outside = fixture();
        fs::write(outside.join("outside-secret"), b"outside").unwrap();
        symlink(&outside, base.join("external")).unwrap();
        symlink(&base, base.join("loop")).unwrap();
        assert!(walk(base.join("external").to_str().unwrap()).is_err());
        let result = walk(base.to_str().unwrap()).unwrap();
        assert_eq!(result.skipped_links, 2);
        assert!(result.paths.iter().any(|(p, _)| p.ends_with("external")));
        assert!(!result
            .paths
            .iter()
            .any(|(p, _)| p.contains("outside-secret")));
        fs::remove_dir_all(base).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn controlled_walk_cancels_before_io_and_mid_scan() {
        let initial =
            walk_with_control("nonexistent", WalkLimits::default(), |_| false).unwrap_err();
        assert_eq!(initial.0, crate::ERR_CANCELLED);
        let base = fixture();
        let mut counts = Vec::new();
        let result = walk_with_control(base.to_str().unwrap(), WalkLimits::default(), |count| {
            counts.push(count);
            count < 3
        });
        assert_eq!(result.unwrap_err().0, crate::ERR_CANCELLED);
        assert_eq!(counts, vec![0, 1, 2, 3]);
        let complete =
            walk_with_control(base.to_str().unwrap(), WalkLimits::default(), |_| true).unwrap();
        assert_eq!(
            complete.paths.len(),
            walk(base.to_str().unwrap()).unwrap().paths.len()
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_junction_targets_and_loops_are_not_traversed() {
        use std::path::{Path, PathBuf};
        struct Junction(PathBuf);
        impl Drop for Junction {
            fn drop(&mut self) {
                let _ = fs::remove_dir(&self.0);
            }
        }
        fn junction(link: &Path, target: &Path) -> Junction {
            let powershell =
                PathBuf::from(std::env::var_os("SystemRoot").expect("Windows SystemRoot"))
                    .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let result = std::process::Command::new(powershell)
                .args(["-NoProfile", "-NonInteractive", "-Command",
                    "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:FS_TEST_LINK -Target $env:FS_TEST_TARGET | Out-Null"])
                .env("FS_TEST_LINK", link).env("FS_TEST_TARGET", target)
                .output().expect("Run Windows junction fixture setup");
            assert!(
                result.status.success(),
                "Junction setup failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            Junction(link.to_path_buf())
        }
        let base = fixture();
        let outside = fixture();
        fs::write(outside.join("outside-secret"), b"preserve this").unwrap();
        // Paths with spaces exercise setup without interpolating paths into script.
        let external = junction(&base.join("external junction"), &outside);
        let looped = junction(&base.join("loop junction"), &base);
        assert!(walk(external.0.to_str().unwrap()).is_err());
        let result = walk(base.to_str().unwrap()).unwrap();
        assert_eq!(result.skipped_links, 2);
        assert!(!result
            .paths
            .iter()
            .any(|(p, _)| p.contains("outside-secret")));
        assert_eq!(
            fs::read(outside.join("outside-secret")).unwrap(),
            b"preserve this"
        );
        let handle = crate::ffi::fs_index_create();
        let root = std::ffi::CString::new(base.to_str().unwrap()).unwrap();
        let pointer = crate::ffi::fs_index_build_report_json(handle, root.as_ptr());
        assert!(!pointer.is_null());
        // SAFETY: the pointer is a successful caller-owned FFI result.
        let report = unsafe { std::ffi::CStr::from_ptr(pointer) }
            .to_str()
            .unwrap();
        assert!(report.contains("\"skipped_links\":2"));
        assert!(report.contains("\"partial\":true"));
        crate::ffi::fs_free_string(pointer);
        assert_eq!(crate::ffi::fs_index_free(handle), crate::FS_OK);
        drop(looped);
        drop(external);
        fs::remove_dir_all(base).unwrap();
        fs::remove_dir_all(outside).unwrap();
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
