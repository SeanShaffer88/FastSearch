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
  The index is kept sorted by (name, path); general queries scan in that order
  and stop early once `max_results` hits are found.

## Additive filtered search API

`fs_search_filtered_json(handle, query, path_contains, extension, kind, max_results)`
returns the same owned JSON array as `fs_search_json`. All string arguments must
be non-null UTF-8; use `""` to disable a filter. Invalid kind returns
`FS_ERR_INVALID_ARG` (0 = both, 1 = files, 2 = directories).

The path filter is a case-insensitive full-path substring; separators are literal
and are not normalized. Extension is the final dot-separated suffix without its
dot, case-insensitive, for files only. A name consisting only of `.txt` has no
extension. Use query `"*"` for all names; an empty query matches nothing. Filters
are ANDed before the limit. Rust callers use `SearchOptions` and
`Index::search_filtered` / `search_filtered_json`.

Prefix-only globs (`name*`) binary-seek the sorted index. The Rust API's public
`entries` must remain sorted by `(name_lower, path)` if modified by a caller.
FFI searches/save pin immutable snapshots outside the registry lock. Free does
not cancel an already pinned call; rebuild publishes a replacement snapshot.
Concurrent builds on one handle publish in completion order. A build whose
handle is freed before publication fails with `FS_ERR_NOT_FOUND`.

FSIDX1 readers now reject invalid escapes, empty/NUL paths and malformed
record prefixes. Files produced by `fs_index_save` remain compatible.
