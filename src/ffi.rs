//! C ABI surface — everything declared in `include/fastsearch.h`.
//!
//! Conventions (house standard, from WinRecovery):
//! - `*mut c_char` results are caller-owned; free with [`fs_free_string`].
//! - Index handles are opaque `u64`s issued by [`fs_index_create`] /
//!   [`fs_index_load`]; 0 is never a valid handle. Free with
//!   [`fs_index_free`]. Handles are thread-safe (the registry is a
//!   process-wide mutex map).
//! - Failures return null / 0 / an `ERR_*` code plus a thread-local
//!   last-error message ([`fs_last_error`]).
//! - Nothing unwinds across the FFI boundary.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::platform;
use crate::{Index, ERR_INTERNAL, ERR_INVALID_ARG, ERR_NOT_FOUND, ERR_PLATFORM, FS_OK};

thread_local! {
    static LAST_ERROR: RefCell<(i32, CString)> =
        RefCell::new((FS_OK, CString::new("").unwrap()));
}

fn set_error(code: i32, msg: &str) {
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = (
            code,
            CString::new(msg).unwrap_or_else(|_| CString::new("error").unwrap()),
        );
    });
}

fn clear_error() {
    set_error(FS_OK, "");
}

fn registry() -> &'static Mutex<HashMap<u64, Index>> {
    static REG: OnceLock<Mutex<HashMap<u64, Index>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

fn new_handle(index: Index) -> u64 {
    let h = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
    registry().lock().unwrap().insert(h, index);
    h
}

fn with_index<R>(handle: u64, f: impl FnOnce(&Index) -> R) -> Result<R, (i32, String)> {
    let reg = registry().lock().unwrap();
    let idx = reg
        .get(&handle)
        .ok_or((ERR_NOT_FOUND, format!("no such index handle: {handle}")))?;
    Ok(f(idx))
}

fn require_str(p: *const c_char, what: &str) -> Result<String, (i32, String)> {
    if p.is_null() {
        return Err((ERR_INVALID_ARG, format!("{what} must not be null")));
    }
    // SAFETY: caller contract — valid NUL-terminated UTF-8 or null.
    let s = unsafe { CStr::from_ptr(p) };
    s.to_str()
        .map(|v| v.to_string())
        .map_err(|_| (ERR_INVALID_ARG, format!("{what} is not valid UTF-8")))
}

fn into_c_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => {
            set_error(ERR_INTERNAL, "result contained an interior NUL byte");
            std::ptr::null_mut()
        }
    }
}

fn guard<T>(f: impl FnOnce() -> Result<T, (i32, String)>) -> Result<T, (i32, String)> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => Err((
            ERR_INTERNAL,
            "internal panic caught at the FFI boundary".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Version & errors
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn fs_version() -> *const c_char {
    static VERSION_C: &[u8] = b"0.1.0\0";
    VERSION_C.as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn fs_version_major() -> u32 {
    crate::VERSION_MAJOR
}

#[no_mangle]
pub extern "C" fn fs_version_minor() -> u32 {
    crate::VERSION_MINOR
}

#[no_mangle]
pub extern "C" fn fs_version_patch() -> u32 {
    crate::VERSION_PATCH
}

#[no_mangle]
pub extern "C" fn fs_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().1.as_ptr())
}

#[no_mangle]
pub extern "C" fn fs_last_error_code() -> c_int {
    LAST_ERROR.with(|e| e.borrow().0) as c_int
}

/// # Safety (caller contract): `p` is null or a string this library
/// returned and has not been freed yet. Documented in the header.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[no_mangle]
pub extern "C" fn fs_free_string(p: *mut c_char) {
    if !p.is_null() {
        // SAFETY: per the caller contract above.
        unsafe {
            drop(CString::from_raw(p));
        }
    }
}

// ---------------------------------------------------------------------------
// Index lifecycle
// ---------------------------------------------------------------------------

/// Create an empty index; returns its handle (never 0).
#[no_mangle]
pub extern "C" fn fs_index_create() -> u64 {
    new_handle(Index::default())
}

/// Free an index. Returns FS_OK, or ERR_NOT_FOUND for a bad handle.
#[no_mangle]
pub extern "C" fn fs_index_free(handle: u64) -> c_int {
    match registry().lock().unwrap().remove(&handle) {
        Some(_) => {
            clear_error();
            FS_OK as c_int
        }
        None => {
            set_error(ERR_NOT_FOUND, &format!("no such index handle: {handle}"));
            ERR_NOT_FOUND as c_int
        }
    }
}

/// Number of entries in an index, or -1 on error (see fs_last_error).
#[no_mangle]
pub extern "C" fn fs_index_count(handle: u64) -> i64 {
    match guard(|| with_index(handle, |i| i.entries.len() as i64)) {
        Ok(n) => {
            clear_error();
            n
        }
        Err((c, m)) => {
            set_error(c, &m);
            -1
        }
    }
}

/// Walk `root_utf8` and REPLACE the index's contents with what is found.
/// Returns FS_OK or an error code. Unreadable subdirectories are skipped
/// (the build still succeeds); a bad root fails.
#[no_mangle]
pub extern "C" fn fs_index_build(handle: u64, root_utf8: *const c_char) -> c_int {
    let root = match require_str(root_utf8, "root") {
        Ok(r) => r,
        Err((c, m)) => {
            set_error(c, &m);
            return c as c_int;
        }
    };
    let result = guard(|| {
        // Check the handle exists before doing the expensive walk.
        with_index(handle, |_| ())?;
        let walked = platform::walk(&root)?;
        let index = Index::from_paths(walked.paths);
        registry().lock().unwrap().insert(handle, index);
        Ok(())
    });
    match result {
        Ok(()) => {
            clear_error();
            FS_OK as c_int
        }
        Err((c, m)) => {
            set_error(c, &m);
            c as c_int
        }
    }
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// Search an index. Returns a caller-owned JSON array string, or null.
/// `max_results` <= 0 means no limit.
#[no_mangle]
pub extern "C" fn fs_search_json(
    handle: u64,
    query_utf8: *const c_char,
    max_results: c_int,
) -> *mut c_char {
    let query = match require_str(query_utf8, "query") {
        Ok(q) => q,
        Err((c, m)) => {
            set_error(c, &m);
            return std::ptr::null_mut();
        }
    };
    let limit = if max_results <= 0 {
        0
    } else {
        max_results as usize
    };
    match guard(|| with_index(handle, |i| i.search_json(&query, limit))) {
        Ok(json) => {
            clear_error();
            into_c_string(json)
        }
        Err((c, m)) => {
            set_error(c, &m);
            std::ptr::null_mut()
        }
    }
}

/// Walk a root and search it in one call (no index kept). Slower than an
/// indexed search; useful for one-off queries and as a correctness
/// baseline for the indexed path.
#[no_mangle]
pub extern "C" fn fs_search_direct_json(
    root_utf8: *const c_char,
    query_utf8: *const c_char,
    max_results: c_int,
) -> *mut c_char {
    let root = match require_str(root_utf8, "root") {
        Ok(r) => r,
        Err((c, m)) => {
            set_error(c, &m);
            return std::ptr::null_mut();
        }
    };
    let query = match require_str(query_utf8, "query") {
        Ok(q) => q,
        Err((c, m)) => {
            set_error(c, &m);
            return std::ptr::null_mut();
        }
    };
    let limit = if max_results <= 0 {
        0
    } else {
        max_results as usize
    };
    let result = guard(|| {
        let walked = platform::walk(&root)?;
        Ok(Index::from_paths(walked.paths).search_json(&query, limit))
    });
    match result {
        Ok(json) => {
            clear_error();
            into_c_string(json)
        }
        Err((c, m)) => {
            set_error(c, &m);
            std::ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Save an index to a file (FSIDX1 text format). FS_OK or an error code.
#[no_mangle]
pub extern "C" fn fs_index_save(handle: u64, path_utf8: *const c_char) -> c_int {
    let path = match require_str(path_utf8, "path") {
        Ok(p) => p,
        Err((c, m)) => {
            set_error(c, &m);
            return c as c_int;
        }
    };
    let result = guard(|| {
        let text = with_index(handle, |i| i.save())?;
        std::fs::write(&path, text)
            .map_err(|e| (ERR_PLATFORM, format!("cannot write '{path}': {e}")))?;
        Ok(())
    });
    match result {
        Ok(()) => {
            clear_error();
            FS_OK as c_int
        }
        Err((c, m)) => {
            set_error(c, &m);
            c as c_int
        }
    }
}

/// Load an index file; returns a new handle, or 0 on error.
#[no_mangle]
pub extern "C" fn fs_index_load(path_utf8: *const c_char) -> u64 {
    let path = match require_str(path_utf8, "path") {
        Ok(p) => p,
        Err((c, m)) => {
            set_error(c, &m);
            return 0;
        }
    };
    let result = guard(|| {
        let file = std::fs::File::open(&path)
            .map_err(|e| (ERR_NOT_FOUND, format!("cannot open '{path}': {e}")))?;
        Index::load_reader(file, crate::IndexLoadLimits::default()).map_err(|e| (ERR_PLATFORM, e))
    });
    match result {
        Ok(index) => {
            clear_error();
            new_handle(index)
        }
        Err((c, m)) => {
            set_error(c, &m);
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn cstr(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    fn take(p: *mut c_char) -> String {
        assert!(!p.is_null());
        // SAFETY: p came from this module in tests.
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string();
        fs_free_string(p);
        s
    }

    #[test]
    fn ffi_lifecycle_and_search() {
        let base = std::env::temp_dir().join(format!("fs-ffi-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("docs")).unwrap();
        fs::write(base.join("docs/quarterly report.xlsx"), "x").unwrap();
        fs::write(base.join("docs/notes.txt"), "x").unwrap();
        fs::write(base.join("photo.jpg"), "x").unwrap();

        let h = fs_index_create();
        assert!(h > 0);
        assert_eq!(fs_index_count(h), 0);
        let root = cstr(base.to_str().unwrap());
        assert_eq!(fs_index_build(h, root.as_ptr()), FS_OK as c_int);
        assert_eq!(fs_index_count(h), 4); // docs dir + 2 docs + photo

        let q = cstr("report");
        let json = take(fs_search_json(h, q.as_ptr(), 0));
        assert!(json.contains("quarterly report.xlsx"), "{json}");
        assert!(!json.contains("notes.txt"));

        // Save / load roundtrip through the FFI.
        let idx_path = base.join("saved.fsidx");
        let p = cstr(idx_path.to_str().unwrap());
        assert_eq!(fs_index_save(h, p.as_ptr()), FS_OK as c_int);
        let h2 = fs_index_load(p.as_ptr());
        assert!(h2 > 0);
        assert_eq!(fs_index_count(h2), 4);

        // Direct search agrees with indexed search.
        let json2 = take(fs_search_direct_json(root.as_ptr(), q.as_ptr(), 0));
        assert_eq!(json, json2);

        assert_eq!(fs_index_free(h), FS_OK as c_int);
        assert_eq!(fs_index_free(h2), FS_OK as c_int);
        assert_eq!(fs_index_free(999_999), ERR_NOT_FOUND as c_int);
        assert_eq!(fs_index_count(999_999), -1);
        assert!(fs_search_json(999_999, q.as_ptr(), 0).is_null());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn null_args_are_invalid() {
        let h = fs_index_create();
        assert_eq!(
            fs_index_build(h, std::ptr::null()),
            ERR_INVALID_ARG as c_int
        );
        assert!(fs_search_json(h, std::ptr::null(), 0).is_null());
        assert_eq!(fs_last_error_code(), ERR_INVALID_ARG as c_int);
        assert_eq!(fs_index_load(std::ptr::null()), 0);
        assert_eq!(fs_index_free(h), FS_OK as c_int);
    }
}
