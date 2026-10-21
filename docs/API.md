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
- Symlinks are never followed (loop safety). Unreadable directories are
  skipped during a build; the root itself must be readable.
- Performance: measured figures live in the README (Linux VM corpora).
  The index is kept sorted by (name, path); searches scan in that order
  and stop early once `max_results` hits are found.

## Loading persisted indexes

FSIDX1 records begin with the ASCII prefix `F ` or `D `, followed by the
escaped UTF-8 path. Malformed prefixes, including multibyte Unicode where
an ASCII prefix is required, return an error without byte-boundary slicing.
Valid Unicode paths and the existing escaping/search semantics are unchanged.
The FFI returns handle 0 with FS_ERR_PLATFORM for a malformed index.

Loading still reads the whole index into memory; file/entry size limits and
streaming are not provided by this fix. Release builds use panic=abort, so
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
