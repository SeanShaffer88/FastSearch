# FastSearch API — v0.1.0

Authoritative declaration: `../include/fastsearch.h`.

## Lifecycle

```
h = fs_index_create()
fs_index_build(h, "C:\\")            // walk + fill (replaces contents)
n = fs_index_count(h)                // entries, or -1 on error
json = fs_search_json(h, "*.docx", 50)
fs_free_string(json)
fs_index_save(h, "C:\\idx.fsidx")    // FSIDX1 text format
fs_index_free(h)
h2 = fs_index_load("C:\\idx.fsidx")  // 0 on error
```

## Search semantics (contractual in v0.x)

- Query contains `*` or `?` → glob against the file/directory NAME,
  case-insensitive. `*` any run, `?` exactly one character.
- Otherwise → split on whitespace; every token must be a case-insensitive
  substring of the name (AND).
- Empty query matches nothing. Results sorted by (lowercase name, path);
  `max_results <= 0` means unlimited.

## Errors

`*_json` returns NULL on failure; integer functions return FS_ERR_*;
handles are 0 on failure. Details: `fs_last_error()` (thread-local).
Bad handle = FS_ERR_NOT_FOUND. Null/non-UTF-8 argument = FS_ERR_INVALID_ARG.
Unusable root = FS_ERR_NOT_FOUND / FS_ERR_PLATFORM.

## Honest limitations (v0.1)

- The index is a snapshot: files created after `fs_index_build` are not
  found until a rebuild (USN Journal freshness is v0.2).
- Build walks the filesystem with std::fs: correct everywhere, not the
  fastest possible initial scan on huge NTFS volumes (MFT is v0.2).
- Observed symlinks and Windows reparse entries are listed but their targets
  are not traversed. Root links/reparse entries are refused; select the real
  directory. Ancestor links and concurrent replacement can race these checks;
  the walker is not a snapshot or security boundary.
- Unreadable descendants are skipped. Rust WalkResult exposes unreadable_dirs,
  unreadable_entries, and skipped_links counts. The new C build-report API
  exposes these counts too. Legacy C search/build APIs do not expose them, so
  successful results are not a completeness claim.
  Failure to enumerate the root returns an error.
- Performance: measured figures live in the README (Linux VM corpora).
  The index is kept sorted by (name, path); searches scan in that order
  and stop early once `max_results` hits are found.

## Loading persisted indexes

FSIDX1 records begin with the ASCII prefix `F ` or `D `, followed by the
escaped UTF-8 path. Malformed prefixes, including multibyte Unicode where
an ASCII prefix is required, return an error without byte-boundary slicing.
Valid Unicode paths and the existing escaping/search semantics are unchanged.
The FFI returns handle 0 with FS_ERR_PLATFORM for a malformed index.

Imported indexes are limited to 256 MiB of UTF-8 input and 2,000,000 records
by default. The file reader consumes at most the byte limit plus one detection
byte, regardless of reported file size. Excess input, excess records, invalid
UTF-8, and read failures return handle 0 with FS_ERR_PLATFORM; opening a missing
or inaccessible file retains the existing FS_ERR_NOT_FOUND mapping. No partial
index handle is returned. This adds a deliberate compatibility limit: a larger
index can be built/saved, but the C loader refuses to import it.

Rust consumers can use Index::load_with_limits or Index::load_reader with an
IndexLoadLimits value. Index::load uses the default limits. Input is still
buffered before parsing; decoded paths, lowercase names, sorting, allocator
capacity, and caller-owned text add memory beyond the input-byte limit. These
are import limits, not a total process-memory guarantee. Filesystem index builds
and saves are not capped by these limits. Release builds use panic=abort, so
catch_unwind is not a substitute for validating malformed inputs.

After `cargo build --release --locked`, run the compiled-library regression:

```sh
python tests/ffi_load_smoke.py /absolute/path/to/fast_search.dll
# Linux development build:
python3 tests/ffi_load_smoke.py target/release/libfast_search.so
```

This script loads the specified library: use only the artifact you built and
reviewed. It tests malformed and valid Unicode indexes through the real C ABI,
including release builds where panics abort. It writes synthetic index files
only inside a temporary test directory. Linux release execution is verified for
this fix; Windows target checks pass, but native Windows execution is pending.

## Filesystem build bounds

The default walker accepts up to 2,000,000 entries, 256 MiB of combined UTF-8
path text, and 256 nested directory levels below the selected root. Exceeding
any bound returns FS_ERR_PLATFORM instead of a truncated successful index.
Existing index contents are retained when fs_index_build fails. Direct search
returns NULL on the same limit failures. Rust callers may use walk_with_limits
with WalkLimits. An iterative depth-first walk bounds open directory iterators
by depth, rather than queuing every sibling directory.

These bounds do not cap total process memory or operating-system call duration;
legacy calls do not support cancellation; use the controlled build below. Skipped unreadable entries still permit a
partial successful result as described above. Directory metadata checks now
inspect Windows reparse attributes; native Windows junction behavior is pending
runtime verification for this change. Linux loop/link and exact-limit tests pass.

## Build reports and handle lifetime

`fs_index_build_report_json(handle, root_utf8)` builds/replaces the index just
like `fs_index_build`, then returns caller-owned JSON (free with
`fs_free_string`). On failure it returns NULL with thread-local error details.
Example shape:

```json
{"schema_version":1,"indexed_entries":4,"unreadable_dirs":0,"unreadable_entries":0,"skipped_links":0,"partial":false}
```

`partial` is true when any skip count is nonzero, including links whose targets
were intentionally not traversed. No per-path details or file contents are
included. A false value means no skips were observed, not a filesystem snapshot
or proof that nothing changed concurrently. Legacy APIs remain available.

Building performs filesystem work outside the registry lock. If a caller frees
that handle before the replacement commits, the build now fails with
FS_ERR_NOT_FOUND; it cannot recreate the freed handle. A deterministic barrier
test exercises that ordering. When multiple builds overlap on a live handle,
the last successful commit wins; serialize builds per handle when ordering matters.
The report describes its own completed build, not future state after another
thread modifies/frees the handle. Freeing does not cancel an in-progress walk; cancellation is explicit below.

## Progress and cancellation

`fs_index_build_controlled_report_json(handle, root, callback, context)` returns
and owns JSON like the build-report API. The optional C callback receives
`(uint64_t examined, void *context)` on the same calling thread: count 0 before
filesystem I/O, then once per discovered entry. Return 0 to continue or nonzero
to cancel. Cancel returns NULL with FS_ERR_CANCELLED (7) and keeps the previous
index. A NULL callback is equivalent to continuing. An invalid handle/argument
fails before any callback. Rust callers can use `walk_with_control` (true means
continue, false cancels).

The callback/context must remain valid until return. Do not throw or unwind
across C ABI. Keep callbacks fast and avoid recursively starting another build;
registry read/free calls are allowed because callbacks run without that lock.
For a GUI, call the build on a worker thread and use the callback to forward
progress/check a host-owned cancellation flag; do not touch thread-affine UI
controls from the worker. Calls are synchronous; the library creates no worker.

Counts are monotonic progress, not a percentage or a known total. Enumeration
and metadata OS calls can block between checkpoints; the final sort/commit is
not interruptible. Cancellation discards the new scan rather than publishing
partial entries, so a previous useful index remains searchable. Cancelling an
initial build leaves the original empty index. Legacy APIs are unchanged.
