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
use std::sync::{Arc, Mutex, OnceLock};

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

fn registry() -> &'static Mutex<HashMap<u64, Arc<Index>>> {
    static REG: OnceLock<Mutex<HashMap<u64, Arc<Index>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

fn new_handle(index: Index) -> u64 {
    let h = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
    registry().lock().unwrap().insert(h, Arc::new(index));
    h
}

fn with_index<R>(handle: u64, f: impl FnOnce(&Index) -> R) -> Result<R, (i32, String)> {
    let idx = registry()
        .lock()
        .unwrap()
        .get(&handle)
        .cloned()
        .ok_or((ERR_NOT_FOUND, format!("no such index handle: {handle}")))?;
    // Pin the immutable snapshot; release the registry lock before work.
    Ok(f(&idx))
}

// A handle freed during a build must never be resurrected.
fn replace_index(handle: u64, index: Index) -> Result<(), (i32, String)> {
    let old = {
        let mut reg = registry().lock().unwrap();
        let slot = reg
            .get_mut(&handle)
            .ok_or((ERR_NOT_FOUND, format!("no such index handle: {handle}")))?;
        std::mem::replace(slot, Arc::new(index))
    };
    drop(old); // potentially large deallocation outside the registry lock
    Ok(())
}

fn build_report_json(walked: &platform::WalkResult) -> String {
    let partial =
        walked.unreadable_dirs != 0 || walked.unreadable_entries != 0 || walked.skipped_links != 0;
    format!("{{\"schema_version\":1,\"indexed_entries\":{},\"unreadable_dirs\":{},\"unreadable_entries\":{},\"skipped_links\":{},\"partial\":{}}}",
        walked.paths.len(), walked.unreadable_dirs, walked.unreadable_entries, walked.skipped_links, partial)
}

fn build_index(handle: u64, root: &str) -> Result<String, (i32, String)> {
    build_index_controlled(handle, root, |_| true)
}

fn build_index_controlled(
    handle: u64,
    root: &str,
    control: impl FnMut(u64) -> bool,
) -> Result<String, (i32, String)> {
    with_index(handle, |_| ())?;
    let walked = platform::walk_with_control(root, platform::WalkLimits::default(), control)?;
    let report = build_report_json(&walked);
    let index = Index::from_paths(walked.paths);
    // Freeing a handle while walking must not resurrect it on completion.
    replace_index(handle, index)?;
    Ok(report)
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
    let removed = registry().lock().unwrap().remove(&handle);
    match removed {
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
/// (the build still succeeds); a bad root or exceeded walk limit fails and
/// retains the previous index. Use fs_index_build_report_json for skip counts.
#[no_mangle]
pub extern "C" fn fs_index_build(handle: u64, root_utf8: *const c_char) -> c_int {
    let root = match require_str(root_utf8, "root") {
        Ok(r) => r,
        Err((c, m)) => {
            set_error(c, &m);
            return c as c_int;
        }
    };
    let result = guard(|| build_index(handle, &root));
    match result {
        Ok(_) => {
            clear_error();
            FS_OK as c_int
        }
        Err((c, m)) => {
            set_error(c, &m);
            c as c_int
        }
    }
}

/// Build and replace an index, returning caller-owned versioned JSON with
/// partial-result counts. Free the result with fs_free_string. NULL on error.
#[no_mangle]
pub extern "C" fn fs_index_build_report_json(handle: u64, root_utf8: *const c_char) -> *mut c_char {
    let result = guard(|| {
        let root = require_str(root_utf8, "root")?;
        build_index(handle, &root)
    });
    match result {
        Ok(report) => {
            clear_error();
            into_c_string(report)
        }
        Err((code, message)) => {
            set_error(code, &message);
            std::ptr::null_mut()
        }
    }
}

/// C callbacks run synchronously on the calling thread, without registry locks.
/// Return zero to continue; nonzero requests cancellation. Callbacks must not
/// unwind; the callback and context must remain valid until this call returns.
pub type ScanControl = Option<unsafe extern "C" fn(u64, *mut std::ffi::c_void) -> c_int>;

/// Build with progress and cancellation, returning caller-owned report JSON.
///
/// # Safety
/// root_utf8 must be NULL or point to a valid NUL-terminated string. Any
/// callback must be callable with context for the duration of this call;
/// it must not unwind and must uphold the validity of data it dereferences.
#[no_mangle]
pub unsafe extern "C" fn fs_index_build_controlled_report_json(
    handle: u64,
    root_utf8: *const c_char,
    callback: ScanControl,
    context: *mut std::ffi::c_void,
) -> *mut c_char {
    let result = guard(|| {
        let root = require_str(root_utf8, "root")?;
        build_index_controlled(handle, &root, |count| match callback {
            // SAFETY: callback and context validity are the caller's contract.
            Some(callback) => unsafe { callback(count, context) == 0 },
            None => true,
        })
    });
    match result {
        Ok(report) => {
            clear_error();
            into_c_string(report)
        }
        Err((code, message)) => {
            set_error(code, &message);
            std::ptr::null_mut()
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

/// Explicit filters; empty strings disable individual filters.
/// kind: 0 = both, 1 = files, 2 = directories. Other values are invalid.
#[no_mangle]
pub extern "C" fn fs_search_filtered_json(
    handle: u64,
    query_utf8: *const c_char,
    path_contains_utf8: *const c_char,
    extension_utf8: *const c_char,
    kind: c_int,
    max_results: c_int,
) -> *mut c_char {
    let result = guard(|| {
        let query = require_str(query_utf8, "query")?;
        let path = require_str(path_contains_utf8, "path filter")?;
        let extension = require_str(extension_utf8, "extension filter")?;
        let directories = match kind {
            0 => None,
            1 => Some(false),
            2 => Some(true),
            _ => return Err((ERR_INVALID_ARG, "kind must be 0, 1, or 2".into())),
        };
        let options = crate::SearchOptions {
            path_contains: &path,
            extension: &extension,
            directories,
        };
        with_index(handle, |i| {
            i.search_filtered_json(&query, max_results.max(0) as usize, options)
        })
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

/// Compare catalogs without touching the filesystem. Blank query means all.
/// max_details <= 0 selects 200; positive values are capped at 10,000.
#[no_mangle]
pub extern "C" fn fs_index_diff_json(
    before: u64,
    after: u64,
    query_utf8: *const c_char,
    max_details: c_int,
) -> *mut c_char {
    let result = guard(|| {
        let query = require_str(query_utf8, "query")?;
        let limit = if max_details <= 0 {
            200
        } else {
            max_details as usize
        };
        // Capture both immutable snapshots together, then release the registry.
        let (a, b) = {
            let reg = registry().lock().unwrap();
            let a = reg
                .get(&before)
                .cloned()
                .ok_or((ERR_NOT_FOUND, "Before index handle not found".into()))?;
            let b = reg
                .get(&after)
                .cloned()
                .ok_or((ERR_NOT_FOUND, "After index handle not found".into()))?;
            (a, b)
        };
        crate::diff::compare(&a, &b, &query, limit)
            .map(|r| r.to_json())
            .map_err(|e| (ERR_PLATFORM, e))
    });
    match result {
        Ok(report) => {
            clear_error();
            into_c_string(report)
        }
        Err((code, message)) => {
            set_error(code, &message);
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
        struct CallbackState {
            handle: u64,
            calls: u64,
            last: u64,
            readable: bool,
        }
        unsafe extern "C" fn cancel_first(count: u64, context: *mut std::ffi::c_void) -> c_int {
            // SAFETY: the test passes a live CallbackState until synchronous return.
            let state = unsafe { &mut *context.cast::<CallbackState>() };
            state.calls += 1;
            state.last = count;
            state.readable &= fs_index_count(state.handle) == 4;
            i32::from(count >= 1)
        }
        let mut state = CallbackState {
            handle: h,
            calls: 0,
            last: 0,
            readable: true,
        };
        // SAFETY: root is a CString and state lives across this synchronous call.
        let cancelled = unsafe {
            fs_index_build_controlled_report_json(
                h,
                root.as_ptr(),
                Some(cancel_first),
                (&mut state as *mut CallbackState).cast(),
            )
        };
        assert!(cancelled.is_null());
        assert_eq!(fs_last_error_code(), crate::ERR_CANCELLED);
        assert_eq!((state.calls, state.last, state.readable), (2, 1, true));
        assert_eq!(fs_index_count(h), 4);
        // SAFETY: CString remains live; a NULL callback never dereferences context.
        let completed = take(unsafe {
            fs_index_build_controlled_report_json(h, root.as_ptr(), None, std::ptr::null_mut())
        });
        assert!(completed.contains("\"indexed_entries\":4"));

        let report = take(fs_index_build_report_json(h, root.as_ptr()));
        assert_eq!(report, "{\"schema_version\":1,\"indexed_entries\":4,\"unreadable_dirs\":0,\"unreadable_entries\":0,\"skipped_links\":0,\"partial\":false}");

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
    fn delayed_build_commit_cannot_resurrect_freed_handle() {
        use std::sync::{Arc, Barrier};
        let handle = fs_index_create();
        let ready = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let r = ready.clone();
        let go = resume.clone();
        let worker = std::thread::spawn(move || {
            with_index(handle, |_| ()).unwrap();
            let prepared = Index::from_paths(vec![("prepared".into(), false)]);
            r.wait();
            go.wait();
            replace_index(handle, prepared)
        });
        ready.wait();
        assert_eq!(fs_index_free(handle), FS_OK);
        resume.wait();
        assert_eq!(worker.join().unwrap().unwrap_err().0, ERR_NOT_FOUND);
        assert_eq!(fs_index_count(handle), -1);
    }

    #[test]
    fn report_marks_each_skipped_category_as_partial() {
        for (dirs, entries, links) in [(1, 0, 0), (0, 1, 0), (0, 0, 1)] {
            let walked = platform::WalkResult {
                unreadable_dirs: dirs,
                unreadable_entries: entries,
                skipped_links: links,
                ..Default::default()
            };
            let report = build_report_json(&walked);
            assert!(report.contains("\"partial\":true"));
            assert!(report.contains(&format!("\"unreadable_dirs\":{dirs}")));
            assert!(report.contains(&format!("\"unreadable_entries\":{entries}")));
            assert!(report.contains(&format!("\"skipped_links\":{links}")));
        }
        assert!(fs_index_build_report_json(0, std::ptr::null()).is_null());
        assert_eq!(fs_last_error_code(), ERR_INVALID_ARG);
        let root = cstr("unused");
        assert!(fs_index_build_report_json(0, root.as_ptr()).is_null());
        assert_eq!(fs_last_error_code(), ERR_NOT_FOUND);
    }

    #[test]
    fn cancelled_build_keeps_existing_index_and_callback_can_reenter() {
        let handle = new_handle(Index::from_paths(vec![("keep-me".into(), false)]));
        let result = build_index_controlled(handle, "unused", |_| {
            assert_eq!(fs_index_count(handle), 1); // no registry lock held
            false
        });
        assert_eq!(result.unwrap_err().0, crate::ERR_CANCELLED);
        assert_eq!(fs_index_count(handle), 1);
        assert_eq!(fs_index_free(handle), FS_OK);
    }

    #[test]
    fn diff_api_handles_same_snapshot_errors_and_immutable_lifetime() {
        let a = new_handle(Index::from_paths(vec![("old.dll".into(), false)]));
        let b = new_handle(Index::from_paths(vec![("new.dll".into(), false)]));
        let query = cstr("*.dll");
        let report = take(fs_index_diff_json(a, b, query.as_ptr(), 1));
        assert!(report.contains("\"added_to_index\":1"));
        assert!(report.contains("\"missing_from_index\":1"));
        assert!(report.contains("\"details_truncated\":true"));
        let same = take(fs_index_diff_json(a, a, cstr("").as_ptr(), 0));
        assert!(same.contains("\"unchanged\":1"));
        assert!(fs_index_diff_json(0, b, query.as_ptr(), 10).is_null());
        assert_eq!(fs_last_error_code(), ERR_NOT_FOUND);
        assert!(fs_index_diff_json(a, b, std::ptr::null(), 10).is_null());
        assert_eq!(fs_last_error_code(), ERR_INVALID_ARG);
        with_index(a, |snapshot| {
            assert_eq!(fs_index_free(a), FS_OK);
            assert_eq!(snapshot.entries[0].path, "old.dll");
        })
        .unwrap();
        assert_eq!(fs_index_count(a), -1);
        assert_eq!(fs_index_free(b), FS_OK);
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

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn snapshot_work_does_not_lock_out_other_handles() {
        let h = fs_index_create();
        let other = fs_index_create();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            with_index(h, |_| {
                started_tx.send(()).unwrap();
                finish_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            })
            .unwrap()
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || done_tx.send(fs_index_count(other)).unwrap());
        let result = done_rx.recv_timeout(Duration::from_secs(2));
        finish_tx.send(()).unwrap();
        worker.join().unwrap();
        reader.join().unwrap();
        assert_eq!(result.unwrap(), 0);
        assert_eq!(fs_index_free(h), FS_OK);
        assert_eq!(fs_index_free(other), FS_OK);
    }

    #[test]
    fn completed_build_cannot_resurrect_freed_handle() {
        let h = fs_index_create();
        assert_eq!(fs_index_free(h), FS_OK);
        assert_eq!(
            replace_index(h, Index::default()).unwrap_err().0,
            ERR_NOT_FOUND
        );
        assert_eq!(fs_index_count(h), -1);
    }

    #[test]
    fn filtered_ffi_validates_and_filters() {
        let h = new_handle(Index::from_paths(vec![
            ("/docs/a.TXT".into(), false),
            ("/docs/b.rs".into(), false),
        ]));
        let q = CString::new("*").unwrap();
        let path = CString::new("DOCS").unwrap();
        let ext = CString::new("txt").unwrap();
        let p = fs_search_filtered_json(h, q.as_ptr(), path.as_ptr(), ext.as_ptr(), 1, 1);
        assert!(!p.is_null());
        let text = unsafe { CStr::from_ptr(p) }.to_str().unwrap();
        assert!(text.contains("a.TXT") && !text.contains("b.rs"));
        fs_free_string(p);
        assert!(
            fs_search_filtered_json(h, q.as_ptr(), path.as_ptr(), ext.as_ptr(), 3, 0).is_null()
        );
        assert_eq!(fs_last_error_code(), ERR_INVALID_ARG);
        assert!(
            fs_search_filtered_json(h, q.as_ptr(), std::ptr::null(), ext.as_ptr(), 0, 0).is_null()
        );
        assert_eq!(fs_last_error_code(), ERR_INVALID_ARG);
        assert_eq!(fs_index_free(h), FS_OK);
    }
}
